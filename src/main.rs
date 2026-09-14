use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use sapiens_agent::browser::{BrowserDriver, PlaywrightCliBrowser};
use sapiens_agent::channels::{self, ChannelConfig};
use sapiens_agent::computer::{ComputerUseAdapter, DesktopAction, WindowsComputerUse};
use sapiens_agent::config::{
    self, AppConfig, ApprovalConfig, McpServerConfig, ProviderConfig, ScheduleConfig,
};
use sapiens_agent::mcp::{McpServer, McpTool};
use sapiens_agent::plugins;
use sapiens_agent::policy::Policy;
use sapiens_agent::providers::{self, ProviderRegistry};
use sapiens_agent::sessions::SessionStore;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const BANNER: &str = r#"
  ███████╗ █████╗ ██████╗ ██╗███████╗███╗   ██╗███████╗
  ██╔════╝██╔══██╗██╔══██╗██║██╔════╝████╗  ██║██╔════╝
  ███████╗███████║██████╔╝██║█████╗  ██╔██╗ ██║███████╗
  ╚════██║██╔══██║██╔═══╝ ██║██╔══╝  ██║╚██╗██║╚════██║
  ███████║██║  ██║██║     ██║███████╗██║ ╚████║███████║
  ╚══════╝╚═╝  ╚═╝╚═╝     ╚═╝╚══════╝╚═╝  ╚═══╝╚══════╝

                         S A P I E N S   A G E N T
"#;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "sapiens-agent",
    bin_name = "sapiens-agent",
    disable_help_subcommand = true,
    version,
    about = "Seu agente pessoal local-first"
)]
struct Cli {
    #[arg(long, global = true)]
    json: bool,
    #[arg(long, global = true)]
    yes: bool,
    /// Abre a WebUI no navegador somente quando solicitado explicitamente.
    #[arg(long, global = true)]
    open_browser: bool,
    /// Mantido por compatibilidade; a WebUI nunca abre por padrão.
    #[arg(long, global = true)]
    no_browser: bool,
    #[arg(long, global = true)]
    verbose: bool,
    #[arg(long, global = true)]
    dry_run: bool,
    #[arg(long, global = true)]
    port: Option<u16>,
    #[arg(long, global = true)]
    workspace: Option<PathBuf>,
    #[arg(long, global = true)]
    config_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug, Clone)]
enum Commands {
    Init,
    Setup,
    Configure,
    Config {
        #[command(subcommand)]
        command: Option<ConfigCommands>,
    },
    Start,
    Restart,
    Serve {
        #[arg(long, default_value = "127.0.0.1:8787")]
        bind: String,
    },
    Chat {
        #[arg(long)]
        provider: Option<String>,
        #[arg(long, default_value = "general")]
        task: String,
        /// Anexa uma ou mais imagens locais à próxima mensagem.
        #[arg(long = "image", value_name = "PATH")]
        image: Vec<PathBuf>,
        prompt: Option<String>,
    },
    Shell {
        #[arg(long)]
        command: String,
        #[arg(long, default_value_t = 60)]
        timeout: u64,
    },
    Status,
    Doctor,
    Schedule {
        #[command(subcommand)]
        command: ScheduleCommands,
    },
    Channel {
        #[command(subcommand)]
        command: ChannelCommands,
    },
    Skills {
        #[command(subcommand)]
        command: SkillsCommands,
    },
    Logs,
    Receipts,
    Version,
    Help,
    Stop,
    Provider {
        #[command(subcommand)]
        command: ProviderCommands,
    },
    Route {
        #[command(subcommand)]
        command: RouteCommands,
    },
    Memory {
        #[command(subcommand)]
        command: MemoryCommands,
    },
    Identity {
        #[command(subcommand)]
        command: IdentityCommands,
    },
    Session {
        #[command(subcommand)]
        command: SessionCommands,
    },
    Browser {
        #[command(subcommand)]
        command: BrowserCommands,
    },
    Computer {
        #[command(subcommand)]
        command: ComputerCommands,
    },
    Mcp {
        #[command(subcommand)]
        command: McpCommands,
    },
    Plugin {
        #[command(subcommand)]
        command: PluginCommands,
    },
    Tools {
        #[command(subcommand)]
        command: ToolCommands,
    },
    Resources {
        #[command(subcommand)]
        command: ResourceCommands,
    },
    Approval {
        #[command(subcommand)]
        command: ApprovalCommands,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum ConfigCommands {
    Get { key: String },
    Set { key: String, value: String },
    Show,
    Export { path: PathBuf },
    Import { path: PathBuf },
    Restore { path: Option<PathBuf> },
}

#[derive(Subcommand, Debug, Clone)]
enum ApprovalCommands {
    List,
    Grant {
        scope: String,
        #[arg(long, default_value = "external_write")]
        risk: String,
        #[arg(long, default_value_t = 3600)]
        expires_secs: u64,
    },
    Revoke {
        scope: String,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum ToolCommands {
    /// Lista ferramentas tipadas e o estado da feature correspondente.
    List {
        #[arg(long)]
        enabled_only: bool,
    },
    /// Descobre uma ferramenta sob demanda e aplica a policy de risco.
    Discover { name: String },
}

#[derive(Subcommand, Debug, Clone)]
enum ScheduleCommands {
    List,
    Add {
        #[arg(long)]
        every: String,
        #[arg(long)]
        task: String,
        #[arg(long)]
        webhook: Option<String>,
        #[arg(long)]
        channel: Option<String>,
        #[arg(long)]
        recipient: Option<String>,
    },
    Once {
        #[arg(long)]
        at: String,
        #[arg(long)]
        task: String,
        #[arg(long)]
        webhook: Option<String>,
        #[arg(long)]
        channel: Option<String>,
        #[arg(long)]
        recipient: Option<String>,
    },
    Cron {
        #[arg(long)]
        expression: String,
        #[arg(long)]
        task: String,
        #[arg(long)]
        webhook: Option<String>,
        #[arg(long)]
        channel: Option<String>,
        #[arg(long)]
        recipient: Option<String>,
    },
    Remove {
        id: String,
    },
    Pause {
        id: String,
    },
    Cancel {
        id: String,
    },
    Resume {
        id: String,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum ProviderCommands {
    List,
    Catalog,
    Add {
        kind: String,
        #[arg(long)]
        alias: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long)]
        api_key_env: Option<String>,
        #[arg(long, default_value = "")]
        model: Option<String>,
        #[arg(long)]
        protocol: Option<String>,
    },
    Configure {
        alias: String,
    },
    Remove {
        alias: String,
    },
    Test {
        alias: String,
    },
    Models {
        alias: String,
    },
    Use {
        alias: String,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum RouteCommands {
    Set { task: String, alias: String },
    Fallback { aliases: String },
}

#[derive(Subcommand, Debug, Clone)]
enum MemoryCommands {
    Search { query: String },
    List,
    Export { path: Option<PathBuf> },
    Delete { session: String },
    Clear,
}

#[derive(Subcommand, Debug, Clone)]
enum IdentityCommands {
    Init,
    Show,
    Path,
}

#[derive(Subcommand, Debug, Clone)]
enum SessionCommands {
    List,
    Cleanup {
        #[arg(long)]
        older_than: u64,
    },
    Rotate {
        id: String,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum ChannelCommands {
    List,
    Add {
        kind: String,
        #[arg(long)]
        name: Option<String>,
    },
    Configure {
        name: String,
    },
    Test {
        name: String,
    },
    Send {
        name: String,
        recipient: String,
        message: String,
    },
    SendMedia {
        name: String,
        recipient: String,
        path: PathBuf,
        #[arg(long)]
        caption: Option<String>,
    },
    Start,
}

#[derive(Subcommand, Debug, Clone)]
enum SkillsCommands {
    List,
    Enable {
        name: String,
    },
    Disable {
        name: String,
    },
    Validate {
        name: Option<String>,
    },
    Create {
        name: String,
        #[arg(long, default_value = "Reusable local workflow")]
        purpose: String,
    },
    Suggest,
    Rollback {
        name: String,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum ResourceCommands {
    Status,
    Profile { name: String },
}

#[derive(Subcommand, Debug, Clone)]
enum BrowserCommands {
    Open {
        url: String,
        #[arg(long, default_value = "default")]
        session: String,
        /// Persiste cookies e armazenamento somente nesta sessão Playwright.
        #[arg(long)]
        persistent: bool,
    },
    Goto {
        url: String,
        #[arg(long, default_value = "default")]
        session: String,
    },
    Snapshot {
        #[arg(long, default_value = "default")]
        session: String,
    },
    Click {
        target: String,
        #[arg(long, default_value = "default")]
        session: String,
    },
    DoubleClick {
        target: String,
        #[arg(long, default_value = "default")]
        session: String,
    },
    Drag {
        start_target: String,
        end_target: String,
        #[arg(long, default_value = "default")]
        session: String,
    },
    Fill {
        target: String,
        text: String,
        #[arg(long, default_value = "default")]
        session: String,
    },
    Hover {
        target: String,
        #[arg(long, default_value = "default")]
        session: String,
    },
    Press {
        target: String,
        key: String,
        #[arg(long, default_value = "default")]
        session: String,
    },
    Scroll {
        dx: i32,
        dy: i32,
        #[arg(long, default_value = "default")]
        session: String,
    },
    GoBack {
        #[arg(long, default_value = "default")]
        session: String,
    },
    GoForward {
        #[arg(long, default_value = "default")]
        session: String,
    },
    Reload {
        #[arg(long, default_value = "default")]
        session: String,
    },
    Check {
        target: String,
        #[arg(long, default_value = "default")]
        session: String,
    },
    Uncheck {
        target: String,
        #[arg(long, default_value = "default")]
        session: String,
    },
    DialogAccept {
        prompt: Option<String>,
        #[arg(long, default_value = "default")]
        session: String,
    },
    DialogDismiss {
        #[arg(long, default_value = "default")]
        session: String,
    },
    Resize {
        width: u32,
        height: u32,
        #[arg(long, default_value = "default")]
        session: String,
    },
    Select {
        target: String,
        value: String,
        #[arg(long, default_value = "default")]
        session: String,
    },
    Wait {
        millis: u64,
        #[arg(long, default_value = "default")]
        session: String,
    },
    Extract {
        target: Option<String>,
        #[arg(long, default_value = "default")]
        session: String,
    },
    Upload {
        files: Vec<String>,
        #[arg(long, default_value = "default")]
        session: String,
    },
    Download {
        target: String,
        output: PathBuf,
        #[arg(long, default_value = "default")]
        session: String,
    },
    TraceStart {
        #[arg(long, default_value = "default")]
        session: String,
    },
    TraceStop {
        #[arg(long, default_value = "default")]
        session: String,
    },
    StateSave {
        output: PathBuf,
        #[arg(long, default_value = "default")]
        session: String,
    },
    StateLoad {
        input: PathBuf,
        #[arg(long, default_value = "default")]
        session: String,
    },
    Screenshot {
        target: Option<String>,
        #[arg(long, default_value = "default")]
        session: String,
    },
    Tabs {
        #[arg(long, default_value = "default")]
        session: String,
    },
    TabNew {
        url: String,
        #[arg(long, default_value = "default")]
        session: String,
    },
    TabSelect {
        index: usize,
        #[arg(long, default_value = "default")]
        session: String,
    },
    TabClose {
        index: usize,
        #[arg(long, default_value = "default")]
        session: String,
    },
    Close {
        #[arg(long, default_value = "default")]
        session: String,
    },
    ProfileRevoke {
        /// Revoga e apaga os dados persistentes da sessão Playwright.
        #[arg(long, default_value = "default")]
        session: String,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum ComputerCommands {
    Plan {
        task: String,
        #[arg(long, default_value = "computer-plan.json")]
        output: PathBuf,
    },
    Auto {
        task: String,
        #[arg(long, default_value = "computer-auto-artifacts")]
        output_dir: PathBuf,
    },
    Run {
        actions: PathBuf,
        #[arg(long, default_value = "computer-run-artifacts")]
        output_dir: PathBuf,
    },
    Screenshot {
        #[arg(long, default_value = "sapiens-computer-screenshot.png")]
        output: PathBuf,
    },
    Click {
        x: u32,
        y: u32,
    },
    Type {
        text: String,
    },
    Key {
        key: String,
    },
    Wait {
        millis: u64,
    },
    EmergencyStop,
    ResetStop,
}

#[derive(Subcommand, Debug, Clone)]
enum McpCommands {
    Servers,
    Health {
        #[arg(long)]
        server: String,
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },
    Diagnose {
        #[arg(long)]
        server: String,
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },
    Add {
        name: String,
        #[arg(long)]
        command: Option<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long, default_value = "stdio")]
        transport: String,
        #[arg(long, default_value = "")]
        credential_env: String,
        #[arg(long)]
        allow: String,
        #[arg(long, default_value = "1")]
        version: String,
    },
    Remove {
        name: String,
    },
    List {
        #[arg(long)]
        command: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long, default_value = "")]
        allow: String,
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },
    Call {
        #[arg(long)]
        command: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        tool: String,
        #[arg(long, default_value = "{}")]
        arguments: String,
        #[arg(long, default_value = "")]
        allow: String,
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum PluginCommands {
    Verify {
        manifest: PathBuf,
    },
    List {
        #[arg(long)]
        directory: Option<PathBuf>,
    },
    Install {
        manifest: PathBuf,
        #[arg(long)]
        destination: Option<PathBuf>,
    },
    Update {
        manifest: PathBuf,
        #[arg(long)]
        destination: Option<PathBuf>,
    },
    Remove {
        path: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    sapiens_agent::observability::init();
    let cli = Cli::parse();
    let path = config_path(cli.config_dir.as_deref())?;
    let mut config = config::load(&path)?;

    let command = cli.command.clone();
    match command {
        None => {
            print_banner();
            if std::io::stdin().is_terminal() {
                if !path.exists() {
                    run_setup(&path, &mut config, &cli, true, false)?;
                }
                run_interactive_menu(&path, &mut config, &cli).await?;
            } else {
                if !path.exists() {
                    initialize(&path, &mut config, cli.workspace.as_deref())?;
                }
                let bind = bind_for(&config, cli.port)?;
                run_server(config, &path, bind, cli.open_browser && !cli.no_browser).await?;
            }
        }
        Some(Commands::Init) => {
            initialize(&path, &mut config, cli.workspace.as_deref())?;
            message(
                &cli,
                "Estado inicializado com segurança.",
                serde_json::json!({"config": path, "workspace": config.security.workspace}),
            );
        }
        Some(Commands::Setup) => {
            print_banner();
            run_setup(&path, &mut config, &cli, true, false)?;
        }
        Some(Commands::Configure) => run_setup(&path, &mut config, &cli, false, false)?,
        Some(Commands::Config { command }) => match command {
            None => run_setup(&path, &mut config, &cli, false, false)?,
            Some(ConfigCommands::Show) => print_redacted(&config)?,
            Some(ConfigCommands::Get { key }) => {
                let value = config::get_value(&config, &key)?;
                message(
                    &cli,
                    &value,
                    serde_json::json!({"key": key, "value": value}),
                );
            }
            Some(ConfigCommands::Set { key, value }) => {
                config::set_value(&mut config, &key, &value)?;
                config::save(&path, &config)?;
                message(
                    &cli,
                    "Configuração atualizada.",
                    serde_json::json!({"key": key, "value": value}),
                );
            }
            Some(ConfigCommands::Export { path: destination }) => {
                export_config(&config, &destination, &cli)?;
            }
            Some(ConfigCommands::Import { path: source }) => {
                if !source.exists() {
                    anyhow::bail!(
                        "arquivo de configuração não encontrado: {}",
                        source.display()
                    );
                }
                let imported = config::load(&source)?;
                config::save(&path, &imported)?;
                message(
                    &cli,
                    "Configuração importada.",
                    serde_json::json!({"source": source, "destination": path}),
                );
            }
            Some(ConfigCommands::Restore { path: backup }) => {
                let source = backup.unwrap_or_else(|| path.with_extension("toml.bak"));
                if !source.exists() {
                    anyhow::bail!(
                        "backup de configuração não encontrado: {}",
                        source.display()
                    );
                }
                let restored = config::load(&source)?;
                config::save(&path, &restored)?;
                message(
                    &cli,
                    "Backup restaurado.",
                    serde_json::json!({"source": source, "destination": path}),
                );
            }
        },
        Some(Commands::Start) => {
            print_banner();
            ensure_initialized(&path, &mut config, &cli)?;
            let bind = bind_for(&config, cli.port)?;
            run_server(config, &path, bind, cli.open_browser && !cli.no_browser).await?;
        }
        Some(Commands::Restart) => {
            print_banner();
            ensure_initialized(&path, &mut config, &cli)?;
            stop_server(&path, &cli)?;
            tokio::time::sleep(Duration::from_millis(150)).await;
            let bind = bind_for(&config, cli.port)?;
            run_server(config, &path, bind, cli.open_browser && !cli.no_browser).await?;
        }
        Some(Commands::Serve { bind }) => {
            print_banner();
            ensure_initialized(&path, &mut config, &cli)?;
            run_server(config, &path, bind, cli.open_browser && !cli.no_browser).await?;
        }
        Some(Commands::Chat {
            provider,
            task,
            image,
            prompt,
        }) => run_chat(config, &path, provider, task, image, prompt).await?,
        Some(Commands::Shell { command, timeout }) => {
            run_shell(&config, &path, command, timeout, &cli).await?
        }
        Some(Commands::Status) => print_status(&config, &path, &cli)?,
        Some(Commands::Doctor) => doctor(&config, &path, &cli)?,
        Some(Commands::Schedule { command }) => {
            schedule_command(&mut config, &path, command, &cli)?
        }
        Some(Commands::Channel { command }) => {
            channel_command(&mut config, &path, command, &cli).await?
        }
        Some(Commands::Skills { command }) => skills_command(&mut config, &path, command, &cli)?,
        Some(Commands::Logs) => logs_command(&path, &cli)?,
        Some(Commands::Version) => println!("Sapiens Agent {}", env!("CARGO_PKG_VERSION")),
        Some(Commands::Help) => {
            let mut command = Cli::command();
            command.print_help()?;
            println!();
        }
        Some(Commands::Stop) => stop_server(&path, &cli)?,
        Some(Commands::Provider { command }) => {
            provider_command(&mut config, &path, command, &cli).await?
        }
        Some(Commands::Route { command }) => route_command(&mut config, &path, command)?,
        Some(Commands::Memory { command }) => memory_command(&config, &path, command, &cli)?,
        Some(Commands::Identity { command }) => identity_command(&path, command, &cli)?,
        Some(Commands::Session { command }) => session_command(&path, command, &cli)?,
        Some(Commands::Browser { command }) => browser_command(&config, &path, command, &cli)?,
        Some(Commands::Computer { command }) => {
            computer_command(&config, &path, command, &cli).await?
        }
        Some(Commands::Mcp { command }) => mcp_command(&mut config, &path, command, &cli)?,
        Some(Commands::Plugin { command }) => plugin_command(&config, &path, command, &cli)?,
        Some(Commands::Tools { command }) => tools_command(&config, &path, command, &cli)?,
        Some(Commands::Resources { command }) => {
            resources_command(&mut config, &path, command, &cli)?
        }
        Some(Commands::Receipts) => receipts_command(&path, &cli)?,
        Some(Commands::Approval { command }) => {
            approval_command(&mut config, &path, command, &cli)?
        }
    }
    Ok(())
}

fn config_path(dir: Option<&Path>) -> Result<PathBuf> {
    Ok(dir
        .map(PathBuf::from)
        .unwrap_or(config::config_dir()?)
        .join("config.toml"))
}

fn print_banner() {
    println!("{BANNER}");
}

async fn run_interactive_menu(path: &Path, config: &mut AppConfig, cli: &Cli) -> Result<()> {
    loop {
        println!();
        println!("  [1] Configurar o agente");
        println!("  [2] Provider e API");
        println!("  [3] Canais");
        println!("  [4] Gateway e interface");
        println!("  [5] Segurança e recursos");
        println!("  [6] Memória, identidade e workspace");
        println!("  [7] Skills, ferramentas e automações");
        println!("  [8] Iniciar, status, ajuda e sair");
        println!("  [0] Voltar/encerrar");
        print!("  Escolha uma opção: ");
        std::io::stdout().flush()?;
        let mut choice = String::new();
        std::io::stdin().read_line(&mut choice)?;
        match choice.trim() {
            "1" => {
                run_setup(path, config, cli, true, true)?;
                println!("  Configuração concluída. Escolha [2] para iniciar.");
            }
            "2" => run_provider_menu(path, config, cli).await?,
            "3" => run_channel_menu(path, config, cli).await?,
            "4" => run_gateway_menu(path, config, cli)?,
            "5" => run_security_menu(path, config, cli)?,
            "6" => run_memory_menu(path, config, cli)?,
            "7" => run_skills_tools_menu(path, config, cli)?,
            "8" => {
                if run_control_menu(path, config, cli).await? {
                    break;
                }
            }
            "0" | "q" | "Q" => {
                println!("  Sapiens Agent encerrado.");
                break;
            }
            _ => println!("  Opção inválida. Escolha um número do menu."),
        }
    }
    Ok(())
}

fn menu_choice(label: &str) -> Result<String> {
    print!("  {label}: ");
    std::io::stdout().flush()?;
    let mut choice = String::new();
    std::io::stdin().read_line(&mut choice)?;
    Ok(choice.trim().to_string())
}

fn save_menu_config(path: &Path, config: &AppConfig) -> Result<()> {
    config::save(path, config)?;
    println!("  Configuração salva com validação.");
    Ok(())
}

/// Select a value from a numbered catalog.  This is intentionally the default
/// interaction for the terminal UX; free-form prompts remain reserved for the
/// explicitly labelled advanced paths.
fn choose_catalog_value(
    label: &str,
    options: &[(String, String)],
    current: &str,
    automatic: bool,
) -> Result<Option<String>> {
    if options.is_empty() {
        return Ok(None);
    }
    let default_index = options
        .iter()
        .position(|(_, value)| value == current)
        .unwrap_or(0);
    if automatic || !std::io::stdin().is_terminal() {
        return Ok(Some(options[default_index].1.clone()));
    }

    println!("\n  {label}");
    for (index, (display, value)) in options.iter().enumerate() {
        let marker = if index == default_index { "*" } else { " " };
        println!("  [{:>2}] {marker} {display} ({value})", index + 1);
    }
    println!("  [0] Voltar/cancelar");
    loop {
        print!("  Escolha [{}]: ", default_index + 1);
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let input = input.trim();
        if input.is_empty() {
            return Ok(Some(options[default_index].1.clone()));
        }
        if matches!(input, "0" | "q" | "Q" | "esc" | "Esc") {
            return Ok(None);
        }
        if let Ok(number) = input.parse::<usize>()
            && let Some((_, value)) = options.get(number.saturating_sub(1))
        {
            return Ok(Some(value.clone()));
        }
        println!("  Opção inválida. Escolha um número do catálogo.");
    }
}

fn static_catalog(options: &[(&str, &str)]) -> Vec<(String, String)> {
    options
        .iter()
        .map(|(display, value)| ((*display).to_string(), (*value).to_string()))
        .collect()
}

fn apply_resource_profile(config: &mut AppConfig, profile: &str) -> Result<()> {
    let (gpu, memory, cpu, concurrent) = match profile {
        "economy" => (35, 1024, 50, 1),
        "balanced" => (70, 2048, 80, 2),
        "performance" => (95, 8192, 100, 4),
        "custom" => return Ok(()),
        _ => anyhow::bail!("perfil de recursos inválido: {profile}"),
    };
    config.resources.profile = profile.to_string();
    config.resources.max_gpu_percent = gpu;
    config.resources.max_memory_mb = memory;
    config.resources.max_cpu_percent = cpu;
    config.resources.max_concurrent = concurrent;
    Ok(())
}

fn apply_capability_profile(config: &mut AppConfig, profile: &str) -> Result<()> {
    match profile {
        "essential" => {
            config.features.browser = false;
            config.features.computer_use = false;
            config.features.shell = false;
            config.features.mcp = false;
            config.features.channels = false;
            config.features.memory = false;
            config.features.scheduler = true;
            config.features.audio = false;
        }
        "automation" => {
            config.features.browser = true;
            config.features.computer_use = false;
            config.features.shell = true;
            config.features.mcp = false;
            config.features.channels = false;
            config.features.memory = true;
            config.features.scheduler = true;
            config.features.audio = false;
        }
        "complete" => {
            config.features.browser = true;
            config.features.computer_use = true;
            config.features.shell = true;
            config.features.mcp = true;
            config.features.channels = true;
            config.features.memory = true;
            config.features.scheduler = true;
            config.features.audio = false;
        }
        "custom" => {}
        _ => anyhow::bail!("perfil de capacidades inválido: {profile}"),
    }
    config.audio.enabled = config.features.audio;
    Ok(())
}

fn provider_catalog_options(config: &AppConfig) -> Vec<(String, String)> {
    let mut options = Vec::new();
    for provider in &config.providers {
        options.push((
            format!(
                "Usar provider existente: {} ({})",
                provider.alias, provider.kind
            ),
            format!("alias:{}", provider.alias),
        ));
    }
    for spec in providers::catalog() {
        options.push((
            format!("{} — {}", spec.label, spec.adapter),
            format!("kind:{}", spec.id),
        ));
    }
    options.push(("Configurar provider depois".into(), "none".into()));
    options.push(("Personalizado/Avançado".into(), "custom".into()));
    options
}

fn configured_provider_options(config: &AppConfig) -> Vec<(String, String)> {
    config
        .providers
        .iter()
        .map(|provider| {
            (
                format!(
                    "{} — {} / {}",
                    provider.alias, provider.kind, provider.model
                ),
                provider.alias.clone(),
            )
        })
        .collect()
}

fn channel_catalog_options() -> Vec<(String, String)> {
    let mut options = channels::catalog()
        .iter()
        .map(|spec| {
            (
                format!("{} — {} [{}]", spec.label, spec.adapter, spec.capabilities),
                spec.id.to_string(),
            )
        })
        .collect::<Vec<_>>();
    options.push(("Personalizado/Avançado".into(), "custom".into()));
    options
}

fn edit_menu_value(
    path: &Path,
    config: &mut AppConfig,
    key: &str,
    label: &str,
    default: &str,
) -> Result<()> {
    let value = setup_ask(label, default, false, true)?;
    config::set_value(config, key, &value)?;
    save_menu_config(path, config)
}

fn edit_menu_bool(
    path: &Path,
    config: &mut AppConfig,
    key: &str,
    label: &str,
    default: bool,
) -> Result<()> {
    let value = setup_ask_bool(label, default, false, true)?;
    config::set_value(config, key, &value.to_string())?;
    save_menu_config(path, config)
}

async fn run_provider_menu(path: &Path, config: &mut AppConfig, cli: &Cli) -> Result<()> {
    loop {
        println!("\n  Provider e API");
        println!("  [1] Listar providers");
        println!("  [2] Ver catálogo e protocolos");
        println!("  [3] Adicionar provider");
        println!("  [4] Configurar provider existente (avançado)");
        println!("  [5] Remover provider");
        println!("  [6] Testar conexão");
        println!("  [7] Listar modelos");
        println!("  [8] Provider ativo e fallback");
        println!("  [0] Voltar");
        match menu_choice("Escolha uma opção")?.as_str() {
            "1" => provider_command(config, path, ProviderCommands::List, cli).await?,
            "2" => provider_command(config, path, ProviderCommands::Catalog, cli).await?,
            "3" => {
                let options = providers::catalog()
                    .iter()
                    .map(|spec| {
                        (
                            format!("{} — {}", spec.label, spec.adapter),
                            spec.id.to_string(),
                        )
                    })
                    .chain(std::iter::once((
                        "Personalizado/Avançado".into(),
                        "custom".into(),
                    )))
                    .collect::<Vec<_>>();
                let Some(kind) =
                    choose_catalog_value("Escolha o provider", &options, "openai", false)?
                else {
                    continue;
                };
                let spec = providers::find_spec(&kind);
                let alias = if kind == "custom" {
                    setup_ask("Alias personalizado", "principal", false, true)?
                } else {
                    let mut candidate = kind.clone();
                    let mut suffix = 2;
                    while config.providers.iter().any(|p| p.alias == candidate) {
                        candidate = format!("{kind}-{suffix}");
                        suffix += 1;
                    }
                    candidate
                };
                let (base_url, model, api_key_env, protocol) = if kind == "custom" {
                    (
                        setup_ask("URL base personalizada", "", false, true)?,
                        setup_ask("Modelo personalizado", "", false, true)?,
                        setup_ask(
                            "Variável da credencial (nunca a chave)",
                            "SAPIENS_API_KEY",
                            false,
                            true,
                        )?,
                        setup_ask("Protocolo avançado", "chat_completions", false, true)?,
                    )
                } else {
                    (
                        String::new(),
                        String::new(),
                        spec.filter(|item| item.credential_hint != "nenhuma")
                            .map(|item| item.credential_hint.to_string())
                            .unwrap_or_default(),
                        spec.and_then(|item| item.protocols.split(',').next())
                            .unwrap_or("chat_completions")
                            .to_string(),
                    )
                };
                config.providers.push(ProviderConfig {
                    alias: alias.clone(),
                    kind,
                    base_url,
                    model,
                    api_key_env,
                    protocol,
                    ..Default::default()
                });
                config.active_provider = Some(alias.clone());
                save_menu_config(path, config)?;
                println!("  Provider {alias} selecionado como ativo.");
            }
            "4" => {
                let options = configured_provider_options(config);
                let Some(alias) = choose_catalog_value(
                    "Provider para configurar (avançado)",
                    &options,
                    config.active_provider.as_deref().unwrap_or(""),
                    false,
                )?
                else {
                    continue;
                };
                provider_command(config, path, ProviderCommands::Configure { alias }, cli).await?;
            }
            "5" => {
                let options = configured_provider_options(config);
                let Some(alias) = choose_catalog_value(
                    "Provider para remover",
                    &options,
                    config.active_provider.as_deref().unwrap_or(""),
                    false,
                )?
                else {
                    continue;
                };
                provider_command(config, path, ProviderCommands::Remove { alias }, cli).await?;
            }
            "6" => {
                let options = configured_provider_options(config);
                let Some(alias) = choose_catalog_value(
                    "Provider para testar",
                    &options,
                    config.active_provider.as_deref().unwrap_or(""),
                    false,
                )?
                else {
                    continue;
                };
                provider_command(config, path, ProviderCommands::Test { alias }, cli).await?;
            }
            "7" => {
                let options = configured_provider_options(config);
                let Some(alias) = choose_catalog_value(
                    "Provider para listar modelos",
                    &options,
                    config.active_provider.as_deref().unwrap_or(""),
                    false,
                )?
                else {
                    continue;
                };
                provider_command(config, path, ProviderCommands::Models { alias }, cli).await?;
            }
            "8" => {
                let options = configured_provider_options(config);
                let Some(alias) = choose_catalog_value(
                    "Provider ativo",
                    &options,
                    config.active_provider.as_deref().unwrap_or(""),
                    false,
                )?
                else {
                    continue;
                };
                config::find_provider(config, &alias)?;
                config.active_provider = Some(alias.clone());
                save_menu_config(path, config)?;
                let fallback_display = if config.fallback.is_empty() {
                    "nenhum".to_string()
                } else {
                    config.fallback.join(", ")
                };
                println!("  Fallback atual preservado: {fallback_display}");
                println!(
                    "  Para editar a cadeia manualmente, use [4] Configurar provider existente (avançado)."
                );
            }
            "0" | "q" | "Q" => break,
            _ => println!("  Opção inválida."),
        }
    }
    Ok(())
}

async fn run_channel_menu(path: &Path, config: &mut AppConfig, cli: &Cli) -> Result<()> {
    loop {
        println!("\n  Canais");
        println!("  [1] Listar catálogo e status");
        println!("  [2] Adicionar canal");
        println!("  [3] Configurar canal");
        println!("  [4] Testar canal");
        println!("  [5] Iniciar canais habilitados");
        println!("  [6] Habilitar/desabilitar canal");
        println!("  [0] Voltar");
        match menu_choice("Escolha uma opção")?.as_str() {
            "1" => channel_command(config, path, ChannelCommands::List, cli).await?,
            "2" => {
                let Some(kind) = choose_catalog_value(
                    "Escolha o canal",
                    &channel_catalog_options(),
                    "telegram",
                    false,
                )?
                else {
                    continue;
                };
                if kind == "custom" {
                    let name = setup_ask("Nome do canal personalizado", "canal", false, true)?;
                    let custom_kind =
                        setup_ask("Tipo do canal personalizado", "webhooks", false, true)?;
                    channel_command(
                        config,
                        path,
                        ChannelCommands::Add {
                            kind: custom_kind,
                            name: Some(name),
                        },
                        cli,
                    )
                    .await?;
                } else {
                    let mut name = kind.clone();
                    let mut suffix = 2;
                    while config.channels.iter().any(|item| item.name == name) {
                        name = format!("{kind}-{suffix}");
                        suffix += 1;
                    }
                    let spec = channels::find_spec(&kind)?;
                    let credential_env = if spec.credential_hint.starts_with("SAPIENS_") {
                        spec.credential_hint.to_string()
                    } else {
                        String::new()
                    };
                    config.channels.push(ChannelConfig {
                        name: name.clone(),
                        kind,
                        enabled: false,
                        credential_env,
                        allowlist: vec![],
                    });
                    save_menu_config(path, config)?;
                    println!(
                        "  Canal {name} adicionado como opcional; configure a allowlist no modo avançado antes de habilitar."
                    );
                }
            }
            "3" => {
                let options = config
                    .channels
                    .iter()
                    .map(|channel| {
                        (
                            format!("{} — {}", channel.name, channel.kind),
                            channel.name.clone(),
                        )
                    })
                    .collect::<Vec<_>>();
                let Some(name) =
                    choose_catalog_value("Canal para configurar", &options, "", false)?
                else {
                    continue;
                };
                let advanced_options = static_catalog(&[
                    ("Manter configuração atual", "keep"),
                    ("Aplicar credencial padrão do catálogo", "default"),
                    ("Configuração avançada (pode solicitar dados)", "advanced"),
                ]);
                let current = config
                    .channels
                    .iter()
                    .find(|item| item.name == name)
                    .map(|item| {
                        if item.credential_env.is_empty() {
                            "default"
                        } else {
                            "keep"
                        }
                    })
                    .unwrap_or("keep");
                let Some(mode) = choose_catalog_value(
                    "Modo de configuração",
                    &advanced_options,
                    current,
                    false,
                )?
                else {
                    continue;
                };
                if mode == "advanced" {
                    channel_command(config, path, ChannelCommands::Configure { name }, cli).await?;
                } else if mode == "default" {
                    let index = config
                        .channels
                        .iter()
                        .position(|item| item.name == name)
                        .unwrap();
                    let spec = channels::find_spec(&config.channels[index].kind)?;
                    if spec.credential_hint.starts_with("SAPIENS_") {
                        config.channels[index].credential_env = spec.credential_hint.into();
                    }
                    save_menu_config(path, config)?;
                    println!(
                        "  Credencial padrão aplicada. O canal permanece desabilitado até a allowlist ser configurada."
                    );
                } else {
                    println!("  Configuração atual preservada.");
                }
            }
            "4" => {
                let options = config
                    .channels
                    .iter()
                    .map(|channel| {
                        (
                            format!("{} — {}", channel.name, channel.kind),
                            channel.name.clone(),
                        )
                    })
                    .collect::<Vec<_>>();
                let Some(name) = choose_catalog_value("Canal para testar", &options, "", false)?
                else {
                    continue;
                };
                channel_command(config, path, ChannelCommands::Test { name }, cli).await?;
            }
            "5" => channel_command(config, path, ChannelCommands::Start, cli).await?,
            "6" => {
                let options = config
                    .channels
                    .iter()
                    .map(|channel| {
                        (
                            format!("{} — {}", channel.name, channel.kind),
                            channel.name.clone(),
                        )
                    })
                    .collect::<Vec<_>>();
                let Some(name) =
                    choose_catalog_value("Canal para habilitar/desabilitar", &options, "", false)?
                else {
                    continue;
                };
                let index = config
                    .channels
                    .iter()
                    .position(|item| item.name == name)
                    .ok_or_else(|| anyhow::anyhow!("canal não encontrado: {name}"))?;
                let enabled_options =
                    static_catalog(&[("Habilitar", "true"), ("Desabilitar", "false")]);
                let Some(enabled_value) = choose_catalog_value(
                    "Estado do canal",
                    &enabled_options,
                    if config.channels[index].enabled {
                        "true"
                    } else {
                        "false"
                    },
                    false,
                )?
                else {
                    continue;
                };
                let enabled = enabled_value == "true";
                config.channels[index].enabled = enabled;
                config.features.channels = config.channels.iter().any(|item| item.enabled);
                save_menu_config(path, config)?;
                println!(
                    "  Canal {}: {}.",
                    name,
                    if enabled {
                        "habilitado"
                    } else {
                        "desabilitado"
                    }
                );
            }
            "0" | "q" | "Q" => break,
            _ => println!("  Opção inválida."),
        }
    }
    Ok(())
}

fn run_gateway_menu(path: &Path, config: &mut AppConfig, cli: &Cli) -> Result<()> {
    loop {
        println!("\n  Gateway e interface");
        println!("  [1] Bind: {}", config.server.bind);
        println!(
            "  [2] Autenticação (variável de ambiente): {}",
            config.server.auth_env
        );
        println!("  [3] Interface: {}", config.interface.mode);
        println!(
            "  [4] Abrir navegador automaticamente: {}",
            config.interface.auto_open_browser
        );
        println!(
            "  [5] Rate limit: {} req/min",
            config.security.max_requests_per_minute
        );
        println!("  [6] Listar sessões do gateway");
        println!("  [7] Ver status do gateway/WebUI");
        println!("  [0] Voltar");
        match menu_choice("Escolha uma opção")?.as_str() {
            "1" => edit_menu_value(
                path,
                config,
                "server.bind",
                "Bind (ex.: 127.0.0.1:8787)",
                &config.server.bind.clone(),
            )?,
            "2" => edit_menu_value(
                path,
                config,
                "server.auth_env",
                "Variável de autenticação (vazio para desativar)",
                &config.server.auth_env.clone(),
            )?,
            "3" => edit_menu_value(
                path,
                config,
                "interface.mode",
                "Interface [powershell/web/both]",
                &config.interface.mode.clone(),
            )?,
            "4" => edit_menu_bool(
                path,
                config,
                "interface.auto_open_browser",
                "Abrir navegador automaticamente?",
                config.interface.auto_open_browser,
            )?,
            "5" => edit_menu_value(
                path,
                config,
                "security.max_requests_per_minute",
                "Máximo de requisições por minuto",
                &config.security.max_requests_per_minute.to_string(),
            )?,
            "6" => session_command(path, SessionCommands::List, cli)?,
            "7" => print_status(config, path, cli)?,
            "0" | "q" | "Q" => break,
            _ => println!("  Opção inválida."),
        }
    }
    Ok(())
}

fn run_security_menu(path: &Path, config: &mut AppConfig, cli: &Cli) -> Result<()> {
    loop {
        println!("\n  Segurança e recursos");
        println!("  [1] Modo de segurança: {}", config.security.mode);
        println!(
            "  [2] Workspace permitido: {}",
            config.security.workspace.display()
        );
        println!(
            "  [3] Domínios permitidos: {}",
            config.security.allowed_domains.join(",")
        );
        println!(
            "  [4] Permitir redes privadas: {}",
            config.security.allow_private_networks
        );
        println!("  [5] Perfil de recursos: {}", config.resources.profile);
        println!("  [6] Limites GPU/CPU/memória/concurrency");
        println!("  [7] Shell allowlist e limites");
        println!("  [8] Listar aprovações persistentes");
        println!("  [9] Conceder aprovação temporária");
        println!("  [10] Revogar aprovação");
        println!("  [11] Emergency stop do computer use");
        println!("  [12] Resetar emergency stop");
        println!("  [0] Voltar");
        match menu_choice("Escolha uma opção")?.as_str() {
            "1" => edit_menu_value(
                path,
                config,
                "security.mode",
                "Modo [readonly/supervised/trusted]",
                &config.security.mode.clone(),
            )?,
            "2" => edit_menu_value(
                path,
                config,
                "security.workspace",
                "Workspace",
                &config.security.workspace.display().to_string(),
            )?,
            "3" => edit_menu_value(
                path,
                config,
                "security.allowed_domains",
                "Domínios separados por vírgula",
                &config.security.allowed_domains.join(","),
            )?,
            "4" => edit_menu_bool(
                path,
                config,
                "security.allow_private_networks",
                "Permitir redes privadas?",
                config.security.allow_private_networks,
            )?,
            "5" => {
                let profile = setup_ask(
                    "Perfil [economy/balanced/performance/custom]",
                    &config.resources.profile,
                    false,
                    true,
                )?;
                resources_command(
                    config,
                    path,
                    ResourceCommands::Profile { name: profile },
                    cli,
                )?;
            }
            "6" => {
                edit_menu_value(
                    path,
                    config,
                    "resources.max_gpu_percent",
                    "GPU máxima (1-100)",
                    &config.resources.max_gpu_percent.to_string(),
                )?;
                edit_menu_value(
                    path,
                    config,
                    "resources.max_cpu_percent",
                    "CPU máxima (1-100)",
                    &config.resources.max_cpu_percent.to_string(),
                )?;
                edit_menu_value(
                    path,
                    config,
                    "resources.max_memory_mb",
                    "Memória máxima em MB",
                    &config.resources.max_memory_mb.to_string(),
                )?;
                edit_menu_value(
                    path,
                    config,
                    "resources.max_concurrent",
                    "Concorrência máxima",
                    &config.resources.max_concurrent.to_string(),
                )?;
            }
            "7" => {
                edit_menu_value(
                    path,
                    config,
                    "shell.allowlist",
                    "Executáveis permitidos separados por vírgula",
                    &config.shell.allowlist.join(","),
                )?;
                edit_menu_value(
                    path,
                    config,
                    "shell.max_output_bytes",
                    "Saída máxima em bytes",
                    &config.shell.max_output_bytes.to_string(),
                )?;
                edit_menu_value(
                    path,
                    config,
                    "shell.max_memory_mb",
                    "Memória do shell em MB",
                    &config.shell.max_memory_mb.to_string(),
                )?;
                edit_menu_value(
                    path,
                    config,
                    "shell.max_processes",
                    "Processos máximos",
                    &config.shell.max_processes.to_string(),
                )?;
                edit_menu_value(
                    path,
                    config,
                    "shell.max_cpu_secs",
                    "CPU máxima do shell em segundos",
                    &config.shell.max_cpu_secs.to_string(),
                )?;
            }
            "8" => approval_command(config, path, ApprovalCommands::List, cli)?,
            "9" => {
                let scope = setup_ask("Escopo (ex.: browser.click)", "", false, true)?;
                let risk = setup_ask("Risco", "external_write", false, true)?;
                let expires_secs = setup_ask("Validade em segundos", "3600", false, true)?
                    .parse()
                    .context("validade deve ser um número inteiro")?;
                if setup_ask_bool("Confirmar aprovação?", false, false, true)? {
                    let mut approved_cli = cli.clone();
                    approved_cli.yes = true;
                    approval_command(
                        config,
                        path,
                        ApprovalCommands::Grant {
                            scope,
                            risk,
                            expires_secs,
                        },
                        &approved_cli,
                    )?;
                } else {
                    println!("  Aprovação cancelada.");
                }
            }
            "10" => {
                let scope = setup_ask("Escopo para revogar", "", false, true)?;
                if setup_ask_bool("Confirmar revogação?", false, false, true)? {
                    let mut approved_cli = cli.clone();
                    approved_cli.yes = true;
                    approval_command(
                        config,
                        path,
                        ApprovalCommands::Revoke { scope },
                        &approved_cli,
                    )?;
                } else {
                    println!("  Revogação cancelada.");
                }
            }
            "11" => {
                let stop_path = sapiens_agent::computer::emergency_stop_path(path);
                std::fs::write(&stop_path, b"stop\n")?;
                println!("  Emergency stop ativado: {}", stop_path.display());
            }
            "12" => {
                if setup_ask_bool("Confirmar reset do emergency stop?", false, false, true)? {
                    let stop_path = sapiens_agent::computer::emergency_stop_path(path);
                    let _ = std::fs::remove_file(&stop_path);
                    println!("  Emergency stop resetado.");
                } else {
                    println!("  Reset cancelado.");
                }
            }
            "0" | "q" | "Q" => break,
            _ => println!("  Opção inválida."),
        }
    }
    Ok(())
}

fn run_memory_menu(path: &Path, config: &mut AppConfig, cli: &Cli) -> Result<()> {
    loop {
        println!("\n  Memória, identidade e workspace");
        println!("  [1] Ativar memória: {}", config.features.memory);
        println!(
            "  [2] Retenção: {} dias (0 = ilimitada)",
            config.memory_retention_days
        );
        println!("  [3] Inicializar arquivos de identidade");
        println!("  [4] Mostrar identidade redigida");
        println!("  [5] Mostrar caminhos de identidade");
        println!("  [6] Listar sessões");
        println!("  [7] Exportar memória");
        println!("  [8] Backend de memória: {}", config.memory_backend);
        println!("  [9] Ver arquivos de identidade/workspace");
        println!("  [0] Voltar");
        match menu_choice("Escolha uma opção")?.as_str() {
            "1" => edit_menu_bool(
                path,
                config,
                "features.memory",
                "Ativar memória?",
                config.features.memory,
            )?,
            "2" => edit_menu_value(
                path,
                config,
                "memory_retention_days",
                "Retenção em dias",
                &config.memory_retention_days.to_string(),
            )?,
            "3" => identity_command(path, IdentityCommands::Init, cli)?,
            "4" => identity_command(path, IdentityCommands::Show, cli)?,
            "5" => identity_command(path, IdentityCommands::Path, cli)?,
            "6" => session_command(path, SessionCommands::List, cli)?,
            "7" => memory_command(config, path, MemoryCommands::Export { path: None }, cli)?,
            "8" => edit_menu_value(
                path,
                config,
                "memory_backend",
                "Backend [jsonl]",
                &config.memory_backend.clone(),
            )?,
            "9" => print_workspace_manifest(config, path)?,
            "0" | "q" | "Q" => break,
            _ => println!("  Opção inválida."),
        }
    }
    Ok(())
}

fn run_skills_tools_menu(path: &Path, config: &mut AppConfig, cli: &Cli) -> Result<()> {
    loop {
        println!("\n  Skills, ferramentas e automações");
        println!("  [1] Listar/validar skills");
        println!("  [2] Sugerir skill para tarefas repetitivas");
        println!("  [3] Criar skill candidata");
        println!("  [4] Listar ferramentas e estados");
        println!("  [5] Scheduler, cron e heartbeat");
        println!("  [6] Alternar browser");
        println!("  [7] Alternar computer use");
        println!("  [8] Alternar shell");
        println!("  [9] MCP: servidores e diagnóstico");
        println!("  [10] Plugins: catálogo e verificação");
        println!("  [11] Logs e receipts");
        println!("  [12] Alternar áudio opcional");
        println!("  [0] Voltar");
        match menu_choice("Escolha uma opção")?.as_str() {
            "1" => skills_command(config, path, SkillsCommands::List, cli)?,
            "2" => skills_command(config, path, SkillsCommands::Suggest, cli)?,
            "3" => {
                let name = setup_ask("Nome da skill", "nova-skill", false, true)?;
                let purpose = setup_ask("Propósito", "Workflow reutilizável local", false, true)?;
                skills_command(config, path, SkillsCommands::Create { name, purpose }, cli)?;
            }
            "4" => tools_command(
                config,
                path,
                ToolCommands::List {
                    enabled_only: false,
                },
                cli,
            )?,
            "5" => run_schedule_menu(path, config, cli)?,
            "6" => edit_menu_bool(
                path,
                config,
                "features.browser",
                "Ativar browser?",
                config.features.browser,
            )?,
            "7" => edit_menu_bool(
                path,
                config,
                "features.computer_use",
                "Ativar computer use?",
                config.features.computer_use,
            )?,
            "8" => edit_menu_bool(
                path,
                config,
                "features.shell",
                "Ativar shell?",
                config.features.shell,
            )?,
            "9" => mcp_command(config, path, McpCommands::Servers, cli)?,
            "10" => plugin_command(config, path, PluginCommands::List { directory: None }, cli)?,
            "11" => {
                logs_command(path, cli)?;
                receipts_command(path, cli)?;
            }
            "12" => edit_menu_bool(
                path,
                config,
                "features.audio",
                "Ativar áudio opcional?",
                config.features.audio,
            )?,
            "0" | "q" | "Q" => break,
            _ => println!("  Opção inválida."),
        }
    }
    Ok(())
}

fn export_config(config: &AppConfig, destination: &Path, cli: &Cli) -> Result<()> {
    if cli.dry_run {
        message(
            cli,
            "Dry-run: configuração não exportada.",
            serde_json::json!({"destination": destination, "saved": false}),
        );
        return Ok(());
    }
    config::save(destination, config)?;
    message(
        cli,
        "Configuração exportada sem valores de segredo; apenas referências de credencial foram preservadas.",
        serde_json::json!({"destination": destination, "saved": true}),
    );
    Ok(())
}

async fn run_control_menu(path: &Path, config: &mut AppConfig, cli: &Cli) -> Result<bool> {
    loop {
        println!("\n  Controle do agente");
        println!("  [1] Iniciar gateway");
        println!("  [2] Parar gateway");
        println!("  [3] Reiniciar gateway");
        println!("  [4] Ver status");
        println!("  [5] Diagnóstico");
        println!("  [6] Reconfigurar agente");
        println!("  [7] Ajuda completa");
        println!("  [8] Exportar configuração");
        println!("  [9] Importar configuração");
        println!("  [10] Restaurar backup");
        println!("  [0] Sair do menu");
        match menu_choice("Escolha uma opção")?.as_str() {
            "1" => {
                let bind = bind_for(config, cli.port)?;
                println!("  Iniciando o gateway no CMD. Use Ctrl+C para parar.");
                run_server(
                    config.clone(),
                    path,
                    bind,
                    cli.open_browser && !cli.no_browser,
                )
                .await?;
                println!("  Gateway encerrado.");
            }
            "2" => stop_server(path, cli)?,
            "3" => {
                stop_server(path, cli)?;
                tokio::time::sleep(Duration::from_millis(150)).await;
                let bind = bind_for(config, cli.port)?;
                println!("  Reiniciando o gateway no CMD. Use Ctrl+C para parar.");
                run_server(
                    config.clone(),
                    path,
                    bind,
                    cli.open_browser && !cli.no_browser,
                )
                .await?;
            }
            "4" => print_status(config, path, cli)?,
            "5" => doctor(config, path, cli)?,
            "6" => run_setup(path, config, cli, false, true)?,
            "7" => {
                let mut command = Cli::command();
                command.print_help()?;
                println!();
            }
            "8" => {
                let destination = setup_ask(
                    "Arquivo de exportação",
                    &path
                        .with_file_name("config.export.toml")
                        .display()
                        .to_string(),
                    false,
                    true,
                )?;
                export_config(config, Path::new(&destination), cli)?;
            }
            "9" => {
                let source = setup_ask(
                    "Arquivo TOML para importar",
                    "config.import.toml",
                    false,
                    true,
                )?;
                if setup_ask_bool("Confirmar importação?", false, false, true)? {
                    if !Path::new(&source).exists() {
                        anyhow::bail!("arquivo de configuração não encontrado: {source}");
                    }
                    let imported = config::load(Path::new(&source))?;
                    config::save(path, &imported)?;
                    *config = imported;
                    println!("  Configuração importada e validada.");
                } else {
                    println!("  Importação cancelada.");
                }
            }
            "10" => {
                let source = setup_ask(
                    "Backup para restaurar",
                    &path.with_extension("toml.bak").display().to_string(),
                    false,
                    true,
                )?;
                if setup_ask_bool("Confirmar restauração?", false, false, true)? {
                    if !Path::new(&source).exists() {
                        anyhow::bail!("backup de configuração não encontrado: {source}");
                    }
                    let restored = config::load(Path::new(&source))?;
                    config::save(path, &restored)?;
                    *config = restored;
                    println!("  Backup restaurado e validado.");
                } else {
                    println!("  Restauração cancelada.");
                }
            }
            "0" | "q" | "Q" => return Ok(true),
            _ => println!("  Opção inválida."),
        }
    }
}

fn initialize(path: &Path, config: &mut AppConfig, workspace: Option<&Path>) -> Result<()> {
    if path.exists() {
        *config = config::load(path)?;
        if let Some(workspace) = workspace {
            config.security.workspace = workspace.to_path_buf();
            std::fs::create_dir_all(workspace)?;
            config::save(path, config)?;
        }
        if let Some(dir) = path.parent() {
            sapiens_agent::identity::ensure(dir)?;
        }
        return Ok(());
    }
    if let Some(workspace) = workspace {
        config.security.workspace = workspace.to_path_buf();
    }
    std::fs::create_dir_all(&config.security.workspace)?;
    config.initialized = true;
    config::save(path, config)?;
    if let Some(dir) = path.parent() {
        sapiens_agent::identity::ensure(dir)?;
    }
    Ok(())
}

fn ensure_initialized(path: &Path, config: &mut AppConfig, cli: &Cli) -> Result<()> {
    if !path.exists() {
        initialize(path, config, cli.workspace.as_deref())?;
    }
    Ok(())
}

fn run_setup(
    path: &Path,
    config: &mut AppConfig,
    cli: &Cli,
    first_run: bool,
    force_interactive: bool,
) -> Result<()> {
    initialize(path, config, cli.workspace.as_deref())?;
    println!(
        "{}",
        if first_run {
            "Primeira configuração do Sapiens Agent"
        } else {
            "Configuração do Sapiens Agent"
        }
    );
    let workspace_default = config.security.workspace.display().to_string();
    let workspace_options = vec![
        (
            format!("Manter workspace atual: {workspace_default}"),
            workspace_default.clone(),
        ),
        (
            "Usar workspace padrão do agente".into(),
            config::config_dir()?
                .join("workspace")
                .display()
                .to_string(),
        ),
        ("Personalizado/Avançado".into(), "custom".into()),
    ];
    let Some(workspace_choice) = choose_catalog_value(
        "Workspace",
        &workspace_options,
        &workspace_default,
        cli.yes && !force_interactive,
    )?
    else {
        println!("Configuração cancelada.");
        return Ok(());
    };
    let workspace = if workspace_choice == "custom" {
        setup_ask(
            "Workspace personalizado",
            &workspace_default,
            cli.yes,
            force_interactive,
        )?
    } else {
        workspace_choice
    };
    config.security.workspace = PathBuf::from(workspace);
    std::fs::create_dir_all(&config.security.workspace)?;

    let provider_options = provider_catalog_options(config);
    let current_provider = config
        .active_provider
        .as_ref()
        .map(|alias| format!("alias:{alias}"))
        .unwrap_or_else(|| "none".into());
    let Some(provider_choice) = choose_catalog_value(
        "Provider e autenticação",
        &provider_options,
        &current_provider,
        cli.yes && !force_interactive,
    )?
    else {
        println!("Configuração cancelada.");
        return Ok(());
    };
    if let Some(alias) = provider_choice.strip_prefix("alias:") {
        config::find_provider(config, alias)?;
        config.active_provider = Some(alias.to_string());
    } else if let Some(kind) = provider_choice.strip_prefix("kind:") {
        let spec = providers::find_spec(kind)
            .ok_or_else(|| anyhow::anyhow!("provider não encontrado no catálogo: {kind}"))?;
        let alias = kind.to_string();
        if let Some(provider) = config.providers.iter_mut().find(|p| p.alias == alias) {
            provider.kind = kind.to_string();
            provider.protocol = spec
                .protocols
                .split(',')
                .next()
                .unwrap_or("chat_completions")
                .into();
            if provider.api_key_env.is_empty() && spec.credential_hint != "nenhuma" {
                provider.api_key_env = spec.credential_hint.into();
            }
        } else {
            config.providers.push(ProviderConfig {
                alias: alias.clone(),
                kind: kind.to_string(),
                protocol: spec
                    .protocols
                    .split(',')
                    .next()
                    .unwrap_or("chat_completions")
                    .into(),
                api_key_env: if spec.credential_hint == "nenhuma" {
                    String::new()
                } else {
                    spec.credential_hint.into()
                },
                ..Default::default()
            });
        }
        config.active_provider = Some(alias);
        println!(
            "  Provider selecionado. Endpoint e modelo podem ser definidos em [2] Provider e API → Avançado."
        );
    } else if provider_choice == "custom" {
        let alias = setup_ask(
            "Alias personalizado",
            "principal",
            cli.yes,
            force_interactive,
        )?;
        let base_url = setup_ask("URL base personalizada", "", cli.yes, force_interactive)?;
        let model = setup_ask("Modelo personalizado", "", cli.yes, force_interactive)?;
        let api_key_env = setup_ask(
            "Variável da credencial (nunca a chave)",
            "SAPIENS_API_KEY",
            cli.yes,
            force_interactive,
        )?;
        if let Some(provider) = config.providers.iter_mut().find(|p| p.alias == alias) {
            provider.kind = "custom".into();
            provider.protocol = "chat_completions".into();
            provider.base_url = base_url;
            provider.model = model;
            provider.api_key_env = api_key_env;
        } else {
            config.providers.push(ProviderConfig {
                alias: alias.clone(),
                base_url,
                model,
                api_key_env,
                ..Default::default()
            });
        }
        config.active_provider = Some(alias);
    }

    let security_options = static_catalog(&[
        ("Somente leitura", "readonly"),
        ("Supervisionado (recomendado)", "supervised"),
        ("Confiável", "trusted"),
        ("YOLO (alto risco)", "yolo"),
    ]);
    if let Some(mode) = choose_catalog_value(
        "Perfil de segurança",
        &security_options,
        &config.security.mode,
        cli.yes && !force_interactive,
    )? {
        config.security.mode = mode;
    } else {
        println!("Configuração cancelada.");
        return Ok(());
    }

    let capability_options = static_catalog(&[
        ("Essencial — leve e seguro", "essential"),
        (
            "Automação — browser, shell, memória e scheduler",
            "automation",
        ),
        ("Completo — todas as capacidades disponíveis", "complete"),
        ("Personalizado/Avançado", "custom"),
    ]);
    if let Some(profile) = choose_catalog_value(
        "Perfil de capacidades",
        &capability_options,
        "custom",
        cli.yes && !force_interactive,
    )? {
        apply_capability_profile(config, &profile)?;
    } else {
        println!("Configuração cancelada.");
        return Ok(());
    }

    let interface_options = static_catalog(&[
        ("PowerShell/CMD — recomendado", "powershell"),
        ("WebUI local — somente quando solicitada", "web"),
        ("PowerShell/CMD + WebUI opcional", "both"),
    ]);
    if let Some(mode) = choose_catalog_value(
        "Interface de configuração",
        &interface_options,
        &config.interface.mode,
        cli.yes && !force_interactive,
    )? {
        config.interface.mode = mode;
        config.interface.auto_open_browser = false;
    } else {
        println!("Configuração cancelada.");
        return Ok(());
    }

    let resource_options = static_catalog(&[
        ("Econômico", "economy"),
        ("Equilibrado (recomendado)", "balanced"),
        ("Desempenho", "performance"),
        ("Personalizado/Avançado", "custom"),
    ]);
    if let Some(profile) = choose_catalog_value(
        "Perfil de recursos",
        &resource_options,
        &config.resources.profile,
        cli.yes && !force_interactive,
    )? {
        apply_resource_profile(config, &profile)?;
    } else {
        println!("Configuração cancelada.");
        return Ok(());
    }

    config.initialized = true;
    config::save(path, config)?;
    println!("Configuração salva em {}", path.display());
    Ok(())
}

fn setup_ask(
    label: &str,
    default: &str,
    automatic: bool,
    force_interactive: bool,
) -> Result<String> {
    if force_interactive {
        return ask_prompt(label, default);
    }
    ask(label, default, automatic)
}

fn setup_ask_bool(
    label: &str,
    default: bool,
    automatic: bool,
    force_interactive: bool,
) -> Result<bool> {
    if force_interactive {
        let value = ask_prompt(label, if default { "S/n" } else { "s/N" })?;
        return Ok(matches!(
            value.to_ascii_lowercase().as_str(),
            "s" | "sim" | "y" | "yes"
        ));
    }
    ask_bool(label, default, automatic)
}

fn ask_prompt(label: &str, default: &str) -> Result<String> {
    print!("{label} [{default}]: ");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let input = input.trim();
    Ok(if input.is_empty() {
        default.to_string()
    } else {
        input.to_string()
    })
}

fn ask(label: &str, default: &str, automatic: bool) -> Result<String> {
    if automatic || !std::io::stdin().is_terminal() {
        return Ok(default.to_string());
    }
    print!("{label} [{default}]: ");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let input = input.trim();
    Ok(if input.is_empty() {
        default.to_string()
    } else {
        input.to_string()
    })
}

fn ask_bool(label: &str, default: bool, automatic: bool) -> Result<bool> {
    let value = ask(label, if default { "S/n" } else { "s/N" }, automatic)?;
    if automatic {
        return Ok(default);
    }
    Ok(matches!(
        value.to_ascii_lowercase().as_str(),
        "s" | "sim" | "y" | "yes"
    ))
}

fn print_workspace_manifest(config: &AppConfig, path: &Path) -> Result<()> {
    let identity_dir = path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let files = [
        "IDENTITY.md",
        "PREFERENCES.md",
        "USER.md",
        "SOUL.md",
        "AGENTS.md",
        "HEARTBEAT.md",
        "TOOLS.md",
        "MEMORY.md",
    ];
    println!("  Workspace: {}", config.security.workspace.display());
    println!("  Arquivos de identidade em: {}", identity_dir.display());
    for file in files {
        let workspace_path = config.security.workspace.join(file);
        let identity_path = identity_dir.join(file);
        let status = if workspace_path.exists() || identity_path.exists() {
            "presente"
        } else {
            "ausente (use identidade init ou crie no workspace)"
        };
        println!("  - {file}: {status}");
    }
    Ok(())
}

fn run_schedule_menu(path: &Path, config: &mut AppConfig, cli: &Cli) -> Result<()> {
    loop {
        println!("\n  Scheduler, cron e automações");
        println!("  [1] Listar tarefas");
        println!("  [2] Criar tarefa por intervalo");
        println!("  [3] Criar tarefa única");
        println!("  [4] Criar tarefa cron UTC");
        println!("  [5] Pausar/retomar tarefa");
        println!("  [6] Cancelar tarefa em execução");
        println!("  [7] Remover tarefa");
        println!("  [0] Voltar");
        match menu_choice("Escolha uma opção")?.as_str() {
            "1" => schedule_command(config, path, ScheduleCommands::List, cli)?,
            "2" => {
                let every = setup_ask("Intervalo (ex.: 15m)", "1h", false, true)?;
                let task = setup_ask("Tarefa", "", false, true)?;
                schedule_command(
                    config,
                    path,
                    ScheduleCommands::Add {
                        every,
                        task,
                        webhook: None,
                        channel: None,
                        recipient: None,
                    },
                    cli,
                )?;
            }
            "3" => {
                let at = setup_ask("Data/hora UTC ISO-8601", "", false, true)?;
                let task = setup_ask("Tarefa", "", false, true)?;
                schedule_command(
                    config,
                    path,
                    ScheduleCommands::Once {
                        at,
                        task,
                        webhook: None,
                        channel: None,
                        recipient: None,
                    },
                    cli,
                )?;
            }
            "4" => {
                let expression =
                    setup_ask("Expressão cron de 5 campos UTC", "0 * * * *", false, true)?;
                let task = setup_ask("Tarefa", "", false, true)?;
                schedule_command(
                    config,
                    path,
                    ScheduleCommands::Cron {
                        expression,
                        task,
                        webhook: None,
                        channel: None,
                        recipient: None,
                    },
                    cli,
                )?;
            }
            "5" => {
                let id = setup_ask("ID da tarefa", "job-", false, true)?;
                let resume = setup_ask_bool("Retomar (não = pausar)?", true, false, true)?;
                schedule_command(
                    config,
                    path,
                    if resume {
                        ScheduleCommands::Resume { id }
                    } else {
                        ScheduleCommands::Pause { id }
                    },
                    cli,
                )?;
            }
            "6" => {
                let id = setup_ask("ID da tarefa", "job-", false, true)?;
                schedule_command(config, path, ScheduleCommands::Cancel { id }, cli)?;
            }
            "7" => {
                let id = setup_ask("ID da tarefa", "job-", false, true)?;
                if setup_ask_bool("Confirmar remoção?", false, false, true)? {
                    schedule_command(config, path, ScheduleCommands::Remove { id }, cli)?;
                } else {
                    println!("  Remoção cancelada.");
                }
            }
            "0" | "q" | "Q" => break,
            _ => println!("  Opção inválida."),
        }
    }
    Ok(())
}

fn identity_command(path: &Path, command: IdentityCommands, cli: &Cli) -> Result<()> {
    let dir = path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    match command {
        IdentityCommands::Init => {
            let files = sapiens_agent::identity::ensure(&dir)?;
            message(
                cli,
                "Arquivos de identidade e preferências prontos para edição.",
                serde_json::json!({
                    "identity": files.identity,
                    "preferences": files.preferences,
                    "precedence": "IDENTITY.md é a base; PREFERENCES.md complementa e prevalece em estilo/formato"
                }),
            );
        }
        IdentityCommands::Path => {
            let files = sapiens_agent::identity::IdentityFiles::in_dir(&dir);
            message(
                cli,
                &format!(
                    "Identidade: {}\nPreferências: {}",
                    files.identity.display(),
                    files.preferences.display()
                ),
                serde_json::json!({"identity": files.identity, "preferences": files.preferences}),
            );
        }
        IdentityCommands::Show => {
            let files = sapiens_agent::identity::IdentityFiles::in_dir(&dir);
            let context = sapiens_agent::identity::load(&dir)?;
            let identity = sapiens_agent::observability::redact(&context.identity);
            let preferences = sapiens_agent::observability::redact(&context.preferences);
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "identity": identity,
                        "preferences": preferences,
                        "files": files
                    }))?
                );
            } else {
                println!(
                    "IDENTITY.md ({})\n{}",
                    files.identity.display(),
                    identity.trim()
                );
                println!(
                    "\nPREFERENCES.md ({})\n{}",
                    files.preferences.display(),
                    preferences.trim()
                );
                println!(
                    "\nPrecedência: preferências prevalecem sobre a identidade apenas em estilo e formato."
                );
            }
        }
    }
    Ok(())
}

fn session_command(path: &Path, command: SessionCommands, cli: &Cli) -> Result<()> {
    let session_path = path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("sessions.json");
    let mut store = sapiens_agent::sessions::FileSessionStore::open(&session_path)?;
    match command {
        SessionCommands::List => {
            let sessions = store.list();
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&sessions)?);
            } else if sessions.is_empty() {
                println!("Nenhuma sessão persistida.");
            } else {
                for session in sessions {
                    println!(
                        "{}\t{}\t{}\t{}",
                        session.id, session.channel, session.identity, session.updated_at
                    );
                }
            }
        }
        SessionCommands::Cleanup { older_than } => {
            let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
            let cutoff = now.saturating_sub(older_than);
            let candidates = store
                .list()
                .iter()
                .filter(|session| session.updated_at < cutoff)
                .count();
            if !cli.dry_run && !cli.yes {
                anyhow::bail!("limpeza de sessões exige --yes após revisar o cutoff");
            }
            let removed = if cli.dry_run {
                0
            } else {
                store.cleanup_before(cutoff)?
            };
            sapiens_agent::observability::append_receipt(
                path,
                "session.cleanup",
                "destructive",
                cli.yes || cli.dry_run,
                &format!("candidates={candidates} removed={removed}"),
                None,
            )?;
            message(
                cli,
                if cli.dry_run {
                    "Dry-run: sessões não removidas."
                } else {
                    "Sessões antigas removidas."
                },
                serde_json::json!({"older_than_secs": older_than, "cutoff": cutoff, "candidates": candidates, "removed": removed}),
            );
        }
        SessionCommands::Rotate { id } => {
            if !cli.dry_run && !cli.yes {
                anyhow::bail!("rotação de sessão exige --yes");
            }
            let current = store
                .get(&id)
                .with_context(|| format!("session not found: {id}"))?;
            let rotated = if cli.dry_run {
                current
            } else {
                store.rotate(&id)?
            };
            sapiens_agent::observability::append_receipt(
                path,
                "session.rotate",
                "external_write",
                cli.yes || cli.dry_run,
                &format!("old_session={id} new_session={}", rotated.id),
                None,
            )?;
            message(
                cli,
                if cli.dry_run {
                    "Dry-run: sessão não rotacionada."
                } else {
                    "Sessão rotacionada."
                },
                serde_json::json!({"old_id": id, "new_id": rotated.id, "rotated": !cli.dry_run}),
            );
        }
    }
    Ok(())
}

fn memory_command(
    config: &AppConfig,
    path: &Path,
    command: MemoryCommands,
    cli: &Cli,
) -> Result<()> {
    let memory_path = path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("memory.jsonl");
    let store = sapiens_agent::memory::MemoryStore::open_with_retention(
        memory_path.clone(),
        config.memory_retention_days,
    )?;
    match command {
        MemoryCommands::Search { query } => {
            let items = store.search(&query)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&items)?);
            } else {
                for item in items {
                    println!("{}\t{}\t{}", item.timestamp, item.session, item.text);
                }
            }
        }
        MemoryCommands::List => {
            let items = store.list()?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&items)?);
            } else if items.is_empty() {
                println!("Memória vazia.");
            } else {
                for item in items {
                    println!("{}\t{}\t{}", item.timestamp, item.session, item.text);
                }
            }
        }
        MemoryCommands::Export { path: destination } => {
            let items = store.list()?;
            if let Some(destination) = destination {
                std::fs::write(&destination, serde_json::to_string_pretty(&items)?)?;
                message(
                    cli,
                    "Memória exportada.",
                    serde_json::json!({"path": destination, "items": items.len()}),
                );
            } else if cli.json {
                println!("{}", serde_json::to_string_pretty(&items)?);
            } else {
                for item in items {
                    println!("{}", serde_json::to_string(&item)?);
                }
            }
        }
        MemoryCommands::Delete { session } => {
            if !cli.dry_run && !cli.yes {
                anyhow::bail!("exclusão de memória exige --yes após revisar a sessão");
            }
            let count = if cli.dry_run {
                store
                    .list()?
                    .iter()
                    .filter(|item| item.session == session)
                    .count()
            } else {
                store.delete_session(&session)?
            };
            message(
                cli,
                if cli.dry_run {
                    "Dry-run: memória não excluída."
                } else {
                    "Memória da sessão excluída."
                },
                serde_json::json!({"session": session, "items": count, "deleted": !cli.dry_run}),
            );
        }
        MemoryCommands::Clear => {
            if !cli.dry_run && !cli.yes {
                anyhow::bail!("limpeza de memória exige --yes");
            }
            let count = store.list()?.len();
            if !cli.dry_run {
                store.clear()?;
            }
            message(
                cli,
                if cli.dry_run {
                    "Dry-run: memória não limpa."
                } else {
                    "Memória limpa."
                },
                serde_json::json!({"items": count, "deleted": !cli.dry_run}),
            );
        }
    }
    Ok(())
}

fn browser_command(
    config: &AppConfig,
    config_path: &Path,
    command: BrowserCommands,
    cli: &Cli,
) -> Result<()> {
    if cli.dry_run {
        message(
            cli,
            "Dry-run: browser não executado.",
            serde_json::json!({"executed": false}),
        );
        return Ok(());
    }
    if !config.features.browser {
        anyhow::bail!("browser está desativado; habilite com sapiens-agent skills enable browser");
    }
    let policy = Policy {
        mode: config.security.mode.clone(),
        workspace: config.security.workspace.clone(),
        allowed_domains: config.security.allowed_domains.clone(),
        allow_private_networks: config.security.allow_private_networks,
    };
    let (session, action_is_write) = match &command {
        BrowserCommands::Open { session, .. }
        | BrowserCommands::Goto { session, .. }
        | BrowserCommands::Snapshot { session }
        | BrowserCommands::Click { session, .. }
        | BrowserCommands::DoubleClick { session, .. }
        | BrowserCommands::Drag { session, .. }
        | BrowserCommands::Fill { session, .. }
        | BrowserCommands::Hover { session, .. }
        | BrowserCommands::Press { session, .. }
        | BrowserCommands::Scroll { session, .. }
        | BrowserCommands::GoBack { session }
        | BrowserCommands::GoForward { session }
        | BrowserCommands::Reload { session }
        | BrowserCommands::Check { session, .. }
        | BrowserCommands::Uncheck { session, .. }
        | BrowserCommands::DialogAccept { session, .. }
        | BrowserCommands::DialogDismiss { session }
        | BrowserCommands::Resize { session, .. }
        | BrowserCommands::Select { session, .. }
        | BrowserCommands::Wait { session, .. }
        | BrowserCommands::Extract { session, .. }
        | BrowserCommands::Upload { session, .. }
        | BrowserCommands::Download { session, .. }
        | BrowserCommands::TraceStart { session }
        | BrowserCommands::TraceStop { session }
        | BrowserCommands::StateSave { session, .. }
        | BrowserCommands::StateLoad { session, .. }
        | BrowserCommands::Screenshot { session, .. }
        | BrowserCommands::Tabs { session }
        | BrowserCommands::TabNew { session, .. }
        | BrowserCommands::TabSelect { session, .. }
        | BrowserCommands::TabClose { session, .. }
        | BrowserCommands::Close { session }
        | BrowserCommands::ProfileRevoke { session } => (
            session.clone(),
            matches!(
                &command,
                BrowserCommands::Click { .. }
                    | BrowserCommands::DoubleClick { .. }
                    | BrowserCommands::Drag { .. }
                    | BrowserCommands::Fill { .. }
                    | BrowserCommands::Check { .. }
                    | BrowserCommands::Uncheck { .. }
                    | BrowserCommands::Select { .. }
                    | BrowserCommands::Press { .. }
                    | BrowserCommands::DialogAccept { .. }
                    | BrowserCommands::DialogDismiss { .. }
                    | BrowserCommands::Resize { .. }
                    | BrowserCommands::Upload { .. }
                    | BrowserCommands::Download { .. }
                    | BrowserCommands::StateSave { .. }
                    | BrowserCommands::StateLoad { .. }
                    | BrowserCommands::ProfileRevoke { .. }
            ),
        ),
    };
    let action_name = match &command {
        BrowserCommands::Open { .. } => "browser.open",
        BrowserCommands::Goto { .. } => "browser.goto",
        BrowserCommands::Snapshot { .. } => "browser.snapshot",
        BrowserCommands::Click { .. } => "browser.click",
        BrowserCommands::DoubleClick { .. } => "browser.double_click",
        BrowserCommands::Drag { .. } => "browser.drag",
        BrowserCommands::Fill { .. } => "browser.fill",
        BrowserCommands::Hover { .. } => "browser.hover",
        BrowserCommands::Press { .. } => "browser.press",
        BrowserCommands::Scroll { .. } => "browser.scroll",
        BrowserCommands::GoBack { .. } => "browser.go_back",
        BrowserCommands::GoForward { .. } => "browser.go_forward",
        BrowserCommands::Reload { .. } => "browser.reload",
        BrowserCommands::Check { .. } => "browser.check",
        BrowserCommands::Uncheck { .. } => "browser.uncheck",
        BrowserCommands::DialogAccept { .. } => "browser.dialog_accept",
        BrowserCommands::DialogDismiss { .. } => "browser.dialog_dismiss",
        BrowserCommands::Resize { .. } => "browser.resize",
        BrowserCommands::Select { .. } => "browser.select",
        BrowserCommands::Wait { .. } => "browser.wait",
        BrowserCommands::Extract { .. } => "browser.extract",
        BrowserCommands::Upload { .. } => "browser.upload",
        BrowserCommands::Download { .. } => "browser.download",
        BrowserCommands::TraceStart { .. } => "browser.trace_start",
        BrowserCommands::TraceStop { .. } => "browser.trace_stop",
        BrowserCommands::StateSave { .. } => "browser.state_save",
        BrowserCommands::StateLoad { .. } => "browser.state_load",
        BrowserCommands::Screenshot { .. } => "browser.screenshot",
        BrowserCommands::Tabs { .. } => "browser.tabs",
        BrowserCommands::TabNew { .. } => "browser.tab_new",
        BrowserCommands::TabSelect { .. } => "browser.tab_select",
        BrowserCommands::TabClose { .. } => "browser.tab_close",
        BrowserCommands::Close { .. } => "browser.close",
        BrowserCommands::ProfileRevoke { .. } => "browser.profile_revoke",
    };
    let approved =
        cli.yes || approval_active(config, action_name) || approval_active(config, "browser.write");
    if action_is_write && config.security.mode == "supervised" && !approved {
        anyhow::bail!("ação de browser exige aprovação; use --yes ou approval grant");
    }
    let persistent = matches!(
        &command,
        BrowserCommands::Open {
            persistent: true,
            ..
        }
    );
    let browser = PlaywrightCliBrowser::new(session)?.with_persistent_profile(persistent);
    match command {
        BrowserCommands::Open { url, .. } => {
            let tab = browser.open(&url, &policy)?;
            browser_result(
                cli,
                serde_json::json!({"id": tab.id, "url": tab.url, "title": tab.title}),
            );
        }
        BrowserCommands::Goto { url, .. } => {
            let tab = browser.goto(&url, &policy)?;
            browser_result(
                cli,
                serde_json::json!({"id": tab.id, "url": tab.url, "title": tab.title}),
            );
        }
        BrowserCommands::Snapshot { .. } => {
            print_browser_text(cli, &browser.snapshot(browser.session_name())?)
        }
        BrowserCommands::Click { target, .. } => print_browser_text(cli, &browser.click(&target)?),
        BrowserCommands::DoubleClick { target, .. } => {
            print_browser_text(cli, &browser.double_click(&target)?)
        }
        BrowserCommands::Drag {
            start_target,
            end_target,
            ..
        } => print_browser_text(cli, &browser.drag(&start_target, &end_target)?),
        BrowserCommands::Fill { target, text, .. } => {
            print_browser_text(cli, &browser.fill(&target, &text)?)
        }
        BrowserCommands::Hover { target, .. } => print_browser_text(cli, &browser.hover(&target)?),
        BrowserCommands::Press { target, key, .. } => {
            print_browser_text(cli, &browser.press(&target, &key)?)
        }
        BrowserCommands::Scroll { dx, dy, .. } => print_browser_text(cli, &browser.scroll(dx, dy)?),
        BrowserCommands::GoBack { .. } => print_browser_text(cli, &browser.go_back()?),
        BrowserCommands::GoForward { .. } => print_browser_text(cli, &browser.go_forward()?),
        BrowserCommands::Reload { .. } => print_browser_text(cli, &browser.reload()?),
        BrowserCommands::Check { target, .. } => print_browser_text(cli, &browser.check(&target)?),
        BrowserCommands::Uncheck { target, .. } => {
            print_browser_text(cli, &browser.uncheck(&target)?)
        }
        BrowserCommands::DialogAccept { prompt, .. } => {
            print_browser_text(cli, &browser.dialog_accept(prompt.as_deref())?)
        }
        BrowserCommands::DialogDismiss { .. } => {
            print_browser_text(cli, &browser.dialog_dismiss()?)
        }
        BrowserCommands::Resize { width, height, .. } => {
            print_browser_text(cli, &browser.resize(width, height)?)
        }
        BrowserCommands::Select { target, value, .. } => {
            print_browser_text(cli, &browser.select(&target, &value)?)
        }
        BrowserCommands::Wait { millis, .. } => print_browser_text(cli, &browser.wait(millis)?),
        BrowserCommands::Extract { target, .. } => {
            print_browser_text(cli, &browser.extract(target.as_deref())?)
        }
        BrowserCommands::Upload { files, .. } => {
            print_browser_text(cli, &browser.upload(&files, &policy)?)
        }
        BrowserCommands::Download { target, output, .. } => {
            print_browser_text(cli, &browser.download(&target, &output, &policy)?)
        }
        BrowserCommands::TraceStart { .. } => print_browser_text(cli, &browser.trace_start()?),
        BrowserCommands::TraceStop { .. } => print_browser_text(cli, &browser.trace_stop()?),
        BrowserCommands::StateSave { output, .. } => {
            print_browser_text(cli, &browser.state_save(&output, &policy)?)
        }
        BrowserCommands::StateLoad { input, .. } => {
            print_browser_text(cli, &browser.state_load(&input, &policy)?)
        }
        BrowserCommands::Screenshot { target, .. } => {
            print_browser_text(cli, &browser.screenshot(target.as_deref())?)
        }
        BrowserCommands::Tabs { .. } => print_browser_text(cli, &browser.tabs()?),
        BrowserCommands::TabNew { url, .. } => {
            let tab = browser.tab_new(&url, &policy)?;
            browser_result(
                cli,
                serde_json::json!({"id": tab.id, "url": tab.url, "title": tab.title}),
            );
        }
        BrowserCommands::TabSelect { index, .. } => {
            let tab = browser.tab_select(index, &policy)?;
            browser_result(
                cli,
                serde_json::json!({"id": tab.id, "url": tab.url, "title": tab.title}),
            );
        }
        BrowserCommands::TabClose { index, .. } => {
            print_browser_text(cli, &browser.tab_close(index)?)
        }
        BrowserCommands::Close { .. } => print_browser_text(cli, &browser.close()?),
        BrowserCommands::ProfileRevoke { .. } => print_browser_text(cli, &browser.delete_data()?),
    }
    sapiens_agent::observability::append_receipt(
        config_path,
        action_name,
        if action_is_write {
            "external_write"
        } else {
            "read"
        },
        approved || !action_is_write,
        "ok",
        Some(browser.session_name()),
    )?;
    Ok(())
}

fn browser_result(cli: &Cli, value: serde_json::Value) {
    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".into())
        );
    } else {
        println!(
            "{}",
            value
                .get("url")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("browser pronto")
        );
    }
}

fn print_browser_text(cli: &Cli, value: &str) {
    if cli.json {
        println!("{value}");
    } else {
        print!("{value}");
    }
}

async fn computer_command(
    config: &AppConfig,
    config_path: &Path,
    command: ComputerCommands,
    cli: &Cli,
) -> Result<()> {
    if matches!(&command, ComputerCommands::EmergencyStop) {
        let stop_path = sapiens_agent::computer::emergency_stop_path(config_path);
        std::fs::write(&stop_path, b"stop\n")?;
        sapiens_agent::observability::append_receipt(
            config_path,
            "computer.emergency_stop",
            "destructive",
            true,
            "computer sequence stop requested",
            None,
        )?;
        message(
            cli,
            "Emergency stop do computer use ativado.",
            serde_json::json!({"stopped": true, "path": stop_path}),
        );
        return Ok(());
    }
    if matches!(&command, ComputerCommands::ResetStop) {
        if !cli.dry_run && !cli.yes {
            anyhow::bail!("reset do emergency stop exige --yes");
        }
        let stop_path = sapiens_agent::computer::emergency_stop_path(config_path);
        if !cli.dry_run {
            let _ = std::fs::remove_file(&stop_path);
        }
        message(
            cli,
            if cli.dry_run {
                "Dry-run: emergency stop não resetado."
            } else {
                "Emergency stop do computer use resetado."
            },
            serde_json::json!({"stopped": stop_path.exists(), "would_reset": cli.dry_run}),
        );
        return Ok(());
    }
    if cli.dry_run {
        message(
            cli,
            "Dry-run: computer use não executado.",
            serde_json::json!({"executed": false}),
        );
        return Ok(());
    }
    if !config.features.computer_use {
        anyhow::bail!(
            "computer use está desativado; habilite com sapiens-agent skills enable computer-use"
        );
    }
    if let ComputerCommands::Plan { task, output } = command.clone() {
        if task.trim().is_empty() {
            anyhow::bail!("computer plan exige uma tarefa não vazia");
        }
        let assessment = sapiens_agent::policy::assess_prompt(&task);
        if assessment.blocked {
            anyhow::bail!(
                "computer plan bloqueado por avaliação de segurança: {}",
                assessment.flags.join(", ")
            );
        }
        let output = workspace_path(&config.security.workspace, output);
        let policy = Policy {
            mode: config.security.mode.clone(),
            workspace: config.security.workspace.clone(),
            allowed_domains: config.security.allowed_domains.clone(),
            allow_private_networks: config.security.allow_private_networks,
        };
        policy.check_path(&output)?;
        let actions = generate_computer_plan(config, &task).await?;
        if cli.dry_run {
            message(
                cli,
                "Dry-run: plano não foi salvo.",
                serde_json::json!({"actions": actions.len(), "saved": false}),
            );
            return Ok(());
        }
        std::fs::write(&output, serde_json::to_vec_pretty(&actions)?)?;
        sapiens_agent::observability::append_receipt(
            config_path,
            "computer.plan",
            "external_write",
            true,
            &format!("actions={} output={}", actions.len(), output.display()),
            None,
        )?;
        message(
            cli,
            "Plano de computer use salvo; revise-o antes de executar com computer run.",
            serde_json::json!({"output": output, "actions": actions.len(), "executed": false}),
        );
        return Ok(());
    }
    if let ComputerCommands::Auto { task, output_dir } = command.clone() {
        if task.trim().is_empty() {
            anyhow::bail!("computer auto exige uma tarefa não vazia");
        }
        let assessment = sapiens_agent::policy::assess_prompt(&task);
        if assessment.blocked {
            anyhow::bail!(
                "computer auto bloqueado por avaliação de segurança: {}",
                assessment.flags.join(", ")
            );
        }
        let approved = cli.yes || approval_active(config, "computer.action");
        if config.security.mode == "supervised" && !approved {
            anyhow::bail!("computer auto exige aprovação explícita; use --yes ou approval grant");
        }
        let actions = generate_computer_plan(config, &task).await?;
        let plan_path = workspace_path(
            &config.security.workspace,
            PathBuf::from("computer-auto-plan.json"),
        );
        let output_dir = workspace_path(&config.security.workspace, output_dir);
        let policy = Policy {
            mode: config.security.mode.clone(),
            workspace: config.security.workspace.clone(),
            allowed_domains: config.security.allowed_domains.clone(),
            allow_private_networks: config.security.allow_private_networks,
        };
        policy.check_path(&plan_path)?;
        policy.check_path(&output_dir)?;
        std::fs::write(&plan_path, serde_json::to_vec_pretty(&actions)?)?;
        sapiens_agent::observability::append_receipt(
            config_path,
            "computer.auto_plan",
            "external_write",
            approved,
            &format!("actions={} plan={}", actions.len(), plan_path.display()),
            None,
        )?;
        let adapter = WindowsComputerUse::new()?;
        let mut policy = policy;
        run_computer_sequence(
            &adapter,
            config,
            config_path,
            &mut policy,
            &plan_path,
            &output_dir,
            cli,
        )?;
        return Ok(());
    }
    let adapter = WindowsComputerUse::new()?;
    let mut policy = Policy {
        mode: config.security.mode.clone(),
        workspace: config.security.workspace.clone(),
        allowed_domains: config.security.allowed_domains.clone(),
        allow_private_networks: config.security.allow_private_networks,
    };
    if let ComputerCommands::Run {
        actions,
        output_dir,
    } = command.clone()
    {
        let actions = workspace_path(&config.security.workspace, actions);
        let output_dir = workspace_path(&config.security.workspace, output_dir);
        run_computer_sequence(
            &adapter,
            config,
            config_path,
            &mut policy,
            &actions,
            &output_dir,
            cli,
        )?;
        return Ok(());
    }
    let action = match command {
        ComputerCommands::Screenshot { output } => {
            let output = workspace_path(&config.security.workspace, output);
            policy.check_path(&output)?;
            let bytes = adapter.screenshot()?;
            std::fs::write(&output, &bytes)?;
            sapiens_agent::observability::append_receipt(
                config_path,
                "computer.screenshot",
                "read",
                true,
                "ok",
                None,
            )?;
            message(
                cli,
                "Screenshot salvo.",
                serde_json::json!({
                    "output": output,
                    "bytes": bytes.len(),
                    "executed": true,
                }),
            );
            return Ok(());
        }
        ComputerCommands::Click { x, y } => DesktopAction::Click { x, y },
        ComputerCommands::Type { text } => DesktopAction::Type { text },
        ComputerCommands::Key { key } => DesktopAction::Keypress { key },
        ComputerCommands::Wait { millis } => DesktopAction::Wait { millis },
        ComputerCommands::Plan { .. }
        | ComputerCommands::Auto { .. }
        | ComputerCommands::Run { .. }
        | ComputerCommands::EmergencyStop
        | ComputerCommands::ResetStop => unreachable!("handled before action dispatch"),
    };
    let approved = cli.yes || approval_active(config, "computer.action");
    if config.security.mode == "supervised" && !approved {
        anyhow::bail!("computer use exige aprovação; use --yes ou approval grant");
    }
    if approved {
        policy.mode = "trusted".into();
    }
    adapter.execute(action, &policy)?;
    sapiens_agent::observability::append_receipt(
        config_path,
        "computer.action",
        "external_write",
        approved,
        "ok",
        None,
    )?;
    message(
        cli,
        "Ação de computer use executada.",
        serde_json::json!({"executed": true}),
    );
    Ok(())
}

fn parse_computer_plan(answer: &str) -> Result<Vec<DesktopAction>> {
    let candidate = answer
        .trim()
        .strip_prefix("```json")
        .and_then(|value| value.strip_suffix("```"))
        .unwrap_or(answer.trim())
        .trim();
    let actions: Vec<DesktopAction> = serde_json::from_str(candidate)
        .or_else(|_| {
            let start = answer
                .find('[')
                .context("provider did not return a JSON action array")?;
            let end = answer
                .rfind(']')
                .context("provider did not close the JSON action array")?;
            serde_json::from_str(&answer[start..=end])
                .context("provider returned invalid computer plan")
        })
        .context("provider returned invalid computer plan")?;
    if actions.is_empty() {
        anyhow::bail!("computer plan cannot be empty");
    }
    if actions.len() > 100 {
        anyhow::bail!("computer plan cannot exceed 100 actions");
    }
    for action in &actions {
        sapiens_agent::computer::validate_action(action)?;
    }
    Ok(actions)
}

async fn generate_computer_plan(config: &AppConfig, task: &str) -> Result<Vec<DesktopAction>> {
    let prompt = format!(
        "Converta a tarefa abaixo em uma sequência JSON válida de ações desktop. Responda somente com um array JSON, sem markdown. Use apenas os tipos click (x,y), type (text), keypress (key), wait (millis) e screenshot. Nunca inclua credenciais ou dados que não estejam na tarefa. Não execute nada. Tarefa: {task}"
    );
    let registry = sapiens_agent::providers::ProviderRegistry::new(config.clone());
    let answer = registry.chat(None, "computer-plan", &prompt).await?;
    parse_computer_plan(&answer)
}

fn workspace_path(workspace: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    }
}

fn run_computer_sequence<A: ComputerUseAdapter>(
    adapter: &A,
    config: &AppConfig,
    config_path: &Path,
    policy: &mut Policy,
    actions_path: &Path,
    output_dir: &Path,
    cli: &Cli,
) -> Result<()> {
    policy.check_path(actions_path)?;
    policy.check_path(output_dir)?;
    let actions: Vec<DesktopAction> = serde_json::from_str(
        &std::fs::read_to_string(actions_path)
            .with_context(|| format!("read computer actions {}", actions_path.display()))?,
    )
    .context("computer actions must be a JSON array of structured actions")?;
    if actions.is_empty() {
        anyhow::bail!("computer action sequence cannot be empty");
    }
    if actions.len() > 100 {
        anyhow::bail!("computer action sequence cannot exceed 100 actions");
    }
    for action in &actions {
        sapiens_agent::computer::validate_action(action)?;
    }
    let stop_path = sapiens_agent::computer::emergency_stop_path(config_path);
    if stop_path.exists() {
        anyhow::bail!("computer sequence cancelled by emergency stop");
    }
    std::fs::create_dir_all(output_dir)?;
    let save_screenshot = |name: &str| -> Result<usize> {
        let bytes = adapter.screenshot()?;
        std::fs::write(output_dir.join(name), &bytes)?;
        Ok(bytes.len())
    };
    let mut screenshots = Vec::new();
    screenshots.push(save_screenshot("before-000.png")?);
    let mut executed = 0usize;
    for (index, action) in actions.into_iter().enumerate() {
        if stop_path.exists() {
            sapiens_agent::observability::append_receipt(
                config_path,
                "computer.sequence.cancelled",
                "destructive",
                true,
                &format!("step={index} emergency_stop=true"),
                None,
            )?;
            anyhow::bail!("computer sequence cancelled by emergency stop");
        }
        let risk = sapiens_agent::computer::requires_confirmation(&action);
        let approved = cli.yes || approval_active(config, "computer.action");
        if policy.requires_approval(risk) && !approved {
            anyhow::bail!("computer step {index} requires approval: {risk:?}");
        }
        if approved {
            policy.mode = "trusted".into();
        }
        adapter.execute(action, policy)?;
        let name = format!("after-{index:03}.png");
        screenshots.push(save_screenshot(&name)?);
        sapiens_agent::observability::append_receipt(
            config_path,
            "computer.sequence.step",
            match risk {
                sapiens_agent::policy::Risk::Read => "read",
                sapiens_agent::policy::Risk::Destructive => "destructive",
                _ => "external_write",
            },
            approved || matches!(risk, sapiens_agent::policy::Risk::Read),
            &format!("step={index} screenshot={name}"),
            None,
        )?;
        executed += 1;
    }
    message(
        cli,
        "Sequência de computer use executada com screenshots antes/depois.",
        serde_json::json!({"executed": executed, "screenshots": screenshots.len(), "output_dir": output_dir}),
    );
    Ok(())
}

fn mcp_command(
    config: &mut AppConfig,
    config_path: &Path,
    command: McpCommands,
    cli: &Cli,
) -> Result<()> {
    match command {
        McpCommands::Servers => {
            let servers = config
                .mcp_servers
                .iter()
                .map(|server| {
                    serde_json::json!({
                        "name": server.name,
                        "command": sapiens_agent::observability::redact(&server.command),
                        "transport": server.transport,
                        "url_configured": !server.url.is_empty(),
                        "credential_env": if server.credential_env.is_empty() {
                            serde_json::Value::Null
                        } else {
                            serde_json::Value::String(format!("env:{}", server.credential_env))
                        },
                        "enabled": server.enabled,
                        "allowlist": server.allowlist,
                        "version": server.version,
                    })
                })
                .collect::<Vec<_>>();
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&servers)?);
            } else if servers.is_empty() {
                println!("Nenhum servidor MCP configurado.");
            } else {
                for server in servers {
                    println!(
                        "{}\t{}\tv{}\t{}",
                        server["name"].as_str().unwrap_or_default(),
                        if server["enabled"].as_bool().unwrap_or(false) {
                            "enabled"
                        } else {
                            "disabled"
                        },
                        server["version"].as_str().unwrap_or("1"),
                        server["allowlist"]
                            .as_array()
                            .map(|items| items.len())
                            .unwrap_or_default()
                    );
                }
            }
            return Ok(());
        }
        McpCommands::Health { server, timeout } => {
            if cli.dry_run {
                message(
                    cli,
                    "Dry-run: health check MCP não executado.",
                    serde_json::json!({"server": server, "network_called": false}),
                );
                return Ok(());
            }
            if !config.features.mcp {
                anyhow::bail!("MCP está desativado; habilite com sapiens-agent skills enable mcp");
            }
            let approved = cli.yes || approval_active(config, "mcp.execute");
            if config.security.mode == "supervised" && !approved {
                anyhow::bail!(
                    "MCP inicia um processo externo; repita com --yes após revisar o comando"
                );
            }
            let stored = config
                .mcp_servers
                .iter()
                .find(|item| item.name == server)
                .context("servidor MCP configurado não encontrado")?;
            if !stored.enabled {
                anyhow::bail!("servidor MCP está desabilitado: {server}");
            }
            let server_config = McpServer {
                name: stored.name.clone(),
                command: stored.command.clone(),
                allowed: true,
                allowlist: stored.allowlist.clone(),
                transport: stored.transport.clone(),
                url: stored.url.clone(),
                credential_env: stored.credential_env.clone(),
            };
            if !server_config.transport.eq_ignore_ascii_case("stdio") {
                let remote_url = server_config.url.clone();
                sapiens_agent::mcp::validate_remote_url(&remote_url)?;
                let policy = Policy {
                    mode: config.security.mode.clone(),
                    workspace: config.security.workspace.clone(),
                    allowed_domains: config.security.allowed_domains.clone(),
                    allow_private_networks: config.security.allow_private_networks,
                };
                policy.check_url(&remote_url)?;
            }
            let started = std::time::Instant::now();
            let tools =
                sapiens_agent::mcp::list_tools(&server_config, Duration::from_secs(timeout))?;
            let healthy = tools.iter().filter(|tool| tool.allowed).count();
            let result = serde_json::json!({
                "server": server,
                "version": stored.version,
                "healthy": true,
                "latency_ms": started.elapsed().as_millis(),
                "tools": tools.len(),
                "allowed_tools": healthy,
            });
            sapiens_agent::observability::append_receipt(
                config_path,
                "mcp.health",
                "read",
                approved,
                "ok",
                None,
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!(
                    "{}: saudável ({} ms, {}/{} ferramentas allowlisted)",
                    server,
                    result["latency_ms"].as_u64().unwrap_or_default(),
                    healthy,
                    tools.len()
                );
            }
            return Ok(());
        }
        McpCommands::Diagnose { server, timeout } => {
            if cli.dry_run {
                message(
                    cli,
                    "Dry-run: diagnóstico MCP não executado.",
                    serde_json::json!({"server": server, "network_called": false}),
                );
                return Ok(());
            }
            if !config.features.mcp {
                anyhow::bail!("MCP está desativado; habilite com sapiens-agent skills enable mcp");
            }
            let approved = cli.yes || approval_active(config, "mcp.execute");
            if config.security.mode == "supervised" && !approved {
                anyhow::bail!(
                    "MCP inicia um processo externo; repita com --yes após revisar o comando"
                );
            }
            let stored = config
                .mcp_servers
                .iter()
                .find(|item| item.name == server)
                .context("servidor MCP configurado não encontrado")?;
            if !stored.enabled {
                anyhow::bail!("servidor MCP está desabilitado: {server}");
            }
            let server_config = McpServer {
                name: stored.name.clone(),
                command: stored.command.clone(),
                allowed: true,
                allowlist: stored.allowlist.clone(),
                transport: stored.transport.clone(),
                url: stored.url.clone(),
                credential_env: stored.credential_env.clone(),
            };
            if !server_config.transport.eq_ignore_ascii_case("stdio") {
                let remote_url = server_config.url.clone();
                sapiens_agent::mcp::validate_remote_url(&remote_url)?;
                let policy = Policy {
                    mode: config.security.mode.clone(),
                    workspace: config.security.workspace.clone(),
                    allowed_domains: config.security.allowed_domains.clone(),
                    allow_private_networks: config.security.allow_private_networks,
                };
                policy.check_url(&remote_url)?;
            }
            let diagnostics = sapiens_agent::mcp::diagnose(
                &server_config,
                Duration::from_secs(timeout.clamp(1, 300)),
            )?;
            sapiens_agent::observability::append_receipt(
                config_path,
                "mcp.diagnose",
                "read",
                approved,
                "ok",
                None,
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&diagnostics)?);
            } else {
                println!(
                    "{}: sessão {} saudável ({} ms, {}/{} ferramentas allowlisted)",
                    diagnostics.server,
                    diagnostics.session_mode,
                    diagnostics.latency_ms,
                    diagnostics.allowed_tools,
                    diagnostics.tools
                );
                println!(
                    "transporte={} versões={} etapas={}",
                    diagnostics.transport,
                    diagnostics.protocol_versions_supported.join(","),
                    diagnostics.lifecycle.join(" -> ")
                );
            }
            return Ok(());
        }
        McpCommands::Add {
            name,
            command,
            url,
            transport,
            credential_env,
            allow,
            version,
        } => {
            if name.trim().is_empty() {
                anyhow::bail!("nome do servidor MCP é obrigatório");
            }
            let transport = transport.to_ascii_lowercase();
            if transport != "stdio" && transport != "http" && transport != "sse" {
                anyhow::bail!("transporte MCP deve ser stdio, http ou sse");
            }
            if transport == "stdio" && command.as_deref().unwrap_or_default().trim().is_empty() {
                anyhow::bail!("transporte stdio exige --command");
            }
            if transport != "stdio" {
                let remote_url = url.as_deref().unwrap_or_default();
                if remote_url.trim().is_empty() {
                    anyhow::bail!("transporte remoto exige --url");
                }
                let policy = Policy {
                    mode: config.security.mode.clone(),
                    workspace: config.security.workspace.clone(),
                    allowed_domains: config.security.allowed_domains.clone(),
                    allow_private_networks: config.security.allow_private_networks,
                };
                sapiens_agent::mcp::validate_remote_url(remote_url)?;
                policy.check_url(remote_url)?;
            }
            if config.mcp_servers.iter().any(|server| server.name == name) {
                anyhow::bail!("servidor MCP já existe: {name}");
            }
            let allowlist = parse_mcp_allowlist(&allow)?;
            if allowlist.is_empty() {
                anyhow::bail!("MCP exige allowlist explícita de ferramentas");
            }
            if cli.dry_run {
                message(
                    cli,
                    "Dry-run: servidor MCP não foi salvo.",
                    serde_json::json!({"name": name, "saved": false}),
                );
                return Ok(());
            }
            config.mcp_servers.push(McpServerConfig {
                name: name.clone(),
                command: command.unwrap_or_default(),
                enabled: true,
                allowlist,
                version,
                transport,
                url: url.unwrap_or_default(),
                credential_env,
            });
            config.features.mcp = true;
            config::save(config_path, config)?;
            sapiens_agent::observability::append_receipt(
                config_path,
                "mcp.server.add",
                "external_write",
                cli.yes,
                "ok",
                None,
            )?;
            message(
                cli,
                "Servidor MCP salvo; execução continuará exigindo aprovação.",
                serde_json::json!({"name": name, "enabled": true}),
            );
            return Ok(());
        }
        McpCommands::Remove { name } => {
            if cli.dry_run {
                message(
                    cli,
                    "Dry-run: servidor MCP não foi removido.",
                    serde_json::json!({"name": name, "deleted": false}),
                );
                return Ok(());
            }
            if !cli.yes {
                anyhow::bail!("remoção de servidor MCP exige --yes após revisar o nome");
            }
            let before = config.mcp_servers.len();
            config.mcp_servers.retain(|server| server.name != name);
            if before == config.mcp_servers.len() {
                anyhow::bail!("servidor MCP não encontrado: {name}");
            }
            config.features.mcp = config.mcp_servers.iter().any(|server| server.enabled);
            config::save(config_path, config)?;
            sapiens_agent::observability::append_receipt(
                config_path,
                "mcp.server.remove",
                "destructive",
                true,
                "ok",
                None,
            )?;
            message(
                cli,
                "Servidor MCP removido.",
                serde_json::json!({"name": name, "deleted": true}),
            );
            return Ok(());
        }
        _ => {}
    }

    if cli.dry_run {
        message(
            cli,
            "Dry-run: MCP não executado.",
            serde_json::json!({"executed": false}),
        );
        return Ok(());
    }
    if !config.features.mcp {
        anyhow::bail!("MCP está desativado; habilite com sapiens-agent skills enable mcp");
    }
    let (command_line, server_name, allow, timeout, call) = match command {
        McpCommands::List {
            command,
            server,
            allow,
            timeout,
        } => (command, server, allow, timeout, None),
        McpCommands::Call {
            command,
            server,
            tool,
            arguments,
            allow,
            timeout,
        } => (command, server, allow, timeout, Some((tool, arguments))),
        _ => unreachable!(),
    };
    let approved = cli.yes || approval_active(config, "mcp.execute");
    if config.security.mode == "supervised" && !approved {
        anyhow::bail!("MCP inicia um processo externo; use --yes ou approval grant");
    }
    let (name, command_line, allowlist, transport, url, credential_env) =
        if let Some(command_line) = command_line {
            (
                server_name.unwrap_or_else(|| "cli".into()),
                command_line,
                parse_mcp_allowlist(&allow)?,
                "stdio".into(),
                String::new(),
                String::new(),
            )
        } else {
            let name = server_name.context("informe --command ou --server")?;
            let stored = config
                .mcp_servers
                .iter()
                .find(|server| server.name == name)
                .context("servidor MCP configurado não encontrado")?;
            if !stored.enabled {
                anyhow::bail!("servidor MCP está desabilitado: {name}");
            }
            let allowlist = if allow.trim().is_empty() {
                stored.allowlist.clone()
            } else {
                parse_mcp_allowlist(&allow)?
            };
            if !stored.transport.eq_ignore_ascii_case("stdio") {
                let policy = Policy {
                    mode: config.security.mode.clone(),
                    workspace: config.security.workspace.clone(),
                    allowed_domains: config.security.allowed_domains.clone(),
                    allow_private_networks: config.security.allow_private_networks,
                };
                sapiens_agent::mcp::validate_remote_url(&stored.url)?;
                policy.check_url(&stored.url)?;
            }
            (
                name,
                stored.command.clone(),
                allowlist,
                stored.transport.clone(),
                stored.url.clone(),
                stored.credential_env.clone(),
            )
        };
    if allowlist.is_empty() {
        anyhow::bail!("MCP exige allowlist explícita de ferramentas");
    }
    let server = McpServer {
        name,
        command: command_line,
        allowed: true,
        allowlist,
        transport,
        url,
        credential_env,
    };
    match call {
        None => {
            let tools = sapiens_agent::mcp::list_tools(&server, Duration::from_secs(timeout))?;
            sapiens_agent::observability::append_receipt(
                config_path,
                "mcp.list",
                "read",
                true,
                "ok",
                None,
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&tools)?);
            } else {
                for tool in tools {
                    println!(
                        "{}\t{}\t{}",
                        tool.name,
                        if tool.allowed { "allowed" } else { "blocked" },
                        tool.description
                    );
                }
            }
        }
        Some((name, arguments)) => {
            let arguments: serde_json::Value =
                serde_json::from_str(&arguments).context("MCP arguments must be valid JSON")?;
            let tool = McpTool {
                server: server.name.clone(),
                name,
                allowed: true,
                description: String::new(),
                server_command: server.command.clone(),
            };
            let result = sapiens_agent::mcp::call_tool(
                &server,
                &tool,
                arguments,
                Duration::from_secs(timeout),
            )?;
            sapiens_agent::observability::append_receipt(
                config_path,
                "mcp.call",
                "external_write",
                approved,
                "ok",
                None,
            )?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }
    Ok(())
}

fn parse_mcp_allowlist(value: &str) -> Result<Vec<String>> {
    Ok(value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect())
}

fn plugin_command(
    config: &AppConfig,
    config_path: &Path,
    command: PluginCommands,
    cli: &Cli,
) -> Result<()> {
    let policy = Policy {
        mode: config.security.mode.clone(),
        workspace: config.security.workspace.clone(),
        allowed_domains: config.security.allowed_domains.clone(),
        allow_private_networks: config.security.allow_private_networks,
    };
    match command {
        PluginCommands::Verify { manifest } => {
            let verification = plugins::verify(&manifest, &policy)?;
            sapiens_agent::observability::append_receipt(
                config_path,
                "plugin.verify",
                "read",
                true,
                "ok",
                None,
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&verification)?);
            } else {
                println!(
                    "Plugin verificado: {} {} ({})",
                    verification.name, verification.version, verification.sha256
                );
                println!(
                    "Execução permanece desabilitada até assinatura válida, origem confiável e sandbox."
                );
            }
        }
        PluginCommands::List { directory } => {
            let directory = directory.unwrap_or_else(|| config.security.workspace.join("plugins"));
            let entries = plugins::list(&directory, &policy)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else if entries.is_empty() {
                println!("Nenhum plugin instalado.");
            } else {
                for entry in entries {
                    println!("{}\tdesabilitado (sandbox pendente)", entry.display());
                }
            }
        }
        PluginCommands::Install {
            manifest,
            destination,
        } => {
            install_or_update_plugin(config, config_path, manifest, destination, false, cli)?;
        }
        PluginCommands::Update {
            manifest,
            destination,
        } => {
            install_or_update_plugin(config, config_path, manifest, destination, true, cli)?;
        }
        PluginCommands::Remove { path } => {
            policy.check_path(&path)?;
            if cli.dry_run {
                message(
                    cli,
                    "Dry-run: plugin não removido.",
                    serde_json::json!({"path": path, "deleted": false}),
                );
            } else {
                let approved = cli.yes || approval_active(config, "plugin.remove");
                if !approved {
                    anyhow::bail!("remoção de plugin exige aprovação; use --yes ou approval grant");
                }
                plugins::remove(&path, &policy)?;
                sapiens_agent::observability::append_receipt(
                    config_path,
                    "plugin.remove",
                    "destructive",
                    approved,
                    "ok",
                    None,
                )?;
                message(
                    cli,
                    "Plugin removido.",
                    serde_json::json!({"path": path, "deleted": true}),
                );
            }
        }
    }
    Ok(())
}

fn install_or_update_plugin(
    config: &AppConfig,
    config_path: &Path,
    manifest: PathBuf,
    destination: Option<PathBuf>,
    update: bool,
    cli: &Cli,
) -> Result<()> {
    let policy = Policy {
        mode: config.security.mode.clone(),
        workspace: config.security.workspace.clone(),
        allowed_domains: config.security.allowed_domains.clone(),
        allow_private_networks: config.security.allow_private_networks,
    };
    let action = if update {
        "plugin.update"
    } else {
        "plugin.install"
    };
    let approved = cli.yes || approval_active(config, action);
    if config.security.mode == "supervised" && !approved {
        anyhow::bail!("{action} exige aprovação; use --yes ou approval grant");
    }
    let destination = destination.unwrap_or_else(|| config.security.workspace.join("plugins"));
    if cli.dry_run {
        let verification = plugins::verify(&manifest, &policy)?;
        message(
            cli,
            "Dry-run: plugin não foi instalado.",
            serde_json::json!({
                "name": verification.name,
                "version": verification.version,
                "destination": destination,
                "installed": false,
                "updated": update,
            }),
        );
        return Ok(());
    }
    let installation = plugins::install(&manifest, &destination, &policy, update)?;
    sapiens_agent::observability::append_receipt(
        config_path,
        action,
        "external_write",
        approved,
        &format!(
            "name={} version={} enabled=false",
            installation.name, installation.version
        ),
        None,
    )?;
    message(
        cli,
        "Plugin instalado em modo desabilitado; sandbox ainda é necessária.",
        serde_json::to_value(installation)?,
    );
    Ok(())
}

async fn run_shell(
    config: &AppConfig,
    config_path: &Path,
    command: String,
    timeout_secs: u64,
    cli: &Cli,
) -> Result<()> {
    if !config.features.shell {
        anyhow::bail!("shell está desativado; habilite com sapiens-agent skills enable shell");
    }
    if timeout_secs == 0 || timeout_secs > 3600 {
        anyhow::bail!("timeout deve estar entre 1 e 3600 segundos");
    }
    if cli.dry_run {
        message(
            cli,
            "Dry-run: comando não executado.",
            serde_json::json!({"command": command, "executed": false}),
        );
        return Ok(());
    }
    let approved = cli.yes || approval_active(config, "shell.exec");
    if config.security.mode == "supervised" && !approved {
        anyhow::bail!("shell exige aprovação; repita com --yes após revisar o comando");
    }
    let policy = sapiens_agent::policy::Policy {
        mode: config.security.mode.clone(),
        workspace: config.security.workspace.clone(),
        allowed_domains: config.security.allowed_domains.clone(),
        allow_private_networks: config.security.allow_private_networks,
    };
    let output = sapiens_agent::shell::run_with_limits(
        &command,
        &config.security.workspace,
        &policy,
        Duration::from_secs(timeout_secs),
        &config.shell.allowlist,
        config.shell.max_output_bytes,
        sapiens_agent::shell::ShellLimits {
            max_memory_mb: config.shell.max_memory_mb,
            max_processes: config.shell.max_processes,
            max_cpu_secs: config.shell.max_cpu_secs,
        },
    )
    .await?;
    sapiens_agent::observability::append_receipt(
        config_path,
        "shell.exec",
        "external_write",
        approved,
        "ok",
        None,
    )?;
    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "command": command,
                "workspace": config.security.workspace,
                "output": output,
                "executed": true,
            }))?
        );
    } else {
        print!("{output}");
    }
    Ok(())
}

async fn run_chat(
    config: AppConfig,
    config_path: &Path,
    provider: Option<String>,
    task: String,
    image_paths: Vec<PathBuf>,
    prompt: Option<String>,
) -> Result<()> {
    let registry = ProviderRegistry::new(config);
    let identity_context = config_path
        .parent()
        .map(sapiens_agent::identity::effective_prompt)
        .transpose()?
        .flatten();
    let mut images = image_paths
        .iter()
        .map(|path| sapiens_agent::providers::image_input_from_path(path))
        .collect::<Result<Vec<_>>>()?;
    let one_shot = prompt.as_deref().is_some_and(|value| !value.is_empty());
    let mut input = prompt.unwrap_or_default();
    if input.is_empty() {
        println!("Chat do Sapiens Agent. Digite /exit para sair.");
    }
    loop {
        if input.is_empty() {
            print!("> ");
            std::io::stdout().flush()?;
            std::io::stdin().read_line(&mut input)?;
            input = input.trim_end().to_string();
        }
        if input == "/exit" || input == "/quit" {
            break;
        }
        if !input.is_empty() {
            let assessment = sapiens_agent::policy::assess_prompt(&input);
            if assessment.blocked {
                eprintln!(
                    "erro: solicitação bloqueada pela policy de segurança ({}): {}",
                    assessment.score,
                    assessment.flags.join(", ")
                );
                input.clear();
                continue;
            }
            match registry
                .chat_with_context_and_images(
                    provider.as_deref(),
                    &task,
                    &sapiens_agent::policy::user_content_for_model(&input),
                    identity_context.as_deref(),
                    &images,
                )
                .await
            {
                Ok(answer) => println!("{answer}"),
                Err(error) => eprintln!("erro: {error:#}"),
            }
            images.clear();
        }
        if one_shot {
            break;
        }
        input.clear();
    }
    Ok(())
}

fn bind_for(config: &AppConfig, port: Option<u16>) -> Result<String> {
    Ok(port
        .map(|p| format!("127.0.0.1:{p}"))
        .unwrap_or_else(|| config.server.bind.clone()))
}

async fn run_server(
    config: AppConfig,
    path: &Path,
    bind: String,
    launch_browser: bool,
) -> Result<()> {
    let pid_path = path.with_file_name("sapiens-agent.pid");
    if pid_path.exists() {
        let stale = std::fs::read_to_string(&pid_path)
            .ok()
            .map(|pid| !pid_is_running(pid.trim()))
            .unwrap_or(true);
        if stale {
            let _ = std::fs::remove_file(&pid_path);
        } else {
            anyhow::bail!(
                "Sapiens Agent já parece estar iniciado; use sapiens-agent status ou stop"
            );
        }
    }
    std::fs::write(&pid_path, std::process::id().to_string())?;
    append_log(path, &format!("start bind={bind}"))?;
    if launch_browser {
        open_browser(&format!("http://{bind}/"));
    }
    let result = sapiens_agent::gateway::serve(config, bind, path.to_path_buf()).await;
    append_log(path, "stop")?;
    let _ = std::fs::remove_file(pid_path);
    result
}

fn append_log(config_path: &Path, event: &str) -> Result<()> {
    use std::fs::OpenOptions;
    let log_path = config_path.with_file_name("sapiens-agent.log");
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    writeln!(file, "{timestamp}\t{event}")?;
    Ok(())
}

fn open_browser(url: &str) {
    #[cfg(windows)]
    {
        let _ = ProcessCommand::new("cmd")
            .args(["/C", "start", "", url])
            .spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = ProcessCommand::new("xdg-open").arg(url).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = ProcessCommand::new("open").arg(url).spawn();
    }
}

fn stop_server(path: &Path, cli: &Cli) -> Result<()> {
    let pid_path = path.with_file_name("sapiens-agent.pid");
    if !pid_path.exists() {
        message(
            cli,
            "Nenhuma instância encontrada.",
            serde_json::json!({"running": false}),
        );
        return Ok(());
    }
    let pid = std::fs::read_to_string(&pid_path)?.trim().to_string();
    if !pid_is_running(&pid) {
        let _ = std::fs::remove_file(pid_path);
        message(
            cli,
            "Estado antigo removido; nenhuma instância estava rodando.",
            serde_json::json!({"pid": pid, "running": false, "stale": true}),
        );
        return Ok(());
    }
    #[cfg(windows)]
    let status = ProcessCommand::new("taskkill")
        .args(["/PID", &pid, "/T", "/F"])
        .status()?;
    #[cfg(not(windows))]
    let status = ProcessCommand::new("kill").args(["-TERM", &pid]).status()?;
    let _ = std::fs::remove_file(pid_path);
    message(
        cli,
        "Sapiens Agent encerrado.",
        serde_json::json!({"pid": pid, "stopped": status.success()}),
    );
    Ok(())
}

fn pid_is_running(pid: &str) -> bool {
    if pid.parse::<u32>().is_err() {
        return false;
    }
    #[cfg(windows)]
    {
        ProcessCommand::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).contains(pid))
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        ProcessCommand::new("kill")
            .args(["-0", pid])
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
}

fn print_status(config: &AppConfig, path: &Path, cli: &Cli) -> Result<()> {
    let running = path.with_file_name("sapiens-agent.pid").exists();
    let active_model = config
        .active_provider
        .as_deref()
        .and_then(|alias| config.providers.iter().find(|item| item.alias == alias))
        .map(|provider| provider.model.clone());
    let enabled_channels: Vec<String> = config
        .channels
        .iter()
        .filter(|channel| channel.enabled)
        .map(|channel| channel.name.clone())
        .collect();
    let memory_path = path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("memory.jsonl");
    let data = serde_json::json!({
        "initialized": config.initialized,
        "config": path,
        "workspace": config.security.workspace,
        "provider": config.active_provider,
        "model": active_model,
        "providers": config.providers.len(),
        "gateway": config.server.bind,
        "gateway_auth": if config.server.auth_env.is_empty() { "disabled" } else { "enabled" },
        "running": running,
        "channels": {"configured": config.channels.len(), "enabled": enabled_channels},
        "schedules": config.schedules.len(),
        "scheduler_limits": config.scheduler,
        "mcp_servers": config.mcp_servers.len(),
        "approvals": config.approvals.len(),
        "memory": {"enabled": config.features.memory, "path": memory_path, "retention_days": config.memory_retention_days},
        "multimodal": {"images": "ready for compatible providers", "audio": "optional/disabled", "video": "optional/disabled"},
        "browser": {
            "enabled": config.features.browser,
            "adapter": "playwright-cli",
            "advanced_actions": "implemented",
            "browser_use": "optional/disabled",
            "stagehand": "optional/disabled"
        },
        "permissions": {"mode": config.security.mode, "private_networks": config.security.allow_private_networks, "max_requests_per_minute": config.security.max_requests_per_minute},
        "features": config.features
    });
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&data)?);
    } else {
        println!(
            "Sapiens Agent\n  status: {}\n  provider: {}\n  gateway: http://{}\n  tarefas: {}\n  workspace: {}",
            if running { "rodando" } else { "parado" },
            config
                .active_provider
                .as_deref()
                .unwrap_or("não configurado"),
            config.server.bind,
            config.schedules.len(),
            config.security.workspace.display()
        );
        println!(
            "  canais: {}/{} habilitados\n  memória: {}\n  multimodal: imagens prontas; áudio/vídeo opcionais\n  scheduler: até {} tarefas\n  segurança: {} ({} req/min)",
            enabled_channels.len(),
            config.channels.len(),
            if config.features.memory {
                "ligada"
            } else {
                "desligada"
            },
            config.scheduler.max_concurrent,
            config.security.mode,
            config.security.max_requests_per_minute
        );
    }
    Ok(())
}

fn doctor(config: &AppConfig, path: &Path, cli: &Cli) -> Result<()> {
    let mut warnings = Vec::new();
    if !config.security.workspace.exists() {
        warnings.push("workspace não existe".to_string());
    }
    if config.providers.is_empty() {
        warnings.push("nenhum provider configurado".to_string());
    }
    if let Some(alias) = &config.active_provider
        && let Some(provider) = config.providers.iter().find(|p| &p.alias == alias)
    {
        if provider.base_url.is_empty() {
            warnings.push(format!("provider {alias} sem base_url"));
        }
        if provider.model.is_empty() {
            warnings.push(format!("provider {alias} sem modelo"));
        }
    }
    let data = serde_json::json!({"ok": warnings.is_empty(), "warnings": warnings, "config": path});
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&data)?);
    } else if data["ok"].as_bool().unwrap_or(false) {
        println!("Doctor: tudo certo.");
    } else {
        println!("Doctor encontrou avisos:");
        for warning in data["warnings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
        {
            println!("  - {warning}");
        }
    }
    Ok(())
}

fn schedule_command(
    config: &mut AppConfig,
    path: &Path,
    command: ScheduleCommands,
    cli: &Cli,
) -> Result<()> {
    match command {
        ScheduleCommands::List => {
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&config.schedules)?);
            } else if config.schedules.is_empty() {
                println!("Nenhuma tarefa agendada.");
            } else {
                for job in &config.schedules {
                    println!(
                        "{}\t{}\t{}\t{}\twebhook={}\tchannel={}\tdelivery={}",
                        job.id,
                        job.kind,
                        job.value,
                        job.task,
                        job.webhook_url.is_some(),
                        job.channel_name.as_deref().unwrap_or("-"),
                        job.last_delivery.as_deref().unwrap_or("-")
                    );
                }
            }
        }
        ScheduleCommands::Add {
            every,
            task,
            webhook,
            channel,
            recipient,
        } => add_schedule(
            config, path, "every", every, task, webhook, channel, recipient, cli,
        )?,
        ScheduleCommands::Once {
            at,
            task,
            webhook,
            channel,
            recipient,
        } => add_schedule(
            config, path, "once", at, task, webhook, channel, recipient, cli,
        )?,
        ScheduleCommands::Cron {
            expression,
            task,
            webhook,
            channel,
            recipient,
        } => add_schedule(
            config, path, "cron", expression, task, webhook, channel, recipient, cli,
        )?,
        ScheduleCommands::Remove { id } => {
            let before = config.schedules.len();
            config.schedules.retain(|job| job.id != id);
            if before == config.schedules.len() {
                anyhow::bail!("tarefa não encontrada: {id}");
            }
            config::save(path, config)?;
            message(cli, "Tarefa removida.", serde_json::json!({"id": id}));
        }
        ScheduleCommands::Pause { id } => set_schedule_enabled(config, path, &id, false, cli)?,
        ScheduleCommands::Resume { id } => set_schedule_enabled(config, path, &id, true, cli)?,
        ScheduleCommands::Cancel { id } => cancel_schedule(config, path, &id, cli)?,
    }
    Ok(())
}

fn set_schedule_enabled(
    config: &mut AppConfig,
    path: &Path,
    id: &str,
    enabled: bool,
    cli: &Cli,
) -> Result<()> {
    if cli.dry_run {
        message(
            cli,
            "Dry-run: estado da tarefa não foi alterado.",
            serde_json::json!({"id": id, "enabled": enabled, "saved": false}),
        );
        return Ok(());
    }
    let job = config
        .schedules
        .iter_mut()
        .find(|job| job.id == id)
        .ok_or_else(|| anyhow::anyhow!("tarefa não encontrada: {id}"))?;
    job.enabled = enabled;
    job.cancel_requested = false;
    if enabled && job.run_state.eq_ignore_ascii_case("interrupted") {
        job.run_state = "idle".into();
        job.active_run_id = None;
        job.started_at = None;
        job.last_error = None;
    }
    config::save(path, config)?;
    message(
        cli,
        if enabled {
            "Tarefa retomada."
        } else {
            "Tarefa pausada."
        },
        serde_json::json!({"id": id, "enabled": enabled}),
    );
    Ok(())
}

fn cancel_schedule(config: &mut AppConfig, path: &Path, id: &str, cli: &Cli) -> Result<()> {
    if cli.dry_run {
        message(
            cli,
            "Dry-run: cancelamento não foi salvo.",
            serde_json::json!({"id": id, "saved": false}),
        );
        return Ok(());
    }
    let job = config
        .schedules
        .iter_mut()
        .find(|job| job.id == id)
        .ok_or_else(|| anyhow::anyhow!("tarefa não encontrada: {id}"))?;
    job.cancel_requested = true;
    job.enabled = false;
    config::save(path, config)?;
    message(
        cli,
        "Cancelamento solicitado; a execução em andamento será interrompida.",
        serde_json::json!({"id": id, "cancel_requested": true, "enabled": false}),
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn add_schedule(
    config: &mut AppConfig,
    path: &Path,
    kind: &str,
    value: String,
    task: String,
    webhook: Option<String>,
    channel: Option<String>,
    recipient: Option<String>,
    cli: &Cli,
) -> Result<()> {
    if value.trim().is_empty() || task.trim().is_empty() {
        anyhow::bail!("schedule exige valor e task não vazios");
    }
    if let Some(url) = &webhook {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            anyhow::bail!("schedule webhook deve usar http:// ou https://");
        }
        let policy = Policy {
            mode: config.security.mode.clone(),
            workspace: config.security.workspace.clone(),
            allowed_domains: config.security.allowed_domains.clone(),
            allow_private_networks: config.security.allow_private_networks,
        };
        policy.check_url(url)?;
    }
    if channel.is_some() != recipient.is_some() {
        anyhow::bail!("entrega em canal exige --channel e --recipient juntos");
    }
    if let (Some(channel_name), Some(channel_recipient)) = (&channel, &recipient) {
        let configured = config
            .channels
            .iter()
            .find(|item| &item.name == channel_name)
            .with_context(|| format!("canal não encontrado: {channel_name}"))?;
        if !configured.enabled {
            anyhow::bail!("canal está desabilitado: {channel_name}");
        }
        if configured.kind != "telegram"
            && configured.kind != "matrix"
            && configured.kind != "whatsapp"
            && configured.kind != "signal"
            && !channels::is_webhook_adapter(&configured.kind)
        {
            anyhow::bail!(
                "canal {} ainda não possui adapter de entrega instalado",
                configured.kind
            );
        }
        if !channels::webhook_recipient_allowed(configured, channel_recipient) {
            anyhow::bail!("destinatário não está allowlisted no canal: {channel_recipient}");
        }
        if config.security.mode == "supervised" && !cli.yes {
            anyhow::bail!("entrega em canal exige --yes após revisar o destinatário");
        }
    }
    match kind {
        "every" => {
            sapiens_agent::scheduler::parse_interval(&value)?;
        }
        "once" => {
            sapiens_agent::scheduler::parse_once(&value)?;
        }
        "cron" => {
            sapiens_agent::scheduler::parse_cron(&value)?;
        }
        _ => anyhow::bail!("tipo de schedule não suportado: {kind}"),
    };
    let lower = task.to_ascii_lowercase();
    let risky = [
        "apagar", "excluir", "deletar", "enviar", "pagar", "comprar", "publicar",
    ]
    .iter()
    .any(|word| lower.contains(word));
    if risky && config.security.mode == "supervised" && !cli.yes {
        anyhow::bail!("tarefa contém ação de risco; revise e repita com --yes");
    }
    let id = format!(
        "job-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis()
    );
    config.schedules.push(ScheduleConfig {
        id: id.clone(),
        kind: kind.into(),
        value,
        task,
        enabled: true,
        created_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        last_run: None,
        run_count: 0,
        last_error: None,
        webhook_url: webhook,
        retry_limit: 2,
        timeout_secs: 300,
        last_result: None,
        last_delivery: None,
        cancel_requested: false,
        channel_name: channel,
        channel_recipient: recipient,
        run_state: "idle".into(),
        active_run_id: None,
        started_at: None,
    });
    if cli.dry_run {
        config.schedules.pop();
        message(
            cli,
            "Dry-run: tarefa não foi salva.",
            serde_json::json!({"id": id, "saved": false}),
        );
        return Ok(());
    }
    config::save(path, config)?;
    message(cli, "Tarefa agendada.", serde_json::json!({"id": id}));
    Ok(())
}

async fn channel_command(
    config: &mut AppConfig,
    path: &Path,
    command: ChannelCommands,
    cli: &Cli,
) -> Result<()> {
    match command {
        ChannelCommands::List => {
            if cli.json {
                let entries: Vec<serde_json::Value> = channels::catalog()
                    .iter()
                    .map(|spec| {
                        let configured: Vec<&ChannelConfig> = config
                            .channels
                            .iter()
                            .filter(|item| item.kind == spec.id)
                            .collect();
                        serde_json::json!({
                            "id": spec.id,
                            "label": spec.label,
                            "transport": spec.transport,
                            "capabilities": spec.capabilities,
                            "credential_hint": spec.credential_hint,
                            "adapter": spec.adapter,
                            "configured": configured,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else {
                println!("CANAL\tADAPTER\tCONFIGURADO\tTRANSPORTE\tCAPACIDADES\tCREDENCIAL");
                for spec in channels::catalog() {
                    let configured = config
                        .channels
                        .iter()
                        .filter(|item| item.kind == spec.id)
                        .map(|item| format!("{}{}", item.name, if item.enabled { "*" } else { "" }))
                        .collect::<Vec<_>>();
                    let configured_text = if configured.is_empty() {
                        "-".to_string()
                    } else {
                        configured.join(",")
                    };
                    println!(
                        "{}\t{}\t{}\t{}\t{}\t{}",
                        spec.id,
                        spec.adapter,
                        configured_text,
                        spec.transport,
                        spec.capabilities,
                        spec.credential_hint
                    );
                }
                println!("* = habilitado; 'opcional' = adapter externo ainda não instalado.");
            }
        }
        ChannelCommands::Add { kind, name } => {
            let spec = channels::find_spec(&kind)?;
            let normalized = spec.id.to_string();
            let channel_name = name.unwrap_or_else(|| normalized.clone());
            if cli.dry_run {
                message(
                    cli,
                    "Dry-run: canal não adicionado.",
                    serde_json::json!({"name": channel_name, "kind": normalized, "saved": false}),
                );
                return Ok(());
            }
            if config.channels.iter().any(|item| item.name == channel_name) {
                anyhow::bail!("canal já existe: {channel_name}");
            }
            let credential_env = if spec.credential_hint.starts_with("SAPIENS_") {
                spec.credential_hint.to_string()
            } else {
                String::new()
            };
            config.channels.push(ChannelConfig {
                name: channel_name.clone(),
                kind: normalized,
                enabled: false,
                credential_env,
                allowlist: vec![],
            });
            config::save(path, config)?;
            message(
                cli,
                "Canal adicionado; execute channel configure para concluir.",
                serde_json::json!({"name": channel_name}),
            );
        }
        ChannelCommands::Configure { name } => {
            if cli.dry_run {
                message(
                    cli,
                    "Dry-run: canal não configurado.",
                    serde_json::json!({"name": name, "saved": false}),
                );
                return Ok(());
            }
            let index = config
                .channels
                .iter()
                .position(|item| item.name == name)
                .ok_or_else(|| anyhow::anyhow!("canal não encontrado: {name}"))?;
            let kind = config.channels[index].kind.clone();
            let spec = channels::find_spec(&kind)?;
            let credential_default = if config.channels[index].credential_env.is_empty() {
                if spec.credential_hint.starts_with("SAPIENS_") {
                    spec.credential_hint
                } else {
                    ""
                }
            } else {
                config.channels[index].credential_env.as_str()
            };
            let allow_default = config.channels[index].allowlist.join(",");
            let credential_env = ask(
                "Variável da credencial (nunca o segredo)",
                credential_default,
                cli.yes,
            )?;
            let allowlist = ask(
                "Allowlist de remetentes (separada por vírgula)",
                &allow_default,
                cli.yes,
            )?;
            let enabled = ask_bool(
                "Habilitar este canal?",
                config.channels[index].enabled,
                cli.yes,
            )?;
            config.channels[index].credential_env = credential_env;
            config.channels[index].allowlist = allowlist
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect();
            config.channels[index].enabled = enabled;
            channels::validate_config(&config.channels[index])?;
            config.features.channels = config.channels.iter().any(|item| item.enabled);
            config::save(path, config)?;
            message(
                cli,
                "Canal configurado.",
                serde_json::json!({"name": name, "enabled": enabled}),
            );
        }
        ChannelCommands::Test { name } => {
            let channel = config
                .channels
                .iter()
                .find(|item| item.name == name)
                .ok_or_else(|| anyhow::anyhow!("canal não encontrado: {name}"))?;
            let spec = channels::find_spec(&channel.kind)?;
            let credential_present = channel.credential_env.is_empty()
                || std::env::var_os(&channel.credential_env).is_some();
            let configured_ready = channel.enabled
                && credential_present
                && (channels::is_local(spec.id) || !channel.allowlist.is_empty());
            let (healthy, status_message): (bool, String) =
                if spec.id == "telegram" && configured_ready {
                    match channels::telegram_test(channel).await {
                        Ok(()) => (true, "Telegram Bot API saudável".into()),
                        Err(error) => (false, format!("Telegram health check falhou: {}", error)),
                    }
                } else if channels::is_webhook_adapter(spec.id) && configured_ready {
                    match channels::webhook_test(channel).await {
                        Ok(status) => (
                            status < 500,
                            format!("webhook {} respondeu HTTP {}", spec.label, status),
                        ),
                        Err(error) => (false, format!("webhook health check falhou: {}", error)),
                    }
                } else if spec.id == "matrix" && configured_ready {
                    let endpoint = channels::matrix_endpoint("account/whoami")?;
                    let policy = Policy {
                        mode: config.security.mode.clone(),
                        workspace: config.security.workspace.clone(),
                        allowed_domains: config.security.allowed_domains.clone(),
                        allow_private_networks: config.security.allow_private_networks,
                    };
                    policy.check_url(&endpoint)?;
                    match channels::matrix_test(channel).await {
                        Ok(()) => (true, "Matrix Client-Server saudável".into()),
                        Err(error) => (false, format!("Matrix health check falhou: {}", error)),
                    }
                } else if spec.id == "whatsapp" && configured_ready {
                    let phone_id = std::env::var("SAPIENS_WHATSAPP_PHONE_NUMBER_ID")
                        .context("SAPIENS_WHATSAPP_PHONE_NUMBER_ID is not configured")?;
                    let endpoint = channels::whatsapp_endpoint(&phone_id, "")?;
                    let policy = Policy {
                        mode: config.security.mode.clone(),
                        workspace: config.security.workspace.clone(),
                        allowed_domains: config.security.allowed_domains.clone(),
                        allow_private_networks: config.security.allow_private_networks,
                    };
                    policy.check_url(&endpoint)?;
                    match channels::whatsapp_test(channel).await {
                        Ok(()) => (true, "WhatsApp Cloud API saudável".into()),
                        Err(error) => (
                            false,
                            format!("WhatsApp Cloud API health check falhou: {}", error),
                        ),
                    }
                } else if spec.id == "signal" && configured_ready {
                    match channels::signal_test(channel).await {
                        Ok(()) => (true, "signal-cli disponível".into()),
                        Err(error) => (false, format!("Signal health check falhou: {}", error)),
                    }
                } else {
                    let healthy = channels::adapter_ready(spec.id) && configured_ready;
                    let message = if healthy {
                        "adapter local saudável"
                    } else if spec.adapter == "opcional" {
                        "adapter externo opcional/desabilitado"
                    } else {
                        "configuração incompleta"
                    };
                    (healthy, message.into())
                };
            let data = serde_json::json!({
                "name": name,
                "kind": spec.id,
                "adapter": spec.adapter,
                "enabled": channel.enabled,
                "credential_present": credential_present,
                "healthy": healthy,
                "message": status_message,
            });
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&data)?);
            } else {
                println!(
                    "{}: {}",
                    name,
                    data["message"].as_str().unwrap_or("indisponível")
                );
            }
        }
        ChannelCommands::Send {
            name,
            recipient,
            message: text,
        } => {
            if cli.dry_run {
                message(
                    cli,
                    "Dry-run: mensagem não enviada.",
                    serde_json::json!({"channel": name, "recipient": recipient, "sent": false}),
                );
                return Ok(());
            }
            let channel = config
                .channels
                .iter()
                .find(|item| item.name == name)
                .ok_or_else(|| anyhow::anyhow!("canal não encontrado: {name}"))?;
            if !channel.enabled {
                anyhow::bail!("canal está desabilitado: {name}");
            }
            let approved = cli.yes || approval_active(config, "channel.send");
            if config.security.mode == "supervised" && !approved {
                anyhow::bail!("envio de canal exige aprovação; use --yes ou approval grant");
            }
            if channel.kind == "telegram" {
                channels::telegram_send(channel, &recipient, &text).await?;
            } else if channels::is_webhook_adapter(&channel.kind) {
                let endpoint = channels::webhook_endpoint(channel)?;
                let policy = Policy {
                    mode: config.security.mode.clone(),
                    workspace: config.security.workspace.clone(),
                    allowed_domains: config.security.allowed_domains.clone(),
                    allow_private_networks: config.security.allow_private_networks,
                };
                policy.check_url(&endpoint)?;
                channels::webhook_send(channel, &recipient, &text).await?;
            } else if channel.kind == "matrix" {
                let endpoint = channels::matrix_endpoint("account/whoami")?;
                let policy = Policy {
                    mode: config.security.mode.clone(),
                    workspace: config.security.workspace.clone(),
                    allowed_domains: config.security.allowed_domains.clone(),
                    allow_private_networks: config.security.allow_private_networks,
                };
                policy.check_url(&endpoint)?;
                channels::matrix_send(channel, &recipient, &text).await?;
            } else if channel.kind == "whatsapp" {
                let phone_id = std::env::var("SAPIENS_WHATSAPP_PHONE_NUMBER_ID")
                    .context("SAPIENS_WHATSAPP_PHONE_NUMBER_ID is not configured")?;
                let endpoint = channels::whatsapp_endpoint(&phone_id, "messages")?;
                let policy = Policy {
                    mode: config.security.mode.clone(),
                    workspace: config.security.workspace.clone(),
                    allowed_domains: config.security.allowed_domains.clone(),
                    allow_private_networks: config.security.allow_private_networks,
                };
                policy.check_url(&endpoint)?;
                channels::whatsapp_send(channel, &recipient, &text).await?;
            } else if channel.kind == "signal" {
                channels::signal_send(channel, &recipient, &text).await?;
            } else {
                anyhow::bail!(
                    "canal {} ainda não possui adapter de envio instalado",
                    channel.kind
                );
            }
            let action = if channel.kind == "telegram" {
                "channel.telegram.send"
            } else if channel.kind == "matrix" {
                "channel.matrix.send"
            } else if channel.kind == "whatsapp" {
                "channel.whatsapp.send"
            } else if channel.kind == "signal" {
                "channel.signal.send"
            } else {
                "channel.webhook.send"
            };
            sapiens_agent::observability::append_receipt(
                path,
                action,
                "external_write",
                approved,
                "ok",
                Some(&recipient),
            )?;
            message(
                cli,
                "Mensagem enviada.",
                serde_json::json!({"channel": name, "recipient": recipient, "sent": true}),
            );
        }
        ChannelCommands::SendMedia {
            name,
            recipient,
            path: media_path,
            caption,
        } => {
            if cli.dry_run {
                message(
                    cli,
                    "Dry-run: mídia não enviada.",
                    serde_json::json!({"channel": name, "recipient": recipient, "sent": false}),
                );
                return Ok(());
            }
            let channel = config
                .channels
                .iter()
                .find(|item| item.name == name)
                .ok_or_else(|| anyhow::anyhow!("canal não encontrado: {name}"))?;
            if !channel.enabled {
                anyhow::bail!("canal está desabilitado: {name}");
            }
            let kind = channels::normalize_kind(&channel.kind);
            if !matches!(kind.as_str(), "whatsapp" | "matrix" | "signal") {
                anyhow::bail!(
                    "channel send-media exige um canal whatsapp, matrix ou signal com adapter real"
                );
            }
            let approved = cli.yes || approval_active(config, "channel.send");
            if config.security.mode == "supervised" && !approved {
                anyhow::bail!("envio de mídia exige aprovação; use --yes ou approval grant");
            }
            let policy = Policy {
                mode: config.security.mode.clone(),
                workspace: config.security.workspace.clone(),
                allowed_domains: config.security.allowed_domains.clone(),
                allow_private_networks: config.security.allow_private_networks,
            };
            let action = match kind.as_str() {
                "whatsapp" => {
                    let phone_id = std::env::var("SAPIENS_WHATSAPP_PHONE_NUMBER_ID")
                        .context("SAPIENS_WHATSAPP_PHONE_NUMBER_ID is not configured")?;
                    policy.check_url(&channels::whatsapp_endpoint(&phone_id, "media")?)?;
                    policy.check_url(&channels::whatsapp_endpoint(&phone_id, "messages")?)?;
                    channels::whatsapp_send_media(
                        channel,
                        &recipient,
                        &media_path,
                        caption.as_deref(),
                        &policy,
                    )
                    .await?;
                    "channel.whatsapp.send_media"
                }
                "matrix" => {
                    channels::matrix_send_media(
                        channel,
                        &recipient,
                        &media_path,
                        caption.as_deref(),
                        &policy,
                    )
                    .await?;
                    "channel.matrix.send_media"
                }
                "signal" => {
                    channels::signal_send_media(
                        channel,
                        &recipient,
                        &media_path,
                        caption.as_deref(),
                        &policy,
                    )
                    .await?;
                    "channel.signal.send_media"
                }
                _ => unreachable!("validated media channel kind"),
            };
            sapiens_agent::observability::append_receipt(
                path,
                action,
                "external_write",
                approved,
                &format!("recipient={} file={}", recipient, media_path.display()),
                Some(&recipient),
            )?;
            message(
                cli,
                "Mídia enviada.",
                serde_json::json!({"channel": name, "recipient": recipient, "sent": true}),
            );
        }
        ChannelCommands::Start => {
            let enabled: Vec<_> = config
                .channels
                .iter()
                .filter(|item| item.enabled)
                .map(|item| item.name.clone())
                .collect();
            let ready: Vec<_> = config
                .channels
                .iter()
                .filter(|item| item.enabled && channels::adapter_ready(&item.kind))
                .map(|item| item.name.clone())
                .collect();
            let optional: Vec<_> = enabled
                .iter()
                .filter(|name| !ready.contains(name))
                .cloned()
                .collect();
            message(
                cli,
                if enabled.is_empty() {
                    "Nenhum canal habilitado; o gateway local continua disponível."
                } else {
                    "Canais com adapters disponíveis registrados; os demais adapters externos permanecem opcionais."
                },
                serde_json::json!({"enabled": enabled, "ready": ready, "optional": optional, "started": true}),
            );
        }
    }
    Ok(())
}

fn skills_command(
    config: &mut AppConfig,
    path: &Path,
    command: SkillsCommands,
    cli: &Cli,
) -> Result<()> {
    let builtin_names = [
        "browser",
        "computer-use",
        "shell",
        "mcp",
        "channels",
        "memory",
        "scheduler",
        "audio",
    ];
    match command {
        SkillsCommands::List => {
            let mut entries = sapiens_agent::skills::discover(path, &config.skills)?;
            for name in builtin_names {
                if !entries.iter().any(|entry| entry.name == name) {
                    entries.push(sapiens_agent::skills::SkillDescriptor {
                        name: name.into(),
                        description: "Runtime feature skill".into(),
                        path: PathBuf::new(),
                        valid: true,
                        enabled: skill_enabled(config, name),
                        scope: "runtime".into(),
                    });
                }
            }
            entries.sort_by(|left, right| left.name.cmp(&right.name));
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else {
                for entry in entries {
                    println!(
                        "{}\t{}\t{}",
                        entry.name,
                        if entry.enabled { "on" } else { "off" },
                        if entry.valid { "valid" } else { "invalid" }
                    );
                }
            }
        }
        SkillsCommands::Enable { name } => {
            if builtin_names.contains(&name.as_str()) {
                set_skill(config, &name, true)?;
            } else {
                sapiens_agent::skills::validate(path, &name)?;
            }
            if !config.skills.contains(&name) {
                config.skills.push(name.clone());
            }
            config::save(path, config)?;
            message(
                cli,
                "Skill habilitada.",
                serde_json::json!({"name": name, "enabled": true}),
            );
        }
        SkillsCommands::Disable { name } => {
            if builtin_names.contains(&name.as_str()) {
                set_skill(config, &name, false)?;
            }
            config.skills.retain(|item| item != &name);
            config::save(path, config)?;
            message(
                cli,
                "Skill desabilitada.",
                serde_json::json!({"name": name, "enabled": false}),
            );
        }
        SkillsCommands::Validate { name } => {
            let descriptors = if let Some(name) = name {
                vec![sapiens_agent::skills::validate(path, &name)?]
            } else {
                sapiens_agent::skills::discover(path, &config.skills)?
            };
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&descriptors)?);
            } else {
                for entry in descriptors {
                    println!(
                        "{}\t{}",
                        entry.name,
                        if entry.valid { "valid" } else { "invalid" }
                    );
                }
            }
        }
        SkillsCommands::Create { name, purpose } => {
            let descriptor = sapiens_agent::skills::create_candidate(path, &name, &purpose)?;
            message(
                cli,
                "Skill candidata criada; revise e valide antes de habilitar.",
                serde_json::to_value(descriptor)?,
            );
        }
        SkillsCommands::Suggest => {
            let suggestions = sapiens_agent::skills::suggestions(path)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&suggestions)?);
            } else if suggestions.is_empty() {
                println!("Nenhuma repetição suficiente para sugerir uma skill.");
            } else {
                for item in suggestions {
                    println!(
                        "{}\t{} ocorrências",
                        item["action"].as_str().unwrap_or("unknown"),
                        item["occurrences"].as_u64().unwrap_or_default()
                    );
                }
            }
        }
        SkillsCommands::Rollback { name } => {
            if !cli.yes {
                anyhow::bail!("rollback é destrutivo; repita com --yes após revisar a skill");
            }
            let directory = sapiens_agent::skills::root(path).join(&name);
            let manifest = directory.join("manifest.json");
            let generated = std::fs::read_to_string(&manifest)
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .and_then(|value| value.get("generated_by").cloned())
                .and_then(|value| value.as_str().map(str::to_string))
                .is_some_and(|value| value == "skill-forge");
            if !generated {
                anyhow::bail!("rollback só pode remover skills geradas pelo skill-forge");
            }
            std::fs::remove_dir_all(&directory)?;
            config.skills.retain(|item| item != &name);
            config::save(path, config)?;
            message(
                cli,
                "Skill gerada revertida.",
                serde_json::json!({"name": name, "removed": true}),
            );
        }
    }
    Ok(())
}

fn resources_command(
    config: &mut AppConfig,
    path: &Path,
    command: ResourceCommands,
    cli: &Cli,
) -> Result<()> {
    match command {
        ResourceCommands::Status => {
            let gpu_backend =
                std::env::var("SAPIENS_GPU_BACKEND").unwrap_or_else(|_| "auto".into());
            let status = serde_json::json!({
                "profile": config.resources.profile,
                "max_gpu_percent": config.resources.max_gpu_percent,
                "max_memory_mb": config.resources.max_memory_mb,
                "max_cpu_percent": config.resources.max_cpu_percent,
                "max_concurrent": config.resources.max_concurrent,
                "gpu_backend": gpu_backend,
                "policy": "bounded; fallback to CPU is allowed",
            });
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                println!("Perfil: {}", status["profile"]);
                println!(
                    "GPU: {}% (backend {})",
                    status["max_gpu_percent"], status["gpu_backend"]
                );
                println!(
                    "CPU: {}% | Memória: {} MB | Concorrência: {}",
                    status["max_cpu_percent"], status["max_memory_mb"], status["max_concurrent"]
                );
            }
        }
        ResourceCommands::Profile { name } => {
            let (gpu, memory, cpu, concurrent) = match name.as_str() {
                "economy" => (35, 1024, 50, 1),
                "balanced" => (70, 2048, 80, 2),
                "performance" => (95, 8192, 100, 4),
                "custom" => (
                    config.resources.max_gpu_percent,
                    config.resources.max_memory_mb,
                    config.resources.max_cpu_percent,
                    config.resources.max_concurrent,
                ),
                _ => anyhow::bail!("perfil deve ser economy, balanced, performance ou custom"),
            };
            config.resources.profile = name.clone();
            config.resources.max_gpu_percent = gpu;
            config.resources.max_memory_mb = memory;
            config.resources.max_cpu_percent = cpu;
            config.resources.max_concurrent = concurrent;
            config::save(path, config)?;
            message(
                cli,
                "Perfil de recursos salvo.",
                serde_json::json!({"profile": name}),
            );
        }
    }
    Ok(())
}

fn skill_enabled(config: &AppConfig, name: &str) -> bool {
    match name {
        "browser" => config.features.browser,
        "computer-use" => config.features.computer_use,
        "shell" => config.features.shell,
        "mcp" => config.features.mcp,
        "channels" => config.features.channels,
        "memory" => config.features.memory,
        "scheduler" => config.features.scheduler,
        _ => false,
    }
}

fn set_skill(config: &mut AppConfig, name: &str, enabled: bool) -> Result<()> {
    match name {
        "browser" => config.features.browser = enabled,
        "computer-use" => config.features.computer_use = enabled,
        "shell" => config.features.shell = enabled,
        "mcp" => config.features.mcp = enabled,
        "channels" => config.features.channels = enabled,
        "memory" => config.features.memory = enabled,
        "scheduler" => config.features.scheduler = enabled,
        _ => anyhow::bail!("skill desconhecida: {name}"),
    }
    Ok(())
}

fn logs_command(path: &Path, cli: &Cli) -> Result<()> {
    let log_path = path.with_file_name("sapiens-agent.log");
    if !log_path.exists() {
        let data = serde_json::json!({"log": log_path, "entries": []});
        if cli.json {
            println!("{}", serde_json::to_string_pretty(&data)?);
        } else {
            println!("Ainda não há registros do gateway.");
        }
        return Ok(());
    }
    let lines: Vec<String> = std::fs::read_to_string(&log_path)?
        .lines()
        .rev()
        .take(200)
        .map(str::to_string)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"log": log_path, "entries": lines}))?
        );
    } else {
        for line in lines {
            println!("{line}");
        }
    }
    Ok(())
}

fn approval_active(config: &AppConfig, action: &str) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    config.approvals.iter().any(|approval| {
        if approval.expires_at <= now {
            return false;
        }
        approval.scope == action
            || approval
                .scope
                .strip_suffix(".*")
                .is_some_and(|prefix| action.starts_with(prefix))
    })
}

fn approval_command(
    config: &mut AppConfig,
    path: &Path,
    command: ApprovalCommands,
    cli: &Cli,
) -> Result<()> {
    match command {
        ApprovalCommands::List => {
            let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
            let entries = config
                .approvals
                .iter()
                .map(|approval| {
                    serde_json::json!({
                        "scope": approval.scope,
                        "risk": approval.risk,
                        "granted_at": approval.granted_at,
                        "expires_at": approval.expires_at,
                        "active": approval.expires_at > now,
                        "granted_by": approval.granted_by,
                    })
                })
                .collect::<Vec<_>>();
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else if entries.is_empty() {
                println!("Nenhuma aprovação persistente.");
            } else {
                for entry in entries {
                    println!(
                        "{}\t{}\t{}",
                        entry["scope"].as_str().unwrap_or_default(),
                        if entry["active"].as_bool().unwrap_or(false) {
                            "active"
                        } else {
                            "expired"
                        },
                        entry["risk"].as_str().unwrap_or_default()
                    );
                }
            }
        }
        ApprovalCommands::Grant {
            scope,
            risk,
            expires_secs,
        } => {
            if cli.dry_run {
                message(
                    cli,
                    "Dry-run: aprovação não foi salva.",
                    serde_json::json!({"scope": scope, "saved": false}),
                );
                return Ok(());
            }
            if !cli.yes {
                anyhow::bail!("conceder aprovação persistente exige --yes após revisar o escopo");
            }
            let allowed_risks = [
                "read",
                "external_write",
                "destructive",
                "secret_input",
                "prompt_injection",
            ];
            if scope.trim().is_empty() || scope == "*" || !scope.contains('.') {
                anyhow::bail!(
                    "escopo deve ser específico, por exemplo shell.exec ou browser.click"
                );
            }
            if !allowed_risks.contains(&risk.as_str()) {
                anyhow::bail!("risco inválido: {risk}");
            }
            if !(1..=2_592_000).contains(&expires_secs) {
                anyhow::bail!("expires-secs deve estar entre 1 segundo e 30 dias");
            }
            let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
            config.approvals.retain(|approval| approval.scope != scope);
            config.approvals.push(ApprovalConfig {
                scope: scope.clone(),
                risk,
                granted_at: now,
                expires_at: now.saturating_add(expires_secs),
                granted_by: "local-cli".into(),
            });
            config::save(path, config)?;
            sapiens_agent::observability::append_receipt(
                path,
                "approval.grant",
                "governance",
                true,
                &format!(
                    "scope={scope} expires_at={}",
                    now.saturating_add(expires_secs)
                ),
                None,
            )?;
            message(
                cli,
                "Aprovação persistente concedida.",
                serde_json::json!({"scope": scope, "expires_at": now.saturating_add(expires_secs)}),
            );
        }
        ApprovalCommands::Revoke { scope } => {
            if cli.dry_run {
                message(
                    cli,
                    "Dry-run: aprovação não foi revogada.",
                    serde_json::json!({"scope": scope, "revoked": false}),
                );
                return Ok(());
            }
            if !cli.yes {
                anyhow::bail!("revogar aprovação persistente exige --yes");
            }
            let before = config.approvals.len();
            config.approvals.retain(|approval| approval.scope != scope);
            if before == config.approvals.len() {
                anyhow::bail!("aprovação não encontrada: {scope}");
            }
            config::save(path, config)?;
            sapiens_agent::observability::append_receipt(
                path,
                "approval.revoke",
                "governance",
                true,
                &format!("scope={scope}"),
                None,
            )?;
            message(
                cli,
                "Aprovação persistente revogada.",
                serde_json::json!({"scope": scope, "revoked": true}),
            );
        }
    }
    Ok(())
}

fn tools_command(
    config: &AppConfig,
    config_path: &Path,
    command: ToolCommands,
    cli: &Cli,
) -> Result<()> {
    let policy = Policy {
        mode: config.security.mode.clone(),
        workspace: config.security.workspace.clone(),
        allowed_domains: config.security.allowed_domains.clone(),
        allow_private_networks: config.security.allow_private_networks,
    };
    let catalog = sapiens_agent::tools::ToolCatalog::for_features(&policy, &config.features);
    match command {
        ToolCommands::List { enabled_only } => {
            let entries = catalog
                .list()
                .iter()
                .filter(|tool| !enabled_only || tool.enabled)
                .map(|tool| {
                    serde_json::json!({
                        "name": tool.name,
                        "description": tool.description,
                        "risk": tool.risk_name(),
                        "feature": tool.feature,
                        "enabled": tool.enabled,
                        "status": tool.status().as_str(),
                    })
                })
                .collect::<Vec<_>>();
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else if entries.is_empty() {
                println!("Nenhuma ferramenta habilitada.");
            } else {
                for entry in entries {
                    println!(
                        "{}\t{}\t{}\t{}",
                        entry["name"].as_str().unwrap_or_default(),
                        entry["status"].as_str().unwrap_or_default(),
                        entry["risk"].as_str().unwrap_or_default(),
                        entry["feature"].as_str().unwrap_or_default(),
                    );
                }
            }
        }
        ToolCommands::Discover { name } => {
            let tool = catalog.discover(&name, &policy)?;
            sapiens_agent::observability::append_receipt(
                config_path,
                "tools.discover",
                "read",
                true,
                &format!("tool={} status={}", tool.name, tool.status().as_str()),
                None,
            )?;
            let result = serde_json::json!({
                "name": tool.name,
                "description": tool.description,
                "risk": tool.risk_name(),
                "feature": tool.feature,
                "status": tool.status().as_str(),
                "execution": "delegated to the owning command with its own policy gate",
            });
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!(
                    "{}: {} (risco={}, feature={})",
                    result["name"].as_str().unwrap_or_default(),
                    result["description"].as_str().unwrap_or_default(),
                    result["risk"].as_str().unwrap_or_default(),
                    result["feature"].as_str().unwrap_or_default(),
                );
            }
        }
    }
    Ok(())
}

fn receipts_command(path: &Path, cli: &Cli) -> Result<()> {
    let receipt_path = sapiens_agent::observability::receipts_path(path);
    if !receipt_path.exists() {
        if cli.json {
            println!("[]");
        } else {
            println!("Ainda não há receipts.");
        }
        return Ok(());
    }
    let lines = std::fs::read_to_string(&receipt_path)?
        .lines()
        .rev()
        .take(200)
        .map(str::to_string)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    if cli.json {
        let values = lines
            .iter()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .collect::<Vec<_>>();
        println!("{}", serde_json::to_string_pretty(&values)?);
    } else {
        for line in lines {
            println!("{line}");
        }
    }
    Ok(())
}

fn configure_provider_fields(provider: &mut ProviderConfig, cli: &Cli) -> Result<()> {
    provider.kind = setup_ask("Família do provider", &provider.kind, cli.yes, false)?;
    provider.protocol = setup_ask(
        "Protocolo [chat_completions/responses/anthropic_messages/gemini/ollama]",
        &provider.protocol,
        cli.yes,
        false,
    )?;
    provider.base_url = setup_ask("URL base ou endpoint", &provider.base_url, cli.yes, false)?;
    provider.model = setup_ask("Modelo", &provider.model, cli.yes, false)?;
    provider.api_key_env = setup_ask(
        "Variável da credencial (nunca a chave)",
        &provider.api_key_env,
        cli.yes,
        false,
    )?;
    provider.timeout_secs = setup_ask(
        "Timeout em segundos",
        &provider.timeout_secs.to_string(),
        cli.yes,
        false,
    )?
    .parse()
    .context("timeout deve ser um número inteiro")?;
    if provider.timeout_secs == 0 {
        anyhow::bail!("timeout deve ser maior que zero");
    }
    provider.retries = setup_ask(
        "Tentativas de retry (0-10)",
        &provider.retries.to_string(),
        cli.yes,
        false,
    )?
    .parse()
    .context("retries deve ser um número inteiro")?;
    if provider.retries > 10 {
        anyhow::bail!("retries deve estar entre 0 e 10");
    }
    provider.streaming = setup_ask_bool("Ativar streaming?", provider.streaming, cli.yes, false)?;
    provider.max_input_chars = setup_ask(
        "Limite de contexto em caracteres",
        &provider.max_input_chars.to_string(),
        cli.yes,
        false,
    )?
    .parse()
    .context("limite de contexto deve ser um número inteiro")?;
    if provider.max_input_chars == 0 {
        anyhow::bail!("limite de contexto deve ser maior que zero");
    }
    provider.max_tokens = setup_ask(
        "Máximo de tokens de saída",
        &provider.max_tokens.to_string(),
        cli.yes,
        false,
    )?
    .parse()
    .context("max_tokens deve ser um número inteiro")?;
    if provider.max_tokens == 0 {
        anyhow::bail!("max_tokens deve ser maior que zero");
    }
    provider.temperature = setup_ask(
        "Temperatura (0-2)",
        &provider.temperature.to_string(),
        cli.yes,
        false,
    )?
    .parse()
    .context("temperatura deve ser um número")?;
    if !(0.0..=2.0).contains(&provider.temperature) {
        anyhow::bail!("temperatura deve estar entre 0 e 2");
    }
    provider.budget_usd = setup_ask(
        "Orçamento máximo em USD (0 = sem limite)",
        &provider.budget_usd.to_string(),
        cli.yes,
        false,
    )?
    .parse()
    .context("orçamento deve ser um número")?;
    provider.input_cost_per_1k_tokens = setup_ask(
        "Custo de entrada por 1k tokens",
        &provider.input_cost_per_1k_tokens.to_string(),
        cli.yes,
        false,
    )?
    .parse()
    .context("custo de entrada deve ser um número")?;
    provider.output_cost_per_1k_tokens = setup_ask(
        "Custo de saída por 1k tokens",
        &provider.output_cost_per_1k_tokens.to_string(),
        cli.yes,
        false,
    )?
    .parse()
    .context("custo de saída deve ser um número")?;
    if provider.budget_usd < 0.0
        || provider.input_cost_per_1k_tokens < 0.0
        || provider.output_cost_per_1k_tokens < 0.0
    {
        anyhow::bail!("orçamento e custos não podem ser negativos");
    }
    provider.circuit_breaker_threshold = setup_ask(
        "Falhas consecutivas para circuit breaker",
        &provider.circuit_breaker_threshold.to_string(),
        cli.yes,
        false,
    )?
    .parse()
    .context("threshold deve ser um número inteiro")?;
    if provider.circuit_breaker_threshold == 0 {
        anyhow::bail!("threshold deve ser maior que zero");
    }
    provider.circuit_breaker_cooldown_secs = setup_ask(
        "Cooldown do circuit breaker em segundos",
        &provider.circuit_breaker_cooldown_secs.to_string(),
        cli.yes,
        false,
    )?
    .parse()
    .context("cooldown deve ser um número inteiro")?;
    if provider.circuit_breaker_cooldown_secs == 0 {
        anyhow::bail!("cooldown deve ser maior que zero");
    }
    let capabilities = setup_ask(
        "Capabilities declaradas separadas por vírgula",
        &provider.capabilities.join(","),
        cli.yes,
        false,
    )?;
    provider.capabilities = capabilities
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect();
    Ok(())
}

async fn provider_command(
    config: &mut AppConfig,
    path: &Path,
    command: ProviderCommands,
    cli: &Cli,
) -> Result<()> {
    match command {
        ProviderCommands::List => {
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&config.providers)?);
            } else if config.providers.is_empty() {
                println!(
                    "Nenhum provider configurado. Use provider catalog ou provider add <tipo>."
                );
            } else {
                for p in &config.providers {
                    println!(
                        "{}\t{}\t{}{}",
                        p.alias,
                        p.kind,
                        p.protocol,
                        if config.active_provider.as_deref() == Some(&p.alias) {
                            "\t(active)"
                        } else {
                            ""
                        }
                    );
                }
            }
        }
        ProviderCommands::Catalog => {
            if cli.json {
                println!("{}", serde_json::to_string_pretty(providers::catalog())?);
            } else {
                println!("TIPO\tPROTOCOLOS\tCREDENCIAL\tADAPTER");
                for spec in providers::catalog() {
                    println!(
                        "{}\t{}\t{}\t{}",
                        spec.id, spec.protocols, spec.credential_hint, spec.adapter
                    );
                }
            }
        }
        ProviderCommands::Add {
            kind,
            alias,
            base_url,
            api_key_env,
            model,
            protocol,
        } => {
            let alias = alias.unwrap_or_else(|| kind.clone());
            if config.providers.iter().any(|p| p.alias == alias) {
                anyhow::bail!("provider alias já existe: {alias}");
            }
            let spec = providers::find_spec(&kind);
            if cli.dry_run {
                message(
                    cli,
                    "Dry-run: provider não foi salvo.",
                    serde_json::json!({
                        "alias": alias,
                        "kind": kind,
                        "protocol": protocol.unwrap_or_else(|| spec.and_then(|item| item.protocols.split(',').next()).unwrap_or("chat_completions").into()),
                        "saved": false,
                    }),
                );
                return Ok(());
            }
            let base_default = base_url.as_deref().unwrap_or("");
            let model_default = model.as_deref().unwrap_or("");
            let key_default = api_key_env
                .as_deref()
                .or_else(|| {
                    spec.map(|item| {
                        if item.credential_hint == "nenhuma" {
                            ""
                        } else {
                            item.credential_hint
                        }
                    })
                })
                .unwrap_or("");
            let protocol = protocol
                .or_else(|| {
                    spec.and_then(|item| item.protocols.split(',').next())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "chat_completions".into());
            let base_url = ask(
                "URL base do provider (vazio para configurar depois)",
                base_default,
                cli.yes,
            )?;
            let model = ask("Nome do modelo", model_default, cli.yes)?;
            let api_key_env = ask("Variável da chave (nunca a chave)", key_default, cli.yes)?;
            let configured = !model.is_empty();
            config.providers.push(ProviderConfig {
                alias: alias.clone(),
                kind,
                protocol,
                base_url,
                api_key_env,
                model,
                ..Default::default()
            });
            config::save(path, config)?;
            message(
                cli,
                "Provider adicionado.",
                serde_json::json!({"alias": alias, "configured": configured}),
            );
        }
        ProviderCommands::Configure { alias } => {
            if cli.dry_run {
                message(
                    cli,
                    "Dry-run: provider não foi configurado.",
                    serde_json::json!({"alias": alias, "saved": false}),
                );
                return Ok(());
            }
            let index = config
                .providers
                .iter()
                .position(|item| item.alias == alias)
                .ok_or_else(|| anyhow::anyhow!("provider não encontrado: {alias}"))?;
            let mut updated = config.providers[index].clone();
            configure_provider_fields(&mut updated, cli)?;
            config.providers[index] = updated;
            config::save(path, config)?;
            message(
                cli,
                "Provider configurado com parâmetros avançados.",
                serde_json::json!({"alias": alias, "saved": true}),
            );
        }
        ProviderCommands::Remove { alias } => {
            if alias.trim().is_empty() {
                anyhow::bail!("alias do provider não pode ser vazio");
            }
            if !cli.yes && !ask_bool("Confirmar remoção do provider?", false, false)? {
                println!("Remoção cancelada.");
                return Ok(());
            }
            let before = config.providers.len();
            config.providers.retain(|provider| provider.alias != alias);
            if before == config.providers.len() {
                anyhow::bail!("provider não encontrado: {alias}");
            }
            if config.active_provider.as_deref() == Some(alias.as_str()) {
                config.active_provider = config
                    .providers
                    .first()
                    .map(|provider| provider.alias.clone());
            }
            config.fallback.retain(|fallback| fallback != &alias);
            config::save(path, config)?;
            message(
                cli,
                "Provider removido.",
                serde_json::json!({"alias": alias, "removed": true}),
            );
        }
        ProviderCommands::Test { alias } => {
            let provider = config::find_provider(config, &alias)?;
            if cli.dry_run {
                message(
                    cli,
                    "Dry-run: provider validado sem chamada de rede.",
                    serde_json::json!({
                        "alias": alias,
                        "configured": !provider.base_url.is_empty() && !provider.model.is_empty(),
                        "protocol": provider.protocol,
                        "network_called": false,
                    }),
                );
            } else {
                let health = ProviderRegistry::new(config.clone()).health(&alias).await?;
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&health)?);
                } else {
                    println!("{alias}: ok ({} ms)", health.latency_ms);
                }
            }
        }
        ProviderCommands::Models { alias } => {
            for model in ProviderRegistry::new(config.clone()).models(&alias).await? {
                println!("{model}");
            }
        }
        ProviderCommands::Use { alias } => {
            config::find_provider(config, &alias)?;
            if cli.dry_run {
                message(
                    cli,
                    "Dry-run: provider ativo não foi alterado.",
                    serde_json::json!({"alias": alias, "saved": false}),
                );
                return Ok(());
            }
            config.active_provider = Some(alias.clone());
            config::save(path, config)?;
            println!("provider ativo: {alias}");
        }
    }
    Ok(())
}

fn route_command(config: &mut AppConfig, path: &Path, command: RouteCommands) -> Result<()> {
    match command {
        RouteCommands::Set { task, alias } => {
            config::find_provider(config, &alias)?;
            config.routes.insert(task.clone(), alias.clone());
            config::save(path, config)?;
            println!("rota {task}={alias}");
        }
        RouteCommands::Fallback { aliases } => {
            let values: Vec<String> = aliases
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            for alias in &values {
                config::find_provider(config, alias)?;
            }
            config.fallback = values;
            config::save(path, config)?;
            println!("fallback: {}", config.fallback.join(","));
        }
    }
    Ok(())
}

fn print_redacted(config: &AppConfig) -> Result<()> {
    let mut value = serde_json::to_value(config)?;
    if let Some(providers) = value.get_mut("providers").and_then(|v| v.as_array_mut()) {
        for provider in providers {
            if let Some(env) = provider.get("api_key_env").and_then(|v| v.as_str()) {
                provider["api_key_env"] = serde_json::Value::String(format!("env:{env}"));
            }
        }
    }
    if let Some(servers) = value
        .get_mut("mcp_servers")
        .and_then(|value| value.as_array_mut())
    {
        for server in servers {
            if server
                .get("url")
                .and_then(|value| value.as_str())
                .is_some_and(|value| !value.is_empty())
            {
                server["url"] = serde_json::Value::String("<configured>".into());
            }
            if let Some(env) = server
                .get("credential_env")
                .and_then(|value| value.as_str())
                && !env.is_empty()
            {
                server["credential_env"] = serde_json::Value::String(format!("env:{env}"));
            }
        }
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn message(cli: &Cli, text: &str, data: serde_json::Value) {
    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&data).unwrap_or_else(|_| "{}".into())
        );
    } else {
        println!("{text}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn computer_plan_parser_accepts_json_and_rejects_empty_plan() {
        let actions =
            parse_computer_plan("```json\n[{\"type\":\"keypress\",\"key\":\"ENTER\"}]\n```")
                .expect("plan");
        assert_eq!(actions.len(), 1);
        assert!(parse_computer_plan("[]").is_err());
        assert!(parse_computer_plan("not-json").is_err());
    }

    struct MockComputer {
        executed: Arc<Mutex<usize>>,
        stop_path: Option<PathBuf>,
    }

    impl ComputerUseAdapter for MockComputer {
        fn screenshot(&self) -> Result<Vec<u8>> {
            Ok(b"mock-png".to_vec())
        }

        fn execute(&self, _action: DesktopAction, _policy: &Policy) -> Result<()> {
            let mut executed = self.executed.lock().expect("mock lock");
            *executed += 1;
            if *executed == 1
                && let Some(path) = &self.stop_path
            {
                std::fs::write(path, b"stop").expect("stop");
            }
            Ok(())
        }
    }

    fn test_cli() -> Cli {
        Cli {
            json: true,
            yes: false,
            open_browser: false,
            no_browser: false,
            verbose: false,
            dry_run: false,
            port: None,
            workspace: None,
            config_dir: None,
            command: None,
        }
    }

    #[test]
    fn computer_sequence_rejects_secret_input_without_approval() {
        let root = std::env::temp_dir().join(format!(
            "sapiens-computer-approval-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("root");
        let actions_path = root.join("actions.json");
        std::fs::write(&actions_path, r#"[{"type":"type","text":"secret"}]"#).expect("actions");
        let config_path = root.join("config.toml");
        let mut config = AppConfig::default();
        config.security.workspace = root.clone();
        let executed = Arc::new(Mutex::new(0));
        let adapter = MockComputer {
            executed: Arc::clone(&executed),
            stop_path: None,
        };
        let mut policy = Policy::supervised(root.clone());
        assert!(
            run_computer_sequence(
                &adapter,
                &config,
                &config_path,
                &mut policy,
                &actions_path,
                &root.join("screenshots"),
                &test_cli(),
            )
            .is_err()
        );
        assert_eq!(*executed.lock().expect("mock lock"), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn computer_sequence_honors_emergency_stop_before_screenshot_or_action() {
        let root = std::env::temp_dir().join(format!(
            "sapiens-computer-emergency-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("root");
        let actions_path = root.join("actions.json");
        std::fs::write(&actions_path, r#"[{"type":"screenshot"}]"#).expect("actions");
        let config_path = root.join("config.toml");
        let stop_path = sapiens_agent::computer::emergency_stop_path(&config_path);
        std::fs::write(&stop_path, b"stop").expect("stop");
        let executed = Arc::new(Mutex::new(0));
        let adapter = MockComputer {
            executed: Arc::clone(&executed),
            stop_path: None,
        };
        let config = AppConfig::default();
        let mut policy = Policy::supervised(root.clone());
        assert!(
            run_computer_sequence(
                &adapter,
                &config,
                &config_path,
                &mut policy,
                &actions_path,
                &root.join("screenshots"),
                &test_cli(),
            )
            .is_err()
        );
        assert_eq!(*executed.lock().expect("mock lock"), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn computer_sequence_stops_cooperatively_between_steps() {
        let root = std::env::temp_dir().join(format!(
            "sapiens-computer-cancel-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("root");
        let actions_path = root.join("actions.json");
        std::fs::write(
            &actions_path,
            r#"[{"type":"screenshot"},{"type":"screenshot"}]"#,
        )
        .expect("actions");
        let config_path = root.join("config.toml");
        let stop_path = sapiens_agent::computer::emergency_stop_path(&config_path);
        let executed = Arc::new(Mutex::new(0));
        let adapter = MockComputer {
            executed: Arc::clone(&executed),
            stop_path: Some(stop_path),
        };
        let config = AppConfig::default();
        let mut policy = Policy::supervised(root.clone());
        assert!(
            run_computer_sequence(
                &adapter,
                &config,
                &config_path,
                &mut policy,
                &actions_path,
                &root.join("screenshots"),
                &test_cli(),
            )
            .is_err()
        );
        assert_eq!(*executed.lock().expect("mock lock"), 1);
        let _ = std::fs::remove_dir_all(root);
    }
}
