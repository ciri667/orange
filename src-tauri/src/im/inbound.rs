use crate::domain::{
    ImProviderSettings, ProposedChange, WorkspaceSnapshot, IM_PROVIDER_FEISHU,
    IM_PROVIDER_QQ, IM_PROVIDER_WEIXIN,
};
use crate::logging::{self, AppEventBuilder, AppLogCategory, AppLogLevel};
use crate::storage;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tauri::AppHandle;

/** IM 文本回复默认截断长度，避免超平台可读边界。 */
pub(crate) const DEFAULT_IM_REPLY_MAX_CHARS: usize = 3500;

/** QQ 官方文本上限，超出后按段发送。 */
pub(crate) const QQ_REPLY_MAX_CHARS: usize = 2000;

/** 个人微信文本上限，超出后按段发送。 */
pub(crate) const WEIXIN_REPLY_MAX_CHARS: usize = 2000;

/**
 * sidecar 输出的标准化入站事件。飞书/QQ/微信共用同一 JSONL 契约；
 * 平台专属字段不要塞进这里，发送所需的 message_id 已包含在通用字段中。
 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImInboundEvent {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub event_id: String,
    #[serde(default)]
    pub message_id: String,
    #[serde(default)]
    pub chat_id: String,
    #[serde(default)]
    pub chat_type: String,
    #[serde(default)]
    pub sender_open_id: String,
    #[serde(default)]
    pub message_type: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub mentions: Vec<ImMention>,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub change_id: String,
    #[serde(default)]
    pub context_token: String,
}

/** 消息中的 @ 元数据；open_id=bot 表示直接 @ 了机器人。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImMention {
    pub open_id: String,
    #[serde(default)]
    pub name: String,
}

/** 拦截原因，日志只记录该文案，不拼接外部 ID。 */
pub(crate) struct ImBlockReason {
    pub reason: String,
}

/**
 * 每个 IM channel 的异步互斥锁。key 只留在进程内存，用来串行化
 * Agent turn、/new 与 /compact，防止旧快照覆盖最新会话状态。
 */
static IM_CHANNEL_OPERATION_LOCKS: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    OnceLock::new();

/** 返回 channel 专属的异步锁。 */
pub(crate) async fn acquire_channel_operation_lock(
    channel_key: &str,
) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = {
        let lock_table = IM_CHANNEL_OPERATION_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
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

/** 根据私聊/群聊、白名单和 @ 要求决定是否处理消息。 */
pub(crate) fn decide_event_handling(
    provider_id: &str,
    settings: &ImProviderSettings,
    event: &ImInboundEvent,
) -> Result<(), ImBlockReason> {
    let label = super::get_im_provider_label(provider_id);

    if !settings.enabled {
        return Err(block_reason(&format!("{label}集成未启用。")));
    }
    if !settings
        .allowed_user_open_ids
        .iter()
        .any(|open_id| open_id == &event.sender_open_id)
    {
        return Err(block_reason(&format!("{label}发送人不在允许名单中。")));
    }

    let is_group_chat = is_group_chat_event(event);

    if is_group_chat {
        if !settings
            .allowed_chat_ids
            .iter()
            .any(|chat_id| chat_id == &event.chat_id)
        {
            return Err(block_reason(&format!("{label}群聊不在允许名单中。")));
        }
        // 卡片 action 是用户主动点击已发送的机器人卡片，不携带消息 mention。
        if settings.require_mention && event.kind != "card_action" && !is_direct_bot_mention(event) {
            return Err(block_reason(&format!("{label}群聊消息未直接 @ 机器人。")));
        }
    }

    Ok(())
}

/** 判断群聊是否直接 @ bot；sidecar 会把 bot 自身 mention 标成 open_id=bot。 */
pub(crate) fn is_direct_bot_mention(event: &ImInboundEvent) -> bool {
    event
        .mentions
        .iter()
        .any(|mention| mention.open_id == "bot" || mention.name == "bot")
}

/** 判断事件是否来自群聊；不能用 chat_id 形态推断。 */
pub(crate) fn is_group_chat_event(event: &ImInboundEvent) -> bool {
    matches!(event.chat_type.as_str(), "group" | "topic_group")
}

/** 构造稳定 IM 会话 key；群聊按用户隔离，私聊按 sender 隔离。 */
pub(crate) fn build_channel_key(provider_id: &str, event: &ImInboundEvent) -> String {
    if is_group_chat_event(event) {
        format!(
            "{provider_id}:group:{}:{}",
            hash_identifier(&event.chat_id),
            hash_identifier(&event.sender_open_id)
        )
    } else {
        format!("{provider_id}:dm:{}", hash_identifier(&event.sender_open_id))
    }
}

/** 解析 IM 文字兜底指令；只接受“详情/确认/取消 + 单个编号”。 */
pub(crate) fn parse_pending_change_text_command(text: &str) -> Option<(&str, &str)> {
    let mut parts = text.split_whitespace();
    let action = normalize_pending_change_action(parts.next()?);
    let change_token = parts.next()?.trim();
    if action.is_none() || change_token.is_empty() || parts.next().is_some() {
        return None;
    }
    Some((action?, change_token))
}

/** 归一化卡片 name 与中文文字指令。 */
pub(crate) fn normalize_pending_change_action(action: &str) -> Option<&str> {
    match action.trim() {
        "details" | "orange_pending_details" | "详情" => Some("details"),
        "confirm" | "orange_pending_confirm" | "确认" => Some("confirm"),
        "cancel" | "orange_pending_cancel" | "取消" => Some("cancel"),
        _ => None,
    }
}

/** 分派已经完成鉴权与群聊门禁的文本事件。 */
pub(crate) async fn dispatch_authorized_text_event(
    app: &AppHandle,
    provider_id: &str,
    event: &ImInboundEvent,
    settings: &ImProviderSettings,
    channel_key: &str,
    card_sent: bool,
) -> String {
    if event.text.trim() == "/status" {
        return build_status_reply(app, provider_id, settings);
    }

    if let Some(command) = super::parse_builtin_command(&event.text) {
        let conversation_kind = if is_group_chat_event(event) {
            "group"
        } else {
            "direct"
        };
        let im_identity = super::build_im_session_identity(
            provider_id,
            channel_key,
            conversation_kind,
            "新会话",
        );
        return crate::commands::handle_im_builtin_command(
            app.clone(),
            provider_id,
            command,
            channel_key,
            settings.default_knowledge_base_ids.clone(),
            im_identity,
        )
        .await;
    }

    if let Some((action, change_token)) = parse_pending_change_text_command(&event.text) {
        return crate::commands::handle_im_pending_change_command(
            app.clone(),
            provider_id,
            channel_key,
            action,
            change_token,
        )
        .await;
    }

    run_agent_for_event(app, provider_id, event, settings, channel_key, card_sent).await
}

/** 为 IM 消息运行橘记 Agent，并返回可发送回平台的短文本。 */
pub(crate) async fn run_agent_for_event(
    app: &AppHandle,
    provider_id: &str,
    event: &ImInboundEvent,
    settings: &ImProviderSettings,
    channel_key: &str,
    card_sent: bool,
) -> String {
    let conversation_kind = if is_group_chat_event(event) {
        "group"
    } else {
        "direct"
    };
    let im_identity = super::build_im_session_identity(
        provider_id,
        channel_key,
        conversation_kind,
        &event.text,
    );
    let result = crate::commands::run_agent_turn_from_im(
        app.clone(),
        provider_id.to_owned(),
        event.text.trim().to_owned(),
        channel_key.to_owned(),
        settings.default_knowledge_base_ids.clone(),
        im_identity,
    )
    .await;
    let label = super::get_im_provider_label(provider_id);

    match result {
        Ok(snapshot) => build_agent_reply_text(&snapshot, card_sent),
        Err(error) if error.starts_with("当前有待确认变更") => error,
        Err(error) => format!("{label}消息处理失败：{}", logging::sanitize_log_text(&error)),
    }
}

/** 从最新 assistant 消息构造回复；待确认 diff 改为在同一会话内审批。 */
pub(crate) fn build_agent_reply_text(snapshot: &WorkspaceSnapshot, card_sent: bool) -> String {
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
    let mut reply = truncate_chars(assistant_content, DEFAULT_IM_REPLY_MAX_CHARS);

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
 */
pub(crate) fn append_pending_change_reply_hint(
    reply: &mut String,
    change: &ProposedChange,
    card_sent: bool,
) {
    if card_sent {
        reply.push_str("\n\n已发送审批卡片，可点击查看详情、确认写入或取消。");
        return;
    }

    let short_code = crate::commands::short_change_code(&change.id);
    reply.push_str(&format!(
        "\n\n请使用下方文字指令审批。\n变更编号：{short_code}\n详情：详情 {short_code}\n确认：确认 {short_code}\n取消：取消 {short_code}"
    ));
}

/** 构造 `/status` 回复，只展示脱敏配置和运行状态。 */
pub(crate) fn build_status_reply(
    app: &AppHandle,
    provider_id: &str,
    settings: &ImProviderSettings,
) -> String {
    let model_enabled = storage::load_user_settings(app)
        .map(|settings| settings.model_config.enabled)
        .unwrap_or(false);
    let status = super::load_gateway_status(app, provider_id).ok();
    let label = super::get_im_provider_label(provider_id);

    format!(
        "橘记 {label}集成状态：{}\n模型：{}\n默认知识库范围：{} 个\n群聊 @：{}",
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

/** 从消息保存可授权候选，返回是否完成保存尝试。 */
pub(crate) fn remember_discovered_peer_from_event(
    app: &AppHandle,
    provider_id: &str,
    event: &ImInboundEvent,
    event_hash: &str,
) -> bool {
    let is_group_chat = is_group_chat_event(event);
    match storage::remember_im_discovered_peer(
        app,
        provider_id,
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
                        "已记录待授权对象。",
                    )
                    .metadata(json!({
                        "providerId": provider_id,
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
                .metadata(json!({ "providerId": provider_id, "eventHash": event_hash })),
            );
            false
        }
    }
}

/** 生成拦截日志元数据，只包含 hash、数量和布尔状态。 */
pub(crate) fn block_metadata(
    provider_id: &str,
    event: &ImInboundEvent,
    settings: &ImProviderSettings,
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
        "providerId": provider_id,
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

/** 处理已鉴权的入站文本或卡片事件，返回应回发的正文。 */
pub(crate) async fn handle_authorized_event(
    app: AppHandle,
    provider_id: &str,
    event: ImInboundEvent,
    settings: ImProviderSettings,
    card_sent: bool,
) -> String {
    let started_at = std::time::Instant::now();
    let event_hash = hash_identifier(&event.event_id);
    let is_group_chat = is_group_chat_event(&event);

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Im,
            "im_message_received",
            "completed",
            "收到 IM 消息事件。",
        )
        .metadata(json!({
            "providerId": provider_id,
            "eventHash": event_hash,
            "messageHash": hash_identifier(&event.message_id),
            "chatHash": hash_identifier(&event.chat_id),
            "senderHash": hash_identifier(&event.sender_open_id),
            "messageType": event.message_type,
            "chatType": event.chat_type,
            "isGroupChat": is_group_chat,
        })),
    );

    if event.kind == "card_action" {
        let channel_key = build_channel_key(provider_id, &event);
        let _operation_guard = acquire_channel_operation_lock(&channel_key).await;
        let action = normalize_pending_change_action(&event.action);
        if action.is_none() || event.change_id.trim().is_empty() {
            return "卡片操作无效或已过期，请使用“详情 <编号>”查看当前待确认改动。".to_owned();
        }
        return crate::commands::handle_im_pending_change_command(
            app,
            provider_id,
            &channel_key,
            action.unwrap_or_default(),
            &event.change_id,
        )
        .await;
    }

    if event.message_type != "text" {
        let label = super::get_im_provider_label(provider_id);
        return format!("暂不支持该{label}消息类型；首版只处理文本消息。");
    }

    let channel_key = build_channel_key(provider_id, &event);
    let _operation_guard = acquire_channel_operation_lock(&channel_key).await;
    let reply =
        dispatch_authorized_text_event(&app, provider_id, &event, &settings, &channel_key, card_sent)
            .await;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Debug,
            AppLogCategory::Im,
            "im_message_dispatched",
            "completed",
            "IM 文本事件已分派。",
        )
        .duration(started_at.elapsed())
        .metadata(json!({
            "providerId": provider_id,
            "eventHash": event_hash,
            "replyChars": reply.chars().count(),
        })),
    );

    reply
}

/** 构造拦截原因。 */
pub(crate) fn block_reason(reason: &str) -> ImBlockReason {
    ImBlockReason {
        reason: reason.to_owned(),
    }
}

/** 对 open_id、chat_id、event_id 做稳定短 hash，日志只记录 hash。 */
pub(crate) fn hash_identifier(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
        .chars()
        .take(16)
        .collect()
}

/** 按字符截断文本，避免 UTF-8 边界被破坏。 */
pub(crate) fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }

    format!("{}...", value.chars().take(max_chars).collect::<String>())
}

/** 按字符把长回复切成多段，供有字数上限的平台逐条发送。 */
pub(crate) fn chunk_chars(value: &str, max_chars: usize) -> Vec<String> {
    if max_chars == 0 {
        return vec![value.to_owned()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();

    for ch in value.chars() {
        if current.chars().count() >= max_chars {
            chunks.push(std::mem::take(&mut current));
        }
        current.push(ch);
    }

    if !current.is_empty() || chunks.is_empty() {
        chunks.push(current);
    }

    chunks
}

/** 已知内置 provider 的展示名；未知 ID 原样返回。 */
pub(crate) fn known_provider_label(provider_id: &str) -> &str {
    match provider_id {
        IM_PROVIDER_FEISHU => "飞书",
        IM_PROVIDER_QQ => "QQ",
        IM_PROVIDER_WEIXIN => "微信",
        _ => provider_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /** 文字兜底审批指令必须严格包含一个短码。 */
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

    /** 群聊按用户隔离，私聊不把 chat_id 写入 key。 */
    #[test]
    fn channel_key_is_stable_and_group_is_per_user() {
        let event = ImInboundEvent {
            kind: "message".to_owned(),
            event_id: "evt".to_owned(),
            message_id: "msg".to_owned(),
            chat_id: "group-secret".to_owned(),
            chat_type: "group".to_owned(),
            sender_open_id: "user-secret".to_owned(),
            message_type: "text".to_owned(),
            text: "hello".to_owned(),
            mentions: Vec::new(),
            action: String::new(),
            change_id: String::new(),
            context_token: String::new(),
        };

        let key = build_channel_key(IM_PROVIDER_QQ, &event);
        assert_eq!(key, build_channel_key(IM_PROVIDER_QQ, &event));
        assert!(key.starts_with("qq:group:"));
        assert!(!key.contains("group-secret"));
        assert!(!key.contains("user-secret"));
    }

    /** 白名单拦截必须在群 @ 判断之前生效。 */
    #[test]
    fn policy_blocks_unknown_sender() {
        let settings = ImProviderSettings {
            provider_id: IM_PROVIDER_QQ.to_owned(),
            enabled: true,
            default_knowledge_base_ids: vec!["kb".to_owned()],
            allowed_user_open_ids: vec!["allowed".to_owned()],
            allowed_chat_ids: Vec::new(),
            discovered_user_open_ids: Vec::new(),
            discovered_chat_ids: Vec::new(),
            require_mention: true,
            updated_at: "now".to_owned(),
            config: crate::domain::ImProviderConfig::Qq(crate::domain::QqProviderConfig {
                app_id: "app".to_owned(),
                secret_key_reference: "ref".to_owned(),
            }),
        };
        let event = ImInboundEvent {
            kind: "message".to_owned(),
            event_id: "evt".to_owned(),
            message_id: "msg".to_owned(),
            chat_id: "dm".to_owned(),
            chat_type: "direct".to_owned(),
            sender_open_id: "stranger".to_owned(),
            message_type: "text".to_owned(),
            text: "hi".to_owned(),
            mentions: Vec::new(),
            action: String::new(),
            change_id: String::new(),
            context_token: String::new(),
        };

        assert!(decide_event_handling(IM_PROVIDER_QQ, &settings, &event).is_err());
    }

    /** 长回复必须按字符而不是字节切分。 */
    #[test]
    fn chunks_unicode_text_by_char_limit() {
        let chunks = chunk_chars("一二三四五六", 2);
        assert_eq!(chunks, vec!["一二".to_owned(), "三四".to_owned(), "五六".to_owned()]);
    }
}
