//! 真实环境命令执行。进程先进入 Job/进程组，再通过 stdin 投递命令；不承诺文件系统隔离。
use crate::runtime::AgentCancel;
use serde::Serialize;
use std::{path::{Path, PathBuf}, process::Stdio, sync::{Arc, atomic::{AtomicUsize, Ordering}}, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

const OUTPUT_LIMIT: usize = 1024 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecResult {
    pub exit_code: Option<i32>, pub stdout: String, pub stderr: String,
    pub timed_out: bool, pub cancelled: bool, pub truncated: bool, pub shell: String,
}
impl ExecResult { pub fn success(&self) -> bool { self.exit_code == Some(0) && !self.timed_out && !self.cancelled } }

/** 仅选择真实可用的 Shell；不加载用户交互配置。 */
pub fn actual_shell() -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        if let Some(paths) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&paths) {
                let path = dir.join("pwsh.exe"); if path.is_file() { return Ok(path); }
            }
        }
        let root = std::env::var_os("SystemRoot").ok_or("SystemRoot 未设置")?;
        let path = PathBuf::from(root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
        if path.is_file() { return Ok(path); }
    }
    #[cfg(unix)]
    for path in ["/bin/bash", "/usr/bin/bash", "/bin/sh"] { if Path::new(path).is_file() { return Ok(path.into()); } }
    Err("未找到可用的非交互 Shell。".into())
}

fn sensitive_environment(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    ["TOKEN", "SECRET", "PASSWORD", "PASSWD", "API_KEY", "APIKEY", "PRIVATE_KEY", "CREDENTIAL", "AUTHORIZATION"].iter().any(|word| name.contains(word))
        || matches!(name.as_str(), "BASH_ENV" | "ENV" | "SHELLOPTS" | "BASHOPTS" | "NODE_OPTIONS" | "PYTHONSTARTUP")
        || name.starts_with("ORANGE_") || name.starts_with("CODEX_")
}

/** 共享字节预算；超额后仍持续排空管道，防止子进程卡在输出写入。 */
async fn drain(mut pipe: impl AsyncRead + Unpin, used: Arc<AtomicUsize>) -> Vec<u8> {
    let mut output = Vec::new(); let mut buffer = [0u8; 8192];
    while let Ok(count) = pipe.read(&mut buffer).await {
        if count == 0 { break; }
        let previous = used.fetch_add(count, Ordering::Relaxed);
        let keep = count.min(OUTPUT_LIMIT.saturating_sub(previous));
        output.extend_from_slice(&buffer[..keep]);
    }
    output
}

/** 当前用户执行，不接用户终端、不注入 Keyring、不使用提权或后台驻留模式。 */
pub async fn execute(command: &str, cwd: &Path, timeout: u64, cancel: &AgentCancel) -> Result<ExecResult, String> {
    if command.trim().is_empty() || command.contains('\0') { return Err("command 不能为空或包含 NUL。".into()); }
    if !cwd.is_dir() { return Err("命令工作目录缺失或失效；请选定有效知识库或 cwd。".into()); }
    if !(5..=1800).contains(&timeout) { return Err("timeoutSeconds 必须在 5–1800 秒之间。".into()); }
    if cancel.is_aborted() { return Err("回合已取消。".into()); }
    let shell = actual_shell()?;
    let mut process = tokio::process::Command::new(&shell);
    #[cfg(windows)]
    { process.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", "-"]); process.creation_flags(0x08000000); }
    #[cfg(unix)]
    {
        if shell.file_name().is_some_and(|name| name == "bash") { process.args(["--noprofile", "--norc"]); }
        process.arg("-s"); process.process_group(0);
    }
    process.current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    for (key, _) in std::env::vars_os() { if sensitive_environment(&key.to_string_lossy()) { process.env_remove(key); } }
    let mut child = process.spawn().map_err(|e| format!("启动 Shell 失败：{e}"))?;
    let pid = child.id().ok_or("进程没有 PID")?;
    let group = ManagedGroup::attach(pid)?;
    let used = Arc::new(AtomicUsize::new(0));
    let stdout = tokio::spawn(drain(child.stdout.take().ok_or("缺少 stdout")?, used.clone()));
    let stderr = tokio::spawn(drain(child.stderr.take().ok_or("缺少 stderr")?, used.clone()));
    // 只有进程管理成功后，Shell 才收到实际命令。
    let mut input = child.stdin.take().ok_or("缺少 stdin")?;
    #[cfg(windows)]
    let script = {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(command.as_bytes());
        format!("[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false); $OutputEncoding=[Console]::OutputEncoding; $ErrorActionPreference='Stop'; $global:LASTEXITCODE=0; try {{ & ([scriptblock]::Create([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{encoded}')))); exit $LASTEXITCODE }} catch {{ [Console]::Error.WriteLine($_.ToString()); exit 1 }}\r\n")
    };
    #[cfg(unix)]
    let script = format!("{command}\n");
    input.write_all(script.as_bytes()).await.map_err(|e| format!("投递命令失败：{e}"))?;
    drop(input);
    let (status, timed_out, cancelled) = tokio::select! {
        result = child.wait() => (Some(result.map_err(|e| e.to_string())?), false, false),
        _ = tokio::time::sleep(Duration::from_secs(timeout)) => (None, true, false),
        _ = cancel.cancelled() => (None, false, true),
    };
    // 即使命令正常退出也清理剩余后代；命令不能偷偷留下后台进程。
    drop(group);
    if status.is_none() { let _ = child.kill().await; let _ = child.wait().await; }
    let stdout = stdout.await.map_err(|e| e.to_string())?;
    let stderr = stderr.await.map_err(|e| e.to_string())?;
    Ok(ExecResult { exit_code: status.and_then(|s| s.code()), stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(), timed_out, cancelled,
        truncated: used.load(Ordering::Relaxed) > OUTPUT_LIMIT, shell: shell.display().to_string() })
}

#[cfg(windows)]
struct ManagedGroup(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
unsafe impl Send for ManagedGroup {}
#[cfg(windows)]
impl ManagedGroup {
    fn attach(pid: u32) -> Result<Self, String> {
        use windows_sys::Win32::{Foundation::CloseHandle, System::{JobObjects::*, Threading::*}};
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() { return Err(format!("创建 Job Object 失败：{}", std::io::Error::last_os_error())); }
            let guard = Self(job);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as *const _, std::mem::size_of_val(&info) as u32) == 0 { return Err("无法配置 Job Object；命令未投递。".into()); }
            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if process.is_null() { return Err("无法管理 Shell 进程；命令未投递。".into()); }
            let attached = AssignProcessToJobObject(job, process); CloseHandle(process);
            if attached == 0 { return Err("Shell 无法加入 Job Object；命令未投递。".into()); }
            Ok(guard)
        }
    }
}
#[cfg(windows)]
impl Drop for ManagedGroup { fn drop(&mut self) { unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0); } } }
#[cfg(unix)]
struct ManagedGroup(u32);
#[cfg(unix)]
impl ManagedGroup { fn attach(pid: u32) -> Result<Self, String> { Ok(Self(pid)) } }
#[cfg(unix)]
impl Drop for ManagedGroup { fn drop(&mut self) { unsafe { libc::kill(-(self.0 as i32), libc::SIGKILL); } } }

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn command_reports_success_failure_and_output_truncation() {
        let dir = tempfile::tempdir().unwrap(); let cancel = AgentCancel::testing_token();
        let ok = execute("echo orange", dir.path(), 5, &cancel).await.unwrap();
        assert!(ok.success(), "{ok:?}"); assert!(ok.stdout.contains("orange"));
        let failed = execute("exit 7", dir.path(), 5, &cancel).await.unwrap();
        assert_eq!(failed.exit_code, Some(7));
        assert!(!failed.success());
        #[cfg(windows)] let noisy = "[Console]::Out.Write('x' * 1100000)";
        #[cfg(unix)] let noisy = "head -c 1100000 /dev/zero";
        let result = execute(noisy, dir.path(), 5, &cancel).await.unwrap();
        assert!(result.truncated); assert!(result.stdout.len() <= OUTPUT_LIMIT);
    }
    #[tokio::test]
    async fn command_cancellation_and_timeout_report_distinct_results() {
        let dir = tempfile::tempdir().unwrap(); let cancel = AgentCancel::testing_token();
        #[cfg(windows)] let slow = "Start-Sleep -Seconds 30";
        #[cfg(unix)] let slow = "sleep 30";
        let trigger = cancel.clone(); tokio::spawn(async move { tokio::time::sleep(Duration::from_millis(200)).await; trigger.abort(); });
        assert!(execute(slow, dir.path(), 5, &cancel).await.unwrap().cancelled);
        assert!(execute(slow, dir.path(), 5, &AgentCancel::testing_token()).await.unwrap().timed_out);
    }

    /** 用真实后代进程验证 Job/进程组回收，覆盖正常退出、超时和停止。 */
    #[tokio::test]
    async fn descendants_are_stopped_after_success_timeout_and_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let spawn = "$child = Start-Process -WindowStyle Hidden -FilePath (Get-Process -Id $PID).Path -ArgumentList '-NoProfile','-NonInteractive','-Command','Start-Sleep -Seconds 30' -PassThru; [Console]::Out.WriteLine($child.Id)";
        #[cfg(unix)]
        let spawn = "sleep 30 & echo $!";
        #[cfg(windows)]
        let slow = "Start-Sleep -Seconds 30";
        #[cfg(unix)]
        let slow = "sleep 30";
        for mode in ["success", "timeout", "cancel"] {
            let cancel = AgentCancel::testing_token();
            let command = if mode == "success" { spawn.to_owned() } else { format!("{spawn}\n{slow}") };
            if mode == "cancel" {
                let trigger = cancel.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    trigger.abort();
                });
            }
            let result = execute(&command, dir.path(), 5, &cancel).await.unwrap();
            assert_eq!(result.timed_out, mode == "timeout", "{result:?}");
            assert_eq!(result.cancelled, mode == "cancel", "{result:?}");
            let pid: u32 = result.stdout.trim().parse().expect("命令须先返回子进程 PID");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            while process_is_running(pid) && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            assert!(!process_is_running(pid), "{mode} 后遗留子进程 {pid}");
        }
    }

    #[cfg(windows)]
    fn process_is_running(pid: u32) -> bool {
        use windows_sys::Win32::{Foundation::{CloseHandle, WAIT_TIMEOUT}, System::Threading::*};
        unsafe {
            let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if process.is_null() { return false; }
            let running = WaitForSingleObject(process, 0) == WAIT_TIMEOUT;
            CloseHandle(process);
            running
        }
    }

    #[cfg(unix)]
    fn process_is_running(pid: u32) -> bool {
        // Linux 的孤儿进程可能等待 init 回收；僵尸已经不能执行，不算残留运行进程。
        #[cfg(target_os = "linux")]
        if std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            stat.rsplit_once(") ").is_some_and(|(_, rest)| rest.starts_with('Z'))
        }) { return false; }
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }

    #[tokio::test]
    async fn commands_preserve_unicode_cwd_and_separate_stdout_stderr_without_shell_state() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("空 格"); std::fs::create_dir(&cwd).unwrap();
        #[cfg(windows)]
        let command = "$env:ORANGE_EXEC_STATE_TEST='old'; [Console]::Out.WriteLine('橘记✓'); [Console]::Out.WriteLine((Get-Location).Path); [Console]::Error.WriteLine('stderr-test')";
        #[cfg(unix)]
        let command = "export ORANGE_EXEC_STATE_TEST=old; printf '橘记✓\\n'; pwd; printf stderr-test >&2";
        let result = execute(command, &cwd, 5, &AgentCancel::testing_token()).await.unwrap();
        assert!(result.success(), "{result:?}");
        assert!(result.stdout.contains("橘记✓")); assert!(result.stdout.contains("空 格"));
        assert!(result.stderr.contains("stderr-test")); assert!(!result.stdout.contains("stderr-test"));
        #[cfg(windows)]
        let check = "if (Test-Path Env:ORANGE_EXEC_STATE_TEST) { throw 'state leaked' }";
        #[cfg(unix)]
        let check = "test -z \"${ORANGE_EXEC_STATE_TEST+x}\"";
        assert!(execute(check, &cwd, 5, &AgentCancel::testing_token()).await.unwrap().success());
    }

    #[test]
    fn secret_and_startup_environment_is_filtered_without_removing_runtime_paths() {
        for name in ["OPENAI_API_KEY", "GITHUB_TOKEN", "password", "AWS_SECRET_ACCESS_KEY", "BASH_ENV", "ENV", "NODE_OPTIONS", "ORANGE_SETTINGS", "CODEX_HOME"] {
            assert!(sensitive_environment(name), "{name}");
        }
        for name in ["PATH", "SystemRoot", "TEMP", "LANG", "HOME"] {
            assert!(!sensitive_environment(name), "{name}");
        }
    }
}
