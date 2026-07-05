use crate::domain::{
    AgentMessage, AgentSession, AgentTurnRequest, FeishuGatewayStatus, FeishuIntegrationSettings,
    WorkspaceSnapshot,
};
use crate::logging::{self, AppEventBuilder, AppLogCategory, AppLogLevel};
use crate::storage::{self, create_id, format_local_datetime};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/** 飞书单个收件人的发送限流间隔；飞书同用户或同群发送上限为 5 QPS。 */
const FEISHU_SEND_INTERVAL: Duration = Duration::from_millis(220);

/** 最近事件去重窗口大小，避免长连接重试或 sidecar 重放导致重复触发 Agent。 */
const RECENT_EVENT_LIMIT: usize = 512;

/** 飞书回复正文最大字符数，避免模型长回复超过纯文本消息的可读边界。 */
const MAX_FEISHU_REPLY_CHARS: usize = 3500;

/** 飞书长连接 sidecar 配置，通过 stdin JSON 注入，避免 appSecret 出现在进程命令行。 */
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FeishuSidecarConfig {
    app_id: String,
    app_secret: String,
    domain: String,
}

/** sidecar 输出的标准化飞书消息事件，stdout 以 JSONL 逐行发送给 Rust 主进程。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeishuInboundEvent {
    #[serde(default)]
    pub kind: String,
    pub event_id: String,
    #[serde(default)]
    pub message_id: String,
    pub chat_id: String,
    pub chat_type: String,
    pub sender_open_id: String,
    pub message_type: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub mentions: Vec<FeishuMention>,
}

/** 飞书消息中的 @ 元数据；只用 open_id 判断是否直接 @ bot。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeishuMention {
    pub open_id: String,
    #[serde(default)]
    pub name: String,
}

/** 飞书网关运行态；child 只在主进程内持有，不进入前端序列化状态。 */
struct FeishuGatewayState {
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
    last_send_at_by_target: std::collections::HashMap<String, Instant>,
}

impl Default for FeishuGatewayState {
    /** 初始化空网关状态，应用启动时不会自动持有任何外部进程。 */
    fn default() -> Self {
        Self {
            child: None,
            running: false,
            connected: false,
            domain: "feishu".to_owned(),
            app_id_configured: false,
            secret_configured: false,
            last_started_at: None,
            last_stopped_at: None,
            last_error: None,
            recent_event_ids: VecDeque::new(),
            recent_event_set: HashSet::new(),
            last_send_at_by_target: std::collections::HashMap::new(),
        }
    }
}

/** 全局飞书网关状态，Tauri commands 和 sidecar reader 线程共享。 */
static FEISHU_GATEWAY_STATE: OnceLock<Mutex<FeishuGatewayState>> = OnceLock::new();

/** 启动飞书长连接网关；只负责拉起 sidecar，消息处理在后台任务中完成。 */
pub async fn start_gateway(app: AppHandle) -> Result<FeishuGatewayStatus, String> {
    let settings = storage::load_im_settings(&app)?.feishu;
    let app_secret =
        storage::load_feishu_app_secret()?.ok_or_else(|| "请先保存飞书 appSecret。".to_owned())?;

    validate_gateway_settings(&settings)?;

    let sidecar_path = sidecar_binary_path(&app)?;
    let mut child = Command::new(&sidecar_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            format!(
                "无法启动飞书 sidecar {}：{error}",
                sidecar_path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("feishu-gateway")
            )
        })?;
    let config = FeishuSidecarConfig {
        app_id: settings.app_id.clone(),
        app_secret,
        domain: settings.domain.clone(),
    };

    if let Some(mut stdin) = child.stdin.take() {
        let config_line = serde_json::to_string(&config)
            .map_err(|error| format!("无法序列化飞书 sidecar 配置：{error}"))?;

        // appSecret 只写入 sidecar stdin，不进入命令行参数或日志。
        writeln!(stdin, "{config_line}")
            .map_err(|error| format!("无法写入飞书 sidecar 配置：{error}"))?;
    }

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let mut state = lock_gateway_state()?;

    if let Some(mut old_child) = state.child.take() {
        let _ = old_child.kill();
    }

    state.running = true;
    state.connected = false;
    state.domain = settings.domain.clone();
    state.app_id_configured = !settings.app_id.trim().is_empty();
    state.secret_configured = true;
    state.last_started_at = Some(format_local_datetime());
    state.last_stopped_at = None;
    state.last_error = None;
    state.child = Some(child);
    drop(state);

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Im,
            "feishu_gateway_start",
            "completed",
            "飞书长连接网关已启动。",
        )
        .metadata(json!({
            "domain": settings.domain,
            "knowledgeBaseCount": settings.default_knowledge_base_ids.len(),
            "allowedUserCount": settings.allowed_user_open_ids.len(),
            "allowedChatCount": settings.allowed_chat_ids.len(),
        })),
    );

    if let Some(stdout) = stdout {
        spawn_stdout_reader(app.clone(), stdout);
    }
    if let Some(stderr) = stderr {
        spawn_stderr_reader(app.clone(), stderr);
    }

    load_gateway_status(&app)
}

/** 停止飞书长连接网关；不会清空配置或 keyring 凭证。 */
pub fn stop_gateway(app: &AppHandle) -> Result<FeishuGatewayStatus, String> {
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
            "feishu_gateway_stop",
            "completed",
            "飞书长连接网关已停止。",
        ),
    );

    Ok(state.to_status())
}

/** 读取飞书网关状态，并补齐当前配置是否存在。 */
pub fn load_gateway_status(app: &AppHandle) -> Result<FeishuGatewayStatus, String> {
    let settings = storage::load_im_settings(app)?.feishu;
    let secret_configured = storage::load_feishu_credential_status()
        .map(|status| status.configured)
        .unwrap_or(false);
    let mut state = lock_gateway_state()?;

    state.domain = settings.domain;
    state.app_id_configured = !settings.app_id.trim().is_empty();
    state.secret_configured = secret_configured;

    Ok(state.to_status())
}

impl FeishuGatewayState {
    /** 转成前端可序列化状态，隐藏 child、限流表和去重队列。 */
    fn to_status(&self) -> FeishuGatewayStatus {
        FeishuGatewayStatus {
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

/** 获取全局网关状态锁；锁只包围轻量内存状态，不包围 Agent 执行。 */
fn lock_gateway_state() -> Result<std::sync::MutexGuard<'static, FeishuGatewayState>, String> {
    FEISHU_GATEWAY_STATE
        .get_or_init(|| Mutex::new(FeishuGatewayState::default()))
        .lock()
        .map_err(|_| "飞书网关状态锁已损坏。".to_owned())
}

/** 校验启动所需的非敏感配置，避免 sidecar 启动后才失败。 */
fn validate_gateway_settings(settings: &FeishuIntegrationSettings) -> Result<(), String> {
    if settings.app_id.trim().is_empty() {
        return Err("请先填写飞书 App ID。".to_owned());
    }
    if settings.default_knowledge_base_ids.is_empty() {
        return Err("请至少选择一个飞书默认知识库范围。".to_owned());
    }
    if settings.allowed_user_open_ids.is_empty() {
        return Err("请至少配置一个允许访问的飞书用户 open_id。".to_owned());
    }

    Ok(())
}

/** 定位 sidecar 二进制；开发态优先使用源码目录下的本机编译产物。 */
fn sidecar_binary_path(app: &AppHandle) -> Result<PathBuf, String> {
    let file_name = if cfg!(target_os = "windows") {
        "feishu-gateway.exe"
    } else {
        "feishu-gateway"
    };
    let dev_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("sidecars")
        .join("bin")
        .join(file_name);

    if is_executable_file(&dev_path) {
        return Ok(dev_path);
    }

    app.path()
        .resource_dir()
        .map(|path| path.join(file_name))
        .map_err(|error| format!("无法定位应用资源目录：{error}"))
        .and_then(|path| {
            if is_executable_file(&path) {
                Ok(path)
            } else {
                Err(
                    "飞书 sidecar 尚未构建或不可执行，请先运行 npm run sidecar:feishu:build。"
                        .to_owned(),
                )
            }
        })
}

/** 判断 sidecar 路径是否是可启动的二进制文件，避免把源码目录误当成命令执行。 */
fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };

    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        // macOS/Linux 需要任一执行位，否则 spawn 会返回 Permission denied。
        metadata.permissions().mode() & 0o111 != 0
    }

    #[cfg(not(unix))]
    {
        true
    }
}

/** 后台读取 sidecar stdout 的 JSONL 消息事件，收到后快速投递到异步 Agent 任务。 */
fn spawn_stdout_reader(app: AppHandle, stdout: impl std::io::Read + Send + 'static) {
    tauri::async_runtime::spawn_blocking(move || {
        let reader = BufReader::new(stdout);

        for line in reader.lines() {
            let Ok(line) = line else {
                record_gateway_error(&app, "飞书 sidecar stdout 读取失败。");
                break;
            };

            if line.trim().is_empty() {
                continue;
            }

            let trimmed_line = line.trim();

            if !trimmed_line.starts_with('{') {
                record_sidecar_stdout_noise(&app, trimmed_line);
                continue;
            }

            match serde_json::from_str::<FeishuInboundEvent>(trimmed_line) {
                Ok(event) => {
                    mark_gateway_connected(&app);
                    let event_app = app.clone();

                    tauri::async_runtime::spawn(async move {
                        handle_inbound_event(event_app, event).await;
                    });
                }
                Err(error) => {
                    record_sidecar_stdout_noise(&app, &format!("飞书 sidecar JSONL 事件格式无效：{error}"));
                }
            }
        }
    });
}

/** 记录 sidecar stdout 中的非事件内容；不把它当断线处理，避免 SDK 日志污染事件通道。 */
fn record_sidecar_stdout_noise(app: &AppHandle, line: &str) {
    logging::write_app_event_best_effort(
        app,
        AppEventBuilder::new(
            AppLogLevel::Warn,
            AppLogCategory::Im,
            "feishu_gateway_stdout_ignored",
            "skipped",
            logging::sanitize_log_text(line),
        ),
    );
}

/** 后台读取 sidecar stderr，只写脱敏运行日志，不影响主流程。 */
fn spawn_stderr_reader(app: AppHandle, stderr: impl std::io::Read + Send + 'static) {
    tauri::async_runtime::spawn_blocking(move || {
        let reader = BufReader::new(stderr);

        for line in reader.lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }

            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    AppLogLevel::Warn,
                    AppLogCategory::Im,
                    "feishu_gateway_stderr",
                    "failed",
                    logging::sanitize_log_text(&line),
                ),
            );
        }
    });
}

/** 标记长连接已收到有效事件，用于设置页显示连接健康度。 */
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
                "feishu_gateway_connected",
                "completed",
                "飞书长连接已收到事件。",
            ),
        );
    }
}

/** 记录网关错误并更新状态；错误文本进入日志前会统一脱敏。 */
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
            "feishu_gateway_disconnected",
            "failed",
            message,
        ),
    );
}

/** 处理单条飞书消息事件：去重、鉴权、运行 Agent、回发文本。 */
async fn handle_inbound_event(app: AppHandle, event: FeishuInboundEvent) {
    let started_at = Instant::now();
    let event_hash = hash_identifier(&event.event_id);

    if !remember_event_id(&event.event_id) {
        return;
    }

    let is_group_chat = event.chat_type == "group" || event.chat_type == "topic_group" || event.chat_id.starts_with("oc_");

    if event.kind == "discovery" {
        remember_discovered_peer_from_event(&app, &event, is_group_chat, &event_hash);
        return;
    }

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Im,
            "feishu_message_received",
            "completed",
            "收到飞书消息事件。",
        )
        .metadata(json!({
            "eventHash": event_hash,
            "messageHash": hash_identifier(&event.message_id),
            "chatHash": hash_identifier(&event.chat_id),
            "senderHash": hash_identifier(&event.sender_open_id),
            "messageType": event.message_type,
            "chatType": event.chat_type,
        })),
    );

    let mut settings = match storage::load_im_settings(&app) {
        Ok(settings) => settings.feishu,
        Err(error) => {
            record_gateway_error(&app, &error);
            return;
        }
    };
    // 先记录可授权候选，再做 allowlist 判断；未授权消息也能在设置页一键加入。
    if remember_discovered_peer_from_event(&app, &event, is_group_chat, &event_hash) {
        if let Ok(next_settings) = storage::load_im_settings(&app) {
            settings = next_settings.feishu;
        }
    } else if let Ok(next_settings) = storage::load_im_settings(&app) {
        settings = next_settings.feishu;
    }
    let decision = decide_event_handling(&settings, &event);

    if let Err(block) = decision {
        logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Im,
                "feishu_message_blocked",
                "blocked",
                block.reason,
            )
            .duration(started_at.elapsed())
            .metadata(block_metadata(&event, &settings, &event_hash)),
        );
        return;
    }

    let reply = if event.message_type != "text" {
        "暂不支持该飞书消息类型；首版只处理文本消息。".to_owned()
    } else if event.text.trim() == "/status" {
        build_status_reply(&app, &settings)
    } else if event.text.trim() == "/reset" {
        reset_im_session(&app, &event, &settings)
    } else {
        run_agent_for_event(&app, &event, &settings).await
    };

    match send_text_reply(&app, &settings, &event, &reply).await {
        Ok(_) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Im,
                "feishu_reply_sent",
                "completed",
                "飞书回复已发送。",
            )
            .duration(started_at.elapsed())
            .metadata(json!({ "eventHash": event_hash, "replyChars": reply.chars().count() })),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Im,
                "feishu_reply_failed",
                "failed",
                error,
            )
            .duration(started_at.elapsed())
            .metadata(json!({ "eventHash": event_hash })),
        ),
    }
}

/** 从消息或会话进入事件中保存可授权候选，返回是否完成保存尝试。 */
fn remember_discovered_peer_from_event(
    app: &AppHandle,
    event: &FeishuInboundEvent,
    is_group_chat: bool,
    event_hash: &str,
) -> bool {
    match storage::remember_feishu_discovered_peer(
        app,
        &event.sender_open_id,
        &event.chat_id,
        is_group_chat,
    ) {
        Ok(changed) => {
            if changed {
                logging::write_app_event_best_effort(
                    app,
                    AppEventBuilder::new(
                        AppLogLevel::Info,
                        AppLogCategory::Im,
                        "feishu_discovered_peer_saved",
                        "completed",
                        "已记录飞书待授权对象。",
                    )
                    .metadata(json!({
                        "eventHash": event_hash,
                        "senderHash": hash_identifier(&event.sender_open_id),
                        "chatHash": hash_identifier(&event.chat_id),
                        "chatType": event.chat_type,
                        "isGroupChat": is_group_chat,
                    })),
                );
            }
            true
        }
        Err(error) => {
            logging::write_app_event_best_effort(
                app,
                AppEventBuilder::new(
                    AppLogLevel::Warn,
                    AppLogCategory::Im,
                    "feishu_discovered_peer_save",
                    "failed",
                    error,
                )
                .metadata(json!({ "eventHash": event_hash })),
            );
            false
        }
    }
}

/** 飞书消息拦截原因，便于在日志中补充脱敏诊断信息。 */
struct FeishuBlockReason {
    reason: String,
}

/** 将事件 ID 放入固定窗口去重集合，返回 false 表示近期已处理过。 */
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

/** 根据私聊/群聊、白名单和 @ 要求决定是否处理消息。 */
fn decide_event_handling(
    settings: &FeishuIntegrationSettings,
    event: &FeishuInboundEvent,
) -> Result<(), FeishuBlockReason> {
    if !settings.enabled {
        return Err(block_reason("飞书集成未启用。"));
    }
    if !settings
        .allowed_user_open_ids
        .iter()
        .any(|open_id| open_id == &event.sender_open_id)
    {
        return Err(block_reason("飞书发送人不在允许名单中。"));
    }

    let is_group_chat = event.chat_type == "group" || event.chat_id.starts_with("oc_");

    if is_group_chat {
        if !settings
            .allowed_chat_ids
            .iter()
            .any(|chat_id| chat_id == &event.chat_id)
        {
            return Err(block_reason("飞书群聊不在允许名单中。"));
        }
        if settings.require_mention && !is_direct_bot_mention(event) {
            return Err(block_reason("飞书群聊消息未直接 @ 机器人。"));
        }
    }

    Ok(())
}

/** 构造拦截原因，避免在调用处重复分配和拼接敏感上下文。 */
fn block_reason(reason: &str) -> FeishuBlockReason {
    FeishuBlockReason {
        reason: reason.to_owned(),
    }
}

/** 生成飞书拦截日志元数据，只包含 hash、数量和布尔状态，不包含原始 open_id/chat_id/正文。 */
fn block_metadata(
    event: &FeishuInboundEvent,
    settings: &FeishuIntegrationSettings,
    event_hash: &str,
) -> Value {
    let is_group_chat = event.chat_type == "group" || event.chat_id.starts_with("oc_");
    let sender_allowed = settings
        .allowed_user_open_ids
        .iter()
        .any(|open_id| open_id == &event.sender_open_id);
    let chat_allowed = settings
        .allowed_chat_ids
        .iter()
        .any(|chat_id| chat_id == &event.chat_id);

    json!({
        "eventHash": event_hash,
        "senderHash": hash_identifier(&event.sender_open_id),
        "chatHash": hash_identifier(&event.chat_id),
        "chatType": event.chat_type,
        "isGroupChat": is_group_chat,
        "senderAllowed": sender_allowed,
        "chatAllowed": !is_group_chat || chat_allowed,
        "directMention": is_direct_bot_mention(event),
        "requireMention": settings.require_mention,
        "allowedUserCount": settings.allowed_user_open_ids.len(),
        "allowedChatCount": settings.allowed_chat_ids.len(),
    })
}

/** 判断群聊是否直接 @ bot；sidecar 会把 bot 自身 mention 标成 open_id=bot。 */
fn is_direct_bot_mention(event: &FeishuInboundEvent) -> bool {
    event
        .mentions
        .iter()
        .any(|mention| mention.open_id == "bot" || mention.name == "bot")
}

/** 构造 `/status` 回复，只展示脱敏配置和运行状态。 */
fn build_status_reply(app: &AppHandle, settings: &FeishuIntegrationSettings) -> String {
    let model_enabled = storage::load_user_settings(app)
        .map(|settings| settings.model_config.enabled)
        .unwrap_or(false);
    let status = load_gateway_status(app).ok();

    format!(
        "橘记 飞书集成状态：{}\n模型：{}\n默认知识库范围：{} 个\n群聊 @：{}",
        if status.as_ref().is_some_and(|status| status.running) {
            "运行中"
        } else {
            "未运行"
        },
        if model_enabled {
            "已启用"
        } else {
            "未启用，使用本地兜底"
        },
        settings.default_knowledge_base_ids.len(),
        if settings.require_mention {
            "需要"
        } else {
            "不需要"
        }
    )
}

/** 处理 `/reset`，删除当前飞书对话映射，下次普通消息会创建新 AgentSession。 */
fn reset_im_session(
    app: &AppHandle,
    event: &FeishuInboundEvent,
    settings: &FeishuIntegrationSettings,
) -> String {
    let channel_key = build_channel_key(event);

    if let Err(error) = storage::delete_im_session_mapping(app, &channel_key) {
        logging::write_app_event_best_effort(
            app,
            AppEventBuilder::new(
                AppLogLevel::Warn,
                AppLogCategory::Im,
                "feishu_session_reset",
                "failed",
                error,
            )
            .metadata(json!({ "channelHash": hash_identifier(&channel_key) })),
        );
        return "重置飞书会话失败，请稍后重试。".to_owned();
    }

    logging::write_app_event_best_effort(
        app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Im,
            "feishu_session_reset",
            "completed",
            "飞书会话映射已重置。",
        )
        .metadata(json!({
            "channelHash": hash_identifier(&channel_key),
            "scopeCount": settings.default_knowledge_base_ids.len(),
        })),
    );

    "已重置当前飞书会话。".to_owned()
}

/** 为飞书消息运行橘记 Agent，并返回可发送回飞书的短文本。 */
async fn run_agent_for_event(
    app: &AppHandle,
    event: &FeishuInboundEvent,
    settings: &FeishuIntegrationSettings,
) -> String {
    let result = crate::commands::run_agent_turn_from_im(
        app.clone(),
        event.text.trim().to_owned(),
        build_channel_key(event),
        settings.default_knowledge_base_ids.clone(),
        "飞书会话".to_owned(),
    )
    .await;

    match result {
        Ok(snapshot) => build_agent_reply_text(&snapshot),
        Err(error) => format!("飞书消息处理失败：{}", logging::sanitize_log_text(&error)),
    }
}

/** 从最新 assistant 消息构造飞书回复；有待确认 diff 时提醒回到桌面端审阅。 */
fn build_agent_reply_text(snapshot: &WorkspaceSnapshot) -> String {
    let session = snapshot
        .sessions
        .iter()
        .find(|session| session.id == snapshot.active_session_id)
        .or_else(|| snapshot.sessions.first());
    let Some(session) = session else {
        return "Agent 未返回可展示内容。".to_owned();
    };
    let assistant_content = session
        .messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
        .map(|message| message.content.trim())
        .filter(|content| !content.is_empty())
        .unwrap_or("Agent 已完成处理。");
    let mut reply = truncate_chars(assistant_content, MAX_FEISHU_REPLY_CHARS).to_owned();

    if session
        .pending_change
        .as_ref()
        .is_some_and(|change| change.status == "pending")
    {
        reply.push_str("\n\n已生成待确认改动，请回到橘记审阅后再写入本地文件。");
    }

    reply
}

/** 构造稳定 IM 会话 key；群聊按用户隔离，私聊按 sender 隔离。 */
fn build_channel_key(event: &FeishuInboundEvent) -> String {
    let is_group_chat = event.chat_type == "group" || event.chat_id.starts_with("oc_");

    if is_group_chat {
        format!(
            "feishu:group:{}:{}",
            hash_identifier(&event.chat_id),
            hash_identifier(&event.sender_open_id)
        )
    } else {
        format!("feishu:dm:{}", hash_identifier(&event.sender_open_id))
    }
}

/** 通过飞书 REST API 回复纯文本消息。 */
async fn send_text_reply(
    app: &AppHandle,
    settings: &FeishuIntegrationSettings,
    event: &FeishuInboundEvent,
    text: &str,
) -> Result<(), String> {
    rate_limit_send_target(&event.chat_id).await?;

    let app_secret = storage::load_feishu_app_secret()?
        .ok_or_else(|| "飞书 appSecret 未配置，无法发送回复。".to_owned())?;
    let token = fetch_tenant_access_token(&settings.domain, &settings.app_id, &app_secret).await?;
    let base_url = feishu_base_url(&settings.domain);
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| format!("无法创建飞书 HTTP client：{error}"))?;
    let response = client
        .post(format!("{base_url}/open-apis/im/v1/messages"))
        .bearer_auth(token)
        .query(&[("receive_id_type", "chat_id")])
        .json(&json!({
            "receive_id": event.chat_id,
            "msg_type": "text",
            "content": serde_json::to_string(&json!({ "text": truncate_chars(text, MAX_FEISHU_REPLY_CHARS) }))
                .map_err(|error| format!("无法序列化飞书文本消息：{error}"))?
        }))
        .send()
        .await
        .map_err(|error| format!("无法发送飞书回复：{error}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("无法读取飞书发送响应：{error}"))?;

    if !status.is_success() {
        return Err(format!(
            "飞书回复失败：HTTP {status} {}",
            logging::sanitize_log_text(&body)
        ));
    }

    let value: Value =
        serde_json::from_str(&body).map_err(|error| format!("无法解析飞书发送响应：{error}"))?;
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);

    if code != 0 {
        return Err(format!(
            "飞书回复失败：code={} msg={}",
            code,
            value
                .get("msg")
                .and_then(Value::as_str)
                .map(logging::sanitize_log_text)
                .unwrap_or_else(|| "unknown".to_owned())
        ));
    }

    logging::write_app_event_best_effort(
        app,
        AppEventBuilder::new(
            AppLogLevel::Debug,
            AppLogCategory::Im,
            "feishu_reply_api",
            "completed",
            "飞书发送 API 调用完成。",
        )
        .metadata(json!({ "chatHash": hash_identifier(&event.chat_id) })),
    );

    Ok(())
}

/** 对单个 chat_id 做进程内限流，避免同一目标超过飞书 5 QPS 上限。 */
async fn rate_limit_send_target(target_id: &str) -> Result<(), String> {
    let wait_duration = {
        let mut state = lock_gateway_state()?;
        let now = Instant::now();
        let wait_duration = state
            .last_send_at_by_target
            .get(target_id)
            .and_then(|last_send_at| {
                FEISHU_SEND_INTERVAL.checked_sub(now.saturating_duration_since(*last_send_at))
            });

        state
            .last_send_at_by_target
            .insert(target_id.to_owned(), now);
        wait_duration
    };

    if let Some(wait_duration) = wait_duration {
        // IM 发送发生在后台任务中；这里用短暂阻塞 sleep 控制同目标 QPS，不影响 WebView 线程。
        std::thread::sleep(wait_duration);
    }

    Ok(())
}

/** 获取飞书 tenant_access_token；首版按请求读取，todo: 增加按 expire 缓存减少 token 请求。 */
async fn fetch_tenant_access_token(
    domain: &str,
    app_id: &str,
    app_secret: &str,
) -> Result<String, String> {
    let base_url = feishu_base_url(domain);
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| format!("无法创建飞书 HTTP client：{error}"))?;
    let response = client
        .post(format!(
            "{base_url}/open-apis/auth/v3/tenant_access_token/internal"
        ))
        .json(&json!({ "app_id": app_id, "app_secret": app_secret }))
        .send()
        .await
        .map_err(|error| format!("无法请求飞书 tenant_access_token：{error}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("无法读取飞书 token 响应：{error}"))?;

    if !status.is_success() {
        return Err(format!(
            "飞书 token 请求失败：HTTP {status} {}",
            logging::sanitize_log_text(&body)
        ));
    }

    let value: Value =
        serde_json::from_str(&body).map_err(|error| format!("无法解析飞书 token 响应：{error}"))?;
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);

    if code != 0 {
        return Err(format!(
            "飞书 token 请求失败：code={} msg={}",
            code,
            value
                .get("msg")
                .and_then(Value::as_str)
                .map(logging::sanitize_log_text)
                .unwrap_or_else(|| "unknown".to_owned())
        ));
    }

    value
        .get("tenant_access_token")
        .and_then(Value::as_str)
        .filter(|token| !token.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "飞书 token 响应缺少 tenant_access_token。".to_owned())
}

/** 根据飞书/Lark 域选择 Open Platform API 根地址。 */
fn feishu_base_url(domain: &str) -> &'static str {
    if domain == "lark" {
        "https://open.larksuite.com"
    } else {
        "https://open.feishu.cn"
    }
}

/** 对 open_id、chat_id、event_id 做稳定短 hash，日志只记录 hash 不记录原文。 */
fn hash_identifier(value: &str) -> String {
    let mut hasher = Sha256::new();

    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
        .chars()
        .take(16)
        .collect()
}

/** 按字符截断文本，避免 UTF-8 边界被破坏。 */
fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }

    format!("{}...", value.chars().take(max_chars).collect::<String>())
}

/** 构造 IM AgentSession，供 commands helper 创建新会话时复用。 */
pub(crate) fn build_im_agent_session(
    title: String,
    knowledge_base_ids: Vec<String>,
) -> AgentSession {
    let now = format_local_datetime();

    AgentSession {
        id: create_id("session-im"),
        title,
        r#type: "knowledge-base".to_owned(),
        knowledge_base_ids,
        active_note_id: None,
        pinned_note_ids: Vec::new(),
        messages: Vec::new(),
        pending_change: None,
        created_at: now.clone(),
        updated_at: now,
        deleted_at: None,
        model_provider_id: None,
    }
}

/** 构造 IM 入口的乐观用户消息，和前端提交保持同一消息复用语义。 */
pub(crate) fn build_im_user_message(prompt: &str) -> AgentMessage {
    AgentMessage {
        id: create_id("user-im"),
        role: "user".to_owned(),
        content: prompt.to_owned(),
        action: Some("ask".to_owned()),
        citations: None,
        tool_calls: None,
    }
}

/** 构造 IM AgentTurnRequest，active note 为空，scope 由 IM 会话控制。 */
pub(crate) fn build_im_turn_request(
    prompt: String,
    session_id: String,
    active_knowledge_base_id: String,
    client_message_id: String,
) -> AgentTurnRequest {
    AgentTurnRequest {
        prompt,
        action: "ask".to_owned(),
        session_id,
        active_knowledge_base_id,
        active_note_id: String::new(),
        client_message_id: Some(client_message_id),
        model_provider_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /** 飞书用户和群 ID 进入日志前必须被 hash，避免明文 open_id/chat_id 落盘。 */
    #[test]
    fn hash_identifier_hides_original_value() {
        let hashed = hash_identifier("ou_secret_user");

        assert_ne!(hashed, "ou_secret_user");
        assert_eq!(hashed.len(), 16);
    }

    /** 私聊和群聊会话 key 必须稳定且群聊按用户隔离。 */
    #[test]
    fn channel_key_is_stable_and_group_is_per_user() {
        let event = FeishuInboundEvent {
            kind: "message".to_owned(),
            event_id: "evt".to_owned(),
            message_id: "msg".to_owned(),
            chat_id: "oc_group".to_owned(),
            chat_type: "group".to_owned(),
            sender_open_id: "ou_user".to_owned(),
            message_type: "text".to_owned(),
            text: "hello".to_owned(),
            mentions: Vec::new(),
        };

        assert_eq!(build_channel_key(&event), build_channel_key(&event));
        assert!(build_channel_key(&event).starts_with("feishu:group:"));
    }

    /** 群聊默认必须直接 @ bot，广播 @all 不应被当作 bot mention。 */
    #[test]
    fn direct_bot_mention_ignores_all_mentions() {
        let mut event = FeishuInboundEvent {
            kind: "message".to_owned(),
            event_id: "evt".to_owned(),
            message_id: "msg".to_owned(),
            chat_id: "oc_group".to_owned(),
            chat_type: "group".to_owned(),
            sender_open_id: "ou_user".to_owned(),
            message_type: "text".to_owned(),
            text: "hello".to_owned(),
            mentions: vec![FeishuMention {
                open_id: "@_all".to_owned(),
                name: "all".to_owned(),
            }],
        };

        assert!(!is_direct_bot_mention(&event));
        event.mentions.push(FeishuMention {
            open_id: "bot".to_owned(),
            name: "bot".to_owned(),
        });
        assert!(is_direct_bot_mention(&event));
    }

    /** 访问控制必须同时校验用户、群聊 allowlist 和 @ 条件。 */
    #[test]
    fn allowlist_blocks_unknown_user_or_group() {
        let settings = FeishuIntegrationSettings {
            enabled: true,
            domain: "feishu".to_owned(),
            app_id: "cli_x".to_owned(),
            secret_key_reference: "secret".to_owned(),
            default_knowledge_base_ids: vec!["kb".to_owned()],
            allowed_user_open_ids: vec!["ou_user".to_owned()],
            allowed_chat_ids: vec!["oc_group".to_owned()],
            discovered_user_open_ids: Vec::new(),
            discovered_chat_ids: Vec::new(),
            require_mention: true,
            updated_at: "now".to_owned(),
        };
        let event = FeishuInboundEvent {
            kind: "message".to_owned(),
            event_id: "evt".to_owned(),
            message_id: "msg".to_owned(),
            chat_id: "oc_group".to_owned(),
            chat_type: "group".to_owned(),
            sender_open_id: "ou_user".to_owned(),
            message_type: "text".to_owned(),
            text: "hello".to_owned(),
            mentions: vec![FeishuMention {
                open_id: "bot".to_owned(),
                name: "bot".to_owned(),
            }],
        };

        assert!(decide_event_handling(&settings, &event).is_ok());
        let mut blocked = event.clone();
        blocked.sender_open_id = "ou_other".to_owned();
        assert!(decide_event_handling(&settings, &blocked).is_err());
    }

    /** `/status` 和 `/reset` 使用纯文本命令，首版不依赖飞书 slash command 菜单。 */
    #[test]
    fn plain_text_commands_are_detected_by_exact_trimmed_text() {
        assert_eq!("/status".trim(), "/status");
        assert_eq!(" /reset ".trim(), "/reset");
    }
}
