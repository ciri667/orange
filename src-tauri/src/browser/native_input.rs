//! 只观察原生 Chromium 内的输入；低级钩子回到本进程执行，不向 Chromium 注入代码。

use std::cell::RefCell;
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;

use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub(super) type InputCallback = Arc<dyn Fn() + Send + Sync>;

thread_local! {
    /// 每个 surface 的钩子有自己的消息线程，避免测试或重建窗口时混用目标 HWND。
    static TARGET: RefCell<Option<(isize, InputCallback)>> = const { RefCell::new(None) };
}

/** 仅窗口自身及其渲染子窗口的输入可以接管，其他应用和地址栏输入不会误触发。 */
fn notify_target(window: HWND) {
    TARGET.with(|target| {
        if let Some((root, callback)) = target.borrow().as_ref() {
            let root = *root as HWND;
            if !window.is_null()
                && unsafe { IsWindowVisible(root) } != 0
                && (window == root || unsafe { IsChild(root, window) } != 0)
            {
                callback();
            }
        }
    });
}

unsafe extern "system" fn mouse_hook(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32
        && matches!(message as u32, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN
            | WM_XBUTTONDOWN | WM_MOUSEWHEEL | WM_MOUSEHWHEEL)
    {
        let event = &*(data as *const MSLLHOOKSTRUCT);
        // CDP 不经过这些钩子；忽略系统注入事件，避免自动化输入反过来撤销自身租约。
        if event.flags & LLMHF_INJECTED == 0 {
            notify_target(WindowFromPoint(event.pt));
        }
    }
    // 这里只观察，始终放行原始输入及其他应用的钩子。
    CallNextHookEx(std::ptr::null_mut(), code, message, data)
}

unsafe extern "system" fn keyboard_hook(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && matches!(message as u32, WM_KEYDOWN | WM_SYSKEYDOWN) {
        let event = &*(data as *const KBDLLHOOKSTRUCT);
        if event.flags & LLKHF_INJECTED == 0 {
            let mut info = GUITHREADINFO {
                cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
                ..Default::default()
            };
            // GetFocus 只读当前线程；GetGUIThreadInfo 可取得跨进程的实际前台焦点。
            if GetGUIThreadInfo(0, &mut info) != 0 {
                notify_target(info.hwndFocus);
            }
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, message, data)
}

/** 钩子必须由持续泵消息的线程持有；surface 释放时显式卸载并退出线程。 */
#[derive(Debug)]
pub(super) struct InputObserver {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl InputObserver {
    pub fn start(hwnd: isize, callback: InputCallback) -> Result<Self, String> {
        let (ready, result) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("orange-browser-input".to_owned())
            .spawn(move || unsafe {
                TARGET.with(|target| *target.borrow_mut() = Some((hwnd, callback)));
                let module = GetModuleHandleW(std::ptr::null());
                let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), module, 0);
                let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), module, 0);
                if mouse.is_null() || keyboard.is_null() {
                    let error = GetLastError();
                    if !mouse.is_null() { UnhookWindowsHookEx(mouse); }
                    if !keyboard.is_null() { UnhookWindowsHookEx(keyboard); }
                    let _ = ready.send(Err(format!("无法监听浏览器用户输入（错误 {error}）。")));
                    return;
                }
                let mut message = MSG::default();
                // 先创建消息队列再回报成功，保证即刻 Drop 也能投递 WM_QUIT。
                PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
                if ready.send(Ok(GetCurrentThreadId())).is_ok() {
                    while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                UnhookWindowsHookEx(keyboard);
                UnhookWindowsHookEx(mouse);
                TARGET.with(|target| *target.borrow_mut() = None);
            })
            .map_err(|error| format!("无法启动浏览器输入监听：{error}"))?;
        match result.recv() {
            Ok(Ok(thread_id)) => Ok(Self { thread_id, thread: Some(thread) }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(error) => {
                let _ = thread.join();
                Err(format!("浏览器输入监听意外退出：{error}"))
            }
        }
    }
}

impl Drop for InputObserver {
    fn drop(&mut self) {
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0); }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /** 真实 HWND 树验证过滤器：外部窗口不接管，目标后代可接管，隐藏后不接管。 */
    #[test]
    fn input_observer_filters_windows_and_releases_its_hooks() {
        unsafe {
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            let root = CreateWindowExW(0, class.as_ptr(), class.as_ptr(), WS_POPUP,
                -30000, -30000, 200, 200, std::ptr::null_mut(), std::ptr::null_mut(),
                std::ptr::null_mut(), std::ptr::null_mut());
            assert!(!root.is_null());
            let child = CreateWindowExW(0, class.as_ptr(), class.as_ptr(), WS_CHILD,
                0, 0, 100, 100, root, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
            let calls = Arc::new(AtomicUsize::new(0));
            let callback: InputCallback = Arc::new({ let calls = calls.clone(); move || { calls.fetch_add(1, Ordering::SeqCst); } });
            let observer = InputObserver::start(root as isize, callback.clone()).unwrap();
            TARGET.with(|target| *target.borrow_mut() = Some((root as isize, callback)));
            ShowWindow(root, SW_SHOWNOACTIVATE);
            notify_target(GetDesktopWindow());
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            notify_target(child);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            ShowWindow(root, SW_HIDE);
            notify_target(child);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            TARGET.with(|target| *target.borrow_mut() = None);
            drop(observer);
            DestroyWindow(root);
        }
    }
}
