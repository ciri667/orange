use crate::domain::{
    AgentMessage, AgentSession, AgentTurnRequest, ImGatewayStatus, ImSessionIdentity,
    IM_PROVIDER_FEISHU,
};
use crate::storage::{create_id, format_local_datetime};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub mod feishu;

/** 会话摘要最大字符数，兼顾历史列表扫描效率和本地消息内容最小暴露。 */
const IM_MESSAGE_PREVIEW_MAX_CHARS: usize = 28;

/** IM 内置指令；provider 在完成鉴权、去重和群聊门禁后据此分派，不会把命令发送给模型。 */
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ImBuiltinCommand {
    Help,
    New,
    Compact,
}

/** 精确识别首版 IM 内置指令；仅允许首尾空白，未知斜杠文本仍作为普通 Agent 提问。 */
pub(crate) fn parse_builtin_command(text: &str) -> Option<ImBuiltinCommand> {
    match text.trim() {
        "/help" => Some(ImBuiltinCommand::Help),
        "/new" | "/reset" => Some(ImBuiltinCommand::New),
        "/compact" => Some(ImBuiltinCommand::Compact),
        _ => None,
    }
}

/** 返回内置指令说明；审批指令同时列出，降低移动端发现成本。 */
pub(crate) fn builtin_command_help_text() -> &'static str {
    "内置指令：\n/help：查看指令说明\n/new：开启新的 Agent 会话（/reset 仍可用）\n/compact：整理当前会话上下文\n/status：查看连接状态\n\n待确认变更：发送“详情 <编号>”、“确认 <编号>”或“取消 <编号>”。"
}

/** 生成新 IM 会话的固定身份摘要，避免把 `/new` 或旧消息误作为会话主题。 */
pub(crate) fn build_im_new_session_identity(identity: &ImSessionIdentity) -> ImSessionIdentity {
    ImSessionIdentity {
        provider_id: identity.provider_id.clone(),
        conversation_kind: identity.conversation_kind.clone(),
        channel_hash: identity.channel_hash.clone(),
        initial_message_preview: "新会话".to_owned(),
        last_message_preview: "新会话".to_owned(),
    }
}

/** 按 provider、会话类型和用户消息生成通用 IM 身份，供后续任意 IM 接入复用。 */
pub(crate) fn build_im_session_identity(
    provider_id: &str,
    channel_key: &str,
    conversation_kind: &str,
    message: &str,
) -> ImSessionIdentity {
    let preview = build_im_message_preview(message);

    ImSessionIdentity {
        provider_id: provider_id.to_owned(),
        conversation_kind: normalize_conversation_kind(conversation_kind).to_owned(),
        // 通道 hash 仅用于稳定区分和审计关联，不能反推外部平台的聊天或用户 ID。
        channel_hash: crate::storage::hash_content(channel_key)
            .chars()
            .take(16)
            .collect(),
        initial_message_preview: preview.clone(),
        last_message_preview: preview,
    }
}

/** 从持久化 channel key 恢复会话身份，供旧版“飞书会话”懒迁移使用。 */
pub(crate) fn build_im_session_identity_from_channel_key(
    channel_key: Option<&str>,
    initial_message: &str,
    last_message: &str,
) -> ImSessionIdentity {
    let (provider_id, conversation_kind) = channel_key
        .map(parse_channel_key_identity)
        .unwrap_or((IM_PROVIDER_FEISHU, "unknown"));
    let fallback_key = channel_key.unwrap_or("legacy:feishu:unknown");
    let mut identity = build_im_session_identity(
        provider_id,
        fallback_key,
        conversation_kind,
        initial_message,
    );
    identity.last_message_preview = build_im_message_preview(last_message);
    identity
}

/** 构造稳定 IM 会话标题；名称只随首条消息确定，后续消息更新最近摘要而不改标题。 */
pub(crate) fn format_im_session_title(identity: &ImSessionIdentity) -> String {
    format!(
        "{} · {} · {}",
        get_im_provider_label(&identity.provider_id),
        get_im_conversation_kind_label(&identity.conversation_kind),
        identity.initial_message_preview,
    )
}

/** 生成适合列表展示的消息摘要，移除机器人占位提及并按 Unicode 字符截断。 */
pub(crate) fn build_im_message_preview(message: &str) -> String {
    let normalized = message
        .split_whitespace()
        .filter(|part| !part.starts_with("@_user_"))
        .collect::<Vec<_>>()
        .join(" ");
    let trimmed = normalized.trim();

    if trimmed.is_empty() {
        return "未命名对话".to_owned();
    }

    let mut preview = trimmed
        .chars()
        .take(IM_MESSAGE_PREVIEW_MAX_CHARS)
        .collect::<String>();
    if trimmed.chars().count() > IM_MESSAGE_PREVIEW_MAX_CHARS {
        preview.push('…');
    }
    preview
}

/** 将外部 provider ID 转换为界面文案，未知 provider 保留其 ID 以支持后续扩展。 */
pub(crate) fn get_im_provider_label(provider_id: &str) -> &str {
    match provider_id {
        IM_PROVIDER_FEISHU => "飞书",
        _ => provider_id,
    }
}

/** 将通用会话类型转成用户可读标签。 */
pub(crate) fn get_im_conversation_kind_label(conversation_kind: &str) -> &str {
    match conversation_kind {
        "direct" => "私聊",
        "group" => "群聊",
        _ => "对话",
    }
}

/** 归一化会话类型，防止外部 provider 传入未支持枚举污染持久化模型。 */
fn normalize_conversation_kind(conversation_kind: &str) -> &str {
    match conversation_kind {
        "direct" | "group" => conversation_kind,
        _ => "unknown",
    }
}

/** 解析现有 channel key 的 provider 和聊天类型；key 本身不进入 UI 或日志。 */
fn parse_channel_key_identity(channel_key: &str) -> (&str, &str) {
    let mut parts = channel_key.split(':');
    let provider_id = parts
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(IM_PROVIDER_FEISHU);
    let conversation_kind = match parts.next() {
        Some("dm") => "direct",
        Some("group") => "group",
        _ => "unknown",
    };

    (provider_id, conversation_kind)
}

/** 启动指定 IM provider 的网关；新增 provider 时在这里增加路由。 */
pub async fn start_gateway(app: AppHandle, provider_id: &str) -> Result<ImGatewayStatus, String> {
    match provider_id {
        IM_PROVIDER_FEISHU => feishu::start_gateway(app).await,
        _ => Err(format!("暂不支持启动 IM provider {provider_id} 的网关。")),
    }
}

/** 停止指定 IM provider 的网关；不会清空配置或凭证。 */
pub fn stop_gateway(app: &AppHandle, provider_id: &str) -> Result<ImGatewayStatus, String> {
    match provider_id {
        IM_PROVIDER_FEISHU => feishu::stop_gateway(app),
        _ => Err(format!("暂不支持停止 IM provider {provider_id} 的网关。")),
    }
}

/** 读取指定 IM provider 的网关状态；状态中只包含脱敏诊断信息。 */
pub fn load_gateway_status(app: &AppHandle, provider_id: &str) -> Result<ImGatewayStatus, String> {
    match provider_id {
        IM_PROVIDER_FEISHU => feishu::load_gateway_status(app),
        _ => Err(format!(
            "暂不支持读取 IM provider {provider_id} 的网关状态。"
        )),
    }
}

/** 定位 IM sidecar 二进制；开发态和打包态都使用 provider registry 产物。 */
pub fn sidecar_binary_path(
    app: &AppHandle,
    provider_id: &str,
    binary_name: &str,
) -> Result<PathBuf, String> {
    let file_name = if cfg!(target_os = "windows") {
        format!("{binary_name}.exe")
    } else {
        binary_name.to_owned()
    };
    let dev_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("sidecars")
        .join("bin")
        .join(&file_name);

    if is_executable_file(&dev_path) {
        return Ok(dev_path);
    }

    app.path()
        .resource_dir()
        .map(|path| path.join(&file_name))
        .map_err(|error| format!("无法定位应用资源目录：{error}"))
        .and_then(|path| {
            if is_executable_file(&path) {
                Ok(path)
            } else {
                Err(format!(
                    "IM sidecar {provider_id} 尚未构建或不可执行，请先运行 npm run sidecar:im:build -- --provider {provider_id}。"
                ))
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

/** 构造 IM AgentSession，供 commands helper 创建新会话时复用。 */
pub(crate) fn build_im_agent_session(
    identity: ImSessionIdentity,
    knowledge_base_ids: Vec<String>,
) -> AgentSession {
    let now = format_local_datetime();

    AgentSession {
        id: create_id("session-im"),
        title: format_im_session_title(&identity),
        im_identity: Some(identity),
        r#type: "knowledge-base".to_owned(),
        knowledge_base_ids,
        active_note_id: None,
        pinned_note_ids: Vec::new(),
        messages: Vec::new(),
        pending_change: None,
        context_summary: None,
        created_at: now.clone(),
        updated_at: now,
        deleted_at: None,
        model_provider_id: None,
        model_id: None,
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
        mentioned_file_ids: Vec::new(),
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
        model_id: None,
        explicit_skill_ids: Vec::new(),
        mentioned_file_ids: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_im_message_preview, build_im_new_session_identity, build_im_session_identity,
        format_im_session_title, parse_builtin_command, ImBuiltinCommand,
    };

    /** 标题摘要需折叠空白、剔除机器人占位提及，并按字符而不是字节截断。 */
    #[test]
    fn im_preview_normalizes_mentions_whitespace_and_long_text() {
        let message = "  @_user_bot\n整理  本周会议纪要，并输出后续待办事项和负责人。";
        let preview = build_im_message_preview(message);

        assert_eq!(preview, "整理 本周会议纪要，并输出后续待办事项和负责人。");
    }

    /** 空内容必须生成稳定的兜底主题，避免会话标题为空。 */
    #[test]
    fn im_preview_falls_back_for_empty_content() {
        assert_eq!(build_im_message_preview("  @_user_bot  "), "未命名对话");
    }

    /** 新会话标题必须携带来源、聊天类型和首条主题，通道原文不得进入标题。 */
    #[test]
    fn im_identity_builds_stable_title_without_raw_channel_key() {
        let identity = build_im_session_identity(
            "feishu",
            "feishu:group:secret-chat:secret-user",
            "group",
            "请整理项目风险",
        );

        assert_eq!(
            format_im_session_title(&identity),
            "飞书 · 群聊 · 请整理项目风险"
        );
        assert_ne!(
            identity.channel_hash,
            "feishu:group:secret-chat:secret-user"
        );
        assert_eq!(identity.channel_hash.len(), 16);
    }

    /** 指令必须精确匹配，旧 `/reset` 兼容新会话语义。 */
    #[test]
    fn parses_builtin_commands_without_capturing_normal_prompts() {
        assert_eq!(
            parse_builtin_command(" /help "),
            Some(ImBuiltinCommand::Help)
        );
        assert_eq!(parse_builtin_command("/new"), Some(ImBuiltinCommand::New));
        assert_eq!(parse_builtin_command("/reset"), Some(ImBuiltinCommand::New));
        assert_eq!(
            parse_builtin_command("/compact"),
            Some(ImBuiltinCommand::Compact)
        );
        assert_eq!(parse_builtin_command("/help more"), None);
        assert_eq!(parse_builtin_command("/unknown"), None);
    }

    /** 新会话的标题不得泄露命令文本或继承上一轮主题。 */
    #[test]
    fn new_session_identity_uses_fixed_safe_preview() {
        let identity =
            build_im_session_identity("feishu", "feishu:dm:chat:user", "direct", "机密事项");
        let new_identity = build_im_new_session_identity(&identity);

        assert_eq!(
            format_im_session_title(&new_identity),
            "飞书 · 私聊 · 新会话"
        );
        assert_eq!(new_identity.channel_hash, identity.channel_hash);
    }
}
