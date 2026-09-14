use crate::policy::Policy;
use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

const MAX_OUTPUT_BYTES: usize = 1_048_576;

#[derive(Debug, Clone, Copy)]
pub struct ShellLimits {
    pub max_memory_mb: u32,
    pub max_processes: u32,
    pub max_cpu_secs: u32,
}

impl Default for ShellLimits {
    fn default() -> Self {
        Self {
            max_memory_mb: 512,
            max_processes: 16,
            max_cpu_secs: 60,
        }
    }
}

pub async fn run(command: &str, cwd: &Path, policy: &Policy) -> Result<String> {
    run_with_timeout(command, cwd, policy, Duration::from_secs(60)).await
}

pub async fn run_with_timeout(
    command: &str,
    cwd: &Path,
    policy: &Policy,
    limit: Duration,
) -> Result<String> {
    run_with_options(command, cwd, policy, limit, &[], MAX_OUTPUT_BYTES).await
}

pub async fn run_with_options(
    command: &str,
    cwd: &Path,
    policy: &Policy,
    limit: Duration,
    allowlist: &[String],
    max_output_bytes: usize,
) -> Result<String> {
    run_with_limits(
        command,
        cwd,
        policy,
        limit,
        allowlist,
        max_output_bytes,
        ShellLimits::default(),
    )
    .await
}

pub async fn run_with_limits(
    command: &str,
    cwd: &Path,
    policy: &Policy,
    limit: Duration,
    allowlist: &[String],
    max_output_bytes: usize,
    limits: ShellLimits,
) -> Result<String> {
    policy.check_path(cwd)?;
    if !allowlist.is_empty() {
        let executable = command.split_whitespace().next().unwrap_or_default();
        let executable = Path::new(executable)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(executable)
            .to_ascii_lowercase();
        if !allowlist
            .iter()
            .any(|allowed| allowed.trim().eq_ignore_ascii_case(&executable))
        {
            bail!("command executable is not in the shell allowlist");
        }
    }
    let lowered = command.to_ascii_lowercase();
    for denied in [
        "format",
        "del ",
        "erase ",
        "rd ",
        "rmdir",
        "rm ",
        "remove-item",
        "rm -rf",
        "git reset --hard",
        "git clean",
        "shutdown",
        "curl ",
        "wget ",
        "invoke-webrequest",
        "invoke-restmethod",
        "bitsadmin",
        "certutil -urlcache",
    ] {
        if lowered.contains(denied) {
            bail!("command blocked by default policy")
        }
    }
    let mut process = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-lc", command]);
        c
    };
    let child = process
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    #[cfg(windows)]
    let _job_guard = apply_windows_limits(&child, limits)?;
    let output = timeout(limit, child.wait_with_output())
        .await
        .map_err(|_| anyhow::anyhow!("command timed out after {}s", limit.as_secs()))??;
    if output.stdout.len().saturating_add(output.stderr.len()) > max_output_bytes {
        bail!("command output exceeded {} bytes", max_output_bytes);
    }
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    if !output.status.success() {
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        bail!("command failed: {text}");
    }
    Ok(text)
}

#[cfg(windows)]
struct WindowsJobGuard(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for WindowsJobGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
fn apply_windows_limits(
    child: &tokio::process::Child,
    limits: ShellLimits,
) -> Result<WindowsJobGuard> {
    use std::mem::size_of;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
        JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_JOB_TIME,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() {
        bail!("could not create Windows Job Object for shell limits");
    }
    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    info.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_ACTIVE_PROCESS | JOB_OBJECT_LIMIT_JOB_MEMORY | JOB_OBJECT_LIMIT_JOB_TIME;
    info.BasicLimitInformation.ActiveProcessLimit = limits.max_processes.max(1);
    info.JobMemoryLimit = usize::try_from(limits.max_memory_mb)
        .unwrap_or(usize::MAX / (1024 * 1024))
        .saturating_mul(1024 * 1024);
    info.BasicLimitInformation.PerJobUserTimeLimit =
        i64::from(limits.max_cpu_secs.max(1)) * 10_000_000;
    let configured = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&mut info as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if configured == 0 {
        unsafe {
            let _ = windows_sys::Win32::Foundation::CloseHandle(job);
        }
        bail!("could not configure Windows Job Object shell limits");
    }
    let process_handle = child
        .raw_handle()
        .context("shell process handle unavailable")?;
    if unsafe { AssignProcessToJobObject(job, process_handle as _) } == 0 {
        unsafe {
            let _ = windows_sys::Win32::Foundation::CloseHandle(job);
        }
        bail!("could not assign shell process to Windows Job Object");
    }
    Ok(WindowsJobGuard(job))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn destructive_commands_are_rejected_before_execution() {
        let policy = Policy::supervised(PathBuf::from("."));
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let result = runtime.block_on(run("rmdir /s workspace", Path::new("."), &policy));
        assert!(result.is_err());
    }

    #[test]
    fn network_commands_are_denied_by_default() {
        let policy = Policy::supervised(PathBuf::from("."));
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let result = runtime.block_on(run("curl https://example.com", Path::new("."), &policy));
        assert!(result.unwrap_err().to_string().contains("blocked"));
    }

    #[test]
    fn configurable_allowlist_rejects_unlisted_executable() {
        let policy = Policy::supervised(PathBuf::from("."));
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let result = runtime.block_on(run_with_options(
            "echo allowed",
            Path::new("."),
            &policy,
            Duration::from_secs(5),
            &["dir".into()],
            MAX_OUTPUT_BYTES,
        ));
        assert!(result.unwrap_err().to_string().contains("allowlist"));
    }

    #[test]
    fn bounded_process_runs_with_configured_os_limits() {
        let policy = Policy::supervised(PathBuf::from("."));
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let result = runtime.block_on(run_with_limits(
            if cfg!(windows) {
                "echo bounded"
            } else {
                "printf bounded"
            },
            Path::new("."),
            &policy,
            Duration::from_secs(5),
            &[],
            MAX_OUTPUT_BYTES,
            ShellLimits {
                max_memory_mb: 128,
                max_processes: 4,
                max_cpu_secs: 5,
            },
        ));
        assert!(result.expect("bounded command").contains("bounded"));
    }
}
