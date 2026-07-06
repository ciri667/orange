use crate::domain::{
    AgentMessage, AgentSession, AgentTurnRequest, ImGatewayStatus, IM_PROVIDER_FEISHU,
};
use crate::storage::{create_id, format_local_datetime};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub mod feishu;

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
