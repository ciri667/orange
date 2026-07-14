use crate::domain::{
    FeishuGatewayStatus, FeishuIntegrationSettings, WorkspaceSnapshot, IM_PROVIDER_FEISHU,
};
use crate::logging::{self, AppEventBuilder, AppLogCategory, AppLogLevel};
use crate::storage::{self, format_local_datetime};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::AppHandle;

/** 飞书单个收件人的发送限流间隔；飞书同用户或同群发送上限为 5 QPS。 */
const FEISHU_SEND_INTERVAL: Duration = Duration::from_millis(220);

/** 最近事件去重窗口大小，避免长连接重试或 sidecar 重放导致重复触发 Agent。 */
const RECENT_EVENT_LIMIT: usize = 512;

/** 飞书回复正文最大字符数，避免模型长回复超过纯文本消息的可读边界。 */
const MAX_FEISHU_REPLY_CHARS: usize = 3500;

/** 飞书卡片中默认展示的改动预览上限，避免在 IM 中泄露完整笔记正文。 */
const MAX_FEISHU_CARD_PREVIEW_CHARS: usize = 480;

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
    /** 仅 card_action 事件携带的卡片操作名称。 */
    #[serde(default)]
    pub action: String,
    /** 仅 card_action 事件携带的待确认变更 ID；由 Rust 再次鉴权并查询。 */
    #[serde(default)]
    pub change_id: String,
}

/** 待确认笔记变更在飞书中展示所需的最小信息，不包含完整 diff 或外部身份。 */
#[derive(Clone, Debug)]
pub struct FeishuPendingChangeCard {
    pub chat_id: String,
    pub chat_type: String,
    pub change_id: String,
    pub short_code: String,
    pub target_path: String,
    pub operation_label: String,
    pub added_lines: usize,
    pub removed_lines: usize,
    pub preview: String,
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

/**
 * 每个 IM channel 的异步互斥锁。它只保留脱敏前的 key 于进程内内存，
 * 用来串行化 Agent turn、/new 与 /compact，防止旧快照覆盖最新会话状态。
 */
static FEISHU_CHANNEL_OPERATION_LOCKS: OnceLock<
    Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
> = OnceLock::new();

/** 启动飞书长连接网关；只负责拉起 sidecar，消息处理在后台任务中完成。 */
pub async fn start_gateway(app: AppHandle) -> Result<FeishuGatewayStatus, String> {
    let settings = storage::load_feishu_integration_settings(&app)?;
    let app_secret = storage::load_im_provider_secret(IM_PROVIDER_FEISHU)?
        .ok_or_else(|| "请先保存飞书 appSecret。".to_owned())?;

    validate_gateway_settings(&settings)?;

    let sidecar_path = super::sidecar_binary_path(&app, IM_PROVIDER_FEISHU, "feishu-gateway")?;
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
            "im_gateway_start",
            "completed",
            "飞书长连接网关已启动。",
        )
        .metadata(json!({
            "providerId": IM_PROVIDER_FEISHU,
            "domain": settings.domain,
            "knowledgeBaseCount": settings.default_knowledge_base_ids.len(),
            "allowedUserCount": settings.allowed_user_open_ids.len(),
            "allowedChatCount": settings.allowed_chat_ids.len(),
            "discoveryOnly": settings.allowed_user_open_ids.is_empty(),
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
            "im_gateway_stop",
            "completed",
            "飞书长连接网关已停止。",
        )
        .metadata(json!({ "providerId": IM_PROVIDER_FEISHU })),
    );

    Ok(state.to_status())
}

/** 读取飞书网关状态，并补齐当前配置是否存在。 */
pub fn load_gateway_status(app: &AppHandle) -> Result<FeishuGatewayStatus, String> {
    let settings = storage::load_feishu_integration_settings(app)?;
    let secret_configured = storage::load_im_provider_credential_status(IM_PROVIDER_FEISHU)
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
            provider_id: IM_PROVIDER_FEISHU.to_owned(),
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

/** 校验建立长连接所需的非敏感配置；用户白名单可为空，以便首次启动后自动发现 open_id。 */
fn validate_gateway_settings(settings: &FeishuIntegrationSettings) -> Result<(), String> {
    if !settings.enabled {
        return Err("请先启用飞书/Lark 集成。".to_owned());
    }
    if settings.app_id.trim().is_empty() {
        return Err("请先填写飞书 App ID。".to_owned());
    }
    if settings.default_knowledge_base_ids.is_empty() {
        return Err("请至少选择一个飞书默认知识库范围。".to_owned());
    }
    Ok(())
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
                    record_sidecar_stdout_noise(
                        &app,
                        &format!("飞书 sidecar JSONL 事件格式无效：{error}"),
                    );
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
            "im_gateway_stdout_ignored",
            "skipped",
            logging::sanitize_log_text(line),
        )
        .metadata(json!({ "providerId": IM_PROVIDER_FEISHU })),
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
                    "im_gateway_stderr",
                    "failed",
                    logging::sanitize_log_text(&line),
                )
                .metadata(json!({ "providerId": IM_PROVIDER_FEISHU })),
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
                "im_gateway_connected",
                "completed",
                "飞书长连接已收到事件。",
            )
            .metadata(json!({ "providerId": IM_PROVIDER_FEISHU })),
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
            "im_gateway_disconnected",
            "failed",
            message,
        )
        .metadata(json!({ "providerId": IM_PROVIDER_FEISHU })),
    );
}

/** 处理单条飞书消息或卡片事件：去重、鉴权、运行 Agent/远程审批，并回发结果。 */
async fn handle_inbound_event(app: AppHandle, event: FeishuInboundEvent) {
    let started_at = Instant::now();
    let event_hash = hash_identifier(&event.event_id);

    if !remember_event_id(&event.event_id) {
        return;
    }

    let is_group_chat = is_group_chat_event(&event);

    if event.kind == "discovery" {
        remember_discovered_peer_from_event(&app, &event, is_group_chat, &event_hash);
        return;
    }

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Im,
            "im_message_received",
            "completed",
            "收到飞书消息事件。",
        )
        .metadata(json!({
            "providerId": IM_PROVIDER_FEISHU,
            "eventHash": event_hash,
            "messageHash": hash_identifier(&event.message_id),
            "chatHash": hash_identifier(&event.chat_id),
            "senderHash": hash_identifier(&event.sender_open_id),
            "messageType": event.message_type,
            "chatType": event.chat_type,
        })),
    );

    let mut settings = match storage::load_feishu_integration_settings(&app) {
        Ok(settings) => settings,
        Err(error) => {
            record_gateway_error(&app, &error);
            return;
        }
    };
    // 先记录可授权候选，再做 allowlist 判断；未授权消息也能在设置页一键加入。
    if remember_discovered_peer_from_event(&app, &event, is_group_chat, &event_hash) {
        if let Ok(next_settings) = storage::load_feishu_integration_settings(&app) {
            settings = next_settings;
        }
    } else if let Ok(next_settings) = storage::load_feishu_integration_settings(&app) {
        settings = next_settings;
    }
    let decision = decide_event_handling(&settings, &event);

    if let Err(block) = decision {
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
            .metadata(block_metadata(&event, &settings, &event_hash)),
        );
        return;
    }

    let reply = if event.kind == "card_action" {
        // 卡片回调没有 @ mention，但仍须经过同一发送人和群聊白名单校验。
        // 审批也会写入会话状态，必须与同一通道的 Agent turn 共享串行队列。
        let channel_key = build_channel_key(&event);
        let _operation_guard = acquire_channel_operation_lock(&channel_key).await;
        handle_card_action_for_event(&app, &event).await
    } else if event.message_type != "text" {
        "暂不支持该飞书消息类型；首版只处理文本消息。".to_owned()
    } else {
        // 同一 channel 的所有文本事件都按顺序执行；命令与普通消息不会并发读取后覆盖彼此的会话快照。
        let channel_key = build_channel_key(&event);
        let _operation_guard = acquire_channel_operation_lock(&channel_key).await;
        dispatch_authorized_text_event(&app, &event, &settings, &channel_key).await
    };

    match send_text_reply(&app, &settings, &event, &reply).await {
        Ok(_) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Im,
                "im_reply_sent",
                "completed",
                "飞书回复已发送。",
            )
            .duration(started_at.elapsed())
            .metadata(json!({
                "providerId": IM_PROVIDER_FEISHU,
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
            .metadata(json!({ "providerId": IM_PROVIDER_FEISHU, "eventHash": event_hash })),
        ),
    }
}

/** 返回 channel 专属的异步锁；锁表仅进程内使用，不写入原始外部 ID 到磁盘或日志。 */
async fn acquire_channel_operation_lock(channel_key: &str) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = {
        let lock_table = FEISHU_CHANNEL_OPERATION_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
        let mut lock_table = lock_table
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        lock_table
            .entry(channel_key.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    lock.lock_owned().await
}

/**
 * 分派已经完成飞书鉴权与群聊门禁的文本事件。内置指令优先于审批文本和 Agent，
 * 因此 `/new`、`/compact` 与 `/help` 绝不会进入模型上下文。
 */
async fn dispatch_authorized_text_event(
    app: &AppHandle,
    event: &FeishuInboundEvent,
    settings: &FeishuIntegrationSettings,
    channel_key: &str,
) -> String {
    if event.text.trim() == "/status" {
        return build_status_reply(app, settings);
    }

    if let Some(command) = super::parse_builtin_command(&event.text) {
        let conversation_kind = if is_group_chat_event(event) {
            "group"
        } else {
            "direct"
        };
        // 仅传递已脱敏的身份摘要给通用命令服务，命令正文不进入会话主题。
        let im_identity = super::build_im_session_identity(
            IM_PROVIDER_FEISHU,
            channel_key,
            conversation_kind,
            "新会话",
        );
        return crate::commands::handle_im_builtin_command(
            app.clone(),
            IM_PROVIDER_FEISHU,
            command,
            channel_key,
            settings.default_knowledge_base_ids.clone(),
            im_identity,
        )
        .await;
    }

    if let Some((action, change_token)) = parse_pending_change_text_command(&event.text) {
        return handle_pending_change_command_for_event(app, event, action, change_token).await;
    }

    run_agent_for_event(app, event, settings).await
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
                        "im_discovered_peer_saved",
                        "completed",
                        "已记录飞书待授权对象。",
                    )
                    .metadata(json!({
                        "providerId": IM_PROVIDER_FEISHU,
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
                    "im_discovered_peer_save",
                    "failed",
                    error,
                )
                .metadata(json!({ "providerId": IM_PROVIDER_FEISHU, "eventHash": event_hash })),
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

    let is_group_chat = is_group_chat_event(event);

    if is_group_chat {
        if !settings
            .allowed_chat_ids
            .iter()
            .any(|chat_id| chat_id == &event.chat_id)
        {
            return Err(block_reason("飞书群聊不在允许名单中。"));
        }
        // 卡片 action 是用户主动点击已发送的机器人卡片，不携带消息 mention；不能因此误拦截。
        if settings.require_mention && event.kind != "card_action" && !is_direct_bot_mention(event)
        {
            return Err(block_reason("飞书群聊消息未直接 @ 机器人。"));
        }
    }

    Ok(())
}

/** 处理已由 sidecar 规范化的卡片 action，变更 ID 仍会在审批服务中二次校验。 */
async fn handle_card_action_for_event(app: &AppHandle, event: &FeishuInboundEvent) -> String {
    let action = normalize_pending_change_action(&event.action);
    if action.is_none() || event.change_id.trim().is_empty() {
        return "卡片操作无效或已过期，请使用“详情 <编号>”查看当前待确认改动。".to_owned();
    }

    handle_pending_change_command_for_event(
        app,
        event,
        action.unwrap_or_default(),
        &event.change_id,
    )
    .await
}

/** 将来自文字或卡片的审批操作统一委托给不依赖前端 WorkspaceSnapshot 的服务接口。 */
async fn handle_pending_change_command_for_event(
    app: &AppHandle,
    event: &FeishuInboundEvent,
    action: &str,
    change_token: &str,
) -> String {
    crate::commands::handle_im_pending_change_command(
        app.clone(),
        IM_PROVIDER_FEISHU,
        &build_channel_key(event),
        action,
        change_token,
    )
    .await
}

/** 解析 IM 文字兜底指令；只接受“详情/确认/取消 + 单个编号”，其他文本继续交给 Agent。 */
fn parse_pending_change_text_command(text: &str) -> Option<(&str, &str)> {
    let mut parts = text.split_whitespace();
    let action = normalize_pending_change_action(parts.next()?);
    let change_token = parts.next()?.trim();
    if action.is_none() || change_token.is_empty() || parts.next().is_some() {
        return None;
    }
    Some((action?, change_token))
}

/** 归一化卡片 name 与中文文字指令，避免审批服务暴露 provider 专属 action。 */
fn normalize_pending_change_action(action: &str) -> Option<&str> {
    match action.trim() {
        "details" | "orange_pending_details" | "详情" => Some("details"),
        "confirm" | "orange_pending_confirm" | "确认" => Some("confirm"),
        "cancel" | "orange_pending_cancel" | "取消" => Some("cancel"),
        _ => None,
    }
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
    let is_group_chat = is_group_chat_event(event);
    let sender_allowed = settings
        .allowed_user_open_ids
        .iter()
        .any(|open_id| open_id == &event.sender_open_id);
    let chat_allowed = settings
        .allowed_chat_ids
        .iter()
        .any(|chat_id| chat_id == &event.chat_id);

    json!({
        "providerId": IM_PROVIDER_FEISHU,
        "eventHash": event_hash,
        "senderHash": hash_identifier(&event.sender_open_id),
        "chatHash": hash_identifier(&event.chat_id),
        "chatType": event.chat_type,
        "isGroupChat": is_group_chat,
        "providerEnabled": settings.enabled,
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

/** 判断飞书事件是否来自群聊；chat_id 形态不能作为依据，单聊也可能出现 oc_* 会话 ID。 */
fn is_group_chat_event(event: &FeishuInboundEvent) -> bool {
    matches!(event.chat_type.as_str(), "group" | "topic_group")
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

/** 为飞书消息运行橘记 Agent，并返回可发送回飞书的短文本。 */
async fn run_agent_for_event(
    app: &AppHandle,
    event: &FeishuInboundEvent,
    settings: &FeishuIntegrationSettings,
) -> String {
    let channel_key = build_channel_key(event);
    let conversation_kind = if is_group_chat_event(event) {
        "group"
    } else {
        "direct"
    };
    // IM 身份只保存通道哈希和清洗后的摘要，避免原始 chat_id/open_id 进入持久化会话。
    let im_identity = super::build_im_session_identity(
        IM_PROVIDER_FEISHU,
        &channel_key,
        conversation_kind,
        &event.text,
    );
    let result = crate::commands::run_agent_turn_from_im(
        app.clone(),
        IM_PROVIDER_FEISHU.to_owned(),
        event.text.trim().to_owned(),
        channel_key,
        settings.default_knowledge_base_ids.clone(),
        im_identity,
    )
    .await;

    match result {
        Ok(snapshot) => {
            // 卡片发送失败不能阻断 Agent 正文回复；回复文本必须据此切换到可执行的文字指令。
            let card_sent = match send_pending_change_card_for_snapshot(
                app, event, settings, &snapshot,
            )
            .await
            {
                Ok(()) => true,
                Err(error) => {
                    logging::write_app_event_best_effort(
                        app,
                        AppEventBuilder::new(
                            AppLogLevel::Warn,
                            AppLogCategory::Im,
                            "im_pending_change_card_send",
                            "failed",
                            error,
                        )
                        .metadata(json!({
                            "providerId": IM_PROVIDER_FEISHU,
                            "chatHash": hash_identifier(&event.chat_id),
                        })),
                    );
                    false
                }
            };
            build_agent_reply_text(&snapshot, card_sent)
        }
        Err(error) if error.starts_with("当前有待确认变更") => error,
        Err(error) => format!("飞书消息处理失败：{}", logging::sanitize_log_text(&error)),
    }
}

/** 从当前 IM 会话的 pending change 构造并发送远程审批卡片。 */
async fn send_pending_change_card_for_snapshot(
    app: &AppHandle,
    event: &FeishuInboundEvent,
    settings: &FeishuIntegrationSettings,
    snapshot: &WorkspaceSnapshot,
) -> Result<(), String> {
    let session = snapshot
        .sessions
        .iter()
        .find(|session| session.id == snapshot.active_session_id)
        .or_else(|| snapshot.sessions.first());
    let Some(change) = session.and_then(|session| {
        session
            .pending_change
            .as_ref()
            .filter(|change| change.status == "pending")
    }) else {
        return Ok(());
    };
    let stats = change.diff_stats.as_ref();
    let card = FeishuPendingChangeCard {
        chat_id: event.chat_id.clone(),
        chat_type: event.chat_type.clone(),
        change_id: change.id.clone(),
        short_code: crate::commands::short_change_code(&change.id),
        target_path: change.target_path.clone(),
        operation_label: pending_change_operation_label(change),
        added_lines: stats.map(|value| value.added_lines).unwrap_or(0),
        removed_lines: stats.map(|value| value.removed_lines).unwrap_or(0),
        // 仅发送变更后内容的短预览；完整 diff 必须通过“详情”操作按需获取。
        preview: truncate_chars(change.next.trim(), MAX_FEISHU_CARD_PREVIEW_CHARS),
    };
    send_pending_change_card(app, settings, &card).await
}

/** 将变更 operation 归一为 IM 卡片可读标签，兼容旧数据仅含 type 的情况。 */
fn pending_change_operation_label(change: &crate::domain::ProposedChange) -> String {
    match change.operation.as_deref().unwrap_or(&change.r#type) {
        "create" | "new" => "新建".to_owned(),
        "append" => "追加".to_owned(),
        "rewrite" | "update" | "edit" => "改写".to_owned(),
        value if !value.trim().is_empty() => value.to_owned(),
        _ => "更新".to_owned(),
    }
}

/** 从最新 assistant 消息构造飞书回复；待确认 diff 改为在同一会话内审批。 */
fn build_agent_reply_text(snapshot: &WorkspaceSnapshot, card_sent: bool) -> String {
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

    if let Some(change) = session
        .pending_change
        .as_ref()
        .filter(|change| change.status == "pending")
    {
        append_pending_change_reply_hint(&mut reply, change, card_sent);
    }

    reply
}

/**
 * 补充 IM 审批提示：卡片成功时只引导点击按钮；发送失败时才暴露可复制的文字降级指令。
 * 变更短码仅用于人工降级，真实授权始终由 Rust 审批服务按会话身份和 pending 状态判断。
 */
fn append_pending_change_reply_hint(
    reply: &mut String,
    change: &crate::domain::ProposedChange,
    card_sent: bool,
) {
    if card_sent {
        reply.push_str("\n\n已发送审批卡片，可点击查看详情、确认写入或取消。");
        return;
    }

    let short_code = crate::commands::short_change_code(&change.id);
    reply.push_str(&format!(
        "\n\n审批卡片暂不可用，请使用下方文字指令。\n变更编号：{short_code}\n详情：详情 {short_code}\n确认：确认 {short_code}\n取消：取消 {short_code}"
    ));
}

/** 构造稳定 IM 会话 key；群聊按用户隔离，私聊按 sender 隔离。 */
fn build_channel_key(event: &FeishuInboundEvent) -> String {
    let is_group_chat = is_group_chat_event(event);

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

    let app_secret = storage::load_im_provider_secret(IM_PROVIDER_FEISHU)?
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
            "im_reply_api",
            "completed",
            "飞书发送 API 调用完成。",
        )
        .metadata(json!({
            "providerId": IM_PROVIDER_FEISHU,
            "chatHash": hash_identifier(&event.chat_id)
        })),
    );

    Ok(())
}

/**
 * 向飞书会话发送远程审批卡片。
 *
 * 审批服务在创建 pending change 后调用本函数；卡片 value 只带不可猜测的完整变更 ID，
 * 实际鉴权、过期判断和文件写入始终由收到 action 后的 Rust 服务完成。
 */
pub async fn send_pending_change_card(
    app: &AppHandle,
    settings: &FeishuIntegrationSettings,
    card: &FeishuPendingChangeCard,
) -> Result<(), String> {
    rate_limit_send_target(&card.chat_id).await?;

    let app_secret = storage::load_im_provider_secret(IM_PROVIDER_FEISHU)?
        .ok_or_else(|| "飞书 appSecret 未配置，无法发送审批卡片。".to_owned())?;
    let token = fetch_tenant_access_token(&settings.domain, &settings.app_id, &app_secret).await?;
    let base_url = feishu_base_url(&settings.domain);
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| format!("无法创建飞书 HTTP client：{error}"))?;
    let content = build_pending_change_card_content(card)?;
    let response = client
        .post(format!("{base_url}/open-apis/im/v1/messages"))
        .bearer_auth(token)
        .query(&[("receive_id_type", "chat_id")])
        .json(&json!({
            "receive_id": card.chat_id,
            "msg_type": "interactive",
            "content": content,
        }))
        .send()
        .await
        .map_err(|error| format!("无法发送飞书审批卡片：{error}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("无法读取飞书审批卡片响应：{error}"))?;

    ensure_feishu_send_success(status, &body, "审批卡片")?;
    logging::write_app_event_best_effort(
        app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Im,
            "im_pending_change_card_sent",
            "completed",
            "飞书待确认变更卡片已发送。",
        )
        .metadata(json!({
            "providerId": IM_PROVIDER_FEISHU,
            "changeHash": hash_identifier(&card.change_id),
            "chatHash": hash_identifier(&card.chat_id),
            "operation": card.operation_label,
            "addedLines": card.added_lines,
            "removedLines": card.removed_lines,
        })),
    );
    Ok(())
}

/** 构造飞书 Card 2.0 JSON；默认只显示摘要，详情操作由服务端再发送截断 diff。 */
fn build_pending_change_card_content(card: &FeishuPendingChangeCard) -> Result<String, String> {
    let preview = truncate_chars(card.preview.trim(), MAX_FEISHU_CARD_PREVIEW_CHARS);
    serde_json::to_string(&json!({
        "schema": "2.0",
        "config": { "width_mode": "fill" },
        "header": {
            "title": { "tag": "plain_text", "content": "橘记：待确认笔记改动" },
            "template": "orange"
        },
        "body": {
            "elements": [
                {
                    "tag": "markdown",
                    "content": format!(
                        "**操作**：{}\\n**目标**：`{}`\\n**变更编号**：`{}`\\n**行数**：+{} / -{}{}",
                        card.operation_label,
                        card.target_path,
                        card.short_code,
                        card.added_lines,
                        card.removed_lines,
                        if preview.is_empty() { String::new() } else { format!("\\n\\n**预览**\\n{}", preview) },
                    )
                },
                {
                    // Card 2.0 不支持 action 容器；三个按钮必须放入 column_set -> column。
                    "tag": "column_set",
                    "flex_mode": "none",
                    "horizontal_spacing": "8px",
                    "columns": [
                        pending_change_card_column(card_action_button("详情", "details", "default", card)),
                        pending_change_card_column(card_action_button("确认写入", "confirm", "primary", card)),
                        pending_change_card_column(card_action_button("取消", "cancel", "danger", card))
                    ]
                }
            ]
        }
    }))
    .map_err(|error| format!("无法序列化飞书审批卡片：{error}"))
}

/** 包装审批按钮为 Card 2.0 列；三个等权列在手机和桌面端均保持稳定布局。 */
fn pending_change_card_column(button: Value) -> Value {
    json!({
        "tag": "column",
        "width": "weighted",
        "weight": 1,
        "vertical_align": "top",
        "elements": [button]
    })
}

/**
 * 创建 Card 2.0 callback 按钮；完整 change_id 仅存在于版本化回调 value，短码只面向用户展示。
 * Rust 在收到 callback 后仍会按发送人、会话和 pending 状态重新鉴权，不信任卡片值本身。
 */
fn card_action_button(
    text: &str,
    action: &str,
    button_type: &str,
    card: &FeishuPendingChangeCard,
) -> Value {
    json!({
        "tag": "button",
        "text": { "tag": "plain_text", "content": text },
        "type": button_type,
        "behaviors": [{
            "type": "callback",
            "value": {
                // Card 2.0 callback 协议版本；sidecar 只接受该精确标记。
                "orange": "pending_change.v1",
                "action": action,
                "changeId": card.change_id,
                // 供 callback 在不读取原始 payload 的前提下保留私聊/群聊鉴权分支。
                "chatType": card.chat_type
            }
        }]
    })
}

/** 统一校验飞书发送响应，确保卡片和文本使用一致的错误脱敏策略。 */
fn ensure_feishu_send_success(
    status: reqwest::StatusCode,
    body: &str,
    label: &str,
) -> Result<(), String> {
    if !status.is_success() {
        return Err(format!(
            "飞书{label}发送失败：HTTP {status} {}",
            logging::sanitize_log_text(body)
        ));
    }
    let value: Value = serde_json::from_str(body)
        .map_err(|error| format!("无法解析飞书{label}发送响应：{error}"))?;
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        return Err(format!(
            "飞书{label}发送失败：code={} msg={}",
            code,
            value
                .get("msg")
                .and_then(Value::as_str)
                .map(logging::sanitize_log_text)
                .unwrap_or_else(|| "unknown".to_owned())
        ));
    }
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

    /** 文字兜底审批指令必须严格包含一个短码，避免普通自然语言被误认为写入操作。 */
    #[test]
    fn parses_pending_change_text_commands() {
        assert_eq!(
            parse_pending_change_text_command("确认 ab12cd"),
            Some(("confirm", "ab12cd"))
        );
        assert_eq!(
            parse_pending_change_text_command("详情 ab12cd"),
            Some(("details", "ab12cd"))
        );
        assert!(parse_pending_change_text_command("确认").is_none());
        assert!(parse_pending_change_text_command("确认 ab12cd 多余内容").is_none());
    }

    /** 卡片发送成功后不应再要求用户手动输入编号；失败时必须保留文字降级入口。 */
    #[test]
    fn pending_change_reply_prefers_card_and_keeps_text_fallback() {
        let change = crate::domain::ProposedChange {
            id: "change-1234567890".to_owned(),
            knowledge_base_id: "kb-a".to_owned(),
            note_id: Some("note-a".to_owned()),
            target_id: Some("note-a".to_owned()),
            target_kind: Some("note".to_owned()),
            file_type: Some("markdown".to_owned()),
            r#type: "rewrite".to_owned(),
            operation: Some("replace".to_owned()),
            title: "改写测试笔记".to_owned(),
            target_path: "notes/example.md".to_owned(),
            original: "旧内容".to_owned(),
            next: "新内容".to_owned(),
            original_hash: "hash".to_owned(),
            status: "pending".to_owned(),
            review_comments: None,
            review_state: None,
            diff_stats: None,
        };
        let mut card_reply = "Agent 已生成改动。".to_owned();
        let mut fallback_reply = card_reply.clone();

        append_pending_change_reply_hint(&mut card_reply, &change, true);
        append_pending_change_reply_hint(&mut fallback_reply, &change, false);

        assert!(card_reply.contains("已发送审批卡片"));
        assert!(!card_reply.contains("确认：确认"));
        assert!(fallback_reply.contains("审批卡片暂不可用"));
        assert!(fallback_reply.contains("确认：确认 change-12345"));
    }

    /** Card 2.0 按钮必须通过版本化 callback behavior 传递受控变更操作。 */
    #[test]
    fn pending_change_card_uses_versioned_callback_actions() {
        let card = FeishuPendingChangeCard {
            chat_id: "oc_private".to_owned(),
            chat_type: "p2p".to_owned(),
            change_id: "unguessable-change-id".to_owned(),
            short_code: "ab12cd".to_owned(),
            target_path: "notes/example.md".to_owned(),
            operation_label: "追加".to_owned(),
            added_lines: 2,
            removed_lines: 1,
            preview: "简短预览".to_owned(),
        };
        let content = build_pending_change_card_content(&card).expect("card JSON should serialize");
        let value: Value = serde_json::from_str(&content).expect("card JSON should parse");
        let elements = value
            .pointer("/body/elements")
            .and_then(Value::as_array)
            .expect("card should contain body elements");
        let actions = elements
            .iter()
            .find(|element| element.get("tag").and_then(Value::as_str) == Some("column_set"))
            .and_then(|element| element.get("columns"))
            .and_then(Value::as_array)
            .expect("card should contain approval button columns");

        assert!(content.contains("确认写入"));
        assert!(content.contains("ab12cd"));
        assert!(!content.contains("\"tag\":\"action\""));
        // 飞书 Card 2.0 同样不支持旧版 note；文字降级由独立文本消息承载。
        assert!(!content.contains("\"tag\":\"note\""));
        assert_eq!(elements.len(), 2);
        assert_eq!(
            value.pointer("/schema").and_then(Value::as_str),
            Some("2.0")
        );
        assert_eq!(
            value.pointer("/config/width_mode").and_then(Value::as_str),
            Some("fill")
        );

        let expected_actions = ["details", "confirm", "cancel"];
        assert_eq!(actions.len(), expected_actions.len());
        for (column, expected_action) in actions.iter().zip(expected_actions) {
            assert_eq!(column.get("tag").and_then(Value::as_str), Some("column"));
            assert_eq!(
                column.get("width").and_then(Value::as_str),
                Some("weighted")
            );
            assert_eq!(column.get("weight").and_then(Value::as_u64), Some(1));
            let button = column
                .pointer("/elements/0")
                .expect("approval column should contain a button");
            let callback_value = button
                .pointer("/behaviors/0/value")
                .expect("button should use a callback behavior");
            assert_eq!(
                button.pointer("/behaviors/0/type").and_then(Value::as_str),
                Some("callback")
            );
            assert_eq!(
                callback_value.get("orange").and_then(Value::as_str),
                Some("pending_change.v1")
            );
            assert_eq!(
                callback_value.get("action").and_then(Value::as_str),
                Some(expected_action)
            );
            assert_eq!(
                callback_value.get("changeId").and_then(Value::as_str),
                Some("unguessable-change-id")
            );
            assert_eq!(
                callback_value.get("chatType").and_then(Value::as_str),
                Some("p2p")
            );
            assert!(callback_value.get("targetPath").is_none());
            assert!(callback_value.get("preview").is_none());
            assert!(button.get("name").is_none());
            assert!(button.get("value").is_none());
        }
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
            action: String::new(),
            change_id: String::new(),
        };

        assert_eq!(build_channel_key(&event), build_channel_key(&event));
        assert!(build_channel_key(&event).starts_with("feishu:group:"));
    }

    /** 单聊可能使用 oc_* chat_id，但只要 chatType 是 p2p，就不能套用群聊 @ 规则。 */
    #[test]
    fn p2p_chat_with_oc_chat_id_is_not_treated_as_group() {
        let settings = FeishuIntegrationSettings {
            enabled: true,
            domain: "feishu".to_owned(),
            app_id: "cli_x".to_owned(),
            secret_key_reference: "secret".to_owned(),
            default_knowledge_base_ids: vec!["kb".to_owned()],
            allowed_user_open_ids: vec!["ou_user".to_owned()],
            allowed_chat_ids: Vec::new(),
            discovered_user_open_ids: Vec::new(),
            discovered_chat_ids: Vec::new(),
            require_mention: true,
            updated_at: "now".to_owned(),
        };
        let event = FeishuInboundEvent {
            kind: "message".to_owned(),
            event_id: "evt".to_owned(),
            message_id: "msg".to_owned(),
            chat_id: "oc_p2p_session".to_owned(),
            chat_type: "p2p".to_owned(),
            sender_open_id: "ou_user".to_owned(),
            message_type: "text".to_owned(),
            text: "hello".to_owned(),
            mentions: Vec::new(),
            action: String::new(),
            change_id: String::new(),
        };

        assert!(decide_event_handling(&settings, &event).is_ok());
        assert!(build_channel_key(&event).starts_with("feishu:dm:"));
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
            action: String::new(),
            change_id: String::new(),
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
            action: String::new(),
            change_id: String::new(),
        };

        assert!(decide_event_handling(&settings, &event).is_ok());
        let mut blocked = event.clone();
        blocked.sender_open_id = "ou_other".to_owned();
        assert!(decide_event_handling(&settings, &blocked).is_err());
    }

    /** 未启用 provider 时不允许启动网关，避免 sidecar 已运行但消息统一被阻断。 */
    #[test]
    fn gateway_validation_rejects_disabled_provider() {
        let settings = FeishuIntegrationSettings {
            enabled: false,
            domain: "feishu".to_owned(),
            app_id: "cli_x".to_owned(),
            secret_key_reference: "secret".to_owned(),
            default_knowledge_base_ids: vec!["kb".to_owned()],
            allowed_user_open_ids: vec!["ou_user".to_owned()],
            allowed_chat_ids: Vec::new(),
            discovered_user_open_ids: Vec::new(),
            discovered_chat_ids: Vec::new(),
            require_mention: true,
            updated_at: "now".to_owned(),
        };

        assert_eq!(
            validate_gateway_settings(&settings).unwrap_err(),
            "请先启用飞书/Lark 集成。"
        );
    }

    /** 首次配置没有用户白名单时仍应允许启动网关，以接收事件并自动发现 open_id。 */
    #[test]
    fn gateway_validation_allows_empty_user_allowlist_for_discovery() {
        let settings = FeishuIntegrationSettings {
            enabled: true,
            domain: "feishu".to_owned(),
            app_id: "cli_x".to_owned(),
            secret_key_reference: "secret".to_owned(),
            default_knowledge_base_ids: vec!["kb".to_owned()],
            allowed_user_open_ids: Vec::new(),
            allowed_chat_ids: Vec::new(),
            discovered_user_open_ids: Vec::new(),
            discovered_chat_ids: Vec::new(),
            require_mention: true,
            updated_at: "now".to_owned(),
        };

        assert!(validate_gateway_settings(&settings).is_ok());
    }

    /** 仅发现模式仍必须默认拒绝普通消息，避免空白名单被误解为允许所有用户。 */
    #[test]
    fn discovery_mode_blocks_messages_before_agent_execution() {
        let settings = FeishuIntegrationSettings {
            enabled: true,
            domain: "feishu".to_owned(),
            app_id: "cli_x".to_owned(),
            secret_key_reference: "secret".to_owned(),
            default_knowledge_base_ids: vec!["kb".to_owned()],
            allowed_user_open_ids: Vec::new(),
            allowed_chat_ids: Vec::new(),
            discovered_user_open_ids: Vec::new(),
            discovered_chat_ids: Vec::new(),
            require_mention: true,
            updated_at: "now".to_owned(),
        };
        let event = FeishuInboundEvent {
            kind: "message".to_owned(),
            event_id: "evt".to_owned(),
            message_id: "msg".to_owned(),
            chat_id: "oc_p2p_session".to_owned(),
            chat_type: "p2p".to_owned(),
            sender_open_id: "ou_candidate".to_owned(),
            message_type: "text".to_owned(),
            text: "hello".to_owned(),
            mentions: Vec::new(),
            action: String::new(),
            change_id: String::new(),
        };

        let block = decide_event_handling(&settings, &event).unwrap_err();

        assert_eq!(block.reason, "飞书发送人不在允许名单中。");
    }

    /** 飞书通过通用解析器精确识别内置指令，未知斜杠文本仍交由 Agent。 */
    #[test]
    fn builtin_commands_use_common_exact_parser() {
        assert_eq!(
            super::super::parse_builtin_command(" /new "),
            Some(super::super::ImBuiltinCommand::New)
        );
        assert_eq!(
            super::super::parse_builtin_command("/reset"),
            Some(super::super::ImBuiltinCommand::New)
        );
        assert_eq!(super::super::parse_builtin_command("/compact now"), None);
        assert_eq!(super::super::parse_builtin_command("/unknown"), None);
    }
}
