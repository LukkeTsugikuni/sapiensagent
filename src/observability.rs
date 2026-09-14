use anyhow::Result;
use serde::Serialize;
use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn init() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
}
pub fn redact(value: &str) -> String {
    let mut redacted = value.to_string();
    for marker in [
        "Bearer ",
        "bearer ",
        "Authorization: Bearer ",
        "authorization: Bearer ",
        "api_key=",
        "api-key=",
        "token=",
        "password=",
        "secret=",
        "api_key:",
        "api-key:",
        "token:",
        "password:",
        "secret:",
        "OPENAI_API_KEY=",
        "ANTHROPIC_API_KEY=",
        "GEMINI_API_KEY=",
        "SAPIENS_API_KEY=",
        "xoxb-",
        "xoxp-",
        "ghp_",
        "github_pat_",
        "\"api_key\":\"",
        "\"token\":\"",
        "\"password\":\"",
        "\"secret\":\"",
    ] {
        redacted = redact_after_marker(&redacted, marker);
    }
    redacted
}

fn redact_after_marker(value: &str, marker: &str) -> String {
    let mut result = value.to_string();
    let mut cursor = 0;
    while cursor < result.len() {
        let Some(relative) = result[cursor..].find(marker) else {
            break;
        };
        let start = cursor + relative + marker.len();
        let mut end = start;
        while end < result.len() {
            let character = result[end..].chars().next().unwrap_or_default();
            if character.is_whitespace()
                || matches!(character, '"' | '\'' | ',' | '}' | ']' | ')' | ';')
            {
                break;
            }
            end += character.len_utf8();
        }
        if end == start {
            cursor = start;
            continue;
        }
        result.replace_range(start..end, "[REDACTED]");
        cursor = start + "[REDACTED]".len();
    }
    result
}

#[derive(Debug, Serialize)]
pub struct Receipt<'a> {
    pub timestamp: u64,
    pub action: &'a str,
    pub risk: &'a str,
    pub approved: bool,
    pub outcome: String,
    pub session: Option<&'a str>,
}

pub fn receipts_path(config_path: &Path) -> PathBuf {
    config_path.with_file_name("sapiens-agent-receipts.jsonl")
}

pub fn append_receipt(
    config_path: &Path,
    action: &str,
    risk: &str,
    approved: bool,
    outcome: &str,
    session: Option<&str>,
) -> Result<()> {
    let path = receipts_path(config_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let receipt = Receipt {
        timestamp: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        action,
        risk,
        approved,
        outcome: redact(outcome),
        session,
    };
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", serde_json::to_string(&receipt)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_common_provider_and_authorization_tokens() {
        let value = redact(
            "Authorization: Bearer abc123 OPENAI_API_KEY=sk-test token:xyz xoxb-secret ghp_value",
        );
        assert!(!value.contains("abc123"));
        assert!(!value.contains("sk-test"));
        assert!(!value.contains("xyz"));
        assert!(!value.contains("xoxb-secret"));
        assert!(!value.contains("ghp_value"));
    }
}
