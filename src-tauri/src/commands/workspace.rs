use super::common::*;

/** 加载工作台启动状态；从 SQLite 恢复会话并重新扫描真实支持文档。 */
#[tauri::command]
pub async fn load_workspace_state(app: AppHandle) -> Result<WorkspaceBootstrapState, String> {
    let load_app = app.clone();
    let index_app = app.clone();
    let started_at = Instant::now();

    let mut bootstrap = run_blocking("加载工作台状态", move || {
        storage::load_workspace_bootstrap_state(&load_app)
    })
    .await?;
    migrate_legacy_pending_changes(&mut bootstrap.snapshot);
    let index_snapshot = bootstrap.snapshot.clone();

    allow_asset_protocol_for_knowledge_bases(&app, &bootstrap.snapshot)?;

    // 启动索引只影响后续检索，不阻塞首屏进入；失败时写 stderr 供桌面日志排查。
    tauri::async_runtime::spawn(async move {
        if let Err(error) = index_snapshot_in_background(index_app, &index_snapshot).await {
            log::warn!(target: "storage", "启动刷新本地检索索引失败：{error}");
        }
    });

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::App,
            "load_workspace_state",
            "completed",
            "已加载工作台状态。",
        )
        .duration(started_at.elapsed())
        .metadata(json!({
            "knowledgeBaseCount": bootstrap.snapshot.knowledge_bases.len(),
            "noteCount": bootstrap.snapshot.notes.len(),
            "documentCount": bootstrap.snapshot.documents.len(),
            // 只记录恢复结果摘要，避免文件路径、标题或正文写入日志。
            "restoredTabCount": bootstrap.editor_state.open_tabs.len(),
            "hasActiveTab": bootstrap.editor_state.active_tab.is_some(),
            "hasActiveKnowledgeBase": !bootstrap.editor_state.active_knowledge_base_id.is_empty(),
        })),
    );

    Ok(bootstrap)
}

/** 保存前端编辑器会话；写入失败由调用方忽略，不应阻塞用户的编辑或标签操作。 */
#[tauri::command]
pub async fn save_workspace_editor_state(
    app: AppHandle,
    editor_state: WorkspaceEditorState,
) -> Result<WorkspaceEditorState, String> {
    let started_at = Instant::now();
    let tab_count = editor_state.open_tabs.len();
    let has_active_tab = editor_state.active_tab.is_some();
    let has_active_knowledge_base = !editor_state.active_knowledge_base_id.is_empty();
    let save_app = app.clone();

    let result = run_blocking("保存编辑器会话", move || {
        storage::save_workspace_editor_state(&save_app, editor_state)
    })
    .await;

    match &result {
        Ok(saved_state) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Editor,
                "save_workspace_editor_state",
                "completed",
                "已保存编辑器会话。",
            )
            .duration(started_at.elapsed())
            // 状态日志仅包含计数和布尔值，防止本地文件信息泄露到应用日志。
            .metadata(json!({
                "requestedTabCount": tab_count,
                "savedTabCount": saved_state.open_tabs.len(),
                "hasActiveTab": has_active_tab,
                "hasActiveKnowledgeBase": has_active_knowledge_base,
            })),
        ),
        Err(_) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Warn,
                AppLogCategory::Editor,
                "save_workspace_editor_state",
                "failed",
                "保存编辑器会话失败。",
            )
            .duration(started_at.elapsed())
            .metadata(json!({
                "failureKind": "storage_write_failed",
                "requestedTabCount": tab_count,
                "hasActiveTab": has_active_tab,
                "hasActiveKnowledgeBase": has_active_knowledge_base,
            })),
        ),
    }

    result
}

/** 将旧会话的 Markdown 专用 pending change 补齐统一目标字段，保持既有审阅可继续确认。 */
pub(super) fn migrate_legacy_pending_changes(snapshot: &mut WorkspaceSnapshot) {
    for session in &mut snapshot.sessions {
        let Some(change) = session.pending_change.as_mut() else {
            continue;
        };
        if change.target_id.is_none() {
            change.target_id = change.note_id.clone();
        }
        if change.target_kind.is_none() {
            change.target_kind = Some(if change.file_type.as_deref() == Some("txt") {
                "document".to_owned()
            } else {
                "note".to_owned()
            });
        }
        if change.file_type.is_none() {
            change.file_type = Some(if change.target_kind.as_deref() == Some("document") {
                "txt".to_owned()
            } else {
                "markdown".to_owned()
            });
        }
    }
}
