use super::inbound::{self, ImInboundEvent};
use crate::domain::{ImGatewayStatus, ImProviderSettings, IM_PROVIDER_WECOM, WECOM_DEFAULT_WS_URL};
use crate::logging::{self, AppEventBuilder, AppLogCategory, AppLogLevel};
use crate::storage::{self, format_local_datetime};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::AppHandle;

/** 企业微信发送间隔，避免同一条 WS 上连续写入过密。 */
const WECOM_SEND_INTERVAL: Duration = Duration::from_millis(120);

/** 最近事件去重窗口大小。 */
const RECENT_EVENT_LIMIT: usize = 512;

/** 鉴权通过后立刻占住流式气泡，避免企业微信因 Agent 耗时超时重试。 */
const PROCESSING_PLACEHOLDER: &str = "橘记正在处理…";

/** 首版只处理文本；其它消息类型回固定说明。 */
const UNSUPPORTED_MESSAGE_REPLY: &str = "暂不支持该消息类型，请发送文本。";

/** 企业微信 sidecar 配置，通过 stdin JSON 注入。 */
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WecomSidecarConfig {
    bot_id: String,
    secret: String,
    robot_name: String,
    ws_url: String,
}

/** 写入 sidecar stdin 的回复命令；reqId 必须与入站回调相同。 */
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WecomSidecarReply {
    kind: String,
    req_id: String,
    stream_id: String,
    text: String,
    finish: bool,
}

struct WecomGatewayState {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
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
}

impl Default for WecomGatewayState {
    fn default() -> Self {
        Self {
            child: None,
            stdin: None,
            running: false,
            connected: false,
            domain: "wecom".to_owned(),
            app_id_configured: false,
            secret_configured: false,
            last_started_at: None,
            last_stopped_at: None,
            last_error: None,
            recent_event_ids: VecDeque::new(),
            recent_event_set: HashSet::new(),
            last_send_at: None,
        }
    }
}

static WECOM_GATEWAY_STATE: OnceLock<Mutex<WecomGatewayState>> = OnceLock::new();

/** 启动企业微信智能机器人长连接网关。 */
pub async fn start_gateway(app: AppHandle) -> Result<ImGatewayStatus, String> {
    let settings = load_wecom_provider(&app)?;
    let secret = storage::load_im_provider_secret(IM_PROVIDER_WECOM)?
        .ok_or_else(|| "请先保存企业微信 Secret。".to_owned())?;
    validate_gateway_settings(&settings)?;
    let config = settings
        .to_wecom_config()
        .ok_or_else(|| "未找到企业微信配置。".to_owned())?;
    let ws_url = if config.ws_url.trim().is_empty() {
        WECOM_DEFAULT_WS_URL.to_owned()
    } else {
        config.ws_url.trim().to_owned()
    };

    let spawned = super::process::spawn_im_sidecar(
        &app,
        IM_PROVIDER_WECOM,
        "wecom-gateway",
        &WecomSidecarConfig {
            bot_id: config.bot_id.clone(),
            secret,
            robot_name: config.robot_name.clone(),
            ws_url,
        },
    )?;
    let mut state = lock_gateway_state()?;

    drop(state.stdin.take());
    if let Some(mut old_child) = state.child.take() {
        let _ = old_child.kill();
        let _ = old_child.wait();
    }

    state.running = true;
    state.connected = false;
    state.domain = "wecom".to_owned();
    state.app_id_configured = !config.bot_id.trim().is_empty();
    state.secret_configured = true;
    state.last_started_at = Some(format_local_datetime());
    state.last_stopped_at = None;
    state.last_error = None;
    state.child = Some(spawned.child);
    state.stdin = spawned.stdin;
    drop(state);

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Im,
            "im_gateway_start",
            "completed",
            "企业微信智能机器人网关已启动。",
        )
        .metadata(json!({
            "providerId": IM_PROVIDER_WECOM,
            "knowledgeBaseCount": settings.default_knowledge_base_ids.len(),
            "allowedUserCount": settings.allowed_user_open_ids.len(),
            "allowedChatCount": settings.allowed_chat_ids.len(),
        })),
    );

    spawn_stdout_reader(app.clone(), spawned.stdout);
    super::process::spawn_stderr_reader(app.clone(), IM_PROVIDER_WECOM.to_owned(), spawned.stderr);
    load_gateway_status(&app)
}

/** 停止企业微信网关；不清空配置或凭证。 */
pub fn stop_gateway(app: &AppHandle) -> Result<ImGatewayStatus, String> {
    let mut state = lock_gateway_state()?;
    drop(state.stdin.take());

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
            "企业微信智能机器人网关已停止。",
        )
        .metadata(json!({ "providerId": IM_PROVIDER_WECOM })),
    );

    Ok(state.to_status())
}

/** 读取企业微信网关状态。 */
pub fn load_gateway_status(app: &AppHandle) -> Result<ImGatewayStatus, String> {
    let settings = load_wecom_provider(app).ok();
    let secret_configured = storage::load_im_provider_credential_status(IM_PROVIDER_WECOM)
        .map(|status| status.configured)
        .unwrap_or(false);
    let mut state = lock_gateway_state()?;
    state.domain = "wecom".to_owned();
    state.app_id_configured = settings
        .as_ref()
        .and_then(ImProviderSettings::to_wecom_config)
        .is_some_and(|config| !config.bot_id.trim().is_empty());
    state.secret_configured = secret_configured;
    Ok(state.to_status())
}

impl WecomGatewayState {
    fn to_status(&self) -> ImGatewayStatus {
        ImGatewayStatus {
            provider_id: IM_PROVIDER_WECOM.to_owned(),
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

fn lock_gateway_state() -> Result<std::sync::MutexGuard<'static, WecomGatewayState>, String> {
    WECOM_GATEWAY_STATE
        .get_or_init(|| Mutex::new(WecomGatewayState::default()))
        .lock()
        .map_err(|_| "企业微信网关状态锁已损坏。".to_owned())
}

fn load_wecom_provider(app: &AppHandle) -> Result<ImProviderSettings, String> {
    storage::load_im_provider_settings(app, IM_PROVIDER_WECOM)
}

fn validate_gateway_settings(settings: &ImProviderSettings) -> Result<(), String> {
    if !settings.enabled {
        return Err("请先启用企业微信集成。".to_owned());
    }
    let bot_id = settings
        .to_wecom_config()
        .map(|config| config.bot_id.trim().to_owned())
        .unwrap_or_default();
    if bot_id.is_empty() {
        return Err("请先填写企业微信智能机器人 BotId。".to_owned());
    }
    if settings.default_knowledge_base_ids.is_empty() {
        return Err("请至少选择一个企业微信默认知识库范围。".to_owned());
    }
    Ok(())
}

fn spawn_stdout_reader(app: AppHandle, stdout: impl std::io::Read + Send + 'static) {
    tauri::async_runtime::spawn_blocking(move || {
        let reader = BufReader::new(stdout);

        for line in reader.lines() {
            let Ok(line) = line else {
                record_gateway_error(&app, "企业微信 sidecar stdout 读取失败。");
                break;
            };
            let trimmed_line = line.trim();
            if trimmed_line.is_empty() {
                continue;
            }
            if !trimmed_line.starts_with('{') {
                super::process::record_stdout_noise(&app, IM_PROVIDER_WECOM, trimmed_line);
                continue;
            }

            let Ok(value) = serde_json::from_str::<Value>(trimmed_line) else {
                super::process::record_stdout_noise(
                    &app,
                    IM_PROVIDER_WECOM,
                    "企业微信 sidecar JSONL 无效。",
                );
                continue;
            };

            if value.get("kind").and_then(Value::as_str) == Some("gateway_status") {
                if value.get("connected").and_then(Value::as_bool) == Some(true) {
                    mark_gateway_connected(&app);
                }
                continue;
            }

            match serde_json::from_value::<ImInboundEvent>(value) {
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
                        IM_PROVIDER_WECOM,
                        &format!("企业微信 sidecar JSONL 事件格式无效：{error}"),
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
    if event_id.trim().is_empty() || state.recent_event_set.contains(event_id) {
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
                "企业微信长连接已订阅成功。",
            )
            .metadata(json!({ "providerId": IM_PROVIDER_WECOM })),
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
        .metadata(json!({ "providerId": IM_PROVIDER_WECOM })),
    );
}

async fn handle_inbound_event(app: AppHandle, event: ImInboundEvent) {
    let started_at = Instant::now();
    let event_hash = inbound::hash_identifier(&event.event_id);
    let mut settings = match load_wecom_provider(&app) {
        Ok(settings) => settings,
        Err(error) => {
            record_gateway_error(&app, &error);
            return;
        }
    };

    inbound::remember_discovered_peer_from_event(&app, IM_PROVIDER_WECOM, &event, &event_hash);
    if let Ok(next_settings) = load_wecom_provider(&app) {
        settings = next_settings;
    }

    if let Err(block) = inbound::decide_event_handling(IM_PROVIDER_WECOM, &settings, &event) {
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
                IM_PROVIDER_WECOM,
                &event,
                &settings,
                &event_hash,
            )),
        );
        return;
    }

    if event.message_type != "text" {
        if let Err(error) = send_stream_reply(&event, UNSUPPORTED_MESSAGE_REPLY, true).await {
            record_reply_result(&app, started_at, &event_hash, Err(error));
        } else {
            record_reply_result(
                &app,
                started_at,
                &event_hash,
                Ok(UNSUPPORTED_MESSAGE_REPLY.chars().count()),
            );
        }
        return;
    }

    if let Err(error) = send_stream_reply(&event, PROCESSING_PLACEHOLDER, false).await {
        record_reply_result(&app, started_at, &event_hash, Err(error));
        return;
    }

    let reply = inbound::handle_authorized_event(
        app.clone(),
        IM_PROVIDER_WECOM,
        event.clone(),
        settings,
        false,
    )
    .await;
    let truncated = inbound::truncate_chars(&reply, inbound::WECOM_REPLY_MAX_CHARS);

    match send_stream_reply(&event, &truncated, true).await {
        Ok(_) => record_reply_result(&app, started_at, &event_hash, Ok(truncated.chars().count())),
        Err(error) => record_reply_result(&app, started_at, &event_hash, Err(error)),
    }
}

fn record_reply_result(
    app: &AppHandle,
    started_at: Instant,
    event_hash: &str,
    result: Result<usize, String>,
) {
    match result {
        Ok(reply_chars) => logging::write_app_event_best_effort(
            app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Im,
                "im_reply_sent",
                "completed",
                "企业微信回复已发送。",
            )
            .duration(started_at.elapsed())
            .metadata(json!({
                "providerId": IM_PROVIDER_WECOM,
                "eventHash": event_hash,
                "replyChars": reply_chars
            })),
        ),
        Err(error) => logging::write_app_event_best_effort(
            app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Im,
                "im_reply_failed",
                "failed",
                error,
            )
            .duration(started_at.elapsed())
            .metadata(json!({ "providerId": IM_PROVIDER_WECOM, "eventHash": event_hash })),
        ),
    }
}

async fn send_stream_reply(event: &ImInboundEvent, text: &str, finish: bool) -> Result<(), String> {
    let (req_id, stream_id) = parse_reply_targets(&event.context_token)?;
    rate_limit_send().await?;
    write_sidecar_reply(&WecomSidecarReply {
        kind: "reply".to_owned(),
        req_id,
        stream_id,
        text: text.to_owned(),
        finish,
    })
}

/** 从 sidecar 写入的 contextToken 拆出 reqId 和 streamId。 */
fn parse_reply_targets(context_token: &str) -> Result<(String, String), String> {
    let mut parts = context_token.splitn(2, '|');
    let req_id = parts.next().unwrap_or("").trim();
    let stream_id = parts.next().unwrap_or("").trim();
    if req_id.is_empty() || stream_id.is_empty() {
        return Err("企业微信回复缺少 reqId 或 streamId。".to_owned());
    }
    Ok((req_id.to_owned(), stream_id.to_owned()))
}

fn write_sidecar_reply(command: &WecomSidecarReply) -> Result<(), String> {
    let mut state = lock_gateway_state()?;
    let stdin = state
        .stdin
        .as_mut()
        .ok_or_else(|| "企业微信 sidecar 未运行。".to_owned())?;
    let line = serde_json::to_string(command)
        .map_err(|error| format!("无法序列化企业微信回复命令：{error}"))?;
    writeln!(stdin, "{line}").map_err(|error| format!("无法写入企业微信 sidecar 回复：{error}"))?;
    stdin
        .flush()
        .map_err(|error| format!("无法刷新企业微信 sidecar 回复：{error}"))?;
    Ok(())
}

async fn rate_limit_send() -> Result<(), String> {
    let wait_duration = {
        let mut state = lock_gateway_state()?;
        let now = Instant::now();
        let wait_duration = state.last_send_at.and_then(|last_send_at| {
            WECOM_SEND_INTERVAL.checked_sub(now.saturating_duration_since(last_send_at))
        });
        state.last_send_at = Some(now);
        wait_duration
    };
    if let Some(wait_duration) = wait_duration {
        tokio::time::sleep(wait_duration).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /** contextToken 必须拆成 reqId 和 streamId，缺一段就拒绝回复。 */
    #[test]
    fn parse_reply_targets_requires_both_parts() {
        assert_eq!(
            parse_reply_targets("req-1|stream-1").unwrap(),
            ("req-1".to_owned(), "stream-1".to_owned())
        );
        assert!(parse_reply_targets("req-1").is_err());
        assert!(parse_reply_targets("|stream-1").is_err());
        assert!(parse_reply_targets("").is_err());
    }
}
