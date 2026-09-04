use super::common::*;

#[tauri::command]
pub async fn create_document(
    app: AppHandle,
    payload: CreateDocumentPayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let knowledge_base_index = snapshot
        .knowledge_bases
        .iter()
        .position(|knowledge_base| knowledge_base.id == payload.knowledge_base_id)
        .ok_or_else(|| "找不到要新建文档的知识库".to_owned())?;
    let knowledge_base = snapshot.knowledge_bases[knowledge_base_index].clone();

    if knowledge_base.status == "error" {
        return Err("当前知识库目录不可访问，无法新建 TXT 文档。".to_owned());
    }

    let root_path = PathBuf::from(&knowledge_base.path);
    let parent_path = payload.parent_path.unwrap_or_default();
    let file_name = payload.file_name;
    let relative_path = run_blocking("创建空白 TXT 文件", move || {
        storage::create_blank_text_document_file(&root_path, &parent_path, file_name.as_deref())
    })
    .await?;
    let created_relative_path = relative_path.clone();
    let document_id = storage::create_stable_document_id(&knowledge_base.id, &relative_path);
    let created_document_path = PathBuf::from(&knowledge_base.path).join(&relative_path);
    let updated_at = read_file_updated_at_or_now(
        &app,
        "create_document",
        &knowledge_base.id,
        "document",
        &document_id,
        &relative_path,
        &created_document_path,
    );
    let new_document = crate::domain::WorkspaceDocument {
        id: document_id.clone(),
        knowledge_base_id: knowledge_base.id.clone(),
        title: document_title_from_path(&relative_path),
        path: relative_path,
        file_type: "txt".to_owned(),
        updated_at,
        content_hash: storage::hash_content(""),
        content: Some(String::new()),
        preview_available: false,
    };

    snapshot.documents.insert(0, new_document);
    snapshot.knowledge_bases[knowledge_base_index].document_count += 1;
    snapshot.knowledge_bases[knowledge_base_index].updated_at = "刚刚".to_owned();
    if let Some(scan_report) = &mut snapshot.knowledge_bases[knowledge_base_index].scan_report {
        scan_report.scanned_file_count += 1;
        *scan_report
            .scanned_by_type
            .entry("txt".to_owned())
            .or_insert(0) += 1;
    }
    snapshot.active_knowledge_base_id = knowledge_base.id.clone();
    snapshot.active_note_id.clear();
    snapshot.active_document_id = document_id;
    normalize_active_entities(&mut snapshot, Some(&knowledge_base.id));

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "create_document",
            "completed",
            "已创建 TXT 文档。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(knowledge_base.id)
        .entity("document", snapshot.active_document_id.clone())
        .relative_path(created_relative_path),
    );

    Ok(snapshot)
}

/** 用户在目录树的指定目录下新建文件夹，成功后只更新目录快照不切换当前笔记。 */
#[tauri::command]
pub async fn create_folder(
    app: AppHandle,
    payload: CreateFolderPayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let knowledge_base_index = snapshot
        .knowledge_bases
        .iter()
        .position(|knowledge_base| knowledge_base.id == payload.knowledge_base_id)
        .ok_or_else(|| "找不到要新建文件夹的知识库".to_owned())?;
    let knowledge_base = snapshot.knowledge_bases[knowledge_base_index].clone();

    if knowledge_base.status == "error" {
        return Err("当前知识库目录不可访问，无法新建文件夹。".to_owned());
    }

    let root_path = PathBuf::from(&knowledge_base.path);
    let parent_path = payload.parent_path;
    let folder_name = payload.folder_name;
    let relative_path = run_blocking("创建文件夹", move || {
        storage::create_folder(&root_path, &parent_path, &folder_name)
    })
    .await?;
    let created_relative_path = relative_path.clone();
    let folder_id = storage::create_stable_folder_id(&knowledge_base.id, &relative_path);
    let folder_entry = FolderEntry {
        id: folder_id.clone(),
        knowledge_base_id: knowledge_base.id.clone(),
        name: folder_name_from_path(&relative_path),
        path: relative_path,
        updated_at: "刚刚".to_owned(),
    };

    // 快照可能来自旧版本或浏览器 fallback，追加前去重，避免同一目录显示两次。
    snapshot.folders.retain(|folder| {
        // 只在当前知识库内去重新建目录，不能影响其他知识库中同名相对目录。
        folder.knowledge_base_id != knowledge_base.id
            || (folder.id != folder_entry.id && folder.path != folder_entry.path)
    });
    snapshot.folders.push(folder_entry);
    snapshot.knowledge_bases[knowledge_base_index].updated_at = "刚刚".to_owned();
    snapshot.active_knowledge_base_id = knowledge_base.id.clone();
    normalize_active_entities(&mut snapshot, Some(&knowledge_base.id));

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "create_folder",
            "completed",
            "已创建文件夹。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(knowledge_base.id)
        .entity("folder", folder_id)
        .relative_path(created_relative_path),
    );

    Ok(snapshot)
}

/** 重命名 Markdown 文件，只修改文件名，并同步更新快照与会话引用。 */

#[tauri::command]
pub async fn rename_document(
    app: AppHandle,
    payload: RenameDocumentPayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let document_index = snapshot
        .documents
        .iter()
        .position(|document| document.id == payload.document_id)
        .ok_or_else(|| "找不到要重命名的文档".to_owned())?;
    let previous_document = snapshot.documents[document_index].clone();

    if previous_document.file_type != "txt" {
        return Err("只有 TXT 文档支持重命名。".to_owned());
    }

    let knowledge_base = snapshot
        .knowledge_bases
        .iter()
        .find(|item| item.id == previous_document.knowledge_base_id)
        .cloned()
        .ok_or_else(|| "找不到文档所属知识库".to_owned())?;
    let root_path = PathBuf::from(&knowledge_base.path);
    let current_relative_path = previous_document.path.clone();
    let next_file_name = payload.next_file_name;
    let (next_relative_path, current_content, current_hash) =
        run_blocking("重命名 TXT 文件", move || {
            storage::rename_text_document_file(&root_path, &current_relative_path, &next_file_name)
        })
        .await?;
    let next_document_id =
        storage::create_stable_document_id(&knowledge_base.id, &next_relative_path);
    let next_document_title = document_title_from_path(&next_relative_path);
    let history_migrate_app = app.clone();
    let history_previous_document_id = payload.document_id.clone();
    let history_next_document_id = next_document_id.clone();
    let history_knowledge_base_id = knowledge_base.id.clone();
    let history_next_relative_path = next_relative_path.clone();
    let history_next_title = next_document_title.clone();

    if let Err(_) = run_blocking("迁移 TXT 历史记录", move || {
        storage::migrate_document_history_target(
            &history_migrate_app,
            "document",
            &history_previous_document_id,
            &history_next_document_id,
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
                "TXT 重命名后历史记录迁移失败。",
            )
            .duration(started_at.elapsed())
            .knowledge_base_id(knowledge_base.id.clone())
            .entity("document", next_document_id.clone())
            .relative_path(next_relative_path.clone())
            .metadata(json!({
                "targetKind": "document",
                "failureKind": "migration_failed",
            })),
        );
    }
    let next_document_path = PathBuf::from(&knowledge_base.path).join(&next_relative_path);
    let updated_at = read_file_updated_at_or_now(
        &app,
        "rename_document",
        &knowledge_base.id,
        "document",
        &next_document_id,
        &next_relative_path,
        &next_document_path,
    );

    snapshot.documents[document_index].id = next_document_id.clone();
    snapshot.documents[document_index].title = next_document_title;
    snapshot.documents[document_index].path = next_relative_path;
    snapshot.documents[document_index].content = Some(current_content);
    snapshot.documents[document_index].content_hash = current_hash;
    snapshot.documents[document_index].updated_at = updated_at;

    if snapshot.active_document_id == payload.document_id {
        snapshot.active_document_id = next_document_id;
        snapshot.active_note_id.clear();
    }

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "rename_document",
            "completed",
            "已重命名 TXT 文档。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(knowledge_base.id)
        .entity("document", snapshot.documents[document_index].id.clone())
        .relative_path(snapshot.documents[document_index].path.clone()),
    );

    Ok(snapshot)
}

/** 删除 Markdown 文件到系统回收站，并从快照中移除笔记和相关会话引用。 */

#[tauri::command]
pub async fn delete_document(
    app: AppHandle,
    payload: DeleteDocumentPayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let document_index = snapshot
        .documents
        .iter()
        .position(|document| document.id == payload.document_id)
        .ok_or_else(|| "找不到要删除的文档".to_owned())?;
    let document = snapshot.documents[document_index].clone();

    if document.file_type != "txt" {
        return Err("只有 TXT 文档支持删除。".to_owned());
    }

    let knowledge_base_index = snapshot
        .knowledge_bases
        .iter()
        .position(|item| item.id == document.knowledge_base_id)
        .ok_or_else(|| "找不到文档所属知识库".to_owned())?;
    let knowledge_base = snapshot.knowledge_bases[knowledge_base_index].clone();
    let root_path = PathBuf::from(&knowledge_base.path);
    let relative_path = document.path.clone();
    let expected_hash = payload.expected_hash;

    run_blocking("删除 TXT 文件", move || {
        storage::trash_text_document_file(&root_path, &relative_path, &expected_hash)
    })
    .await?;

    clear_document_history_after_delete_best_effort(
        &app,
        "document",
        document.id.clone(),
        knowledge_base.id.clone(),
        document.path.clone(),
        started_at,
    )
    .await;

    snapshot.documents.remove(document_index);
    snapshot.knowledge_bases[knowledge_base_index].document_count = snapshot.knowledge_bases
        [knowledge_base_index]
        .document_count
        .saturating_sub(1);
    snapshot.knowledge_bases[knowledge_base_index].updated_at = "刚刚".to_owned();

    if let Some(scan_report) = &mut snapshot.knowledge_bases[knowledge_base_index].scan_report {
        scan_report.scanned_file_count = scan_report.scanned_file_count.saturating_sub(1);
        if let Some(txt_count) = scan_report.scanned_by_type.get_mut("txt") {
            *txt_count = txt_count.saturating_sub(1);
        }
    }

    if snapshot.active_document_id == payload.document_id {
        snapshot.active_document_id.clear();
    }

    normalize_active_entities(&mut snapshot, Some(&knowledge_base.id));

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "delete_document",
            "completed",
            "已将 TXT 文档移入回收站。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(knowledge_base.id)
        .entity("document", document.id)
        .relative_path(document.path),
    );

    Ok(snapshot)
}

/** 保存当前笔记正文，校验知识库边界和文件 hash 后原子写回 Markdown。 */

#[tauri::command]
pub async fn save_document_content(
    app: AppHandle,
    payload: SaveDocumentContentPayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let document_index = snapshot
        .documents
        .iter()
        .position(|document| document.id == payload.document_id)
        .ok_or_else(|| "找不到要保存的文档".to_owned())?;

    if snapshot.documents[document_index].file_type != "txt" {
        return Err("只有 TXT 文档支持保存。".to_owned());
    }

    let knowledge_base = snapshot
        .knowledge_bases
        .iter()
        .find(|item| item.id == snapshot.documents[document_index].knowledge_base_id)
        .ok_or_else(|| "找不到文档所属知识库".to_owned())?;
    let knowledge_base_id = knowledge_base.id.clone();
    let document_relative_path = snapshot.documents[document_index].path.clone();
    let target_path = storage::resolve_existing_file_inside_root(
        PathBuf::from(&knowledge_base.path).as_path(),
        &document_relative_path,
    )?;

    let read_path = target_path.clone();
    let current_content = run_blocking("读取待保存 TXT 文件", move || {
        fs::read_to_string(&read_path).map_err(|error| format!("无法读取待保存 TXT 文件：{error}"))
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
                "save_document_content",
                "blocked",
                "目标 TXT 文件已被外部修改，已阻止保存。",
            )
            .duration(started_at.elapsed())
            .knowledge_base_id(knowledge_base_id.clone())
            .entity("document", payload.document_id.clone())
            .relative_path(document_relative_path.clone()),
        );

        return Err("目标文件已被外部修改，已阻止保存。请重新扫描后再编辑。".to_owned());
    }

    capture_document_history_before_write(
        &app,
        storage::DocumentHistoryCapture {
            target_kind: "document".to_owned(),
            knowledge_base_id: knowledge_base_id.clone(),
            target_id: payload.document_id.clone(),
            relative_path: document_relative_path.clone(),
            title: snapshot.documents[document_index].title.clone(),
            file_type: "txt".to_owned(),
            content: current_content,
            source: "manual-save".to_owned(),
            session_id: None,
            change_id: None,
            operation_id: None,
        },
        AppLogCategory::Editor,
        "save_document_content",
        started_at,
    )
    .await?;

    let write_path = target_path.clone();
    let write_content = payload.content.clone();

    run_blocking("保存 TXT 文件", move || {
        storage::atomic_write_text_document(&write_path, &write_content)
    })
    .await?;

    let updated_at = read_file_updated_at_or_now(
        &app,
        "save_document_content",
        &knowledge_base_id,
        "document",
        &payload.document_id,
        &document_relative_path,
        &target_path,
    );
    let next_hash = storage::hash_content(&payload.content);
    snapshot.documents[document_index].content = Some(payload.content);
    snapshot.documents[document_index].content_hash = next_hash;
    snapshot.documents[document_index].updated_at = updated_at;
    snapshot.active_note_id.clear();
    snapshot.active_document_id = payload.document_id;
    normalize_active_entities(&mut snapshot, None);

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "save_document_content",
            "completed",
            "已保存 TXT 文档。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(knowledge_base_id)
        .entity("document", snapshot.active_document_id.clone())
        .relative_path(document_relative_path),
    );

    Ok(snapshot)
}

/** 加载 docx/pdf/图片文档预览，命令层负责定位知识库并把路径授权给 asset protocol。 */
#[tauri::command]
pub async fn load_document_preview(
    app: AppHandle,
    payload: LoadDocumentPreviewPayload,
) -> Result<DocumentPreview, String> {
    let snapshot = payload.snapshot;
    let document = snapshot
        .documents
        .iter()
        .find(|document| document.id == payload.document_id)
        .cloned()
        .ok_or_else(|| "找不到要预览的文档".to_owned())?;
    let knowledge_base = snapshot
        .knowledge_bases
        .iter()
        .find(|item| item.id == document.knowledge_base_id)
        .cloned()
        .ok_or_else(|| "找不到文档所属知识库".to_owned())?;
    let root_path = PathBuf::from(&knowledge_base.path);

    allow_asset_protocol_directory(&app, &root_path)?;

    run_blocking("加载文档预览", move || {
        storage::load_document_preview(&root_path, &document)
    })
    .await
}
