//! Agent 结构化文件变更集的确认与拒绝入口。
//!
//! Skill 只提供说明和资源，真正的文件操作统一走 Agent 工具生成的变更集。
use crate::{domain::*, storage};
use tauri::AppHandle;

/** 应用持久化变更集；正文和路径从可信记录重载，前端只提交选择项。 */
pub fn apply_agent_change_set(
    app: &AppHandle,
    snapshot: WorkspaceSnapshot,
) -> Result<WorkspaceSnapshot, String> {
    let session = snapshot
        .sessions
        .iter()
        .find(|session| session.id == snapshot.active_session_id)
        .ok_or("会话不存在")?;
    let change_set = session.pending_change_set.as_ref().ok_or("没有变更集")?;
    crate::file_changes::apply(
        app,
        &session.id,
        &change_set.id,
        &change_set
            .operations
            .iter()
            .filter(|operation| operation.selected)
            .map(|operation| operation.id.clone())
            .collect::<Vec<_>>(),
    )
}

/** 拒绝当前变更集并持久化状态。 */
pub fn reject_agent_change_set(
    app: &AppHandle,
    snapshot: WorkspaceSnapshot,
) -> Result<WorkspaceSnapshot, String> {
    let mut trusted = storage::load_workspace_snapshot(app)?;
    let session = trusted
        .sessions
        .iter_mut()
        .find(|session| session.id == snapshot.active_session_id)
        .ok_or("会话不存在")?;
    if let Some(change_set) = session.pending_change_set.as_mut() {
        if matches!(change_set.status.as_str(), "pending" | "partial") {
            change_set.status = "rejected".into();
        }
    }
    storage::save_session_records(app, std::slice::from_ref(session))?;
    trusted.active_session_id = snapshot.active_session_id;
    Ok(trusted)
}

