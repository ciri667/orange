//! 浏览器面板的 Tauri 命令：状态、启用/禁用、下载、用户导航与接管。
//!
//! 模型侧操作审批走统一的 operation_approval，不再有浏览器专属审批 IPC。

use tauri::{AppHandle, Emitter};

use crate::browser::{
    self, user_navigate as browser_navigate, user_take_over, BrowserState,
};

/** 读取浏览器整体状态；面板和设置页共用。 */
#[tauri::command]
pub async fn browser_status(app: AppHandle) -> Result<BrowserState, String> {
    Ok(browser::browser_status(&app).await)
}

/** 用户明确启动浏览器或重试失败的启动。 */
#[tauri::command]
pub async fn browser_start(app: AppHandle) -> Result<BrowserState, String> {
    browser::user_start(&app).await
}

#[tauri::command]
pub async fn browser_create_tab(app: AppHandle) -> Result<BrowserState, String> {
    browser::user_create_tab(&app).await
}

#[tauri::command]
pub async fn browser_activate_tab(app: AppHandle, tab_id: String) -> Result<BrowserState, String> {
    browser::user_activate_tab(&app, &tab_id).await
}

#[tauri::command]
pub async fn browser_show_tab(app: AppHandle, tab_id: String) -> Result<BrowserState, String> {
    browser::show_tab(&app, &tab_id).await
}

/** 面板输入只用于用户操作，Agent 仍通过既有 run 工具闭集调用。 */
#[tauri::command]
pub async fn browser_user_input(
    app: AppHandle,
    tab_id: String,
    input: browser::BrowserUserInput,
) -> Result<(), String> {
    browser::user_input(&app, &tab_id, input).await
}

/** 用户在设置里开启/关闭浏览器功能；关闭时同时停掉浏览器进程。 */
#[tauri::command]
pub async fn set_browser_enabled(app: AppHandle, enabled: bool) -> Result<BrowserState, String> {
    let mut settings = crate::storage::load_user_settings(&app)?;
    settings.browser.enabled = enabled;
    crate::storage::save_user_settings(&app, &settings)?;
    if !enabled {
        browser::shutdown(&app).await;
    }
    let state = browser::browser_status(&app).await;
    let _ = app.emit(crate::browser::BROWSER_STATE_EVENT, &state);
    Ok(state)
}

/** 首次启用时下载固定版本 Chromium；绝不静默下载。 */
#[tauri::command]
pub async fn download_browser_chromium(
    app: AppHandle,
) -> Result<crate::browser::ChromiumInstallStatus, String> {
    if !browser::browser_settings_enabled(&app) {
        return Err("请先在设置中启用浏览器功能。".to_owned());
    }
    let version = browser::ensure_chromium_downloaded(&app).await?;
    browser::publish_state_public(&app).await;
    let status = browser::chromium_install_status(&app).await;
    Ok(crate::browser::ChromiumInstallStatus {
        version: Some(version),
        ..status
    })
}

/** 查询 Chromium 安装状态，供设置页展示。 */
#[tauri::command]
pub async fn browser_chromium_status(
    app: AppHandle,
) -> Result<crate::browser::ChromiumInstallStatus, String> {
    Ok(browser::chromium_install_status(&app).await)
}

/** 用户在面板地址栏手动导航。 */
#[tauri::command]
pub async fn browser_user_navigate(app: AppHandle, url: String, tab_id: Option<String>) -> Result<BrowserState, String> {
    browser_navigate(&app, &url, tab_id.as_deref()).await
}

/** 用户点击“我来接管”。 */
#[tauri::command]
pub async fn browser_user_take_over(
    app: AppHandle,
    session_id: Option<String>,
) -> Result<BrowserState, String> {
    user_take_over(&app, session_id).await
}

/** 面板关闭某个标签页。 */
#[tauri::command]
pub async fn browser_close_tab(app: AppHandle, tab_id: String) -> Result<BrowserState, String> {
    browser::user_close_tab(&app, &tab_id).await
}

/** 清除站点数据：关闭浏览器并删除隔离 profile。 */
#[tauri::command]
pub async fn browser_clear_site_data(app: AppHandle) -> Result<(), String> {
    browser::clear_site_data(&app).await
}

/** 面板开关实时画面（CDP screencast）。 */
#[tauri::command]
pub async fn browser_set_screencast(app: AppHandle, enabled: bool) -> Result<(), String> {
    browser::set_screencast_enabled(&app, enabled).await
}
