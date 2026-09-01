use super::agent::build_im_identity_event;
use super::common::*;

/** 读取持久化 Agent 会话，并按当前工作台快照清理已失效的知识库或笔记引用。 */
#[tauri::command]
pub async fn load_sessions(
    app: AppHandle,
    payload: LoadSessionsPayload,
) -> Result<Vec<AgentSession>, String> {
    run_blocking("读取 Agent 会话", move || {
        let mut sessions = storage::load_sessions_for_snapshot(&app, &payload.snapshot)?;
        let migrated_sessions = storage::migrate_legacy_im_session_identities(&app, &mut sessions)?;

        if !migrated_sessions.is_empty() {
            storage::save_session_records(&app, &migrated_sessions)?;
            for session in migrated_sessions {
                if let Some(identity) = session.im_identity {
                    logging::write_app_event_best_effort(
                        &app,
                        build_im_identity_event(&session.id, &identity, "migrated"),
                    );
                }
            }
        }

        Ok(sessions)
    })
    .await
}

/** 保存单个 Agent 会话，供前端创建会话或更新消息后统一进入 SQLite。 */
#[tauri::command]
pub async fn save_session(
    app: AppHandle,
    payload: SaveSessionPayload,
) -> Result<WorkspaceSnapshot, String> {
    run_blocking("保存 Agent 会话", move || {
        storage::save_session(&app, payload.snapshot, payload.session)
    })
    .await
}

/** 逻辑删除单个 Agent 会话；记录保留在 SQLite payload 中但不再进入普通会话列表。 */
#[tauri::command]
pub async fn delete_session(
    app: AppHandle,
    payload: DeleteSessionPayload,
) -> Result<WorkspaceSnapshot, String> {
    run_blocking("删除 Agent 会话", move || {
        storage::delete_session(&app, payload.snapshot, &payload.session_id)
    })
    .await
}

/** 更新当前会话工具范围；后端强制保留激活知识库并剔除不存在的引用。 */
#[tauri::command]
pub async fn update_session_scope(
    app: AppHandle,
    payload: UpdateSessionScopePayload,
) -> Result<WorkspaceSnapshot, String> {
    run_blocking("更新 Agent 会话范围", move || {
        storage::update_session_scope(
            &app,
            payload.snapshot,
            &payload.session_id,
            payload.knowledge_base_ids,
            &payload.active_knowledge_base_id,
        )
    })
    .await
}

/** 从历史会话恢复知识库、笔记和会话焦点。 */
#[tauri::command]
pub async fn restore_session_context(
    app: AppHandle,
    payload: RestoreSessionContextPayload,
) -> Result<WorkspaceSnapshot, String> {
    run_blocking("恢复 Agent 会话上下文", move || {
        storage::restore_session_context(&app, payload.snapshot, &payload.session_id)
    })
    .await
}

/** 截断会话到指定用户消息并删除 transcript；进行中的回合必须先由前端停止。 */
#[tauri::command]
pub async fn rewind_agent_session(
    app: AppHandle,
    payload: RewindAgentSessionPayload,
) -> Result<WorkspaceSnapshot, String> {
    if runtime::is_session_turn_active(&payload.session_id) {
        return Err("当前会话仍有进行中的回合，请先停止后再编辑。".to_owned());
    }

    run_blocking("回退 Agent 会话", move || {
        storage::rewind_agent_session(
            &app,
            payload.snapshot,
            &payload.session_id,
            &payload.message_id,
            &payload.prompt,
        )
    })
    .await
}
