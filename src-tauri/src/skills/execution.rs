//! 旧 Skill 执行 IPC 的兼容入口。运行永久停用，文件变更交给通用变更集。
use crate::{domain::*, storage};
use tauri::AppHandle;
pub fn approve_and_execute(_: &AppHandle, _: WorkspaceSnapshot) -> Result<WorkspaceSnapshot,String> {
    Err("旧 Skill 运行已停用；请重新读取指令并通过通用 exec 提出操作。旧产物副本保留。".into())
}
pub fn can_auto_execute(_: &AgentSession, _: &SkillExecutionRequest, _: &AgentSecuritySettings) -> bool { false }
pub fn reject_execution(app: &AppHandle, snapshot: WorkspaceSnapshot) -> Result<WorkspaceSnapshot,String> {
    let mut trusted=storage::load_workspace_snapshot(app)?;
    let session=trusted.sessions.iter_mut().find(|s|s.id==snapshot.active_session_id).ok_or("会话不存在")?;
    if let Some(request)=session.pending_execution.as_mut() { request.status="expired".into(); }
    storage::save_session_records(app,std::slice::from_ref(session))?; trusted.active_session_id=snapshot.active_session_id; Ok(trusted)
}
pub fn apply_change_set(app: &AppHandle, snapshot: WorkspaceSnapshot) -> Result<WorkspaceSnapshot,String> { apply_agent_change_set(app,snapshot) }
pub fn reject_change_set(app: &AppHandle, snapshot: WorkspaceSnapshot) -> Result<WorkspaceSnapshot,String> { reject_agent_change_set(app,snapshot) }
/** 兼容 payload 只提取标识及选择；正文和路径完全从持久化记录重载。 */
pub fn apply_agent_change_set(app: &AppHandle, snapshot: WorkspaceSnapshot) -> Result<WorkspaceSnapshot,String> {
    let session=snapshot.sessions.iter().find(|s|s.id==snapshot.active_session_id).ok_or("会话不存在")?;
    let set=session.pending_change_set.as_ref().ok_or("没有变更集")?;
    crate::file_changes::apply(app,&session.id,&set.id,&set.operations.iter().filter(|o|o.selected).map(|o|o.id.clone()).collect::<Vec<_>>())
}
pub fn reject_agent_change_set(app: &AppHandle, snapshot: WorkspaceSnapshot) -> Result<WorkspaceSnapshot,String> {
    let mut trusted=storage::load_workspace_snapshot(app)?;
    let session=trusted.sessions.iter_mut().find(|s|s.id==snapshot.active_session_id).ok_or("会话不存在")?;
    if let Some(set)=session.pending_change_set.as_mut() { if matches!(set.status.as_str(),"pending"|"partial") { set.status="rejected".into(); } }
    storage::save_session_records(app,std::slice::from_ref(session))?; trusted.active_session_id=snapshot.active_session_id; Ok(trusted)
}
pub fn can_auto_apply_agent_change_set(session: &AgentSession, settings: &AgentSecuritySettings) -> bool {
    crate::agent_writes::allows_autonomous_auto_apply(session,settings) && session.pending_change_set.as_ref().is_some_and(|s|s.status=="pending")
}
