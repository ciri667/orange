//! macOS 原生浏览 surface 的骨架。
//!
//! Windows 走 `win32_surface`：独立 Chromium 进程 + `SetParent` 嵌入 + CDP。
//! macOS 不能重父化别的进程的 `NSWindow`，计划改为在 Tauri 窗口里挂一个
//! `WKWebView`，并通过 `WKContentWorld` 隔离的 JS 桥做自动化，避免和页面脚本
//! 抢 `window.webkit.messageHandlers`。
//!
//! 目前 macOS 仍使用 screencast 兼容路径（`surface_mode()` 返回 `"stream"`）；
//! 本文件先把接口约定和落地步骤固定下来，等 macOS 机器或 CI 可用时实现。

#![cfg(target_os = "macos")]
#![allow(dead_code)]

use std::time::Duration;

/// 原生 surface 的几何信息，单位是逻辑像素（points）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub scale: f64,
}

/// macOS 侧 `WKWebView` 的句柄包装。
///
/// 真实实现需要持有 `Retained<WKWebView>` 与创建脚本消息处理器的 block；
/// 句柄本身只用来在 Rust 侧识别"当前是否已经有一个可用的 web view"。
pub struct WebKitSurface {
    /// 占位：真实实现里是 `Retained<WKWebView>`。
    _private: (),
}

impl WebKitSurface {
    /// 在 `host`（Tauri 主窗口的 `NSView`）里创建并挂载一个 `WKWebView`。
    ///
    /// 落地时要做三件事：
    /// 1. `WKWebViewConfiguration` 里 `setWebsiteDataStore:` 指向应用自己的
    ///    非持久化或独立 data store，保证**不读**用户日常 Safari 的数据；
    /// 2. 用 `WKContentWorld::worldWithName("OrangeAgent")` 注册
    ///    `WKScriptMessageHandlerWithReply`，把自动化桥和页面自己的 world 隔开；
    /// 3. `setAutoresizingMask:` 或 Auto Layout 约束跟随宿主视图尺寸。
    pub fn attach(_host: isize) -> Result<Self, String> {
        Err("macOS 原生 surface 尚未实现；当前仍使用 screencast 兼容路径。".to_owned())
    }

    /// 导航到 http/https 页面；URL 策略由 `policy` 模块先行校验。
    pub fn navigate(&self, _url: &str) -> Result<(), String> {
        Err("macOS 原生 surface 尚未实现。".to_owned())
    }

    /// 在隔离 world 里执行一段 JS 并等待返回值。
    pub fn evaluate(&self, _script: &str, _timeout: Duration) -> Result<String, String> {
        Err("macOS 原生 surface 尚未实现。".to_owned())
    }

    /// 把 web view 摆到面板位置；坐标系是宿主视图的左上原点。
    pub fn set_bounds(&self, _bounds: SurfaceBounds) {}

    pub fn show(&self) {}

    pub fn hide(&self) {}

    pub fn focus(&self) {}

    /// 从宿主视图移除并释放；WebKit 的释放必须回到主线程。
    pub fn detach(self) {}
}

/// 是否有可用的原生 surface 实现；macOS 实现落地前保持 `false`，
/// 让上层继续走 screencast，而不是把面板变成空白。
pub fn native_surface_available() -> bool {
    false
}
