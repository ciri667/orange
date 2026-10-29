//! Windows native surface for the fixed headful Chromium.
//!
//! Chromium runs as a separate process for CDP and profile isolation. Its top-level
//! window is re-parented into the Tauri window so the browser panel can show a real
//! Chromium surface instead of a JPEG screencast.

#![cfg(windows)]

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, POINT, TRUE};
use windows_sys::Win32::Graphics::Gdi::{CreateRectRgn, DeleteObject, ScreenToClient, SetWindowRgn};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetClassNameW, GetClientRect, GetWindowLongPtrW, GetWindowRect,
    GetWindowThreadProcessId, IsWindow, SetForegroundWindow, SetParent, SetWindowLongPtrW,
    SetWindowPos, ShowWindow, GWL_STYLE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOZORDER,
    SW_HIDE, SW_SHOWNOACTIVATE,
    WS_CAPTION, WS_CHILD, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
};
use super::native_input::{InputCallback, InputObserver};

/// 用户在原生 Chromium 里点击或按键时回调；用于撤销 Agent 控制租约。
static INPUT_CALLBACK: OnceLock<InputCallback> = OnceLock::new();

/// 注册全局用户输入回调。只允许设置一次，后续调用忽略。
pub fn set_user_input_callback(callback: InputCallback) {
    let _ = INPUT_CALLBACK.set(callback);
}

/// Chrome 自带工具栏的最大可信高度（物理像素）。`--app` 模式的工具栏通常
/// 在 0–90px；超过这个值说明窗口还没布局好，差值不能当真。
const MAX_CHROME_TOOLBAR_INSET: i32 = 96;

/// 客户区小于这个高度时认为窗口尚未真正显示，不做 inset 标定。
const MIN_CALIBRATION_CLIENT_HEIGHT: i32 = 200;

/// 由客户区高度与内容高度算出新的 `top_inset`。
///
/// 抽成纯函数是为了能直接测：白屏事故的根因就是这里算出了一个上千像素的
/// 假内边距，把整个 Chromium 画面推到面板上方几百像素处。
fn next_top_inset(current: i32, client_height: i32, content_height: i32) -> i32 {
    // 客户区太小时窗口还没真正显示，此时算出来的差值没有意义。
    if client_height < MIN_CALIBRATION_CLIENT_HEIGHT {
        return current.clamp(0, MAX_CHROME_TOOLBAR_INSET);
    }
    let inset = client_height - content_height;
    if (0..=MAX_CHROME_TOOLBAR_INSET).contains(&inset) {
        inset
    } else {
        // 不可信的差值：保留上一次可信值，首次则用 0，绝不把画面推走。
        current.clamp(0, MAX_CHROME_TOOLBAR_INSET)
    }
}

/// A top-level Chromium window that has been re-parented into the Tauri window.
#[derive(Debug)]
pub struct ChromiumSurface {
    hwnd: isize,
    /// Chrome's in-window toolbar height in physical pixels. The child window is
    /// shifted up by this amount and made taller so its web-content viewport
    /// exactly fills the panel.
    top_inset: i32,
    /// 注册失败不能悄悄忽略；观察器与窗口同寿命，释放时卸载两个低级钩子。
    _input_observer: InputObserver,
}

/// Enumerates top-level Chrome windows belonging to `pid`.
fn windows_for_pid(pid: u32) -> Vec<isize> {
    struct Context {
        pid: u32,
        windows: Vec<isize>,
    }

    unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> i32 {
        let context = &mut *(lparam as *mut Context);
        let mut window_pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut window_pid);
        if window_pid == context.pid {
            let mut class_name = [0u16; 128];
            let length = GetClassNameW(hwnd, class_name.as_mut_ptr(), class_name.len() as i32);
            if length > 0 {
                let class = String::from_utf16_lossy(&class_name[..length as usize]);
                // Chromium 的顶层窗口既有 `Chrome_WidgetWin_0` 也有 `_1`，取决于
                // 版本与启动方式；用非零矩形排掉 0x0 的辅助/托盘窗口。
                if class.starts_with("Chrome_WidgetWin") {
                    let mut rect = windows_sys::Win32::Foundation::RECT::default();
                    if GetWindowRect(hwnd, &mut rect) != 0
                        && rect.right - rect.left > 120
                        && rect.bottom - rect.top > 120
                    {
                        context.windows.push(hwnd as isize);
                    }
                }
            }
        }
        TRUE
    }

    let mut context = Context {
        pid,
        windows: Vec::new(),
    };
    unsafe {
        EnumWindows(Some(callback), &mut context as *mut Context as LPARAM);
    }
    context.windows
}

/// Waits for the top-level Chromium browser window. Chrome may create the
/// CDP-requested window hidden until the page is brought to front, so hidden
/// windows are accepted and explicitly shown before embedding.
pub fn wait_for_new_window(pid: u32, timeout: Duration) -> Result<isize, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(hwnd) = windows_for_pid(pid).into_iter().next() {
            return Ok(hwnd);
        }
        if Instant::now() >= deadline {
            return Err("等待 Chromium 窗口超时。".to_owned());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

impl ChromiumSurface {
    /// Re-parents a Chromium window into `host_hwnd` and strips its native frame.
    pub fn embed(host_hwnd: isize, chrome_hwnd: isize) -> Result<Self, String> {
        if unsafe { IsWindow(chrome_hwnd as HWND) } == 0 {
            return Err("Chromium 窗口句柄无效。".to_owned());
        }
        unsafe {
            let style = GetWindowLongPtrW(chrome_hwnd as HWND, GWL_STYLE);
            let remove = (WS_POPUP | WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX
                | WS_MAXIMIZEBOX) as isize;
            let next = (style & !remove) | WS_CHILD as isize;
            SetWindowLongPtrW(chrome_hwnd as HWND, GWL_STYLE, next);

            SetLastError(0);
            let old_parent = SetParent(chrome_hwnd as HWND, host_hwnd as HWND);
            let error = GetLastError();
            if old_parent.is_null() && error != 0 {
                return Err(format!("无法把 Chromium 窗口嵌入应用窗口（错误 {error}）。"));
            }
            // Keep it hidden until the panel reports its real bounds.
            ShowWindow(chrome_hwnd as HWND, SW_HIDE);
        }
        // 不能跨进程 SetWindowSubclass；低级输入钩子在本进程的独立消息线程执行。
        let callback = INPUT_CALLBACK.get().cloned().unwrap_or_else(|| Arc::new(|| {}));
        let observer = match InputObserver::start(chrome_hwnd, callback) {
            Ok(observer) => observer,
            Err(error) => {
                unsafe { SetParent(chrome_hwnd as HWND, std::ptr::null_mut()); }
                return Err(error);
            }
        };
        Ok(Self {
            hwnd: chrome_hwnd,
            top_inset: 0,
            _input_observer: observer,
        })
    }

    /// Native window handle; used by tests and diagnostics.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn hwnd(&self) -> isize {
        self.hwnd
    }

    /// 直接读取 Chromium 渲染子窗口的边界，避免窗口布局依赖可能被 alert 阻塞的 CDP。
    fn calibrate_inset(&mut self) {
        struct ContentRect { root: HWND, top: Option<i32> }
        unsafe extern "system" fn find_content(hwnd: HWND, data: LPARAM) -> i32 {
            let context = &mut *(data as *mut ContentRect);
            let mut name = [0u16; 128];
            let length = GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32);
            if String::from_utf16_lossy(&name[..length.max(0) as usize]) == "Chrome_RenderWidgetHostHWND" {
                let mut rect = windows_sys::Win32::Foundation::RECT::default();
                if GetWindowRect(hwnd, &mut rect) != 0 && rect.bottom > rect.top {
                    let mut point = POINT { x: rect.left, y: rect.top };
                    if ScreenToClient(context.root, &mut point) != 0 {
                        context.top = Some(point.y);
                        return 0;
                    }
                }
            }
            TRUE
        }
        let mut context = ContentRect { root: self.hwnd as HWND, top: None };
        let mut client = windows_sys::Win32::Foundation::RECT::default();
        unsafe {
            EnumChildWindows(self.hwnd as HWND, Some(find_content), &mut context as *mut _ as LPARAM);
            GetClientRect(self.hwnd as HWND, &mut client);
        }
        if let Some(top) = context.top {
            self.top_inset = next_top_inset(self.top_inset, client.bottom, client.bottom - top);
        }
    }

    /// Applies logical panel bounds. The native child is extended above the panel
    /// by `top_inset`, clipping Chrome's own toolbar outside the visible area.
    pub fn set_bounds(&mut self, x: f64, y: f64, width: f64, height: f64, scale_factor: f64) {
        self.calibrate_inset();
        let scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
            scale_factor
        } else {
            1.0
        };
        let x = (x * scale_factor).round() as i32;
        let y = (y * scale_factor).round() as i32;
        let width = (width * scale_factor).max(1.0).round() as i32;
        let height = (height * scale_factor).max(1.0).round() as i32;
        let native_y = y - self.top_inset;
        let native_height = height + self.top_inset;
        unsafe {
            SetWindowPos(
                self.hwnd as HWND,
                std::ptr::null_mut(),
                x,
                native_y,
                width,
                native_height,
                SWP_FRAMECHANGED | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            // 子窗口的父级是主窗口，必须显式裁掉工具栏，不能只把它上移到地址栏上方。
            let region = CreateRectRgn(0, self.top_inset, width, native_height);
            if !region.is_null() && SetWindowRgn(self.hwnd as HWND, region, TRUE) == 0 {
                DeleteObject(region);
            }
        }
    }

    pub fn show(&self) {
        unsafe {
            ShowWindow(self.hwnd as HWND, SW_SHOWNOACTIVATE);
        }
    }

    pub fn hide(&self) {
        unsafe {
            ShowWindow(self.hwnd as HWND, SW_HIDE);
        }
    }

    pub fn focus(&self) {
        unsafe {
            if SetFocus(self.hwnd as HWND).is_null() {
                SetForegroundWindow(self.hwnd as HWND);
            }
        }
    }

    /// Detaches the child so Chromium can close its own window normally.
    pub fn detach(&self) {
        unsafe {
            ShowWindow(self.hwnd as HWND, SW_HIDE);
            SetParent(self.hwnd as HWND, std::ptr::null_mut());
        }
    }
}

// `windows-sys` uses raw pointers for HWND; SetLastError is kept here so the
// caller can distinguish "desktop was the old parent" from a real failure.
use windows_sys::Win32::Foundation::SetLastError;

#[allow(dead_code)]
fn _assert_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ChromiumSurface>();
}

#[cfg(test)]
mod tests {
    use super::*;

    /** 白屏事故：最小化窗口里算出 ~1243px 的假内边距，把画面推到 y=-1018。 */
    #[test]
    fn rejects_implausible_toolbar_inset() {
        // 客户区 1919、内容 676 → 差值 1243，远超任何真实工具栏高度。
        assert_eq!(next_top_inset(0, 1919, 676), 0);
    }

    /** 真实工具栏高度仍然要能标定出来。 */
    #[test]
    fn keeps_plausible_toolbar_inset() {
        assert_eq!(next_top_inset(0, 745, 679), 66);
        assert_eq!(next_top_inset(0, 700, 700), 0);
    }

    /** 窗口还没显示（客户区很小）时不能改动已有可信值。 */
    #[test]
    fn ignores_calibration_while_window_is_tiny() {
        assert_eq!(next_top_inset(66, 26, 0), 66);
        assert_eq!(next_top_inset(0, 0, 0), 0);
    }

    /** 任何情况下都不能留下超过工具栏上限的内边距。 */
    #[test]
    fn never_exceeds_toolbar_cap() {
        assert_eq!(next_top_inset(5000, 400, 300), MAX_CHROME_TOOLBAR_INSET);
        assert!(next_top_inset(0, 1919, 676) <= MAX_CHROME_TOOLBAR_INSET);
    }
}
