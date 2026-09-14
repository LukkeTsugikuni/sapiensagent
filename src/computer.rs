use crate::policy::{Policy, Risk};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    process::{Command, Stdio},
    thread,
    time::Duration,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DesktopAction {
    Click { x: u32, y: u32 },
    Type { text: String },
    Keypress { key: String },
    Wait { millis: u64 },
    Screenshot,
}

pub fn emergency_stop_path(config_path: &std::path::Path) -> std::path::PathBuf {
    config_path.with_file_name("sapiens-agent-computer-stop")
}

pub trait ComputerUseAdapter: Send + Sync {
    fn screenshot(&self) -> Result<Vec<u8>>;
    fn execute(&self, action: DesktopAction, policy: &Policy) -> Result<()>;
}

pub fn requires_confirmation(action: &DesktopAction) -> Risk {
    match action {
        DesktopAction::Click { .. } => Risk::ExternalWrite,
        DesktopAction::Type { .. } | DesktopAction::Keypress { .. } => Risk::SecretInput,
        DesktopAction::Wait { .. } | DesktopAction::Screenshot => Risk::Read,
    }
}

pub fn validate_action(action: &DesktopAction) -> Result<()> {
    match action {
        DesktopAction::Click { x, y } => {
            if *x > 16_384 || *y > 16_384 {
                bail!("computer click coordinates exceed the safe desktop boundary");
            }
        }
        DesktopAction::Type { text } => {
            if text.chars().count() > 4_096 {
                bail!("computer type text cannot exceed 4096 characters");
            }
            if text
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
            {
                bail!("computer type text contains an unsupported control character");
            }
        }
        DesktopAction::Keypress { key } => {
            let _ = normalize_key(key)?;
        }
        DesktopAction::Wait { millis } => {
            if *millis > 60_000 {
                bail!("computer wait cannot exceed 60000 milliseconds");
            }
        }
        DesktopAction::Screenshot => {}
    }
    Ok(())
}

pub struct WindowsComputerUse;

impl WindowsComputerUse {
    pub fn new() -> Result<Self> {
        if !cfg!(windows) {
            bail!("computer use adapter is currently supported only on Windows");
        }
        Ok(Self)
    }
}

impl ComputerUseAdapter for WindowsComputerUse {
    fn screenshot(&self) -> Result<Vec<u8>> {
        let output =
            std::env::temp_dir().join(format!("sapiens-agent-screen-{}.png", std::process::id()));
        let output_literal = powershell_single_quote(&output.to_string_lossy());
        let script = format!(
            r#"
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
$bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$bitmap = New-Object System.Drawing.Bitmap $bounds.Width, $bounds.Height
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
$bitmap.Save('{output_literal}', [System.Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose()
$bitmap.Dispose()
"#
        );
        run_powershell(&script, Duration::from_secs(30))?;
        let bytes = std::fs::read(&output).context("computer screenshot was not written")?;
        let _ = std::fs::remove_file(&output);
        Ok(bytes)
    }

    fn execute(&self, action: DesktopAction, policy: &Policy) -> Result<()> {
        validate_action(&action)?;
        let risk = requires_confirmation(&action);
        if policy.requires_approval(risk) {
            bail!("computer action requires approval: {risk:?}");
        }
        match action {
            DesktopAction::Click { x, y } => {
                let script = format!(
                    r#"
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class SapiensMouse {{
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
}}
"@
[SapiensMouse]::SetCursorPos({x}, {y})
[SapiensMouse]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
[SapiensMouse]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
"#
                );
                run_powershell(&script, Duration::from_secs(10)).map(|_| ())
            }
            DesktopAction::Type { text } => {
                let escaped = powershell_single_quote(&text);
                let script = format!(
                    r#"
Add-Type -AssemblyName System.Windows.Forms
[System.Windows.Forms.SendKeys]::SendWait('{escaped}')
"#
                );
                run_powershell(&script, Duration::from_secs(10)).map(|_| ())
            }
            DesktopAction::Keypress { key } => {
                let send_keys = normalize_key(&key)?;
                let script = format!(
                    r#"
Add-Type -AssemblyName System.Windows.Forms
[System.Windows.Forms.SendKeys]::SendWait('{send_keys}')
"#
                );
                run_powershell(&script, Duration::from_secs(10)).map(|_| ())
            }
            DesktopAction::Wait { millis } => {
                thread::sleep(Duration::from_millis(millis));
                Ok(())
            }
            DesktopAction::Screenshot => {
                let _ = self.screenshot()?;
                Ok(())
            }
        }
    }
}

fn run_powershell(script: &str, timeout: Duration) -> Result<String> {
    let encoded = base64_utf16(script);
    let mut child = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &encoded,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("PowerShell is required for the Windows computer-use adapter")?;
    let start = std::time::Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            bail!("computer-use PowerShell action timed out");
        }
        thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!(
            "computer-use action failed: {}",
            crate::observability::redact(&String::from_utf8_lossy(&output.stderr)).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn base64_utf16(value: &str) -> String {
    let bytes = value
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    base64_encode(&bytes)
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::new();
    for chunk in bytes.chunks(3) {
        let first = chunk[0] as u32;
        let second = chunk.get(1).copied().unwrap_or_default() as u32;
        let third = chunk.get(2).copied().unwrap_or_default() as u32;
        let value = (first << 16) | (second << 8) | third;
        output.push(ALPHABET[((value >> 18) & 63) as usize] as char);
        output.push(ALPHABET[((value >> 12) & 63) as usize] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}

fn powershell_single_quote(value: &str) -> String {
    value.replace('\'', "''")
}

fn normalize_key(value: &str) -> Result<String> {
    let normalized = value.trim().to_ascii_uppercase();
    let result = match normalized.as_str() {
        "ENTER" => "{ENTER}",
        "TAB" => "{TAB}",
        "ESC" | "ESCAPE" => "{ESC}",
        "SPACE" => " ",
        "UP" => "{UP}",
        "DOWN" => "{DOWN}",
        "LEFT" => "{LEFT}",
        "RIGHT" => "{RIGHT}",
        "CTRL+C" => "^C",
        "CTRL+V" => "^V",
        "CTRL+A" => "^A",
        _ => bail!("keypress is not in the safe allowlist"),
    };
    Ok(result.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_actions_do_not_require_confirmation() {
        assert_eq!(
            requires_confirmation(&DesktopAction::Screenshot),
            Risk::Read
        );
        assert_eq!(
            requires_confirmation(&DesktopAction::Wait { millis: 1 }),
            Risk::Read
        );
    }

    #[test]
    fn keypresses_are_allowlisted() {
        assert_eq!(normalize_key("enter").expect("key"), "{ENTER}");
        assert!(normalize_key("ALT+F4").is_err());
    }

    #[test]
    fn structured_actions_have_safe_bounds_before_execution() {
        assert!(
            validate_action(&DesktopAction::Click {
                x: 16_384,
                y: 16_384
            })
            .is_ok()
        );
        assert!(validate_action(&DesktopAction::Click { x: 16_385, y: 1 }).is_err());
        assert!(validate_action(&DesktopAction::Wait { millis: 60_001 }).is_err());
        assert!(
            validate_action(&DesktopAction::Type {
                text: "\u{0000}".into()
            })
            .is_err()
        );
        assert!(
            validate_action(&DesktopAction::Keypress {
                key: "ALT+F4".into()
            })
            .is_err()
        );
    }

    #[test]
    fn powershell_paths_escape_single_quotes() {
        assert_eq!(
            powershell_single_quote("C:\\work\\it's.png"),
            "C:\\work\\it''s.png"
        );
    }

    #[test]
    fn structured_actions_round_trip_without_secret_logging() {
        let action: DesktopAction =
            serde_json::from_str(r#"{"type":"keypress","key":"ENTER"}"#).expect("action");
        assert!(matches!(action, DesktopAction::Keypress { key } if key == "ENTER"));
        let encoded = serde_json::to_string(&DesktopAction::Type {
            text: "private text".into(),
        })
        .expect("json");
        assert!(encoded.contains("private text"));
    }
}
