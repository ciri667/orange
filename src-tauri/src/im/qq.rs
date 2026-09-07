use super::inbound::{self, ImInboundEvent};
use crate::domain::{ImGatewayStatus, ImProviderSettings, IM_PROVIDER_QQ};
use crate::logging::{self, AppEventBuilder, AppLogCategory, AppLogLevel};
use crate::storage::{self, format_local_datetime};
use reqwest::Client;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashSet, VecDeque};
use std::io::{BufRead, BufReader};
use std::process::Child;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::AppHandle;

/** QQ 发送限流间隔，避免触发官方主动消息配额。 */
const QQ_SEND_INTERVAL: Duration = Duration::from_millis(280);

/** 最近事件去重窗口大小。 */
const RECENT_EVENT_LIMIT: usize = 512;

/** QQ sidecar 配置，通过 stdin JSON 注入。 */
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct QqSidecarConfig {
    app_id: String,
    app_secret: String,
}

struct QqGatewayState {
    child: Option<Child>,
    running: bool,
    connected: bool,
    domain: String,
    app_id_configured: bool,
    secret_configured: bool,
    last_started_at: Option<String>,
    last_stopped_at: Option<String>,
    last_error: Option<String>,
    recent_event_ids: VecDeque<String>,
    recent_event_set: HashSet<String>,
    last_send_at: Option<Instant>,
    access_token: Option<String>,
    access_token_expire_at: Option<Instant>,
}

impl Default for QqGatewayState {
    fn default() -> Self {
        Self {
            child: None,
            running: false,
            connected: false,
            domain: "qq".to_owned(),
            app_id_configured: false,
            secret_configured: false,
            last_started_at: None,
            last_stopped_at: None,
            last_error: None,
            recent_event_ids: VecDeque::new(),
            recent_event_set: HashSet::new(),
            last_send_at: None,
            access_token: None,
            access_token_expire_at: None,
        }
    }
}

static QQ_GATEWAY_STATE: OnceLock<Mutex<QqGatewayState>> = OnceLock::new();

/** 启动 QQ 官方机器人 WebSocket 网关。 */
pub async fn start_gateway(app: AppHandle) -> Result<ImGatewayStatus, String> {
    let settings = load_qq_provider(&app)?;
    let app_secret = storage::load_im_provider_secret(IM_PROVIDER_QQ)?
        .ok_or_else(|| "请先保存 QQ AppSecret。".to_owned())?;
    validate_gateway_settings(&settings)?;
    let app_id = settings
        .to_qq_config()
        .map(|config| config.app_id.clone())
        .unwrap_or_default();

    let spawned = super::process::spawn_im_sidecar(
        &app,
        IM_PROVIDER_QQ,
        "qq-gateway",
        &QqSidecarConfig {
            app_id: app_id.clone(),
            app_secret,
        },
    )?;
    let mut state = lock_gateway_state()?;

    if let Some(mut old_child) = state.child.take() {
        let _ = old_child.kill();
    }

    state.running = true;
    state.connected = false;
    state.domain = "qq".to_owned();
    state.app_id_configured = !app_id.trim().is_empty();
    state.secret_configured = true;
    state.last_started_at = Some(format_local_datetime());
    state.last_stopped_at = None;
    state.last_error = None;
    state.child = Some(spawned.child);
    drop(state);

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Im,
            "im_gateway_start",
            "completed",
            "QQ 官方机器人网关已启动。",
        )
        .metadata(json!({
            "providerId": IM_PROVIDER_QQ,
            "knowledgeBaseCount": settings.default_knowledge_base_ids.len(),
            "allowedUserCount": settings.allowed_user_open_ids.len(),
            "allowedChatCount": settings.allowed_chat_ids.len(),
        })),
    );

    spawn_stdout_reader(app.clone(), spawned.stdout);
    super::process::spawn_stderr_reader(app.clone(), IM_PROVIDER_QQ.to_owned(), spawned.stderr);
    load_gateway_status(&app)
}

/** 停止 QQ 网关；不清空配置或凭证。 */
pub fn stop_gateway(app: &AppHandle) -> Result<ImGatewayStatus, String> {
    let mut state = lock_gateway_state()?;

    if let Some(mut child) = state.child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }

    state.running = false;
    state.connected = false;
    state.last_stopped_at = Some(format_local_datetime());
    state.access_token = None;
    state.access_token_expire_at = None;

    logging::write_app_event_best_effort(
        app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Im,
            "im_gateway_stop",
            "completed",
            "QQ 官方机器人网关已停止。",
        )
        .metadata(json!({ "providerId": IM_PROVIDER_QQ })),
    );

    Ok(state.to_status())
}

/** 读取 QQ 网关状态。 */
pub fn load_gateway_status(app: &AppHandle) -> Result<ImGatewayStatus, String> {
    let settings = load_qq_provider(app).ok();
    let secret_configured = storage::load_im_provider_credential_status(IM_PROVIDER_QQ)
        .map(|status| status.configured)
        .unwrap_or(false);
    let mut state = lock_gateway_state()?;
    state.domain = "qq".to_owned();
    state.app_id_configured = settings
        .as_ref()
        .and_then(ImProviderSettings::to_qq_config)
        .is_some_and(|config| !config.app_id.trim().is_empty());
    state.secret_configured = secret_configured;
    Ok(state.to_status())
}

impl QqGatewayState {
    fn to_status(&self) -> ImGatewayStatus {
        ImGatewayStatus {
            provider_id: IM_PROVIDER_QQ.to_owned(),
            running: self.running,
            connected: self.connected,
            domain: self.domain.clone(),
            app_id_configured: self.app_id_configured,
            secret_configured: self.secret_configured,
            last_started_at: self.last_started_at.clone(),
            last_stopped_at: self.last_stopped_at.clone(),
            last_error: self.last_error.clone(),
        }
    }
}

fn lock_gateway_state() -> Result<std::sync::MutexGuard<'static, QqGatewayState>, String> {
    QQ_GATEWAY_STATE
        .get_or_init(|| Mutex::new(QqGatewayState::default()))
        .lock()
        .map_err(|_| "QQ 网关状态锁已损坏。".to_owned())
}

fn load_qq_provider(app: &AppHandle) -> Result<ImProviderSettings, String> {
    storage::load_im_provider_settings(app, IM_PROVIDER_QQ)
}

fn validate_gateway_settings(settings: &ImProviderSettings) -> Result<(), String> {
    if !settings.enabled {
        return Err("请先启用 QQ 官方机器人集成。".to_owned());
    }
    let app_id = settings
        .to_qq_config()
        .map(|config| config.app_id.trim().to_owned())
        .unwrap_or_default();
    if app_id.is_empty() {
        return Err("请先填写 QQ App ID。".to_owned());
    }
    if settings.default_knowledge_base_ids.is_empty() {
        return Err("请至少选择一个 QQ 默认知识库范围。".to_owned());
    }
    Ok(())
}

fn spawn_stdout_reader(app: AppHandle, stdout: impl std::io::Read + Send + 'static) {
    tauri::async_runtime::spawn_blocking(move || {
        let reader = BufReader::new(stdout);

        for line in reader.lines() {
            let Ok(line) = line else {
                record_gateway_error(&app, "QQ sidecar stdout 读取失败。");
                break;
            };
            let trimmed_line = line.trim();
            if trimmed_line.is_empty() {
                continue;
            }
            if !trimmed_line.starts_with('{') {
                super::process::record_stdout_noise(&app, IM_PROVIDER_QQ, trimmed_line);
                continue;
            }

            match serde_json::from_str::<ImInboundEvent>(trimmed_line) {
                Ok(mut event) => {
                    event.kind = if event.kind.trim().is_empty() {
                        "message".to_owned()
                    } else {
                        event.kind
                    };
                    mark_gateway_connected(&app);
                    if !remember_event_id(&event.event_id) {
                        continue;
                    }
                    let event_app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        handle_inbound_event(event_app, event).await;
                    });
                }
                Err(error) => {
                    super::process::record_stdout_noise(
                        &app,
                        IM_PROVIDER_QQ,
                        &format!("QQ sidecar JSONL 事件格式无效：{error}"),
                    );
                }
            }
        }
    });
}

fn remember_event_id(event_id: &str) -> bool {
    let Ok(mut state) = lock_gateway_state() else {
        return true;
    };
    if state.recent_event_set.contains(event_id) {
        return false;
    }
    state.recent_event_ids.push_back(event_id.to_owned());
    state.recent_event_set.insert(event_id.to_owned());
    while state.recent_event_ids.len() > RECENT_EVENT_LIMIT {
        if let Some(removed_id) = state.recent_event_ids.pop_front() {
            state.recent_event_set.remove(&removed_id);
        }
    }
    true
}

fn mark_gateway_connected(app: &AppHandle) {
    if let Ok(mut state) = lock_gateway_state() {
        if state.connected {
            return;
        }
        state.connected = true;
        logging::write_app_event_best_effort(
            app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Im,
                "im_gateway_connected",
                "completed",
                "QQ 长连接已收到事件。",
            )
            .metadata(json!({ "providerId": IM_PROVIDER_QQ })),
        );
    }
}

fn record_gateway_error(app: &AppHandle, message: &str) {
    if let Ok(mut state) = lock_gateway_state() {
        state.last_error = Some(logging::sanitize_log_text(message));
        state.connected = false;
    }
    logging::write_app_event_best_effort(
        app,
        AppEventBuilder::new(
            AppLogLevel::Warn,
            AppLogCategory::Im,
            "im_gateway_disconnected",
            "failed",
            message,
        )
        .metadata(json!({ "providerId": IM_PROVIDER_QQ })),
    );
}

async fn handle_inbound_event(app: AppHandle, event: ImInboundEvent) {
    let started_at = Instant::now();
    let event_hash = inbound::hash_identifier(&event.event_id);
    let mut settings = match load_qq_provider(&app) {
        Ok(settings) => settings,
        Err(error) => {
            record_gateway_error(&app, &error);
            return;
        }
    };

    if event.kind == "discovery" {
        inbound::remember_discovered_peer_from_event(&app, IM_PROVIDER_QQ, &event, &event_hash);
        return;
    }

    inbound::remember_discovered_peer_from_event(&app, IM_PROVIDER_QQ, &event, &event_hash);
    if let Ok(next_settings) = load_qq_provider(&app) {
        settings = next_settings;
    }

    if let Err(block) = inbound::decide_event_handling(IM_PROVIDER_QQ, &settings, &event) {
        logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Im,
                "im_message_blocked",
                "blocked",
                block.reason,
            )
            .duration(started_at.elapsed())
            .metadata(inbound::block_metadata(
                IM_PROVIDER_QQ,
                &event,
                &settings,
                &event_hash,
            )),
        );
        return;
    }

    let reply = inbound::handle_authorized_event(
        app.clone(),
        IM_PROVIDER_QQ,
        event.clone(),
        settings.clone(),
        false,
    )
    .await;

    match send_text_reply(&app, &settings, &event, &reply).await {
        Ok(_) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Im,
                "im_reply_sent",
                "completed",
                "QQ 回复已发送。",
            )
            .duration(started_at.elapsed())
            .metadata(json!({
                "providerId": IM_PROVIDER_QQ,
                "eventHash": event_hash,
                "replyChars": reply.chars().count()
            })),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Im,
                "im_reply_failed",
                "failed",
                error,
            )
            .duration(started_at.elapsed())
            .metadata(json!({ "providerId": IM_PROVIDER_QQ, "eventHash": event_hash })),
        ),
    }
}

async fn send_text_reply(
    app: &AppHandle,
    settings: &ImProviderSettings,
    event: &ImInboundEvent,
    text: &str,
) -> Result<(), String> {
    let chunks = inbound::chunk_chars(text, inbound::QQ_REPLY_MAX_CHARS);
    let token = fetch_access_token(app, settings).await?;
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| format!("无法创建 QQ HTTP client：{error}"))?;
    let is_group = inbound::is_group_chat_event(event);
    let url = if is_group {
        format!(
            "https://api.sgroup.qq.com/v2/groups/{}/messages",
            event.chat_id
        )
    } else {
        format!(
            "https://api.sgroup.qq.com/v2/users/{}/messages",
            event.sender_open_id
        )
    };

    for (index, chunk) in chunks.iter().enumerate() {
        rate_limit_send().await?;
        let mut payload = json!({
            "content": chunk,
            "msg_type": 0,
            "msg_seq": index + 1,
        });
        if !event.message_id.trim().is_empty() {
            payload["msg_id"] = json!(event.message_id);
        }
        let response = client
            .post(&url)
            .header("Authorization", format!("QQBot {token}"))
            .json(&payload)
            .send()
            .await
            .map_err(|error| format!("无法发送 QQ 回复：{error}"))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| format!("无法读取 QQ 发送响应：{error}"))?;
        if !status.is_success() {
            return Err(format!(
                "QQ 回复失败：HTTP {status} {}",
                logging::sanitize_log_text(&body)
            ));
        }
    }

    logging::write_app_event_best_effort(
        app,
        AppEventBuilder::new(
            AppLogLevel::Debug,
            AppLogCategory::Im,
            "im_reply_api",
            "completed",
            "QQ 发送 API 调用完成。",
        )
        .metadata(json!({
            "providerId": IM_PROVIDER_QQ,
            "chatHash": inbound::hash_identifier(&event.chat_id)
        })),
    );
    Ok(())
}

async fn fetch_access_token(
    _app: &AppHandle,
    settings: &ImProviderSettings,
) -> Result<String, String> {
    if let Ok(state) = lock_gateway_state() {
        if let (Some(token), Some(expire_at)) =
            (state.access_token.clone(), state.access_token_expire_at)
        {
            if expire_at > Instant::now() + Duration::from_secs(60) {
                return Ok(token);
            }
        }
    }

    let app_id = settings
        .to_qq_config()
        .map(|config| config.app_id.clone())
        .ok_or_else(|| "QQ App ID 未配置。".to_owned())?;
    let app_secret = storage::load_im_provider_secret(IM_PROVIDER_QQ)?
        .ok_or_else(|| "QQ AppSecret 未配置，无法发送回复。".to_owned())?;
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| format!("无法创建 QQ HTTP client：{error}"))?;
    let response = client
        .post("https://bots.qq.com/app/getAppAccessToken")
        .json(&json!({ "appId": app_id, "clientSecret": app_secret }))
        .send()
        .await
        .map_err(|error| format!("无法请求 QQ access_token：{error}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("无法读取 QQ token 响应：{error}"))?;
    if !status.is_success() {
        return Err(format!(
            "QQ token 请求失败：HTTP {status} {}",
            logging::sanitize_log_text(&body)
        ));
    }
    let value: Value =
        serde_json::from_str(&body).map_err(|error| format!("无法解析 QQ token 响应：{error}"))?;
    let token = value
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| "QQ token 响应缺少 access_token。".to_owned())?
        .to_owned();
    let expires_in = value
        .get("expires_in")
        .and_then(Value::as_u64)
        .unwrap_or(7200);
    if let Ok(mut state) = lock_gateway_state() {
        state.access_token = Some(token.clone());
        state.access_token_expire_at = Some(Instant::now() + Duration::from_secs(expires_in));
    }
    Ok(token)
}

async fn rate_limit_send() -> Result<(), String> {
    let wait_duration = {
        let mut state = lock_gateway_state()?;
        let now = Instant::now();
        let wait_duration = state.last_send_at.and_then(|last_send_at| {
            QQ_SEND_INTERVAL.checked_sub(now.saturating_duration_since(last_send_at))
        });
        state.last_send_at = Some(now);
        wait_duration
    };
    if let Some(wait_duration) = wait_duration {
        tokio::time::sleep(wait_duration).await;
    }
    Ok(())
}
