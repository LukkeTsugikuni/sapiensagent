use sapiens_agent::{
    browser::{BrowserDriver, PlaywrightCliBrowser},
    config::{self, AppConfig, ScheduleConfig},
    policy::{Policy, Risk},
};
use std::{
    io::{Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
#[test]
fn default_is_supervised_and_private_networks_are_blocked() {
    let config = AppConfig::default();
    assert_eq!(config.security.mode, "supervised");
    let policy = Policy::supervised(PathBuf::from("."));
    assert!(policy.check_url("http://localhost:3000").is_err());
    assert!(policy.requires_approval(Risk::ExternalWrite));
}

struct TestServer {
    child: Child,
    pid_path: PathBuf,
}

impl TestServer {
    fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.pid_path);
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.pid_path);
    }
}

fn start_test_server(executable: &PathBuf, config_dir: &PathBuf) -> TestServer {
    let pid_path = config_dir.join("sapiens-agent.pid");
    let child = Command::new(executable)
        .args(["start", "--config-dir"])
        .arg(config_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start gateway process");
    TestServer { child, pid_path }
}

fn wait_for_health(child: &mut Child, base_url: &str) {
    let client = reqwest::blocking::Client::new();
    for _ in 0..120 {
        if let Ok(Some(status)) = child.try_wait() {
            panic!("gateway exited before health check: {status}");
        }
        if let Ok(response) = client.get(format!("{base_url}/health")).send()
            && response.status().is_success()
        {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("gateway did not become healthy: {base_url}");
}

#[test]
fn gateway_session_survives_a_real_process_restart() {
    let root =
        std::env::temp_dir().join(format!("sapiens-acceptance-restart-{}", std::process::id()));
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("free port")
        .local_addr()
        .expect("address")
        .port();
    let config = AppConfig {
        initialized: true,
        server: config::ServerConfig {
            bind: format!("127.0.0.1:{port}"),
            ..Default::default()
        },
        security: config::SecurityConfig {
            workspace,
            ..Default::default()
        },
        features: config::FeaturesConfig {
            scheduler: false,
            ..Default::default()
        },
        ..Default::default()
    };
    config::save(&root.join("config.toml"), &config).expect("config");
    let executable = std::env::var_os("CARGO_BIN_EXE_sapiens-agent")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/debug/sapiens-agent.exe"));
    let base_url = format!("http://127.0.0.1:{port}");
    let mut first = start_test_server(&executable, &root);
    wait_for_health(&mut first.child, &base_url);

    let client = reqwest::blocking::Client::new();
    let response = client
        .post(format!("{base_url}/v1/chat"))
        .json(&serde_json::json!({
            "message": "crie a sessão de teste",
            "session": "restart:alice"
        }))
        .send()
        .expect("chat request");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
    assert!(root.join("sessions.json").exists());
    first.stop();

    let mut second = start_test_server(&executable, &root);
    wait_for_health(&mut second.child, &base_url);
    let response = client
        .post(format!("{base_url}/v1/sessions/restart:alice/cancel"))
        .send()
        .expect("cancel request after restart");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        response.json::<serde_json::Value>().expect("cancel json")["cancelled"],
        true
    );
    second.stop();
    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn scheduler_recovers_running_job_on_a_real_process_restart() {
    let root = std::env::temp_dir().join(format!(
        "sapiens-acceptance-scheduler-restart-{}",
        std::process::id()
    ));
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("free port")
        .local_addr()
        .expect("address")
        .port();
    let config = AppConfig {
        initialized: true,
        server: config::ServerConfig {
            bind: format!("127.0.0.1:{port}"),
            ..Default::default()
        },
        security: config::SecurityConfig {
            workspace,
            ..Default::default()
        },
        schedules: vec![ScheduleConfig {
            id: "recover-job".into(),
            kind: "once".into(),
            value: "now".into(),
            task: "não executar novamente durante recovery".into(),
            enabled: true,
            run_state: "running".into(),
            active_run_id: Some("recover-job:old-process".into()),
            started_at: Some(123),
            ..Default::default()
        }],
        ..Default::default()
    };
    let config_path = root.join("config.toml");
    config::save(&config_path, &config).expect("config");
    let executable = std::env::var_os("CARGO_BIN_EXE_sapiens-agent")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/debug/sapiens-agent.exe"));
    let base_url = format!("http://127.0.0.1:{port}");
    let mut server = start_test_server(&executable, &root);
    wait_for_health(&mut server.child, &base_url);
    for _ in 0..40 {
        if config::load(&config_path)
            .expect("load recovered config")
            .schedules
            .first()
            .is_some_and(|job| job.run_state == "interrupted")
        {
            server.stop();
            std::fs::remove_dir_all(root).expect("cleanup");
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    server.stop();
    panic!("scheduler did not persist interrupted state after restart");
}

#[test]
fn computer_plan_and_auto_complete_a_real_read_only_smoke() {
    let root = std::env::temp_dir().join(format!(
        "sapiens-acceptance-computer-plan-{}",
        std::process::id()
    ));
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("provider listener");
    let address = listener.local_addr().expect("provider address");
    let provider_response =
        r#"{"choices":[{"message":{"content":"[{\"type\":\"screenshot\"}]"}}]}"#;
    let server = thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().expect("provider request");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("provider read timeout");
            let mut request = [0_u8; 32_768];
            let _ = stream.read(&mut request).expect("provider read");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                provider_response.len(),
                provider_response
            );
            stream
                .write_all(response.as_bytes())
                .expect("provider response");
        }
    });
    let config = AppConfig {
        initialized: true,
        active_provider: Some("local-plan".into()),
        providers: vec![config::ProviderConfig {
            alias: "local-plan".into(),
            protocol: "chat_completions".into(),
            base_url: format!("http://{address}/v1"),
            api_key_env: String::new(),
            model: "test".into(),
            ..Default::default()
        }],
        security: config::SecurityConfig {
            workspace,
            ..Default::default()
        },
        features: config::FeaturesConfig {
            computer_use: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let config_path = root.join("config.toml");
    config::save(&config_path, &config).expect("config");
    let executable = std::env::var_os("CARGO_BIN_EXE_sapiens-agent")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/debug/sapiens-agent.exe"));
    let run = |args: &[&str]| {
        let output = Command::new(&executable)
            .arg("--config-dir")
            .arg(&root)
            .arg("--yes")
            .args(args)
            .output()
            .expect("run computer command");
        assert!(
            output.status.success(),
            "computer command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run(&[
        "computer",
        "plan",
        "capture the desktop",
        "--output",
        "plan.json",
    ]);
    run(&[
        "computer",
        "auto",
        "capture the desktop",
        "--output-dir",
        "auto-artifacts",
    ]);
    server.join().expect("provider server");
    assert!(root.join("workspace/plan.json").exists());
    assert!(root.join("workspace/computer-auto-plan.json").exists());
    assert!(
        root.join("workspace/auto-artifacts/before-000.png")
            .exists()
    );
    assert!(root.join("workspace/auto-artifacts/after-000.png").exists());
    assert!(root.join("sapiens-agent-receipts.jsonl").exists());
    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn cli_chat_image_reaches_a_real_provider_request() {
    let root = std::env::temp_dir().join(format!(
        "sapiens-acceptance-cli-image-{}",
        std::process::id()
    ));
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let image_path = workspace.join("sample.png");
    std::fs::write(&image_path, [0_u8, 1, 2, 3]).expect("image");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("provider listener");
    let address = listener.local_addr().expect("provider address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("provider request");
        let mut request = [0_u8; 32_768];
        let count = stream.read(&mut request).expect("provider read");
        let request = String::from_utf8_lossy(&request[..count]);
        assert!(request.contains("data:image/png;base64,AAECAw=="));
        assert!(!request.contains("sample.png"));
        let body = r#"{"choices":[{"message":{"content":"imagem ok"}}]}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("provider response");
    });
    let config = AppConfig {
        initialized: true,
        active_provider: Some("local-image".into()),
        providers: vec![config::ProviderConfig {
            alias: "local-image".into(),
            protocol: "chat_completions".into(),
            base_url: format!("http://{address}/v1"),
            api_key_env: String::new(),
            model: "test".into(),
            ..Default::default()
        }],
        security: config::SecurityConfig {
            workspace: workspace.clone(),
            ..Default::default()
        },
        ..Default::default()
    };
    config::save(&root.join("config.toml"), &config).expect("config");
    let executable = std::env::var_os("CARGO_BIN_EXE_sapiens-agent")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/debug/sapiens-agent.exe"));
    let output = Command::new(&executable)
        .arg("--config-dir")
        .arg(&root)
        .arg("chat")
        .arg("--image")
        .arg(&image_path)
        .arg("descreva")
        .output()
        .expect("run image chat");
    assert!(
        output.status.success(),
        "image chat failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("imagem ok"));
    server.join().expect("provider server");
    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn browser_adapter_smoke_against_local_page_when_enabled() {
    if std::env::var("SAPIENS_RUN_BROWSER_E2E").as_deref() != Ok("1") {
        return;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("page listener");
    let address = listener.local_addr().expect("page address");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = Arc::clone(&stop);
    let server = thread::spawn(move || {
        while !server_stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut request = [0_u8; 8_192];
                    let _ = stream.read(&mut request);
                    let body = r#"<!doctype html><title>Local E2E</title><main><label for="message">Mensagem</label><input id="message"><button id="send" onclick="document.querySelector('#result').textContent=document.querySelector('#message').value">Enviar</button><output id="result"></output></main>"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("local page server failed: {error}"),
            }
        }
    });
    let policy = Policy {
        mode: "trusted".into(),
        workspace: std::env::temp_dir(),
        allowed_domains: vec![],
        allow_private_networks: true,
    };
    let session = format!("sapiens-e2e-{}", std::process::id());
    let browser = PlaywrightCliBrowser::new(session).expect("browser");
    let url = format!("http://{address}/");
    let tab = browser.open(&url, &policy).expect("open local page");
    assert_eq!(tab.url, url);
    assert!(
        browser
            .snapshot(&tab.id)
            .expect("snapshot")
            .contains("Mensagem")
    );
    browser.fill("#message", "browser ok").expect("fill");
    browser.click("#send").expect("click");
    assert!(
        browser
            .extract(Some("#result"))
            .expect("extract")
            .contains("browser ok")
    );
    browser.close().expect("close browser");
    stop.store(true, Ordering::Relaxed);
    server.join().expect("page server");
}
