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
