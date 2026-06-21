use crate::agent;
use crate::domain::{
    AgentTurnPayload, AgentTurnResult, ChangePayload, CreateFolderPayload, CreateNotePayload,
    DeleteNotePayload, FolderEntry, KnowledgeBaseSelection, RemoveKnowledgeBasePayload,
    RenameNotePayload, RescanKnowledgeBasePayload, SaveNoteContentPayload,
    ScanKnowledgeBasePayload, ScanReport, WorkspaceSnapshot,
};
use crate::storage;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

/** 加载工作台初始状态；从 SQLite 恢复已连接知识库并重新扫描真实 Markdown。 */
#[tauri::command]
pub async fn load_workspace_state(app: AppHandle) -> Result<WorkspaceSnapshot, String> {
    let load_app = app.clone();

    run_blocking("加载工作台状态", move || {
        let snapshot = storage::load_workspace_snapshot(&load_app)?;

        // 启动恢复后立即刷新 FTS，确保外部编辑器改过的 Markdown 能被本轮 Agent 检索命中。
        storage::index_snapshot(&load_app, &snapshot)?;

        Ok(snapshot)
    })
    .await
}

/** 打开系统目录选择器，让用户连接一个本地 Markdown 知识库。 */
#[tauri::command]
pub async fn select_knowledge_base(app: AppHandle) -> Result<KnowledgeBaseSelection, String> {
    let (sender, mut receiver) = tauri::async_runtime::channel(1);

    app.dialog()
        .file()
        .set_title("选择 Markdown 知识库目录")
        .pick_folder(move |selected_path| {
            let _ = sender.blocking_send(selected_path);
        });

    let selected_path = receiver
        .recv()
        .await
        .flatten()
        .ok_or_else(|| "未选择知识库目录".to_owned())?;
    let path = selected_path
        .as_path()
        .ok_or_else(|| "无法读取所选目录路径".to_owned())?
        .to_path_buf();
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("本地知识库")
        .to_owned();
    let count_path = path.clone();
    let note_count =
        tauri::async_runtime::spawn_blocking(move || count_markdown_files(&count_path))
            .await
            .map_err(|error| format!("统计 Markdown 文件时后台任务失败：{error}"))??;

    Ok(KnowledgeBaseSelection {
        id: storage::create_id("kb"),
        name,
        path: path.to_string_lossy().to_string(),
        note_count,
    })
}

/** 扫描用户选择的 Markdown 目录，并合并进当前工作台快照。 */
#[tauri::command]
pub async fn scan_knowledge_base(
    app: AppHandle,
    payload: ScanKnowledgeBasePayload,
) -> Result<WorkspaceSnapshot, String> {
    let mut snapshot = payload.snapshot;
    let selection = payload.selection;
    let (knowledge_base, folders, notes) = run_blocking("扫描 Markdown 知识库", move || {
        storage::scan_markdown_directory(&selection)
    })
    .await?;
    let knowledge_base_id = knowledge_base.id.clone();

    snapshot.active_knowledge_base_id = knowledge_base.id.clone();
    snapshot.active_note_id = notes
        .first()
        .map(|note| note.id.clone())
        .unwrap_or_default();
    snapshot.active_session_id = ensure_knowledge_base_session(&mut snapshot, &knowledge_base);
    snapshot.knowledge_bases.push(knowledge_base);
    snapshot.folders.extend(folders);
    snapshot.notes.extend(notes);
    normalize_knowledge_base_flags(&mut snapshot);
    normalize_active_entities(&mut snapshot, Some(&knowledge_base_id));

    index_snapshot_in_background(app, &snapshot).await?;

    Ok(snapshot)
}

/** 重新扫描一个已连接知识库，用真实 Markdown 文件替换该知识库的缓存笔记。 */
#[tauri::command]
pub async fn rescan_knowledge_base(
    app: AppHandle,
    payload: RescanKnowledgeBasePayload,
) -> Result<WorkspaceSnapshot, String> {
    let mut snapshot = payload.snapshot;
    let knowledge_base_index = snapshot
        .knowledge_bases
        .iter()
        .position(|knowledge_base| knowledge_base.id == payload.knowledge_base_id)
        .ok_or_else(|| "找不到要重新扫描的知识库".to_owned())?;
    let previous_knowledge_base = snapshot.knowledge_bases[knowledge_base_index].clone();
    let selection = KnowledgeBaseSelection {
        id: previous_knowledge_base.id.clone(),
        name: previous_knowledge_base.name.clone(),
        path: previous_knowledge_base.path.clone(),
        note_count: previous_knowledge_base.note_count,
    };
    let previous_active_note_id = snapshot.active_note_id.clone();
    let scan_result = run_blocking("重新扫描 Markdown 知识库", move || {
        storage::scan_markdown_directory(&selection)
    })
    .await;
    let (mut rescanned_knowledge_base, rescanned_folders, rescanned_notes) = match scan_result {
        Ok(result) => result,
        Err(error) => {
            let error_message = format!("无法访问已连接目录：{error}");
            let mut failed_knowledge_base = previous_knowledge_base;

            failed_knowledge_base.status = "error".to_owned();
            failed_knowledge_base.description = error_message.clone();
            failed_knowledge_base.note_count = 0;
            failed_knowledge_base.updated_at = "刚刚".to_owned();
            failed_knowledge_base.scan_report = Some(ScanReport {
                scanned_file_count: 0,
                failed_file_count: 1,
                skipped_directories: Vec::new(),
                errors: vec![error_message],
            });
            snapshot.knowledge_bases[knowledge_base_index] = failed_knowledge_base;
            snapshot
                .notes
                .retain(|note| note.knowledge_base_id != payload.knowledge_base_id);
            snapshot
                .folders
                .retain(|folder| folder.knowledge_base_id != payload.knowledge_base_id);
            normalize_sessions_after_rescan(&mut snapshot, &payload.knowledge_base_id);
            normalize_knowledge_base_flags(&mut snapshot);
            normalize_active_entities(&mut snapshot, Some(&payload.knowledge_base_id));
            index_snapshot_in_background(app, &snapshot).await?;

            return Ok(snapshot);
        }
    };

    rescanned_knowledge_base.semantic_index_enabled =
        previous_knowledge_base.semantic_index_enabled;
    rescanned_knowledge_base.is_default = previous_knowledge_base.is_default;
    rescanned_knowledge_base.updated_at = "刚刚".to_owned();
    rescanned_knowledge_base.note_count = rescanned_notes.len();
    snapshot.knowledge_bases[knowledge_base_index] = rescanned_knowledge_base.clone();

    // 重扫只替换目标知识库的笔记，其他知识库和会话消息保持不变。
    snapshot
        .notes
        .retain(|note| note.knowledge_base_id != payload.knowledge_base_id);
    snapshot
        .folders
        .retain(|folder| folder.knowledge_base_id != payload.knowledge_base_id);
    snapshot.folders.extend(rescanned_folders);
    snapshot.notes.extend(rescanned_notes);
    normalize_sessions_after_rescan(&mut snapshot, &payload.knowledge_base_id);

    if snapshot.active_knowledge_base_id == payload.knowledge_base_id {
        snapshot.active_note_id = snapshot
            .notes
            .iter()
            .find(|note| note.id == previous_active_note_id)
            .or_else(|| {
                snapshot
                    .notes
                    .iter()
                    .find(|note| note.knowledge_base_id == payload.knowledge_base_id)
            })
            .map(|note| note.id.clone())
            .unwrap_or_default();
        snapshot.active_session_id =
            ensure_knowledge_base_session(&mut snapshot, &rescanned_knowledge_base);
    }
    normalize_knowledge_base_flags(&mut snapshot);
    normalize_active_entities(&mut snapshot, Some(&payload.knowledge_base_id));

    index_snapshot_in_background(app, &snapshot).await?;

    Ok(snapshot)
}

/** 用户主动新建空白 Markdown，直接落盘并打开为当前可编辑笔记。 */
#[tauri::command]
pub async fn create_note(
    app: AppHandle,
    payload: CreateNotePayload,
) -> Result<WorkspaceSnapshot, String> {
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
    let note_id = storage::create_stable_note_id(&knowledge_base.id, &relative_path);
    let new_note = crate::domain::Note {
        id: note_id.clone(),
        knowledge_base_id: knowledge_base.id.clone(),
        title: note_title_from_path(&relative_path),
        path: relative_path,
        content: String::new(),
        tags: Vec::new(),
        updated_at: "刚刚".to_owned(),
        backlinks: Vec::new(),
        content_hash: storage::hash_content(""),
    };

    snapshot.notes.insert(0, new_note);
    snapshot.knowledge_bases[knowledge_base_index].note_count += 1;
    snapshot.knowledge_bases[knowledge_base_index].updated_at = "刚刚".to_owned();
    if let Some(scan_report) = &mut snapshot.knowledge_bases[knowledge_base_index].scan_report {
        scan_report.scanned_file_count += 1;
    }
    snapshot.active_knowledge_base_id = knowledge_base.id.clone();
    snapshot.active_note_id = note_id;
    normalize_active_entities(&mut snapshot, Some(&knowledge_base.id));
    index_snapshot_in_background(app, &snapshot).await?;

    Ok(snapshot)
}

/** 用户在目录树的指定目录下新建文件夹，成功后只更新目录快照不切换当前笔记。 */
#[tauri::command]
pub async fn create_folder(
    app: AppHandle,
    payload: CreateFolderPayload,
) -> Result<WorkspaceSnapshot, String> {
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
    let folder_entry = FolderEntry {
        id: storage::create_stable_folder_id(&knowledge_base.id, &relative_path),
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
    index_snapshot_in_background(app, &snapshot).await?;

    Ok(snapshot)
}

/** 重命名 Markdown 文件，只修改文件名，并同步更新快照与会话引用。 */
#[tauri::command]
pub async fn rename_note(
    app: AppHandle,
    payload: RenameNotePayload,
) -> Result<WorkspaceSnapshot, String> {
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

    snapshot.notes[note_index].id = next_note_id.clone();
    snapshot.notes[note_index].title = next_title;
    snapshot.notes[note_index].path = next_relative_path.clone();
    snapshot.notes[note_index].content = current_content;
    snapshot.notes[note_index].content_hash = current_hash;
    snapshot.notes[note_index].updated_at = "刚刚".to_owned();

    replace_note_reference_after_rename(
        &mut snapshot,
        &payload.note_id,
        &next_note_id,
        &next_relative_path,
    );
    index_snapshot_in_background(app, &snapshot).await?;

    Ok(snapshot)
}

/** 删除 Markdown 文件到系统回收站，并从快照中移除笔记和相关会话引用。 */
#[tauri::command]
pub async fn delete_note(
    app: AppHandle,
    payload: DeleteNotePayload,
) -> Result<WorkspaceSnapshot, String> {
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
    index_snapshot_in_background(app, &snapshot).await?;

    Ok(snapshot)
}

/** 保存当前笔记正文，校验知识库边界和文件 hash 后原子写回 Markdown。 */
#[tauri::command]
pub async fn save_note_content(
    app: AppHandle,
    payload: SaveNoteContentPayload,
) -> Result<WorkspaceSnapshot, String> {
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
    let target_path = storage::resolve_existing_file_inside_root(
        PathBuf::from(&knowledge_base.path).as_path(),
        &snapshot.notes[note_index].path,
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
        return Err("目标文件已被外部修改，已阻止保存。请重新扫描后再编辑。".to_owned());
    }

    let write_path = target_path.clone();
    let write_content = payload.content.clone();

    run_blocking("保存 Markdown 文件", move || {
        storage::atomic_write_markdown(&write_path, &write_content)
    })
    .await?;

    let next_hash = storage::hash_content(&payload.content);
    snapshot.notes[note_index].content = payload.content;
    snapshot.notes[note_index].content_hash = next_hash;
    snapshot.notes[note_index].updated_at = "刚刚".to_owned();
    snapshot.active_note_id = payload.note_id;
    normalize_active_entities(&mut snapshot, None);
    index_snapshot_in_background(app, &snapshot).await?;

    Ok(snapshot)
}

/** 移除知识库授权记录和本地索引缓存，不删除用户目录中的 Markdown 文件。 */
#[tauri::command]
pub async fn remove_knowledge_base(
    app: AppHandle,
    payload: RemoveKnowledgeBasePayload,
) -> Result<WorkspaceSnapshot, String> {
    let mut snapshot = payload.snapshot;

    snapshot
        .knowledge_bases
        .retain(|knowledge_base| knowledge_base.id != payload.knowledge_base_id);
    snapshot
        .notes
        .retain(|note| note.knowledge_base_id != payload.knowledge_base_id);
    snapshot
        .folders
        .retain(|folder| folder.knowledge_base_id != payload.knowledge_base_id);

    // 会话只移除目标知识库范围；失去全部范围的会话同步删除，避免保留不可用上下文。
    snapshot.sessions.retain_mut(|session| {
        session
            .knowledge_base_ids
            .retain(|id| id != &payload.knowledge_base_id);
        session
            .pinned_note_ids
            .retain(|note_id| snapshot.notes.iter().any(|note| note.id == *note_id));

        if session
            .active_note_id
            .as_ref()
            .is_some_and(|note_id| !snapshot.notes.iter().any(|note| note.id == *note_id))
        {
            session.active_note_id = None;
        }

        !session.knowledge_base_ids.is_empty()
    });

    normalize_knowledge_base_flags(&mut snapshot);
    normalize_active_entities(&mut snapshot, None);
    index_snapshot_in_background(app, &snapshot).await?;

    Ok(snapshot)
}

/** 运行 Agent 单轮 loop，检索作为工具由 Agent 自行选择。 */
#[tauri::command]
pub async fn run_agent_turn(
    app: AppHandle,
    payload: AgentTurnPayload,
) -> Result<AgentTurnResult, String> {
    let result = agent::run_agent_turn(&app, payload.snapshot, payload.request);

    // 每轮后刷新本地索引，确保新草稿确认前不会写入，但现有笔记编辑能被检索。
    index_snapshot_in_background(app, &result.snapshot).await?;

    Ok(result)
}

/** 确认待写入 diff，校验知识库边界和内容 hash 后原子写回 Markdown。 */
#[tauri::command]
pub async fn apply_proposed_change(
    app: AppHandle,
    payload: ChangePayload,
) -> Result<WorkspaceSnapshot, String> {
    let mut snapshot = payload.snapshot;
    let session_index = snapshot
        .sessions
        .iter()
        .position(|session| session.id == snapshot.active_session_id)
        .ok_or_else(|| "找不到当前 Agent 会话".to_owned())?;
    let Some(change) = snapshot.sessions[session_index].pending_change.clone() else {
        return Ok(snapshot);
    };
    let knowledge_base = snapshot
        .knowledge_bases
        .iter()
        .find(|item| item.id == change.knowledge_base_id)
        .ok_or_else(|| "找不到变更所属知识库".to_owned())?;
    let target_path = storage::resolve_inside_root(
        PathBuf::from(&knowledge_base.path).as_path(),
        &change.target_path,
    )?;

    if change.r#type == "create" {
        // 新建草稿不能覆盖用户已有文件；如路径已存在，应重新生成不同目标路径的 diff。
        if target_path.exists() {
            return Err("目标 Markdown 已存在，已阻止覆盖。请重新生成草稿路径。".to_owned());
        }

        let write_path = target_path.clone();
        let next_content = change.next.clone();

        run_blocking("写入新 Markdown 文件", move || {
            storage::atomic_write_markdown(&write_path, &next_content)
        })
        .await?;
        snapshot.notes.insert(
            0,
            crate::domain::Note {
                id: storage::create_stable_note_id(&change.knowledge_base_id, &change.target_path),
                knowledge_base_id: change.knowledge_base_id.clone(),
                title: change.title.replace("创建《", "").replace("》草稿", ""),
                path: change.target_path.clone(),
                content: change.next.clone(),
                tags: vec!["Agent".to_owned(), "草稿".to_owned()],
                updated_at: "刚刚".to_owned(),
                backlinks: Vec::new(),
                content_hash: storage::hash_content(&change.next),
            },
        );
    } else if let Some(note_id) = &change.note_id {
        let note_index = snapshot
            .notes
            .iter()
            .position(|note| note.id == *note_id)
            .ok_or_else(|| "找不到待写入笔记".to_owned())?;
        let read_path = target_path.clone();
        let fallback_content = snapshot.notes[note_index].content.clone();
        let current_content = run_blocking("读取待写入 Markdown 文件", move || {
            Ok(fs::read_to_string(&read_path).unwrap_or(fallback_content))
        })
        .await?;
        let current_hash = storage::hash_content(&current_content);

        // hash 不一致说明文件可能被外部修改，必须阻止写入并要求用户重新生成 diff。
        if current_hash != change.original_hash
            && snapshot.notes[note_index].content_hash != change.original_hash
        {
            return Err("目标文件已变化，已阻止写入。请重新生成 diff。".to_owned());
        }

        let next_content = current_content.replace(&change.original, &change.next);
        let write_path = target_path.clone();
        let write_content = next_content.clone();

        run_blocking("写回 Markdown 文件", move || {
            storage::atomic_write_markdown(&write_path, &write_content)
        })
        .await?;
        snapshot.notes[note_index].content = next_content.clone();
        snapshot.notes[note_index].content_hash = storage::hash_content(&next_content);
        snapshot.notes[note_index].updated_at = "刚刚".to_owned();
    }

    snapshot.sessions[session_index].pending_change = Some(crate::domain::ProposedChange {
        status: "accepted".to_owned(),
        ..change
    });
    index_snapshot_in_background(app, &snapshot).await?;

    Ok(snapshot)
}

/** 拒绝待写入 diff，只更新会话状态，不修改任何 Markdown 文件。 */
#[tauri::command]
pub fn reject_proposed_change(payload: ChangePayload) -> Result<WorkspaceSnapshot, String> {
    let mut snapshot = payload.snapshot;
    let session_index = snapshot
        .sessions
        .iter()
        .position(|session| session.id == snapshot.active_session_id)
        .ok_or_else(|| "找不到当前 Agent 会话".to_owned())?;

    if let Some(change) = snapshot.sessions[session_index].pending_change.clone() {
        snapshot.sessions[session_index].pending_change = Some(crate::domain::ProposedChange {
            status: "rejected".to_owned(),
            ..change
        });
    }

    Ok(snapshot)
}

/** 在 Tauri 后台阻塞线程中运行文件系统或 SQLite 重任务，避免卡住 WebView 主线程。 */
async fn run_blocking<T, F>(label: &str, task: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|error| format!("{label}后台任务失败：{error}"))?
}

/** 在后台刷新 SQLite/FTS5 索引，确保大知识库写索引时界面仍可响应。 */
async fn index_snapshot_in_background(
    app: AppHandle,
    snapshot: &WorkspaceSnapshot,
) -> Result<(), String> {
    let index_app = app.clone();
    let index_snapshot = snapshot.clone();

    run_blocking("刷新本地检索索引", move || {
        storage::index_snapshot(&index_app, &index_snapshot)
    })
    .await
}

/** 统计目录中的 Markdown 文件数量，用于目录选择后的即时反馈。 */
fn count_markdown_files(root: &PathBuf) -> Result<usize, String> {
    let mut count = 0;

    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(storage::should_walk_entry)
        .filter_map(Result::ok)
    {
        let path = entry.path();

        // 只统计 Markdown 文件；真实扫描阶段会进一步解析标题、标签和正文。
        if path.is_file()
            && matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("md") | Some("markdown")
            )
        {
            count += 1;
        }
    }

    Ok(count)
}

/** 确保知识库至少有一个默认会话，并返回这个会话 ID。 */
fn ensure_knowledge_base_session(
    snapshot: &mut WorkspaceSnapshot,
    knowledge_base: &crate::domain::KnowledgeBase,
) -> String {
    if let Some(session) = snapshot.sessions.iter().find(|session| {
        session.r#type == "knowledge-base"
            && session.knowledge_base_ids.len() == 1
            && session.knowledge_base_ids[0] == knowledge_base.id
    }) {
        return session.id.clone();
    }

    let session = storage::create_default_agent_session(knowledge_base);
    let session_id = session.id.clone();

    snapshot.sessions.insert(0, session);
    session_id
}

/** 从新建文件相对路径提取初始标题，空白正文会在重扫时继续使用文件名。 */
fn note_title_from_path(relative_path: &str) -> String {
    Path::new(relative_path)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("未命名")
        .to_owned()
}

/** 从目录相对路径取最后一级名称，用于 create_folder 后立即生成前端目录节点。 */
fn folder_name_from_path(relative_path: &str) -> String {
    Path::new(relative_path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("未命名目录")
        .to_owned()
}

/** 重命名后把活跃笔记、固定笔记和待确认 diff 中的旧 note id 迁移到新 id。 */
fn replace_note_reference_after_rename(
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
                change.target_path = next_relative_path.to_owned();
            }
        }
    }
}

/** 删除后清理会话中的笔记引用和待确认 diff，避免 UI 指向已移入回收站的文件。 */
fn remove_note_references_after_delete(snapshot: &mut WorkspaceSnapshot, note_id: &str) {
    if snapshot.active_note_id == note_id {
        snapshot.active_note_id.clear();
    }

    for session in &mut snapshot.sessions {
        if session.active_note_id.as_deref() == Some(note_id) {
            session.active_note_id = None;
        }

        session.pinned_note_ids.retain(|id| id != note_id);

        if session
            .pending_change
            .as_ref()
            .is_some_and(|change| change.note_id.as_deref() == Some(note_id))
        {
            session.pending_change = None;
        }
    }
}

/** 规范知识库默认标记，保证列表中最多只有第一项是默认知识库。 */
fn normalize_knowledge_base_flags(snapshot: &mut WorkspaceSnapshot) {
    for (index, knowledge_base) in snapshot.knowledge_bases.iter_mut().enumerate() {
        knowledge_base.is_default = index == 0;
    }
}

/** 修正活跃知识库、笔记和会话，避免扫描、移除后工作台指向不存在的对象。 */
fn normalize_active_entities(
    snapshot: &mut WorkspaceSnapshot,
    preferred_knowledge_base_id: Option<&str>,
) {
    if snapshot.knowledge_bases.is_empty() {
        snapshot.active_knowledge_base_id.clear();
        snapshot.active_note_id.clear();
        snapshot.active_session_id.clear();
        return;
    }

    let active_knowledge_base_exists = snapshot
        .knowledge_bases
        .iter()
        .any(|knowledge_base| knowledge_base.id == snapshot.active_knowledge_base_id);

    if !active_knowledge_base_exists {
        snapshot.active_knowledge_base_id = preferred_knowledge_base_id
            .and_then(|knowledge_base_id| {
                snapshot
                    .knowledge_bases
                    .iter()
                    .find(|knowledge_base| knowledge_base.id == knowledge_base_id)
                    .map(|knowledge_base| knowledge_base.id.clone())
            })
            .or_else(|| {
                snapshot
                    .knowledge_bases
                    .first()
                    .map(|knowledge_base| knowledge_base.id.clone())
            })
            .unwrap_or_default();
    }

    let active_note_exists = snapshot
        .notes
        .iter()
        .any(|note| note.id == snapshot.active_note_id);

    if !active_note_exists {
        snapshot.active_note_id = snapshot
            .notes
            .iter()
            .find(|note| note.knowledge_base_id == snapshot.active_knowledge_base_id)
            .map(|note| note.id.clone())
            .unwrap_or_default();
    }

    let active_session_exists = snapshot
        .sessions
        .iter()
        .any(|session| session.id == snapshot.active_session_id);

    if !active_session_exists {
        let active_knowledge_base = snapshot
            .knowledge_bases
            .iter()
            .find(|knowledge_base| knowledge_base.id == snapshot.active_knowledge_base_id)
            .cloned();

        if let Some(knowledge_base) = active_knowledge_base {
            snapshot.active_session_id = ensure_knowledge_base_session(snapshot, &knowledge_base);
        }
    }
}

/** 重扫后清理会话中已经不存在的笔记引用，避免上下文指向旧文件。 */
fn normalize_sessions_after_rescan(snapshot: &mut WorkspaceSnapshot, knowledge_base_id: &str) {
    let note_ids: std::collections::HashSet<String> = snapshot
        .notes
        .iter()
        .filter(|note| note.knowledge_base_id == knowledge_base_id)
        .map(|note| note.id.clone())
        .collect();

    for session in &mut snapshot.sessions {
        // 只有绑定目标知识库的会话需要修正，其他会话上下文不能被重扫影响。
        if !session
            .knowledge_base_ids
            .iter()
            .any(|id| id == knowledge_base_id)
        {
            continue;
        }

        if session
            .active_note_id
            .as_ref()
            .is_some_and(|active_note_id| !note_ids.contains(active_note_id))
        {
            session.active_note_id = None;
        }

        session
            .pinned_note_ids
            .retain(|note_id| note_ids.contains(note_id));
    }
}
