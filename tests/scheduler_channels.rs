use sapiens_agent::{
    channels::ChannelConfig,
    config::{ScheduleConfig, SecurityConfig},
    scheduler::deliver_outputs,
};
use std::sync::OnceLock;

static TEST_ENV_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

#[tokio::test]
async fn scheduled_delivery_reaches_matrix_and_whatsapp_adapters() {
    let _guard = TEST_ENV_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        for expected in ["/_matrix/client/v3/rooms/", "/123/messages"] {
            let (mut stream, _) = listener.accept().await.expect("request");
            let mut buffer = vec![0_u8; 16_384];
            let count = tokio::io::AsyncReadExt::read(&mut stream, &mut buffer)
                .await
                .expect("read");
            let request = String::from_utf8_lossy(&buffer[..count]);
            assert!(request.contains(expected), "unexpected request: {request}");
            let body = if expected.starts_with("/_matrix") {
                r#"{"event_id":"$matrix-test"}"#
            } else {
                r#"{"messages":[{"id":"wamid-test"}]}"#
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
                .await
                .expect("write");
        }
    });

    let matrix_home = format!("http://{address}");
    let whatsapp_base = format!("http://{address}/v20.0");
    let matrix_token_env = format!("SAPIENS_TEST_MATRIX_TOKEN_{}", std::process::id());
    let whatsapp_token_env = format!("SAPIENS_TEST_WHATSAPP_TOKEN_{}", std::process::id());
    unsafe {
        std::env::set_var("SAPIENS_MATRIX_HOMESERVER", &matrix_home);
        std::env::set_var("SAPIENS_WHATSAPP_GRAPH_BASE_URL", &whatsapp_base);
        std::env::set_var("SAPIENS_WHATSAPP_PHONE_NUMBER_ID", "123");
        std::env::set_var(&matrix_token_env, "matrix-test-token");
        std::env::set_var(&whatsapp_token_env, "whatsapp-test-token");
    }

    let security = SecurityConfig {
        allow_private_networks: true,
        ..Default::default()
    };
    let root = std::env::temp_dir().join(format!(
        "sapiens-scheduler-channel-integration-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).expect("root");
    let config_path = root.join("config.toml");
    let matrix = ChannelConfig {
        name: "matrix-test".into(),
        kind: "matrix".into(),
        enabled: true,
        credential_env: matrix_token_env.clone(),
        allowlist: vec!["!room:test".into()],
    };
    let whatsapp = ChannelConfig {
        name: "whatsapp-test".into(),
        kind: "whatsapp".into(),
        enabled: true,
        credential_env: whatsapp_token_env.clone(),
        allowlist: vec!["5511999999999".into()],
    };
    let matrix_job = ScheduleConfig {
        id: "matrix-job".into(),
        channel_name: Some(matrix.name.clone()),
        channel_recipient: Some("!room:test".into()),
        ..Default::default()
    };
    let whatsapp_job = ScheduleConfig {
        id: "whatsapp-job".into(),
        channel_name: Some(whatsapp.name.clone()),
        channel_recipient: Some("5511999999999".into()),
        ..Default::default()
    };
    deliver_outputs(
        &matrix_job,
        &[matrix],
        &security,
        &config_path,
        123,
        "matrix answer",
        None,
    )
    .await
    .expect("matrix delivery");
    deliver_outputs(
        &whatsapp_job,
        &[whatsapp],
        &security,
        &config_path,
        123,
        "whatsapp answer",
        None,
    )
    .await
    .expect("WhatsApp delivery");
    server.await.expect("server");

    let receipts =
        std::fs::read_to_string(root.join("sapiens-agent-receipts.jsonl")).expect("receipts");
    assert!(receipts.contains("matrix-test"));
    assert!(receipts.contains("whatsapp-test"));

    unsafe {
        std::env::remove_var("SAPIENS_MATRIX_HOMESERVER");
        std::env::remove_var("SAPIENS_WHATSAPP_GRAPH_BASE_URL");
        std::env::remove_var("SAPIENS_WHATSAPP_PHONE_NUMBER_ID");
        std::env::remove_var(&matrix_token_env);
        std::env::remove_var(&whatsapp_token_env);
    }
    std::fs::remove_dir_all(root).expect("cleanup");
}
