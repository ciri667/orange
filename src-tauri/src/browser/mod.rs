//! 浏览器 Agent：受控 Chromium 会话、元素快照驱动的模型操作和面板实时画面。
//!
//! 模型通过 `run + kind: "browser"` 操作页面；用户在面板里实时查看并可随时接管。
//! 截图只用于面板预览和操作凭证，不进入模型上下文。

pub mod fetcher;
pub mod manager;
mod input;
pub mod policy;
mod session;
mod snapshot;
#[cfg(windows)]
mod native_input;
#[cfg(windows)]
mod surface_host;
#[cfg(windows)]
mod win32_surface;
#[cfg(target_os = "macos")]
mod wkwebview_surface;
pub mod types;

pub use fetcher::*;
pub use manager::*;
pub use policy::*;
pub use types::*;
pub use input::BrowserUserInput;
