//! 浏览器会话管理：独立 Chromium 生命周期、控制租约、审批、快照与输入。
//!
//! Agent 与用户操作同一个受控浏览器会话。用户可以随时在面板“接管”，
//! 接管后当前 Agent 回合的后续浏览器操作立即失败并禁止重试。
//! 所有模型可见结果都来自文本/元素快照；截图只用于面板预览和操作凭证。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chromiumoxide::element::Element;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;

use super::policy::{
    bing_search_url, normalize_web_url, operation_kind,
    session_allows_browser, snapshot_reference_matches, url_origin,
    MAX_ELEMENT_TEXT_CHARS, MAX_SNAPSHOT_ELEMENTS,
    MAX_TEXT_SNAPSHOT_CHARS,
};
use super::session::{BrowserSessionState, FrameSink};
use super::types::*;
use crate::domain::AgentSession;
use crate::runtime::AgentCancel;

pub const BROWSER_STATE_EVENT: &str = "browser-state-changed";
pub const BROWSER_FRAME_EVENT: &str = "browser-frame";
pub const BROWSER_ACTIVITY_EVENT: &str = "browser-activity";
pub const BROWSER_RECEIPT_EVENT: &str = "browser-receipt";
pub const BROWSER_USER_TAKEOVER: &str = "BROWSER_USER_TAKEOVER";

/** 控制租约：同一时间只有一个 Agent 会话持有操作权；接管后进 blocked。 */
#[derive(Default)]
struct BrowserControl {
    session_id: Option<String>,
    started_at: Option<Instant>,
    revision: u64,
    blocked_sessions: std::collections::HashSet<String>,
}

pub struct BrowserManager {
    state: Mutex<Option<BrowserSessionState>>,
    control: Mutex<BrowserControl>,
    approval_state: Mutex<Option<BrowserState>>,
    approval_elements: Mutex<HashMap<String, Value>>,
    /** 最近一次用户手动导航时间，用于让面板活动提示和 screencast 保持冷静。 */
    last_user_navigate: Mutex<Option<Instant>>,
    /** 最近一次操作凭证，键为 receiptId。 */
    receipts: Mutex<std::collections::VecDeque<BrowserReceipt>>,
    /** 画面开关是用户意图，重启和标签切换后恢复到这一状态。 */
    screencast_enabled: std::sync::atomic::AtomicBool,
    launch_error: Mutex<Option<String>>,
    /** 接管会唤醒正在审批/执行的操作，不必等待 CDP 超时。 */
    takeover_revision: tokio::sync::watch::Sender<u64>,
}

impl Default for BrowserManager {
    fn default() -> Self {
        Self {
            state: Mutex::new(None),
            control: Mutex::new(BrowserControl::default()),
            approval_state: Mutex::new(None),
            approval_elements: Mutex::new(HashMap::new()),
            last_user_navigate: Mutex::new(None),
            receipts: Mutex::new(std::collections::VecDeque::new()),
            screencast_enabled: std::sync::atomic::AtomicBool::new(false),
            launch_error: Mutex::new(None),
            takeover_revision: tokio::sync::watch::channel(0).0,
        }
    }
}

pub async fn browser_manager() -> &'static BrowserManager {
    static MANAGER: std::sync::OnceLock<BrowserManager> = std::sync::OnceLock::new();
    MANAGER.get_or_init(BrowserManager::default)
}

fn chromium_root(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法读取应用数据目录：{error}"))?
        .join("browser")
        .join("chromium"))
}

fn chromium_executable_path(app: &AppHandle) -> Result<PathBuf, String> {
    // 下载器和启动器共用平台路径，安装状态与实际启动目标保持一致。
    super::fetcher::chromium_executable(app)
}

pub fn chromium_installed(app: &AppHandle) -> bool {
    super::fetcher::chromium_is_ready(app)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/** 读取当前完整状态；浏览器未启动时返回 available=false 的空状态。 */
pub async fn browser_status(app: &AppHandle) -> BrowserState {
    let manager = browser_manager().await;
    let enabled = super::fetcher::browser_settings_enabled(app);
    let installed = chromium_installed(app);
    let control = manager.control.lock().await;
    let control_state = BrowserControlState {
        running: control.started_at.is_some(),
        session_id: control.session_id.clone(),
        status: if control.started_at.is_some() {
            "running".to_owned()
        } else {
            "idle".to_owned()
        },
        revision: control.revision,
        elapsed_ms: control
            .started_at
            .map(|started| started.elapsed().as_millis())
            .unwrap_or_default(),
    };
    drop(control);

    let mut state = manager.state.lock().await;
    if let Some(session) = state.as_mut() {
        if session.browser.try_wait().ok().flatten().is_some() {
            session.close().await;
            *state = None;
        } else if let Err(error) = session.sync_tabs(false).await {
            *manager.launch_error.lock().await = Some(error);
            session.close().await;
            *state = None;
        }
    }
    let Some(session) = state.as_ref() else {
        return BrowserState {
            available: false,
            enabled,
            chromium_installed: installed,
            launch_error: manager.launch_error.lock().await.clone(),
            tabs: Vec::new(),
            active_tab_id: None,
            visible_tab_id: None,
            control: control_state,
        };
    };
    let mut tabs = Vec::new();
    for (tab_id, tab) in &session.tabs {
        let (url, title) = tab_identity(&tab.page).await;
        tabs.push(BrowserTabView {
            tab_id: tab_id.clone(),
            url,
            title,
            active: session.active_tab_id == *tab_id,
            visible: session.visible_tab_id == *tab_id,
        });
    }
    tabs.sort_by(|left, right| left.tab_id.cmp(&right.tab_id));
    BrowserState {
        available: true,
        enabled,
        chromium_installed: installed,
        launch_error: None,
        active_tab_id: (!session.active_tab_id.is_empty()).then(|| session.active_tab_id.clone()),
        visible_tab_id: (!session.visible_tab_id.is_empty())
            .then(|| session.visible_tab_id.clone()),
        tabs,
        control: control_state,
    }
}

/** 尽力读取标签页 URL 和标题；CDP 短暂不可达时返回空串而不是报错。 */
async fn tab_identity(page: &chromiumoxide::Page) -> (String, String) {
    let url = page.url().await.ok().flatten().unwrap_or_default();
    // 网页脚本卡住时标题查询可能等到 CDP 超时；面板状态读取不能因此阻塞用户操作。
    let title = tokio::time::timeout(Duration::from_millis(500), page.get_title())
        .await
        .ok()
        .and_then(Result::ok)
        .flatten()
        .unwrap_or_default();
    (url, title)
}

async fn publish_state(app: &AppHandle) {
    let state = browser_status(app).await;
    *browser_manager().await.approval_state.lock().await = Some(state.clone());
    let _ = app.emit(BROWSER_STATE_EVENT, state);
}

/** 供命令层在状态变化后广播最新状态。 */
pub async fn publish_state_public(app: &AppHandle) {
    publish_state(app).await;
}

pub async fn set_activity(app: &AppHandle, activity: &str, tab_id: Option<String>) {
    let _ = app.emit(
        BROWSER_ACTIVITY_EVENT,
        BrowserActivity {
            activity: activity.to_owned(),
            tab_id,
        },
    );
}

async fn begin_control(manager: &BrowserManager, session_id: &str) -> Result<(), String> {
    let mut control = manager.control.lock().await;
    if control.blocked_sessions.contains(session_id) {
        return Err(BROWSER_USER_TAKEOVER.to_owned());
    }
    if control
        .session_id
        .as_ref()
        .is_some_and(|owner| owner != session_id)
    {
        return Err("BROWSER_CONTROL_BUSY".to_owned());
    }
    if control.session_id.is_none() {
        control.session_id = Some(session_id.to_owned());
    }
    control.started_at = Some(Instant::now());
    Ok(())
}

async fn end_control(manager: &BrowserManager, session_id: &str) {
    let mut control = manager.control.lock().await;
    if control.session_id.as_deref() == Some(session_id) {
        control.started_at = None;
    }
}

/** 接管封锁只持续当前 Agent 回合；下一轮用户发送新消息后可以再次使用浏览器。 */
pub async fn finish_agent_turn(session_id: &str) {
    finish_control(browser_manager().await, session_id).await;
}

/** 主循环也检查接管状态，覆盖用户在两个浏览器调用之间接管的情况。 */
pub async fn agent_turn_was_taken_over(session_id: &str) -> bool {
    browser_manager()
        .await
        .control
        .lock()
        .await
        .blocked_sessions
        .contains(session_id)
}

async fn finish_control(manager: &BrowserManager, session_id: &str) {
    let mut control = manager.control.lock().await;
    control.blocked_sessions.remove(session_id);
    if control.session_id.as_deref() == Some(session_id) {
        control.session_id = None;
        control.started_at = None;
    }
}

/** 用户创建和切换标签也会先接管，避免与 Agent 同时操作不同页面。 */
pub async fn user_create_tab(app: &AppHandle) -> Result<BrowserState, String> {
    user_take_over(app, None).await?;
    ensure_browser(app).await?;
    let manager = browser_manager().await;
    if let Some(session) = manager.state.lock().await.as_mut() {
        session.create_tab().await?;
    }
    refresh_screencast(app).await?;
    publish_state(app).await;
    Ok(browser_status(app).await)
}

pub async fn user_activate_tab(app: &AppHandle, tab_id: &str) -> Result<BrowserState, String> {
    user_take_over(app, None).await?;
    let manager = browser_manager().await;
    let mut state = manager.state.lock().await;
    let session = state
        .as_mut()
        .ok_or_else(|| "浏览器尚未启动。".to_owned())?;
    session.activate_tab(tab_id).await?;
    drop(state);
    refresh_screencast(app).await?;
    publish_state(app).await;
    Ok(browser_status(app).await)
}

/** 页签查看与接管分离；仅更新实时画面的来源。 */
pub async fn show_tab(app: &AppHandle, tab_id: &str) -> Result<BrowserState, String> {
    let manager = browser_manager().await;
    let mut state = manager.state.lock().await;
    state.as_mut().ok_or_else(|| "浏览器尚未启动。".to_owned())?.show_tab(tab_id).await?;
    drop(state);
    refresh_screencast(app).await?;
    publish_state(app).await;
    Ok(browser_status(app).await)
}

/** 每个用户输入都先撤销 Agent 操作权；只发送到当前可见页面。 */
pub async fn user_input(
    app: &AppHandle,
    tab_id: &str,
    input: super::BrowserUserInput,
) -> Result<(), String> {
    input.validate()?;
    let manager = browser_manager().await;
    if revoke_control(manager, None).await {
        set_activity(app, "user-takeover", None).await;
    }
    let mut state = manager.state.lock().await;
    let session = state
        .as_mut()
        .ok_or_else(|| "浏览器尚未启动。".to_owned())?;
    if session.visible_tab_id != tab_id {
        return Err("标签页已切换，请在当前画面中操作。".to_owned());
    }
    let tab = session
        .tabs
        .get_mut(tab_id)
        .ok_or_else(|| "标签页已关闭。".to_owned())?;
    super::input::send_input(&tab.page, input).await?;
    tab.page_generation += 1;
    Ok(())
}

/** 用户点击“我来接管”：封锁当前持有会话并清空租约，本回合内不可恢复。 */
pub async fn user_take_over(
    app: &AppHandle,
    session_id: Option<String>,
) -> Result<BrowserState, String> {
    let manager = browser_manager().await;
    revoke_control(manager, session_id).await;
    set_activity(app, "user-takeover", None).await;
    publish_state(app).await;
    Ok(browser_status(app).await)
}

/** 接管的核心操作不等待页面状态，键盘/滚轮连续输入不会反复查询所有标签页。 */
async fn revoke_control(manager: &BrowserManager, session_id: Option<String>) -> bool {
    let mut control = manager.control.lock().await;
    let owner = control.session_id.clone().or(session_id);
    let revoked = owner.is_some();
    if let Some(owner) = owner {
        crate::operation_approval::cancel_session(&owner);
        control.blocked_sessions.insert(owner);
        control.revision += 1;
        control.session_id = None;
        control.started_at = None;
    }
    if revoked {
        manager.takeover_revision.send_replace(control.revision);
    }
    drop(control);
    // 接管时拒绝待确认操作，唤醒 Agent；输入永远只到用户当前可见标签。
    manager.approval_elements.lock().await.clear();
    revoked
}

/** 面板上的用户手动导航；只改地址，不经过模型。 */
pub async fn user_navigate(app: &AppHandle, url: &str, tab_id: Option<&str>) -> Result<BrowserState, String> {
    let url = normalize_web_url(url)?;
    let manager = browser_manager().await;
    {
        let mut last = manager.last_user_navigate.lock().await;
        *last = Some(Instant::now());
    }
    // 面板可能在浏览器进程启动前就可见（设置一开启就渲染），
    // 用户在这里导航等同于要求打开浏览器，因此按需启动。
    user_take_over(app, None).await?;
    ensure_browser(app).await?;
    let mut state = manager.state.lock().await;
    let session = state
        .as_mut()
        .ok_or_else(|| "浏览器尚未启动。".to_owned())?;
    let active_tab_id = if let Some(id) = tab_id { id.to_owned() } else if !session.visible_tab_id.is_empty() { session.visible_tab_id.clone() } else if !session.active_tab_id.is_empty() { session.active_tab_id.clone() } else { session.create_tab().await? };
    session.activate_tab(&active_tab_id).await?;
    let tab = session
        .tabs
        .get_mut(&active_tab_id)
        .ok_or_else(|| "找不到当前标签页。".to_owned())?;
    tab.page
        .goto(url.as_str())
        .await
        .map_err(cdp_error_string)?;
    tab.page_generation += 1;
    drop(state);
    refresh_screencast(app).await?;
    set_activity(app, "user-navigate", None).await;
    publish_state(app).await;
    Ok(browser_status(app).await)
}

/** CDP 错误转 String，用于返回 Result<_, String> 的公开函数。 */
fn cdp_error_string(error: chromiumoxide::error::CdpError) -> String {
    error.to_string()
}

/** 关闭指定标签页（面板按钮）；关闭当前标签后回退到第一个剩余标签。 */
pub async fn user_close_tab(app: &AppHandle, tab_id: &str) -> Result<BrowserState, String> {
    user_take_over(app, None).await?;
    let manager = browser_manager().await;
    let mut state = manager.state.lock().await;
    let session = state
        .as_mut()
        .ok_or_else(|| "浏览器尚未启动。".to_owned())?;
    session.close_tab(tab_id).await?;
    drop(state);
    refresh_screencast(app).await?;
    publish_state(app).await;
    Ok(browser_status(app).await)
}

/** 清除站点数据：关闭浏览器并删除隔离 profile 目录，下次启动重建。 */
pub async fn clear_site_data(app: &AppHandle) -> Result<(), String> {
    user_take_over(app, None).await?;
    let manager = browser_manager().await;
    // 保持状态锁直到数据删除完成，防止另一条导航抢先重建同一个 profile。
    let mut state = manager.state.lock().await;
    if let Some(mut session) = state.take() {
        session.close().await;
    }
    let profile = chromium_root(app)?.join("profile");
    super::session::remove_profile(&profile).await?;
    drop(state);
    manager.receipts.lock().await.clear();
    *manager.launch_error.lock().await = None;
    publish_state(app).await;
    Ok(())
}

/** 关闭并等待 Chromium 完整退出后清空运行态。 */
pub async fn shutdown(app: &AppHandle) {
    let manager = browser_manager().await;
    revoke_control(manager, None).await;
    let mut state = manager.state.lock().await;
    if let Some(mut session) = state.take() {
        session.close().await;
    }
    drop(state);
    let mut control = manager.control.lock().await;
    control.session_id = None;
    control.started_at = None;
    drop(control);
    publish_state(app).await;
}

/** 面板开启画面等同于要求启动浏览器；关闭时只停止画面流。 */
pub async fn set_screencast_enabled(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let manager = browser_manager().await;
    manager
        .screencast_enabled
        .store(enabled, std::sync::atomic::Ordering::SeqCst);
    if enabled {
        ensure_browser(app).await?;
    }
    refresh_screencast(app).await?;
    publish_state(app).await;
    Ok(())
}

fn frame_sink(app: &AppHandle) -> FrameSink {
    let app = app.clone();
    Arc::new(move |frame| {
        let _ = app.emit(BROWSER_FRAME_EVENT, frame);
    })
}

async fn refresh_screencast(app: &AppHandle) -> Result<(), String> {
    let manager = browser_manager().await;
    let mut state = manager.state.lock().await;
    if let Some(session) = state.as_mut() {
        session
            .set_screencast(
                manager
                    .screencast_enabled
                    .load(std::sync::atomic::Ordering::SeqCst),
                frame_sink(app),
            )
            .await?;
    }
    Ok(())
}

/** 历史审批入口不再拥有执行权。 */
pub async fn decide_browser_operation(_: &AppHandle, _: &str, _: bool) -> Result<(),String> {
    Err("旧浏览器审批已结束，请使用当前操作审批。".into())
}

/** 只读已发布的本地缓存，审批之前不发起 CDP 或网络访问。 */
pub async fn prepare_operation_approval(app: &AppHandle, session: &AgentSession, args: &Value) -> Result<Value,String> {
    crate::permissions::ensure(session,crate::permissions::Operation::ExternalTool)?;
    if !args.is_object() || args["operation"].as_str().and_then(operation_kind).is_none() { return Err("浏览器操作参数无效。".into()); }
    if !super::fetcher::browser_settings_enabled(app) { return Err("浏览器功能未启用。".into()); }
    let manager=browser_manager().await;
    begin_control(manager,&session.id).await?;
    let mut prepared=args.clone();
    let cached=manager.approval_state.lock().await.clone();
    if !matches!(args["operation"].as_str(),Some("browser.get_state"|"browser.create_tab")) && args["tabId"].is_null() {
        if let Some(tab)=cached.as_ref().and_then(|s|s.active_tab_id.as_ref()) { prepared["tabId"]=json!(tab); }
    }
    let tab=cached.as_ref().and_then(|s|s.tabs.iter().find(|t|prepared["tabId"]==t.tab_id));
    let element=if let Some(reference)=args["reference"].as_str() {manager.approval_elements.lock().await.get(reference).cloned()} else {None};
    prepared["approvalContext"]=json!({"currentUrl":tab.map(|t|&t.url),"tabTitle":tab.map(|t|&t.title),"element":element});
    Ok(prepared)
}

/** 启动与恢复在同一把状态锁下串行，避免导航与清除数据同时写 profile。 */
async fn ensure_browser(app: &AppHandle) -> Result<(), String> {
    if !super::fetcher::browser_settings_enabled(app) {
        return Err("浏览器功能未启用，请先在设置中开启。".to_owned());
    }
    let manager = browser_manager().await;
    let mut state = manager.state.lock().await;
    if let Some(session) = state.as_mut() {
        if session.browser.try_wait().ok().flatten().is_some()
            || session.sync_tabs(false).await.is_err()
        {
            session.close().await;
            *state = None;
        }
    }
    if state.is_none() {
        let result = BrowserSessionState::launch(
            chromium_executable_path(app)?,
            chromium_root(app)?.join("profile"),
        )
        .await;
        match result {
            Ok(session) => {
                *state = Some(session);
                *manager.launch_error.lock().await = None;
            }
            Err(error) => {
                *manager.launch_error.lock().await = Some(error.clone());
                drop(state);
                publish_state(app).await;
                return Err(error);
            }
        }
    }
    Ok(())
}

/** 面板明确启动或重试；返回完整状态并恢复用户的画面开关。 */
pub async fn user_start(app: &AppHandle) -> Result<BrowserState, String> {
    ensure_browser(app).await?;
    refresh_screencast(app).await?;
    publish_state(app).await;
    Ok(browser_status(app).await)
}

/**
 * 元素快照脚本：在页面主世界读取渲染后的 DOM。
 * 返回 snapshotId、元素列表（role/name/text/value/state/rect）和正文文本。
 * 密码字段 value 固定返回 "secret"，不把明文带给模型。
 */
fn snapshot_script() -> &'static str {
    r#"(() => {
      const snapshotId = window.__orangeSnapshotId || `snap-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      window.__orangeSnapshotId = snapshotId;
      const nodes = Array.from(document.querySelectorAll('a,button,input,textarea,select,[role="button"],[role="link"],[role="checkbox"],[role="combobox"],[role="tab"],[role="menuitem"]'))
        .filter((node) => node.isConnected && node.getClientRects().length > 0)
        .slice(0, 200);
      const elements = nodes.map((node, index) => {
        let reference = node.dataset.orangeRef;
        if (!reference || !reference.startsWith(snapshotId)) {
          reference = `${snapshotId}-${index}`;
          node.dataset.orangeRef = reference;
        }
        const rect = node.getBoundingClientRect();
        const isPassword = node.type === 'password';
        const text = isPassword ? null : (node.innerText || '').trim().slice(0, 240) || null;
        const name = (node.getAttribute('aria-label') || node.getAttribute('name') || node.getAttribute('placeholder') || text || '').slice(0, 240);
        return {
          snapshotId,
          reference,
          role: node.getAttribute('role') || node.tagName.toLowerCase(),
          name,
          text,
          value: isPassword ? 'secret' : ((node.value || '').toString().slice(0, 240) || null),
          state: node.disabled ? 'disabled' : (node.checked === true ? 'checked' : null),
          rect: { x: rect.x, y: rect.y, width: rect.width, height: rect.height },
        };
      });
      const text = (document.body?.innerText || '').slice(0, 12000);
      return { snapshotId, elements, text };
    })()"#
}

/** 读取标签页快照：一次 evaluate 拿文本与元素，再在 Rust 侧做分页。 */
pub(super) async fn capture_snapshot(
    page: &chromiumoxide::Page,
    text_offset: usize,
) -> Result<BrowserSnapshot, (String, String)> {
    let result = page.evaluate(snapshot_script()).await.map_err(cdp_error)?;
    let mut value: Value = result.into_value().map_err(|error| {
        (
            "SNAPSHOT_FAILED".to_owned(),
            format!("快照解析失败：{error}"),
        )
    })?;
    let snapshot_id = value
        .get("snapshotId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let body_text = value
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let (page_text, truncated, next_offset) =
        super::policy::slice_text_page(&body_text, text_offset, MAX_TEXT_SNAPSHOT_CHARS);
    let elements = match value.get_mut("elements") {
        Some(Value::Array(items)) => take_elements(Value::Array(std::mem::take(items))),
        _ => Vec::new(),
    };
    Ok(BrowserSnapshot {
        snapshot_id,
        page_generation: 0,
        captured_at: now_ms(),
        blocks: vec![BrowserTextBlock {
            id: "body".to_owned(),
            text: page_text,
        }],
        elements,
        truncated,
        next_text_offset: next_offset,
        next_element_offset: None,
    })
}

/** 把脚本返回的元素 JSON 收成 DTO；越界字段截断，缺失字段留空。 */
fn take_elements(value: Value) -> Vec<BrowserElement> {
    let Some(items) = value.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .take(MAX_SNAPSHOT_ELEMENTS)
        .filter_map(|item| {
            let rect = item.get("rect")?;
            Some(BrowserElement {
                snapshot_id: item
                    .get("snapshotId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                reference: item
                    .get("reference")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                role: item
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                name: item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .chars()
                    .take(MAX_ELEMENT_TEXT_CHARS)
                    .collect(),
                text: item.get("text").and_then(Value::as_str).map(|text| {
                    text.chars()
                        .take(MAX_ELEMENT_TEXT_CHARS)
                        .collect::<String>()
                }),
                value: item.get("value").and_then(Value::as_str).map(|text| {
                    text.chars()
                        .take(MAX_ELEMENT_TEXT_CHARS)
                        .collect::<String>()
                }),
                state: item
                    .get("state")
                    .and_then(Value::as_str)
                    .filter(|state| !state.is_empty())
                    .map(str::to_owned),
                rect: BrowserRect {
                    x: rect.get("x").and_then(Value::as_f64).unwrap_or_default(),
                    y: rect.get("y").and_then(Value::as_f64).unwrap_or_default(),
                    width: rect
                        .get("width")
                        .and_then(Value::as_f64)
                        .unwrap_or_default(),
                    height: rect
                        .get("height")
                        .and_then(Value::as_f64)
                        .unwrap_or_default(),
                },
            })
        })
        .collect()
}

/** 按元素 reference 查找 CDP Element；引用失效时返回 SNAPSHOT_EXPIRED。 */
async fn find_element_by_reference(
    page: &chromiumoxide::Page,
    reference: &str,
) -> Result<Element, (String, String)> {
    let escaped = reference.replace('\\', "\\\\").replace('"', "\\\"");
    let selector = format!("[data-orange-ref=\"{escaped}\"]");
    page.find_element(selector).await.map_err(|_| {
        (
            "SNAPSHOT_EXPIRED".to_owned(),
            "元素引用已失效，请重新获取页面快照。".to_owned(),
        )
    })
}

/** 动作前保存目标区域截图与归一化坐标作为操作凭证；只发面板事件。 */
async fn record_receipt(
    app: &AppHandle,
    tab_id: &str,
    operation: &str,
    page: &chromiumoxide::Page,
    rect: Option<&BrowserRect>,
) {
    let manager = browser_manager().await;
    let (x, y) = match rect {
        Some(rect) => (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0),
        None => (0.0, 0.0),
    };
    let url = page.url().await.ok().flatten().unwrap_or_default();
    let receipt = BrowserReceipt {
        receipt_id: crate::storage::create_id("receipt"),
        tab_id: tab_id.to_owned(),
        operation: operation.to_owned(),
        x,
        y,
        url,
        created_at: now_ms(),
    };
    // 动作前的目标区域截图只作为凭证存档，不进入模型上下文；
    // 第一版仅记录坐标与 URL，避免面板外出现第二份图像数据流。
    let mut receipts = manager.receipts.lock().await;
    if receipts.len() >= 20 {
        receipts.pop_front();
    }
    receipts.push_back(receipt.clone());
    drop(receipts);
    let _ = app.emit(BROWSER_RECEIPT_EVENT, receipt);
}

/** 写一条浏览器审计事件；只含 operation、tabId、origin、元素摘要和错误码。 */
fn write_audit(app: &AppHandle, session_id: &str, entry: BrowserAuditEntry) {
    let metadata = serde_json::to_value(&entry).unwrap_or_else(|_| json!({}));
    crate::logging::write_app_event_best_effort(
        app,
        crate::logging::AppEventBuilder::new(
            crate::logging::AppLogLevel::Info,
            crate::logging::AppLogCategory::Agent,
            "browser_operation",
            if entry.error_code.is_some() {
                "failed"
            } else {
                "completed"
            },
            format!("浏览器操作 {}", entry.operation),
        )
        .session_id(session_id)
        .metadata(metadata),
    );
}

/** 把 (code, message) 错误对转成失败的运行结果。 */
fn failed_run(code: &str, message: &str) -> ToolExecutionOutcome {
    ToolExecutionOutcome {
        result: ToolExecutionResultPayload {
            success: false,
            summary: message.to_owned(),
            payload: json!({ "ok": false, "error": { "code": code, "message": message } }),
            citations: Vec::new(),
            audit_fragment: None,
        },
    }
}

/** 浏览器分支的执行结果：与同步工具的 ToolExecutionResult 同形，供 runtime 统一消费。 */
pub(crate) struct ToolExecutionResultPayload {
    pub success: bool,
    pub summary: String,
    pub payload: Value,
    pub citations: Vec<crate::domain::Citation>,
    pub audit_fragment: Option<String>,
}

pub(crate) struct ToolExecutionOutcome {
    pub result: ToolExecutionResultPayload,
}

impl ToolExecutionOutcome {
    /** runtime 需要的三个出口：模型 payload、UI 轨迹和审计片段。 */
    pub fn into_parts(
        self,
    ) -> (
        bool,
        String,
        Value,
        Vec<crate::domain::Citation>,
        Option<String>,
    ) {
        (
            self.result.success,
            self.result.summary,
            self.result.payload,
            self.result.citations,
            self.result.audit_fragment,
        )
    }
}

type OperationResult = Result<ToolExecutionOutcome, (String, String)>;

fn cdp_error(error: chromiumoxide::error::CdpError) -> (String, String) {
    ("CDP_ERROR".to_owned(), error.to_string())
}

/**
 * `browser` 的主入口：校验权限 → 控制租约 → 审批 → 分发。
 * 用户接管后返回 BROWSER_USER_TAKEOVER，模型在本回合内不能再重试。
 */
pub async fn execute_run_request(
    app: &AppHandle,
    session: &AgentSession,
    args: &Value,
    cancel: &AgentCancel,
    permit: &crate::operation_approval::Permit,
) -> ToolExecutionOutcome {
    if let Err(e)=permit.validate(app,session,"browser",args,cancel) { return failed_run("PERMISSION_DENIED",&e); }
    let started = Instant::now();
    let request = match serde_json::from_value::<BrowserRunRequest>(args.clone()) {
        Ok(request) => request,
        Err(error) => return failed_run("INVALID_REQUEST", &format!("浏览器请求无效：{error}")),
    };
    let operation = request.operation.as_str();
    if operation_kind(operation).is_none() {
        return failed_run(
            "UNKNOWN_OPERATION",
            &format!("未知浏览器操作 {operation}。"),
        );
    }
    if !session_allows_browser(
        session.im_identity.is_some(),
        session.schedule_identity.is_some(),
        session.plan_mode,
        &session.security_level,
    ) {
        return failed_run("PERMISSION_DENIED", "当前会话安全级别不允许浏览器操作。");
    }
    if !super::fetcher::browser_settings_enabled(app) {
        return failed_run("BROWSER_DISABLED", "浏览器功能未启用。请先在设置中开启。");
    }

    let session_id = session.id.clone();
    let manager = browser_manager().await;
    if let Err(error) = begin_control(&manager, &session_id).await {
        let code = if error == BROWSER_USER_TAKEOVER {
            BROWSER_USER_TAKEOVER
        } else {
            "BROWSER_CONTROL_BUSY"
        };
        write_audit(
            app,
            &session_id,
            BrowserAuditEntry {
                operation: operation.to_owned(),
                tab_id: request
                    .params
                    .get("tabId")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                url_origin: None,
                element_summary: None,
                approval: super::policy::APPROVAL_DENIED.to_owned(),
                duration_ms: started.elapsed().as_millis(),
                error_code: Some(code.to_owned()),
            },
        );
        let message = if code == BROWSER_USER_TAKEOVER {
            "用户已接管浏览器，本轮不能再重试。"
        } else {
            "浏览器正在由其他 Agent 会话控制，请等待该会话结束。"
        };
        return failed_run(code, message);
    }

    let approval_result = if session.security_level == "full-confirm" { super::policy::APPROVAL_GRANTED } else { super::policy::APPROVAL_NOT_REQUIRED }.to_owned();
    publish_state(app).await;
    let mut takeover = manager.takeover_revision.subscribe();
    let result = if manager
        .control
        .lock()
        .await
        .blocked_sessions
        .contains(&session_id)
    {
        Err((
            BROWSER_USER_TAKEOVER.to_owned(),
            "用户已接管浏览器，本轮不能再重试。".to_owned(),
        ))
    } else {
        if let Err(e)=permit.validate(app,session,"browser",args,cancel) {
            end_control(&manager,&session_id).await;
            return failed_run("PERMISSION_DENIED",&e);
        }
        // 接管和 Stop 能释放正在等待 CDP 的状态锁，用户不必等导航或加载超时。
        tokio::select! {
            biased;
            _ = takeover.changed() => Err((BROWSER_USER_TAKEOVER.to_owned(), "用户已接管浏览器，本轮不能再重试。".to_owned())),
            _ = cancel.cancelled() => Err(("OPERATION_ABORTED".to_owned(), "用户已停止本轮。".to_owned())),
            result = dispatch_operation(app, &request, cancel) => result,
        }
    };
    end_control(&manager, &session_id).await;
    if let Err(error) = refresh_screencast(app).await {
        log::warn!("浏览器画面恢复失败：{error}");
    }
    publish_state(app).await;
    match result {
        Ok(mut outcome) => {
            if let Some(elements)=outcome.result.payload["snapshot"]["elements"].as_array() {
                let mut cache=manager.approval_elements.lock().await;
                cache.clear();
                for element in elements { if let Some(reference)=element["reference"].as_str() { cache.insert(reference.into(),element.clone()); } }
            }
            outcome.result.audit_fragment = Some(format!(
                "浏览器操作：operation={operation} duration_ms={} status=completed",
                started.elapsed().as_millis()
            ));
            write_audit(
                app,
                &session_id,
                BrowserAuditEntry {
                    operation: operation.to_owned(),
                    tab_id: request
                        .params
                        .get("tabId")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    url_origin: outcome
                        .result
                        .payload
                        .get("tab")
                        .and_then(|tab| tab.get("url"))
                        .and_then(Value::as_str)
                        .and_then(url_origin),
                    element_summary: request
                        .params
                        .get("reference")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    approval: approval_result,
                    duration_ms: started.elapsed().as_millis(),
                    error_code: None,
                },
            );
            outcome
        }
        Err((code, message)) => {
            write_audit(
                app,
                &session_id,
                BrowserAuditEntry {
                    operation: operation.to_owned(),
                    tab_id: request
                        .params
                        .get("tabId")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    url_origin: None,
                    element_summary: request
                        .params
                        .get("reference")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    approval: approval_result,
                    duration_ms: started.elapsed().as_millis(),
                    error_code: Some(code.clone()),
                },
            );
            let mut outcome = failed_run(&code, &message);
            outcome.result.audit_fragment = Some(format!(
                "浏览器操作：operation={operation} duration_ms={} status=failed code={code}",
                started.elapsed().as_millis()
            ));
            outcome
        }
    }
}

/** 按操作名分发到具体 CDP 调用；所有操作都要求先持有控制租约。 */
async fn dispatch_operation(
    app: &AppHandle,
    request: &BrowserRunRequest,
    cancel: &AgentCancel,
) -> OperationResult {
    if cancel.is_aborted() {
        return Err((
            "OPERATION_ABORTED".to_owned(),
            "用户已停止本轮。".to_owned(),
        ));
    }
    ensure_browser(app)
        .await
        .map_err(|error| ("BROWSER_UNAVAILABLE".to_owned(), error))?;
    let operation = request.operation.as_str();
    let params = &request.params;
    let manager = browser_manager().await;
    let mut state = manager.state.lock().await;
    let session = state.as_mut().ok_or_else(|| {
        (
            "BROWSER_UNAVAILABLE".to_owned(),
            "浏览器尚未启动。".to_owned(),
        )
    })?;

    let requested_tab = params
        .get("tabId")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut result = BrowserRunResult {
        ok: true,
        operation: operation.to_owned(),
        tab: None,
        snapshot: None,
        error: None,
    };
    match operation {
        "browser.get_state" => {
            // 返回真实标签列表；空的成功结果无法让模型知道该操作哪个页面。
            let mut tabs = Vec::new();
            for id in session.tabs.keys() {
                tabs.push(tab_view(session, id).await?);
            }
            return Ok(ToolExecutionOutcome {
                result: ToolExecutionResultPayload {
                    success: true,
                    summary: format!("已读取 {} 个浏览器标签页。", tabs.len()),
                    payload: json!({ "ok": true, "operation": operation, "tabs": tabs, "activeTabId": session.active_tab_id, "visibleTabId": session.visible_tab_id }),
                    citations: Vec::new(),
                    audit_fragment: None,
                },
            });
        }
        "browser.create_tab" => {
            let new_tab_id = session.create_tab_for_request(params).await?;
            result.tab = Some(tab_view(session, &new_tab_id).await?);
            return finish_operation(result, operation);
        }
        "browser.activate_tab" | "browser.switch_tab" => {
            let tab_id =
                requested_tab.ok_or(("INVALID_REQUEST".to_owned(), "缺少 tabId。".to_owned()))?;
            if !session.tabs.contains_key(&tab_id) {
                return Err((
                    "TAB_NOT_FOUND".to_owned(),
                    format!("找不到标签页 {tab_id}。"),
                ));
            }
            session
                .activate_agent_tab(&tab_id)
                .map_err(|error| ("CDP_ERROR".to_owned(), error))?;
            result.tab = Some(tab_view(session, &tab_id).await?);
            return finish_operation(result, operation);
        }
        "browser.release_tab" => {
            if session.tabs.contains_key(&session.visible_tab_id) { session.active_tab_id = session.visible_tab_id.clone(); }
            return finish_operation(result, operation);
        }
        "browser.close_tab" => {
            let tab_id =
                requested_tab.ok_or(("INVALID_REQUEST".to_owned(), "缺少 tabId。".to_owned()))?;
            session
                .close_tab(&tab_id)
                .await
                .map_err(|error| ("CDP_ERROR".to_owned(), error))?;
            result.tab = None;
            return finish_operation(result, operation);
        }
        _ => {}
    }
    // 列表和管理操作允许零网页；导航或搜索才按需创建默认页。
    if requested_tab.is_none() && session.tabs.is_empty() && matches!(operation, "tab.navigate" | "tab.search") { session.create_tab_for_request(&json!({})).await?; }
    let tab_id = requested_tab
        .clone()
        .unwrap_or_else(|| session.active_tab_id.clone());
    let tab = session.tabs.get_mut(&tab_id).ok_or_else(|| {
        (
            "TAB_NOT_FOUND".to_owned(),
            format!("找不到标签页 {tab_id}。"),
        )
    })?;
    let page = tab.page.clone();
    let generation = tab.page_generation;
    set_activity(app, operation, Some(tab_id.clone())).await;

    match operation {
        "tab.navigate" => {
            let url = params
                .get("url")
                .and_then(Value::as_str)
                .ok_or(("INVALID_REQUEST".to_owned(), "缺少 url。".to_owned()))?;
            let url = normalize_web_url(url).map_err(|error| ("INVALID_URL".to_owned(), error))?;
            page.goto(url.as_str()).await.map_err(cdp_error)?;
            bump_generation(session, &tab_id);
            result.tab = Some(tab_view(session, &tab_id).await?);
        }
        "tab.search" => {
            let query = params
                .get("query")
                .and_then(Value::as_str)
                .ok_or(("INVALID_REQUEST".to_owned(), "缺少 query。".to_owned()))?;
            let url = bing_search_url(query).map_err(|error| ("INVALID_URL".to_owned(), error))?;
            page.goto(url.as_str()).await.map_err(cdp_error)?;
            bump_generation(session, &tab_id);
            result.tab = Some(tab_view(session, &tab_id).await?);
        }
        "tab.reload" => {
            page.reload().await.map_err(cdp_error)?;
            bump_generation(session, &tab_id);
            result.tab = Some(tab_view(session, &tab_id).await?);
        }
        "tab.go_back" => {
            navigate_history(&page, -1).await?;
            bump_generation(session, &tab_id);
            result.tab = Some(tab_view(session, &tab_id).await?);
        }
        "tab.go_forward" => {
            navigate_history(&page, 1).await?;
            bump_generation(session, &tab_id);
            result.tab = Some(tab_view(session, &tab_id).await?);
        }
        "tab.stop_loading" => {
            page.execute(chromiumoxide::cdp::browser_protocol::page::StopLoadingParams::default())
                .await
                .map_err(cdp_error)?;
            result.tab = Some(tab_view(session, &tab_id).await?);
        }
        "tab.wait_for_load" => {
            page.wait_for_navigation().await.map_err(cdp_error)?;
            result.tab = Some(tab_view(session, &tab_id).await?);
        }
        "page.wait_for" => {
            let text = params
                .get("text")
                .and_then(Value::as_str)
                .ok_or(("INVALID_REQUEST".to_owned(), "缺少 text。".to_owned()))?;
            wait_for_text(&page, text).await?;
        }
        "page.text.snapshot" | "page.elements.snapshot" => {
            let text_offset = params
                .get("textOffset")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let mut snapshot = capture_snapshot(&page, text_offset).await?;
            snapshot.page_generation = generation;
            let element_offset = params
                .get("elementOffset")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            result.snapshot = Some(super::snapshot::project_model_snapshot(
                snapshot,
                operation,
                element_offset,
            ));
        }
        "page.element.click"
        | "page.element.fill"
        | "page.element.type_text"
        | "page.element.press_key"
        | "page.element.select_option"
        | "page.element.set_checked"
        | "page.element.scroll_into_view" => {
            let reference = params.get("reference").and_then(Value::as_str).ok_or((
                "INVALID_REQUEST".to_owned(),
                "缺少元素 reference。".to_owned(),
            ))?;
            let element = find_element_by_reference(&page, reference).await?;
            let rect = element
                .bounding_box()
                .await
                .ok()
                .map(|box_point| BrowserRect {
                    x: box_point.x,
                    y: box_point.y,
                    width: box_point.width,
                    height: box_point.height,
                });
            record_receipt(app, &tab_id, operation, &page, rect.as_ref()).await;
            match operation {
                "page.element.click" => {
                    element.scroll_into_view().await.map_err(cdp_error)?;
                    element.click().await.map_err(cdp_error)?;
                }
                "page.element.fill" | "page.element.type_text" => {
                    let text = params
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    element.focus().await.map_err(cdp_error)?;
                    if operation == "page.element.fill" {
                        // fill 替换已有值；type_text 保留追加语义。用原生输入事件兼容 React 表单。
                        super::input::send_input(
                            &page,
                            super::BrowserUserInput::Key {
                                key: "a".to_owned(),
                                modifiers: 2,
                            },
                        )
                        .await
                        .map_err(|error| ("CDP_ERROR".to_owned(), error))?;
                        element.press_key("Backspace").await.map_err(cdp_error)?;
                    }
                    // type_str 只识别预定义键名，中文会报 Key not found；InsertText 支持任意文字。
                    super::input::send_input(
                        &page,
                        super::BrowserUserInput::Text {
                            text: text.to_owned(),
                        },
                    )
                    .await
                    .map_err(|error| ("CDP_ERROR".to_owned(), error))?;
                }
                "page.element.press_key" => {
                    let key = params.get("key").and_then(Value::as_str).unwrap_or("Enter");
                    element.focus().await.map_err(cdp_error)?;
                    element.press_key(key).await.map_err(cdp_error)?;
                }
                "page.element.select_option" => {
                    let value = params
                        .get("value")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let value_json = serde_json::json!(value).to_string();
                    element
                        .call_js_fn(format!(
                            "function() {{ const option = Array.from(this.options).find((option) => option.value === {value_json} || option.text === {value_json}); this.value = option ? option.value : {value_json}; this.dispatchEvent(new Event('input', {{ bubbles: true }})); this.dispatchEvent(new Event('change', {{ bubbles: true }})); }}"
                        ), false)
                        .await
                        .map_err(cdp_error)?;
                }
                "page.element.set_checked" => {
                    let checked = params
                        .get("checked")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                    element
                        .call_js_fn(format!(
                            "function() {{ if (this.checked !== {checked}) {{ this.click(); }} else {{ this.dispatchEvent(new Event('change', {{ bubbles: true }})); }} }}"
                        ), false)
                        .await
                        .map_err(cdp_error)?;
                }
                "page.element.scroll_into_view" => {
                    element.scroll_into_view().await.map_err(cdp_error)?;
                }
                _ => {}
            }
            bump_generation(session, &tab_id);
            result.tab = Some(tab_view(session, &tab_id).await?);
        }
        _ => {
            return Err((
                "UNKNOWN_OPERATION".to_owned(),
                "未知浏览器操作。".to_owned(),
            ))
        }
    }
    finish_operation(result, operation)
}

/** 等待页面出现指定文本；最多 15 秒，每 250ms 轮询一次正文。 */
async fn wait_for_text(page: &chromiumoxide::Page, text: &str) -> Result<(), (String, String)> {
    let deadline = Instant::now() + Duration::from_secs(15);
    let needle = text.to_owned();
    while Instant::now() < deadline {
        let body = page.evaluate("document.body?.innerText || ''").await;
        if let Ok(result) = body {
            if let Ok(value) = result.into_value::<String>() {
                if value.contains(&needle) {
                    return Ok(());
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err((
        "WAIT_TIMEOUT".to_owned(),
        format!("等待文本「{text}」超时。"),
    ))
}

/** 通过 CDP 导航历史前进/后退；offset 为 -1（后退）或 1（前进）。 */
async fn navigate_history(page: &chromiumoxide::Page, offset: i64) -> Result<(), (String, String)> {
    use chromiumoxide::cdp::browser_protocol::page::{
        GetNavigationHistoryParams, NavigateToHistoryEntryParams,
    };
    let history = page
        .execute(GetNavigationHistoryParams {})
        .await
        .map_err(cdp_error)?;
    let index = history.result.current_index + offset;
    if index < 0 {
        return Err(("HISTORY_BOUND".to_owned(), "已经是最早的页面。".to_owned()));
    }
    let Some(entry) = history.result.entries.get(index as usize) else {
        return Err(("HISTORY_BOUND".to_owned(), "已经是最新的页面。".to_owned()));
    };
    page.execute(NavigateToHistoryEntryParams::new(entry.id))
        .await
        .map_err(cdp_error)?;
    Ok(())
}

fn bump_generation(session: &mut BrowserSessionState, tab_id: &str) {
    if let Some(tab) = session.tabs.get_mut(tab_id) {
        tab.page_generation += 1;
    }
}

async fn tab_view(
    session: &BrowserSessionState,
    tab_id: &str,
) -> Result<BrowserTabView, (String, String)> {
    let tab = session
        .tabs
        .get(tab_id)
        .ok_or(("TAB_NOT_FOUND".to_owned(), "标签页不存在。".to_owned()))?;
    let (url, title) = tab_identity(&tab.page).await;
    Ok(BrowserTabView {
        tab_id: tab_id.to_owned(),
        url,
        title,
        active: session.active_tab_id == tab_id,
        visible: session.visible_tab_id == tab_id,
    })
}

/** 包装成功结果：统一 summary 文案。 */
fn finish_operation(mut result: BrowserRunResult, operation: &str) -> OperationResult {
    result.ok = true;
    let summary = if result.error.is_some() {
        result
            .error
            .as_ref()
            .map(|error| error.message.clone())
            .unwrap_or_default()
    } else {
        format!("浏览器操作 {operation} 已完成。")
    };
    let payload = serde_json::to_value(result)
        .map_err(|error| ("INTERNAL_ERROR".to_owned(), error.to_string()))?;
    Ok(ToolExecutionOutcome {
        result: ToolExecutionResultPayload {
            success: true,
            summary,
            payload,
            citations: Vec::new(),
            audit_fragment: None,
        },
    })
}

/// `snapshot_reference_matches` 保留给后续输入校验增强；这里提前引用避免 dead_code。
#[allow(dead_code)]
fn reference_matches_current(reference: &str, snapshot_id: &str) -> bool {
    snapshot_reference_matches(reference, snapshot_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /** 构造一个带独立 control 状态的管理器，避免测试间共享全局单例。 */
    fn test_manager() -> BrowserManager {
        BrowserManager::default()
    }

    #[tokio::test]
    async fn begin_control_allows_same_session_and_rejects_others() {
        let manager = test_manager();
        assert!(begin_control(&manager, "session-a").await.is_ok());
        assert!(begin_control(&manager, "session-a").await.is_ok());
        let error = begin_control(&manager, "session-b").await.unwrap_err();
        assert_eq!(error, "BROWSER_CONTROL_BUSY");
        end_control(&manager, "session-a").await;
        // 一轮内跨工具调用保留所有权；回合完成后才允许另一个会话申请。
        manager.control.lock().await.session_id = None;
        assert!(begin_control(&manager, "session-b").await.is_ok());
    }

    #[tokio::test]
    async fn user_take_over_blocks_owner_until_turn_finishes() {
        let manager = test_manager();
        assert!(begin_control(&manager, "session-a").await.is_ok());
        end_control(&manager, "session-a").await;

        assert!(revoke_control(&manager, None).await);

        let error = begin_control(&manager, "session-a").await.unwrap_err();
        assert_eq!(error, "BROWSER_USER_TAKEOVER");
        finish_control(&manager, "session-a").await;
        assert!(begin_control(&manager, "session-a").await.is_ok());
        finish_control(&manager, "session-a").await;
        // 其他会话不受影响。
        assert!(begin_control(&manager, "session-b").await.is_ok());
    }

    #[tokio::test]
    async fn take_over_clears_active_lease_and_bumps_revision() {
        let manager = test_manager();
        assert!(begin_control(&manager, "session-a").await.is_ok());
        {
            let control = manager.control.lock().await;
            assert_eq!(control.session_id.as_deref(), Some("session-a"));
        }
        let mut revision = manager.takeover_revision.subscribe();
        assert!(revoke_control(&manager, None).await);
        assert!(revision.has_changed().unwrap());
        assert_eq!(*revision.borrow_and_update(), 1);
        let control = manager.control.lock().await;
        assert!(control.session_id.is_none());
        assert_eq!(control.revision, 1);
        assert!(control.blocked_sessions.contains("session-a"));
    }

    #[test]
    fn failed_run_uses_structured_error_payload() {
        let outcome = failed_run("SOME_CODE", "出错了。");
        assert!(!outcome.result.success);
        assert_eq!(outcome.result.summary, "出错了。");
        assert_eq!(outcome.result.payload["error"]["code"], "SOME_CODE");
        let (success, summary, payload, _citations, _audit) = outcome.into_parts();
        assert!(!success);
        assert_eq!(summary, "出错了。");
        assert_eq!(payload["error"]["code"], "SOME_CODE");
    }

    #[test]
    fn browser_run_request_parses_kind_and_params() {
        let args: Value = serde_json::json!({ "kind": "browser", "operation": "tab.navigate", "url": "https://example.com" });
        let request: BrowserRunRequest = serde_json::from_value(args).unwrap();
        assert_eq!(request.kind.as_deref(), Some("browser"));
        assert_eq!(request.operation, "tab.navigate");
        assert_eq!(request.params["url"], "https://example.com");
    }

    #[test]
    fn browser_run_request_rejects_skill_args_without_operation() {
        // runtime 只在 kind="browser" 时调用浏览器分支；skill 参数缺 operation，
        // 应该被拒绝而不是误解析，保证两条分支互不串扰。
        let args: Value = serde_json::json!({ "skillId": "demo" });
        assert!(serde_json::from_value::<BrowserRunRequest>(args).is_err());
    }

    #[test]
    fn session_gate_matches_policy() {
        // 与 policy::session_allows_browser 联动，防止运行时和策略定义漂移。
        assert!(session_allows_browser(false, false, false, "full-confirm"));
        assert!(!session_allows_browser(true, false, false, "full-auto"));
        assert!(!session_allows_browser(false, false, false, "workspace-readonly"));
    }
}
