use crate::{
    config::AppConfig, memory::MemoryStore, policy::Policy, providers::ProviderRegistry,
    sessions::FileSessionStore,
};
use anyhow::Context;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{
        Path as AxumPath, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse},
    routing::{get, post},
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use std::{
    collections::HashMap,
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

#[derive(Clone)]
struct AppState {
    registry: ProviderRegistry,
    memory: Arc<Mutex<MemoryStore>>,
    sessions: Arc<Mutex<FileSessionStore>>,
    memory_enabled: bool,
    identity_context: Option<String>,
    workspace: PathBuf,
    config_path: PathBuf,
    webhook_dedup_path: PathBuf,
    rate_limit: Arc<Mutex<HashMap<String, RateBucket>>>,
    seen_webhook_events: Arc<Mutex<HashMap<String, u64>>>,
    max_requests_per_minute: u32,
    emergency_stop: Arc<AtomicBool>,
    auth_env: String,
}

#[derive(Debug, Clone, Copy)]
struct RateBucket {
    window_started: u64,
    count: u32,
}

#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub message: String,
    #[serde(default)]
    pub images: Vec<crate::providers::ImageInput>,
    pub provider: Option<String>,
    pub task: Option<String>,
    pub session: Option<String>,
    pub channel: Option<String>,
    pub identity: Option<String>,
    pub sender: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub answer: String,
    pub provider: String,
    pub latency_ms: u128,
    pub estimated_cost_usd: f64,
    pub cost_source: String,
    pub risk_score: u8,
    pub risk_flags: Vec<String>,
    pub fallback: bool,
    pub fallback_reason: Option<String>,
}

pub async fn serve(config: AppConfig, bind: String, config_path: PathBuf) -> anyhow::Result<()> {
    let data_dir = config_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let memory = MemoryStore::open_with_retention(
        data_dir.join("memory.jsonl"),
        config.memory_retention_days,
    )?;
    let sessions = FileSessionStore::open(data_dir.join("sessions.json"))?;
    let memory_enabled = config.features.memory;
    let identity_context = crate::identity::effective_prompt(&data_dir)?;
    let workspace = config.security.workspace.clone();
    let max_requests_per_minute = config.security.max_requests_per_minute.max(1);
    let auth_env = config.server.auth_env.clone();
    let webhook_dedup_path = data_dir.join("whatsapp-webhook-events.json");
    let scheduler_config = Arc::new(Mutex::new(config.clone()));
    let scheduler_path = config_path.clone();
    tokio::spawn(crate::scheduler::run(
        scheduler_config,
        scheduler_path,
        ProviderRegistry::new(config.clone()),
    ));
    let matrix_channels: Vec<crate::channels::ChannelConfig> = if config.features.channels {
        config
            .channels
            .iter()
            .filter(|channel| {
                channel.enabled && crate::channels::normalize_kind(&channel.kind) == "matrix"
            })
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    let signal_channels: Vec<crate::channels::ChannelConfig> = if config.features.channels {
        config
            .channels
            .iter()
            .filter(|channel| {
                channel.enabled && crate::channels::normalize_kind(&channel.kind) == "signal"
            })
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    let state = AppState {
        registry: ProviderRegistry::new(config),
        memory: Arc::new(Mutex::new(memory)),
        sessions: Arc::new(Mutex::new(sessions)),
        memory_enabled,
        identity_context,
        workspace,
        config_path: config_path.clone(),
        webhook_dedup_path: webhook_dedup_path.clone(),
        rate_limit: Arc::new(Mutex::new(HashMap::new())),
        seen_webhook_events: Arc::new(Mutex::new(load_webhook_dedup(&webhook_dedup_path))),
        max_requests_per_minute,
        emergency_stop: Arc::new(AtomicBool::new(false)),
        auth_env,
    };
    for channel in matrix_channels {
        let worker_state = state.clone();
        tokio::spawn(matrix_sync_worker(worker_state, channel));
    }
    for channel in signal_channels {
        let worker_state = state.clone();
        tokio::spawn(signal_receive_worker(worker_state, channel));
    }
    let app = Router::new()
        .route("/health", get(health))
        .route("/favicon.ico", get(favicon))
        .route("/", get(index))
        .route("/v1/chat", post(chat))
        .route("/v1/ws", get(websocket))
        .route("/v1/webhooks/{channel}", get(webhook_verify).post(webhook))
        .route("/v1/sessions/{id}/cancel", post(cancel_session))
        .route("/v1/emergency-stop", post(emergency_stop))
        .with_state(state);
    let addr: SocketAddr = bind.parse()?;
    println!("Sapiens Agent ativo em http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn matrix_sync_worker(state: AppState, channel: crate::channels::ChannelConfig) {
    let channel_name = channel.name.clone();
    let cursor_path = matrix_cursor_path(&state.config_path, &channel_name);
    let mut since = read_matrix_cursor(&cursor_path);
    let own_user_id = match crate::channels::matrix_user_id(&channel).await {
        Ok(user_id) => user_id,
        Err(error) => {
            let _ = crate::observability::append_receipt(
                &state.config_path,
                "channel.matrix.health",
                "read",
                false,
                &format!("channel={channel_name} error={error}"),
                Some(&channel_name),
            );
            return;
        }
    };
    let mut retry_delay = std::time::Duration::from_secs(2);
    loop {
        if state.emergency_stop.load(Ordering::Acquire) {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            continue;
        }
        let endpoint = match crate::channels::matrix_endpoint("sync") {
            Ok(endpoint) => endpoint,
            Err(error) => {
                record_matrix_worker_error(&state, &channel_name, "endpoint", &error);
                tokio::time::sleep(retry_delay).await;
                retry_delay = (retry_delay * 2).min(std::time::Duration::from_secs(60));
                continue;
            }
        };
        let policy = Policy {
            mode: "readonly".into(),
            workspace: state.workspace.clone(),
            allowed_domains: Vec::new(),
            allow_private_networks: false,
        };
        if let Err(error) = policy.check_url(&endpoint) {
            record_matrix_worker_error(&state, &channel_name, "policy", &error);
            break;
        }
        match crate::channels::matrix_sync(&channel, since.as_deref(), 30_000).await {
            Ok(batch) => {
                retry_delay = std::time::Duration::from_secs(2);
                let mut advance_cursor = true;
                for message in batch.messages {
                    if own_user_id == message.sender {
                        continue;
                    }
                    if !crate::channels::matrix_inbound_allowed(&channel, &message) {
                        let _ = crate::observability::append_receipt(
                            &state.config_path,
                            "channel.matrix.reject",
                            "external_write",
                            false,
                            &format!(
                                "event={} room={} sender=not-allowlisted",
                                message.event_id, message.room_id
                            ),
                            Some(&message.room_id),
                        );
                        continue;
                    }
                    let mut request_images = Vec::new();
                    let request_message = if let Some(media_uri) = message.media_uri.as_deref() {
                        let output_dir = state
                            .workspace
                            .join("matrix-media")
                            .join(safe_component(&channel_name));
                        let policy = read_only_policy(&state);
                        let max_bytes = std::env::var("SAPIENS_MATRIX_MAX_MEDIA_BYTES")
                            .ok()
                            .and_then(|value| value.parse::<u64>().ok())
                            .unwrap_or(25 * 1024 * 1024)
                            .min(50 * 1024 * 1024);
                        match crate::channels::matrix_download_media(
                            &channel,
                            media_uri,
                            &output_dir,
                            max_bytes,
                            message.mime_type.as_deref(),
                            message.media_size,
                            &policy,
                        )
                        .await
                        {
                            Ok(media) => {
                                if media.mime_type.starts_with("image/")
                                    && let Ok(image) = crate::providers::image_input_from_path(
                                        Path::new(&media.path),
                                    )
                                {
                                    request_images.push(image);
                                }
                                format!(
                                    "O usuário enviou uma mídia Matrix do tipo {} ({} bytes). Arquivo recebido no workspace em {}.{}",
                                    message.msgtype,
                                    media.size_bytes,
                                    media.path,
                                    message
                                        .filename
                                        .as_deref()
                                        .filter(|value| !value.trim().is_empty())
                                        .map(|filename| format!(" Nome informado: {filename}."))
                                        .unwrap_or_default()
                                )
                            }
                            Err(error) => {
                                let _ = crate::observability::append_receipt(
                                    &state.config_path,
                                    "channel.matrix.media",
                                    "read",
                                    false,
                                    &format!(
                                        "event={} room={} media_uri={} error={error}",
                                        message.event_id, message.room_id, media_uri
                                    ),
                                    Some(&message.room_id),
                                );
                                continue;
                            }
                        }
                    } else {
                        message.body.clone()
                    };
                    let request = ChatRequest {
                        message: request_message,
                        images: request_images,
                        provider: None,
                        task: Some("matrix.inbound".into()),
                        session: Some(format!("matrix:{}:{}", channel_name, message.room_id)),
                        channel: Some("matrix".into()),
                        identity: Some(message.sender.clone()),
                        sender: Some(message.sender.clone()),
                    };
                    match process_chat(&state, request).await {
                        Ok(response) => {
                            let approved = matrix_auto_reply_approved(&state.config_path);
                            if approved {
                                if let Err(error) = crate::channels::matrix_send(
                                    &channel,
                                    &message.room_id,
                                    &response.answer,
                                )
                                .await
                                {
                                    advance_cursor = false;
                                    record_matrix_worker_error(
                                        &state,
                                        &channel_name,
                                        "send",
                                        &error,
                                    );
                                } else {
                                    let _ = crate::observability::append_receipt(
                                        &state.config_path,
                                        "channel.matrix.reply",
                                        "external_write",
                                        true,
                                        &format!(
                                            "event={} room={}",
                                            message.event_id, message.room_id
                                        ),
                                        Some(&message.room_id),
                                    );
                                }
                            } else {
                                let _ = crate::observability::append_receipt(
                                    &state.config_path,
                                    "channel.matrix.inbound",
                                    "read",
                                    false,
                                    &format!(
                                        "event={} room={} auto_reply=disabled",
                                        message.event_id, message.room_id
                                    ),
                                    Some(&message.room_id),
                                );
                            }
                            let _ = response;
                        }
                        Err((status, error)) => {
                            if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
                                advance_cursor = false;
                            }
                            record_matrix_worker_error(
                                &state,
                                &channel_name,
                                "process",
                                &anyhow::anyhow!("HTTP {status}: {error}"),
                            );
                        }
                    }
                }
                if advance_cursor && !batch.next_batch.is_empty() {
                    if let Err(error) = write_matrix_cursor(&cursor_path, &batch.next_batch) {
                        record_matrix_worker_error(&state, &channel_name, "cursor", &error);
                    } else {
                        since = Some(batch.next_batch);
                    }
                }
            }
            Err(error) => {
                record_matrix_worker_error(&state, &channel_name, "sync", &error);
                tokio::time::sleep(retry_delay).await;
                retry_delay = (retry_delay * 2).min(std::time::Duration::from_secs(60));
            }
        }
    }
}

fn matrix_auto_reply_approved(config_path: &std::path::Path) -> bool {
    auto_reply_approved(
        config_path,
        "SAPIENS_MATRIX_AUTO_REPLY",
        "channel.matrix.send",
    )
}

fn signal_auto_reply_approved(config_path: &std::path::Path) -> bool {
    auto_reply_approved(
        config_path,
        "SAPIENS_SIGNAL_AUTO_REPLY",
        "channel.signal.send",
    )
}

fn whatsapp_auto_reply_approved(config_path: &std::path::Path) -> bool {
    auto_reply_approved(
        config_path,
        "SAPIENS_WHATSAPP_AUTO_REPLY",
        "channel.whatsapp.send",
    )
}

fn auto_reply_approved(config_path: &std::path::Path, flag: &str, action: &str) -> bool {
    let requested = std::env::var(flag)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
        .unwrap_or(false);
    if !requested {
        return false;
    }
    let Ok(config) = crate::config::load(config_path) else {
        return false;
    };
    if config.security.mode == "trusted" {
        return true;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    config.approvals.iter().any(|approval| {
        approval.expires_at > now
            && (approval.scope == action
                || approval.scope == "channel.send"
                || approval.scope == "channel.*")
    })
}

async fn signal_receive_worker(state: AppState, channel: crate::channels::ChannelConfig) {
    let channel_name = channel.name.clone();
    let mut retry_delay = std::time::Duration::from_secs(5);
    loop {
        if state.emergency_stop.load(Ordering::Acquire) {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            continue;
        }
        match crate::channels::signal_receive(&channel, 5).await {
            Ok(messages) => {
                retry_delay = std::time::Duration::from_secs(5);
                for message in messages {
                    if !crate::channels::signal_inbound_allowed(&channel, &message) {
                        let _ = crate::observability::append_receipt(
                            &state.config_path,
                            "channel.signal.reject",
                            "external_write",
                            false,
                            &format!("event={} sender=not-allowlisted", message.event_id),
                            Some(&message.sender),
                        );
                        continue;
                    }
                    let conversation = message
                        .group_id
                        .as_deref()
                        .unwrap_or(message.sender.as_str())
                        .to_string();
                    let mut request_message = message.text.clone();
                    let mut downloaded_media = Vec::new();
                    let mut request_images = Vec::new();
                    let max_bytes = std::env::var("SAPIENS_SIGNAL_MAX_MEDIA_BYTES")
                        .ok()
                        .and_then(|value| value.parse::<u64>().ok())
                        .unwrap_or(25 * 1024 * 1024)
                        .min(50 * 1024 * 1024);
                    for attachment in &message.attachment_details {
                        let output_dir = state
                            .workspace
                            .join("signal-media")
                            .join(safe_component(&channel_name));
                        let policy = read_only_policy(&state);
                        match crate::channels::signal_download_attachment(
                            &channel,
                            attachment,
                            &message.sender,
                            message.group_id.as_deref(),
                            &output_dir,
                            max_bytes,
                            &policy,
                        )
                        .await
                        {
                            Ok(media) => downloaded_media.push(media),
                            Err(error) => {
                                record_signal_worker_error(&state, &channel_name, "media", &error);
                            }
                        }
                    }
                    for media in &downloaded_media {
                        if media.mime_type.starts_with("image/")
                            && let Ok(image) =
                                crate::providers::image_input_from_path(Path::new(&media.path))
                        {
                            request_images.push(image);
                        }
                        if !request_message.is_empty() {
                            request_message.push('\n');
                        }
                        request_message.push_str(&format!(
                            "Mídia Signal recebida: {} ({} bytes, {}). Arquivo no workspace: {}.",
                            media.attachment_id, media.size_bytes, media.mime_type, media.path
                        ));
                    }
                    if request_message.trim().is_empty() {
                        continue;
                    }
                    let request = ChatRequest {
                        message: request_message,
                        images: request_images,
                        provider: None,
                        task: Some("signal.inbound".into()),
                        session: Some(format!("signal:{channel_name}:{conversation}")),
                        channel: Some("signal".into()),
                        identity: Some(message.sender.clone()),
                        sender: Some(message.sender.clone()),
                    };
                    match process_chat(&state, request).await {
                        Ok(response) if signal_auto_reply_approved(&state.config_path) => {
                            let result = if let Some(group_id) = message.group_id.as_deref() {
                                crate::channels::signal_send_group(
                                    &channel,
                                    group_id,
                                    &response.answer,
                                )
                                .await
                            } else {
                                crate::channels::signal_send(
                                    &channel,
                                    &message.sender,
                                    &response.answer,
                                )
                                .await
                            };
                            if let Err(error) = result {
                                record_signal_worker_error(&state, &channel_name, "send", &error);
                            } else {
                                let _ = crate::observability::append_receipt(
                                    &state.config_path,
                                    "channel.signal.reply",
                                    "external_write",
                                    true,
                                    &format!("event={}", message.event_id),
                                    Some(&conversation),
                                );
                            }
                        }
                        Ok(_) => {
                            let _ = crate::observability::append_receipt(
                                &state.config_path,
                                "channel.signal.inbound",
                                "read",
                                false,
                                &format!("event={} auto_reply=disabled", message.event_id),
                                Some(&conversation),
                            );
                        }
                        Err((status, error)) => record_signal_worker_error(
                            &state,
                            &channel_name,
                            "process",
                            &anyhow::anyhow!("HTTP {status}: {error}"),
                        ),
                    }
                }
            }
            Err(error) => {
                record_signal_worker_error(&state, &channel_name, "receive", &error);
                tokio::time::sleep(retry_delay).await;
                retry_delay = (retry_delay * 2).min(std::time::Duration::from_secs(60));
            }
        }
    }
}

fn record_signal_worker_error(
    state: &AppState,
    channel_name: &str,
    phase: &str,
    error: &anyhow::Error,
) {
    let _ = crate::observability::append_receipt(
        &state.config_path,
        "channel.signal.error",
        "read",
        false,
        &format!("channel={channel_name} phase={phase} error={error}"),
        Some(channel_name),
    );
}

fn read_only_policy(state: &AppState) -> Policy {
    let (allowed_domains, allow_private_networks) = crate::config::load(&state.config_path)
        .map(|config| {
            (
                config.security.allowed_domains,
                config.security.allow_private_networks,
            )
        })
        .unwrap_or_default();
    Policy {
        mode: "readonly".into(),
        workspace: state.workspace.clone(),
        allowed_domains,
        allow_private_networks,
    }
}

fn safe_component(value: &str) -> String {
    let component: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if component.is_empty() {
        "channel".into()
    } else {
        component
    }
}

fn matrix_cursor_path(config_path: &std::path::Path, channel_name: &str) -> PathBuf {
    let safe: String = channel_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    config_path.with_file_name(format!("matrix-{safe}.cursor"))
}

fn read_matrix_cursor(path: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn write_matrix_cursor(path: &std::path::Path, cursor: &str) -> anyhow::Result<()> {
    let temp = path.with_extension("cursor.tmp");
    std::fs::write(&temp, cursor)?;
    std::fs::rename(temp, path)?;
    Ok(())
}

fn record_matrix_worker_error(
    state: &AppState,
    channel_name: &str,
    phase: &str,
    error: &anyhow::Error,
) {
    let _ = crate::observability::append_receipt(
        &state.config_path,
        "channel.matrix.error",
        "read",
        false,
        &format!("channel={channel_name} phase={phase} error={error}"),
        Some(channel_name),
    );
}

async fn health(State(state): State<AppState>) -> (StatusCode, &'static str) {
    if state.emergency_stop.load(Ordering::Acquire) {
        (StatusCode::SERVICE_UNAVAILABLE, "emergency stop active")
    } else {
        (StatusCode::OK, "ok")
    }
}

async fn favicon() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn index() -> Html<&'static str> {
    Html(
        r#"<!doctype html><meta charset=utf-8><meta name=viewport content="width=device-width,initial-scale=1"><title>Sapiens Agent</title><style>:root{color-scheme:dark;--bg:#0b1020;--panel:#121a2d;--line:#263653;--text:#e8eefc;--muted:#92a4c6;--accent:#6ea8fe}*{box-sizing:border-box}body{font:16px system-ui,-apple-system,Segoe UI,sans-serif;background:radial-gradient(circle at top,#172542,var(--bg) 55%);color:var(--text);max-width:820px;margin:0 auto;padding:48px 20px}main{background:rgba(18,26,45,.9);border:1px solid var(--line);border-radius:18px;padding:28px;box-shadow:0 20px 60px #0006}h1{margin:0 0 6px;letter-spacing:.04em}p{color:var(--muted);margin-top:0}textarea{display:block;width:100%;min-height:150px;resize:vertical;background:#0b1222;color:var(--text);border:1px solid var(--line);border-radius:10px;padding:14px;font:inherit;margin:22px 0 12px}button{background:var(--accent);color:#08101f;border:0;border-radius:9px;padding:10px 18px;font-weight:700;cursor:pointer}button:disabled{opacity:.55;cursor:wait}pre{white-space:pre-wrap;background:#0b1222;border:1px solid var(--line);border-radius:10px;padding:14px;min-height:52px;color:#cfe0ff}</style><main><h1>Sapiens Agent</h1><p>Gateway local • modo seguro • pronto para conversar</p><textarea id=m aria-label="Mensagem" placeholder="Digite uma mensagem..."></textarea><button id=b onclick=send()>Enviar</button><pre id=o aria-live=polite></pre><script>async function send(){let m=document.getElementById('m').value.trim(),b=document.getElementById('b'),o=document.getElementById('o');if(!m){o.textContent='Digite uma mensagem.';return}b.disabled=true;o.textContent='Processando...';try{let r=await fetch('/v1/chat',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({message:m})}),data=await r.json();o.textContent=data.answer||data.error||JSON.stringify(data,null,2)}catch(e){o.textContent='Falha de conexão com o gateway.'}finally{b.disabled=false}}</script></main>"#,
    )
}

async fn chat(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ChatRequest>,
) -> impl IntoResponse {
    if let Err((status, error)) = authorize(&state, &headers) {
        return (
            status,
            Json(serde_json::json!({
                "error": error,
                "status": status.as_u16(),
            })),
        )
            .into_response();
    }
    match process_chat(&state, req).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err((status, error)) => (
            status,
            Json(serde_json::json!({
                "error": error,
                "status": status.as_u16(),
            })),
        )
            .into_response(),
    }
}

async fn process_chat(
    state: &AppState,
    req: ChatRequest,
) -> Result<ChatResponse, (StatusCode, String)> {
    if req.message.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "message is required".into()));
    }
    let assessment = crate::policy::assess_prompt(&req.message);
    if assessment.blocked {
        let flags = assessment.flags.join(",");
        let _ = crate::observability::append_receipt(
            &state.config_path,
            "chat.blocked",
            "prompt_injection",
            false,
            &format!("risk_score={} flags={flags}", assessment.score),
            req.session.as_deref(),
        );
        return Err((
            StatusCode::FORBIDDEN,
            format!("request blocked by safety policy: {flags}"),
        ));
    }
    if state.emergency_stop.load(Ordering::Acquire) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "emergency stop active".into(),
        ));
    }
    let task = req.task.as_deref().unwrap_or("general");
    let channel = req.channel.as_deref().unwrap_or("webchat");
    let identity = req.identity.as_deref().unwrap_or("local");
    let rate_key = format!("{channel}:{identity}");
    if !allow_request(&state.rate_limit, &rate_key, state.max_requests_per_minute).await {
        return Err((StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded".into()));
    }
    let requested_session = req.session.is_some();
    let session_id = req
        .session
        .clone()
        .unwrap_or_else(|| format!("{channel}:{identity}"));
    let mut sessions = state.sessions.lock().await;
    let mut session_id = session_id;
    let session = sessions
        .ensure(
            &session_id,
            channel,
            identity,
            "sapiens-agent",
            &state.workspace,
        )
        .map_err(internal_error)?;
    if session.cancelled {
        if requested_session {
            return Err((StatusCode::CONFLICT, "session is cancelled".into()));
        }
        let rotated = sessions.rotate(&session_id).map_err(internal_error)?;
        session_id = rotated.id;
        sessions
            .ensure(
                &session_id,
                channel,
                identity,
                "sapiens-agent",
                &state.workspace,
            )
            .map_err(internal_error)?;
    }
    drop(sessions);
    let model_message = crate::policy::user_content_for_model(&req.message);
    let outcome = state
        .registry
        .chat_detailed_with_context_and_images(
            req.provider.as_deref(),
            task,
            &model_message,
            state.identity_context.as_deref(),
            &req.images,
        )
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
    let fallback_reason = outcome
        .attempts
        .first()
        .map(|attempt| format!("{}: {}", attempt.alias, attempt.error));
    if state.memory_enabled {
        let _ = state.memory.lock().await.append(
            &session_id,
            &format!("user: {}\nassistant: {}", req.message, outcome.answer),
        );
    }
    Ok(ChatResponse {
        answer: outcome.answer,
        provider: outcome.provider,
        latency_ms: outcome.latency_ms,
        estimated_cost_usd: outcome.estimated_cost_usd,
        cost_source: outcome.cost_source,
        risk_score: assessment.score,
        risk_flags: assessment.flags,
        fallback: !outcome.attempts.is_empty(),
        fallback_reason,
    })
}

async fn websocket(
    upgrade: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    authorize(&state, &headers)?;
    Ok(upgrade.on_upgrade(move |socket| websocket_loop(socket, state)))
}

async fn websocket_loop(mut socket: WebSocket, state: AppState) {
    while let Some(Ok(message)) = socket.recv().await {
        let Message::Text(text) = message else {
            continue;
        };
        let text = text.to_string();
        let request = serde_json::from_str::<ChatRequest>(&text).unwrap_or(ChatRequest {
            message: text,
            images: Vec::new(),
            provider: None,
            task: None,
            session: None,
            channel: Some("websocket".into()),
            identity: Some("local".into()),
            sender: Some("local".into()),
        });
        let response = match process_chat(&state, request).await {
            Ok(response) => serde_json::to_string(&response)
                .unwrap_or_else(|_| "{\"error\":\"response serialization failed\"}".into()),
            Err((status, error)) => serde_json::json!({
                "error": error,
                "status": status.as_u16(),
            })
            .to_string(),
        };
        if socket.send(Message::Text(response.into())).await.is_err() {
            break;
        }
    }
}

async fn webhook(
    State(state): State<AppState>,
    AxumPath(channel_name): AxumPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, (StatusCode, String)> {
    authorize(&state, &headers)?;
    let configured = load_webhook_channel(&state, &channel_name).await?;
    if body.len() > 1_048_576 {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            "webhook payload exceeds 1 MiB".into(),
        ));
    }
    if crate::channels::normalize_kind(&configured.kind) == "whatsapp" {
        verify_whatsapp_signature(&headers, &body)?;
        return process_whatsapp_webhook(&state, &configured, &channel_name, &body).await;
    }
    let mut req: ChatRequest = serde_json::from_slice(&body).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid webhook JSON: {error}"),
        )
    })?;
    let expected = std::env::var(&configured.credential_env).map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "webhook credential unavailable".into(),
        )
    })?;
    let supplied = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .or_else(|| {
            headers
                .get("x-sapiens-channel-token")
                .and_then(|value| value.to_str().ok())
        })
        .ok_or((StatusCode::UNAUTHORIZED, "webhook token required".into()))?;
    if !constant_time_equal(supplied.as_bytes(), expected.as_bytes()) {
        return Err((StatusCode::UNAUTHORIZED, "webhook token rejected".into()));
    }
    let sender = req
        .sender
        .clone()
        .or_else(|| req.identity.clone())
        .ok_or((StatusCode::FORBIDDEN, "webhook sender required".into()))?;
    if configured.allowlist.is_empty()
        || !configured
            .allowlist
            .iter()
            .any(|allowed| allowed == "*" || allowed == &sender)
    {
        return Err((
            StatusCode::FORBIDDEN,
            "webhook sender is not allowlisted".into(),
        ));
    }
    req.channel = Some("webhooks".into());
    req.identity = Some(sender.clone());
    req.sender = Some(sender);
    Ok(Json(
        serde_json::to_value(process_chat(&state, req).await?)
            .map_err(|error| internal_error(error.into()))?,
    ))
}

async fn webhook_verify(
    State(state): State<AppState>,
    AxumPath(channel_name): AxumPath<String>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Result<(StatusCode, String), (StatusCode, String)> {
    authorize(&state, &headers)?;
    let configured = load_webhook_channel(&state, &channel_name).await?;
    if crate::channels::normalize_kind(&configured.kind) != "whatsapp" {
        return Err((
            StatusCode::METHOD_NOT_ALLOWED,
            "GET verification is only available for WhatsApp channels".into(),
        ));
    }
    let mode = query
        .get("hub.mode")
        .map(String::as_str)
        .unwrap_or_default();
    let supplied = query
        .get("hub.verify_token")
        .map(String::as_str)
        .unwrap_or_default();
    let challenge = query
        .get("hub.challenge")
        .cloned()
        .ok_or((StatusCode::BAD_REQUEST, "hub.challenge is required".into()))?;
    let expected = std::env::var("SAPIENS_WHATSAPP_VERIFY_TOKEN").map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "WhatsApp webhook verify token is unavailable".into(),
        )
    })?;
    if mode != "subscribe" || !constant_time_equal(supplied.as_bytes(), expected.as_bytes()) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "WhatsApp webhook verification rejected".into(),
        ));
    }
    Ok((StatusCode::OK, challenge))
}

async fn load_webhook_channel(
    state: &AppState,
    name: &str,
) -> Result<crate::channels::ChannelConfig, (StatusCode, String)> {
    let _ = state;
    let config = crate::config::load(&state.config_path).map_err(internal_error)?;
    let channel = config
        .channels
        .into_iter()
        .find(|item| {
            item.name == name
                && matches!(
                    crate::channels::normalize_kind(&item.kind).as_str(),
                    "webhooks" | "whatsapp"
                )
        })
        .ok_or((StatusCode::NOT_FOUND, "webhook channel not found".into()))?;
    if !channel.enabled {
        return Err((StatusCode::FORBIDDEN, "webhook channel is disabled".into()));
    }
    Ok(channel)
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    let mut difference: u8 = if left.len() == right.len() { 0 } else { 1 };
    for index in 0..left.len().max(right.len()) {
        difference |= left.get(index).copied().unwrap_or_default()
            ^ right.get(index).copied().unwrap_or_default();
    }
    difference == 0
}

type HmacSha256 = Hmac<Sha256>;

fn verify_whatsapp_signature(headers: &HeaderMap, body: &[u8]) -> Result<(), (StatusCode, String)> {
    let secret = std::env::var("SAPIENS_WHATSAPP_APP_SECRET").map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "WhatsApp webhook app secret is unavailable".into(),
        )
    })?;
    if secret.is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "WhatsApp webhook app secret is empty".into(),
        ));
    }
    let supplied = headers
        .get("x-hub-signature-256")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("sha256="))
        .and_then(decode_hex)
        .ok_or((
            StatusCode::UNAUTHORIZED,
            "WhatsApp webhook signature required".into(),
        ))?;
    verify_signature_with_secret(body, secret.as_bytes(), &supplied).map_err(|_| {
        (
            StatusCode::UNAUTHORIZED,
            "WhatsApp webhook signature rejected".into(),
        )
    })
}

fn verify_signature_with_secret(body: &[u8], secret: &[u8], supplied: &[u8]) -> anyhow::Result<()> {
    let mut mac = HmacSha256::new_from_slice(secret).context("invalid webhook secret")?;
    mac.update(body);
    mac.verify_slice(supplied)
        .map_err(|_| anyhow::anyhow!("webhook signature mismatch"))
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    let mut decoded = Vec::with_capacity(value.len() / 2);
    let bytes = value.as_bytes();
    for index in (0..bytes.len()).step_by(2) {
        let high = hex_digit(bytes[index])?;
        let low = hex_digit(bytes[index + 1])?;
        decoded.push((high << 4) | low);
    }
    Some(decoded)
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct WhatsAppInboundMessage {
    id: String,
    sender: String,
    timestamp: Option<String>,
    kind: String,
    text: Option<String>,
    media_id: Option<String>,
    caption: Option<String>,
}

#[derive(Debug, Clone)]
struct WhatsAppStatusEvent {
    id: String,
    status: String,
    recipient: Option<String>,
    timestamp: Option<String>,
    errors: Vec<String>,
}

#[derive(Debug, Default)]
struct WhatsAppParsedEvents {
    messages: Vec<WhatsAppInboundMessage>,
    statuses: Vec<WhatsAppStatusEvent>,
}

fn parse_whatsapp_events(body: &[u8]) -> anyhow::Result<WhatsAppParsedEvents> {
    let payload: Value =
        serde_json::from_slice(body).context("WhatsApp webhook JSON is invalid")?;
    if payload.get("object").and_then(Value::as_str) != Some("whatsapp_business_account") {
        anyhow::bail!("unsupported WhatsApp webhook object");
    }
    let mut parsed = WhatsAppParsedEvents::default();
    let entries = payload
        .get("entry")
        .and_then(Value::as_array)
        .context("WhatsApp webhook entry is missing")?;
    for entry in entries {
        let changes = entry
            .get("changes")
            .and_then(Value::as_array)
            .context("WhatsApp webhook changes are missing")?;
        for change in changes {
            if change.get("field").and_then(Value::as_str) != Some("messages") {
                continue;
            }
            if let Some(items) = change.pointer("/value/messages").and_then(Value::as_array) {
                for item in items {
                    let Some(sender) = item.get("from").and_then(Value::as_str) else {
                        continue;
                    };
                    let kind = item.get("type").and_then(Value::as_str).unwrap_or_default();
                    let text = item.pointer("/text/body").and_then(Value::as_str);
                    let media_id = item
                        .pointer(&format!("/{kind}/id"))
                        .and_then(Value::as_str)
                        .filter(|value| !value.trim().is_empty())
                        .map(str::to_string);
                    let caption = item
                        .pointer(&format!("/{kind}/caption"))
                        .and_then(Value::as_str)
                        .filter(|value| !value.trim().is_empty())
                        .map(str::to_string);
                    let is_text =
                        kind == "text" && text.is_some_and(|value| !value.trim().is_empty());
                    let is_media =
                        matches!(kind, "image" | "audio" | "video" | "document" | "sticker")
                            && media_id.is_some();
                    if !is_text && !is_media {
                        continue;
                    }
                    if sender.trim().is_empty() {
                        continue;
                    }
                    parsed.messages.push(WhatsAppInboundMessage {
                        id: item
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .to_string(),
                        sender: sender.to_string(),
                        timestamp: item
                            .get("timestamp")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        kind: kind.to_string(),
                        text: text.map(str::to_string),
                        media_id,
                        caption,
                    });
                    if parsed.messages.len() >= 50 {
                        anyhow::bail!("WhatsApp webhook contains too many messages");
                    }
                }
            }
            if let Some(items) = change.pointer("/value/statuses").and_then(Value::as_array) {
                for item in items {
                    let Some(id) = item
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|value| !value.trim().is_empty())
                    else {
                        continue;
                    };
                    let Some(status) = item
                        .get("status")
                        .and_then(Value::as_str)
                        .filter(|value| !value.trim().is_empty())
                    else {
                        continue;
                    };
                    let errors = item
                        .get("errors")
                        .and_then(Value::as_array)
                        .map(|errors| {
                            errors
                                .iter()
                                .filter_map(|error| {
                                    error
                                        .get("title")
                                        .or_else(|| error.get("message"))
                                        .and_then(Value::as_str)
                                })
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default();
                    parsed.statuses.push(WhatsAppStatusEvent {
                        id: id.to_string(),
                        status: status.to_string(),
                        recipient: item
                            .get("recipient_id")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        timestamp: item
                            .get("timestamp")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        errors,
                    });
                    if parsed.statuses.len() >= 100 {
                        anyhow::bail!("WhatsApp webhook contains too many status events");
                    }
                }
            }
        }
    }
    Ok(parsed)
}

async fn process_whatsapp_webhook(
    state: &AppState,
    configured: &crate::channels::ChannelConfig,
    channel_name: &str,
    body: &[u8],
) -> Result<Json<Value>, (StatusCode, String)> {
    let parsed = parse_whatsapp_events(body).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid WhatsApp webhook: {error}"),
        )
    })?;
    let mut responses = Vec::with_capacity(parsed.messages.len() + parsed.statuses.len());
    for status in parsed.statuses {
        let dedup_key = format!("status:{}", status.id);
        if remember_webhook_event(state, &dedup_key).await {
            responses.push(serde_json::json!({
                "id": status.id,
                "status": "duplicate",
                "event": "delivery_status",
            }));
            continue;
        }
        let _ = crate::observability::append_receipt(
            &state.config_path,
            "channel.whatsapp.status",
            "read",
            false,
            &format!(
                "id={} status={} recipient={} errors={}",
                status.id,
                status.status,
                status.recipient.as_deref().unwrap_or("unknown"),
                status.errors.len()
            ),
            status.recipient.as_deref(),
        );
        responses.push(serde_json::json!({
            "id": status.id,
            "timestamp": status.timestamp,
            "event": "delivery_status",
            "status": status.status,
            "recipient": status.recipient,
            "errors": status.errors,
        }));
    }
    for inbound in parsed.messages {
        if inbound.id != "unknown" && remember_webhook_event(state, &inbound.id).await {
            responses.push(serde_json::json!({
                "id": inbound.id,
                "timestamp": inbound.timestamp,
                "status": "duplicate",
            }));
            continue;
        }
        if !configured
            .allowlist
            .iter()
            .any(|allowed| allowed == "*" || allowed == &inbound.sender)
        {
            return Err((
                StatusCode::FORBIDDEN,
                "WhatsApp sender is not allowlisted".into(),
            ));
        }
        let mut attachments = Vec::new();
        let message = if inbound.kind == "text" {
            inbound.text.clone().unwrap_or_default()
        } else {
            let Some(media_id) = inbound.media_id.as_deref() else {
                continue;
            };
            let output_dir = state
                .workspace
                .join("whatsapp-media")
                .join(safe_component(channel_name));
            let policy = read_only_policy(state);
            let max_bytes = std::env::var("SAPIENS_WHATSAPP_MAX_MEDIA_BYTES")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(25 * 1024 * 1024)
                .min(50 * 1024 * 1024);
            match crate::channels::whatsapp_download_media(
                configured,
                media_id,
                &output_dir,
                max_bytes,
                &policy,
            )
            .await
            {
                Ok(media) => {
                    attachments.push(media.clone());
                    format!(
                        "O usuário enviou uma mídia WhatsApp do tipo {} ({} bytes). Arquivo recebido no workspace em {}.{}",
                        inbound.kind,
                        media.size_bytes,
                        media.path,
                        inbound
                            .caption
                            .as_deref()
                            .map(|caption| format!(" Legenda: {caption}"))
                            .unwrap_or_default()
                    )
                }
                Err(error) => {
                    let _ = crate::observability::append_receipt(
                        &state.config_path,
                        "channel.whatsapp.media",
                        "read",
                        false,
                        &format!("id={} media_id={} error={error}", inbound.id, media_id),
                        Some(&inbound.sender),
                    );
                    responses.push(serde_json::json!({
                        "id": inbound.id,
                        "timestamp": inbound.timestamp,
                        "event": "media",
                        "status": "rejected",
                        "error": "media download was rejected by policy or integrity checks",
                    }));
                    continue;
                }
            }
        };
        let session = format!("whatsapp:{channel_name}:{}", inbound.sender);
        let images = attachments
            .iter()
            .filter(|media| media.mime_type.starts_with("image/"))
            .filter_map(|media| {
                crate::providers::image_input_from_path(Path::new(&media.path)).ok()
            })
            .collect();
        let request = ChatRequest {
            message,
            images,
            provider: None,
            task: Some("whatsapp.inbound".into()),
            session: Some(session),
            channel: Some("whatsapp".into()),
            identity: Some(inbound.sender.clone()),
            sender: Some(inbound.sender.clone()),
        };
        match process_chat(state, request).await {
            Ok(response) => {
                let mut reply_status = "processed";
                let auto_reply = whatsapp_auto_reply_approved(&state.config_path);
                let mut reply_error = None;
                if auto_reply {
                    if let Err(error) = crate::channels::whatsapp_send(
                        configured,
                        &inbound.sender,
                        &response.answer,
                    )
                    .await
                    {
                        reply_status = "reply_failed";
                        reply_error = Some(crate::observability::redact(&error.to_string()));
                    } else {
                        let _ = crate::observability::append_receipt(
                            &state.config_path,
                            "channel.whatsapp.reply",
                            "external_write",
                            true,
                            &format!("id={} recipient={}", inbound.id, inbound.sender),
                            Some(&inbound.sender),
                        );
                    }
                }
                responses.push(serde_json::json!({
                    "id": inbound.id,
                    "timestamp": inbound.timestamp,
                    "event": inbound.kind,
                    "status": reply_status,
                    "auto_reply": auto_reply,
                    "reply_error": reply_error,
                    "attachments": attachments,
                    "response": response,
                }));
            }
            Err((status, error)) => responses.push(serde_json::json!({
                "id": inbound.id,
                "timestamp": inbound.timestamp,
                "status": "rejected",
                "http_status": status.as_u16(),
                "error": error,
            })),
        }
    }
    Ok(Json(serde_json::json!({
        "received": true,
        "channel": "whatsapp",
        "processed": responses.iter().filter(|item| item["status"] == "processed").count(),
        "responses": responses,
    })))
}

const WEBHOOK_DEDUP_RETENTION_SECS: u64 = 86_400;
const MAX_WEBHOOK_DEDUP_ENTRIES: usize = 10_000;

fn load_webhook_dedup(path: &std::path::Path) -> HashMap<String, u64> {
    let now = unix_now();
    let Ok(text) = fs::read_to_string(path) else {
        return HashMap::new();
    };
    let Ok(mut seen) = serde_json::from_str::<HashMap<String, u64>>(&text) else {
        return HashMap::new();
    };
    seen.retain(|key, timestamp| {
        !key.trim().is_empty()
            && key.len() <= 256
            && now.saturating_sub(*timestamp) < WEBHOOK_DEDUP_RETENTION_SECS
    });
    trim_webhook_dedup(&mut seen);
    seen
}

fn trim_webhook_dedup(seen: &mut HashMap<String, u64>) {
    while seen.len() > MAX_WEBHOOK_DEDUP_ENTRIES {
        let Some(oldest) = seen
            .iter()
            .min_by_key(|(_, timestamp)| **timestamp)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        seen.remove(&oldest);
    }
}

fn persist_webhook_dedup(
    path: &std::path::Path,
    seen: &HashMap<String, u64>,
) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, serde_json::to_vec(seen)?)?;
    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(path);
        fs::rename(&temp, path).map_err(|_| error)?;
    }
    Ok(())
}

async fn remember_webhook_event(state: &AppState, key: &str) -> bool {
    let now = unix_now();
    let (duplicate, snapshot) = {
        let mut seen = state.seen_webhook_events.lock().await;
        seen.retain(|_, timestamp| now.saturating_sub(*timestamp) < WEBHOOK_DEDUP_RETENTION_SECS);
        if seen.contains_key(key) {
            (true, None)
        } else {
            seen.insert(key.to_string(), now);
            trim_webhook_dedup(&mut seen);
            (false, Some(seen.clone()))
        }
    };
    if let Some(snapshot) = snapshot
        && let Err(error) = persist_webhook_dedup(&state.webhook_dedup_path, &snapshot)
    {
        let _ = crate::observability::append_receipt(
            &state.config_path,
            "channel.whatsapp.dedup",
            "write",
            false,
            &format!("persistent deduplication unavailable: {error}"),
            None,
        );
    }
    duplicate
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

async fn cancel_session(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    authorize(&state, &headers)?;
    let cancelled = state
        .sessions
        .lock()
        .await
        .cancel(&id)
        .map_err(internal_error)?;
    if !cancelled {
        return Err((StatusCode::NOT_FOUND, "session not found".into()));
    }
    Ok(Json(serde_json::json!({"id": id, "cancelled": true})))
}

async fn emergency_stop(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    authorize(&state, &headers)?;
    state.emergency_stop.store(true, Ordering::Release);
    crate::observability::append_receipt(
        &state.config_path,
        "emergency_stop",
        "destructive",
        true,
        "gateway stopped",
        None,
    )
    .map_err(internal_error)?;
    Ok(Json(serde_json::json!({"stopped": true})))
}

async fn allow_request(
    buckets: &Arc<Mutex<HashMap<String, RateBucket>>>,
    key: &str,
    limit: u32,
) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let mut buckets = buckets.lock().await;
    let bucket = buckets.entry(key.to_string()).or_insert(RateBucket {
        window_started: now,
        count: 0,
    });
    if now.saturating_sub(bucket.window_started) >= 60 {
        bucket.window_started = now;
        bucket.count = 0;
    }
    if bucket.count >= limit {
        return false;
    }
    bucket.count += 1;
    true
}

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), (StatusCode, String)> {
    if state.auth_env.trim().is_empty() {
        return Ok(());
    }
    let expected = std::env::var(&state.auth_env).map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "gateway auth unavailable".into(),
        )
    })?;
    let supplied = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .or_else(|| {
            headers
                .get("x-sapiens-auth")
                .and_then(|value| value.to_str().ok())
        })
        .ok_or((
            StatusCode::UNAUTHORIZED,
            "gateway authentication required".into(),
        ))?;
    if constant_time_equal(supplied.as_bytes(), expected.as_bytes()) {
        Ok(())
    } else {
        Err((
            StatusCode::UNAUTHORIZED,
            "gateway authentication rejected".into(),
        ))
    }
}

fn internal_error(error: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use hmac::Mac;

    #[test]
    fn parses_text_and_media_messages_and_delivery_statuses() {
        let payload = serde_json::json!({
            "object": "whatsapp_business_account",
            "entry": [{"changes": [{"field": "messages", "value": {
                "messages": [
                    {"from": "5511999999999", "id": "wamid.1", "timestamp": "1700000000", "type": "text", "text": {"body": "oi"}},
                    {"from": "5511999999999", "id": "wamid.2", "type": "image", "image": {"id": "media.1"}}
                ]
            }}, {"field": "messages", "value": {
                "statuses": [{"id":"wamid.sent","status":"delivered","recipient_id":"5511999999999","timestamp":"1700000001"}]
            }}]}]
        });
        let parsed = parse_whatsapp_events(payload.to_string().as_bytes()).expect("events");
        assert_eq!(parsed.statuses.len(), 1);
        assert_eq!(parsed.statuses[0].status, "delivered");
        let messages = parsed.messages;
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].sender, "5511999999999");
        assert_eq!(messages[0].text.as_deref(), Some("oi"));
        assert_eq!(messages[1].kind, "image");
        assert_eq!(messages[1].media_id.as_deref(), Some("media.1"));
    }

    #[test]
    fn verifies_whatsapp_signature_and_rejects_tampering() {
        let body = br#"{"object":"whatsapp_business_account"}"#;
        let mut mac = HmacSha256::new_from_slice(b"app-secret").expect("mac");
        mac.update(body);
        let signature = mac.finalize().into_bytes();
        assert!(verify_signature_with_secret(body, b"app-secret", &signature).is_ok());
        assert!(verify_signature_with_secret(body, b"wrong-secret", &signature).is_err());
    }

    #[test]
    fn rejects_invalid_whatsapp_objects() {
        let payload = br#"{"object":"other"}"#;
        assert!(parse_whatsapp_events(payload).is_err());
    }

    #[test]
    fn suppresses_duplicate_whatsapp_delivery_statuses_within_retention_window() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let root = std::env::temp_dir().join(format!(
                "sapiens-whatsapp-dedup-test-{}",
                std::process::id()
            ));
            std::fs::create_dir_all(&root).expect("root");
            let config_path = root.join("config.toml");
            let state = AppState {
                registry: ProviderRegistry::new(AppConfig::default()),
                memory: Arc::new(Mutex::new(
                    MemoryStore::open_with_retention(root.join("memory.jsonl"), 30)
                        .expect("memory"),
                )),
                sessions: Arc::new(Mutex::new(
                    FileSessionStore::open(root.join("sessions.json")).expect("sessions"),
                )),
                memory_enabled: false,
                identity_context: None,
                workspace: root.clone(),
                config_path,
                webhook_dedup_path: root.join("whatsapp-webhook-events.json"),
                rate_limit: Arc::new(Mutex::new(HashMap::new())),
                seen_webhook_events: Arc::new(Mutex::new(HashMap::new())),
                max_requests_per_minute: 60,
                emergency_stop: Arc::new(AtomicBool::new(false)),
                auth_env: String::new(),
            };
            let configured = crate::channels::ChannelConfig {
                name: "whatsapp".into(),
                kind: "whatsapp".into(),
                enabled: true,
                ..Default::default()
            };
            let payload = br#"{"object":"whatsapp_business_account","entry":[{"changes":[{"field":"messages","value":{"statuses":[{"id":"wamid.status-1","status":"delivered"}]}}]}]}"#;
            let first = process_whatsapp_webhook(&state, &configured, "whatsapp", payload)
                .await
                .expect("first status");
            assert_eq!(first.0["responses"][0]["status"], "delivered");
            let persisted = load_webhook_dedup(&state.webhook_dedup_path);
            assert!(persisted.contains_key("status:wamid.status-1"));
            *state.seen_webhook_events.lock().await = persisted;
            let second = process_whatsapp_webhook(&state, &configured, "whatsapp", payload)
                .await
                .expect("duplicate status");
            assert_eq!(second.0["responses"][0]["status"], "duplicate");
            std::fs::remove_dir_all(root).expect("cleanup");
        });
    }

    #[test]
    fn default_cancelled_chat_session_is_rotated_but_explicit_session_is_not() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let root = std::env::temp_dir().join(format!(
                "sapiens-default-session-rotation-test-{}",
                std::process::id()
            ));
            std::fs::create_dir_all(&root).expect("root");
            let sessions = FileSessionStore::open(root.join("sessions.json")).expect("sessions");
            let state = AppState {
                registry: ProviderRegistry::new(AppConfig::default()),
                memory: Arc::new(Mutex::new(
                    MemoryStore::open(root.join("memory.jsonl")).expect("memory"),
                )),
                sessions: Arc::new(Mutex::new(sessions)),
                memory_enabled: false,
                identity_context: None,
                workspace: root.clone(),
                config_path: root.join("config.toml"),
                webhook_dedup_path: root.join("webhook-events.json"),
                rate_limit: Arc::new(Mutex::new(HashMap::new())),
                seen_webhook_events: Arc::new(Mutex::new(HashMap::new())),
                max_requests_per_minute: 60,
                emergency_stop: Arc::new(AtomicBool::new(false)),
                auth_env: String::new(),
            };
            {
                let mut store = state.sessions.lock().await;
                store
                    .ensure("webchat:local", "webchat", "local", "sapiens-agent", &root)
                    .expect("default session");
                store.cancel("webchat:local").expect("cancel default");
            }
            let request = ChatRequest {
                message: "olá".into(),
                images: Vec::new(),
                provider: None,
                task: None,
                session: None,
                channel: None,
                identity: None,
                sender: None,
            };
            let error = process_chat(&state, request)
                .await
                .expect_err("missing provider should be the next failure");
            assert_eq!(error.0, StatusCode::BAD_GATEWAY);
            let sessions = state.sessions.lock().await.list();
            assert_eq!(sessions.len(), 1);
            assert!(!sessions[0].cancelled);
            assert_ne!(sessions[0].id, "webchat:local");

            {
                let mut store = state.sessions.lock().await;
                store
                    .ensure("webchat:local", "webchat", "local", "sapiens-agent", &root)
                    .expect("explicit session");
                store.cancel("webchat:local").expect("cancel explicit");
            }
            let explicit = ChatRequest {
                message: "olá".into(),
                images: Vec::new(),
                provider: None,
                task: None,
                session: Some("webchat:local".into()),
                channel: None,
                identity: None,
                sender: None,
            };
            let error = process_chat(&state, explicit)
                .await
                .expect_err("explicit cancelled session must remain blocked");
            assert_eq!(error.0, StatusCode::CONFLICT);
            std::fs::remove_dir_all(root).expect("cleanup");
        });
    }
}
