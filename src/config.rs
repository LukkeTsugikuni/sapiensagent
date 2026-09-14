use crate::channels::ChannelConfig;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct AppConfig {
    pub initialized: bool,
    pub providers: Vec<ProviderConfig>,
    pub active_provider: Option<String>,
    pub routes: HashMap<String, String>,
    pub fallback: Vec<String>,
    pub security: SecurityConfig,
    pub server: ServerConfig,
    pub shell: ShellConfig,
    pub features: FeaturesConfig,
    pub interface: InterfaceConfig,
    pub resources: ResourceConfig,
    pub audio: AudioConfig,
    pub schedules: Vec<ScheduleConfig>,
    pub scheduler: SchedulerLimits,
    pub channels: Vec<ChannelConfig>,
    pub mcp_servers: Vec<McpServerConfig>,
    pub approvals: Vec<ApprovalConfig>,
    pub skills: Vec<String>,
    pub memory_retention_days: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    pub enabled: bool,
    pub allowlist: Vec<String>,
    pub version: String,
    pub transport: String,
    pub url: String,
    pub credential_env: String,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            command: String::new(),
            enabled: false,
            allowlist: vec![],
            version: "1".into(),
            transport: "stdio".into(),
            url: String::new(),
            credential_env: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ApprovalConfig {
    pub scope: String,
    pub risk: String,
    pub granted_at: u64,
    pub expires_at: u64,
    pub granted_by: String,
}

impl Default for ApprovalConfig {
    fn default() -> Self {
        Self {
            scope: String::new(),
            risk: "external_write".into(),
            granted_at: 0,
            expires_at: 0,
            granted_by: "local-cli".into(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub alias: String,
    pub kind: String,
    pub protocol: String,
    pub base_url: String,
    pub api_key_env: String,
    pub model: String,
    pub timeout_secs: u64,
    pub retries: u8,
    pub streaming: bool,
    pub max_input_chars: u32,
    pub max_tokens: u32,
    pub budget_usd: f64,
    pub input_cost_per_1k_tokens: f64,
    pub output_cost_per_1k_tokens: f64,
    pub circuit_breaker_threshold: u8,
    pub circuit_breaker_cooldown_secs: u64,
    pub capabilities: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    pub mode: String,
    pub workspace: PathBuf,
    pub allowed_domains: Vec<String>,
    pub allow_private_networks: bool,
    pub max_requests_per_minute: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub bind: String,
    pub auth_env: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellConfig {
    pub allowlist: Vec<String>,
    pub max_output_bytes: usize,
    pub max_memory_mb: u32,
    pub max_processes: u32,
    pub max_cpu_secs: u32,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            allowlist: vec![],
            max_output_bytes: 1_048_576,
            max_memory_mb: 512,
            max_processes: 16,
            max_cpu_secs: 60,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FeaturesConfig {
    pub browser: bool,
    pub computer_use: bool,
    pub shell: bool,
    pub mcp: bool,
    pub channels: bool,
    pub memory: bool,
    pub scheduler: bool,
    pub audio: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct InterfaceConfig {
    pub mode: String,
    pub auto_open_browser: bool,
}

impl Default for InterfaceConfig {
    fn default() -> Self {
        Self {
            mode: "powershell".into(),
            auto_open_browser: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ResourceConfig {
    pub profile: String,
    pub max_gpu_percent: u8,
    pub max_memory_mb: u32,
    pub max_cpu_percent: u8,
    pub max_concurrent: u8,
}

impl Default for ResourceConfig {
    fn default() -> Self {
        Self {
            profile: "balanced".into(),
            max_gpu_percent: 70,
            max_memory_mb: 2048,
            max_cpu_percent: 80,
            max_concurrent: 2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    pub enabled: bool,
    pub max_bytes: u64,
    pub max_seconds: u32,
    pub transcription_provider: String,
    pub voice_reply: bool,
    pub retain_files: bool,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_bytes: 25 * 1024 * 1024,
            max_seconds: 300,
            transcription_provider: "provider".into(),
            voice_reply: false,
            retain_files: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScheduleConfig {
    pub id: String,
    pub kind: String,
    pub value: String,
    pub task: String,
    pub enabled: bool,
    pub created_at: u64,
    pub last_run: Option<u64>,
    pub run_count: u64,
    pub last_error: Option<String>,
    pub webhook_url: Option<String>,
    pub retry_limit: u8,
    pub timeout_secs: u64,
    pub last_result: Option<String>,
    pub last_delivery: Option<String>,
    pub cancel_requested: bool,
    pub channel_name: Option<String>,
    pub channel_recipient: Option<String>,
    pub run_state: String,
    pub active_run_id: Option<String>,
    pub started_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SchedulerLimits {
    pub max_concurrent: u8,
    pub max_depth: u8,
    pub max_tokens: u32,
    pub max_cost_usd: f64,
}

impl Default for SchedulerLimits {
    fn default() -> Self {
        Self {
            max_concurrent: 1,
            max_depth: 3,
            max_tokens: 4096,
            max_cost_usd: 0.0,
        }
    }
}

impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            kind: "every".into(),
            value: String::new(),
            task: String::new(),
            enabled: false,
            created_at: 0,
            last_run: None,
            run_count: 0,
            last_error: None,
            webhook_url: None,
            retry_limit: 2,
            timeout_secs: 300,
            last_result: None,
            last_delivery: None,
            cancel_requested: false,
            channel_name: None,
            channel_recipient: None,
            run_state: "idle".into(),
            active_run_id: None,
            started_at: None,
        }
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            alias: String::new(),
            kind: "custom".into(),
            protocol: "chat_completions".into(),
            base_url: String::new(),
            api_key_env: "OPENAI_API_KEY".into(),
            model: String::new(),
            timeout_secs: 60,
            retries: 2,
            streaming: false,
            max_input_chars: 12_000,
            max_tokens: 2048,
            budget_usd: 0.0,
            input_cost_per_1k_tokens: 0.0,
            output_cost_per_1k_tokens: 0.0,
            circuit_breaker_threshold: 3,
            circuit_breaker_cooldown_secs: 30,
            capabilities: vec![],
        }
    }
}
impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            mode: "supervised".into(),
            workspace: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            allowed_domains: vec![],
            allow_private_networks: false,
            max_requests_per_minute: 60,
        }
    }
}
impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8787".into(),
            auth_env: String::new(),
        }
    }
}
impl Default for FeaturesConfig {
    fn default() -> Self {
        Self {
            browser: false,
            computer_use: false,
            shell: false,
            mcp: false,
            channels: false,
            memory: false,
            scheduler: true,
            audio: false,
        }
    }
}

pub fn config_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("CLAW_HOME") {
        return Ok(PathBuf::from(path));
    }
    if cfg!(windows) {
        if let Some(path) = std::env::var_os("APPDATA") {
            return Ok(PathBuf::from(path).join("sapiens-agent"));
        }
    } else if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path).join("sapiens-agent"));
    }
    Ok(std::env::current_dir()?.join(".sapiens-agent"))
}
pub fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}
pub fn memory_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("memory.jsonl"))
}
pub fn load(path: &Path) -> Result<AppConfig> {
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("read config {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parse config {}", path.display()))
}
pub fn save(path: &Path, config: &AppConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, toml::to_string_pretty(config)?)?;
    Ok(())
}
pub fn find_provider<'a>(config: &'a AppConfig, alias: &str) -> Result<&'a ProviderConfig> {
    config
        .providers
        .iter()
        .find(|p| p.alias == alias)
        .with_context(|| format!("unknown provider alias: {alias}"))
}

pub fn get_value(config: &AppConfig, key: &str) -> Result<String> {
    let value = match key {
        "initialized" => config.initialized.to_string(),
        "active_provider" => config.active_provider.clone().unwrap_or_default(),
        "server.bind" => config.server.bind.clone(),
        "server.auth_env" => config.server.auth_env.clone(),
        "shell.allowlist" => config.shell.allowlist.join(","),
        "shell.max_output_bytes" => config.shell.max_output_bytes.to_string(),
        "shell.max_memory_mb" => config.shell.max_memory_mb.to_string(),
        "shell.max_processes" => config.shell.max_processes.to_string(),
        "shell.max_cpu_secs" => config.shell.max_cpu_secs.to_string(),
        "security.mode" => config.security.mode.clone(),
        "security.workspace" => config.security.workspace.display().to_string(),
        "security.max_requests_per_minute" => config.security.max_requests_per_minute.to_string(),
        "memory_retention_days" => config.memory_retention_days.to_string(),
        "features.browser" => config.features.browser.to_string(),
        "features.computer_use" => config.features.computer_use.to_string(),
        "features.shell" => config.features.shell.to_string(),
        "features.mcp" => config.features.mcp.to_string(),
        "features.channels" => config.features.channels.to_string(),
        "features.memory" => config.features.memory.to_string(),
        "features.scheduler" => config.features.scheduler.to_string(),
        "features.audio" => config.features.audio.to_string(),
        "interface.mode" => config.interface.mode.clone(),
        "interface.auto_open_browser" => config.interface.auto_open_browser.to_string(),
        "resources.profile" => config.resources.profile.clone(),
        "resources.max_gpu_percent" => config.resources.max_gpu_percent.to_string(),
        "resources.max_memory_mb" => config.resources.max_memory_mb.to_string(),
        "resources.max_cpu_percent" => config.resources.max_cpu_percent.to_string(),
        "resources.max_concurrent" => config.resources.max_concurrent.to_string(),
        "audio.enabled" => config.audio.enabled.to_string(),
        "audio.max_bytes" => config.audio.max_bytes.to_string(),
        "audio.max_seconds" => config.audio.max_seconds.to_string(),
        "audio.transcription_provider" => config.audio.transcription_provider.clone(),
        "audio.voice_reply" => config.audio.voice_reply.to_string(),
        "audio.retain_files" => config.audio.retain_files.to_string(),
        "scheduler.max_concurrent" => config.scheduler.max_concurrent.to_string(),
        "scheduler.max_depth" => config.scheduler.max_depth.to_string(),
        "scheduler.max_tokens" => config.scheduler.max_tokens.to_string(),
        "scheduler.max_cost_usd" => config.scheduler.max_cost_usd.to_string(),
        key if key.starts_with("provider.") && key.ends_with(".model") => {
            let alias = key
                .trim_start_matches("provider.")
                .trim_end_matches(".model");
            find_provider(config, alias)?.model.clone()
        }
        key if key.starts_with("provider.") && key.ends_with(".max_input_chars") => {
            let alias = key
                .trim_start_matches("provider.")
                .trim_end_matches(".max_input_chars");
            find_provider(config, alias)?.max_input_chars.to_string()
        }
        key if key.starts_with("provider.") && key.ends_with(".budget_usd") => {
            let alias = key
                .trim_start_matches("provider.")
                .trim_end_matches(".budget_usd");
            find_provider(config, alias)?.budget_usd.to_string()
        }
        key if key.starts_with("provider.") && key.ends_with(".input_cost_per_1k_tokens") => {
            let alias = key
                .trim_start_matches("provider.")
                .trim_end_matches(".input_cost_per_1k_tokens");
            find_provider(config, alias)?
                .input_cost_per_1k_tokens
                .to_string()
        }
        key if key.starts_with("provider.") && key.ends_with(".output_cost_per_1k_tokens") => {
            let alias = key
                .trim_start_matches("provider.")
                .trim_end_matches(".output_cost_per_1k_tokens");
            find_provider(config, alias)?
                .output_cost_per_1k_tokens
                .to_string()
        }
        key if key.starts_with("provider.") && key.ends_with(".circuit_breaker_threshold") => {
            let alias = key
                .trim_start_matches("provider.")
                .trim_end_matches(".circuit_breaker_threshold");
            find_provider(config, alias)?
                .circuit_breaker_threshold
                .to_string()
        }
        key if key.starts_with("provider.") && key.ends_with(".circuit_breaker_cooldown_secs") => {
            let alias = key
                .trim_start_matches("provider.")
                .trim_end_matches(".circuit_breaker_cooldown_secs");
            find_provider(config, alias)?
                .circuit_breaker_cooldown_secs
                .to_string()
        }
        _ => anyhow::bail!("unknown config key: {key}"),
    };
    Ok(value)
}

pub fn set_value(config: &mut AppConfig, key: &str, value: &str) -> Result<()> {
    match key {
        "active_provider" => {
            find_provider(config, value)?;
            config.active_provider = Some(value.to_string());
        }
        "server.bind" => config.server.bind = value.to_string(),
        "server.auth_env" => config.server.auth_env = value.to_string(),
        "shell.allowlist" => {
            config.shell.allowlist = value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_ascii_lowercase)
                .collect();
        }
        "shell.max_output_bytes" => {
            let limit = value
                .parse::<usize>()
                .with_context(|| format!("invalid shell output limit: {value}"))?;
            if !(1..=16 * 1024 * 1024).contains(&limit) {
                anyhow::bail!("shell.max_output_bytes must be between 1 and 16777216");
            }
            config.shell.max_output_bytes = limit;
        }
        "shell.max_memory_mb" => {
            let limit = value
                .parse::<u32>()
                .with_context(|| format!("invalid shell memory limit: {value}"))?;
            if !(64..=16_384).contains(&limit) {
                anyhow::bail!("shell.max_memory_mb must be between 64 and 16384");
            }
            config.shell.max_memory_mb = limit;
        }
        "shell.max_processes" => {
            let limit = value
                .parse::<u32>()
                .with_context(|| format!("invalid shell process limit: {value}"))?;
            if !(1..=256).contains(&limit) {
                anyhow::bail!("shell.max_processes must be between 1 and 256");
            }
            config.shell.max_processes = limit;
        }
        "shell.max_cpu_secs" => {
            let limit = value
                .parse::<u32>()
                .with_context(|| format!("invalid shell CPU limit: {value}"))?;
            if !(1..=86_400).contains(&limit) {
                anyhow::bail!("shell.max_cpu_secs must be between 1 and 86400");
            }
            config.shell.max_cpu_secs = limit;
        }
        "security.mode" if ["readonly", "supervised", "trusted"].contains(&value) => {
            config.security.mode = value.to_string()
        }
        "security.workspace" => config.security.workspace = PathBuf::from(value),
        "security.max_requests_per_minute" => {
            let limit = value
                .parse::<u32>()
                .with_context(|| format!("invalid request limit: {value}"))?;
            if limit == 0 {
                anyhow::bail!("security.max_requests_per_minute must be greater than zero");
            }
            config.security.max_requests_per_minute = limit;
        }
        "memory_retention_days" => {
            config.memory_retention_days = value
                .parse::<u64>()
                .with_context(|| format!("invalid memory retention: {value}"))?;
        }
        "features.browser" => config.features.browser = parse_bool(value)?,
        "features.computer_use" => config.features.computer_use = parse_bool(value)?,
        "features.shell" => config.features.shell = parse_bool(value)?,
        "features.mcp" => config.features.mcp = parse_bool(value)?,
        "features.channels" => config.features.channels = parse_bool(value)?,
        "features.memory" => config.features.memory = parse_bool(value)?,
        "features.scheduler" => config.features.scheduler = parse_bool(value)?,
        "features.audio" => {
            let enabled = parse_bool(value)?;
            config.features.audio = enabled;
            config.audio.enabled = enabled;
        }
        "interface.mode" if ["powershell", "web", "both"].contains(&value) => {
            config.interface.mode = value.to_string()
        }
        "interface.mode" => anyhow::bail!("interface.mode must be powershell, web, or both"),
        "interface.auto_open_browser" => config.interface.auto_open_browser = parse_bool(value)?,
        "resources.profile"
            if ["economy", "balanced", "performance", "custom"].contains(&value) =>
        {
            config.resources.profile = value.to_string()
        }
        "resources.profile" => {
            anyhow::bail!("resources.profile must be economy, balanced, performance, or custom")
        }
        "resources.max_gpu_percent" => {
            let limit = value
                .parse::<u8>()
                .with_context(|| format!("invalid GPU limit: {value}"))?;
            if limit == 0 || limit > 100 {
                anyhow::bail!("resources.max_gpu_percent must be between 1 and 100");
            }
            config.resources.max_gpu_percent = limit;
        }
        "resources.max_memory_mb" => {
            let limit = value
                .parse::<u32>()
                .with_context(|| format!("invalid memory limit: {value}"))?;
            if !(256..=65_536).contains(&limit) {
                anyhow::bail!("resources.max_memory_mb must be between 256 and 65536");
            }
            config.resources.max_memory_mb = limit;
        }
        "resources.max_cpu_percent" => {
            let limit = value
                .parse::<u8>()
                .with_context(|| format!("invalid CPU limit: {value}"))?;
            if limit == 0 || limit > 100 {
                anyhow::bail!("resources.max_cpu_percent must be between 1 and 100");
            }
            config.resources.max_cpu_percent = limit;
        }
        "resources.max_concurrent" => {
            let limit = value
                .parse::<u8>()
                .with_context(|| format!("invalid concurrency: {value}"))?;
            if limit == 0 || limit > 32 {
                anyhow::bail!("resources.max_concurrent must be between 1 and 32");
            }
            config.resources.max_concurrent = limit;
        }
        "audio.enabled" => {
            let enabled = parse_bool(value)?;
            config.audio.enabled = enabled;
            config.features.audio = enabled;
        }
        "audio.max_bytes" => {
            let limit = value
                .parse::<u64>()
                .with_context(|| format!("invalid audio size: {value}"))?;
            if !(1..=100 * 1024 * 1024).contains(&limit) {
                anyhow::bail!("audio.max_bytes must be between 1 and 104857600");
            }
            config.audio.max_bytes = limit;
        }
        "audio.max_seconds" => {
            let limit = value
                .parse::<u32>()
                .with_context(|| format!("invalid audio duration: {value}"))?;
            if !(1..=3600).contains(&limit) {
                anyhow::bail!("audio.max_seconds must be between 1 and 3600");
            }
            config.audio.max_seconds = limit;
        }
        "audio.transcription_provider" => config.audio.transcription_provider = value.to_string(),
        "audio.voice_reply" => config.audio.voice_reply = parse_bool(value)?,
        "audio.retain_files" => config.audio.retain_files = parse_bool(value)?,
        "scheduler.max_concurrent" => {
            let limit = value
                .parse::<u8>()
                .with_context(|| format!("invalid scheduler concurrency: {value}"))?;
            if limit == 0 {
                anyhow::bail!("scheduler.max_concurrent must be greater than zero");
            }
            config.scheduler.max_concurrent = limit;
        }
        "scheduler.max_depth" => {
            let limit = value
                .parse::<u8>()
                .with_context(|| format!("invalid scheduler depth: {value}"))?;
            if limit == 0 {
                anyhow::bail!("scheduler.max_depth must be greater than zero");
            }
            config.scheduler.max_depth = limit;
        }
        "scheduler.max_tokens" => {
            let limit = value
                .parse::<u32>()
                .with_context(|| format!("invalid scheduler token limit: {value}"))?;
            if limit == 0 {
                anyhow::bail!("scheduler.max_tokens must be greater than zero");
            }
            config.scheduler.max_tokens = limit;
        }
        "scheduler.max_cost_usd" => {
            config.scheduler.max_cost_usd = parse_nonnegative_float(value, "scheduler cost")?;
        }
        key if key.starts_with("provider.") && key.ends_with(".model") => {
            let alias = key
                .trim_start_matches("provider.")
                .trim_end_matches(".model");
            let provider = config
                .providers
                .iter_mut()
                .find(|p| p.alias == alias)
                .with_context(|| format!("unknown provider alias: {alias}"))?;
            provider.model = value.to_string();
        }
        key if key.starts_with("provider.") && key.ends_with(".max_input_chars") => {
            let alias = key
                .trim_start_matches("provider.")
                .trim_end_matches(".max_input_chars");
            let limit = value
                .parse::<u32>()
                .with_context(|| format!("invalid provider input limit: {value}"))?;
            if limit == 0 {
                anyhow::bail!("provider max_input_chars must be greater than zero");
            }
            let provider = config
                .providers
                .iter_mut()
                .find(|p| p.alias == alias)
                .with_context(|| format!("unknown provider alias: {alias}"))?;
            provider.max_input_chars = limit;
        }
        key if key.starts_with("provider.") && key.ends_with(".budget_usd") => {
            let alias = key
                .trim_start_matches("provider.")
                .trim_end_matches(".budget_usd");
            let budget = parse_nonnegative_float(value, "provider budget")?;
            let provider = config
                .providers
                .iter_mut()
                .find(|p| p.alias == alias)
                .with_context(|| format!("unknown provider alias: {alias}"))?;
            provider.budget_usd = budget;
        }
        key if key.starts_with("provider.") && key.ends_with(".input_cost_per_1k_tokens") => {
            let alias = key
                .trim_start_matches("provider.")
                .trim_end_matches(".input_cost_per_1k_tokens");
            let cost = parse_nonnegative_float(value, "provider input cost")?;
            let provider = config
                .providers
                .iter_mut()
                .find(|p| p.alias == alias)
                .with_context(|| format!("unknown provider alias: {alias}"))?;
            provider.input_cost_per_1k_tokens = cost;
        }
        key if key.starts_with("provider.") && key.ends_with(".output_cost_per_1k_tokens") => {
            let alias = key
                .trim_start_matches("provider.")
                .trim_end_matches(".output_cost_per_1k_tokens");
            let cost = parse_nonnegative_float(value, "provider output cost")?;
            let provider = config
                .providers
                .iter_mut()
                .find(|p| p.alias == alias)
                .with_context(|| format!("unknown provider alias: {alias}"))?;
            provider.output_cost_per_1k_tokens = cost;
        }
        key if key.starts_with("provider.") && key.ends_with(".circuit_breaker_threshold") => {
            let alias = key
                .trim_start_matches("provider.")
                .trim_end_matches(".circuit_breaker_threshold");
            let threshold = value
                .parse::<u8>()
                .with_context(|| format!("invalid circuit breaker threshold: {value}"))?;
            if threshold == 0 {
                anyhow::bail!("circuit breaker threshold must be greater than zero");
            }
            let provider = config
                .providers
                .iter_mut()
                .find(|p| p.alias == alias)
                .with_context(|| format!("unknown provider alias: {alias}"))?;
            provider.circuit_breaker_threshold = threshold;
        }
        key if key.starts_with("provider.") && key.ends_with(".circuit_breaker_cooldown_secs") => {
            let alias = key
                .trim_start_matches("provider.")
                .trim_end_matches(".circuit_breaker_cooldown_secs");
            let cooldown = value
                .parse::<u64>()
                .with_context(|| format!("invalid circuit breaker cooldown: {value}"))?;
            if cooldown == 0 {
                anyhow::bail!("circuit breaker cooldown must be greater than zero");
            }
            let provider = config
                .providers
                .iter_mut()
                .find(|p| p.alias == alias)
                .with_context(|| format!("unknown provider alias: {alias}"))?;
            provider.circuit_breaker_cooldown_secs = cooldown;
        }
        "security.mode" => anyhow::bail!("security.mode must be readonly, supervised or trusted"),
        _ => anyhow::bail!("unknown or read-only config key: {key}"),
    }
    Ok(())
}

fn parse_bool(value: &str) -> Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "on" | "yes" | "1" => Ok(true),
        "false" | "off" | "no" | "0" => Ok(false),
        _ => anyhow::bail!("expected true/false, got {value}"),
    }
}

fn parse_nonnegative_float(value: &str, label: &str) -> Result<f64> {
    let parsed = value
        .parse::<f64>()
        .with_context(|| format!("invalid {label}: {value}"))?;
    if !parsed.is_finite() || parsed < 0.0 {
        anyhow::bail!("{label} must be a finite non-negative number");
    }
    Ok(parsed)
}
