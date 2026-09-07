use super::inbound::{self, ImInboundEvent};
use crate::domain::{
    ImGatewayStatus, ImLoginStatus, ImProviderSettings, IM_PROVIDER_WEIXIN, WEIXIN_DEFAULT_BASE_URL,
};
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
use uuid::Uuid;

const WEIXIN_SEND_INTERVAL: Duration = Duration::from_millis(280);
const RECENT_EVENT_LIMIT: usize = 512;
const SESSION_EXPIRED_ERRCODE: i64 = -14;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WeixinSidecarConfig {
    mode: String,
    token: String,
    base_url: String,
}

struct WeixinGatewayState {
    child: Option<Child>,
    login_child: Option<Child>,
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
    login_status: String,
    login_qr_image_base64: Option<String>,
    login_account_id: Option<String>,
    login_message: String,
}

impl Default for WeixinGatewayState {
    fn default() -> Self {
        Self {
            child: None,
            login_child: None,
            running: false,
            connected: false,
            domain: "weixin".to_owned(),
            app_id_configured: false,
            secret_configured: false,
            last_started_at: None,
            last_stopped_at: None,
            last_error: None,
            recent_event_ids: VecDeque::new(),
            recent_event_set: HashSet::new(),
            last_send_at: None,
            login_status: "idle".to_owned(),
            login_qr_image_base64: None,
            login_account_id: None,
            login_message: "尚未开始扫码登录。".to_owned(),
        }
    }
}

static WEIXIN_GATEWAY_STATE: OnceLock<Mutex<WeixinGatewayState>> = OnceLock::new();

/** 启动个人微信长轮询网关。 */
pub async fn start_gateway(app: AppHandle) -> Result<ImGatewayStatus, String> {
    let settings = load_weixin_provider(&app)?;
    let token = storage::load_im_provider_secret(IM_PROVIDER_WEIXIN)?
        .ok_or_else(|| "请先扫码登录个人微信。".to_owned())?;
    validate_gateway_settings(&settings)?;
    let config = settings
        .to_weixin_config()
        .ok_or_else(|| "未找到个人微信配置。".to_owned())?;
    let base_url = if config.base_url.trim().is_empty() {
        WEIXIN_DEFAULT_BASE_URL.to_owned()
    } else {
        config.base_url.trim().trim_end_matches('/').to_owned()
    };

    stop_login_process()?;
    let spawned = super::process::spawn_im_sidecar(
        &app,
        IM_PROVIDER_WEIXIN,
        "weixin-gateway",
        &WeixinSidecarConfig {
            mode: "gateway".to_owned(),
            token,
            base_url,
        },
    )?;
    let mut state = lock_gateway_state()?;
    if let Some(mut old_child) = state.child.take() {
        let _ = old_child.kill();
    }
    state.running = true;
    state.connected = false;
    state.domain = "weixin".to_owned();
    state.app_id_configured = !config.account_id.trim().is_empty();
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
            "个人微信网关已启动。",
        )
        .metadata(json!({
            "providerId": IM_PROVIDER_WEIXIN,
            "knowledgeBaseCount": settings.default_knowledge_base_ids.len(),
            "allowedUserCount": settings.allowed_user_open_ids.len(),
            "allowedChatCount": settings.allowed_chat_ids.len(),
        })),
    );

    spawn_gateway_stdout_reader(app.clone(), spawned.stdout);
    super::process::spawn_stderr_reader(app.clone(), IM_PROVIDER_WEIXIN.to_owned(), spawned.stderr);
    load_gateway_status(&app)
}

/** 停止个人微信网关。 */
pub fn stop_gateway(app: &AppHandle) -> Result<ImGatewayStatus, String> {
    let mut state = lock_gateway_state()?;
    if let Some(mut child) = state.child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    state.running = false;
    state.connected = false;
    state.last_stopped_at = Some(format_local_datetime());

    logging::write_app_event_best_effort(
        app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Im,
            "im_gateway_stop",
            "completed",
            "个人微信网关已停止。",
        )
        .metadata(json!({ "providerId": IM_PROVIDER_WEIXIN })),
    );
    Ok(state.to_status())
}

/** 读取个人微信网关状态。 */
pub fn load_gateway_status(app: &AppHandle) -> Result<ImGatewayStatus, String> {
    let settings = load_weixin_provider(app).ok();
    let secret_configured = storage::load_im_provider_credential_status(IM_PROVIDER_WEIXIN)
        .map(|status| status.configured)
        .unwrap_or(false);
    let mut state = lock_gateway_state()?;
    state.domain = "weixin".to_owned();
    state.app_id_configured = settings
        .as_ref()
        .and_then(ImProviderSettings::to_weixin_config)
        .is_some_and(|config| !config.account_id.trim().is_empty());
    state.secret_configured = secret_configured;
    Ok(state.to_status())
}

/** 启动个人微信扫码登录 sidecar。 */
pub async fn start_login(app: AppHandle) -> Result<ImLoginStatus, String> {
    let settings = load_weixin_provider(&app).ok();
    let base_url = settings
        .as_ref()
        .and_then(ImProviderSettings::to_weixin_config)
        .map(|config| config.base_url.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| WEIXIN_DEFAULT_BASE_URL.to_owned());

    stop_login_process()?;
    {
        let mut state = lock_gateway_state()?;
        state.login_status = "wait".to_owned();
        state.login_qr_image_base64 = None;
        state.login_account_id = None;
        state.login_message = "正在获取登录二维码。".to_owned();
    }

    let spawned = super::process::spawn_im_sidecar(
        &app,
        IM_PROVIDER_WEIXIN,
        "weixin-gateway",
        &WeixinSidecarConfig {
            mode: "login".to_owned(),
            token: String::new(),
            base_url,
        },
    )?;
    {
        let mut state = lock_gateway_state()?;
        state.login_child = Some(spawned.child);
    }

    spawn_login_stdout_reader(app.clone(), spawned.stdout);
    super::process::spawn_stderr_reader(app.clone(), IM_PROVIDER_WEIXIN.to_owned(), spawned.stderr);
    load_login_status()
}

/** 读取扫码登录状态。 */
pub fn load_login_status() -> Result<ImLoginStatus, String> {
    let state = lock_gateway_state()?;
    Ok(ImLoginStatus {
        provider_id: IM_PROVIDER_WEIXIN.to_owned(),
        status: state.login_status.clone(),
        qr_image_base64: state.login_qr_image_base64.clone(),
        account_id: state.login_account_id.clone(),
        message: state.login_message.clone(),
    })
}

/** 取消扫码登录。 */
pub fn cancel_login() -> Result<ImLoginStatus, String> {
    stop_login_process()?;
    let mut state = lock_gateway_state()?;
    state.login_status = "idle".to_owned();
    state.login_qr_image_base64 = None;
    state.login_message = "已取消扫码登录。".to_owned();
    Ok(ImLoginStatus {
        provider_id: IM_PROVIDER_WEIXIN.to_owned(),
        status: state.login_status.clone(),
        qr_image_base64: None,
        account_id: state.login_account_id.clone(),
        message: state.login_message.clone(),
    })
}

impl WeixinGatewayState {
    fn to_status(&self) -> ImGatewayStatus {
        ImGatewayStatus {
            provider_id: IM_PROVIDER_WEIXIN.to_owned(),
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

fn lock_gateway_state() -> Result<std::sync::MutexGuard<'static, WeixinGatewayState>, String> {
    WEIXIN_GATEWAY_STATE
        .get_or_init(|| Mutex::new(WeixinGatewayState::default()))
        .lock()
        .map_err(|_| "微信网关状态锁已损坏。".to_owned())
}

fn load_weixin_provider(app: &AppHandle) -> Result<ImProviderSettings, String> {
    storage::load_im_provider_settings(app, IM_PROVIDER_WEIXIN)
}

fn validate_gateway_settings(settings: &ImProviderSettings) -> Result<(), String> {
    if !settings.enabled {
        return Err("请先启用个人微信集成。".to_owned());
    }
    if settings.default_knowledge_base_ids.is_empty() {
        return Err("请至少选择一个微信默认知识库范围。".to_owned());
    }
    Ok(())
}

fn stop_login_process() -> Result<(), String> {
    let mut state = lock_gateway_state()?;
    if let Some(mut child) = state.login_child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    Ok(())
}

fn spawn_gateway_stdout_reader(app: AppHandle, stdout: impl std::io::Read + Send + 'static) {
    tauri::async_runtime::spawn_blocking(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else {
                record_gateway_error(&app, "微信 sidecar stdout 读取失败。");
                break;
            };
            let trimmed_line = line.trim();
            if trimmed_line.is_empty() {
                continue;
            }
            if !trimmed_line.starts_with('{') {
                super::process::record_stdout_noise(&app, IM_PROVIDER_WEIXIN, trimmed_line);
                continue;
            }
            match serde_json::from_str::<Value>(trimmed_line) {
                Ok(value) => {
                    let kind = value
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("message");
                    if kind == "session_expired" {
                        record_session_expired(&app);
                        break;
                    }
                    match serde_json::from_value::<ImInboundEvent>(value) {
                        Ok(mut event) => {
                            if event.kind.trim().is_empty() {
                                event.kind = "message".to_owned();
                            }
                            mark_gateway_connected(&app);
                            if !remember_event_id(&event.event_id) {
                                continue;
                            }
                            let event_app = app.clone();
                            tauri::async_runtime::spawn(async move {
                                handle_inbound_event(event_app, event).await;
                            });
                        }
                        Err(error) => super::process::record_stdout_noise(
                            &app,
                            IM_PROVIDER_WEIXIN,
                            &format!("微信 sidecar JSONL 事件格式无效：{error}"),
                        ),
                    }
                }
                Err(error) => super::process::record_stdout_noise(
                    &app,
                    IM_PROVIDER_WEIXIN,
                    &format!("微信 sidecar JSONL 无效：{error}"),
                ),
            }
        }
    });
}

fn spawn_login_stdout_reader(app: AppHandle, stdout: impl std::io::Read + Send + 'static) {
    tauri::async_runtime::spawn_blocking(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else {
                break;
            };
            let trimmed_line = line.trim();
            if trimmed_line.is_empty() || !trimmed_line.starts_with('{') {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(trimmed_line) else {
                continue;
            };
            let kind = value.get("kind").and_then(Value::as_str).unwrap_or("");
            match kind {
                "qr" => {
                    let qr = value
                        .get("qrImageBase64")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    if let Ok(mut state) = lock_gateway_state() {
                        state.login_status = "wait".to_owned();
                        state.login_qr_image_base64 = if qr.is_empty() { None } else { Some(qr) };
                        state.login_message = "请使用手机微信扫描二维码。".to_owned();
                    }
                }
                "login" => {
                    let status = value
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("wait")
                        .to_owned();
                    let message = value
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    if status == "confirmed" {
                        let token = value
                            .get("token")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned();
                        let account_id = value
                            .get("accountId")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned();
                        let base_url = value
                            .get("baseUrl")
                            .and_then(Value::as_str)
                            .unwrap_or(WEIXIN_DEFAULT_BASE_URL)
                            .to_owned();
                        if !token.trim().is_empty() {
                            let _ = persist_weixin_login(&app, &token, &account_id, &base_url);
                        }
                        if let Ok(mut state) = lock_gateway_state() {
                            state.login_status = "confirmed".to_owned();
                            state.login_account_id = Some(account_id);
                            state.login_qr_image_base64 = None;
                            state.login_message = if message.is_empty() {
                                "微信登录成功，token 已保存。".to_owned()
                            } else {
                                message
                            };
                        }
                    } else if let Ok(mut state) = lock_gateway_state() {
                        state.login_status = status;
                        state.login_message = if message.is_empty() {
                            "扫码登录未完成。".to_owned()
                        } else {
                            message
                        };
                    }
                }
                _ => {}
            }
        }
    });
}

fn persist_weixin_login(
    app: &AppHandle,
    token: &str,
    account_id: &str,
    base_url: &str,
) -> Result<(), String> {
    storage::save_im_provider_secret(IM_PROVIDER_WEIXIN, token)?;
    storage::update_weixin_account(app, account_id, base_url)
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
                "个人微信长轮询已收到事件。",
            )
            .metadata(json!({ "providerId": IM_PROVIDER_WEIXIN })),
        );
    }
}

fn record_session_expired(app: &AppHandle) {
    if let Ok(mut state) = lock_gateway_state() {
        if let Some(mut child) = state.child.take() {
            let _ = child.kill();
        }
        state.running = false;
        state.connected = false;
        state.last_error = Some("微信登录已过期，请重新扫码。".to_owned());
        state.login_status = "expired".to_owned();
        state.login_message = "微信登录已过期，请重新扫码。".to_owned();
    }
    logging::write_app_event_best_effort(
        app,
        AppEventBuilder::new(
            AppLogLevel::Warn,
            AppLogCategory::Im,
            "im_gateway_disconnected",
            "failed",
            "微信登录已过期，请重新扫码。",
        )
        .metadata(json!({ "providerId": IM_PROVIDER_WEIXIN, "errcode": SESSION_EXPIRED_ERRCODE })),
    );
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
        .metadata(json!({ "providerId": IM_PROVIDER_WEIXIN })),
    );
}

async fn handle_inbound_event(app: AppHandle, event: ImInboundEvent) {
    let started_at = Instant::now();
    let event_hash = inbound::hash_identifier(&event.event_id);
    let mut settings = match load_weixin_provider(&app) {
        Ok(settings) => settings,
        Err(error) => {
            record_gateway_error(&app, &error);
            return;
        }
    };

    inbound::remember_discovered_peer_from_event(&app, IM_PROVIDER_WEIXIN, &event, &event_hash);
    if let Ok(next_settings) = load_weixin_provider(&app) {
        settings = next_settings;
    }

    if let Err(block) = inbound::decide_event_handling(IM_PROVIDER_WEIXIN, &settings, &event) {
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
                IM_PROVIDER_WEIXIN,
                &event,
                &settings,
                &event_hash,
            )),
        );
        return;
    }

    let reply = inbound::handle_authorized_event(
        app.clone(),
        IM_PROVIDER_WEIXIN,
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
                "微信回复已发送。",
            )
            .duration(started_at.elapsed())
            .metadata(json!({
                "providerId": IM_PROVIDER_WEIXIN,
                "eventHash": event_hash,
                "replyChars": reply.chars().count()
            })),
        ),
        Err(error) => {
            if error.contains(&SESSION_EXPIRED_ERRCODE.to_string()) {
                record_session_expired(&app);
            }
            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    AppLogLevel::Error,
                    AppLogCategory::Im,
                    "im_reply_failed",
                    "failed",
                    error,
                )
                .duration(started_at.elapsed())
                .metadata(json!({ "providerId": IM_PROVIDER_WEIXIN, "eventHash": event_hash })),
            );
        }
    }
}

async fn send_text_reply(
    app: &AppHandle,
    settings: &ImProviderSettings,
    event: &ImInboundEvent,
    text: &str,
) -> Result<(), String> {
    let token = storage::load_im_provider_secret(IM_PROVIDER_WEIXIN)?
        .ok_or_else(|| "微信 token 未配置，无法发送回复。".to_owned())?;
    let base_url = settings
        .to_weixin_config()
        .map(|config| config.base_url.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| WEIXIN_DEFAULT_BASE_URL.to_owned());
    let client = Client::builder()
        .timeout(Duration::from_secs(40))
        .build()
        .map_err(|error| format!("无法创建微信 HTTP client：{error}"))?;
    let chunks = inbound::chunk_chars(text, inbound::WEIXIN_REPLY_MAX_CHARS);
    let target = if inbound::is_group_chat_event(event) {
        event.chat_id.clone()
    } else {
        event.sender_open_id.clone()
    };

    for chunk in chunks {
        rate_limit_send().await?;
        let payload = json!({
            "base_info": { "channel_version": "1.0.0" },
            "msg": {
                "from_user_id": "",
                "to_user_id": target,
                "client_id": format!("orange-{}", Uuid::new_v4()),
                "message_type": 2,
                "message_state": 2,
                "item_list": [{ "type": 1, "text_item": { "text": chunk } }],
                "context_token": if event.context_token.trim().is_empty() {
                    Value::Null
                } else {
                    json!(event.context_token)
                },
            }
        });
        let response = client
            .post(format!("{}/ilink/bot/sendmessage", base_url.trim_end_matches('/')))
            .header("Content-Type", "application/json")
            .header("AuthorizationType", "ilink_bot_token")
            .header("Authorization", format!("Bearer {token}"))
            .header("X-WECHAT-UIN", random_wechat_uin())
            .json(&payload)
            .send()
            .await
            .map_err(|error| format!("无法发送微信回复：{error}"))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| format!("无法读取微信发送响应：{error}"))?;
        if !status.is_success() {
            return Err(format!(
                "微信回复失败：HTTP {status} {}",
                logging::sanitize_log_text(&body)
            ));
        }
        if let Ok(value) = serde_json::from_str::<Value>(&body) {
            let errcode = value
                .get("errcode")
                .or_else(|| value.get("ret"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            if errcode != 0 {
                return Err(format!(
                    "微信回复失败：errcode={errcode} {}",
                    value
                        .get("errmsg")
                        .and_then(Value::as_str)
                        .map(logging::sanitize_log_text)
                        .unwrap_or_default()
                ));
            }
        }
    }

    logging::write_app_event_best_effort(
        app,
        AppEventBuilder::new(
            AppLogLevel::Debug,
            AppLogCategory::Im,
            "im_reply_api",
            "completed",
            "微信发送 API 调用完成。",
        )
        .metadata(json!({
            "providerId": IM_PROVIDER_WEIXIN,
            "chatHash": inbound::hash_identifier(&event.chat_id)
        })),
    );
    Ok(())
}

async fn rate_limit_send() -> Result<(), String> {
    let wait_duration = {
        let mut state = lock_gateway_state()?;
        let now = Instant::now();
        let wait_duration = state.last_send_at.and_then(|last_send_at| {
            WEIXIN_SEND_INTERVAL.checked_sub(now.saturating_duration_since(last_send_at))
        });
        state.last_send_at = Some(now);
        wait_duration
    };
    if let Some(wait_duration) = wait_duration {
        tokio::time::sleep(wait_duration).await;
    }
    Ok(())
}

fn random_wechat_uin() -> String {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;
    STANDARD.encode(Uuid::new_v4().as_u128().to_string().as_bytes())
}
