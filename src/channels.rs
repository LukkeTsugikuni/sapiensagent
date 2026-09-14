use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::process::{Command as StdCommand, Stdio};
use tokio::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundEvent {
    pub channel: String,
    pub account: String,
    pub sender: String,
    pub conversation: String,
    pub text: String,
    pub attachments: Vec<String>,
    pub reply_target: serde_json::Value,
    pub requested_capabilities: Vec<String>,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub transport: &'static str,
    pub capabilities: &'static str,
    pub credential_hint: &'static str,
    pub adapter: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChannelConfig {
    pub name: String,
    pub kind: String,
    pub enabled: bool,
    pub credential_env: String,
    pub allowlist: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MatrixInboundMessage {
    pub event_id: String,
    pub room_id: String,
    pub sender: String,
    pub body: String,
    pub msgtype: String,
    pub media_uri: Option<String>,
    pub mime_type: Option<String>,
    pub media_size: Option<u64>,
    pub filename: Option<String>,
    pub origin_server_ts: Option<u64>,
    pub mentions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MatrixSyncBatch {
    pub next_batch: String,
    pub messages: Vec<MatrixInboundMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignalInboundMessage {
    pub event_id: String,
    pub sender: String,
    pub group_id: Option<String>,
    pub text: String,
    pub attachments: Vec<String>,
    pub attachment_details: Vec<SignalAttachment>,
    pub timestamp: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignalAttachment {
    pub id: String,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MatrixDownloadedMedia {
    pub media_uri: String,
    pub path: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignalDownloadedMedia {
    pub attachment_id: String,
    pub path: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WhatsAppDownloadedMedia {
    pub media_id: String,
    pub path: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub sha256: String,
}

pub trait Channel: Send + Sync {
    fn name(&self) -> &'static str;
    fn verify_sender(&self, sender: &str) -> bool;
}

pub async fn telegram_test(config: &ChannelConfig) -> Result<()> {
    let token = telegram_token(config)?;
    let response: serde_json::Value = Client::new()
        .get(format!("https://api.telegram.org/bot{token}/getMe"))
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .context("Telegram getMe request failed")?
        .error_for_status()
        .context("Telegram getMe returned an error")?
        .json()
        .await
        .context("Telegram getMe response was not JSON")?;
    if response.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        bail!(
            "Telegram health check failed: {}",
            crate::observability::redact(&response.to_string())
        );
    }
    Ok(())
}

pub async fn telegram_send(config: &ChannelConfig, recipient: &str, message: &str) -> Result<()> {
    if recipient.trim().is_empty() || message.trim().is_empty() {
        bail!("Telegram recipient and message are required");
    }
    if !telegram_recipient_allowed(config, recipient) {
        bail!("Telegram recipient is not allowlisted");
    }
    let token = telegram_token(config)?;
    let endpoint = format!("https://api.telegram.org/bot{token}/sendMessage");
    let payload = json!({"chat_id": recipient, "text": message});
    let client = Client::new();
    let mut last_error = String::from("Telegram sendMessage failed");
    for attempt in 0..3 {
        match client
            .post(&endpoint)
            .timeout(std::time::Duration::from_secs(30))
            .json(&payload)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                let body: serde_json::Value = response
                    .json()
                    .await
                    .context("Telegram sendMessage response was not JSON")?;
                if body.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
                    bail!(
                        "Telegram sendMessage failed: {}",
                        crate::observability::redact(&body.to_string())
                    );
                }
                return Ok(());
            }
            Ok(response) => {
                let status = response.status();
                last_error = format!("Telegram sendMessage returned HTTP {status}");
                if !is_retryable_status(status) {
                    break;
                }
            }
            Err(error) => last_error = crate::observability::redact(&error.to_string()),
        }
        if attempt < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(250 * (attempt + 1))).await;
        }
    }
    bail!(last_error)
}

pub async fn matrix_test(config: &ChannelConfig) -> Result<()> {
    matrix_user_id(config).await?;
    Ok(())
}

pub async fn matrix_user_id(config: &ChannelConfig) -> Result<String> {
    let token = matrix_token(config)?;
    let endpoint = matrix_endpoint("account/whoami")?;
    let response: serde_json::Value = Client::new()
        .get(endpoint)
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .context("Matrix whoami request failed")?
        .error_for_status()
        .context("Matrix whoami returned an error")?
        .json()
        .await
        .context("Matrix whoami response was not JSON")?;
    response
        .get("user_id")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .context("Matrix health check returned an incomplete response")
}

pub async fn matrix_send(config: &ChannelConfig, recipient: &str, message: &str) -> Result<()> {
    if recipient.trim().is_empty() || message.trim().is_empty() {
        bail!("Matrix room and message are required");
    }
    if !telegram_recipient_allowed(config, recipient) {
        bail!("Matrix room is not allowlisted");
    }
    let token = matrix_token(config)?;
    let endpoint = matrix_send_endpoint(recipient)?;
    let payload = json!({"msgtype": "m.text", "body": message});
    let client = Client::new();
    let mut last_error = String::from("Matrix send event failed");
    for attempt in 0..3 {
        match client
            .put(&endpoint)
            .bearer_auth(&token)
            .timeout(std::time::Duration::from_secs(30))
            .json(&payload)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                let body: serde_json::Value = response
                    .json()
                    .await
                    .context("Matrix send event response was not JSON")?;
                if body
                    .get("event_id")
                    .and_then(serde_json::Value::as_str)
                    .is_none()
                {
                    bail!("Matrix send event returned an incomplete response");
                }
                return Ok(());
            }
            Ok(response) => {
                let status = response.status();
                last_error = format!("Matrix send event returned HTTP {status}");
                if !is_retryable_status(status) {
                    break;
                }
            }
            Err(error) => last_error = crate::observability::redact(&error.to_string()),
        }
        if attempt < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(250 * (attempt + 1))).await;
        }
    }
    bail!(last_error)
}

pub async fn matrix_send_media(
    config: &ChannelConfig,
    room: &str,
    path: &std::path::Path,
    caption: Option<&str>,
    policy: &crate::policy::Policy,
) -> Result<()> {
    if room.trim().is_empty() {
        bail!("Matrix room is required");
    }
    if !telegram_recipient_allowed(config, room) {
        bail!("Matrix room is not allowlisted");
    }
    policy.check_path(path)?;
    let metadata =
        std::fs::metadata(path).with_context(|| format!("read media {}", path.display()))?;
    let max_bytes = std::env::var("SAPIENS_MATRIX_MAX_MEDIA_BYTES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(25 * 1024 * 1024)
        .clamp(1, 50 * 1024 * 1024);
    if metadata.len() == 0 || metadata.len() > max_bytes {
        bail!("Matrix media file exceeds the configured size limit");
    }
    let filename = path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .context("Matrix media filename is invalid")?;
    let mime_type = whatsapp_media_mime(path)?;
    let bytes = std::fs::read(path).with_context(|| format!("read media {}", path.display()))?;
    if bytes.len() as u64 > max_bytes {
        bail!("Matrix media file exceeds the configured size limit");
    }
    let token = matrix_token(config)?;
    let upload_endpoint = matrix_media_upload_endpoint()?;
    policy.check_url(&upload_endpoint)?;
    let upload: serde_json::Value = Client::new()
        .post(upload_endpoint)
        .query(&[("filename", filename)])
        .bearer_auth(&token)
        .header("content-type", &mime_type)
        .body(bytes.clone())
        .timeout(std::time::Duration::from_secs(90))
        .send()
        .await
        .context("Matrix media upload request failed")?
        .error_for_status()
        .context("Matrix media upload returned an error")?
        .json()
        .await
        .context("Matrix media upload response was not JSON")?;
    let content_uri = upload
        .get("content_uri")
        .and_then(serde_json::Value::as_str)
        .filter(|value| value.starts_with("mxc://") && value.len() > 6)
        .context("Matrix media upload response is missing a valid content_uri")?;
    let msgtype = matrix_media_msgtype(&mime_type);
    let mut content = serde_json::Map::new();
    content.insert("msgtype".into(), json!(msgtype));
    content.insert(
        "body".into(),
        json!(
            caption
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(filename)
        ),
    );
    content.insert("url".into(), json!(content_uri));
    content.insert(
        "info".into(),
        json!({"mimetype": mime_type, "size": bytes.len()}),
    );
    let endpoint = matrix_send_endpoint(room)?;
    policy.check_url(&endpoint)?;
    let response: serde_json::Value = Client::new()
        .put(endpoint)
        .bearer_auth(token)
        .json(&serde_json::Value::Object(content))
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .context("Matrix media message request failed")?
        .error_for_status()
        .context("Matrix media message returned an error")?
        .json()
        .await
        .context("Matrix media message response was not JSON")?;
    if response
        .get("event_id")
        .and_then(serde_json::Value::as_str)
        .is_none()
    {
        bail!("Matrix media message returned an incomplete response");
    }
    Ok(())
}

/// Fetches one incremental Matrix Client-Server `/sync` batch.
/// The caller persists `next_batch` only after it has durably handled the
/// returned events. The initial call must omit `since`.
pub async fn matrix_sync(
    config: &ChannelConfig,
    since: Option<&str>,
    timeout_ms: u64,
) -> Result<MatrixSyncBatch> {
    let token = matrix_token(config)?;
    let timeout_ms = timeout_ms.clamp(1, 120_000);
    let endpoint = matrix_endpoint("sync")?;
    let client = Client::new();
    let mut last_error = String::from("Matrix sync failed");
    for attempt in 0..3 {
        let mut request = client
            .get(&endpoint)
            .bearer_auth(&token)
            .query(&[
                ("timeout", timeout_ms.to_string()),
                ("set_presence", "offline".into()),
            ])
            .timeout(std::time::Duration::from_millis(
                timeout_ms.saturating_add(30_000),
            ));
        if let Some(since) = since.filter(|value| !value.trim().is_empty()) {
            request = request.query(&[("since", since)]);
        }
        match request.send().await {
            Ok(response) if response.status().is_success() => {
                let payload: serde_json::Value = response
                    .json()
                    .await
                    .context("Matrix sync response was not JSON")?;
                return parse_matrix_sync(&payload);
            }
            Ok(response) => {
                let status = response.status();
                last_error = format!("Matrix sync returned HTTP {status}");
                if !(status.is_server_error()
                    || status == reqwest::StatusCode::REQUEST_TIMEOUT
                    || status == reqwest::StatusCode::TOO_MANY_REQUESTS)
                {
                    break;
                }
            }
            Err(error) => last_error = crate::observability::redact(&error.to_string()),
        }
        if attempt < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(250 * (attempt + 1))).await;
        }
    }
    bail!(last_error)
}

pub fn parse_matrix_sync(payload: &serde_json::Value) -> Result<MatrixSyncBatch> {
    let next_batch = payload
        .get("next_batch")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .context("Matrix sync response is missing next_batch")?
        .to_string();
    let mut messages = Vec::new();
    let joined = payload
        .pointer("/rooms/join")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (room_id, room) in joined {
        let events = room
            .pointer("/timeline/events")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        for event in events {
            if event.get("type").and_then(serde_json::Value::as_str) != Some("m.room.message") {
                continue;
            }
            let Some(event_id) = event
                .get("event_id")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
            else {
                continue;
            };
            let Some(sender) = event
                .get("sender")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
            else {
                continue;
            };
            let content = event.get("content").cloned().unwrap_or_default();
            let msgtype = content
                .get("msgtype")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let body = content
                .get("body")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let media_uri = content
                .get("url")
                .and_then(serde_json::Value::as_str)
                .filter(|value| value.starts_with("mxc://") && value.len() > 6)
                .map(str::to_string);
            let is_text = matches!(msgtype, "m.text" | "m.notice") && !body.trim().is_empty();
            let is_media = matches!(msgtype, "m.image" | "m.audio" | "m.video" | "m.file")
                && media_uri.is_some();
            if !is_text && !is_media {
                continue;
            }
            let mentions = content
                .pointer("/m.mentions/user_ids")
                .and_then(serde_json::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            messages.push(MatrixInboundMessage {
                event_id: event_id.to_string(),
                room_id: room_id.clone(),
                sender: sender.to_string(),
                body,
                msgtype: msgtype.to_string(),
                media_uri,
                mime_type: content
                    .pointer("/info/mimetype")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                media_size: content
                    .pointer("/info/size")
                    .and_then(serde_json::Value::as_u64),
                filename: content
                    .get("filename")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| content.get("body").and_then(serde_json::Value::as_str))
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string),
                origin_server_ts: event
                    .get("origin_server_ts")
                    .and_then(serde_json::Value::as_u64),
                mentions,
            });
            if messages.len() >= 100 {
                bail!("Matrix sync batch contains too many message events");
            }
        }
    }
    Ok(MatrixSyncBatch {
        next_batch,
        messages,
    })
}

pub fn matrix_inbound_allowed(config: &ChannelConfig, message: &MatrixInboundMessage) -> bool {
    !config.allowlist.is_empty()
        && config.allowlist.iter().any(|allowed| {
            allowed == "*" || allowed == &message.sender || allowed == &message.room_id
        })
}

/// Downloads one Matrix `mxc://` attachment through the authenticated media
/// endpoint. The media server name remains a path component; the homeserver
/// is the only network destination contacted by the agent.
pub async fn matrix_download_media(
    config: &ChannelConfig,
    media_uri: &str,
    output_dir: &std::path::Path,
    max_bytes: u64,
    mime_hint: Option<&str>,
    expected_size: Option<u64>,
    policy: &crate::policy::Policy,
) -> Result<MatrixDownloadedMedia> {
    let (server_name, media_id) = parse_mxc_uri(media_uri)?;
    if max_bytes == 0 || max_bytes > 50 * 1024 * 1024 {
        bail!("Matrix media max_bytes must be between 1 and 50 MiB");
    }
    if expected_size.is_some_and(|size| size > max_bytes) {
        bail!("Matrix media exceeds the configured size limit");
    }
    policy.check_path(output_dir)?;
    let token = matrix_token(config)?;
    let endpoint = matrix_media_download_endpoint(&server_name, &media_id)?;
    policy.check_url(&endpoint)?;
    let response = Client::new()
        .get(&endpoint)
        .query(&[("allow_remote", "false")])
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(90))
        .send()
        .await
        .context("Matrix media download request failed")?
        .error_for_status()
        .context("Matrix media download returned an error")?;
    if response
        .content_length()
        .is_some_and(|size| size > max_bytes)
    {
        bail!("Matrix media download exceeds the configured size limit");
    }
    let header_mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let bytes = response
        .bytes()
        .await
        .context("Matrix media download body could not be read")?;
    if bytes.is_empty() || bytes.len() as u64 > max_bytes {
        bail!("Matrix media download exceeds the configured size limit");
    }
    let actual_hash = hex_encode(&Sha256::digest(&bytes));
    let mime_type = mime_hint
        .or(header_mime.as_deref())
        .unwrap_or("application/octet-stream")
        .trim()
        .to_ascii_lowercase();
    if mime_type.chars().any(|character| character.is_control()) {
        bail!("Matrix media MIME type is invalid");
    }
    let extension = whatsapp_media_extension(&mime_type);
    let file_key = hex_encode(&Sha256::digest(media_uri.as_bytes()));
    std::fs::create_dir_all(output_dir)?;
    let path = output_dir.join(format!("media-{file_key}.{extension}"));
    policy.check_path(&path)?;
    std::fs::write(&path, &bytes)?;
    Ok(MatrixDownloadedMedia {
        media_uri: media_uri.to_string(),
        path: path.display().to_string(),
        mime_type,
        size_bytes: bytes.len() as u64,
        sha256: actual_hash,
    })
}

fn parse_mxc_uri(media_uri: &str) -> Result<(String, String)> {
    let value = media_uri
        .strip_prefix("mxc://")
        .context("Matrix media URI must use mxc://")?;
    let (server_name, media_id) = value
        .split_once('/')
        .context("Matrix media URI is missing server or media id")?;
    if server_name.trim().is_empty()
        || media_id.trim().is_empty()
        || media_id.contains('/')
        || media_id.contains('?')
        || media_id.contains('#')
        || server_name.chars().any(|character| character.is_control())
        || media_id.chars().any(|character| character.is_control())
    {
        bail!("Matrix media URI is invalid");
    }
    Ok((server_name.to_string(), media_id.to_string()))
}

fn matrix_media_download_endpoint(server_name: &str, media_id: &str) -> Result<String> {
    let base = std::env::var("SAPIENS_MATRIX_HOMESERVER")
        .context("SAPIENS_MATRIX_HOMESERVER is not configured")?;
    let base = base.trim_end_matches('/');
    let mut parsed = url::Url::parse(base).context("Matrix homeserver URL is invalid")?;
    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        bail!("Matrix homeserver URL must use http or https");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("Matrix homeserver URL userinfo is blocked");
    }
    {
        let mut segments = parsed
            .path_segments_mut()
            .map_err(|_| anyhow::anyhow!("Matrix homeserver URL cannot be a base"))?;
        segments.pop_if_empty();
        segments.extend([
            "_matrix",
            "client",
            "v1",
            "media",
            "download",
            server_name,
            media_id,
        ]);
    }
    Ok(parsed.to_string())
}

pub async fn whatsapp_test(config: &ChannelConfig) -> Result<()> {
    let token = whatsapp_token(config)?;
    let phone_id = whatsapp_phone_number_id()?;
    let endpoint = whatsapp_endpoint(&phone_id, "")?;
    let response: serde_json::Value = Client::new()
        .get(endpoint)
        .query(&[("fields", "id,display_phone_number")])
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .context("WhatsApp Cloud API phone check failed")?
        .error_for_status()
        .context("WhatsApp Cloud API phone check returned an error")?
        .json()
        .await
        .context("WhatsApp Cloud API response was not JSON")?;
    if response
        .get("id")
        .and_then(serde_json::Value::as_str)
        .is_none()
    {
        bail!("WhatsApp Cloud API returned an incomplete phone response");
    }
    Ok(())
}

pub async fn whatsapp_send(config: &ChannelConfig, recipient: &str, message: &str) -> Result<()> {
    if recipient.trim().is_empty() || message.trim().is_empty() {
        bail!("WhatsApp recipient and message are required");
    }
    if !telegram_recipient_allowed(config, recipient) {
        bail!("WhatsApp recipient is not allowlisted");
    }
    let token = whatsapp_token(config)?;
    let phone_id = whatsapp_phone_number_id()?;
    let endpoint = whatsapp_endpoint(&phone_id, "messages")?;
    let payload = json!({
        "messaging_product": "whatsapp",
        "recipient_type": "individual",
        "to": recipient,
        "type": "text",
        "text": {"preview_url": false, "body": message}
    });
    let client = Client::new();
    let mut last_error = String::from("WhatsApp Cloud API send failed");
    for attempt in 0..3 {
        match client
            .post(&endpoint)
            .bearer_auth(&token)
            .timeout(std::time::Duration::from_secs(30))
            .json(&payload)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                let body: serde_json::Value = response
                    .json()
                    .await
                    .context("WhatsApp Cloud API response was not JSON")?;
                if body
                    .pointer("/messages/0/id")
                    .and_then(serde_json::Value::as_str)
                    .is_none()
                {
                    bail!("WhatsApp Cloud API send returned an incomplete response");
                }
                return Ok(());
            }
            Ok(response) => {
                let status = response.status();
                last_error = format!("WhatsApp Cloud API returned HTTP {status}");
                if !(status.is_server_error()
                    || status == reqwest::StatusCode::REQUEST_TIMEOUT
                    || status == reqwest::StatusCode::TOO_MANY_REQUESTS)
                {
                    break;
                }
            }
            Err(error) => last_error = crate::observability::redact(&error.to_string()),
        }
        if attempt < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(250 * (attempt + 1))).await;
        }
    }
    bail!(last_error)
}

/// Retrieves and stores an inbound WhatsApp Cloud API media object.
/// The media URL is short-lived, is checked by the caller's network policy,
/// and the downloaded bytes are capped and verified against Meta's SHA-256.
pub async fn whatsapp_download_media(
    config: &ChannelConfig,
    media_id: &str,
    output_dir: &std::path::Path,
    max_bytes: u64,
    policy: &crate::policy::Policy,
) -> Result<WhatsAppDownloadedMedia> {
    validate_whatsapp_media_id(media_id)?;
    if max_bytes == 0 || max_bytes > 50 * 1024 * 1024 {
        bail!("WhatsApp media max_bytes must be between 1 and 50 MiB");
    }
    policy.check_path(output_dir)?;
    let token = whatsapp_token(config)?;
    let phone_id = whatsapp_phone_number_id()?;
    let metadata_endpoint = whatsapp_media_endpoint(media_id)?;
    policy.check_url(&metadata_endpoint)?;
    let metadata: serde_json::Value = Client::new()
        .get(&metadata_endpoint)
        .query(&[("phone_number_id", phone_id)])
        .bearer_auth(&token)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .context("WhatsApp media metadata request failed")?
        .error_for_status()
        .context("WhatsApp media metadata returned an error")?
        .json()
        .await
        .context("WhatsApp media metadata was not JSON")?;
    let media_url = metadata
        .get("url")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .context("WhatsApp media metadata is missing url")?;
    let media_url = url::Url::parse(media_url).context("WhatsApp media URL is invalid")?;
    if media_url.scheme() != "https" && media_url.scheme() != "http" {
        bail!("WhatsApp media URL must use http or https");
    }
    if !media_url.username().is_empty() || media_url.password().is_some() {
        bail!("WhatsApp media URL userinfo is blocked");
    }
    policy.check_url(media_url.as_str())?;
    let expected_size = metadata
        .get("file_size")
        .and_then(serde_json::Value::as_u64);
    if expected_size.is_some_and(|size| size > max_bytes) {
        bail!("WhatsApp media exceeds the configured size limit");
    }
    let expected_hash = metadata
        .get("sha256")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if !expected_hash.is_empty()
        && (expected_hash.len() != 64
            || !expected_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')))
    {
        bail!("WhatsApp media metadata has an invalid sha256");
    }
    let response = Client::new()
        .get(media_url)
        .bearer_auth(&token)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .context("WhatsApp media download request failed")?
        .error_for_status()
        .context("WhatsApp media download returned an error")?;
    if response
        .content_length()
        .is_some_and(|size| size > max_bytes)
    {
        bail!("WhatsApp media download exceeds the configured size limit");
    }
    let bytes = response
        .bytes()
        .await
        .context("WhatsApp media download body could not be read")?;
    if bytes.len() as u64 > max_bytes {
        bail!("WhatsApp media download exceeds the configured size limit");
    }
    let actual_hash = hex_encode(&Sha256::digest(&bytes));
    if !expected_hash.is_empty()
        && !constant_time_equal_bytes(actual_hash.as_bytes(), expected_hash.as_bytes())
    {
        bail!("WhatsApp media sha256 verification failed");
    }
    let mime_type = metadata
        .get("mime_type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("application/octet-stream")
        .trim()
        .to_ascii_lowercase();
    let extension = whatsapp_media_extension(&mime_type);
    let file_key = hex_encode(&Sha256::digest(media_id.as_bytes()));
    std::fs::create_dir_all(output_dir)?;
    let path = output_dir.join(format!("media-{file_key}.{extension}"));
    policy.check_path(&path)?;
    std::fs::write(&path, &bytes)?;
    Ok(WhatsAppDownloadedMedia {
        media_id: media_id.to_string(),
        path: path.display().to_string(),
        mime_type,
        size_bytes: bytes.len() as u64,
        sha256: actual_hash,
    })
}

pub async fn whatsapp_send_media(
    config: &ChannelConfig,
    recipient: &str,
    path: &std::path::Path,
    caption: Option<&str>,
    policy: &crate::policy::Policy,
) -> Result<()> {
    if recipient.trim().is_empty() {
        bail!("WhatsApp recipient is required");
    }
    if !telegram_recipient_allowed(config, recipient) {
        bail!("WhatsApp recipient is not allowlisted");
    }
    policy.check_path(path)?;
    let metadata =
        std::fs::metadata(path).with_context(|| format!("read media {}", path.display()))?;
    let max_bytes = std::env::var("SAPIENS_WHATSAPP_MAX_MEDIA_BYTES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(25 * 1024 * 1024)
        .clamp(1, 50 * 1024 * 1024);
    if metadata.len() == 0 || metadata.len() > max_bytes {
        bail!("WhatsApp media file exceeds the configured size limit");
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .context("WhatsApp media filename is invalid")?;
    let mime_type = whatsapp_media_mime(path)?;
    let bytes = std::fs::read(path).with_context(|| format!("read media {}", path.display()))?;
    if bytes.len() as u64 > max_bytes {
        bail!("WhatsApp media file exceeds the configured size limit");
    }
    let token = whatsapp_token(config)?;
    let phone_id = whatsapp_phone_number_id()?;
    let upload_endpoint = whatsapp_endpoint(&phone_id, "media")?;
    policy.check_url(&upload_endpoint)?;
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(file_name.to_string())
        .mime_str(&mime_type)
        .context("WhatsApp media MIME type is invalid")?;
    let upload: serde_json::Value = Client::new()
        .post(upload_endpoint)
        .bearer_auth(&token)
        .multipart(
            reqwest::multipart::Form::new()
                .text("messaging_product", "whatsapp")
                .part("file", part),
        )
        .timeout(std::time::Duration::from_secs(90))
        .send()
        .await
        .context("WhatsApp media upload request failed")?
        .error_for_status()
        .context("WhatsApp media upload returned an error")?
        .json()
        .await
        .context("WhatsApp media upload response was not JSON")?;
    let media_id = upload
        .get("id")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .context("WhatsApp media upload response is missing id")?;
    let media_kind = whatsapp_media_kind(&mime_type)?;
    let mut media = serde_json::Map::new();
    media.insert("id".into(), serde_json::Value::String(media_id.to_string()));
    if let Some(caption) = caption.filter(|value| !value.trim().is_empty()) {
        media.insert(
            "caption".into(),
            serde_json::Value::String(caption.to_string()),
        );
    }
    let mut payload = serde_json::Map::new();
    payload.insert("messaging_product".into(), json!("whatsapp"));
    payload.insert("recipient_type".into(), json!("individual"));
    payload.insert("to".into(), json!(recipient));
    payload.insert("type".into(), json!(media_kind));
    payload.insert(media_kind.into(), serde_json::Value::Object(media));
    let send_endpoint = whatsapp_endpoint(&phone_id, "messages")?;
    policy.check_url(&send_endpoint)?;
    let response: serde_json::Value = Client::new()
        .post(send_endpoint)
        .bearer_auth(token)
        .json(&serde_json::Value::Object(payload))
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .context("WhatsApp media message request failed")?
        .error_for_status()
        .context("WhatsApp media message returned an error")?
        .json()
        .await
        .context("WhatsApp media message response was not JSON")?;
    if response
        .pointer("/messages/0/id")
        .and_then(serde_json::Value::as_str)
        .is_none()
    {
        bail!("WhatsApp media message returned an incomplete response");
    }
    Ok(())
}

fn whatsapp_media_mime(path: &std::path::Path) -> Result<String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mime = match extension.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "aac" => "audio/aac",
        "m4a" | "mp4a" => "audio/mp4",
        "mp3" => "audio/mpeg",
        "amr" => "audio/amr",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "pdf" => "application/pdf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => bail!(
            "unsupported WhatsApp media extension; use a supported image/audio/video/document file"
        ),
    };
    Ok(mime.into())
}

fn whatsapp_media_kind(mime_type: &str) -> Result<&'static str> {
    if mime_type.starts_with("image/") {
        Ok("image")
    } else if mime_type.starts_with("audio/") {
        Ok("audio")
    } else if mime_type.starts_with("video/") {
        Ok("video")
    } else if mime_type.starts_with("application/") {
        Ok("document")
    } else {
        bail!("unsupported WhatsApp media MIME type")
    }
}

pub fn whatsapp_media_endpoint(media_id: &str) -> Result<String> {
    validate_whatsapp_media_id(media_id)?;
    let base = std::env::var("SAPIENS_WHATSAPP_GRAPH_BASE_URL")
        .unwrap_or_else(|_| "https://graph.facebook.com/v20.0".into());
    let base = base.trim_end_matches('/');
    let mut parsed = url::Url::parse(base).context("WhatsApp Graph base URL is invalid")?;
    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        bail!("WhatsApp Graph base URL must use http or https");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("WhatsApp Graph base URL userinfo is blocked");
    }
    {
        let mut segments = parsed
            .path_segments_mut()
            .map_err(|_| anyhow::anyhow!("WhatsApp Graph URL cannot be a base"))?;
        segments.pop_if_empty();
        segments.push(media_id);
    }
    Ok(parsed.to_string())
}

fn validate_whatsapp_media_id(media_id: &str) -> Result<()> {
    if media_id.trim().is_empty()
        || media_id.len() > 256
        || media_id
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
    {
        bail!("WhatsApp media id is invalid");
    }
    Ok(())
}

fn whatsapp_media_extension(mime_type: &str) -> &'static str {
    match mime_type {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "audio/aac" => "aac",
        "audio/mp4" => "m4a",
        "audio/mpeg" => "mp3",
        "audio/amr" => "amr",
        "audio/ogg" => "ogg",
        "video/mp4" => "mp4",
        "application/pdf" => "pdf",
        "application/vnd.ms-powerpoint" => "ppt",
        "application/msword" => "doc",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "xlsx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => "pptx",
        _ => "bin",
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn constant_time_equal_bytes(left: &[u8], right: &[u8]) -> bool {
    let mut difference: u8 = if left.len() == right.len() { 0 } else { 1 };
    for index in 0..left.len().max(right.len()) {
        difference |= left.get(index).copied().unwrap_or_default()
            ^ right.get(index).copied().unwrap_or_default();
    }
    difference == 0
}

/// Checks the locally installed signal-cli without modifying Signal state.
/// Account registration and server connectivity are verified by the first send.
pub async fn signal_test(config: &ChannelConfig) -> Result<()> {
    let _account = signal_account(config)?;
    let command = signal_cli_command();
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        Command::new(&command)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .context("signal-cli health check timed out")?
    .with_context(|| format!("signal-cli executable unavailable: {command}"))?;
    if !output.status.success() {
        let details = String::from_utf8_lossy(&output.stderr);
        bail!(
            "signal-cli health check failed: {}",
            crate::observability::redact(details.trim())
        );
    }
    Ok(())
}

/// Sends one text message through the locally installed signal-cli binary.
/// The executable is invoked directly (no shell), so message contents cannot
/// become command-line syntax.
pub async fn signal_send(config: &ChannelConfig, recipient: &str, message: &str) -> Result<()> {
    if recipient.trim().is_empty() || message.trim().is_empty() {
        bail!("Signal recipient and message are required");
    }
    if let Some(group_id) = recipient.strip_prefix("group:") {
        return signal_send_group(config, group_id, message).await;
    }
    validate_signal_argument(recipient, "Signal recipient")?;
    if !telegram_recipient_allowed(config, recipient) {
        bail!("Signal recipient is not allowlisted");
    }
    let account = signal_account(config)?;
    validate_signal_argument(&account, "Signal account")?;
    let command = signal_cli_command();
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        Command::new(&command)
            .arg("-a")
            .arg(&account)
            .arg("send")
            .arg("-m")
            .arg(message)
            .arg(recipient)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .context("Signal send timed out")?
    .with_context(|| format!("signal-cli executable unavailable: {command}"))?;
    if !output.status.success() {
        let details = String::from_utf8_lossy(&output.stderr);
        bail!(
            "Signal send failed: {}",
            crate::observability::redact(details.trim())
        );
    }
    Ok(())
}

pub async fn signal_send_group(
    config: &ChannelConfig,
    group_id: &str,
    message: &str,
) -> Result<()> {
    if group_id.trim().is_empty() || message.trim().is_empty() {
        bail!("Signal group and message are required");
    }
    validate_signal_argument(group_id, "Signal group")?;
    if !telegram_recipient_allowed(config, group_id) {
        bail!("Signal group is not allowlisted");
    }
    let account = signal_account(config)?;
    validate_signal_argument(&account, "Signal account")?;
    let command = signal_cli_command();
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        Command::new(&command)
            .arg("-a")
            .arg(&account)
            .arg("send")
            .arg("-g")
            .arg(group_id)
            .arg("-m")
            .arg(message)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .context("Signal group send timed out")?
    .with_context(|| format!("signal-cli executable unavailable: {command}"))?;
    if !output.status.success() {
        let details = String::from_utf8_lossy(&output.stderr);
        bail!(
            "Signal group send failed: {}",
            crate::observability::redact(details.trim())
        );
    }
    Ok(())
}

pub async fn signal_send_media(
    config: &ChannelConfig,
    recipient: &str,
    path: &std::path::Path,
    caption: Option<&str>,
    policy: &crate::policy::Policy,
) -> Result<()> {
    if recipient.trim().is_empty() {
        bail!("Signal recipient is required");
    }
    let (group_id, direct_recipient) = recipient
        .strip_prefix("group:")
        .map_or((None, Some(recipient)), |group| (Some(group), None));
    let allowed_recipient = group_id.or(direct_recipient).unwrap_or_default();
    validate_signal_argument(allowed_recipient, "Signal recipient")?;
    if !telegram_recipient_allowed(config, allowed_recipient) {
        bail!("Signal recipient is not allowlisted");
    }
    policy.check_path(path)?;
    let metadata =
        std::fs::metadata(path).with_context(|| format!("read media {}", path.display()))?;
    let max_bytes = std::env::var("SAPIENS_SIGNAL_MAX_MEDIA_BYTES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(25 * 1024 * 1024)
        .clamp(1, 50 * 1024 * 1024);
    if metadata.len() == 0 || metadata.len() > max_bytes {
        bail!("Signal media file exceeds the configured size limit");
    }
    let account = signal_account(config)?;
    validate_signal_argument(&account, "Signal account")?;
    let command = signal_cli_command();
    let args = signal_media_args(&account, group_id, direct_recipient, path, caption)?;
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(90),
        Command::new(&command)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .context("Signal media send timed out")?
    .with_context(|| format!("signal-cli executable unavailable: {command}"))?;
    if !output.status.success() {
        let details = String::from_utf8_lossy(&output.stderr);
        bail!(
            "Signal media send failed: {}",
            crate::observability::redact(details.trim())
        );
    }
    Ok(())
}

fn signal_media_args(
    account: &str,
    group_id: Option<&str>,
    direct_recipient: Option<&str>,
    path: &std::path::Path,
    caption: Option<&str>,
) -> Result<Vec<String>> {
    validate_signal_argument(account, "Signal account")?;
    if group_id.is_none() && direct_recipient.is_none() {
        bail!("Signal recipient is required");
    }
    let mut args = vec!["-a".to_string(), account.to_string(), "send".into()];
    if let Some(group_id) = group_id {
        validate_signal_argument(group_id, "Signal group")?;
        args.extend(["-g".into(), group_id.to_string()]);
    }
    if let Some(recipient) = direct_recipient {
        validate_signal_argument(recipient, "Signal recipient")?;
        args.push(recipient.to_string());
    }
    if let Some(caption) = caption.filter(|value| !value.trim().is_empty()) {
        args.extend(["-m".into(), caption.to_string()]);
    }
    args.extend(["--attachment".into(), path.display().to_string()]);
    Ok(args)
}

/// Receives one bounded batch from the locally installed signal-cli.
/// signal-cli emits one JSON envelope per line. Attachment metadata is kept
/// until the worker can fetch the bytes through the explicit getAttachment
/// command under the same sender/group allowlist.
pub async fn signal_receive(
    config: &ChannelConfig,
    timeout_secs: u64,
) -> Result<Vec<SignalInboundMessage>> {
    let account = signal_account(config)?;
    validate_signal_argument(&account, "Signal account")?;
    let timeout_secs = timeout_secs.clamp(1, 300);
    let command = signal_cli_command();
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs.saturating_add(30)),
        Command::new(&command)
            .arg("-a")
            .arg(&account)
            .arg("receive")
            .arg("--json")
            .arg("--max-messages")
            .arg("50")
            .arg("--timeout")
            .arg(timeout_secs.to_string())
            .arg("--ignore-stories")
            .arg("--ignore-avatars")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .context("Signal receive timed out")?
    .with_context(|| format!("signal-cli executable unavailable: {command}"))?;
    if !output.status.success() {
        let details = String::from_utf8_lossy(&output.stderr);
        bail!(
            "Signal receive failed: {}",
            crate::observability::redact(details.trim())
        );
    }
    parse_signal_receive_output(&String::from_utf8_lossy(&output.stdout))
}

pub fn parse_signal_receive_output(output: &str) -> Result<Vec<SignalInboundMessage>> {
    let mut messages = Vec::new();
    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let payload: serde_json::Value = serde_json::from_str(line)
            .with_context(|| "signal-cli returned a non-JSON receive line")?;
        let Some(envelope) = payload.get("envelope") else {
            continue;
        };
        let Some(data) = envelope.get("dataMessage") else {
            continue;
        };
        let text = data
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let Some(sender) = envelope
            .get("source")
            .or_else(|| envelope.get("sourceNumber"))
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
        else {
            continue;
        };
        let timestamp = envelope
            .get("timestamp")
            .or_else(|| data.get("timestamp"))
            .and_then(serde_json::Value::as_u64);
        let source_device = envelope
            .get("sourceDevice")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default();
        let event_id = format!(
            "{}:{}:{}",
            sender,
            timestamp.unwrap_or_default(),
            source_device
        );
        let group_id = data
            .pointer("/groupInfo/groupId")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let attachment_details: Vec<SignalAttachment> = data
            .get("attachments")
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let id = item
                            .get("id")
                            .or_else(|| item.get("attachmentId"))
                            .and_then(json_scalar_string)?;
                        Some(SignalAttachment {
                            id,
                            filename: item
                                .get("filename")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string),
                            content_type: item
                                .get("contentType")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string),
                            size_bytes: item.get("size").and_then(serde_json::Value::as_u64),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        if text.trim().is_empty() && attachment_details.is_empty() {
            continue;
        }
        let attachments = attachment_details
            .iter()
            .map(|attachment| attachment.id.clone())
            .collect();
        messages.push(SignalInboundMessage {
            event_id,
            sender: sender.to_string(),
            group_id,
            text,
            attachments,
            attachment_details,
            timestamp,
        });
        if messages.len() >= 100 {
            bail!("Signal receive batch contains too many messages");
        }
    }
    Ok(messages)
}

fn json_scalar_string(value: &serde_json::Value) -> Option<String> {
    value
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| value.as_u64().map(|value| value.to_string()))
}

/// Fetches one Signal attachment through signal-cli's explicit attachment
/// command and writes it under the workspace. signal-cli returns Base64 data,
/// so the decoded bytes are capped before being persisted.
pub async fn signal_download_attachment(
    config: &ChannelConfig,
    attachment: &SignalAttachment,
    sender: &str,
    group_id: Option<&str>,
    output_dir: &std::path::Path,
    max_bytes: u64,
    policy: &crate::policy::Policy,
) -> Result<SignalDownloadedMedia> {
    if max_bytes == 0 || max_bytes > 50 * 1024 * 1024 {
        bail!("Signal media max_bytes must be between 1 and 50 MiB");
    }
    validate_signal_argument(&attachment.id, "Signal attachment id")?;
    if let Some(group_id) = group_id {
        validate_signal_argument(group_id, "Signal group")?;
        if !telegram_recipient_allowed(config, group_id) {
            bail!("Signal group is not allowlisted");
        }
    } else {
        validate_signal_argument(sender, "Signal sender")?;
        if !telegram_recipient_allowed(config, sender) {
            bail!("Signal sender is not allowlisted");
        }
    }
    if attachment.size_bytes.is_some_and(|size| size > max_bytes) {
        bail!("Signal media exceeds the configured size limit");
    }
    policy.check_path(output_dir)?;
    let account = signal_account(config)?;
    validate_signal_argument(&account, "Signal account")?;
    let command = signal_cli_command();
    let mut request = Command::new(&command);
    request
        .arg("-a")
        .arg(&account)
        .arg("getAttachment")
        .arg("--id")
        .arg(&attachment.id);
    if let Some(group_id) = group_id {
        request.arg("--group-id").arg(group_id);
    } else {
        request.arg("--recipient").arg(sender);
    }
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(90),
        request
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .context("Signal attachment download timed out")?
    .with_context(|| format!("signal-cli executable unavailable: {command}"))?;
    if !output.status.success() {
        let details = String::from_utf8_lossy(&output.stderr);
        bail!(
            "Signal attachment download failed: {}",
            crate::observability::redact(details.trim())
        );
    }
    let encoded = String::from_utf8_lossy(&output.stdout);
    let encoded_limit = max_bytes.saturating_mul(4).div_ceil(3).saturating_add(4);
    if encoded.len() as u64 > encoded_limit {
        bail!("Signal media download exceeds the configured size limit");
    }
    let bytes = decode_base64(&encoded)?;
    if bytes.is_empty() || bytes.len() as u64 > max_bytes {
        bail!("Signal media download exceeds the configured size limit");
    }
    let mime_type = attachment
        .content_type
        .as_deref()
        .unwrap_or("application/octet-stream")
        .split(';')
        .next()
        .unwrap_or("application/octet-stream")
        .trim()
        .to_ascii_lowercase();
    if mime_type.chars().any(|character| character.is_control()) {
        bail!("Signal media MIME type is invalid");
    }
    let extension = whatsapp_media_extension(&mime_type);
    let file_key = hex_encode(&Sha256::digest(attachment.id.as_bytes()));
    std::fs::create_dir_all(output_dir)?;
    let path = output_dir.join(format!("media-{file_key}.{extension}"));
    policy.check_path(&path)?;
    std::fs::write(&path, &bytes)?;
    Ok(SignalDownloadedMedia {
        attachment_id: attachment.id.clone(),
        path: path.display().to_string(),
        mime_type,
        size_bytes: bytes.len() as u64,
        sha256: hex_encode(&Sha256::digest(&bytes)),
    })
}

fn decode_base64(value: &str) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut accumulator = 0_u32;
    let mut bits = 0_u8;
    let mut padding = false;
    for byte in value.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        if byte == b'=' {
            padding = true;
            continue;
        }
        if padding {
            bail!("Signal attachment returned invalid Base64");
        }
        let digit = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => bail!("Signal attachment returned invalid Base64"),
        } as u32;
        accumulator = (accumulator << 6) | digit;
        bits = bits.saturating_add(6);
        if bits >= 8 {
            bits -= 8;
            output.push(((accumulator >> bits) & 0xff) as u8);
        }
    }
    if bits > 4 || (padding && bits == 0) {
        bail!("Signal attachment returned invalid Base64");
    }
    Ok(output)
}

pub fn signal_inbound_allowed(config: &ChannelConfig, message: &SignalInboundMessage) -> bool {
    !config.allowlist.is_empty()
        && config.allowlist.iter().any(|allowed| {
            allowed == "*"
                || allowed == &message.sender
                || message.group_id.as_deref() == Some(allowed.as_str())
        })
}

fn signal_cli_command() -> String {
    std::env::var("SAPIENS_SIGNAL_CLI_COMMAND").unwrap_or_else(|_| "signal-cli".into())
}

fn signal_account(config: &ChannelConfig) -> Result<String> {
    if config.credential_env.trim().is_empty() {
        bail!("Signal credential_env is not configured");
    }
    let account = std::env::var(&config.credential_env).with_context(|| {
        format!(
            "missing Signal account environment variable {}",
            config.credential_env
        )
    })?;
    if account.trim().is_empty() {
        bail!("Signal account environment variable is empty");
    }
    Ok(account)
}

fn validate_signal_argument(value: &str, label: &str) -> Result<()> {
    if value.starts_with('-') || value.chars().any(|character| character.is_control()) {
        bail!("{label} contains an invalid command argument");
    }
    Ok(())
}

pub async fn webhook_test(config: &ChannelConfig) -> Result<u16> {
    let endpoint = webhook_endpoint(config)?;
    let response = Client::new()
        .get(endpoint)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .context("channel webhook probe failed")?;
    let status = response.status();
    if status.is_server_error() {
        bail!("channel webhook probe returned HTTP {status}");
    }
    Ok(status.as_u16())
}

pub async fn webhook_send(config: &ChannelConfig, recipient: &str, message: &str) -> Result<()> {
    if recipient.trim().is_empty() || message.trim().is_empty() {
        bail!("channel recipient and message are required");
    }
    if !webhook_recipient_allowed(config, recipient) {
        bail!("channel recipient is not allowlisted");
    }
    let kind = normalize_kind(&config.kind);
    if !is_webhook_adapter(&kind) {
        bail!("channel kind does not have a webhook adapter: {kind}");
    }
    let endpoint = webhook_endpoint(config)?;
    let payload = webhook_payload(&kind, message);
    let client = Client::new();
    let mut last_error = String::from("channel webhook delivery failed");
    for attempt in 0..3 {
        match client
            .post(&endpoint)
            .header("content-type", "application/json")
            .timeout(std::time::Duration::from_secs(30))
            .json(&payload)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => return Ok(()),
            Ok(response) => {
                let status = response.status();
                last_error = format!("{kind} webhook returned HTTP {status}");
                if !(status.is_server_error()
                    || status == reqwest::StatusCode::REQUEST_TIMEOUT
                    || status == reqwest::StatusCode::TOO_MANY_REQUESTS)
                {
                    break;
                }
            }
            Err(error) => last_error = crate::observability::redact(&error.to_string()),
        }
        if attempt < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(250 * (attempt + 1))).await;
        }
    }
    bail!(last_error)
}

pub fn webhook_recipient_allowed(config: &ChannelConfig, recipient: &str) -> bool {
    telegram_recipient_allowed(config, recipient)
}

pub fn webhook_endpoint(config: &ChannelConfig) -> Result<String> {
    if config.credential_env.trim().is_empty() {
        bail!("channel webhook credential_env is not configured");
    }
    let endpoint = std::env::var(&config.credential_env).with_context(|| {
        format!(
            "missing credential environment variable {}",
            config.credential_env
        )
    })?;
    if endpoint.trim().is_empty() {
        bail!("channel webhook environment variable is empty");
    }
    let parsed = url::Url::parse(&endpoint).context("channel webhook URL is invalid")?;
    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        bail!("channel webhook URL must use http or https");
    }
    if parsed.username() != "" || parsed.password().is_some() {
        bail!("channel webhook URL userinfo is blocked");
    }
    Ok(endpoint)
}

pub fn is_webhook_adapter(kind: &str) -> bool {
    matches!(
        normalize_kind(kind).as_str(),
        "discord" | "slack" | "google-chat" | "teams"
    )
}

fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    status.is_server_error()
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
}

fn webhook_payload(kind: &str, message: &str) -> serde_json::Value {
    match normalize_kind(kind).as_str() {
        "discord" => json!({"content": message}),
        "slack" | "google-chat" | "teams" => json!({"text": message}),
        _ => unreachable!("validated webhook adapter"),
    }
}

pub fn telegram_recipient_allowed(config: &ChannelConfig, recipient: &str) -> bool {
    !config.allowlist.is_empty()
        && config
            .allowlist
            .iter()
            .any(|allowed| allowed == "*" || allowed == recipient)
}

fn telegram_token(config: &ChannelConfig) -> Result<String> {
    if config.credential_env.trim().is_empty() {
        bail!("Telegram credential_env is not configured");
    }
    let token = std::env::var(&config.credential_env).with_context(|| {
        format!(
            "missing credential environment variable {}",
            config.credential_env
        )
    })?;
    if token.trim().is_empty() {
        bail!("Telegram credential environment variable is empty");
    }
    Ok(token)
}

fn matrix_token(config: &ChannelConfig) -> Result<String> {
    if config.credential_env.trim().is_empty() {
        bail!("Matrix credential_env is not configured");
    }
    let token = std::env::var(&config.credential_env).with_context(|| {
        format!(
            "missing Matrix credential environment variable {}",
            config.credential_env
        )
    })?;
    if token.trim().is_empty() {
        bail!("Matrix credential environment variable is empty");
    }
    Ok(token)
}

pub fn matrix_endpoint(suffix: &str) -> Result<String> {
    let base = std::env::var("SAPIENS_MATRIX_HOMESERVER")
        .context("SAPIENS_MATRIX_HOMESERVER is not configured")?;
    let base = base.trim_end_matches('/');
    let parsed = url::Url::parse(base).context("Matrix homeserver URL is invalid")?;
    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        bail!("Matrix homeserver URL must use http or https");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("Matrix homeserver URL userinfo is blocked");
    }
    Ok(format!("{base}/_matrix/client/v3/{suffix}"))
}

fn matrix_media_upload_endpoint() -> Result<String> {
    let base = std::env::var("SAPIENS_MATRIX_HOMESERVER")
        .context("SAPIENS_MATRIX_HOMESERVER is not configured")?;
    let base = base.trim_end_matches('/');
    let parsed = url::Url::parse(base).context("Matrix homeserver URL is invalid")?;
    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        bail!("Matrix homeserver URL must use http or https");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("Matrix homeserver URL userinfo is blocked");
    }
    Ok(format!("{base}/_matrix/media/v3/upload"))
}

fn matrix_media_msgtype(mime_type: &str) -> &'static str {
    if mime_type.starts_with("image/") {
        "m.image"
    } else if mime_type.starts_with("audio/") {
        "m.audio"
    } else if mime_type.starts_with("video/") {
        "m.video"
    } else {
        "m.file"
    }
}

fn matrix_send_endpoint(room: &str) -> Result<String> {
    let base = matrix_endpoint("")?;
    let mut parsed = url::Url::parse(&base)?;
    let txn_id = format!("sapiens-{}", unix_nanos());
    {
        let mut segments = parsed
            .path_segments_mut()
            .map_err(|_| anyhow::anyhow!("Matrix homeserver URL cannot be a base"))?;
        segments.pop_if_empty();
        segments.extend(["rooms", room, "send", "m.room.message", &txn_id]);
    }
    Ok(parsed.to_string())
}

fn unix_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

fn whatsapp_token(config: &ChannelConfig) -> Result<String> {
    if config.credential_env.trim().is_empty() {
        bail!("WhatsApp credential_env is not configured");
    }
    let token = std::env::var(&config.credential_env).with_context(|| {
        format!(
            "missing WhatsApp credential environment variable {}",
            config.credential_env
        )
    })?;
    if token.trim().is_empty() {
        bail!("WhatsApp credential environment variable is empty");
    }
    Ok(token)
}

fn whatsapp_phone_number_id() -> Result<String> {
    let value = std::env::var("SAPIENS_WHATSAPP_PHONE_NUMBER_ID")
        .context("SAPIENS_WHATSAPP_PHONE_NUMBER_ID is not configured")?;
    if value.trim().is_empty() || !value.chars().all(|character| character.is_ascii_digit()) {
        bail!("SAPIENS_WHATSAPP_PHONE_NUMBER_ID must contain only digits");
    }
    Ok(value)
}

pub fn whatsapp_endpoint(phone_id: &str, suffix: &str) -> Result<String> {
    let base = std::env::var("SAPIENS_WHATSAPP_GRAPH_BASE_URL")
        .unwrap_or_else(|_| "https://graph.facebook.com/v20.0".into());
    let base = base.trim_end_matches('/');
    let parsed = url::Url::parse(base).context("WhatsApp Graph base URL is invalid")?;
    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        bail!("WhatsApp Graph base URL must use http or https");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("WhatsApp Graph base URL userinfo is blocked");
    }
    let suffix = suffix.trim_matches('/');
    if suffix.is_empty() {
        Ok(format!("{base}/{phone_id}"))
    } else {
        Ok(format!("{base}/{phone_id}/{suffix}"))
    }
}

pub struct CliChannel;
impl Channel for CliChannel {
    fn name(&self) -> &'static str {
        "cli"
    }
    fn verify_sender(&self, sender: &str) -> bool {
        sender == "local"
    }
}

const CATALOG: &[ChannelSpec] = &[
    ChannelSpec {
        id: "cli",
        label: "CLI/REPL",
        transport: "stdin",
        capabilities: "texto",
        credential_hint: "nenhuma",
        adapter: "pronto",
    },
    ChannelSpec {
        id: "webchat",
        label: "WebChat",
        transport: "gateway-local",
        capabilities: "texto",
        credential_hint: "nenhuma",
        adapter: "pronto",
    },
    ChannelSpec {
        id: "http-rest",
        label: "HTTP REST",
        transport: "http",
        capabilities: "texto,mídia",
        credential_hint: "nenhuma",
        adapter: "pronto",
    },
    ChannelSpec {
        id: "websocket",
        label: "WebSocket",
        transport: "websocket",
        capabilities: "texto,streaming",
        credential_hint: "nenhuma",
        adapter: "pronto",
    },
    ChannelSpec {
        id: "webhooks",
        label: "Webhooks",
        transport: "http",
        capabilities: "texto,eventos",
        credential_hint: "token por variável",
        adapter: "pronto",
    },
    ChannelSpec {
        id: "telegram",
        label: "Telegram",
        transport: "Bot API",
        capabilities: "texto",
        credential_hint: "SAPIENS_TELEGRAM_TOKEN",
        adapter: "pronto",
    },
    ChannelSpec {
        id: "discord",
        label: "Discord",
        transport: "incoming webhook",
        capabilities: "texto",
        credential_hint: "SAPIENS_DISCORD_WEBHOOK_URL",
        adapter: "pronto (webhook)",
    },
    ChannelSpec {
        id: "whatsapp",
        label: "WhatsApp Cloud API/bridge",
        transport: "Cloud API",
        capabilities: "texto,mídia,grupos,status",
        credential_hint: "SAPIENS_WHATSAPP_TOKEN",
        adapter: "pronto (Cloud API texto/mídia recebida-enviada/status)",
    },
    ChannelSpec {
        id: "slack",
        label: "Slack",
        transport: "incoming webhook",
        capabilities: "texto",
        credential_hint: "SAPIENS_SLACK_WEBHOOK_URL",
        adapter: "pronto (webhook)",
    },
    ChannelSpec {
        id: "signal",
        label: "Signal",
        transport: "signal-cli",
        capabilities: "texto,mídia,grupos",
        credential_hint: "SAPIENS_SIGNAL_ACCOUNT",
        adapter: "pronto (signal-cli texto/recebimento/mídia inbound-outbound)",
    },
    ChannelSpec {
        id: "matrix",
        label: "Matrix",
        transport: "Client-Server",
        capabilities: "texto,mídia,grupos,menções",
        credential_hint: "SAPIENS_MATRIX_TOKEN",
        adapter: "pronto (Client-Server texto/sync/mídia inbound-outbound)",
    },
    ChannelSpec {
        id: "google-chat",
        label: "Google Chat",
        transport: "webhook/API",
        capabilities: "texto,grupos",
        credential_hint: "SAPIENS_GOOGLE_CHAT_WEBHOOK_URL",
        adapter: "pronto (webhook)",
    },
    ChannelSpec {
        id: "teams",
        label: "Microsoft Teams",
        transport: "Bot Framework",
        capabilities: "texto",
        credential_hint: "SAPIENS_TEAMS_WEBHOOK_URL",
        adapter: "pronto (webhook)",
    },
    ChannelSpec {
        id: "imessage",
        label: "iMessage/macOS node",
        transport: "node",
        capabilities: "texto,mídia",
        credential_hint: "node macOS",
        adapter: "opcional",
    },
    ChannelSpec {
        id: "zalo",
        label: "Zalo",
        transport: "Bot API",
        capabilities: "texto,mídia",
        credential_hint: "token",
        adapter: "opcional",
    },
    ChannelSpec {
        id: "wechat",
        label: "WeChat/Weixin",
        transport: "bot/bridge",
        capabilities: "texto,mídia",
        credential_hint: "bridge externo",
        adapter: "opcional",
    },
    ChannelSpec {
        id: "qq",
        label: "QQ",
        transport: "bot/bridge",
        capabilities: "texto,mídia",
        credential_hint: "bridge externo",
        adapter: "opcional",
    },
    ChannelSpec {
        id: "dingtalk",
        label: "DingTalk",
        transport: "webhook/API",
        capabilities: "texto,mídia",
        credential_hint: "webhook/token",
        adapter: "opcional",
    },
    ChannelSpec {
        id: "feishu",
        label: "Feishu/Lark",
        transport: "webhook/API",
        capabilities: "texto,mídia",
        credential_hint: "app credentials",
        adapter: "opcional",
    },
    ChannelSpec {
        id: "wecom",
        label: "WeCom",
        transport: "webhook/API",
        capabilities: "texto,mídia",
        credential_hint: "webhook/token",
        adapter: "opcional",
    },
    ChannelSpec {
        id: "line",
        label: "LINE",
        transport: "Messaging API",
        capabilities: "texto,mídia",
        credential_hint: "SAPIENS_LINE_TOKEN",
        adapter: "opcional",
    },
    ChannelSpec {
        id: "vk",
        label: "VK",
        transport: "Bot API",
        capabilities: "texto,mídia",
        credential_hint: "token",
        adapter: "opcional",
    },
    ChannelSpec {
        id: "irc",
        label: "IRC",
        transport: "IRC",
        capabilities: "texto",
        credential_hint: "nenhuma/token",
        adapter: "opcional",
    },
    ChannelSpec {
        id: "onebot",
        label: "OneBot v11",
        transport: "HTTP/WebSocket",
        capabilities: "texto,mídia",
        credential_hint: "token",
        adapter: "opcional",
    },
    ChannelSpec {
        id: "delta-chat",
        label: "Delta Chat",
        transport: "IMAP/SMTP",
        capabilities: "texto,mídia",
        credential_hint: "conta de e-mail",
        adapter: "opcional",
    },
    ChannelSpec {
        id: "nostr",
        label: "Nostr",
        transport: "relay",
        capabilities: "texto",
        credential_hint: "chave no cofre",
        adapter: "opcional",
    },
    ChannelSpec {
        id: "twitch",
        label: "Twitch",
        transport: "IRC/EventSub",
        capabilities: "texto",
        credential_hint: "OAuth/token",
        adapter: "opcional",
    },
    ChannelSpec {
        id: "mattermost",
        label: "Mattermost",
        transport: "REST/WebSocket",
        capabilities: "texto,mídia",
        credential_hint: "token",
        adapter: "opcional",
    },
    ChannelSpec {
        id: "rocket-chat",
        label: "Rocket.Chat",
        transport: "REST/WebSocket",
        capabilities: "texto,mídia",
        credential_hint: "token",
        adapter: "opcional",
    },
    ChannelSpec {
        id: "discourse",
        label: "Discourse",
        transport: "webhook/API",
        capabilities: "texto",
        credential_hint: "API key",
        adapter: "opcional",
    },
    ChannelSpec {
        id: "email",
        label: "Email IMAP/SMTP",
        transport: "IMAP/SMTP",
        capabilities: "texto,mídia",
        credential_hint: "cofre/variáveis",
        adapter: "opcional",
    },
    ChannelSpec {
        id: "mqtt",
        label: "MQTT",
        transport: "MQTT",
        capabilities: "texto,eventos",
        credential_hint: "broker/token",
        adapter: "opcional",
    },
    ChannelSpec {
        id: "acp",
        label: "ACP/stdio",
        transport: "stdio",
        capabilities: "texto,tool-calls",
        credential_hint: "nenhuma",
        adapter: "opcional",
    },
    ChannelSpec {
        id: "mcp",
        label: "MCP",
        transport: "stdio/SSE",
        capabilities: "tools/resources",
        credential_hint: "allowlist",
        adapter: "opcional",
    },
    ChannelSpec {
        id: "android",
        label: "Android/Termux",
        transport: "node",
        capabilities: "texto,ações",
        credential_hint: "node pareado",
        adapter: "opcional",
    },
    ChannelSpec {
        id: "desktop",
        label: "Desktop companion",
        transport: "node",
        capabilities: "texto,ações,mídia",
        credential_hint: "node pareado",
        adapter: "opcional",
    },
    ChannelSpec {
        id: "hardware",
        label: "Hardware/camera",
        transport: "adapter",
        capabilities: "mídia,sensores",
        credential_hint: "permissão local",
        adapter: "opcional",
    },
];

pub fn catalog() -> &'static [ChannelSpec] {
    CATALOG
}

pub fn normalize_kind(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace(['_', ' '], "-")
}

pub fn find_spec(kind: &str) -> Result<&'static ChannelSpec> {
    let normalized = normalize_kind(kind);
    catalog()
        .iter()
        .find(|spec| spec.id == normalized)
        .ok_or_else(|| {
            anyhow::anyhow!("canal desconhecido: {kind}; use sapiens-agent channel list")
        })
}

pub fn is_local(kind: &str) -> bool {
    matches!(
        normalize_kind(kind).as_str(),
        "cli" | "webchat" | "http-rest" | "websocket"
    )
}

pub fn adapter_ready(kind: &str) -> bool {
    if normalize_kind(kind) == "signal" {
        return signal_cli_available();
    }
    matches!(
        normalize_kind(kind).as_str(),
        "cli"
            | "webchat"
            | "http-rest"
            | "websocket"
            | "webhooks"
            | "telegram"
            | "discord"
            | "slack"
            | "google-chat"
            | "teams"
            | "matrix"
            | "whatsapp"
    )
}

pub fn signal_cli_available() -> bool {
    StdCommand::new(signal_cli_command())
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub fn validate_config(config: &ChannelConfig) -> Result<()> {
    let spec = find_spec(&config.kind)?;
    if config.name.trim().is_empty() {
        bail!("nome do canal não pode ser vazio");
    }
    if config.enabled
        && (!is_local(spec.id) || spec.id == "webhooks")
        && config.credential_env.trim().is_empty()
    {
        bail!("canal {} exige credential_env para ser habilitado", spec.id);
    }
    if config.enabled && !is_local(spec.id) && config.allowlist.is_empty() {
        bail!("canal {} exige allowlist explícita", spec.id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex as StdMutex, OnceLock};

    static ENV_LOCK: OnceLock<StdMutex<()>> = OnceLock::new();

    #[test]
    fn catalog_contains_local_and_remote_entries() {
        assert!(
            catalog()
                .iter()
                .any(|item| item.id == "cli" && item.adapter == "pronto")
        );
        assert!(
            catalog()
                .iter()
                .any(|item| item.id == "telegram" && item.adapter == "pronto")
        );
    }

    #[test]
    fn kind_normalization_is_stable() {
        assert_eq!(normalize_kind(" Google_Chat "), "google-chat");
        assert!(find_spec("telegram").is_ok());
    }

    #[test]
    fn parses_matrix_incremental_sync_and_filters_non_text_events() {
        let payload = serde_json::json!({
            "next_batch": "s42",
            "rooms": {"join": {
                "!room:test": {"timeline": {"events": [
                    {"type":"m.room.message","event_id":"$text","sender":"@ana:test","origin_server_ts":1700000000000_u64,"content":{"msgtype":"m.text","body":"olá","m.mentions":{"user_ids":["@sapiens:test"]}}},
                    {"type":"m.room.message","event_id":"$notice","sender":"@ana:test","content":{"msgtype":"m.notice","body":"aviso"}},
                    {"type":"m.room.message","event_id":"$image","sender":"@ana:test","content":{"msgtype":"m.image","body":"imagem.png","url":"mxc://media.test/abc123","info":{"mimetype":"image/png","size":7}}},
                    {"type":"m.room.member","event_id":"$state","sender":"@ana:test","content":{}}
                ]}}
            }}
        });
        let batch = parse_matrix_sync(&payload).expect("sync batch");
        assert_eq!(batch.next_batch, "s42");
        assert_eq!(batch.messages.len(), 3);
        assert_eq!(batch.messages[0].event_id, "$text");
        assert_eq!(batch.messages[0].mentions, vec!["@sapiens:test"]);
        assert_eq!(batch.messages[2].msgtype, "m.image");
        assert_eq!(
            batch.messages[2].media_uri.as_deref(),
            Some("mxc://media.test/abc123")
        );
        assert_eq!(batch.messages[2].mime_type.as_deref(), Some("image/png"));
        assert_eq!(batch.messages[2].media_size, Some(7));
        let config = ChannelConfig {
            allowlist: vec!["!room:test".into()],
            ..Default::default()
        };
        assert!(matrix_inbound_allowed(&config, &batch.messages[0]));
        let sender_config = ChannelConfig {
            allowlist: vec!["@ana:test".into()],
            ..Default::default()
        };
        assert!(matrix_inbound_allowed(&sender_config, &batch.messages[1]));
    }

    #[test]
    fn rejects_matrix_sync_without_cursor() {
        assert!(parse_matrix_sync(&serde_json::json!({"rooms": {}})).is_err());
    }

    #[test]
    fn matrix_media_download_saves_bounded_authenticated_bytes() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let _guard = ENV_LOCK
            .get_or_init(|| StdMutex::new(()))
            .lock()
            .expect("env lock");
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request");
            let mut request = [0_u8; 16_384];
            let count = stream.read(&mut request).expect("read");
            let text = String::from_utf8_lossy(&request[..count]);
            assert!(text.starts_with(
                "GET /_matrix/client/v1/media/download/media.test/abc123?allow_remote=false"
            ));
            assert!(
                text.to_ascii_lowercase()
                    .contains("authorization: bearer test-token")
            );
            let body = "png-data";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).expect("write");
        });
        unsafe {
            std::env::set_var("SAPIENS_MATRIX_HOMESERVER", format!("http://{address}"));
            std::env::set_var("SAPIENS_MATRIX_TOKEN", "test-token");
        }
        let root =
            std::env::temp_dir().join(format!("sapiens-matrix-download-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("root");
        let config = ChannelConfig {
            name: "matrix".into(),
            kind: "matrix".into(),
            enabled: true,
            credential_env: "SAPIENS_MATRIX_TOKEN".into(),
            allowlist: vec!["!room:test".into()],
        };
        let policy = crate::policy::Policy {
            mode: "readonly".into(),
            workspace: root.clone(),
            allowed_domains: vec![],
            allow_private_networks: true,
        };
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let media = runtime
            .block_on(matrix_download_media(
                &config,
                "mxc://media.test/abc123",
                &root.join("inbound"),
                1024,
                Some("image/png"),
                Some(8),
                &policy,
            ))
            .expect("download");
        assert_eq!(media.size_bytes, 8);
        assert_eq!(std::fs::read(&media.path).expect("saved"), b"png-data");
        server.join().expect("server");
        let _ = std::fs::remove_dir_all(root);
        unsafe {
            std::env::remove_var("SAPIENS_MATRIX_HOMESERVER");
            std::env::remove_var("SAPIENS_MATRIX_TOKEN");
        }
    }

    #[test]
    fn remote_enabled_channel_requires_credential_reference() {
        let channel = ChannelConfig {
            name: "support".into(),
            kind: "telegram".into(),
            enabled: true,
            ..Default::default()
        };
        assert!(validate_config(&channel).is_err());
    }

    #[test]
    fn telegram_recipient_requires_explicit_allowlist() {
        let channel = ChannelConfig {
            name: "telegram".into(),
            kind: "telegram".into(),
            enabled: true,
            credential_env: "SAPIENS_TELEGRAM_TOKEN".into(),
            allowlist: vec!["123".into()],
        };
        assert!(telegram_recipient_allowed(&channel, "123"));
        assert!(!telegram_recipient_allowed(&channel, "456"));
    }

    #[test]
    fn signal_arguments_cannot_be_interpreted_as_options() {
        assert!(validate_signal_argument("+5511999999999", "recipient").is_ok());
        assert!(validate_signal_argument("group-id", "recipient").is_ok());
        assert!(validate_signal_argument("--help", "recipient").is_err());
        assert!(validate_signal_argument("bad\nvalue", "recipient").is_err());
    }

    #[test]
    fn parses_signal_json_envelopes_and_allowlists_group_or_sender() {
        let output = r#"{"envelope":{"source":"+5511999999999","sourceDevice":2,"timestamp":1700000000000,"dataMessage":{"timestamp":1700000000000,"message":"oi","attachments":[{"id":"att-1","filename":"foto.jpg","contentType":"image/jpeg","size":12}],"groupInfo":{"groupId":"R1"}}}}"#;
        let output = format!(
            "{output}\n{{\"envelope\":{{\"source\":\"+5511999999999\",\"timestamp\":1700000000001,\"dataMessage\":{{\"attachments\":[{{\"id\":42,\"contentType\":\"image/png\"}}]}}}}}}"
        );
        let messages = parse_signal_receive_output(&output).expect("signal messages");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].sender, "+5511999999999");
        assert_eq!(messages[0].group_id.as_deref(), Some("R1"));
        assert_eq!(messages[0].attachments, vec!["att-1"]);
        assert_eq!(
            messages[0].attachment_details[0].filename.as_deref(),
            Some("foto.jpg")
        );
        assert_eq!(messages[1].attachments, vec!["42"]);
        assert!(signal_inbound_allowed(
            &ChannelConfig {
                allowlist: vec!["R1".into()],
                ..Default::default()
            },
            &messages[0]
        ));
        assert!(!signal_inbound_allowed(
            &ChannelConfig {
                allowlist: vec!["+5500000000000".into()],
                ..Default::default()
            },
            &messages[0]
        ));
    }

    #[test]
    fn signal_media_args_keep_attachment_inside_direct_or_group_target() {
        let direct = signal_media_args(
            "+5511000000000",
            None,
            Some("+5511999999999"),
            std::path::Path::new("workspace/photo.png"),
            Some("foto"),
        )
        .expect("direct args");
        assert!(
            direct
                .windows(2)
                .any(|pair| pair[0] == "--attachment" && pair[1] == "workspace/photo.png")
        );
        assert!(
            direct
                .windows(2)
                .any(|pair| pair[0] == "-m" && pair[1] == "foto")
        );
        let group = signal_media_args(
            "+5511000000000",
            Some("GROUP-ID"),
            None,
            std::path::Path::new("photo.png"),
            None,
        )
        .expect("group args");
        assert!(
            group
                .windows(2)
                .any(|pair| pair[0] == "-g" && pair[1] == "GROUP-ID")
        );
        assert!(
            signal_media_args(
                "+5511000000000",
                Some("--bad"),
                None,
                std::path::Path::new("photo.png"),
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn decodes_signal_attachment_base64_with_whitespace() {
        assert_eq!(decode_base64("aG\nVsbG8=").expect("base64"), b"hello");
        assert!(decode_base64("not base64!").is_err());
    }

    #[test]
    fn webhook_adapters_use_expected_payloads_and_allowlist() {
        assert!(is_webhook_adapter("Discord"));
        assert_eq!(
            webhook_payload("discord", "hello"),
            json!({"content": "hello"})
        );
        assert_eq!(webhook_payload("slack", "hello"), json!({"text": "hello"}));
        let channel = ChannelConfig {
            kind: "teams".into(),
            allowlist: vec!["room-1".into()],
            ..Default::default()
        };
        assert!(webhook_recipient_allowed(&channel, "room-1"));
        assert!(!webhook_recipient_allowed(&channel, "room-2"));
    }

    #[test]
    fn matrix_text_adapter_checks_whoami_and_sends_allowlisted_room() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let _guard = ENV_LOCK
            .get_or_init(|| StdMutex::new(()))
            .lock()
            .expect("env lock");
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            for index in 0..2 {
                let (mut stream, _) = listener.accept().expect("request");
                let mut request = [0_u8; 16_384];
                let count = stream.read(&mut request).expect("read");
                let text = String::from_utf8_lossy(&request[..count]);
                assert!(
                    text.to_ascii_lowercase()
                        .contains("authorization: bearer test-token")
                );
                let (status, body) = if index == 0 {
                    assert!(text.starts_with("GET /_matrix/client/v3/account/whoami HTTP/1.1"));
                    ("200 OK", r#"{"user_id":"@sapiens:test"}"#)
                } else {
                    assert!(text.starts_with(
                        "PUT /_matrix/client/v3/rooms/!room:test/send/m.room.message/"
                    ));
                    assert!(text.contains(r#""msgtype":"m.text""#));
                    ("200 OK", r#"{"event_id":"$event"}"#)
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("write");
            }
        });
        unsafe {
            std::env::set_var("SAPIENS_MATRIX_HOMESERVER", format!("http://{address}"));
            std::env::set_var("SAPIENS_MATRIX_TOKEN", "test-token");
        }
        let config = ChannelConfig {
            name: "matrix".into(),
            kind: "matrix".into(),
            enabled: true,
            credential_env: "SAPIENS_MATRIX_TOKEN".into(),
            allowlist: vec!["!room:test".into()],
        };
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            matrix_test(&config).await.expect("whoami");
            matrix_send(&config, "!room:test", "hello")
                .await
                .expect("send");
        });
        assert!(
            runtime
                .block_on(matrix_send(&config, "!other:test", "blocked"))
                .is_err()
        );
        server.join().expect("server");
        unsafe {
            std::env::remove_var("SAPIENS_MATRIX_HOMESERVER");
            std::env::remove_var("SAPIENS_MATRIX_TOKEN");
        }
    }

    #[test]
    fn matrix_send_retries_transient_http_failure() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let _guard = ENV_LOCK
            .get_or_init(|| StdMutex::new(()))
            .lock()
            .expect("env lock");
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            for attempt in 0..2 {
                let (mut stream, _) = listener.accept().expect("request");
                let mut request = [0_u8; 16_384];
                let count = stream.read(&mut request).expect("read");
                let text = String::from_utf8_lossy(&request[..count]);
                assert!(
                    text.starts_with(
                        "PUT /_matrix/client/v3/rooms/!room:test/send/m.room.message/"
                    )
                );
                let (status, body) = if attempt == 0 {
                    ("503 Service Unavailable", "{}")
                } else {
                    ("200 OK", r#"{"event_id":"$retry"}"#)
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("write");
            }
        });
        unsafe {
            std::env::set_var("SAPIENS_MATRIX_HOMESERVER", format!("http://{address}"));
            std::env::set_var("SAPIENS_MATRIX_TOKEN", "test-token");
        }
        let config = ChannelConfig {
            name: "matrix".into(),
            kind: "matrix".into(),
            enabled: true,
            credential_env: "SAPIENS_MATRIX_TOKEN".into(),
            allowlist: vec!["!room:test".into()],
        };
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime
            .block_on(matrix_send(&config, "!room:test", "retry me"))
            .expect("retry send");
        server.join().expect("server");
        unsafe {
            std::env::remove_var("SAPIENS_MATRIX_HOMESERVER");
            std::env::remove_var("SAPIENS_MATRIX_TOKEN");
        }
    }

    #[test]
    fn matrix_sync_calls_incremental_endpoint_and_parses_cursor() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let _guard = ENV_LOCK
            .get_or_init(|| StdMutex::new(()))
            .lock()
            .expect("env lock");
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request");
            let mut request = [0_u8; 16_384];
            let count = stream.read(&mut request).expect("read");
            let text = String::from_utf8_lossy(&request[..count]);
            assert!(text.starts_with("GET /_matrix/client/v3/sync?"));
            assert!(text.contains("since=cursor-1"));
            assert!(
                text.to_ascii_lowercase()
                    .contains("authorization: bearer test-token")
            );
            let body = serde_json::json!({
                "next_batch": "cursor-2",
                "rooms": {"join": {"!room:test": {"timeline": {"events": [{
                    "type": "m.room.message",
                    "event_id": "$event",
                    "sender": "@ana:test",
                    "content": {"msgtype": "m.text", "body": "oi"}
                }]}}}}
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("write");
        });
        unsafe {
            std::env::set_var("SAPIENS_MATRIX_HOMESERVER", format!("http://{address}"));
            std::env::set_var("SAPIENS_MATRIX_TOKEN", "test-token");
        }
        let config = ChannelConfig {
            name: "matrix".into(),
            kind: "matrix".into(),
            enabled: true,
            credential_env: "SAPIENS_MATRIX_TOKEN".into(),
            allowlist: vec!["!room:test".into()],
        };
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let batch = runtime
            .block_on(matrix_sync(&config, Some("cursor-1"), 1))
            .expect("sync");
        assert_eq!(batch.next_batch, "cursor-2");
        assert_eq!(batch.messages[0].room_id, "!room:test");
        server.join().expect("server");
        unsafe {
            std::env::remove_var("SAPIENS_MATRIX_HOMESERVER");
            std::env::remove_var("SAPIENS_MATRIX_TOKEN");
        }
    }

    #[test]
    fn matrix_media_send_uploads_and_publishes_mxc_event() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let _guard = ENV_LOCK
            .get_or_init(|| StdMutex::new(()))
            .lock()
            .expect("env lock");
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            for index in 0..2 {
                let (mut stream, _) = listener.accept().expect("request");
                let mut request = [0_u8; 32_768];
                let count = stream.read(&mut request).expect("read");
                let text = String::from_utf8_lossy(&request[..count]);
                let body =
                    if index == 0 {
                        assert!(text.starts_with(
                            "POST /_matrix/media/v3/upload?filename=photo.png HTTP/1.1"
                        ));
                        r#"{"content_uri":"mxc://test/media-1"}"#
                    } else {
                        assert!(text.starts_with(
                            "PUT /_matrix/client/v3/rooms/!room:test/send/m.room.message/"
                        ));
                        assert!(text.contains(r#""msgtype":"m.image""#));
                        assert!(text.contains(r#""url":"mxc://test/media-1""#));
                        r#"{"event_id":"$media-event"}"#
                    };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("write");
            }
        });
        unsafe {
            std::env::set_var("SAPIENS_MATRIX_HOMESERVER", format!("http://{address}"));
            std::env::set_var("SAPIENS_MATRIX_TOKEN", "test-token");
        }
        let root =
            std::env::temp_dir().join(format!("sapiens-matrix-media-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("root");
        let media_path = root.join("photo.png");
        std::fs::write(&media_path, b"png-bytes").expect("media");
        let config = ChannelConfig {
            name: "matrix".into(),
            kind: "matrix".into(),
            enabled: true,
            credential_env: "SAPIENS_MATRIX_TOKEN".into(),
            allowlist: vec!["!room:test".into()],
        };
        let policy = crate::policy::Policy {
            mode: "trusted".into(),
            workspace: root.clone(),
            allowed_domains: vec![],
            allow_private_networks: true,
        };
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime
            .block_on(matrix_send_media(
                &config,
                "!room:test",
                &media_path,
                Some("foto"),
                &policy,
            ))
            .expect("send media");
        server.join().expect("server");
        let _ = std::fs::remove_dir_all(&root);
        unsafe {
            std::env::remove_var("SAPIENS_MATRIX_HOMESERVER");
            std::env::remove_var("SAPIENS_MATRIX_TOKEN");
        }
    }

    #[test]
    fn whatsapp_cloud_adapter_checks_phone_and_sends_allowlisted_text() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let _guard = ENV_LOCK
            .get_or_init(|| StdMutex::new(()))
            .lock()
            .expect("env lock");
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            for index in 0..2 {
                let (mut stream, _) = listener.accept().expect("request");
                let mut request = [0_u8; 16_384];
                let count = stream.read(&mut request).expect("read");
                let text = String::from_utf8_lossy(&request[..count]);
                assert!(
                    text.to_ascii_lowercase()
                        .contains("authorization: bearer test-token")
                );
                if index == 0 {
                    assert!(text.starts_with("GET /123?fields=id%2Cdisplay_phone_number HTTP/1.1"));
                } else {
                    assert!(text.starts_with("POST /123/messages HTTP/1.1"));
                    assert!(text.contains("messaging_product"));
                }
                let body = if index == 0 {
                    r#"{"id":"123","display_phone_number":"+5500000000000"}"#
                } else {
                    r#"{"messages":[{"id":"wamid.test"}]}"#
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("write");
            }
        });
        unsafe {
            std::env::set_var(
                "SAPIENS_WHATSAPP_GRAPH_BASE_URL",
                format!("http://{address}"),
            );
            std::env::set_var("SAPIENS_WHATSAPP_TOKEN", "test-token");
            std::env::set_var("SAPIENS_WHATSAPP_PHONE_NUMBER_ID", "123");
        }
        let config = ChannelConfig {
            name: "whatsapp".into(),
            kind: "whatsapp".into(),
            enabled: true,
            credential_env: "SAPIENS_WHATSAPP_TOKEN".into(),
            allowlist: vec!["5500000000000".into()],
        };
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            whatsapp_test(&config).await.expect("phone check");
            whatsapp_send(&config, "5500000000000", "hello")
                .await
                .expect("send");
        });
        assert!(
            runtime
                .block_on(whatsapp_send(&config, "5500000000001", "blocked"))
                .is_err()
        );
        server.join().expect("server");
        unsafe {
            std::env::remove_var("SAPIENS_WHATSAPP_GRAPH_BASE_URL");
            std::env::remove_var("SAPIENS_WHATSAPP_TOKEN");
            std::env::remove_var("SAPIENS_WHATSAPP_PHONE_NUMBER_ID");
        }
    }

    #[test]
    fn whatsapp_media_download_checks_size_hash_and_policy() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let _guard = ENV_LOCK
            .get_or_init(|| StdMutex::new(()))
            .lock()
            .expect("env lock");
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let bytes = b"verified-media".to_vec();
        let good_hash = hex_encode(&Sha256::digest(&bytes));
        let server_bytes = bytes.clone();
        let server = std::thread::spawn(move || {
            for index in 0..4 {
                let (mut stream, _) = listener.accept().expect("request");
                let mut request = [0_u8; 16_384];
                let count = stream.read(&mut request).expect("read");
                let text = String::from_utf8_lossy(&request[..count]);
                let body = if index % 2 == 0 {
                    let expected_hash = if index == 0 {
                        good_hash.as_str()
                    } else {
                        "0000000000000000000000000000000000000000000000000000000000000000"
                    };
                    format!(
                        "{{\"url\":\"http://{address}/download\",\"mime_type\":\"image/png\",\"file_size\":{},\"sha256\":\"{}\"}}",
                        server_bytes.len(),
                        expected_hash
                    )
                } else {
                    String::from("verified-media")
                };
                let content_type = if index % 2 == 0 {
                    "application/json"
                } else {
                    "image/png"
                };
                if index == 0 || index == 2 {
                    assert!(text.starts_with("GET /media-1?phone_number_id=123 HTTP/1.1"));
                } else {
                    assert!(text.starts_with("GET /download HTTP/1.1"));
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("write");
            }
        });
        unsafe {
            std::env::set_var(
                "SAPIENS_WHATSAPP_GRAPH_BASE_URL",
                format!("http://{address}"),
            );
            std::env::set_var("SAPIENS_WHATSAPP_TOKEN", "test-token");
            std::env::set_var("SAPIENS_WHATSAPP_PHONE_NUMBER_ID", "123");
        }
        let root =
            std::env::temp_dir().join(format!("sapiens-whatsapp-media-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("root");
        let config = ChannelConfig {
            name: "whatsapp".into(),
            kind: "whatsapp".into(),
            enabled: true,
            credential_env: "SAPIENS_WHATSAPP_TOKEN".into(),
            allowlist: vec!["5511999999999".into()],
        };
        let policy = crate::policy::Policy {
            mode: "readonly".into(),
            workspace: root.clone(),
            allowed_domains: vec![],
            allow_private_networks: true,
        };
        let output_dir = root.join("media");
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let media = runtime
            .block_on(whatsapp_download_media(
                &config,
                "media-1",
                &output_dir,
                1024,
                &policy,
            ))
            .expect("download");
        assert_eq!(media.mime_type, "image/png");
        assert_eq!(media.size_bytes, bytes.len() as u64);
        assert!(std::path::Path::new(&media.path).exists());
        assert!(
            runtime
                .block_on(whatsapp_download_media(
                    &config,
                    "media-1",
                    &output_dir,
                    1024,
                    &policy,
                ))
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&root);
        server.join().expect("server");
        unsafe {
            std::env::remove_var("SAPIENS_WHATSAPP_GRAPH_BASE_URL");
            std::env::remove_var("SAPIENS_WHATSAPP_TOKEN");
            std::env::remove_var("SAPIENS_WHATSAPP_PHONE_NUMBER_ID");
        }
    }

    #[test]
    fn whatsapp_media_send_uploads_inside_workspace_and_sends_media_id() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let _guard = ENV_LOCK
            .get_or_init(|| StdMutex::new(()))
            .lock()
            .expect("env lock");
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            for index in 0..2 {
                let (mut stream, _) = listener.accept().expect("request");
                let mut request = [0_u8; 32_768];
                let count = stream.read(&mut request).expect("read");
                let text = String::from_utf8_lossy(&request[..count]);
                let body = if index == 0 {
                    assert!(text.starts_with("POST /123/media HTTP/1.1"));
                    r#"{"id":"media-uploaded"}"#
                } else {
                    assert!(text.starts_with("POST /123/messages HTTP/1.1"));
                    assert!(text.contains(r#""type":"image""#));
                    assert!(text.contains(r#""id":"media-uploaded""#));
                    r#"{"messages":[{"id":"wamid-media"}]}"#
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("write");
            }
        });
        unsafe {
            std::env::set_var(
                "SAPIENS_WHATSAPP_GRAPH_BASE_URL",
                format!("http://{address}"),
            );
            std::env::set_var("SAPIENS_WHATSAPP_TOKEN", "test-token");
            std::env::set_var("SAPIENS_WHATSAPP_PHONE_NUMBER_ID", "123");
        }
        let root =
            std::env::temp_dir().join(format!("sapiens-whatsapp-send-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("root");
        let media_path = root.join("photo.png");
        std::fs::write(&media_path, b"png-bytes").expect("media");
        let config = ChannelConfig {
            name: "whatsapp".into(),
            kind: "whatsapp".into(),
            enabled: true,
            credential_env: "SAPIENS_WHATSAPP_TOKEN".into(),
            allowlist: vec!["5511999999999".into()],
        };
        let policy = crate::policy::Policy {
            mode: "trusted".into(),
            workspace: root.clone(),
            allowed_domains: vec![],
            allow_private_networks: true,
        };
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime
            .block_on(whatsapp_send_media(
                &config,
                "5511999999999",
                &media_path,
                Some("foto"),
                &policy,
            ))
            .expect("send media");
        server.join().expect("server");
        let _ = std::fs::remove_dir_all(&root);
        unsafe {
            std::env::remove_var("SAPIENS_WHATSAPP_GRAPH_BASE_URL");
            std::env::remove_var("SAPIENS_WHATSAPP_TOKEN");
            std::env::remove_var("SAPIENS_WHATSAPP_PHONE_NUMBER_ID");
        }
    }
}
