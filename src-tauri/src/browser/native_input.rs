//! 只观察原生 Chromium 内的输入；低级钩子回到本进程执行，不向 Chromium 注入代码。

use std::cell::RefCell;
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;

use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/**
 * 用户输入意图。
 *
 * 只有会改变页面状态的输入才算接管。滚轮只调整视口：用户看着长页面时必然会
 * 上下滚动，如果这也算接管，Agent 的整轮工具调用会在用户读页面时被误停。
 */
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UserInputSignal {
    /// 点击、按键等会改变页面状态的输入。
    Operation,
    /// 滚轮等只改变视口位置的输入。
    View,
}

pub(super) type InputCallback = Arc<dyn Fn(UserInputSignal) + Send + Sync>;

thread_local! {
    /// 每个 surface 的钩子有自己的消息线程，避免测试或重建窗口时混用目标 HWND。
    static TARGET: RefCell<Option<(isize, InputCallback)>> = const { RefCell::new(None) };
}

/** 鼠标按钮是页面操作；滚轮只滚动视口，不撤销 Agent 租约。 */
fn mouse_signal(message: u32) -> Option<UserInputSignal> {
    match message {
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN => {
            Some(UserInputSignal::Operation)
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => Some(UserInputSignal::View),
        _ => None,
    }
}

/** 键盘一律按页面操作处理：低级钩子无法区分方向键翻页和输入框内移动光标。 */
fn keyboard_signal(message: u32) -> Option<UserInputSignal> {
    matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN).then_some(UserInputSignal::Operation)
}

/** 仅窗口自身及其渲染子窗口的输入可以接管，其他应用和地址栏输入不会误触发。 */
fn notify_target(window: HWND, signal: UserInputSignal) {
    TARGET.with(|target| {
        if let Some((root, callback)) = target.borrow().as_ref() {
            let root = *root as HWND;
            if !window.is_null()
                && unsafe { IsWindowVisible(root) } != 0
                && (window == root || unsafe { IsChild(root, window) } != 0)
            {
                callback(signal);
            }
        }
    });
}

unsafe extern "system" fn mouse_hook(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        if let Some(signal) = mouse_signal(message as u32) {
            let event = &*(data as *const MSLLHOOKSTRUCT);
            // CDP 不经过这些钩子；忽略系统注入事件，避免自动化输入反过来撤销自身租约。
            if event.flags & LLMHF_INJECTED == 0 {
                notify_target(WindowFromPoint(event.pt), signal);
            }
        }
    }
    // 这里只观察，始终放行原始输入及其他应用的钩子。
    CallNextHookEx(std::ptr::null_mut(), code, message, data)
}

unsafe extern "system" fn keyboard_hook(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        if let Some(signal) = keyboard_signal(message as u32) {
            let event = &*(data as *const KBDLLHOOKSTRUCT);
            if event.flags & LLKHF_INJECTED == 0 {
                let mut info = GUITHREADINFO {
                    cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
                    ..Default::default()
                };
                // GetFocus 只读当前线程；GetGUIThreadInfo 可取得跨进程的实际前台焦点。
                if GetGUIThreadInfo(0, &mut info) != 0 {
                    notify_target(info.hwndFocus, signal);
                }
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
    use std::sync::Mutex;

    /** 滚轮属于查看，点击和按键才需要接管；键盘无法区分翻页与编辑光标。 */
    #[test]
    fn wheel_is_view_while_clicks_and_keys_are_operations() {
        assert_eq!(mouse_signal(WM_MOUSEWHEEL), Some(UserInputSignal::View));
        assert_eq!(mouse_signal(WM_MOUSEHWHEEL), Some(UserInputSignal::View));
        assert_eq!(
            mouse_signal(WM_LBUTTONDOWN),
            Some(UserInputSignal::Operation)
        );
        assert_eq!(
            mouse_signal(WM_RBUTTONDOWN),
            Some(UserInputSignal::Operation)
        );
        assert_eq!(mouse_signal(WM_MOUSEMOVE), None);
        assert_eq!(keyboard_signal(WM_KEYDOWN), Some(UserInputSignal::Operation));
        assert_eq!(
            keyboard_signal(WM_SYSKEYDOWN),
            Some(UserInputSignal::Operation)
        );
        assert_eq!(keyboard_signal(WM_KEYUP), None);
    }

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
            let calls = Arc::new(Mutex::new(Vec::new()));
            let callback: InputCallback = Arc::new({
                let calls = calls.clone();
                move |signal| calls.lock().unwrap().push(signal)
            });
            let observer = InputObserver::start(root as isize, callback.clone()).unwrap();
            TARGET.with(|target| *target.borrow_mut() = Some((root as isize, callback)));
            ShowWindow(root, SW_SHOWNOACTIVATE);
            notify_target(GetDesktopWindow(), UserInputSignal::Operation);
            assert!(calls.lock().unwrap().is_empty());
            notify_target(child, UserInputSignal::View);
            assert_eq!(*calls.lock().unwrap(), vec![UserInputSignal::View]);
            notify_target(child, UserInputSignal::Operation);
            assert_eq!(calls.lock().unwrap().len(), 2);
            ShowWindow(root, SW_HIDE);
            notify_target(child, UserInputSignal::Operation);
            assert_eq!(calls.lock().unwrap().len(), 2);
            TARGET.with(|target| *target.borrow_mut() = None);
            drop(observer);
            DestroyWindow(root);
        }
    }
}
