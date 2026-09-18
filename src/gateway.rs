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
    memory: Arc<Mutex<MemoryStore>>,
    sessions: Arc<Mutex<FileSessionStore>>,
    memory_enabled: bool,
    audio_enabled: bool,
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
pub struct ChatTurn {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub message: String,
    #[serde(default)]
    pub images: Vec<crate::providers::ImageInput>,
    #[serde(default)]
    pub history: Vec<ChatTurn>,
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
    let audio_enabled = config.features.audio && config.audio.enabled;
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
        memory: Arc::new(Mutex::new(memory)),
        sessions: Arc::new(Mutex::new(sessions)),
        memory_enabled,
        audio_enabled,
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
        .route("/v1/status", get(api_status))
        .route("/v1/providers", get(api_providers))
        .route("/v1/providers/test", post(api_provider_test))
        .route("/v1/config", get(api_config).post(api_config_update))
        .route("/v1/skills", get(api_skills))
        .route("/v1/sessions", get(api_sessions))
        .route("/v1/receipts", get(api_receipts))
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
                    if !state.audio_enabled
                        && (message.msgtype == "m.audio"
                            || message
                                .mime_type
                                .as_deref()
                                .is_some_and(|mime| mime.starts_with("audio/")))
                    {
                        let _ = crate::observability::append_receipt(
                            &state.config_path,
                            "channel.matrix.audio",
                            "read",
                            false,
                            &format!(
                                "event={} room={} audio=disabled",
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
                        history: Vec::new(),
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
                        if !state.audio_enabled
                            && attachment
                                .content_type
                                .as_deref()
                                .is_some_and(|mime| mime.starts_with("audio/"))
                        {
                            let _ = crate::observability::append_receipt(
                                &state.config_path,
                                "channel.signal.audio",
                                "read",
                                false,
                                &format!("attachment={} audio=disabled", attachment.id),
                                Some(&message.sender),
                            );
                            continue;
                        }
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
                        history: Vec::new(),
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
        r###"<!doctype html>
<html lang="pt-BR">
<head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Sapiens Agent — Control</title>
<style>
:root{color-scheme:dark;--bg:#0b0d0f;--surface:#121619;--surface2:#171c20;--surface3:#1c2328;--line:#2a343a;--text:#f1f4f2;--muted:#9aa7a8;--accent:#67d8c1;--accent2:#a5f0dc;--good:#72dfae;--warn:#f2bd68;--bad:#ff7d86;--shadow:0 22px 70px #0007}
*{box-sizing:border-box}body{margin:0;font:14px Inter,ui-sans-serif,system-ui,-apple-system,"Segoe UI",sans-serif;background:radial-gradient(circle at 80% -20%,#19332f 0,#0b0d0f 44%);color:var(--text)}button,input,select,textarea{font:inherit}button{cursor:pointer}.shell{display:grid;grid-template-columns:252px 1fr;min-height:100vh}.rail{padding:24px 16px;border-right:1px solid var(--line);background:#0d1012e8;backdrop-filter:blur(14px)}.brand{display:flex;gap:11px;align-items:center;margin:4px 8px 30px}.mark{width:36px;height:36px;border:1px solid var(--accent);border-radius:11px;display:grid;place-items:center;color:var(--accent2);font-weight:850;font-size:18px;box-shadow:0 0 22px #67d8c122}.brand strong{letter-spacing:.14em;font-size:13px}.brand small{display:block;color:var(--muted);font-size:10px;letter-spacing:.1em;margin-top:4px}.nav{display:grid;gap:4px}.nav button{display:flex;align-items:center;gap:10px;background:transparent;color:var(--muted);text-align:left;border:1px solid transparent;padding:10px 12px;border-radius:9px}.nav button.active,.nav button:hover{background:var(--surface3);border-color:var(--line);color:var(--text)}.nav .icon{width:18px;text-align:center;color:var(--accent)}.rail-foot{position:fixed;bottom:18px;width:220px;color:var(--muted);font-size:11px;line-height:1.5}.content{padding:30px clamp(18px,4vw,58px);max-width:1400px;width:100%}.top{display:flex;justify-content:space-between;gap:18px;align-items:flex-start;margin-bottom:24px}.eyebrow{color:var(--accent);font-weight:750;letter-spacing:.14em;text-transform:uppercase;font-size:10px}.top h1{margin:7px 0 5px;font-size:30px;letter-spacing:-.02em}.muted{color:var(--muted)}.pill{border:1px solid var(--line);border-radius:999px;padding:8px 12px;color:var(--good);white-space:nowrap;background:#0d1715}.pill.bad{color:var(--bad);background:#1b1013}.grid{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:12px}.card{background:linear-gradient(145deg,#171d20f2,#111517f2);border:1px solid var(--line);border-radius:14px;padding:18px;box-shadow:var(--shadow)}.card h2{font-size:16px;margin:0 0 7px;letter-spacing:-.01em}.card h3{font-size:13px;margin:0}.metric{font-size:25px;font-weight:800;margin-top:8px}.label{color:var(--muted);font-size:11px;letter-spacing:.04em;text-transform:uppercase}.section{margin-top:14px}..two{display:grid;grid-template-columns:1.35fr .65fr;gap:12px}..three{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:12px}..rows{display:grid;gap:8px}.row{display:flex;align-items:center;justify-content:space-between;gap:12px;padding:9px 0;border-bottom:1px solid #263034}.row:last-child{border-bottom:0}.tag{font-size:10px;border-radius:999px;padding:4px 8px;border:1px solid var(--line);white-space:nowrap}.on{color:var(--good)}.off{color:var(--warn)}.bad{color:var(--bad)}.provider{display:grid;grid-template-columns:1fr auto;gap:10px;padding:14px 0;border-bottom:1px solid #263034}.provider:last-child{border:0}.provider-meta{display:flex;gap:7px;align-items:center;flex-wrap:wrap;margin-top:6px}.provider-actions{display:flex;gap:7px;align-items:center;flex-wrap:wrap;justify-content:flex-end}.provider input{max-width:210px}.catalog{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:9px}.channel{padding:12px;border:1px solid var(--line);border-radius:11px;background:#111719}.channel strong{display:block;margin-bottom:5px}.skill{display:flex;justify-content:space-between;gap:10px;padding:11px 0;border-bottom:1px solid #263034}.skill:last-child{border:0}.notice{border-left:3px solid var(--accent);background:#10211f;padding:11px 13px;border-radius:0 9px 9px 0;color:#c8ded8;line-height:1.5}.empty{padding:18px 0;color:var(--muted)}textarea,input,select{width:100%;background:#0b1012;color:var(--text);border:1px solid var(--line);border-radius:9px;padding:10px;outline:0}textarea:focus,input:focus,select:focus{border-color:var(--accent);box-shadow:0 0 0 3px #67d8c11c}textarea{min-height:142px;resize:vertical}.field{display:grid;gap:6px}.field span{font-size:12px;color:var(--muted)}.actions{display:flex;gap:8px;flex-wrap:wrap;margin-top:11px}.primary,.secondary,.danger{border-radius:9px;padding:9px 13px;font-weight:700}.primary{background:var(--accent);color:#07100e;border:0}.primary:hover{background:var(--accent2)}.secondary{background:transparent;color:var(--text);border:1px solid var(--line)}.secondary:hover{background:var(--surface3)}.danger{background:transparent;color:var(--bad);border:1px solid #63323a}.result{white-space:pre-wrap;min-height:42px;color:#d6e5e1;background:#0b1012;border:1px solid var(--line);border-radius:9px;padding:11px;margin-top:11px}.chat-box{display:grid;gap:10px}.chat-head{display:flex;justify-content:space-between;align-items:center;gap:10px}.chat-head select{max-width:300px}.chat-answer{min-height:120px;line-height:1.6}.hide{display:none}.small{font-size:12px}.mono{font-family:ui-monospace,SFMono-Regular,Consolas,monospace;font-size:12px}.toast{position:fixed;right:20px;bottom:20px;max-width:360px;background:var(--surface3);border:1px solid var(--line);padding:12px 14px;border-radius:10px;box-shadow:var(--shadow);z-index:10}.toast.good{border-color:#2b705b}.toast.error{border-color:#8f3b47;color:#ffd5d9}@media(max-width:1050px){.shell{grid-template-columns:210px 1fr}.grid{grid-template-columns:repeat(2,minmax(0,1fr))}.catalog{grid-template-columns:repeat(2,minmax(0,1fr))}.rail-foot{width:180px}}@media(max-width:760px){.shell{grid-template-columns:1fr}.rail{border-right:0;border-bottom:1px solid var(--line);padding:13px 12px}.brand{margin:2px 5px 13px}.nav{display:flex;overflow:auto}.nav button{white-space:nowrap}.rail-foot{display:none}.content{padding:22px 14px}.two,.three{grid-template-columns:1fr}.top{display:block}.pill{display:inline-block;margin-top:13px}.provider{grid-template-columns:1fr}.provider-actions{justify-content:flex-start}.catalog{grid-template-columns:1fr}}@media(max-width:480px){.grid{grid-template-columns:1fr}.top h1{font-size:26px}}
.two{display:grid;grid-template-columns:1.35fr .65fr;gap:12px}.three{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:12px}.rows{display:grid;gap:8px}@media(max-width:760px){body{overflow-x:hidden}.shell,.rail,main,.content,.nav{min-width:0}.rail,.nav{width:100%}.two,.three{grid-template-columns:1fr}}
</style><style>.chat-history{display:grid;gap:10px;max-height:480px;overflow:auto;padding:4px 2px}.chat-message{display:grid;gap:5px;max-width:86%;padding:11px 13px;border:1px solid var(--line);border-radius:12px;line-height:1.6;white-space:pre-wrap}.chat-message.user{justify-self:end;background:#12302b;border-color:#2d6b5d}.chat-message.assistant{justify-self:start;background:#0d1417}.chat-message.system{justify-self:center;color:var(--muted);font-size:12px}.chat-role{font-size:10px;letter-spacing:.1em;text-transform:uppercase;color:var(--accent);font-weight:800}.chat-empty{color:var(--muted);padding:24px 8px;text-align:center}</style></head>
<body><div class="shell"><aside class="rail"><div class="brand"><div class="mark">S</div><div><strong>SAPIENS</strong><small>AGENT CONTROL</small></div></div><nav class="nav" aria-label="Navegação principal">
<button class="active" data-tab="overview"><span class="icon">◈</span>Visão geral</button><button data-tab="chat"><span class="icon">✦</span>Chat</button><button data-tab="providersPanel"><span class="icon">◌</span>Providers e modelos</button><button data-tab="channelsPanel"><span class="icon">⌁</span>Canais</button><button data-tab="skillsPanel"><span class="icon">◇</span>Skills</button><button data-tab="memoryPanel"><span class="icon">▣</span>Memória e sessões</button><button data-tab="automationPanel"><span class="icon">◷</span>Automações</button><button data-tab="toolsPanel"><span class="icon">⚙</span>Ferramentas</button><button data-tab="securityPanel"><span class="icon">◆</span>Segurança</button><button data-tab="diagnosticsPanel"><span class="icon">≋</span>Logs e diagnóstico</button><button data-tab="configure"><span class="icon">☷</span>Configuração</button>
</nav><div class="rail-foot">Local-first · supervisionado<br>O navegador é opcional. O PowerShell continua sendo o caminho principal.</div></aside>
<main class="content"><header class="top"><div><div class="eyebrow">Sapiens Agent · local-first</div><h1 id="title">Visão geral</h1><div class="muted" id="subtitle">Controle seu agente sem sair do computador.</div></div><div class="pill" id="health">● verificando gateway</div></header>
<section id="overview" class="tab"><div class="grid"><div class="card"><div class="label">Gateway</div><div class="metric" id="gateway">—</div><div class="muted small">processo local</div></div><div class="card"><div class="label">Provider ativo</div><div class="metric" id="activeProvider">—</div><div class="muted small" id="activeModel">modelo não carregado</div></div><div class="card"><div class="label">Canais</div><div class="metric" id="channelCount">—</div><div class="muted small">conectados</div></div><div class="card"><div class="label">Skills</div><div class="metric" id="skillCount">—</div><div class="muted small">válidas</div></div></div><div class="two section"><div class="card"><h2>Capacidades</h2><p class="muted small">Estado real carregado do arquivo de configuração.</p><div class="rows" id="capabilities"></div></div><div class="card"><h2>Recursos</h2><p class="muted small">Limites do perfil atual.</p><div class="rows" id="resources"></div></div></div><div class="card section"><h2>Comece por aqui</h2><div class="three"><div class="notice"><strong>Converse</strong><br><span class="small">Abra o Chat e envie uma mensagem para o provider ativo.</span></div><div class="notice"><strong>Configure</strong><br><span class="small">Use Providers e modelos para testar ou trocar o Ollama.</span></div><div class="notice"><strong>Supervisione</strong><br><span class="small">Permissões e ações externas permanecem sob aprovação.</span></div></div></div></section>
<section id="chat" class="tab hide"><div class="card chat-box"><div class="chat-head"><div><h2>Chat local</h2><p class="muted small">Converse pelo gateway e acompanhe toda a sessão nesta tela.</p></div><div class="chat-selectors"><label class="small muted" for="chatProviderSelect">Provider e modelo</label><select id="chatProviderSelect" aria-label="Provider e modelo"><option value="">Carregando...</option></select><label class="small muted" for="sessionSelect">Sessão</label><select id="sessionSelect" aria-label="Sessão"><option value="webchat:local">Sessão local</option></select></div></div><div id="chatHistory" class="chat-history" aria-live="polite"><div class="chat-empty">Pronto para conversar. Envie a primeira mensagem.</div></div><textarea id="message" aria-label="Mensagem" placeholder="Escreva uma tarefa ou pergunta... (Enter envia; Shift+Enter quebra linha)"></textarea><div class="actions"><button class="primary" id="send" onclick="sendChat()">Enviar mensagem</button><button class="secondary" onclick="newSession()">Nova conversa</button><button class="secondary" onclick="clearConversation()">Limpar conversa</button></div><div class="muted small" id="chatMeta">Sessão local · aguardando mensagem</div></div></section>
<section id="providersPanel" class="tab hide"><div class="card"><h2>Providers e modelos</h2><p class="muted small">A configuração abaixo é a mesma usada pelo PowerShell. Nenhuma chave é exibida.</p><div id="providerList" class="rows"></div></div></section>
<section id="channelsPanel" class="tab hide"><div class="card"><h2>Canais</h2><p class="muted small">Catálogo de integrações com estado honesto: disponível não significa configurado.</p><div id="channelList" class="catalog"></div><div class="card section"><h2>Áudio e mídia</h2><p class="muted small">Áudio fica desligado por padrão e só aparece como pronto quando houver canal e provider compatíveis.</p><div id="audioState" class="result"></div></div></div></section>
<section id="skillsPanel" class="tab hide"><div class="card"><h2>Skills carregadas</h2><p class="muted small">Skills reais possuem documentação, validação e escopo. Repetições podem virar candidatas revisáveis.</p><div id="skillList" class="rows"></div></div></section>
<section id="memoryPanel" class="tab hide"><div class="two"><div class="card"><h2>Memória</h2><p class="muted small">Estado persistente do agente.</p><div id="memoryState" class="rows"></div></div><div class="card"><h2>Sessões recentes</h2><div id="sessionList" class="rows"></div></div></div></section>
<section id="automationPanel" class="tab hide"><div class="card"><h2>Automações</h2><p class="muted small">Tarefas agendadas ficam sob os limites configurados e podem exigir aprovação.</p><div id="scheduleList" class="rows"></div></div></section>
<section id="toolsPanel" class="tab hide"><div class="card"><h2>Ferramentas e capacidades</h2><p class="muted small">O painel mostra o que está pronto, opcional ou desligado; ele não promete um adaptador inexistente.</p><div id="toolList" class="rows"></div></div></section>
<section id="securityPanel" class="tab hide"><div class="two"><div class="card"><h2>Segurança</h2><div id="securityState" class="rows"></div></div><div class="card"><h2>Princípios ativos</h2><div class="notice">Modo supervisionado, gateway local, limites de requisições e credenciais fora da interface. Ações destrutivas e escritas externas devem passar por aprovação.</div></div></div></section>
<section id="diagnosticsPanel" class="tab hide"><div class="card"><div class="chat-head"><div><h2>Logs e diagnóstico</h2><p class="muted small">Receipts recentes com segredos redigidos.</p></div><button class="secondary" onclick="refresh()">Atualizar</button></div><div id="receiptList" class="rows"></div></div></section>
<section id="configure" class="tab hide"><div class="card"><h2>Configuração rápida</h2><p class="muted small">As alterações usam a mesma configuração do PowerShell, são validadas antes de salvar e podem exigir reinício do gateway.</p><div class="two"><label class="field"><span>Interface preferida</span><select id="interfaceMode"><option>powershell</option><option>web</option><option>both</option></select></label><label class="field"><span>Perfil de recursos</span><select id="resourceProfile"><option>economy</option><option>balanced</option><option>performance</option><option>custom</option></select></label></div><label class="field section"><span><input type="checkbox" id="audioEnabled"> habilitar áudio opcional nos canais</span></label><div class="actions"><button class="primary" onclick="saveConfig()">Salvar configuração</button><button class="secondary" onclick="refresh()">Recarregar</button></div><div class="result" id="configResult" aria-live="polite"></div></div></section>
</main></div><div id="toast" class="toast hide" role="status"></div>
<script>
const $=id=>document.getElementById(id);let config={},snapshot={},chatHistory=[];
const channelCatalog=[['Telegram','bot','Pronto para configuração'],['Discord','bot','Pronto para configuração'],['Slack','bot','Pronto para configuração'],['WhatsApp','qr','Requer pareamento'],['Signal','secure','Requer signal-cli'],['Matrix','matrix','Requer homeserver'],['Microsoft Teams','bot','Requer app'],['Google Chat','bot','Requer app'],['WebChat','web','Incluído no gateway local'],['Email','mail','Adaptador opcional'],['SMS','sms','Adaptador opcional'],['Voice Call','voice','Plugin opcional']];
document.querySelectorAll('[data-tab]').forEach(b=>b.onclick=()=>showTab(b.dataset.tab,b.textContent));
function showTab(id,label){document.querySelectorAll('.tab').forEach(x=>x.classList.add('hide'));$(id).classList.remove('hide');document.querySelectorAll('.nav button').forEach(x=>x.classList.remove('active'));const active=[...document.querySelectorAll('.nav button')].find(x=>x.dataset.tab===id);if(active)active.classList.add('active');$('title').textContent=label.replace(/^[^A-Za-zÀ-ÿ0-9]*/,'').trim();if(id==='diagnosticsPanel')loadReceipts()}
async function api(url,options){const r=await fetch(url,options);let data={};try{data=await r.json()}catch(_){data={error:'resposta inválida do gateway'}}if(!r.ok)throw Error(data.error||'falha de API');return data}
api=async function(url,options){const r=await fetch(url,options);const raw=await r.text();let data={};try{data=raw?JSON.parse(raw):{}}catch(_){data={error:raw||'resposta inválida do gateway'}}if(!r.ok)throw Error(data.error||raw||'falha de API');return data}
function esc(value){return String(value??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]))}
function row(name,value,good=true){return '<div class="row"><span>'+esc(name)+'</span><span class="tag '+(good?'on':'off')+'">'+esc(value)+'</span></div>'}
function toast(message,error=false){const t=$('toast');t.textContent=message;t.className='toast '+(error?'error':'good');clearTimeout(window.toastTimer);window.toastTimer=setTimeout(()=>t.className='toast hide',3600)}
function providerId(alias){return 'model-'+String(alias).replace(/[^a-z0-9_-]/gi,'_')}
function activeProvider(){return (snapshot.providers||[]).find(p=>p.alias===config.active_provider)}
async function refresh(){try{const [s,c,k,p,ss]=await Promise.all([api('/v1/status'),api('/v1/config'),api('/v1/skills'),api('/v1/providers'),api('/v1/sessions')]);config=c;snapshot={status:s,providers:p.providers||[],skills:k,sessions:ss};const active=activeProvider();$('health').textContent='● gateway saudável';$('health').className='pill';$('gateway').textContent=s.gateway;$('activeProvider').textContent=active?.alias||'nenhum';$('activeModel').textContent=active?.model||'modelo não configurado';$('channelCount').textContent=s.channels.filter(x=>x.enabled).length;$('skillCount').textContent=k.filter(x=>x.valid).length;$('capabilities').innerHTML=Object.entries(s.features).map(([n,v])=>row(n,v?'pronto':'desligado',v)).join('');$('resources').innerHTML=[row('perfil',s.resources.profile),row('GPU máxima',s.resources.max_gpu_percent+'%'),row('memória',s.resources.max_memory_mb+' MB'),row('CPU máxima',s.resources.max_cpu_percent+'%')].join('');$('interfaceMode').value=c.interface.mode;$('resourceProfile').value=c.resources.profile;$('audioEnabled').checked=c.audio.enabled;renderProviders();renderChannels();renderSkills();renderMemory();renderAutomation();renderTools();renderSecurity();renderSessions()}catch(e){$('health').textContent='● gateway indisponível';$('health').className='pill bad';$('configResult').textContent=e.message;toast(e.message,true)}}
function renderProviders(){const list=snapshot.providers||[];const active=config.active_provider||'';const reserves=Array.isArray(config.fallback)?config.fallback:[];const routing='<div class="card" style="margin-bottom:16px"><h3>Roteamento das conversas</h3><p class="muted small">Escolha o provider principal. As reservas só entram quando o principal falhar ou atingir limite.</p><div class="two"><label class="field"><span>Provider principal</span><select id="activeProviderSelect" aria-label="Provider principal">'+(list.length?list.map(p=>'<option value="'+esc(p.alias)+'" '+(p.alias===active?'selected':'')+'>'+esc(p.alias)+' · '+esc(p.model||'modelo não definido')+'</option>').join(''):'<option value="">Nenhum provider configurado</option>')+'</select></label><div class="field"><span>Providers reserva, na ordem</span><div id="fallbackOptions" class="rows">'+(list.filter(p=>p.alias!==active).length?list.filter(p=>p.alias!==active).map(p=>'<label class="small"><input type="checkbox" value="'+esc(p.alias)+'" '+(reserves.includes(p.alias)?'checked':'')+'> '+esc(p.alias)+' · '+esc(p.model||'modelo não definido')+'</label>').join(''):'<span class="muted small">Adicione outro provider para habilitar reservas.</span>')+'</div></div></div><div class="actions"><button class="primary" onclick="saveRouting()" '+(list.length?'':'disabled')+'>Salvar roteamento</button><span class="muted small" id="routingResult"></span></div></div>';const providers=list.length?list.map(p=>{const isActive=p.alias===active;const isReserve=reserves.includes(p.alias);const id=providerId(p.alias);return '<div class="provider"><div><h3>'+esc(p.alias)+' '+(isActive?'<span class="tag on">principal</span>':isReserve?'<span class="tag">reserva</span>':'')+'</h3><div class="provider-meta"><span class="tag">'+esc(p.kind)+'</span><span class="tag">protocolo: '+esc(p.protocol)+'</span><span class="tag '+(p.local?'on':'off')+'">'+(p.local?'local':'remoto')+'</span></div><div class="muted small mono">'+esc(p.base_url)+' · '+esc(p.model)+'</div></div><div class="provider-actions"><input id="'+id+'" value="'+esc(p.model)+'" aria-label="Modelo de '+esc(p.alias)+'"><button class="secondary" onclick="testProvider('+JSON.stringify(p.alias)+',this)">Testar</button><button class="secondary" onclick="saveModel('+JSON.stringify(p.alias)+')">Salvar modelo</button>'+(isActive?'':'<button class="primary" onclick="activateProvider('+JSON.stringify(p.alias)+')">Ativar</button>')+'<div class="small" id="test-'+id+'"></div></div></div>'}).join(''):'<div class="empty">Nenhum provider configurado. Use o menu do PowerShell para adicionar um.</div>';$('providerList').innerHTML=routing+providers}
function renderChannels(){const configured=snapshot.status?.channels||[];$('channelList').innerHTML=channelCatalog.map(([name,kind,description])=>{const item=configured.find(c=>c.kind===kind||c.name.toLowerCase()===name.toLowerCase());const state=item?.enabled?'habilitado':item?'opcional':'não configurado';return '<div class="channel"><strong>'+esc(name)+'</strong><span class="tag '+(item?.enabled?'on':'off')+'">'+state+'</span><div class="muted small">'+esc(description)+'</div></div>'}).join('');const a=snapshot.status?.audio||{};$('audioState').textContent=a.enabled?'Áudio habilitado na configuração. O canal ainda precisa declarar suporte.':'Áudio desligado por padrão; habilite em Configuração quando houver um canal compatível.'}
function renderSkills(){const k=snapshot.skills||[];$('skillList').innerHTML=k.length?k.map(x=>'<div class="skill"><span><strong>'+esc(x.name)+'</strong><br><span class="muted small">'+esc(x.description||'sem descrição')+'</span></span><span class="tag '+(x.valid?'on':'off')+'">'+(x.valid?'válida':'revisar')+'</span></div>').join(''):'<div class="empty">Nenhuma skill encontrada.</div>'}
function renderMemory(){const c=snapshot.status||{};$('memoryState').innerHTML=[row('memória',c.features?.memory?'habilitada':'desligada',c.features?.memory),row('backend',config.memory_backend||'jsonl'),row('retenção',String(config.memory_retention_days||0)+' dias'),row('sessões',String((snapshot.sessions||[]).length))].join('')}
function renderSessions(){const sessions=snapshot.sessions||[];const current=$('sessionSelect').value;$('sessionSelect').innerHTML=(sessions.length?sessions:[{id:'webchat:local'}]).map(s=>'<option value="'+esc(s.id)+'">'+esc(s.id)+(s.cancelled?' · cancelada':'')+'</option>').join('');if([...$('sessionSelect').options].some(o=>o.value===current))$('sessionSelect').value=current;$('sessionList').innerHTML=sessions.length?sessions.map(s=>row(s.id,s.cancelled?'cancelada':'ativa',!s.cancelled)).join(''):'<div class="empty">Nenhuma sessão criada ainda.</div>'}
function renderAutomation(){const schedules=config.schedules||[];$('scheduleList').innerHTML=schedules.length?schedules.map(s=>'<div class="row"><span><strong>'+esc(s.id||'tarefa')+'</strong><br><span class="muted small">'+esc(s.task||'sem descrição')+'</span></span><span class="tag '+(s.enabled?'on':'off')+'">'+(s.enabled?'ativa':'pausada')+'</span></div>').join(''):'<div class="empty">Nenhuma automação configurada. O scheduler está disponível pelo PowerShell.</div>'}
function renderTools(){const f=snapshot.status?.features||{};const names=[['browser','Navegador'],['computer_use','Controle do computador'],['shell','Shell'],['mcp','MCP'],['channels','Canais'],['memory','Memória'],['scheduler','Agendador'],['audio','Áudio']];$('toolList').innerHTML=names.map(([key,label])=>row(label,f[key]?'pronto':'desligado',!!f[key])).join('')}
function renderSecurity(){const s=snapshot.status||{};$('securityState').innerHTML=[row('modo',config.security?.mode||'supervised',true),row('gateway',config.server?.bind||'local',true),row('rede privada',config.security?.allow_private_networks?'permitida':'bloqueada',!config.security?.allow_private_networks),row('requisições/minuto',config.security?.max_requests_per_minute||'—',true),row('workspace',config.security?.workspace||'—',true)].join('')}
async function loadReceipts(){try{const data=await api('/v1/receipts');$('receiptList').innerHTML=data.length?data.map(x=>'<div class="result mono">'+esc(JSON.stringify(x))+'</div>').join(''):'<div class="empty">Ainda não há receipts.</div>'}catch(e){$('receiptList').innerHTML='<div class="result">'+esc(e.message)+'</div>'}}
async function saveConfig(){const out=$('configResult');try{for(const [key,value] of [['interface.mode',$('interfaceMode').value],['resources.profile',$('resourceProfile').value],['features.audio',$('audioEnabled').checked?'true':'false']])await api('/v1/config',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({key,value})});out.textContent='Configuração salva com validação. Se o gateway avisar reinício, reinicie pelo menu do PowerShell.';toast('Configuração salva');await refresh()}catch(e){out.textContent=e.message;toast(e.message,true)}}
async function testProvider(alias,button){const target=$('test-'+providerId(alias));button.disabled=true;target.textContent='testando...';try{const d=await api('/v1/providers/test',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({alias})});target.textContent='ok · '+d.latency_ms+' ms';target.className='small on';toast(alias+' respondeu corretamente')}catch(e){target.textContent=e.message;target.className='small bad';toast(e.message,true)}finally{button.disabled=false}}
async function activateProvider(alias){try{await api('/v1/config',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({key:'active_provider',value:alias})});toast(alias+' agora está ativo');await refresh()}catch(e){toast(e.message,true)}}
async function saveRouting(){const active=$('activeProviderSelect')?.value||'';const fallback=[...document.querySelectorAll('#fallbackOptions input[type=checkbox]:checked')].map(x=>x.value).filter(x=>x!==active);const result=$('routingResult');if(!active){result.textContent='Escolha um provider principal.';return}try{await api('/v1/config',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({key:'active_provider',value:active})});await api('/v1/config',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({key:'fallback',value:fallback.join(',')})});result.textContent='Roteamento salvo.';toast('Provider principal e reservas atualizados');await refresh()}catch(e){result.textContent=e.message;toast(e.message,true)}}
async function saveModel(alias){try{const value=$(providerId(alias)).value.trim();if(!value)throw Error('informe um modelo');await api('/v1/config',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({key:'provider.'+alias+'.model',value})});toast('Modelo salvo; o próximo chat usará a configuração atualizada');await refresh()}catch(e){toast(e.message,true)}}
function renderChat(){const target=$('chatHistory');if(!chatHistory.length){target.innerHTML='<div class="chat-empty">Pronto para conversar. Envie a primeira mensagem.</div>';return}target.innerHTML=chatHistory.map(item=>'<div class="chat-message '+esc(item.role)+'"><span class="chat-role">'+(item.role==='user'?'Você':item.role==='assistant'?'Sapiens':'Sistema')+'</span><span>'+esc(item.content)+'</span></div>').join('');target.scrollTop=target.scrollHeight}
function clearConversation(){chatHistory=[];renderChat();$('chatMeta').textContent='Conversa limpa · pronta para nova mensagem';$('message').focus()}
function newSession(){chatHistory=[];$('sessionSelect').value='webchat:local:'+Date.now();renderChat();$('chatMeta').textContent='Nova conversa · pronta para mensagem';$('message').focus()}
async function sendChat(){const text=$('message').value.trim();if(!text){$('chatMeta').textContent='Digite uma mensagem antes de enviar.';return}const b=$('send');b.disabled=true;chatHistory.push({role:'user',content:text});renderChat();$('message').value='';$('chatMeta').textContent='Processando no provider local...';try{const d=await api('/v1/chat',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({message:text,history:chatHistory.slice(0,-1),session:$('sessionSelect').value,channel:'webchat',identity:'local'})});chatHistory.push({role:'assistant',content:d.answer||JSON.stringify(d,null,2)});renderChat();$('chatMeta').textContent=(d.provider||'provider')+' · '+(d.latency_ms||0)+' ms';await refresh()}catch(e){chatHistory.push({role:'system',content:e.message});renderChat();$('chatMeta').textContent='Falha no provider';toast(e.message,true)}finally{b.disabled=false;$('message').focus()}}
$('message').addEventListener('keydown',e=>{if(e.key==='Enter'&&!e.shiftKey){e.preventDefault();sendChat()}});refresh();
</script><script>
const sapiensOriginalRefresh=refresh;
const sapiensOriginalRenderProviders=renderProviders;
let chatCooldownUntil=0;
let chatCooldownTimer=null;
const chatStyle=document.createElement('style');
chatStyle.textContent='.chat-selectors{display:grid;gap:6px;min-width:250px;align-items:start}.chat-selectors select{max-width:300px}.field.section span{display:flex;align-items:center;gap:8px}.field.section input[type=checkbox]{width:auto;flex:0 0 auto}@media(max-width:760px){.chat-head{align-items:stretch;flex-direction:column}.chat-selectors{min-width:0}.chat-selectors select{max-width:none}}';
document.head.appendChild(chatStyle);
renderProviders=function(){
  sapiensOriginalRenderProviders();
  document.querySelectorAll('.provider').forEach(card=>{
    const heading=card.querySelector('h3');
    const alias=(heading?.textContent||'').replace(/\s+(principal|reserva)\s*$/,'').trim();
    card.querySelectorAll('.provider-actions button').forEach(button=>{
      button.removeAttribute('onclick');
      const action=button.textContent.trim();
      if(action==='Testar')button.onclick=()=>testProvider(alias,button);
      else if(action==='Salvar modelo')button.onclick=()=>saveModel(alias);
      else if(action==='Ativar')button.onclick=()=>activateProvider(alias);
    });
  });
};
function renderChatProviderSelect(){
  const select=$('chatProviderSelect');
  if(!select)return;
  const list=snapshot.providers||[];
  const canonicalAlias=config.active_provider||'';
  const canonical=list.find(p=>p.alias===canonicalAlias);
  const canonicalSignature=canonical?[canonical.alias,canonical.model||'',canonical.protocol||'',canonical.base_url||''].join('|'):'';
  const previousCanonicalSignature=sessionStorage.getItem('sapiens.chatProviderCanonical')||'';
  const canonicalChanged=Boolean(previousCanonicalSignature&&canonicalSignature&&previousCanonicalSignature!==canonicalSignature);
  const stored=sessionStorage.getItem('sapiens.chatProvider')||'';
  const storedSignature=sessionStorage.getItem('sapiens.chatProviderSignature')||'';
  const storedProvider=list.find(p=>p.alias===stored);
  const storedCurrentSignature=storedProvider?[storedProvider.alias,storedProvider.model||'',storedProvider.protocol||'',storedProvider.base_url||''].join('|'):'';
  const storedIsCurrent=Boolean(!canonicalChanged&&stored&&storedSignature&&storedSignature===storedCurrentSignature);
  const selected=storedIsCurrent?stored:canonicalAlias;
  if((canonicalChanged||stored&&!storedIsCurrent)&&stored){sessionStorage.removeItem('sapiens.chatProvider');sessionStorage.removeItem('sapiens.chatProviderSignature')}
  select.innerHTML=list.length?list.map(p=>'<option value="'+esc(p.alias)+'" '+(p.alias===selected?'selected':'')+'>'+esc(p.alias)+' · '+esc(p.model||'modelo não configurado')+'</option>').join(''):'<option value="">Nenhum provider configurado</option>';
  if(selected)select.value=selected;
  if(canonicalSignature)sessionStorage.setItem('sapiens.chatProviderCanonical',canonicalSignature);
}
function selectedChatProvider(){return $('chatProviderSelect')?.value||config.active_provider||null}
function formatChatProvider(alias){const p=(snapshot.providers||[]).find(item=>item.alias===alias);return (alias||'provider')+(p?.model?' · '+p.model:'')}
function setChatCooldown(seconds){
  chatCooldownUntil=Date.now()+Math.max(1,seconds)*1000;
  clearInterval(chatCooldownTimer);
  const update=()=>{
    const remaining=Math.ceil((chatCooldownUntil-Date.now())/1000);
    const button=$('send');
    if(remaining<=0){clearInterval(chatCooldownTimer);chatCooldownTimer=null;if(button)button.disabled=false;return}
    if(button)button.disabled=true;
    $('chatMeta').textContent='Limite temporário atingido · tente novamente em '+remaining+'s';
  };
  update();chatCooldownTimer=setInterval(update,1000);
}
api=async function(url,options){
  const response=await fetch(url,options);
  const raw=await response.text();
  let data={};try{data=raw?JSON.parse(raw):{}}catch(_){data={error:raw||'resposta inválida do gateway'}}
  if(!response.ok){const error=new Error(data.error||raw||'falha de API');error.status=response.status;throw error}
  return data;
};
refresh=async function(){await sapiensOriginalRefresh();renderChatProviderSelect()};
const chatProviderSelect=$('chatProviderSelect');
if(chatProviderSelect)chatProviderSelect.addEventListener('change',()=>{
  const provider=(snapshot.providers||[]).find(item=>item.alias===chatProviderSelect.value);
  const signature=provider?[provider.alias,provider.model||'',provider.protocol||'',provider.base_url||''].join('|'):'';
  sessionStorage.setItem('sapiens.chatProvider',chatProviderSelect.value);
  sessionStorage.setItem('sapiens.chatProviderSignature',signature);
  $('chatMeta').textContent='Pronto · '+formatChatProvider(chatProviderSelect.value)
});
window.sendChat=async function(){
  const text=$('message').value.trim();
  if(!text){$('chatMeta').textContent='Digite uma mensagem antes de enviar.';return}
  const remaining=Math.ceil((chatCooldownUntil-Date.now())/1000);
  if(remaining>0){setChatCooldown(remaining);return}
  const provider=selectedChatProvider();
  const button=$('send');button.disabled=true;
  const last=chatHistory[chatHistory.length-1];
  const previous=chatHistory[chatHistory.length-2];
  if(last?.role==='system'&&last.retryText===text&&previous?.role==='user'&&previous.content===text){chatHistory.splice(-2,2)}
  chatHistory.push({role:'user',content:text});renderChat();
  try{
    const data=await api('/v1/chat',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({message:text,history:chatHistory.slice(0,-1),provider,session:$('sessionSelect').value,channel:'webchat',identity:'local'})});
    $('message').value='';
    chatHistory.push({role:'assistant',content:data.answer||JSON.stringify(data,null,2)});renderChat();
    $('chatMeta').textContent=formatChatProvider(data.provider)+' · '+(data.latency_ms||0)+' ms';
    await refresh();
  }catch(error){
    const is429=error.status===429||/HTTP 429|Too Many Requests/i.test(error.message);
    const friendly=is429?'O limite gratuito do OpenRouter foi atingido. Aguarde alguns segundos ou escolha outro provider/modelo.':error.message;
    chatHistory.push({role:'system',content:friendly,retryText:text});renderChat();$('message').value=text;
    $('chatMeta').textContent=is429?'Provider limitado · sua mensagem foi preservada':'Falha no provider';toast(friendly,true);
    if(is429){const match=error.message.match(/(\d+)\s*segundos?/i);setChatCooldown(match?Number(match[1]):10)}
  }finally{if(!chatCooldownUntil||chatCooldownUntil<=Date.now())button.disabled=false;$('message').focus()}
};
refresh();
</script></body></html>"###,
    )
}

async fn api_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, String)> {
    authorize(&state, &headers)?;
    let config = crate::config::load(&state.config_path).map_err(internal_error)?;
    let skills =
        crate::skills::discover(&state.config_path, &config.skills).map_err(internal_error)?;
    let active_model = config
        .active_provider
        .as_deref()
        .and_then(|alias| {
            config
                .providers
                .iter()
                .find(|provider| provider.alias == alias)
        })
        .map(|provider| provider.model.clone());
    Ok(Json(serde_json::json!({
        "gateway": "online",
        "active_provider": config.active_provider,
        "active_model": active_model,
        "features": config.features,
        "resources": config.resources,
        "audio": config.audio,
        "providers": config.providers.iter().map(|p| serde_json::json!({"alias": p.alias, "kind": p.kind, "protocol": p.protocol, "capabilities": p.capabilities})).collect::<Vec<_>>(),
        "channels": config.channels.iter().map(|c| serde_json::json!({"name": c.name, "kind": c.kind, "enabled": c.enabled})).collect::<Vec<_>>(),
        "skills_valid": skills.iter().filter(|skill| skill.valid).count(),
    })))
}

async fn api_providers(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, String)> {
    authorize(&state, &headers)?;
    let config = crate::config::load(&state.config_path).map_err(internal_error)?;
    let active_model = config
        .active_provider
        .as_deref()
        .and_then(|alias| {
            config
                .providers
                .iter()
                .find(|provider| provider.alias == alias)
        })
        .map(|provider| provider.model.clone());
    Ok(Json(serde_json::json!({
        "active": config.active_provider,
        "active_model": active_model,
        "providers": config.providers.iter().map(|provider| serde_json::json!({
            "alias": provider.alias,
            "kind": provider.kind,
            "protocol": provider.protocol,
            "base_url": provider.base_url,
            "model": provider.model,
            "capabilities": provider.capabilities,
            "streaming": provider.streaming,
            "timeout_secs": provider.timeout_secs,
            "retries": provider.retries,
            "local": provider.protocol == "ollama" || provider.base_url.starts_with("http://127.0.0.1") || provider.base_url.starts_with("http://localhost"),
        })).collect::<Vec<_>>(),
    })))
}

#[derive(Debug, Deserialize)]
struct ProviderTestRequest {
    alias: String,
}

async fn api_provider_test(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ProviderTestRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    authorize(&state, &headers)?;
    let config = crate::config::load(&state.config_path).map_err(internal_error)?;
    let registry = ProviderRegistry::new(config);
    let health = registry.health(&request.alias).await.map_err(|error| {
        (
            StatusCode::BAD_GATEWAY,
            format!("provider test failed: {error}"),
        )
    })?;
    let probe = registry
        .chat_detailed(
            Some(&request.alias),
            "provider_test",
            "Responda apenas: PROVIDER_OK",
        )
        .await
        .map_err(|error| {
            (
                StatusCode::BAD_GATEWAY,
                format!("provider generation test failed: {error}"),
            )
        })?;
    Ok(Json(serde_json::json!({
        "alias": health.alias,
        "protocol": health.protocol,
        "ok": health.ok,
        "status": health.status,
        "latency_ms": probe.latency_ms,
        "generation": true,
        "probe": probe.answer,
    })))
}

async fn api_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, String)> {
    authorize(&state, &headers)?;
    let sessions = state.sessions.lock().await.list();
    Ok(Json(
        serde_json::to_value(sessions).map_err(|error| internal_error(error.into()))?,
    ))
}

async fn api_receipts(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, String)> {
    authorize(&state, &headers)?;
    let path = crate::observability::receipts_path(&state.config_path);
    if !path.exists() {
        return Ok(Json(serde_json::json!([])));
    }
    let values = fs::read_to_string(&path)
        .map_err(|error| internal_error(error.into()))?
        .lines()
        .rev()
        .take(80)
        .filter_map(|line| serde_json::from_str::<Value>(&crate::observability::redact(line)).ok())
        .collect::<Vec<_>>();
    Ok(Json(serde_json::json!(
        values.into_iter().rev().collect::<Vec<_>>()
    )))
}

async fn api_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, String)> {
    authorize(&state, &headers)?;
    let config = crate::config::load(&state.config_path).map_err(internal_error)?;
    Ok(Json(
        serde_json::to_value(config).map_err(|error| internal_error(error.into()))?,
    ))
}

#[derive(Debug, Deserialize)]
struct ConfigUpdate {
    key: String,
    value: String,
}

async fn api_config_update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(update): Json<ConfigUpdate>,
) -> Result<Json<Value>, (StatusCode, String)> {
    authorize(&state, &headers)?;
    let mut config = crate::config::load(&state.config_path).map_err(internal_error)?;
    crate::config::set_value(&mut config, &update.key, &update.value).map_err(internal_error)?;
    crate::config::save(&state.config_path, &config).map_err(internal_error)?;
    let _ = crate::observability::append_receipt(
        &state.config_path,
        "config.web_update",
        "external_write",
        true,
        &format!("key={}", update.key),
        None,
    );
    let active_model = config
        .active_provider
        .as_deref()
        .and_then(|alias| {
            config
                .providers
                .iter()
                .find(|provider| provider.alias == alias)
        })
        .map(|provider| provider.model.clone());
    Ok(Json(serde_json::json!({
        "saved": true,
        "key": update.key,
        "active_provider": config.active_provider,
        "active_model": active_model,
    })))
}

async fn api_skills(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, String)> {
    authorize(&state, &headers)?;
    let config = crate::config::load(&state.config_path).map_err(internal_error)?;
    let skills =
        crate::skills::discover(&state.config_path, &config.skills).map_err(internal_error)?;
    Ok(Json(
        serde_json::to_value(skills).map_err(|error| internal_error(error.into()))?,
    ))
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
    let model_message = conversation_prompt(&req.history, &model_message);
    // Reload the canonical config for every chat request. This keeps the
    // gateway and WebUI synchronized after a PowerShell or WebUI update and
    // prevents stale provider protocol/model pairs from remaining in memory.
    let runtime_config = crate::config::load(&state.config_path).map_err(internal_error)?;
    let registry = ProviderRegistry::new(runtime_config);
    let outcome = registry
        .chat_detailed_with_context_and_images(
            req.provider.as_deref(),
            task,
            &model_message,
            state.identity_context.as_deref(),
            &req.images,
        )
        .await
        .map_err(|e| {
            let error = e.to_string();
            let status = if error.contains("HTTP 429") {
                StatusCode::TOO_MANY_REQUESTS
            } else {
                StatusCode::BAD_GATEWAY
            };
            (status, error)
        })?;
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

fn conversation_prompt(history: &[ChatTurn], current: &str) -> String {
    if history.is_empty() {
        return current.to_string();
    }
    let mut prompt = String::from(
        "Contexto recente da conversa. Use-o para manter continuidade, mas responda somente à mensagem atual:\n",
    );
    for turn in history.iter().rev().take(12).rev() {
        let role = match turn.role.as_str() {
            "assistant" => "Sapiens",
            "user" => "Usuário",
            _ => "Sistema",
        };
        let content: String = turn.content.chars().take(4_000).collect();
        prompt.push_str(role);
        prompt.push_str(": ");
        prompt.push_str(&content);
        prompt.push('\n');
    }
    prompt.push_str("\nMensagem atual do usuário:\n");
    prompt.push_str(current);
    prompt
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
            history: Vec::new(),
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
        if !state.audio_enabled && inbound.kind == "audio" {
            let _ = crate::observability::append_receipt(
                &state.config_path,
                "channel.whatsapp.audio",
                "read",
                false,
                &format!("id={} audio=disabled", inbound.id),
                Some(&inbound.sender),
            );
            responses.push(serde_json::json!({
                "id": inbound.id,
                "timestamp": inbound.timestamp,
                "event": "media",
                "status": "ignored",
                "reason": "audio disabled",
            }));
            continue;
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
            history: Vec::new(),
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
                memory: Arc::new(Mutex::new(
                    MemoryStore::open_with_retention(root.join("memory.jsonl"), 30)
                        .expect("memory"),
                )),
                sessions: Arc::new(Mutex::new(
                    FileSessionStore::open(root.join("sessions.json")).expect("sessions"),
                )),
                memory_enabled: false,
                audio_enabled: false,
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
                memory: Arc::new(Mutex::new(
                    MemoryStore::open(root.join("memory.jsonl")).expect("memory"),
                )),
                sessions: Arc::new(Mutex::new(sessions)),
                memory_enabled: false,
                audio_enabled: false,
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
                history: Vec::new(),
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
                history: Vec::new(),
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
