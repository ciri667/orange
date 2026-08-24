use super::common::*;

/** 文档历史目标上下文，只包含可写文件所需的脱敏定位信息。 */
#[derive(Clone, Debug)]
pub(super) struct DocumentHistoryTargetContext {
    target_kind: String,
    entity_type: &'static str,
    entity_id: String,
    knowledge_base_id: String,
    relative_path: String,
    title: String,
    file_type: String,
}

/** 从快照解析历史记录目标；首版只允许 Markdown note 和 TXT document。 */
pub(super) fn resolve_document_history_target(
    snapshot: &WorkspaceSnapshot,
    target_kind: &str,
    target_id: &str,
) -> Result<DocumentHistoryTargetContext, String> {
    match target_kind {
        "note" => {
            let note = snapshot
                .notes
                .iter()
                .find(|item| item.id == target_id)
                .ok_or_else(|| "找不到要查看历史的 Markdown 笔记。".to_owned())?;

            Ok(DocumentHistoryTargetContext {
                target_kind: "note".to_owned(),
                entity_type: "note",
                entity_id: note.id.clone(),
                knowledge_base_id: note.knowledge_base_id.clone(),
                relative_path: note.path.clone(),
                title: note.title.clone(),
                file_type: "markdown".to_owned(),
            })
        }
        "document" => {
            let document = snapshot
                .documents
                .iter()
                .find(|item| item.id == target_id)
                .ok_or_else(|| "找不到要查看历史的文档。".to_owned())?;

            if document.file_type != "txt" {
                return Err("只有 TXT 文档支持历史记录。".to_owned());
            }

            Ok(DocumentHistoryTargetContext {
                target_kind: "document".to_owned(),
                entity_type: "document",
                entity_id: document.id.clone(),
                knowledge_base_id: document.knowledge_base_id.clone(),
                relative_path: document.path.clone(),
                title: document.title.clone(),
                file_type: "txt".to_owned(),
            })
        }
        _ => Err("该文件类型暂不支持历史记录。".to_owned()),
    }
}

#[tauri::command]
pub async fn load_document_history(
    app: AppHandle,
    payload: LoadDocumentHistoryPayload,
) -> Result<Vec<DocumentHistoryEntry>, String> {
    let started_at = Instant::now();
    let target = resolve_document_history_target(
        &payload.snapshot,
        &payload.target_kind,
        &payload.target_id,
    )?;
    let target_kind = target.target_kind.clone();
    let target_id = target.entity_id.clone();
    let history_app = app.clone();
    let entries = run_blocking("读取文档历史记录", move || {
        storage::load_document_history(&history_app, &target_kind, &target_id)
    })
    .await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "load_document_history",
            "completed",
            "已读取文档历史记录。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(target.knowledge_base_id)
        .entity(target.entity_type, target.entity_id)
        .relative_path(target.relative_path)
        .metadata(json!({
            "targetKind": target.target_kind,
            "fileType": target.file_type,
            "entryCount": entries.len(),
        })),
    );

    Ok(entries)
}

/** 读取单条历史记录正文快照，供前端 diff 和恢复确认使用。 */
#[tauri::command]
pub async fn load_document_history_entry(
    app: AppHandle,
    payload: LoadDocumentHistoryEntryPayload,
) -> Result<DocumentHistoryEntryDetail, String> {
    run_blocking("读取文档历史详情", move || {
        storage::load_document_history_entry(&app, &payload.entry_id)
    })
    .await
}

/** 清空当前文件历史记录；只删除历史快照，不删除用户文档。 */
#[tauri::command]
pub async fn clear_document_history(
    app: AppHandle,
    payload: ClearDocumentHistoryPayload,
) -> Result<(), String> {
    let started_at = Instant::now();
    let target = resolve_document_history_target(
        &payload.snapshot,
        &payload.target_kind,
        &payload.target_id,
    )?;
    let target_kind = target.target_kind.clone();
    let target_id = target.entity_id.clone();
    let clear_app = app.clone();
    let clear_result = run_blocking("清空文档历史记录", move || {
        storage::clear_document_history(&clear_app, &target_kind, &target_id)
    })
    .await;

    match clear_result {
        Ok(summary) => {
            let has_cleanup_failures = summary.cleanup_failure_count > 0;

            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    if has_cleanup_failures {
                        AppLogLevel::Warn
                    } else {
                        AppLogLevel::Info
                    },
                    AppLogCategory::Editor,
                    "clear_document_history",
                    if has_cleanup_failures {
                        "partial"
                    } else {
                        "completed"
                    },
                    if has_cleanup_failures {
                        "文档历史记录已清空，但部分快照清理失败。"
                    } else {
                        "已清空文档历史记录。"
                    },
                )
                .duration(started_at.elapsed())
                .knowledge_base_id(target.knowledge_base_id)
                .entity(target.entity_type, target.entity_id)
                .relative_path(target.relative_path)
                .metadata(json!({
                    "targetKind": target.target_kind,
                    "fileType": target.file_type,
                    "removedCount": summary.removed_count,
                    "cleanupFailureCount": summary.cleanup_failure_count,
                })),
            );

            Ok(())
        }
        Err(error) => {
            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    AppLogLevel::Error,
                    AppLogCategory::Editor,
                    "clear_document_history",
                    "failed",
                    "清空文档历史记录失败。",
                )
                .duration(started_at.elapsed())
                .knowledge_base_id(target.knowledge_base_id)
                .entity(target.entity_type, target.entity_id)
                .relative_path(target.relative_path)
                .metadata(json!({
                    "targetKind": target.target_kind,
                    "fileType": target.file_type,
                })),
            );

            Err(error)
        }
    }
}

/** 恢复指定历史版本；恢复前先捕获当前版本，所以回档操作本身可撤销。 */
#[tauri::command]
pub async fn restore_document_history_entry(
    app: AppHandle,
    payload: RestoreDocumentHistoryEntryPayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let expected_hash = payload.expected_hash;
    let entry_id = payload.entry_id;
    let mut snapshot = payload.snapshot;
    let load_app = app.clone();
    let entry_id_for_load = entry_id.clone();
    let detail = match run_blocking("读取待恢复历史版本", move || {
        storage::load_document_history_entry(&load_app, &entry_id_for_load)
    })
    .await
    {
        Ok(detail) => detail,
        Err(error) => {
            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    AppLogLevel::Error,
                    AppLogCategory::Editor,
                    "restore_document_history_entry",
                    "failed",
                    "读取待恢复历史版本失败。",
                )
                .duration(started_at.elapsed())
                .entity("history", entry_id),
            );

            return Err(error);
        }
    };
    let target = resolve_document_history_target(
        &snapshot,
        &detail.entry.target_kind,
        &detail.entry.target_id,
    )?;

    if detail.entry.file_type != target.file_type {
        return Err("历史版本文件类型与当前文件不一致，已阻止恢复。".to_owned());
    }

    if target.target_kind == "note" {
        let note_index = snapshot
            .notes
            .iter()
            .position(|note| note.id == target.entity_id)
            .ok_or_else(|| "找不到要恢复的 Markdown 笔记。".to_owned())?;
        let knowledge_base = snapshot
            .knowledge_bases
            .iter()
            .find(|item| item.id == target.knowledge_base_id)
            .cloned()
            .ok_or_else(|| "找不到笔记所属知识库。".to_owned())?;
        let target_path = storage::resolve_existing_file_inside_root(
            PathBuf::from(&knowledge_base.path).as_path(),
            &target.relative_path,
        )?;
        let read_path = target_path.clone();
        let current_content = run_blocking("读取待恢复 Markdown 文件", move || {
            fs::read_to_string(&read_path)
                .map_err(|error| format!("无法读取待恢复 Markdown 文件：{error}"))
        })
        .await?;
        let current_hash = storage::hash_content(&current_content);

        if current_hash != expected_hash {
            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    AppLogLevel::Warn,
                    AppLogCategory::Editor,
                    "restore_document_history_entry",
                    "blocked",
                    "目标 Markdown 文件已被外部修改，已阻止恢复。",
                )
                .duration(started_at.elapsed())
                .knowledge_base_id(target.knowledge_base_id.clone())
                .entity(target.entity_type, target.entity_id.clone())
                .relative_path(target.relative_path.clone())
                .metadata(json!({ "entryId": detail.entry.id.clone() })),
            );

            return Err("目标文件已被外部修改，已阻止恢复。请重新扫描后再操作。".to_owned());
        }

        capture_document_history_before_write(
            &app,
            storage::DocumentHistoryCapture {
                target_kind: target.target_kind.clone(),
                knowledge_base_id: target.knowledge_base_id.clone(),
                target_id: target.entity_id.clone(),
                relative_path: target.relative_path.clone(),
                title: target.title.clone(),
                file_type: target.file_type.clone(),
                content: current_content,
                source: "restore".to_owned(),
                session_id: None,
                change_id: None,
                operation_id: None,
            },
            AppLogCategory::Editor,
            "restore_document_history_entry",
            started_at,
        )
        .await?;

        let write_path = target_path.clone();
        let restored_content = detail.content.clone();

        run_blocking("恢复 Markdown 历史版本", move || {
            storage::atomic_write_markdown(&write_path, &restored_content)
        })
        .await?;

        let updated_at = read_file_updated_at_or_now(
            &app,
            "restore_document_history_entry",
            &target.knowledge_base_id,
            target.entity_type,
            &target.entity_id,
            &target.relative_path,
            &target_path,
        );
        let next_hash = storage::hash_content(&detail.content);

        snapshot.notes[note_index].content = detail.content.clone();
        snapshot.notes[note_index].content_hash = next_hash;
        snapshot.notes[note_index].updated_at = updated_at;
        snapshot.active_note_id = target.entity_id.clone();
        snapshot.active_document_id.clear();
    } else {
        let document_index = snapshot
            .documents
            .iter()
            .position(|document| document.id == target.entity_id)
            .ok_or_else(|| "找不到要恢复的 TXT 文档。".to_owned())?;
        let knowledge_base = snapshot
            .knowledge_bases
            .iter()
            .find(|item| item.id == target.knowledge_base_id)
            .cloned()
            .ok_or_else(|| "找不到文档所属知识库。".to_owned())?;
        let target_path = storage::resolve_existing_file_inside_root(
            PathBuf::from(&knowledge_base.path).as_path(),
            &target.relative_path,
        )?;
        let read_path = target_path.clone();
        let current_content = run_blocking("读取待恢复 TXT 文件", move || {
            fs::read_to_string(&read_path)
                .map_err(|error| format!("无法读取待恢复 TXT 文件：{error}"))
        })
        .await?;
        let current_hash = storage::hash_content(&current_content);

        if current_hash != expected_hash {
            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    AppLogLevel::Warn,
                    AppLogCategory::Editor,
                    "restore_document_history_entry",
                    "blocked",
                    "目标 TXT 文件已被外部修改，已阻止恢复。",
                )
                .duration(started_at.elapsed())
                .knowledge_base_id(target.knowledge_base_id.clone())
                .entity(target.entity_type, target.entity_id.clone())
                .relative_path(target.relative_path.clone())
                .metadata(json!({ "entryId": detail.entry.id.clone() })),
            );

            return Err("目标文件已被外部修改，已阻止恢复。请重新扫描后再操作。".to_owned());
        }

        capture_document_history_before_write(
            &app,
            storage::DocumentHistoryCapture {
                target_kind: target.target_kind.clone(),
                knowledge_base_id: target.knowledge_base_id.clone(),
                target_id: target.entity_id.clone(),
                relative_path: target.relative_path.clone(),
                title: target.title.clone(),
                file_type: target.file_type.clone(),
                content: current_content,
                source: "restore".to_owned(),
                session_id: None,
                change_id: None,
                operation_id: None,
            },
            AppLogCategory::Editor,
            "restore_document_history_entry",
            started_at,
        )
        .await?;

        let write_path = target_path.clone();
        let restored_content = detail.content.clone();

        run_blocking("恢复 TXT 历史版本", move || {
            storage::atomic_write_text_document(&write_path, &restored_content)
        })
        .await?;

        let updated_at = read_file_updated_at_or_now(
            &app,
            "restore_document_history_entry",
            &target.knowledge_base_id,
            target.entity_type,
            &target.entity_id,
            &target.relative_path,
            &target_path,
        );
        let next_hash = storage::hash_content(&detail.content);

        snapshot.documents[document_index].content = Some(detail.content.clone());
        snapshot.documents[document_index].content_hash = next_hash;
        snapshot.documents[document_index].updated_at = updated_at;
        snapshot.active_note_id.clear();
        snapshot.active_document_id = target.entity_id.clone();
    }

    normalize_active_entities(&mut snapshot, None);
    index_snapshot_in_background(app.clone(), &snapshot).await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "restore_document_history_entry",
            "completed",
            "已恢复文档历史版本。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(target.knowledge_base_id)
        .entity(target.entity_type, target.entity_id)
        .relative_path(target.relative_path)
        .metadata(json!({
            "entryId": detail.entry.id.clone(),
            "targetKind": target.target_kind,
            "fileType": target.file_type,
            "byteSize": detail.entry.byte_size,
            "lineCount": detail.entry.line_count,
        })),
    );

    Ok(snapshot)
}
