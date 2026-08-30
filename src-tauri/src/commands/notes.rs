use super::common::*;

/** 用户主动新建空白 Markdown，直接落盘并打开为当前可编辑笔记。 */
#[tauri::command]
pub async fn create_note(
    app: AppHandle,
    payload: CreateNotePayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let knowledge_base_index = snapshot
        .knowledge_bases
        .iter()
        .position(|knowledge_base| knowledge_base.id == payload.knowledge_base_id)
        .ok_or_else(|| "找不到要新建笔记的知识库".to_owned())?;
    let knowledge_base = snapshot.knowledge_bases[knowledge_base_index].clone();

    if knowledge_base.status == "error" {
        return Err("当前知识库目录不可访问，无法新建笔记。".to_owned());
    }

    let root_path = PathBuf::from(&knowledge_base.path);
    let parent_path = payload.parent_path.unwrap_or_default();
    let file_name = payload.file_name;
    let relative_path = run_blocking("创建空白 Markdown 文件", move || {
        storage::create_blank_markdown_file(&root_path, &parent_path, file_name.as_deref())
    })
    .await?;
    let created_relative_path = relative_path.clone();
    let note_id = storage::create_stable_note_id(&knowledge_base.id, &relative_path);
    let created_note_path = PathBuf::from(&knowledge_base.path).join(&relative_path);
    let updated_at = read_file_updated_at_or_now(
        &app,
        "create_note",
        &knowledge_base.id,
        "note",
        &note_id,
        &relative_path,
        &created_note_path,
    );
    let new_note = crate::domain::Note {
        id: note_id.clone(),
        knowledge_base_id: knowledge_base.id.clone(),
        title: note_title_from_path(&relative_path),
        path: relative_path,
        content: String::new(),
        tags: Vec::new(),
        updated_at,
        backlinks: Vec::new(),
        content_hash: storage::hash_content(""),
    };

    snapshot.notes.insert(0, new_note);
    snapshot.knowledge_bases[knowledge_base_index].note_count += 1;
    snapshot.knowledge_bases[knowledge_base_index].updated_at = "刚刚".to_owned();
    if let Some(scan_report) = &mut snapshot.knowledge_bases[knowledge_base_index].scan_report {
        scan_report.scanned_file_count += 1;
        *scan_report
            .scanned_by_type
            .entry("markdown".to_owned())
            .or_insert(0) += 1;
    }
    snapshot.active_knowledge_base_id = knowledge_base.id.clone();
    snapshot.active_note_id = note_id;
    snapshot.active_document_id.clear();
    normalize_active_entities(&mut snapshot, Some(&knowledge_base.id));
    index_snapshot_in_background(app.clone(), &snapshot).await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "create_note",
            "completed",
            "已创建 Markdown 笔记。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(knowledge_base.id)
        .entity("note", snapshot.active_note_id.clone())
        .relative_path(created_relative_path),
    );

    Ok(snapshot)
}

/** 在知识库根目录创建带模板的 AGENTS.md；已存在则拒绝覆盖。 */
#[tauri::command]
pub async fn create_project_instruction(
    app: AppHandle,
    payload: CreateProjectInstructionPayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let knowledge_base_index = snapshot
        .knowledge_bases
        .iter()
        .position(|knowledge_base| knowledge_base.id == payload.knowledge_base_id)
        .ok_or_else(|| "找不到要创建说明书的知识库".to_owned())?;
    let knowledge_base = snapshot.knowledge_bases[knowledge_base_index].clone();

    if knowledge_base.status == "error" {
        return Err("当前知识库目录不可访问，无法创建项目说明书。".to_owned());
    }

    let root_path = PathBuf::from(&knowledge_base.path);
    let relative_path = run_blocking("创建项目说明书", move || {
        storage::create_project_instruction_file(&root_path)
    })
    .await?;
    let created_relative_path = relative_path.clone();
    let content = storage::project_instruction_template().to_owned();
    let note_id = storage::create_stable_note_id(&knowledge_base.id, &relative_path);
    let created_note_path = PathBuf::from(&knowledge_base.path).join(&relative_path);
    let updated_at = read_file_updated_at_or_now(
        &app,
        "create_project_instruction",
        &knowledge_base.id,
        "note",
        &note_id,
        &relative_path,
        &created_note_path,
    );
    let title = storage::extract_markdown_title(&created_note_path, &content);
    let new_note = crate::domain::Note {
        id: note_id.clone(),
        knowledge_base_id: knowledge_base.id.clone(),
        title,
        path: relative_path,
        content,
        tags: Vec::new(),
        updated_at,
        backlinks: Vec::new(),
        content_hash: storage::hash_content(storage::project_instruction_template()),
    };

    snapshot.notes.insert(0, new_note);
    snapshot.knowledge_bases[knowledge_base_index].note_count += 1;
    snapshot.knowledge_bases[knowledge_base_index].updated_at = "刚刚".to_owned();
    if let Some(scan_report) = &mut snapshot.knowledge_bases[knowledge_base_index].scan_report {
        scan_report.scanned_file_count += 1;
        *scan_report
            .scanned_by_type
            .entry("markdown".to_owned())
            .or_insert(0) += 1;
    }
    snapshot.active_knowledge_base_id = knowledge_base.id.clone();
    snapshot.active_note_id = note_id;
    snapshot.active_document_id.clear();
    normalize_active_entities(&mut snapshot, Some(&knowledge_base.id));
    index_snapshot_in_background(app.clone(), &snapshot).await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "create_project_instruction",
            "completed",
            "已创建知识库 Agent 说明书。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(knowledge_base.id)
        .entity("note", snapshot.active_note_id.clone())
        .relative_path(created_relative_path),
    );

    Ok(snapshot)
}

#[tauri::command]
pub async fn rename_note(
    app: AppHandle,
    payload: RenameNotePayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let note_index = snapshot
        .notes
        .iter()
        .position(|note| note.id == payload.note_id)
        .ok_or_else(|| "找不到要重命名的笔记".to_owned())?;
    let previous_note = snapshot.notes[note_index].clone();
    let knowledge_base = snapshot
        .knowledge_bases
        .iter()
        .find(|item| item.id == previous_note.knowledge_base_id)
        .cloned()
        .ok_or_else(|| "找不到笔记所属知识库".to_owned())?;
    let root_path = PathBuf::from(&knowledge_base.path);
    let current_relative_path = previous_note.path.clone();
    let next_file_name = payload.next_file_name;
    let (next_relative_path, current_content, current_hash) =
        run_blocking("重命名 Markdown 文件", move || {
            storage::rename_markdown_file(&root_path, &current_relative_path, &next_file_name)
        })
        .await?;
    let next_note_id = storage::create_stable_note_id(&knowledge_base.id, &next_relative_path);
    let next_title =
        storage::extract_markdown_title(Path::new(&next_relative_path), &current_content);
    let history_migrate_app = app.clone();
    let history_previous_note_id = payload.note_id.clone();
    let history_next_note_id = next_note_id.clone();
    let history_knowledge_base_id = knowledge_base.id.clone();
    let history_next_relative_path = next_relative_path.clone();
    let history_next_title = next_title.clone();

    if let Err(_) = run_blocking("迁移 Markdown 历史记录", move || {
        storage::migrate_document_history_target(
            &history_migrate_app,
            "note",
            &history_previous_note_id,
            &history_next_note_id,
            &history_knowledge_base_id,
            &history_next_relative_path,
            &history_next_title,
        )
    })
    .await
    {
        logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Warn,
                AppLogCategory::Editor,
                "document_history_migration",
                "failed",
                "Markdown 重命名后历史记录迁移失败。",
            )
            .duration(started_at.elapsed())
            .knowledge_base_id(knowledge_base.id.clone())
            .entity("note", next_note_id.clone())
            .relative_path(next_relative_path.clone())
            .metadata(json!({
                "targetKind": "note",
                "failureKind": "migration_failed",
            })),
        );
    }
    let next_note_path = PathBuf::from(&knowledge_base.path).join(&next_relative_path);
    let updated_at = read_file_updated_at_or_now(
        &app,
        "rename_note",
        &knowledge_base.id,
        "note",
        &next_note_id,
        &next_relative_path,
        &next_note_path,
    );

    snapshot.notes[note_index].id = next_note_id.clone();
    snapshot.notes[note_index].title = next_title;
    snapshot.notes[note_index].path = next_relative_path.clone();
    snapshot.notes[note_index].tags = storage::extract_tags(&current_content);
    snapshot.notes[note_index].content = current_content;
    snapshot.notes[note_index].content_hash = current_hash;
    snapshot.notes[note_index].updated_at = updated_at;
    snapshot.active_document_id.clear();

    replace_note_reference_after_rename(
        &mut snapshot,
        &payload.note_id,
        &next_note_id,
        &next_relative_path,
    );
    index_snapshot_in_background(app.clone(), &snapshot).await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "rename_note",
            "completed",
            "已重命名 Markdown 笔记。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(knowledge_base.id)
        .entity("note", next_note_id)
        .relative_path(next_relative_path),
    );

    Ok(snapshot)
}

/** 重命名 txt 文档，只修改文件名，并同步更新快照。 */

#[tauri::command]
pub async fn delete_note(
    app: AppHandle,
    payload: DeleteNotePayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let note_index = snapshot
        .notes
        .iter()
        .position(|note| note.id == payload.note_id)
        .ok_or_else(|| "找不到要删除的笔记".to_owned())?;
    let note = snapshot.notes[note_index].clone();
    let knowledge_base_index = snapshot
        .knowledge_bases
        .iter()
        .position(|item| item.id == note.knowledge_base_id)
        .ok_or_else(|| "找不到笔记所属知识库".to_owned())?;
    let knowledge_base = snapshot.knowledge_bases[knowledge_base_index].clone();
    let root_path = PathBuf::from(&knowledge_base.path);
    let relative_path = note.path.clone();
    let expected_hash = payload.expected_hash;

    run_blocking("删除 Markdown 文件", move || {
        storage::trash_markdown_file(&root_path, &relative_path, &expected_hash)
    })
    .await?;

    clear_document_history_after_delete_best_effort(
        &app,
        "note",
        note.id.clone(),
        knowledge_base.id.clone(),
        note.path.clone(),
        started_at,
    )
    .await;

    snapshot.notes.remove(note_index);
    snapshot.knowledge_bases[knowledge_base_index].note_count = snapshot.knowledge_bases
        [knowledge_base_index]
        .note_count
        .saturating_sub(1);
    snapshot.knowledge_bases[knowledge_base_index].updated_at = "刚刚".to_owned();

    if let Some(scan_report) = &mut snapshot.knowledge_bases[knowledge_base_index].scan_report {
        scan_report.scanned_file_count = scan_report.scanned_file_count.saturating_sub(1);
    }

    remove_note_references_after_delete(&mut snapshot, &payload.note_id);
    normalize_active_entities(&mut snapshot, Some(&knowledge_base.id));
    index_snapshot_in_background(app.clone(), &snapshot).await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "delete_note",
            "completed",
            "已将 Markdown 笔记移入回收站。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(knowledge_base.id)
        .entity("note", note.id)
        .relative_path(note.path),
    );

    Ok(snapshot)
}

/** 删除 txt 文档到系统回收站，并从快照中移除普通文档引用。 */

#[tauri::command]
pub async fn save_note_content(
    app: AppHandle,
    payload: SaveNoteContentPayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let note_index = snapshot
        .notes
        .iter()
        .position(|note| note.id == payload.note_id)
        .ok_or_else(|| "找不到要保存的笔记".to_owned())?;
    let knowledge_base = snapshot
        .knowledge_bases
        .iter()
        .find(|item| item.id == snapshot.notes[note_index].knowledge_base_id)
        .ok_or_else(|| "找不到笔记所属知识库".to_owned())?;
    let knowledge_base_id = knowledge_base.id.clone();
    let note_relative_path = snapshot.notes[note_index].path.clone();
    let target_path = storage::resolve_existing_file_inside_root(
        PathBuf::from(&knowledge_base.path).as_path(),
        &note_relative_path,
    )?;

    let read_path = target_path.clone();
    let current_content = run_blocking("读取待保存 Markdown 文件", move || {
        fs::read_to_string(&read_path)
            .map_err(|error| format!("无法读取待保存 Markdown 文件：{error}"))
    })
    .await?;
    let current_hash = storage::hash_content(&current_content);

    // expectedHash 来自用户开始编辑时的文件版本；不一致说明外部编辑器已改动，必须先重扫。
    if current_hash != payload.expected_hash {
        logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Warn,
                AppLogCategory::Editor,
                "save_note_content",
                "blocked",
                "目标 Markdown 文件已被外部修改，已阻止保存。",
            )
            .duration(started_at.elapsed())
            .knowledge_base_id(knowledge_base_id.clone())
            .entity("note", payload.note_id.clone())
            .relative_path(note_relative_path.clone()),
        );

        return Err("目标文件已被外部修改，已阻止保存。请重新扫描后再编辑。".to_owned());
    }

    capture_document_history_before_write(
        &app,
        storage::DocumentHistoryCapture {
            target_kind: "note".to_owned(),
            knowledge_base_id: knowledge_base_id.clone(),
            target_id: payload.note_id.clone(),
            relative_path: note_relative_path.clone(),
            title: snapshot.notes[note_index].title.clone(),
            file_type: "markdown".to_owned(),
            content: current_content,
            source: "manual-save".to_owned(),
            session_id: None,
            change_id: None,
            operation_id: None,
        },
        AppLogCategory::Editor,
        "save_note_content",
        started_at,
    )
    .await?;

    let write_path = target_path.clone();
    let write_content = payload.content.clone();

    run_blocking("保存 Markdown 文件", move || {
        storage::atomic_write_markdown(&write_path, &write_content)
    })
    .await?;

    let updated_at = read_file_updated_at_or_now(
        &app,
        "save_note_content",
        &knowledge_base_id,
        "note",
        &payload.note_id,
        &note_relative_path,
        &target_path,
    );
    let next_hash = storage::hash_content(&payload.content);
    snapshot.notes[note_index].tags = storage::extract_tags(&payload.content);
    snapshot.notes[note_index].content = payload.content;
    snapshot.notes[note_index].content_hash = next_hash;
    snapshot.notes[note_index].updated_at = updated_at;
    snapshot.active_note_id = payload.note_id;
    snapshot.active_document_id.clear();
    normalize_active_entities(&mut snapshot, None);
    index_snapshot_in_background(app.clone(), &snapshot).await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "save_note_content",
            "completed",
            "已保存 Markdown 笔记。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(knowledge_base_id)
        .entity("note", snapshot.active_note_id.clone())
        .relative_path(note_relative_path),
    );

    Ok(snapshot)
}

/** 保存当前笔记粘贴的图片附件，只负责落盘和返回 Markdown 片段，不写回正文。 */
#[tauri::command]
pub async fn save_note_image_attachments(
    app: AppHandle,
    payload: SaveNoteImageAttachmentsPayload,
) -> Result<Vec<crate::domain::SavedNoteImageAttachment>, String> {
    let started_at = Instant::now();
    let note_count = payload.images.len();
    let note_id = payload.note_id.clone();
    let snapshot = payload.snapshot;
    let note = snapshot
        .notes
        .iter()
        .find(|item| item.id == note_id)
        .ok_or_else(|| "找不到要保存图片的 Markdown 笔记。".to_owned())?;
    let knowledge_base = snapshot
        .knowledge_bases
        .iter()
        .find(|item| item.id == note.knowledge_base_id)
        .ok_or_else(|| "找不到图片附件所属知识库。".to_owned())?;
    let knowledge_base_id = knowledge_base.id.clone();
    let note_entity_id = note.id.clone();
    let root_path = PathBuf::from(&knowledge_base.path);
    let note_relative_path = note.path.clone();
    let write_note_relative_path = note_relative_path.clone();
    let images = payload.images;

    let save_result = run_blocking("保存粘贴图片附件", move || {
        storage::save_note_image_attachments(&root_path, &write_note_relative_path, &images)
    })
    .await;

    match save_result {
        Ok(saved_attachments) => {
            let total_byte_size: usize = saved_attachments
                .iter()
                .map(|attachment| attachment.byte_size)
                .sum();

            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    AppLogLevel::Info,
                    AppLogCategory::Editor,
                    "paste_image_attachment",
                    "completed",
                    "已保存粘贴图片附件。",
                )
                .duration(started_at.elapsed())
                .knowledge_base_id(knowledge_base_id.clone())
                .entity("note", note_entity_id.clone())
                .relative_path(note_relative_path.clone())
                .metadata(json!({
                    "imageCount": note_count,
                    "savedCount": saved_attachments.len(),
                    "totalBytes": total_byte_size,
                })),
            );

            Ok(saved_attachments)
        }
        Err(error) => {
            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    AppLogLevel::Warn,
                    AppLogCategory::Editor,
                    "paste_image_attachment",
                    "failed",
                    "粘贴图片附件保存失败。",
                )
                .duration(started_at.elapsed())
                .knowledge_base_id(knowledge_base_id)
                .entity("note", note_entity_id)
                .relative_path(note_relative_path)
                .metadata(json!({
                    "imageCount": note_count,
                })),
            );

            Err(error)
        }
    }
}

/** 保存当前 txt 文档正文，校验知识库边界和文件 hash 后原子写回本地文件。 */

/** 重命名后把活跃笔记、固定笔记和待确认 diff 中的旧 note id 迁移到新 id。 */
pub(super) fn replace_note_reference_after_rename(
    snapshot: &mut WorkspaceSnapshot,
    previous_note_id: &str,
    next_note_id: &str,
    next_relative_path: &str,
) {
    if snapshot.active_note_id == previous_note_id {
        snapshot.active_note_id = next_note_id.to_owned();
    }

    for session in &mut snapshot.sessions {
        if session.active_note_id.as_deref() == Some(previous_note_id) {
            session.active_note_id = Some(next_note_id.to_owned());
        }

        for pinned_note_id in &mut session.pinned_note_ids {
            if pinned_note_id == previous_note_id {
                *pinned_note_id = next_note_id.to_owned();
            }
        }
        session.pinned_note_ids.sort();
        session.pinned_note_ids.dedup();

        if let Some(change) = &mut session.pending_change {
            if change.note_id.as_deref() == Some(previous_note_id) {
                change.note_id = Some(next_note_id.to_owned());
                change.target_id = Some(next_note_id.to_owned());
                change.target_path = next_relative_path.to_owned();
            }
        }
    }
}

/** 删除后清理会话中的笔记引用和待确认 diff，避免 UI 指向已移入回收站的文件。 */
pub(super) fn remove_note_references_after_delete(snapshot: &mut WorkspaceSnapshot, note_id: &str) {
    if snapshot.active_note_id == note_id {
        snapshot.active_note_id.clear();
    }

    for session in &mut snapshot.sessions {
        if session.active_note_id.as_deref() == Some(note_id) {
            session.active_note_id = None;
        }

        session.pinned_note_ids.retain(|id| id != note_id);

        if session.pending_change.as_ref().is_some_and(|change| {
            change.note_id.as_deref() == Some(note_id)
                || (change.target_kind.as_deref() == Some("note")
                    && change.target_id.as_deref() == Some(note_id))
        }) {
            session.pending_change = None;
        }
    }
}
