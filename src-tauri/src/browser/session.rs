//! Chromium 进程、真实标签页和画面流的生命周期；不涉及 Agent 权限或模型输入。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::page::{
    CaptureScreenshotFormat, CaptureScreenshotParams, EventScreencastFrame,
    ScreencastFrameAckParams, StartScreencastFormat, StartScreencastParams, StopScreencastParams,
};
use chromiumoxide::handler::viewport::Viewport;
use futures_util::StreamExt;

use super::types::BrowserFrame;

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
        let config = BrowserConfig::builder()
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
            .arg("--disable-extensions")
            .arg("--disable-sync")
            .arg("--disable-features=Translate,MediaRouter,PasswordLeakDetection")
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--mute-audio")
            .build()
            .map_err(|error| format!("浏览器配置无效：{error}"))?;
        let (browser, mut handler) = Browser::launch(config)
            .await
            .map_err(|error| format!("无法启动独立 Chromium：{error}"))?;
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
        };
        if let Err(error) = session.sync_tabs(true).await {
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
        // 两个选中标识必须指向存活标签；按需补建空白页后同步选择。
        if !self.tabs.contains_key(&self.active_tab_id) {
            self.active_tab_id = self.tabs.keys().min().cloned().unwrap_or_default();
        }
        if !self.tabs.contains_key(&self.visible_tab_id) {
            self.visible_tab_id = self.active_tab_id.clone();
        }
        Ok(())
    }

    pub async fn create_tab(&mut self) -> Result<String, String> {
        let page = self
            .browser
            .new_page("about:blank")
            .await
            .map_err(|error| error.to_string())?;
        let id = page.target_id().as_ref().to_owned();
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
        let id = self
            .create_tab()
            .await
            .map_err(|error| ("CDP_ERROR".to_owned(), error))?;
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
        self.active_tab_id = tab_id.to_owned();
        self.visible_tab_id = tab_id.to_owned();
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
        if self.tabs.is_empty() {
            self.create_tab().await?;
        } else if !self.tabs.contains_key(&self.active_tab_id)
            || !self.tabs.contains_key(&self.visible_tab_id)
        {
            let next = self.tabs.keys().min().cloned().unwrap();
            self.activate_tab(&next).await?;
        }
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
        self.tabs.clear();
        self.handler_task.abort();
    }
}

impl Drop for BrowserSessionState {
    fn drop(&mut self) {
        if let Some(task) = self.frame_task.take() {
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
    use std::io::{Read, Write};
    use std::time::Instant;

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
        assert_eq!(session.visible_tab_id, created);
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
        assert_eq!(session.visible_tab_id, first);

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
        assert_eq!(
            session.tabs.len(),
            1,
            "closing the last page must create a usable blank page"
        );
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
        assert!(!restarted.tabs.is_empty());
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
}
