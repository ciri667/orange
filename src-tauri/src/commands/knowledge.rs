use super::common::*;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/**
 * 已通过系统目录对话框确认、但尚未连接的知识库目录。
 *
 * `scan_knowledge_base` 需要一个刚生成的 ID 和目录路径，两者都由前端回传。若后端无条件
 * 接受这对参数，就等于给出一个「读任意目录正文并回传」的入口；而目录选择本应只能来自
 * 系统对话框。这里把「对话框确实选过这个目录」记录成一次性票据：扫描时按 ID 取出比对，
 * 用掉即作废。这样既不改变前端协议，也不再依赖前端自觉。
 */
fn pending_knowledge_base_selections() -> &'static Mutex<HashMap<String, PathBuf>> {
    static PENDING: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

/** 登记一个待连接目录；返回给前端的 ID 只能用于这一次扫描。 */
fn remember_pending_selection(path: &Path) -> Result<String, String> {
    let id = storage::create_id("kb");
    let mut pending = pending_knowledge_base_selections()
        .lock()
        .map_err(|_| "知识库选择登记表已损坏。".to_owned())?;
    // 上界保护：正常情况下每个票据都会被紧接着的扫描消费，残留只会来自取消操作。
    if pending.len() >= 32 {
        pending.clear();
    }
    pending.insert(id.clone(), path.to_path_buf());
    Ok(id)
}

/** 取出并作废一个待连接票据；ID 不存在或路径不一致都视为未授权。 */
fn take_pending_selection(id: &str, path: &Path) -> bool {
    let Ok(mut pending) = pending_knowledge_base_selections().lock() else {
        return false;
    };
    let Some(recorded) = pending.remove(id) else {
        return false;
    };
    match (fs::canonicalize(&recorded), fs::canonicalize(path)) {
        (Ok(recorded), Ok(requested)) => recorded == requested,
        _ => false,
    }
}

#[tauri::command]
pub async fn select_knowledge_base(app: AppHandle) -> Result<KnowledgeBaseSelection, String> {
    let started_at = Instant::now();
    let (sender, mut receiver) = tauri::async_runtime::channel(1);

    app.dialog()
        .file()
        .set_title("选择支持文档知识库目录")
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

    // ID 与路径在同一个票据里生成，扫描阶段据此确认目录确实来自对话框。
    let id = remember_pending_selection(&path)?;
    let selection = KnowledgeBaseSelection {
        id,
        name,
        path: path.to_string_lossy().to_string(),
        note_count,
    };

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::KnowledgeBase,
            "select_knowledge_base",
            "completed",
            "已选择知识库目录。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(selection.id.clone())
        .metadata(json!({ "noteCount": selection.note_count })),
    );

    Ok(selection)
}

/** 扫描用户选择的支持文档目录，并合并进当前工作台快照。 */
#[tauri::command]
pub async fn scan_knowledge_base(
    app: AppHandle,
    payload: ScanKnowledgeBasePayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let selection = payload.selection;
    let selected_knowledge_base_id = selection.id.clone();

    // 目录必须能追溯到系统选择对话框：用一次性票据把「选过这个目录」这件事钉在
    // ID 上，而不是让前端各自传一个 ID 和一个路径、后端无条件相信它们配套。
    // 已连接的知识库走 rescan；这里只接受尚未登记的票据。
    let persisted_roots = run_blocking("读取知识库列表", {
        let app = app.clone();
        move || storage::load_knowledge_base_roots(&app)
    })
    .await?;
    if persisted_roots
        .iter()
        .any(|(id, _)| id == &selected_knowledge_base_id)
    {
        return Err("该知识库已连接，请改用「重新扫描」。".to_owned());
    }
    let requested_path = PathBuf::from(&selection.path);
    if !take_pending_selection(&selected_knowledge_base_id, &requested_path) {
        return Err(
            "该目录未经过系统选择对话框确认，已阻止扫描。请通过「添加知识库」重新选择目录。"
                .to_owned(),
        );
    }

    let (knowledge_base, folders, notes, documents) =
        run_blocking("扫描支持文档知识库", move || {
            storage::scan_supported_documents_directory(&selection)
        })
        .await?;
    let knowledge_base_id = knowledge_base.id.clone();

    allow_asset_protocol_directory(&app, Path::new(&knowledge_base.path))?;

    snapshot.active_knowledge_base_id = knowledge_base.id.clone();
    snapshot.active_note_id = notes
        .first()
        .map(|note| note.id.clone())
        .unwrap_or_default();
    snapshot.active_document_id = if snapshot.active_note_id.is_empty() {
        documents
            .first()
            .map(|document| document.id.clone())
            .unwrap_or_default()
    } else {
        String::new()
    };
    snapshot.knowledge_bases.push(knowledge_base);
    snapshot.folders.extend(folders);
    snapshot.notes.extend(notes);
    snapshot.documents.extend(documents);
    normalize_knowledge_base_flags(&mut snapshot);
    normalize_active_entities(&mut snapshot, Some(&knowledge_base_id));

    reindex_knowledge_base_in_background(app.clone(), &snapshot, knowledge_base_id.clone()).await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::KnowledgeBase,
            "scan_knowledge_base",
            "completed",
            "已连接并扫描知识库。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(knowledge_base_id)
        .metadata(json!({
            "folderCount": snapshot.folders.iter().filter(|folder| folder.knowledge_base_id == selected_knowledge_base_id).count(),
            "noteCount": snapshot.notes.iter().filter(|note| note.knowledge_base_id == selected_knowledge_base_id).count(),
            "documentCount": snapshot.documents.iter().filter(|document| document.knowledge_base_id == selected_knowledge_base_id).count(),
        })),
    );

    Ok(snapshot)
}

/** 重新扫描一个已连接知识库，用真实支持文档替换该知识库的缓存条目。 */
#[tauri::command]
pub async fn rescan_knowledge_base(
    app: AppHandle,
    payload: RescanKnowledgeBasePayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let requested_knowledge_base_id = payload.knowledge_base_id.clone();
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
    let previous_active_document_id = snapshot.active_document_id.clone();
    let scan_result = run_blocking("重新扫描支持文档知识库", move || {
        storage::scan_supported_documents_directory(&selection)
    })
    .await;
    let (mut rescanned_knowledge_base, rescanned_folders, rescanned_notes, rescanned_documents) =
        match scan_result {
            Ok(result) => result,
            Err(error) => {
                let error_message = format!("无法访问已连接目录：{error}");
                let mut failed_knowledge_base = previous_knowledge_base;

                failed_knowledge_base.status = "error".to_owned();
                failed_knowledge_base.description = error_message.clone();
                failed_knowledge_base.note_count = 0;
                failed_knowledge_base.document_count = 0;
                failed_knowledge_base.updated_at = "刚刚".to_owned();
                failed_knowledge_base.scan_report = Some(ScanReport {
                    scanned_file_count: 0,
                    scanned_by_type: crate::domain::default_scanned_by_type(),
                    failed_file_count: 1,
                    skipped_directories: Vec::new(),
                    errors: vec![error_message.clone()],
                    unsupported_files: Vec::new(),
                    indexed_document_count: 0,
                    indexed_chunk_count: 0,
                    failed_document_count: 0,
                    index_failure_summary: None,
                });
                snapshot.knowledge_bases[knowledge_base_index] = failed_knowledge_base;
                snapshot
                    .notes
                    .retain(|note| note.knowledge_base_id != payload.knowledge_base_id);
                snapshot
                    .folders
                    .retain(|folder| folder.knowledge_base_id != payload.knowledge_base_id);
                snapshot
                    .documents
                    .retain(|document| document.knowledge_base_id != payload.knowledge_base_id);
                normalize_sessions_after_rescan(&mut snapshot, &payload.knowledge_base_id);
                normalize_knowledge_base_flags(&mut snapshot);
                normalize_active_entities(&mut snapshot, Some(&payload.knowledge_base_id));
                reindex_knowledge_base_in_background(
                    app.clone(),
                    &snapshot,
                    payload.knowledge_base_id.clone(),
                )
                .await?;

                logging::write_app_event_best_effort(
                    &app,
                    AppEventBuilder::new(
                        AppLogLevel::Warn,
                        AppLogCategory::KnowledgeBase,
                        "rescan_knowledge_base",
                        "failed",
                        error_message,
                    )
                    .duration(started_at.elapsed())
                    .knowledge_base_id(requested_knowledge_base_id.clone()),
                );

                return Ok(snapshot);
            }
        };

    rescanned_knowledge_base.semantic_index_enabled =
        previous_knowledge_base.semantic_index_enabled;
    rescanned_knowledge_base.is_default = previous_knowledge_base.is_default;
    rescanned_knowledge_base.updated_at = "刚刚".to_owned();
    rescanned_knowledge_base.note_count = rescanned_notes.len();
    rescanned_knowledge_base.document_count = rescanned_documents.len();
    allow_asset_protocol_directory(&app, Path::new(&rescanned_knowledge_base.path))?;
    snapshot.knowledge_bases[knowledge_base_index] = rescanned_knowledge_base.clone();

    // 重扫只替换目标知识库的文件条目，其他知识库和会话消息保持不变。
    snapshot
        .notes
        .retain(|note| note.knowledge_base_id != payload.knowledge_base_id);
    snapshot
        .folders
        .retain(|folder| folder.knowledge_base_id != payload.knowledge_base_id);
    snapshot
        .documents
        .retain(|document| document.knowledge_base_id != payload.knowledge_base_id);
    snapshot.folders.extend(rescanned_folders);
    snapshot.notes.extend(rescanned_notes);
    snapshot.documents.extend(rescanned_documents);
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
        snapshot.active_document_id = if snapshot.active_note_id.is_empty() {
            snapshot
                .documents
                .iter()
                .find(|document| document.id == previous_active_document_id)
                .or_else(|| {
                    snapshot
                        .documents
                        .iter()
                        .find(|document| document.knowledge_base_id == payload.knowledge_base_id)
                })
                .map(|document| document.id.clone())
                .unwrap_or_default()
        } else {
            String::new()
        };
    }
    normalize_knowledge_base_flags(&mut snapshot);
    normalize_active_entities(&mut snapshot, Some(&payload.knowledge_base_id));

    reindex_knowledge_base_in_background(app.clone(), &snapshot, payload.knowledge_base_id.clone())
        .await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::KnowledgeBase,
            "rescan_knowledge_base",
            "completed",
            "已重新扫描知识库。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(requested_knowledge_base_id)
        .metadata(json!({
            "noteCount": rescanned_knowledge_base.note_count,
            "documentCount": rescanned_knowledge_base.document_count,
        })),
    );

    Ok(snapshot)
}

#[tauri::command]
pub async fn remove_knowledge_base(
    app: AppHandle,
    payload: RemoveKnowledgeBasePayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let removed_knowledge_base_id = payload.knowledge_base_id.clone();
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
    snapshot
        .documents
        .retain(|document| document.knowledge_base_id != payload.knowledge_base_id);

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
    remove_knowledge_base_from_index_in_background(app.clone(), payload.knowledge_base_id.clone())
        .await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::KnowledgeBase,
            "remove_knowledge_base",
            "completed",
            "已移除知识库授权。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(removed_knowledge_base_id),
    );

    Ok(snapshot)
}

/** 统计目录中的 Markdown 文件数量，用于目录选择后的即时反馈。 */
pub(super) fn count_markdown_files(root: &PathBuf) -> Result<usize, String> {
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

/** 规范知识库默认标记，保证列表中最多只有第一项是默认知识库。 */
pub(super) fn normalize_knowledge_base_flags(snapshot: &mut WorkspaceSnapshot) {
    for (index, knowledge_base) in snapshot.knowledge_bases.iter_mut().enumerate() {
        knowledge_base.is_default = index == 0;
    }
}

/** 重扫后清理会话中已经不存在的笔记引用，避免上下文指向旧文件。 */
pub(crate) fn normalize_sessions_after_rescan(
    snapshot: &mut WorkspaceSnapshot,
    knowledge_base_id: &str,
) {
    let note_ids: std::collections::HashSet<String> =
        snapshot.notes.iter().map(|note| note.id.clone()).collect();

    for session in &mut snapshot.sessions {
        // 只有绑定目标知识库的会话需要修正；多知识库会话中的其他有效笔记引用必须保留。
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

        if session
            .pending_change
            .as_ref()
            .and_then(|change| change.note_id.as_ref())
            .is_some_and(|note_id| !note_ids.contains(note_id))
        {
            session.pending_change = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{remember_pending_selection, take_pending_selection};

    /**
     * 正常流程：对话框登记过的 ID + 同一目录，扫描时应当放行。
     *
     * 这条路径是「添加知识库」的唯一合法入口，不能被授权校验误伤。
     */
    #[test]
    fn pending_selection_accepts_the_recorded_directory_once() {
        let dir = tempfile::tempdir().expect("temp dir");
        let id = remember_pending_selection(dir.path()).expect("remember selection");

        assert!(take_pending_selection(&id, dir.path()));
        // 票据一次性：第二次必须失败，避免同一 ID 被复用去扫描别的目录。
        assert!(!take_pending_selection(&id, dir.path()));
    }

    /**
     * 伪造 ID：从未经过对话框登记时直接拒绝。
     *
     * 这是本次修复的核心——旧实现只信任前端传来的 ID 与路径，等于给出读任意目录的入口。
     */
    #[test]
    fn pending_selection_rejects_unregistered_id() {
        let dir = tempfile::tempdir().expect("temp dir");

        assert!(!take_pending_selection("kb-forged", dir.path()));
    }

    /**
     * 换目录：用登记过的 ID 去扫另一个目录必须拒绝。
     *
     * 否则攻击者只要先正常选一次目录拿到 ID，就能拿它去读任意别的路径。
     */
    #[test]
    fn pending_selection_rejects_a_different_directory() {
        let registered = tempfile::tempdir().expect("registered dir");
        let other = tempfile::tempdir().expect("other dir");
        let id = remember_pending_selection(registered.path()).expect("remember selection");

        assert!(!take_pending_selection(&id, other.path()));
        // 失败的尝试同样作废票据，防止反复试探。
        assert!(!take_pending_selection(&id, registered.path()));
    }

    /** 目录已被删除等无法规范化的情形按未授权处理，而不是放行。 */
    #[test]
    fn pending_selection_rejects_missing_directory() {
        let dir = tempfile::tempdir().expect("temp dir");
        let id = remember_pending_selection(dir.path()).expect("remember selection");
        let missing = dir.path().join("does-not-exist");

        assert!(!take_pending_selection(&id, &missing));
    }
}
