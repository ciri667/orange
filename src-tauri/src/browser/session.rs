//! Chromium 进程、真实标签页和画面流的生命周期；不涉及 Agent 权限或模型输入。

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::page::{
    CaptureScreenshotFormat, CaptureScreenshotParams, EventJavascriptDialogClosed,
    EventJavascriptDialogOpening, EventScreencastFrame, ScreencastFrameAckParams,
    StartScreencastFormat, StartScreencastParams, StopScreencastParams,
};
use chromiumoxide::handler::viewport::Viewport;
use chromiumoxide::handler::{Handler, HandlerConfig};
use futures_util::StreamExt;
use tokio::sync::Mutex;

use super::policy::MAX_ELEMENT_TEXT_CHARS;
use super::types::{BrowserDialog, BrowserFrame};
use crate::terminal::process_guard::{attach_owned_child, ProcessGuard};

pub(super) const VIEWPORT_WIDTH: u32 = 1280;
pub(super) const VIEWPORT_HEIGHT: u32 = 860;
pub(super) type FrameSink = Arc<dyn Fn(BrowserFrame) + Send + Sync>;

/** 一个真实 CDP 标签页；代计数用于使旧元素引用失效。 */
pub(super) struct BrowserTab {
    pub page: chromiumoxide::Page,
    pub page_generation: u64,
}

/** 唯一拥有 Chromium 进程，关闭时必须等待进程退出再释放 profile。 */
pub(super) struct BrowserSessionState {
    pub browser: Browser,
    handler_task: tokio::task::JoinHandle<()>,
    pub tabs: HashMap<String, BrowserTab>,
    pub active_tab_id: String,
    pub visible_tab_id: String,
    frame_task: Option<tokio::task::JoinHandle<()>>,
    frame_tab_id: Option<String>,
    /// Chromium 真正切到前台的页；后台页不产生渲染帧，元素操作前必须对齐它。
    front_tab_id: String,
    /// 页面弹窗由事件泵写入；模型据此决定 accept 还是 dismiss。
    pub dialogs: Arc<Mutex<HashMap<String, BrowserDialog>>>,
    dialog_watched: HashSet<String>,
    dialog_tasks: Vec<tokio::task::JoinHandle<()>>,
    /// 只有自己启动的实例才挂 Job；接管来的实例由它原本的进程归属决定。
    process_guard: Option<ProcessGuard>,
}

/** 读 profile 里的 DevToolsActivePort 端口；文件缺失说明上次是正常退出，没有残留实例。 */
async fn active_port(profile: &Path) -> Option<u16> {
    let contents = tokio::fs::read_to_string(profile.join("DevToolsActivePort"))
        .await
        .ok()?;
    parse_active_port(&contents)
}

/** DevToolsActivePort 第一行是调试端口；0 或非数字都表示没有可用端点。 */
fn parse_active_port(contents: &str) -> Option<u16> {
    let port = contents.lines().next()?.trim().parse::<u16>().ok()?;
    (port > 0).then_some(port)
}

/** 端口上的调试端点是否仍在应答；只探测连通性，不建立 CDP 会话。 */
async fn devtools_endpoint_alive(port: u16) -> bool {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    tokio::task::spawn_blocking(move || {
        std::net::TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_ok()
    })
    .await
    .unwrap_or(false)
}

/** 接管上次遗留的实例：连不上说明端口已随进程消失，按正常流程重新启动。 */
async fn attach_running(profile: &Path) -> Option<(Browser, Handler)> {
    let port = active_port(profile).await?;
    let url = format!("http://127.0.0.1:{port}");
    // 视口和超时沿用自启实例的配置，接管后画面尺寸与超时行为保持一致。
    let config = HandlerConfig {
        viewport: Some(Viewport {
            width: VIEWPORT_WIDTH,
            height: VIEWPORT_HEIGHT,
            ..Viewport::default()
        }),
        request_timeout: Duration::from_secs(15),
        ..HandlerConfig::default()
    };
    match tokio::time::timeout(
        Duration::from_secs(5),
        Browser::connect_with_config(url, config),
    )
    .await
    {
        Ok(Ok(pair)) => Some(pair),
        Ok(Err(error)) => {
            log::info!("浏览器接管残留实例失败，改为重新启动：{error}");
            None
        }
        Err(_) => {
            log::warn!("浏览器接管残留实例超时，改为重新启动");
            None
        }
    }
}

/** 启动失败说明：端口还有实例应答时给出可操作下一步，而不是把 CDP 原文甩给用户和模型。 */
async fn describe_launch_failure(profile: &Path, detail: &str) -> String {
    if let Some(port) = active_port(profile).await {
        if devtools_endpoint_alive(port).await {
            return format!(
                "内置浏览器内核仍被另一个 Chromium 实例占用：profile 目录被锁住，调试端口 {port} 仍在应答。\
                 请先结束残留的 chrome.exe（路径含 app.orange.desktop/browser/chromium），再点「重试打开」；\
                 刚结束过的话等一两秒让它彻底退出。原始错误：{detail}"
            );
        }
    }
    format!("无法启动独立 Chromium：{detail}")
}

/** 独立 Chromium 的启动配置：new headless、固定视口，禁止隐式开窗。 */
fn build_config(executable: PathBuf, profile: PathBuf) -> Result<BrowserConfig, String> {
    BrowserConfig::builder()
        .chrome_executable(executable)
        .user_data_dir(profile)
        .new_headless_mode()
        .respect_https_errors()
        .window_size(VIEWPORT_WIDTH, VIEWPORT_HEIGHT)
        .viewport(Viewport {
            width: VIEWPORT_WIDTH,
            height: VIEWPORT_HEIGHT,
            ..Viewport::default()
        })
        .request_timeout(Duration::from_secs(15))
        // chromiumoxide 自动添加 `--`，带值参数用键值对传入；否则启动开关会失效。
        .arg("disable-extensions")
        .arg("disable-sync")
        .arg((
            "disable-features",
            "Translate,MediaRouter,PasswordLeakDetection",
        ))
        .arg("no-first-run")
        .arg("no-default-browser-check")
        // 首次点击只由 create_tab 建页，禁止 Chromium 额外打开默认 New Tab。
        .arg("no-startup-window")
        .arg("mute-audio")
        .build()
        .map_err(|error| format!("浏览器配置无效：{error}"))
}

impl BrowserSessionState {
    /** 新版 Chromium 使用 new headless；视口与画面、输入坐标共用同一尺寸。 */
    pub async fn launch(executable: PathBuf, profile: PathBuf) -> Result<Self, String> {
        if !executable.is_file() {
            return Err("独立 Chromium 尚未安装。请在设置中先下载浏览器内核。".to_owned());
        }
        tokio::fs::create_dir_all(&profile)
            .await
            .map_err(|error| format!("无法创建浏览器数据目录：{error}"))?;
        // 一个 profile 只能有一个 Chromium 实例。上次异常退出（崩溃、dev 重编译、被强杀）会留下孤儿，
        // 这时直接启动会撞上单实例机制：新进程立刻退出、stderr 为空，用户和模型都只看到「无法启动」。
        // 能连上就接管它（保留上次打开的网页），连不上再走正常启动。
        if let Some((browser, handler)) = attach_running(&profile).await {
            log::info!("浏览器已接管仍在运行的 Chromium 实例");
            return Self::assemble(browser, handler, None, true).await;
        }
        let config = build_config(executable.clone(), profile.clone())?;
        let (browser, handler) = match Browser::launch(config).await {
            Ok(pair) => pair,
            Err(error) => return Err(describe_launch_failure(&profile, &error.to_string()).await),
        };
        // 自己启动的 Chromium 挂进 Job：应用被强杀时由内核回收整棵进程树，不再留孤儿。
        let guard = attach_owned_child(std::process::id(), &executable);
        Self::assemble(browser, handler, guard, false).await
    }

    /**
     * 事件泵、初始页签同步和进程归属都在这里收口。
     * 接管已有实例时要先补一次 target 快照，否则 handler 只跟踪连接之后新建的页面。
     */
    async fn assemble(
        browser: Browser,
        mut handler: Handler,
        process_guard: Option<ProcessGuard>,
        import_existing_targets: bool,
    ) -> Result<Self, String> {
        // CDP 事件泵必须持续运行，关闭和等待进程退出完成后才停止它。
        let handler_task = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if let Err(error) = event {
                    log::warn!("浏览器 CDP 连接结束：{error}");
                    break;
                }
            }
        });
        let mut session = Self {
            browser,
            handler_task,
            tabs: HashMap::new(),
            active_tab_id: String::new(),
            visible_tab_id: String::new(),
            frame_task: None,
            frame_tab_id: None,
            front_tab_id: String::new(),
            dialogs: Arc::new(Mutex::new(HashMap::new())),
            dialog_watched: HashSet::new(),
            dialog_tasks: Vec::new(),
            process_guard,
        };
        if import_existing_targets {
            if let Err(error) = session.browser.fetch_targets().await {
                log::warn!("浏览器接管实例后读取已有页签失败：{error}");
            }
        }
        if let Err(error) = session.sync_tabs(false).await {
            session.close().await;
            return Err(error);
        }
        Ok(session)
    }

    /** 采用真实 targetId，收录网页弹出的标签页并移除 Chromium 已关闭的标签页。 */
    pub async fn sync_tabs(&mut self, create_if_empty: bool) -> Result<(), String> {
        let pages = self
            .browser
            .pages()
            .await
            .map_err(|error| error.to_string())?;
        let ids: std::collections::HashSet<String> = pages
            .iter()
            .map(|page| page.target_id().as_ref().to_owned())
            .collect();
        self.tabs.retain(|id, _| ids.contains(id));
        for page in pages {
            self.tabs
                .entry(page.target_id().as_ref().to_owned())
                .or_insert(BrowserTab {
                    page,
                    page_generation: 0,
                });
        }
        if create_if_empty && self.tabs.is_empty() {
            self.create_tab().await?;
        }
        self.watch_unwatched_dialogs();
        // 两个选中标识必须指向存活标签；按需补建空白页后同步选择。
        if !self.tabs.contains_key(&self.active_tab_id) {
            self.active_tab_id = self.tabs.keys().min().cloned().unwrap_or_default();
        }
        if !self.tabs.contains_key(&self.visible_tab_id) { self.visible_tab_id.clear(); }
        if !self.tabs.contains_key(&self.front_tab_id) { self.front_tab_id.clear(); }
        Ok(())
    }

    pub async fn create_tab(&mut self) -> Result<String, String> {
        let page = self
            .browser
            .new_page("about:blank")
            .await
            .map_err(|error| error.to_string())?;
        let id = page.target_id().as_ref().to_owned();
        self.watch_dialogs(id.clone(), page.clone());
        self.tabs.insert(
            id.clone(),
            BrowserTab {
                page,
                page_generation: 0,
            },
        );
        self.activate_tab(&id).await?;
        Ok(id)
    }

    /** 给尚未订阅的标签页挂弹窗监听；sync_tabs 每次都会调用，靠 dialog_watched 去重。 */
    fn watch_unwatched_dialogs(&mut self) {
        let pending: Vec<(String, chromiumoxide::Page)> = self
            .tabs
            .iter()
            .filter(|(id, _)| !self.dialog_watched.contains(*id))
            .map(|(id, tab)| (id.clone(), tab.page.clone()))
            .collect();
        for (tab_id, page) in pending {
            self.watch_dialogs(tab_id, page);
        }
    }

    /**
     * 订阅 JavaScript 弹窗事件。
     *
     * alert / confirm / prompt 会阻塞渲染进程，绝大多数 CDP 调用一起吊住；
     * 事件缓存让模型先知道「有弹窗、是哪种、说了什么」，再用页面操作解开。
     */
    fn watch_dialogs(&mut self, tab_id: String, page: chromiumoxide::Page) {
        if !self.dialog_watched.insert(tab_id.clone()) {
            return;
        }
        let dialogs = self.dialogs.clone();
        let handle = tokio::spawn(async move {
            let (Ok(mut opened), Ok(mut closed)) = (
                page.event_listener::<EventJavascriptDialogOpening>().await,
                page.event_listener::<EventJavascriptDialogClosed>().await,
            ) else {
                return;
            };
            loop {
                tokio::select! {
                    event = opened.next() => {
                        let Some(event) = event else { break };
                        let dialog = BrowserDialog {
                            tab_id: tab_id.clone(),
                            // 事件里的 enum 直接取 Debug 名再归一，避免依赖生成代码的变体拼写。
                            dialog_type: format!("{:?}", event.r#type).to_lowercase(),
                            message: event.message.chars().take(MAX_ELEMENT_TEXT_CHARS).collect(),
                            default_prompt: event.default_prompt.clone(),
                        };
                        dialogs.lock().await.insert(tab_id.clone(), dialog);
                    }
                    event = closed.next() => {
                        if event.is_none() { break }
                        dialogs.lock().await.remove(&tab_id);
                    }
                }
            }
        });
        self.dialog_tasks.push(handle);
    }

    /** 当前已知的页面弹窗；模型据此决定 accept 还是 dismiss。 */
    pub async fn open_dialogs(&self) -> Vec<BrowserDialog> {
        self.dialogs.lock().await.values().cloned().collect()
    }

    /** 处理完弹窗就清掉本地记录，不必等 dialogClosed 事件。 */
    pub async fn forget_dialog(&self, tab_id: &str) {
        self.dialogs.lock().await.remove(tab_id);
    }

    /** Agent 创建标签时先校验可选地址，再导航；非法地址不得留下空白标签。 */
    pub async fn create_tab_for_request(
        &mut self,
        params: &serde_json::Value,
    ) -> Result<String, (String, String)> {
        let url = params
            .get("url")
            .map(|value| {
                let value = value
                    .as_str()
                    .ok_or_else(|| ("INVALID_URL".to_owned(), "url 必须是字符串。".to_owned()))?;
                super::policy::normalize_web_url(value)
                    .map_err(|error| ("INVALID_URL".to_owned(), error))
            })
            .transpose()?;
        let previous_visible = self.visible_tab_id.clone();
        let id = self
            .create_tab()
            .await
            .map_err(|error| ("CDP_ERROR".to_owned(), error))?;
        // Agent 新建页面只改变操作目标，不能切走用户正在观察的网页。
        self.visible_tab_id = previous_visible;
        if let Some(url) = url {
            let tab = self.tabs.get_mut(&id).unwrap();
            // 等待导航完成后才返回 URL 和标题，避免把 about:blank 报成目标页。
            tab.page
                .goto(url)
                .await
                .map_err(|error| ("CDP_ERROR".to_owned(), error.to_string()))?;
            tab.page_generation = tab.page_generation.saturating_add(1);
        }
        Ok(id)
    }

    /** 真实激活目标页面，让导航、输入和画面始终指向同一个标签。 */
    pub async fn activate_tab(&mut self, tab_id: &str) -> Result<(), String> {
        let tab = self
            .tabs
            .get(tab_id)
            .ok_or_else(|| "标签页已关闭。".to_owned())?;
        tab.page
            .bring_to_front()
            .await
            .map_err(|error| error.to_string())?;
        self.front_tab_id = tab_id.to_owned();
        self.active_tab_id = tab_id.to_owned();
        self.visible_tab_id = tab_id.to_owned();
        Ok(())
    }

    /** 查看页面仅切换画面来源，不改变 Agent 的默认操作目标。 */
    pub async fn show_tab(&mut self, tab_id: &str) -> Result<(), String> {
        let tab = self.tabs.get(tab_id).ok_or_else(|| "标签页已关闭。".to_owned())?;
        tab.page.bring_to_front().await.map_err(|error| error.to_string())?;
        self.front_tab_id = tab_id.to_owned();
        self.visible_tab_id = tab_id.to_owned();
        Ok(())
    }

    /**
     * 把目标页真的切到 Chromium 前台。
     *
     * 后台标签页不产生渲染帧：布局类、IntersectionObserver 类 CDP 调用会一直不返回，
     * 只能等 chromiumoxide 的请求超时。任何会碰页面的操作都要先走这一步。
     */
    pub async fn ensure_front(&mut self, tab_id: &str) -> Result<(), String> {
        if self.front_tab_id == tab_id {
            return Ok(());
        }
        let page = self
            .tabs
            .get(tab_id)
            .ok_or_else(|| "标签页已关闭。".to_owned())?
            .page
            .clone();
        page.bring_to_front()
            .await
            .map_err(|error| error.to_string())?;
        self.front_tab_id = tab_id.to_owned();
        Ok(())
    }

    /** Agent 切换自己的目标页不影响用户选择的画面，但必须同时切换前台页。 */
    pub async fn activate_agent_tab(&mut self, tab_id: &str) -> Result<(), String> {
        if !self.tabs.contains_key(tab_id) { return Err("标签页已关闭。".to_owned()); }
        self.ensure_front(tab_id).await?;
        self.active_tab_id = tab_id.to_owned();
        Ok(())
    }

    /** 先成功关闭实际页面再更新索引；关掉最后一页后自动创建空白页。 */
    pub async fn close_tab(&mut self, tab_id: &str) -> Result<(), String> {
        let page = self
            .tabs
            .get(tab_id)
            .ok_or_else(|| "标签页已关闭。".to_owned())?
            .page
            .clone();
        page.close().await.map_err(|error| error.to_string())?;
        self.tabs.remove(tab_id);
        self.dialog_watched.remove(tab_id);
        self.dialogs.lock().await.remove(tab_id);
        if !self.tabs.contains_key(&self.active_tab_id) { self.active_tab_id = self.tabs.keys().min().cloned().unwrap_or_default(); }
        if !self.tabs.contains_key(&self.visible_tab_id) { self.visible_tab_id.clear(); }
        if !self.tabs.contains_key(&self.front_tab_id) { self.front_tab_id.clear(); }
        Ok(())
    }

    /** 每次最多保留一个转发任务；切换标签或关闭画面会停止旧流。 */
    pub async fn set_screencast(&mut self, enabled: bool, sink: FrameSink) -> Result<(), String> {
        let target = enabled.then(|| self.visible_tab_id.clone());
        if self.frame_tab_id == target
            && self
                .frame_task
                .as_ref()
                .is_some_and(|task| !task.is_finished())
        {
            return Ok(());
        }
        if let Some(task) = self.frame_task.take() {
            task.abort();
        }
        if let Some(id) = self.frame_tab_id.take() {
            if let Some(tab) = self.tabs.get(&id) {
                let _ = tab.page.execute(StopScreencastParams {}).await;
            }
        }
        let Some(tab_id) = target.filter(|id| !id.is_empty()) else {
            return Ok(());
        };
        let page = self
            .tabs
            .get(&tab_id)
            .ok_or_else(|| "标签页已关闭。".to_owned())?
            .page
            .clone();
        // 必须先订阅再启动；静态页面往往只发首帧，反过来会永远等不到画面。
        let mut events = page
            .event_listener::<EventScreencastFrame>()
            .await
            .map_err(|error| error.to_string())?;
        page.execute(
            StartScreencastParams::builder()
                .format(StartScreencastFormat::Jpeg)
                .quality(65)
                .max_width(VIEWPORT_WIDTH as i64)
                .max_height(VIEWPORT_HEIGHT as i64)
                .every_nth_frame(1)
                .build(),
        )
        .await
        .map_err(|error| error.to_string())?;
        let relay_page = page.clone();
        let relay_id = tab_id.clone();
        let relay_sink = sink.clone();
        self.frame_task = Some(tokio::spawn(async move {
            let mut pending_frame = None;
            let mut ticks = tokio::time::interval(Duration::from_millis(120));
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    event = events.next() => {
                        let Some(event) = event else { break; };
                        // 节流只合并中间帧，最后一帧在下个 tick 送出，静态页面不会停在旧画面。
                        pending_frame = Some(BrowserFrame {
                            tab_id: relay_id.clone(),
                            data: String::from_utf8_lossy(event.data.as_ref()).to_string(),
                            width: event.metadata.device_width as i64,
                            height: event.metadata.device_height as i64,
                        });
                        // 被合并的帧也必须 ack，否则 CDP 会停止继续推帧。
                        if relay_page.execute(ScreencastFrameAckParams::new(event.session_id)).await.is_err() {
                            break;
                        }
                    }
                    _ = ticks.tick() => {
                        if let Some(frame) = pending_frame.take() { relay_sink(frame); }
                    }
                }
            }
        }));
        self.frame_tab_id = Some(tab_id.clone());
        // 主动补一帧，静态页也可立即显示；失败不影响已启动的实时流。
        if let Ok(shot) = page
            .execute(
                CaptureScreenshotParams::builder()
                    .format(CaptureScreenshotFormat::Jpeg)
                    .quality(65)
                    .build(),
            )
            .await
        {
            sink(BrowserFrame {
                tab_id,
                data: String::from_utf8_lossy(shot.result.data.as_ref()).to_string(),
                width: VIEWPORT_WIDTH as i64,
                height: VIEWPORT_HEIGHT as i64,
            });
        }
        Ok(())
    }

    /** 保持事件泵直到 Browser.close + wait 完成，超时后只终止本会话拥有的进程。 */
    pub async fn close(&mut self) {
        if let Some(task) = self.frame_task.take() {
            task.abort();
        }
        let _ = tokio::time::timeout(Duration::from_secs(3), self.browser.close()).await;
        if !matches!(
            tokio::time::timeout(Duration::from_secs(5), self.browser.wait()).await,
            Ok(Ok(_))
        ) {
            let _ = self.browser.kill().await;
            let _ = tokio::time::timeout(Duration::from_secs(3), self.browser.wait()).await;
        }
        // Job 句柄一关就等于强杀整棵进程树；先显式结束，避免渲染进程比主进程活得更久。
        if let Some(guard) = self.process_guard.take() {
            guard.terminate();
        }
        // 弹窗监听任务随会话一起停，别在浏览器已经关掉后继续等事件。
        for task in self.dialog_tasks.drain(..) {
            task.abort();
        }
        self.dialog_watched.clear();
        self.dialogs.lock().await.clear();
        self.tabs.clear();
        self.handler_task.abort();
    }
}

impl Drop for BrowserSessionState {
    fn drop(&mut self) {
        if let Some(task) = self.frame_task.take() {
            task.abort();
        }
        for task in self.dialog_tasks.drain(..) {
            task.abort();
        }
        self.handler_task.abort();
    }
}

/** Windows 子进程退出后文件句柄可能稍晚释放；有限重试且只删除明确的 profile。 */
pub(super) async fn remove_profile(profile: &std::path::Path) -> Result<(), String> {
    for attempt in 0..20 {
        match tokio::fs::remove_dir_all(profile).await {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error)
                if attempt < 19
                    && (matches!(error.raw_os_error(), Some(32) | Some(33))
                        || matches!(
                            error.kind(),
                            std::io::ErrorKind::PermissionDenied
                                | std::io::ErrorKind::DirectoryNotEmpty
                        )) =>
            {
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
            Err(error) => return Err(format!("无法清除浏览器数据：{error}")),
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::super::input::{send_input, BrowserUserInput};
    use super::*;
    use base64::Engine;
    use futures_util::FutureExt;
    use std::io::{Read, Write};
    use std::time::Instant;

    /** 启动进程不能隐式创建 New Tab；首次和后续新建都只能增加一个空白页。 */
    #[tokio::test]
    #[ignore = "requires ORANGE_BROWSER_TEST_EXECUTABLE pointing to an installed Chromium"]
    async fn live_browser_only_opens_explicitly_created_tabs() {
        let executable = PathBuf::from(std::env::var("ORANGE_BROWSER_TEST_EXECUTABLE").unwrap());
        let directory = tempfile::tempdir().unwrap();
        let profile = directory.path().join("profile");
        // 复用测试 profile 再次启动，覆盖首次启动与已有会话数据两条路径。
        for _ in 0..2 {
            let mut session = BrowserSessionState::launch(executable.clone(), profile.clone())
                .await
                .unwrap();
            let result = async {
                assert!(
                    session.tabs.is_empty(),
                    "launch must not create a startup tab"
                );
                for expected in 1..=3 {
                    let id = session.create_tab().await.unwrap();
                    session.sync_tabs(false).await.unwrap();
                    assert_eq!(
                        session.tabs.len(),
                        expected,
                        "each request must create exactly one tab"
                    );
                    assert_eq!(session.active_tab_id, id);
                    assert_eq!(session.visible_tab_id, id);
                    for tab in session.tabs.values() {
                        assert_eq!(
                            tab.page.url().await.unwrap().as_deref(),
                            Some("about:blank")
                        );
                    }
                }
            };
            // 断言失败也关闭测试进程，避免临时 profile 被 Windows 文件句柄占用。
            let outcome = std::panic::AssertUnwindSafe(result).catch_unwind().await;
            session.close().await;
            if let Err(error) = outcome {
                std::panic::resume_unwind(error);
            }
        }
        remove_profile(&profile).await.unwrap();
    }

    /** 仅使用显式指定的已有内核和临时 profile，绝不下载或读取用户浏览器数据。 */
    #[tokio::test]
    #[ignore = "requires ORANGE_BROWSER_TEST_EXECUTABLE pointing to an installed Chromium"]
    async fn live_browser_navigates_streams_accepts_input_and_releases_profile() {
        let executable = PathBuf::from(std::env::var("ORANGE_BROWSER_TEST_EXECUTABLE").unwrap());
        let directory = tempfile::tempdir().unwrap();
        let profile = directory.path().join("profile");
        // 本地 HTTP 页面消除外部站点、代理和网络状态对生命周期验证的影响。
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let server_stop = stopped.clone();
        let server = std::thread::spawn(move || {
            while !server_stop.load(std::sync::atomic::Ordering::SeqCst) {
                if let Ok((mut stream, _)) = listener.accept() {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    let mut buffer = [0; 4096];
                    let _ = stream.read(&mut buffer);
                    let body = "<!doctype html><html><head><title>Orange browser fixture</title></head><body style='margin:0;background:white'><input id='field' style='position:absolute;left:20px;top:20px;width:200px;height:30px' value='original'><button id='button' style='position:absolute;left:20px;top:80px' onclick='this.textContent=\"clicked\"'>click</button><a href='/popup' target='_blank' id='popup'>popup</a><div style='height:2400px'></div></body></html>";
                    let reply = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    let _ = stream.write_all(reply.as_bytes());
                } else {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        });
        let mut session = BrowserSessionState::launch(executable.clone(), profile.clone())
            .await
            .unwrap();
        session.create_tab().await.unwrap();
        assert!(!session.active_tab_id.is_empty());
        assert!(session.tabs.contains_key(&session.visible_tab_id));
        let initial_tab = session.active_tab_id.clone();
        for url in [
            serde_json::json!("file:///secret.txt"),
            serde_json::json!(42),
            serde_json::json!(""),
        ] {
            let error = session
                .create_tab_for_request(&serde_json::json!({ "url": url }))
                .await
                .unwrap_err();
            assert_eq!(error.0, "INVALID_URL");
            assert_eq!(session.tabs.len(), 1, "invalid URLs must not create a tab");
        }
        let blank = session
            .create_tab_for_request(&serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(
            session.tabs[&blank].page.url().await.unwrap().as_deref(),
            Some("about:blank")
        );
        session.close_tab(&blank).await.unwrap();
        let destination = format!("http://{address}/");
        let created = session
            .create_tab_for_request(&serde_json::json!({ "url": destination }))
            .await
            .unwrap();
        assert_eq!(
            session.tabs[&created].page.url().await.unwrap().as_deref(),
            Some(destination.as_str())
        );
        assert_eq!(session.tabs[&created].page_generation, 1);
        assert_eq!(session.active_tab_id, created);
        assert_eq!(session.visible_tab_id, initial_tab);
        session.show_tab(&created).await.unwrap();
        session.close_tab(&initial_tab).await.unwrap();
        let first = session.active_tab_id.clone();
        let page = session.tabs[&first].page.clone();
        assert_eq!(
            page.get_title().await.unwrap().as_deref(),
            Some("Orange browser fixture")
        );
        let snapshot = super::super::manager::capture_snapshot(&page, 0)
            .await
            .unwrap();
        assert!(
            !snapshot.elements.is_empty(),
            "Agent snapshot must contain actual interactive elements"
        );
        assert!(snapshot
            .elements
            .iter()
            .any(|element| element.value.as_deref() == Some("original")));

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink: FrameSink = Arc::new(move |frame| {
            let _ = tx.send(frame);
        });
        session.set_screencast(true, sink.clone()).await.unwrap();
        let frame = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(frame.tab_id, first);
        let jpeg = base64::engine::general_purpose::STANDARD
            .decode(&frame.data)
            .unwrap();
        assert!(
            image::load_from_memory(&jpeg).is_ok(),
            "frame must be a valid JPEG"
        );
        assert_eq!(
            (frame.width, frame.height),
            (VIEWPORT_WIDTH as i64, VIEWPORT_HEIGHT as i64)
        );
        let task_id = session.frame_task.as_ref().unwrap().id();
        session.set_screencast(true, sink.clone()).await.unwrap();
        assert_eq!(
            session.frame_task.as_ref().unwrap().id(),
            task_id,
            "repeated enables must reuse one relay"
        );

        send_input(&page, BrowserUserInput::Click { x: 50.0, y: 35.0 })
            .await
            .unwrap();
        send_input(
            &page,
            BrowserUserInput::Key {
                key: "a".to_owned(),
                modifiers: 2,
            },
        )
        .await
        .unwrap();
        send_input(
            &page,
            BrowserUserInput::Key {
                key: "Backspace".to_owned(),
                modifiers: 0,
            },
        )
        .await
        .unwrap();
        send_input(
            &page,
            BrowserUserInput::Text {
                text: "中文输入成功".to_owned(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            page.evaluate("document.getElementById('field').value")
                .await
                .unwrap()
                .into_value::<String>()
                .unwrap(),
            "中文输入成功"
        );
        send_input(&page, BrowserUserInput::Click { x: 40.0, y: 90.0 })
            .await
            .unwrap();
        assert_eq!(
            page.evaluate("document.getElementById('button').textContent")
                .await
                .unwrap()
                .into_value::<String>()
                .unwrap(),
            "clicked"
        );
        send_input(
            &page,
            BrowserUserInput::Scroll {
                x: 400.0,
                y: 400.0,
                delta_x: 0.0,
                delta_y: 500.0,
            },
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(
            page.evaluate("window.scrollY")
                .await
                .unwrap()
                .into_value::<f64>()
                .unwrap()
                > 0.0
        );

        // 页面自己打开的标签也要被收录，关闭后不能留下无法切换的旧 targetId。
        page.find_element("#popup")
            .await
            .unwrap()
            .click()
            .await
            .unwrap();
        let popup_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            session.sync_tabs(true).await.unwrap();
            if session.tabs.len() == 2 {
                break;
            }
            assert!(
                Instant::now() < popup_deadline,
                "popup must appear in managed tabs"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let popup = session
            .tabs
            .keys()
            .find(|id| **id != first)
            .unwrap()
            .clone();
        session.activate_tab(&popup).await.unwrap();
        session.close_tab(&popup).await.unwrap();
        assert!(session.visible_tab_id.is_empty());
        session.show_tab(&first).await.unwrap();

        let second = session.create_tab().await.unwrap();
        assert_ne!(first, second);
        session.set_screencast(true, sink.clone()).await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let frame =
                tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), rx.recv())
                    .await
                    .unwrap()
                    .unwrap();
            if frame.tab_id == second {
                break;
            }
        }
        session.activate_tab(&first).await.unwrap();
        assert_eq!(session.visible_tab_id, first);
        session.close_tab(&second).await.unwrap();
        session.close_tab(&first).await.unwrap();
        assert!(session.tabs.is_empty(), "closing the last page must leave no replacement page");
        // Page.close 应答先于 targetDestroyed；等待事件泵移除旧 target 后再检查真实页签。
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                session.sync_tabs(false).await.unwrap();
                if session.tabs.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("closed targets must disappear after CDP close events");
        assert!(session.tabs.is_empty());
        session.create_tab().await.unwrap();
        session.set_screencast(false, sink.clone()).await.unwrap();
        assert!(session.frame_task.is_none());
        // 静态页重新开启也必须收到首帧，不能一直停在“等待页面画面”。
        while rx.try_recv().is_ok() {}
        session.set_screencast(true, sink.clone()).await.unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .is_some());
        session.set_screencast(false, sink).await.unwrap();
        session.close().await;
        assert!(session.browser.try_wait().unwrap().is_some());
        remove_profile(&profile).await.unwrap();
        assert!(
            !profile.exists(),
            "profile must be deletable after Chromium exits"
        );
        // 清除后可以重新启动，避免一次清除把浏览器永久留在空会话。
        let mut restarted = BrowserSessionState::launch(executable, profile.clone())
            .await
            .unwrap();
        assert!(restarted.tabs.is_empty());
        restarted.create_tab().await.unwrap();
        // 可选公网烟测仍使用临时 profile；默认回归仅依赖本地页面。
        if let Ok(url) = std::env::var("ORANGE_BROWSER_TEST_URL") {
            let page = &restarted.tabs[&restarted.active_tab_id].page;
            page.goto(url).await.unwrap();
            assert!(page
                .get_title()
                .await
                .unwrap()
                .is_some_and(|title| !title.is_empty()));
        }
        restarted.close().await;
        remove_profile(&profile).await.unwrap();
        stopped.store(true, std::sync::atomic::Ordering::SeqCst);
        server.join().unwrap();
    }

    /** 残留实例占着 profile 时必须接管，而不是让第二个 Chromium 直接退出把面板卡死。 */
    #[tokio::test]
    #[ignore = "requires ORANGE_BROWSER_TEST_EXECUTABLE pointing to an installed Chromium"]
    async fn live_browser_adopts_the_instance_still_holding_the_profile() {
        let executable = PathBuf::from(std::env::var("ORANGE_BROWSER_TEST_EXECUTABLE").unwrap());
        let directory = tempfile::tempdir().unwrap();
        let profile = directory.path().join("profile");
        // 第一次启动故意不 close，模拟应用被强杀后留下的孤儿实例。
        let mut leftover = BrowserSessionState::launch(executable.clone(), profile.clone())
            .await
            .unwrap();
        let tab = leftover.create_tab().await.unwrap();
        let mut adopted = BrowserSessionState::launch(executable, profile.clone())
            .await
            .unwrap();
        adopted.sync_tabs(false).await.unwrap();
        assert!(
            adopted.tabs.contains_key(&tab),
            "接管后必须能看到残留实例里已经打开的页签"
        );
        adopted.close().await;
        leftover.close().await;
        remove_profile(&profile).await.unwrap();
    }

    /** DevToolsActivePort 只有端口行有效；0、空文件和脏数据都不能当成可用端点。 */
    #[test]
    fn active_port_reads_only_a_valid_first_line() {
        assert_eq!(
            parse_active_port("55512\n/devtools/browser/stale\n"),
            Some(55512)
        );
        assert_eq!(parse_active_port("0\n/devtools/browser/stale"), None);
        assert_eq!(parse_active_port(""), None);
        assert_eq!(parse_active_port("not-a-port"), None);
    }

    /** 正常退出也会留下 DevToolsActivePort；端口关掉后不能误报成「被占用」。 */
    #[tokio::test]
    async fn launch_failure_only_blames_a_leftover_chromium_when_its_port_answers() {
        let directory = tempfile::tempdir().unwrap();
        let profile = directory.path();
        assert!(active_port(profile).await.is_none());
        assert_eq!(
            describe_launch_failure(profile, "ExitStatus(21)").await,
            "无法启动独立 Chromium：ExitStatus(21)"
        );

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::fs::write(
            profile.join("DevToolsActivePort"),
            format!("{port}\n/devtools/browser/stale\n"),
        )
        .unwrap();
        assert_eq!(active_port(profile).await, Some(port));
        let blocked = describe_launch_failure(profile, "ExitStatus(21)").await;
        assert!(
            blocked.contains("残留") && blocked.contains(&port.to_string()),
            "端口仍在应答时必须提示结束残留进程：{blocked}"
        );

        drop(listener);
        let recovered = describe_launch_failure(profile, "ExitStatus(21)").await;
        assert!(
            recovered.starts_with("无法启动独立 Chromium："),
            "端口关闭后要回到原始错误：{recovered}"
        );
    }
}
