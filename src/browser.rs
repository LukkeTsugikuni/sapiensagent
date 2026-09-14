use crate::policy::Policy;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct BrowserTab {
    pub id: String,
    pub url: String,
    pub title: String,
}

pub trait BrowserDriver: Send + Sync {
    fn open(&self, url: &str, policy: &Policy) -> Result<BrowserTab>;
    fn snapshot(&self, tab_id: &str) -> Result<String>;
}

#[derive(Debug, Clone)]
pub struct PlaywrightCliBrowser {
    session: String,
    timeout: Duration,
    persistent: bool,
}

impl PlaywrightCliBrowser {
    pub fn new(session: impl Into<String>) -> Result<Self> {
        let session = session.into();
        if session.trim().is_empty()
            || session
                .chars()
                .any(|character| !(character.is_ascii_alphanumeric() || "-_.".contains(character)))
        {
            bail!("browser session must contain only letters, numbers, '-' '_' or '.'");
        }
        Ok(Self {
            session,
            timeout: Duration::from_secs(60),
            persistent: false,
        })
    }

    pub fn with_persistent_profile(mut self, enabled: bool) -> Self {
        self.persistent = enabled;
        self
    }

    pub fn session_name(&self) -> &str {
        &self.session
    }

    pub fn goto(&self, url: &str, policy: &Policy) -> Result<BrowserTab> {
        self.check_navigation(url, policy)?;
        self.invoke(&["goto".into(), url.into()])?;
        self.current_tab(policy)
    }

    pub fn click(&self, target: &str) -> Result<String> {
        self.invoke(&["click".into(), target.into()])
    }

    pub fn double_click(&self, target: &str) -> Result<String> {
        self.invoke(&["dblclick".into(), target.into()])
    }

    pub fn drag(&self, start_target: &str, end_target: &str) -> Result<String> {
        self.invoke(&["drag".into(), start_target.into(), end_target.into()])
    }

    pub fn check(&self, target: &str) -> Result<String> {
        self.invoke(&["check".into(), target.into()])
    }

    pub fn uncheck(&self, target: &str) -> Result<String> {
        self.invoke(&["uncheck".into(), target.into()])
    }

    pub fn fill(&self, target: &str, text: &str) -> Result<String> {
        self.invoke(&["fill".into(), target.into(), text.into()])
    }

    pub fn hover(&self, target: &str) -> Result<String> {
        self.invoke(&["hover".into(), target.into()])
    }

    pub fn press(&self, target: &str, key: &str) -> Result<String> {
        self.invoke(&["press".into(), target.into(), key.into()])
    }

    pub fn scroll(&self, dx: i32, dy: i32) -> Result<String> {
        self.invoke(&["mousewheel".into(), dx.to_string(), dy.to_string()])
    }

    pub fn go_back(&self) -> Result<String> {
        self.invoke(&["go-back".into()])
    }

    pub fn go_forward(&self) -> Result<String> {
        self.invoke(&["go-forward".into()])
    }

    pub fn reload(&self) -> Result<String> {
        self.invoke(&["reload".into()])
    }

    pub fn dialog_accept(&self, prompt: Option<&str>) -> Result<String> {
        let mut args = vec!["dialog-accept".into()];
        if let Some(prompt) = prompt {
            args.push(prompt.into());
        }
        self.invoke(&args)
    }

    pub fn dialog_dismiss(&self) -> Result<String> {
        self.invoke(&["dialog-dismiss".into()])
    }

    pub fn resize(&self, width: u32, height: u32) -> Result<String> {
        validate_viewport(width, height)?;
        self.invoke(&["resize".into(), width.to_string(), height.to_string()])
    }

    pub fn select(&self, target: &str, value: &str) -> Result<String> {
        self.invoke(&["select".into(), target.into(), value.into()])
    }

    pub fn wait(&self, millis: u64) -> Result<String> {
        if millis > 60_000 {
            bail!("browser wait cannot exceed 60000 milliseconds");
        }
        self.invoke(&[
            "eval".into(),
            format!(
                "async () => {{ await new Promise(resolve => setTimeout(resolve, {millis})); return {{waited_ms: {millis}}}; }}"
            ),
        ])
    }

    pub fn extract(&self, target: Option<&str>) -> Result<String> {
        let mut args = vec!["eval".into()];
        if target.is_some() {
            args.push("element => element.innerText".into());
            args.push(target.unwrap_or_default().into());
        } else {
            args.push("() => document.body.innerText".into());
        }
        self.invoke(&args)
    }

    pub fn upload(&self, files: &[String], policy: &Policy) -> Result<String> {
        if files.is_empty() {
            bail!("browser upload requires at least one file");
        }
        let mut args = vec!["upload".into()];
        for file in files {
            policy.check_path(std::path::Path::new(file))?;
            args.push(file.clone());
        }
        self.invoke(&args)
    }

    pub fn download(&self, target: &str, output: &Path, policy: &Policy) -> Result<String> {
        if target.trim().is_empty() {
            bail!("browser download requires a CSS selector");
        }
        policy.check_path(output)?;
        if let Some(parent) = output.parent()
            && !parent.exists()
        {
            bail!("browser download destination parent must exist");
        }
        let target = serde_json::to_string(target).context("encode download selector")?;
        let output = serde_json::to_string(&output.to_string_lossy())
            .context("encode download destination")?;
        let script = format!(
            "async () => {{ const download = await page.waitForEvent('download', {{ timeout: 30000 }}); await page.locator({target}).first().click(); await download.saveAs({output}); return {{suggested_filename: download.suggestedFilename(), path: {output}}}; }}"
        );
        self.invoke(&["run-code".into(), script])
    }

    pub fn trace_start(&self) -> Result<String> {
        self.invoke(&["tracing-start".into()])
    }

    pub fn trace_stop(&self) -> Result<String> {
        self.invoke(&["tracing-stop".into()])
    }

    pub fn state_save(&self, output: &Path, policy: &Policy) -> Result<String> {
        check_browser_output_path(output, policy)?;
        self.invoke(&["state-save".into(), output.to_string_lossy().into_owned()])
    }

    pub fn state_load(&self, input: &Path, policy: &Policy) -> Result<String> {
        policy.check_path(input)?;
        if !input.is_file() {
            bail!("browser storage state file does not exist")
        }
        self.invoke(&["state-load".into(), input.to_string_lossy().into_owned()])
    }

    pub fn screenshot(&self, target: Option<&str>) -> Result<String> {
        let mut args = vec!["screenshot".into()];
        if let Some(target) = target {
            args.push(target.into());
        }
        self.invoke(&args)
    }

    pub fn tabs(&self) -> Result<String> {
        self.invoke(&["tab-list".into()])
    }

    pub fn tab_new(&self, url: &str, policy: &Policy) -> Result<BrowserTab> {
        self.check_navigation(url, policy)?;
        self.invoke(&["tab-new".into(), url.into()])?;
        self.current_tab(policy)
    }

    pub fn tab_select(&self, index: usize, policy: &Policy) -> Result<BrowserTab> {
        self.invoke(&["tab-select".into(), index.to_string()])?;
        self.current_tab(policy)
    }

    pub fn tab_close(&self, index: usize) -> Result<String> {
        self.invoke(&["tab-close".into(), index.to_string()])
    }

    pub fn close(&self) -> Result<String> {
        self.invoke(&["close".into()])
    }

    pub fn delete_data(&self) -> Result<String> {
        self.invoke(&["delete-data".into()])
    }

    fn current_tab(&self, policy: &Policy) -> Result<BrowserTab> {
        let output = self.invoke(&[
            "eval".into(),
            "() => ({url: location.href, title: document.title})".into(),
        ])?;
        let result = cli_result(&output)?;
        let url = result
            .get("url")
            .and_then(Value::as_str)
            .context("Playwright did not return the current URL")?
            .to_string();
        self.check_navigation(&url, policy)?;
        Ok(BrowserTab {
            id: self.session.clone(),
            url,
            title: result
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
    }

    fn check_navigation(&self, url: &str, policy: &Policy) -> Result<()> {
        let scheme = url
            .split_once("://")
            .map(|(scheme, _)| scheme.to_ascii_lowercase())
            .unwrap_or_default();
        if scheme != "http" && scheme != "https" {
            bail!("browser accepts only http or https URLs");
        }
        policy.check_url(url)
    }

    fn invoke(&self, command_args: &[String]) -> Result<String> {
        let custom = std::env::var_os("SAPIENS_PLAYWRIGHT_COMMAND");
        let executable = custom
            .as_deref()
            .unwrap_or_else(|| std::ffi::OsStr::new(if cfg!(windows) { "npx.cmd" } else { "npx" }));
        let mut command = Command::new(executable);
        if custom.is_none() {
            command
                .arg("--yes")
                .arg("--package")
                .arg("@playwright/cli")
                .arg("playwright-cli");
        }
        let mut args = command_args.to_vec();
        if self.persistent && args.first().is_some_and(|command| command == "open") {
            args.push("--persistent".into());
        }
        let output = command
            .arg(format!("-s={}", self.session))
            .args(args)
            .arg("--json")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "Playwright CLI unavailable; install Node.js and @playwright/cli or set SAPIENS_PLAYWRIGHT_COMMAND (executable {})",
                    executable.to_string_lossy()
                )
            })?;
        let started = Instant::now();
        let mut child = output;
        loop {
            if child.try_wait()?.is_some() {
                break;
            }
            if started.elapsed() >= self.timeout {
                let _ = child.kill();
                bail!(
                    "Playwright command timed out after {} seconds",
                    self.timeout.as_secs()
                );
            }
            thread::sleep(Duration::from_millis(25));
        }
        let output = child.wait_with_output()?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        if !output.status.success() {
            let stderr = crate::observability::redact(&String::from_utf8_lossy(&output.stderr));
            bail!(
                "Playwright command failed ({}): {}",
                output.status,
                stderr.trim()
            );
        }
        Ok(stdout)
    }
}

fn check_browser_output_path(output: &Path, policy: &Policy) -> Result<()> {
    policy.check_path(output)?;
    if let Some(parent) = output.parent()
        && !parent.exists()
    {
        bail!("browser output destination parent must exist")
    }
    Ok(())
}

fn validate_viewport(width: u32, height: u32) -> Result<()> {
    if !(320..=7680).contains(&width) || !(240..=4320).contains(&height) {
        bail!("browser viewport must be between 320x240 and 7680x4320")
    }
    Ok(())
}

impl BrowserDriver for PlaywrightCliBrowser {
    fn open(&self, url: &str, policy: &Policy) -> Result<BrowserTab> {
        self.check_navigation(url, policy)?;
        self.invoke(&["open".into(), url.into()])?;
        self.current_tab(policy)
    }

    fn snapshot(&self, tab_id: &str) -> Result<String> {
        if tab_id != self.session {
            bail!("unknown browser session: {tab_id}");
        }
        self.invoke(&["snapshot".into()])
    }
}

pub struct OptionalBrowser;
impl BrowserDriver for OptionalBrowser {
    fn open(&self, url: &str, policy: &Policy) -> Result<BrowserTab> {
        policy.check_url(url)?;
        anyhow::bail!("browser adapter disabled; enable the Playwright adapter explicitly")
    }

    fn snapshot(&self, _tab_id: &str) -> Result<String> {
        anyhow::bail!("browser adapter disabled")
    }
}

fn cli_result(output: &str) -> Result<Value> {
    let envelope: Value = serde_json::from_str(output).context("invalid Playwright JSON output")?;
    let result = envelope
        .get("result")
        .context("Playwright output did not contain result")?;
    if let Some(text) = result.as_str() {
        return serde_json::from_str(text).context("invalid Playwright result JSON");
    }
    Ok(result.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn browser_rejects_private_navigation_before_spawning() {
        let browser = PlaywrightCliBrowser::new("test-session").expect("browser");
        let policy = Policy::supervised(PathBuf::from("."));
        assert!(browser.open("http://127.0.0.1:8787", &policy).is_err());
    }

    #[test]
    fn parses_playwright_eval_result() {
        let result =
            cli_result(r#"{"result":"{\"url\":\"https://example.com/\",\"title\":\"Example\"}"}"#)
                .expect("result");
        assert_eq!(result["url"], "https://example.com/");
        assert_eq!(result["title"], "Example");
    }

    #[test]
    fn download_rejects_a_destination_outside_workspace_before_spawning() {
        let browser = PlaywrightCliBrowser::new("test-session").expect("browser");
        let policy = Policy::supervised(PathBuf::from("."));
        assert!(
            browser
                .download("a", Path::new("C:\\outside\\download.bin"), &policy)
                .is_err()
        );
    }

    #[test]
    fn advanced_browser_actions_validate_without_spawning() {
        assert!(validate_viewport(319, 800).is_err());
        assert!(validate_viewport(1280, 239).is_err());
        assert!(validate_viewport(1280, 720).is_ok());
    }
}
