//! 终端进程树归属于自己的系统资源，关闭页签和应用退出都走同一清理路径。

#[cfg(windows)]
pub struct ProcessGuard(usize);

#[cfg(windows)]
impl ProcessGuard {
    pub fn attach(pid: u32) -> Result<Self, String> {
        use windows_sys::Win32::{Foundation::CloseHandle, System::{JobObjects::*, Threading::*}};
        // Shell 尚未接收用户输入，先加入 Job，再把可用终端返回给前端。
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() { return Err("无法创建终端进程组。".into()); }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            let configured = SetInformationJobObject(job, JobObjectExtendedLimitInformation, (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(), std::mem::size_of_val(&limits) as u32) != 0;
            let assigned = !process.is_null() && configured && AssignProcessToJobObject(job, process) != 0;
            if !process.is_null() { CloseHandle(process); }
            if !assigned { CloseHandle(job); return Err("无法将 Shell 绑定到终端进程组。".into()); }
            Ok(Self(job as usize))
        }
    }

    pub fn terminate(&self) {
        // Job 包含 Shell 与它启动的子进程，不能只结束父 Shell。
        unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0 as _, 1); }
    }
}

#[cfg(windows)]
impl Drop for ProcessGuard {
    fn drop(&mut self) { unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0 as _); } }
}

/**
 * 在 parent_pid 的直接子进程里找到由 executable 启动的那个进程，并把它的整棵进程树绑进 Job。
 *
 * Chromium 会派生 GPU / 渲染 / 工具进程，只盯浏览器主进程会留下孤儿；
 * 绑定之后父进程无论是正常退出、崩溃还是被强杀，内核都会把这些子进程一起结束。
 */
#[cfg(windows)]
pub fn attach_owned_child(
    parent_pid: u32,
    executable: &std::path::Path,
) -> Option<ProcessGuard> {
    ProcessGuard::attach(find_child_pid(parent_pid, executable)?).ok()
}

#[cfg(unix)]
pub fn attach_owned_child(
    _parent_pid: u32,
    _executable: &std::path::Path,
) -> Option<ProcessGuard> {
    // Unix 没有 Job Object；子进程回收由调用方在退出路径上显式结束。
    None
}

/** 快照一次进程表，按“父进程 + 可执行文件名”定位刚启动的子进程；唯一命中才认。 */
#[cfg(windows)]
fn find_child_pid(parent_pid: u32, executable: &std::path::Path) -> Option<u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    // 只比文件名：完整镜像路径在短路径 / verbatim 前缀上极易不同名，
    // 而父进程是自己，且我们只启动一个 chrome.exe，命中多个反而说明状态异常。
    let expected = executable.file_name()?.to_string_lossy().to_lowercase();
    let mut matches = Vec::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                if entry.th32ParentProcessID == parent_pid {
                    let name = String::from_utf16_lossy(
                        &entry.szExeFile[..entry
                            .szExeFile
                            .iter()
                            .position(|unit| *unit == 0)
                            .unwrap_or(entry.szExeFile.len())],
                    );
                    if name.to_lowercase() == expected {
                        matches.push(entry.th32ProcessID);
                    }
                }
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
    }
    match matches.as_slice() {
        [pid] => Some(*pid),
        _ => None,
    }
}

#[cfg(unix)]
pub struct ProcessGuard(libc::pid_t);

#[cfg(unix)]
impl ProcessGuard {
    pub fn attach(pid: u32) -> Result<Self, String> {
        let pid = pid as libc::pid_t;
        // portable-pty 在新会话中启动 Shell，绝不向应用自己的进程组发送信号。
        if unsafe { libc::getpgid(pid) } != pid { return Err("Shell 未进入独立终端进程组。".into()); }
        Ok(Self(pid))
    }
    pub fn terminate(&self) { unsafe { libc::kill(-self.0, libc::SIGKILL); } }
}
