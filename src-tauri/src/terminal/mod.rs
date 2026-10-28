//! 用户交互终端独立于 Agent 工具；输入与输出只走专用 IPC Channel，不进入日志或模型。

// Job Object 是同一份系统资源策略：终端和浏览器进程树都靠它兜底回收。
pub(crate) mod process_guard;

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::{collections::HashMap, io::{Read, Write}, path::{Path, PathBuf}, sync::{Arc, Mutex, OnceLock}, thread::JoinHandle};
use process_guard::ProcessGuard;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalDescriptor { pub terminal_id: String, pub shell: String, pub cwd: String }

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TerminalEvent { Output { data: Vec<u8> }, Exit { code: Option<u32> }, Error { message: String } }

type EventSink = Arc<dyn Fn(TerminalEvent) + Send + Sync>;

/** PTY 主端由会话持有，视图隐藏不会释放它；等待线程负责先排空输出再报告退出。 */
struct TerminalSession {
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    guard: ProcessGuard,
    waiter: Mutex<Option<JoinHandle<()>>>,
}

fn sessions() -> &'static Mutex<HashMap<String, Arc<TerminalSession>>> {
    static SESSIONS: OnceLock<Mutex<HashMap<String, Arc<TerminalSession>>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn find(id: &str) -> Result<Arc<TerminalSession>, String> {
    sessions().lock().map_err(|_| "终端状态不可用。")?.get(id).cloned().ok_or_else(|| "终端已关闭。".into())
}

/** 仅从环境 PATH 选择真实可执行文件，不依赖 Codex 或开发机特定安装路径。 */
fn executable_in_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| std::env::split_paths(&path).map(|dir| dir.join(name)).find(|path| path.is_file()))
}

fn default_shell() -> Result<PathBuf, String> {
    #[cfg(windows)]
    { executable_in_path("pwsh.exe").or_else(|| executable_in_path("powershell.exe")).ok_or_else(|| "未找到 PowerShell，请检查系统 PATH。".into()) }
    #[cfg(unix)]
    { Ok(std::env::var_os("SHELL").map(PathBuf::from).filter(|path| path.is_absolute() && path.is_file()).unwrap_or_else(|| PathBuf::from("/bin/sh"))) }
}

fn size(cols: u16, rows: u16) -> Result<PtySize, String> {
    if cols == 0 || rows == 0 || cols > 1000 || rows > 1000 { return Err("终端尺寸无效。".into()); }
    Ok(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
}

/**
 * Windows 的 canonicalize 返回 `\\?\` 前缀的 verbatim 路径。把它直接作为 Shell 的起始目录，
 * PowerShell 的提示符会退化成 `PS Microsoft.PowerShell.Core\FileSystem::\\?\D:\...>`，
 * 工具栏也会显示这串前缀。这里只把盘符路径和 UNC 路径还原成普通写法；卷 GUID 等
 * 无法简化的路径保持原样，避免拼出无效路径。
 *
 * 普通写法受 MAX_PATH 限制，超过 259 字符时继续用 verbatim 路径，否则 CreateProcess 会失败。
 */
fn display_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        let simplified = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            Some(format!(r"\\{rest}"))
        } else {
            text.strip_prefix(r"\\?\").and_then(|rest| {
                let mut chars = rest.chars();
                let drive = chars.next();
                let colon = chars.next();
                let separator = chars.next();
                (drive.is_some_and(|drive| drive.is_ascii_alphabetic()) && colon == Some(':') && separator == Some('\\')).then(|| rest.to_owned())
            })
        };
        if let Some(simplified) = simplified.filter(|simplified| simplified.len() < 260) {
            return PathBuf::from(simplified);
        }
    }
    path
}

/** 创建完成前绑定进程树；失败必须回收已经启动的 Shell。 */
fn spawn(cwd: &Path, mut command: CommandBuilder, shell: String, cols: u16, rows: u16, sink: EventSink) -> Result<TerminalDescriptor, String> {
    let cwd = display_path(cwd.canonicalize().map_err(|_| "终端起始目录不存在。")?);
    if !cwd.is_dir() { return Err("终端起始位置不是目录。".into()); }
    command.cwd(&cwd);
    command.env("TERM", "xterm-256color");
    let pair = native_pty_system().openpty(size(cols, rows)?).map_err(|error| format!("创建 PTY 失败：{error}"))?;
    let mut reader = pair.master.try_clone_reader().map_err(|error| error.to_string())?;
    let writer = pair.master.take_writer().map_err(|error| error.to_string())?;
    let mut child = pair.slave.spawn_command(command).map_err(|error| format!("启动 Shell 失败：{error}"))?;
    drop(pair.slave);
    let guard = match child.process_id().ok_or_else(|| "无法获取 Shell 进程。".to_owned()).and_then(ProcessGuard::attach) {
        Ok(guard) => guard,
        Err(error) => { let _ = child.kill(); let _ = child.wait(); return Err(error); }
    };
    let id = uuid::Uuid::new_v4().to_string();
    let session = Arc::new(TerminalSession { master: Mutex::new(Some(pair.master)), writer: Mutex::new(Some(writer)), guard, waiter: Mutex::new(None) });
    let output_sink = sink.clone();
    let reader_thread = std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => output_sink(TerminalEvent::Output { data: buffer[..count].to_vec() }),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                // PTY 关闭时部分 Unix 实现返回 EIO，等同于输出结束。
                Err(_) => break,
            }
        }
    });
    let owner = session.clone();
    let waiter = std::thread::spawn(move || {
        let status = child.wait();
        // 父 Shell 自然退出也清理剩余子进程，否则继承 PTY 的后台程序会阻止 EOF。
        owner.guard.terminate();
        // ConPTY 的输出 EOF 依赖主端释放；先释放它再等待读线程，避免关闭时互相等待。
        if let Ok(mut writer) = owner.writer.lock() { writer.take(); }
        if let Ok(mut master) = owner.master.lock() { master.take(); }
        let _ = reader_thread.join();
        match status {
            Ok(status) => sink(TerminalEvent::Exit { code: Some(status.exit_code()) }),
            Err(_) => sink(TerminalEvent::Error { message: "等待终端退出失败。".into() }),
        }
    });
    *session.waiter.lock().map_err(|_| "终端状态不可用。")? = Some(waiter);
    sessions().lock().map_err(|_| "终端状态不可用。")?.insert(id.clone(), session);
    Ok(TerminalDescriptor { terminal_id: id, shell, cwd: cwd.to_string_lossy().into_owned() })
}

pub fn create(app: &tauri::AppHandle, knowledge_base_id: &str, cols: u16, rows: u16, channel: tauri::ipc::Channel<TerminalEvent>) -> Result<TerminalDescriptor, String> {
    let snapshot = crate::storage::load_workspace_snapshot(app)?;
    let knowledge_base = snapshot.knowledge_bases.iter().find(|item| item.id == knowledge_base_id).ok_or("知识库不存在。")?;
    let shell = default_shell()?;
    let mut command = CommandBuilder::new(&shell);
    #[cfg(windows)]
    { command.args(["-NoLogo", "-NoProfile"]); }
    #[cfg(unix)]
    { command.arg("-i"); }
    spawn(Path::new(&knowledge_base.path), command, shell.file_name().unwrap_or_default().to_string_lossy().into_owned(), cols, rows, Arc::new(move |event| { let _ = channel.send(event); }))
}

pub fn input(id: &str, data: &str) -> Result<(), String> {
    if data.len() > 1024 * 1024 { return Err("单次终端输入过大。".into()); }
    let session = find(id)?;
    let mut writer = session.writer.lock().map_err(|_| "终端输入不可用。")?;
    let writer = writer.as_mut().ok_or("终端已退出。")?;
    writer.write_all(data.as_bytes()).and_then(|_| writer.flush()).map_err(|_| "终端输入失败，进程可能已退出。".into())
}

pub fn resize(id: &str, cols: u16, rows: u16) -> Result<(), String> {
    find(id)?.master.lock().map_err(|_| "终端状态不可用。")?.as_ref().ok_or("终端已退出。")?.resize(size(cols, rows)?).map_err(|_| "调整终端尺寸失败。".into())
}

pub fn close(id: &str) -> Result<(), String> {
    let session = sessions().lock().map_err(|_| "终端状态不可用。")?.remove(id);
    if let Some(session) = session {
        session.guard.terminate();
        if let Some(waiter) = session.waiter.lock().map_err(|_| "终端状态不可用。")?.take() { let _ = waiter.join(); }
    }
    Ok(())
}

pub fn shutdown() {
    let ids = sessions().lock().map(|sessions| sessions.keys().cloned().collect::<Vec<_>>()).unwrap_or_default();
    for id in ids { let _ = close(&id); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_size_and_missing_directory() {
        assert!(size(0, 24).is_err());
        assert!(size(80, 0).is_err());
        assert!(spawn(Path::new("/orange-definitely-missing-directory"), CommandBuilder::new("missing"), "missing".into(), 80, 24, Arc::new(|_| {})).is_err());
    }
    #[test]
    fn shell_cwd_never_exposes_a_verbatim_prefix() {
        let plain = if cfg!(windows) { PathBuf::from(r"D:\document\orange") } else { PathBuf::from("/tmp/orange") };
        assert_eq!(display_path(plain.clone()), plain);
        #[cfg(windows)]
        {
            assert_eq!(display_path(PathBuf::from(r"\\?\D:\document\橘记")), PathBuf::from(r"D:\document\橘记"));
            assert_eq!(display_path(PathBuf::from(r"\\?\UNC\server\share")), PathBuf::from(r"\\server\share"));
            // 卷 GUID 路径无法简化，必须原样保留，否则会指向不存在的盘符。
            assert_eq!(display_path(PathBuf::from(r"\\?\Volume{1}\doc")), PathBuf::from(r"\\?\Volume{1}\doc"));
            // 超过 MAX_PATH 时保留 verbatim 前缀，否则 CreateProcess 找不到目录。
            let long = format!(r"\\?\D:\{}", "a".repeat(300));
            assert_eq!(display_path(PathBuf::from(&long)), PathBuf::from(&long));
        }
    }
    #[test]
    fn interactive_terminal_preserves_output_and_can_be_closed() {
        let dir = tempfile::tempdir().unwrap();
        let shell = default_shell().unwrap();
        let mut command = CommandBuilder::new(shell);
        #[cfg(windows)] command.args(["-NoLogo", "-NoProfile"]);
        #[cfg(unix)] command.arg("-i");
        let (tx, rx) = std::sync::mpsc::channel();
        let descriptor = spawn(dir.path(), command, "test".into(), 80, 24, Arc::new(move |event| { let _ = tx.send(event); })).unwrap();
        input(&descriptor.terminal_id, "echo orange-terminal-test\r\n").unwrap();
        resize(&descriptor.terminal_id, 100, 30).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let mut output = Vec::new();
        while !String::from_utf8_lossy(&output).contains("orange-terminal-test") {
            match rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) {
                Ok(TerminalEvent::Output { data }) => {
                    output.extend(data);
                    if String::from_utf8_lossy(&output).contains("\u{1b}[6n") {
                        // 测试客户端没有 xterm 解析器，显式回应 ConPTY 的光标位置询问。
                        input(&descriptor.terminal_id, "\u{1b}[1;1R").unwrap();
                    }
                }
                _ => break,
            }
        }
        close(&descriptor.terminal_id).unwrap();
        assert!(String::from_utf8_lossy(&output).contains("orange-terminal-test"), "terminal output: {:?}", String::from_utf8_lossy(&output));
        assert!(find(&descriptor.terminal_id).is_err());
        close(&descriptor.terminal_id).unwrap();
    }
}
