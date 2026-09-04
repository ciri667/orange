use super::*;

pub(crate) struct StoredKnowledgeBase {
    id: String,
    name: String,
    path: String,
    semantic_index_enabled: bool,
    updated_at: String,
}

/** 全量重建 FTS，供知识库扫描和启动索引使用。 */
pub fn index_snapshot(app: &AppHandle, snapshot: &WorkspaceSnapshot) -> Result<(), String> {
    index_snapshot_with_mode(app, snapshot, IndexMode::ReplaceAll)
}

/** 只 upsert 快照中的笔记，供 Agent 回合使用，避免过期快照删掉其它会话写入的笔记。 */
pub fn upsert_snapshot_index(app: &AppHandle, snapshot: &WorkspaceSnapshot) -> Result<(), String> {
    index_snapshot_with_mode(app, snapshot, IndexMode::Upsert)
}

/** 只刷新给定笔记的 FTS 行，供编辑器单文件保存使用。 */
pub fn upsert_notes_index(app: &AppHandle, notes: &[Note]) -> Result<(), String> {
    if notes.is_empty() {
        return Ok(());
    }
    let mut connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| format!("无法启动索引事务：{error}"))?;
    for note in notes {
        upsert_note_index(&transaction, note)?;
    }
    transaction
        .commit()
        .map_err(|error| format!("无法提交索引事务：{error}"))?;
    Ok(())
}

#[derive(Clone, Copy)]
pub(crate) enum IndexMode {
    ReplaceAll,
    Upsert,
}

fn index_snapshot_with_mode(
    app: &AppHandle,
    snapshot: &WorkspaceSnapshot,
    mode: IndexMode,
) -> Result<(), String> {
    let index_signature = build_index_signature(snapshot);
    let mut connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    let should_rebuild_index = match mode {
        IndexMode::Upsert => true,
        IndexMode::ReplaceAll => {
            let completed_signature = COMPLETED_INDEX_SIGNATURE
                .get_or_init(|| Mutex::new(None))
                .lock()
                .map_err(|_| "FTS 索引签名锁已损坏。".to_owned())?;

            completed_signature.as_deref() != Some(index_signature.as_str())
        }
    };
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| format!("无法启动索引事务：{error}"))?;

    if should_rebuild_index {
        apply_index_to_transaction(&transaction, snapshot, mode)?;
    }

    transaction
        .commit()
        .map_err(|error| format!("无法提交索引事务：{error}"))?;

    if should_rebuild_index {
        if matches!(mode, IndexMode::ReplaceAll) {
            let mut completed_signature = COMPLETED_INDEX_SIGNATURE
                .get_or_init(|| Mutex::new(None))
                .lock()
                .map_err(|_| "FTS 索引签名锁已损坏。".to_owned())?;

            *completed_signature = Some(index_signature);
        }
    }

    Ok(())
}

/** 在已有事务中写入索引；ReplaceAll 会清空后重建，Upsert 只覆盖快照里出现的笔记。 */
pub(crate) fn apply_index_to_transaction(
    transaction: &rusqlite::Transaction<'_>,
    snapshot: &WorkspaceSnapshot,
    mode: IndexMode,
) -> Result<(), String> {
    if matches!(mode, IndexMode::ReplaceAll) {
        transaction
            .execute("DELETE FROM note_fts", [])
            .map_err(|error| format!("无法清理 FTS 索引：{error}"))?;
        transaction
            .execute("DELETE FROM notes", [])
            .map_err(|error| format!("无法清理笔记索引：{error}"))?;
        transaction
            .execute("DELETE FROM knowledge_bases", [])
            .map_err(|error| format!("无法清理知识库索引：{error}"))?;
    }

    for knowledge_base in &snapshot.knowledge_bases {
        transaction
            .execute(
                "INSERT OR REPLACE INTO knowledge_bases (id, name, path, semantic_index_enabled, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    &knowledge_base.id,
                    &knowledge_base.name,
                    &knowledge_base.path,
                    if knowledge_base.semantic_index_enabled { 1 } else { 0 },
                    &knowledge_base.updated_at
                ],
            )
            .map_err(|error| format!("无法写入知识库索引：{error}"))?;
    }

    for note in &snapshot.notes {
        upsert_note_index(transaction, note)?;
    }

    Ok(())
}

/** 从索引中删除指定笔记，供删除/重命名后的旧 ID 使用。 */
pub fn remove_note_ids_from_index(app: &AppHandle, note_ids: &[String]) -> Result<(), String> {
    if note_ids.is_empty() {
        return Ok(());
    }
    let mut connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| format!("无法启动索引事务：{error}"))?;
    remove_note_ids_from_transaction(&transaction, note_ids)?;
    transaction
        .commit()
        .map_err(|error| format!("无法提交索引事务：{error}"))?;
    Ok(())
}

/** 重扫单个知识库：只替换该库的笔记索引，其它库保持不变。 */
pub fn reindex_knowledge_base_notes(
    app: &AppHandle,
    snapshot: &WorkspaceSnapshot,
    knowledge_base_id: &str,
) -> Result<(), String> {
    let mut connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| format!("无法启动索引事务：{error}"))?;
    reindex_knowledge_base_notes_in_transaction(&transaction, snapshot, knowledge_base_id)?;
    transaction
        .commit()
        .map_err(|error| format!("无法提交索引事务：{error}"))?;
    Ok(())
}

/** 移除知识库授权时删掉该库的笔记索引。 */
pub fn remove_knowledge_base_from_index(
    app: &AppHandle,
    knowledge_base_id: &str,
) -> Result<(), String> {
    let mut connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| format!("无法启动索引事务：{error}"))?;
    remove_knowledge_base_from_transaction(&transaction, knowledge_base_id)?;
    transaction
        .commit()
        .map_err(|error| format!("无法提交索引事务：{error}"))?;
    Ok(())
}

pub(crate) fn remove_note_ids_from_transaction(
    transaction: &rusqlite::Transaction<'_>,
    note_ids: &[String],
) -> Result<(), String> {
    for note_id in note_ids {
        transaction
            .execute("DELETE FROM note_fts WHERE note_id = ?1", params![note_id])
            .map_err(|error| format!("无法删除 FTS 索引：{error}"))?;
        transaction
            .execute("DELETE FROM notes WHERE id = ?1", params![note_id])
            .map_err(|error| format!("无法删除笔记索引：{error}"))?;
    }
    Ok(())
}

pub(crate) fn remove_knowledge_base_from_transaction(
    transaction: &rusqlite::Transaction<'_>,
    knowledge_base_id: &str,
) -> Result<(), String> {
    transaction
        .execute(
            "DELETE FROM note_fts WHERE knowledge_base_id = ?1",
            params![knowledge_base_id],
        )
        .map_err(|error| format!("无法删除知识库 FTS 索引：{error}"))?;
    transaction
        .execute(
            "DELETE FROM notes WHERE knowledge_base_id = ?1",
            params![knowledge_base_id],
        )
        .map_err(|error| format!("无法删除知识库笔记索引：{error}"))?;
    transaction
        .execute(
            "DELETE FROM knowledge_bases WHERE id = ?1",
            params![knowledge_base_id],
        )
        .map_err(|error| format!("无法删除知识库索引：{error}"))?;
    Ok(())
}

pub(crate) fn reindex_knowledge_base_notes_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    snapshot: &WorkspaceSnapshot,
    knowledge_base_id: &str,
) -> Result<(), String> {
    remove_knowledge_base_from_transaction(transaction, knowledge_base_id)?;
    if let Some(knowledge_base) = snapshot
        .knowledge_bases
        .iter()
        .find(|item| item.id == knowledge_base_id)
    {
        transaction
            .execute(
                "INSERT OR REPLACE INTO knowledge_bases (id, name, path, semantic_index_enabled, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    &knowledge_base.id,
                    &knowledge_base.name,
                    &knowledge_base.path,
                    if knowledge_base.semantic_index_enabled { 1 } else { 0 },
                    &knowledge_base.updated_at
                ],
            )
            .map_err(|error| format!("无法写入知识库索引：{error}"))?;
    }
    for note in snapshot
        .notes
        .iter()
        .filter(|note| note.knowledge_base_id == knowledge_base_id)
    {
        upsert_note_index(transaction, note)?;
    }
    Ok(())
}

fn upsert_note_index(transaction: &rusqlite::Transaction<'_>, note: &Note) -> Result<(), String> {
    transaction
        .execute("DELETE FROM note_fts WHERE note_id = ?1", params![&note.id])
        .map_err(|error| format!("无法更新 FTS 索引：{error}"))?;
    transaction
        .execute(
            "INSERT OR REPLACE INTO notes (id, knowledge_base_id, title, path, content_hash, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![&note.id, &note.knowledge_base_id, &note.title, &note.path, &note.content_hash, &note.updated_at],
        )
        .map_err(|error| format!("无法写入笔记索引：{error}"))?;
    transaction
        .execute(
            "INSERT INTO note_fts (note_id, knowledge_base_id, title, path, body) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![&note.id, &note.knowledge_base_id, &note.title, &note.path, &note.content],
        )
        .map_err(|error| format!("无法写入 FTS 索引：{error}"))?;
    Ok(())
}

pub fn load_workspace_snapshot(app: &AppHandle) -> Result<WorkspaceSnapshot, String> {
    let connection = open_database(app)?;
    let mut statement = connection
        .prepare("SELECT id, name, path, semantic_index_enabled, updated_at FROM knowledge_bases ORDER BY rowid")
        .map_err(|error| format!("无法读取知识库列表：{error}"))?;
    let stored_rows = statement
        .query_map([], |row| {
            Ok(StoredKnowledgeBase {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
                semantic_index_enabled: row.get::<_, i64>(3)? == 1,
                updated_at: row.get(4)?,
            })
        })
        .map_err(|error| format!("无法查询知识库列表：{error}"))?;
    let mut stored_knowledge_bases = Vec::new();

    for stored_row in stored_rows {
        stored_knowledge_bases
            .push(stored_row.map_err(|error| format!("无法解析知识库记录：{error}"))?);
    }

    // 文件系统扫描可能耗时较长，必须先释放 SQLite statement，避免长读锁阻塞后台 FTS 重建。
    drop(statement);

    let mut knowledge_bases = Vec::new();
    let mut folders = Vec::new();
    let mut notes = Vec::new();
    let mut documents = Vec::new();

    for stored_knowledge_base in stored_knowledge_bases {
        let selection = KnowledgeBaseSelection {
            id: stored_knowledge_base.id.clone(),
            name: stored_knowledge_base.name.clone(),
            path: stored_knowledge_base.path.clone(),
            note_count: 0,
        };

        // 启动时以本地文件为准重新扫描，避免 SQLite 缓存覆盖用户在外部编辑器中的修改。
        match scan_supported_documents_directory(&selection) {
            Ok((mut knowledge_base, scanned_folders, scanned_notes, scanned_documents)) => {
                knowledge_base.semantic_index_enabled =
                    stored_knowledge_base.semantic_index_enabled;
                knowledge_base.updated_at = stored_knowledge_base.updated_at;
                knowledge_base.is_default = knowledge_bases.is_empty();
                knowledge_base.note_count = scanned_notes.len();
                knowledge_base.document_count = scanned_documents.len();
                folders.extend(scanned_folders);
                notes.extend(scanned_notes);
                documents.extend(scanned_documents);
                knowledge_bases.push(knowledge_base);
            }
            Err(error) => {
                let error_message = format!("无法访问已连接目录：{error}");

                knowledge_bases.push(KnowledgeBase {
                    id: stored_knowledge_base.id,
                    name: stored_knowledge_base.name,
                    path: stored_knowledge_base.path,
                    description: error_message.clone(),
                    status: "error".to_owned(),
                    note_count: 0,
                    document_count: 0,
                    updated_at: stored_knowledge_base.updated_at,
                    is_default: knowledge_bases.is_empty(),
                    semantic_index_enabled: stored_knowledge_base.semantic_index_enabled,
                    scan_report: Some(ScanReport {
                        scanned_file_count: 0,
                        scanned_by_type: create_scanned_by_type_counter(),
                        failed_file_count: 1,
                        skipped_directories: Vec::new(),
                        errors: vec![error_message],
                    }),
                });
            }
        }
    }

    let active_knowledge_base_id = knowledge_bases
        .first()
        .map(|knowledge_base| knowledge_base.id.clone())
        .unwrap_or_default();
    // 冷启动只定位默认知识库，不再把排序后的首个文件当作用户正在编辑的文件。
    // 真实编辑器焦点由 load_workspace_bootstrap_state 恢复的会话状态决定。
    let active_note_id = String::new();
    let active_document_id = String::new();
    let mut snapshot = WorkspaceSnapshot {
        knowledge_bases,
        folders,
        notes,
        documents,
        sessions: Vec::new(),
        active_knowledge_base_id,
        active_note_id,
        active_document_id,
        active_session_id: String::new(),
    };
    snapshot.sessions = load_sessions_for_snapshot(app, &snapshot)?;

    snapshot.active_session_id = snapshot
        .sessions
        .iter()
        .find(|session| {
            session
                .knowledge_base_ids
                .iter()
                .any(|knowledge_base_id| knowledge_base_id == &snapshot.active_knowledge_base_id)
        })
        .map(|session| session.id.clone())
        .unwrap_or_default();

    Ok(snapshot)
}

/** 构造空编辑器会话，用于首次启动或历史记录不可用时保持编辑区空白。 */
pub(crate) fn empty_workspace_editor_state(
    active_knowledge_base_id: String,
) -> WorkspaceEditorState {
    WorkspaceEditorState {
        active_knowledge_base_id,
        open_tabs: Vec::new(),
        active_tab: None,
        updated_at: format_local_datetime(),
    }
}

/** 判断标签是否引用当前扫描快照中仍可访问的同类型文件。 */
pub(crate) fn workspace_editor_tab_exists(
    snapshot: &WorkspaceSnapshot,
    tab: &WorkspaceEditorTab,
) -> bool {
    match tab.kind.as_str() {
        "note" => snapshot.notes.iter().any(|note| note.id == tab.id),
        "document" => snapshot
            .documents
            .iter()
            .any(|document| document.id == tab.id),
        _ => false,
    }
}

/** 过滤失效、重复或类型非法的编辑器标签，并修正活动知识库与焦点。 */
pub fn normalize_workspace_editor_state(
    snapshot: &WorkspaceSnapshot,
    mut state: WorkspaceEditorState,
) -> WorkspaceEditorState {
    let mut seen_tabs = HashSet::new();

    // 保留原始标签顺序，IDE 式恢复依赖该顺序；重复项只保留首次打开的位置。
    state.open_tabs.retain(|tab| {
        workspace_editor_tab_exists(snapshot, tab)
            && seen_tabs.insert((tab.kind.clone(), tab.id.clone()))
    });

    if !snapshot
        .knowledge_bases
        .iter()
        .any(|knowledge_base| knowledge_base.id == state.active_knowledge_base_id)
    {
        state.active_knowledge_base_id = snapshot.active_knowledge_base_id.clone();
    }

    // 活动项必须同时有效且已经在打开标签中；否则保持编辑区空白，而非回退到首文件。
    state.active_tab = state
        .active_tab
        .filter(|active_tab| state.open_tabs.iter().any(|tab| tab == active_tab));
    state
}

/** 读取 SQLite 中的原始编辑器会话；损坏的历史 JSON 不应阻断应用启动。 */
pub fn load_workspace_editor_state(
    app: &AppHandle,
) -> Result<Option<WorkspaceEditorState>, String> {
    let connection = open_database(app)?;
    let payload_json = connection
        .query_row(
            "SELECT payload_json FROM workspace_editor_state WHERE singleton_id = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("无法读取编辑器会话状态：{error}"))?;

    let Some(payload_json) = payload_json else {
        return Ok(None);
    };

    match serde_json::from_str::<WorkspaceEditorState>(&payload_json) {
        Ok(state) => Ok(Some(state)),
        Err(error) => {
            // 不记录 JSON 内容、路径或标题，避免损坏数据泄露本地文件信息。
            log::warn!(
                target: "workspace_session",
                "忽略无法解析的编辑器会话状态：error_kind=json_deserialize"
            );
            let _ = error;
            Ok(None)
        }
    }
}

/** 扫描完成后组装启动状态，并按当前文件系统过滤编辑器会话中的失效引用。 */
pub fn load_workspace_bootstrap_state(app: &AppHandle) -> Result<WorkspaceBootstrapState, String> {
    let snapshot = load_workspace_snapshot(app)?;
    let editor_state = load_workspace_editor_state(app)?
        .map(|state| normalize_workspace_editor_state(&snapshot, state))
        .unwrap_or_else(|| empty_workspace_editor_state(snapshot.active_knowledge_base_id.clone()));

    log::info!(
        target: "workspace_session",
        "恢复编辑器会话：open_tab_count={} has_active_tab={} restored={}",
        editor_state.open_tabs.len(),
        editor_state.active_tab.is_some(),
        !editor_state.open_tabs.is_empty() || editor_state.active_tab.is_some()
    );

    Ok(WorkspaceBootstrapState {
        snapshot,
        editor_state,
    })
}

/** 保存编辑器会话到 SQLite 单例记录；时间戳由后端统一生成，避免客户端伪造。 */
pub fn save_workspace_editor_state(
    app: &AppHandle,
    mut state: WorkspaceEditorState,
) -> Result<WorkspaceEditorState, String> {
    let mut seen_tabs = HashSet::new();
    state.open_tabs.retain(|tab| {
        matches!(tab.kind.as_str(), "note" | "document")
            && !tab.id.trim().is_empty()
            && seen_tabs.insert((tab.kind.clone(), tab.id.clone()))
    });
    state.active_tab = state
        .active_tab
        .filter(|active_tab| state.open_tabs.iter().any(|tab| tab == active_tab));
    state.updated_at = format_local_datetime();

    let payload_json = serde_json::to_string(&state)
        .map_err(|error| format!("无法序列化编辑器会话状态：{error}"))?;
    let connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    connection
        .execute(
            "INSERT INTO workspace_editor_state (singleton_id, payload_json, updated_at) VALUES (1, ?1, ?2) \
             ON CONFLICT(singleton_id) DO UPDATE SET payload_json = excluded.payload_json, updated_at = excluded.updated_at",
            params![payload_json, state.updated_at],
        )
        .map_err(|error| format!("无法保存编辑器会话状态：{error}"))?;

    log::debug!(
        target: "workspace_session",
        "已保存编辑器会话：open_tab_count={} has_active_tab={}",
        state.open_tabs.len(),
        state.active_tab.is_some()
    );
    Ok(state)
}

/** 使用 SQLite/FTS5 索引检索会话允许范围内的笔记，失败时由 Agent 层决定是否降级。 */
pub fn search_notes(
    app: &AppHandle,
    snapshot: &WorkspaceSnapshot,
    knowledge_base_ids: &[String],
    prompt: &str,
    limit: usize,
) -> Result<Vec<crate::domain::Citation>, String> {
    let limit = limit.max(1);
    let selected_ids: HashSet<&str> = knowledge_base_ids.iter().map(String::as_str).collect();

    if selected_ids.is_empty() {
        return Ok(Vec::new());
    }

    let mut citations = search_note_fts(app, snapshot, &selected_ids, prompt)?;
    let fallback_citations = search_snapshot_notes(snapshot, &selected_ids, prompt);
    let mut seen_note_ids: HashSet<String> = citations
        .iter()
        .map(|citation| citation.note_id.clone())
        .collect();

    // FTS5 对中文长句可能命中较少，补充快照子串检索保证首版中文体验可用。
    for citation in fallback_citations {
        if seen_note_ids.insert(citation.note_id.clone()) {
            citations.push(citation);
        }

        if citations.len() >= limit {
            break;
        }
    }

    citations.sort_by(|left, right| right.score.total_cmp(&left.score));
    citations.truncate(limit);

    Ok(citations)
}

/** 执行 FTS5 查询，并把索引结果转换成 Agent 可展示的引用来源。 */
pub(crate) fn search_note_fts(
    app: &AppHandle,
    snapshot: &WorkspaceSnapshot,
    selected_ids: &HashSet<&str>,
    prompt: &str,
) -> Result<Vec<crate::domain::Citation>, String> {
    let fts_terms = build_fts_terms(prompt);

    if fts_terms.is_empty() {
        return Ok(Vec::new());
    }

    let connection = open_database(app)?;
    let fts_query = fts_terms
        .iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let mut statement = connection
        .prepare(
            "SELECT note_id, knowledge_base_id, title, path, snippet(note_fts, 4, '', '', '...', 32), bm25(note_fts)
             FROM note_fts
             WHERE note_fts MATCH ?1
             ORDER BY bm25(note_fts)
             LIMIT 16",
        )
        .map_err(|error| format!("无法准备 FTS 检索：{error}"))?;
    let rows = statement
        .query_map(params![fts_query], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, f64>(5)?,
            ))
        })
        .map_err(|error| format!("无法执行 FTS 检索：{error}"))?;
    let mut citations = Vec::new();

    for row in rows {
        let (note_id, knowledge_base_id, title, path, snippet, rank) =
            row.map_err(|error| format!("无法读取 FTS 命中结果：{error}"))?;

        // 会话 scope 是工具权限边界，FTS 命中后仍要按本轮允许知识库过滤。
        if !selected_ids.contains(knowledge_base_id.as_str()) {
            continue;
        }
        if crate::storage::is_root_project_instruction_path(&path) {
            continue;
        }

        if let Some(knowledge_base) = snapshot
            .knowledge_bases
            .iter()
            .find(|item| item.id == knowledge_base_id)
        {
            citations.push(crate::domain::Citation {
                knowledge_base_id,
                knowledge_base_name: knowledge_base.name.clone(),
                note_id,
                title,
                path,
                snippet,
                score: 1.0 / (1.0 + rank.abs()),
                location: None,
            });
        }
    }

    Ok(citations)
}

/** 将用户输入拆成 FTS5 查询词，避免把标点和空白带进 MATCH 语法。 */
pub(crate) fn build_fts_terms(prompt: &str) -> Vec<String> {
    prompt
        .split(|character: char| {
            character.is_whitespace()
                || character.is_ascii_punctuation()
                || "，。！？；：、（）《》「」".contains(character)
        })
        .map(str::trim)
        .filter(|term| term.chars().count() > 1)
        .take(8)
        .map(str::to_owned)
        .collect()
}

/** 快照级子串检索，作为 FTS5 无命中或中文分词不足时的本地降级方案。 */
pub(crate) fn search_snapshot_notes(
    snapshot: &WorkspaceSnapshot,
    selected_ids: &HashSet<&str>,
    prompt: &str,
) -> Vec<crate::domain::Citation> {
    let prompt_terms: Vec<String> = prompt
        .split_whitespace()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase)
        .collect();
    let mut citations: Vec<crate::domain::Citation> = snapshot
        .notes
        .iter()
        .filter(|note| selected_ids.contains(note.knowledge_base_id.as_str()))
        .filter(|note| !crate::storage::is_root_project_instruction_path(&note.path))
        .filter_map(|note| {
            let searchable_text = format!(
                "{} {} {} {}",
                note.title,
                note.path,
                note.tags.join(" "),
                note.content
            )
            .to_lowercase();
            let term_score = prompt_terms
                .iter()
                .filter(|term| searchable_text.contains(term.as_str()))
                .count() as f64;
            let fallback_score = ["写入", "隐私", "检索", "agent", "本地"]
                .iter()
                .filter(|term| searchable_text.contains(*term))
                .count() as f64;
            let score = term_score * 2.0 + fallback_score;

            if score <= 0.0 {
                return None;
            }

            let knowledge_base = snapshot
                .knowledge_bases
                .iter()
                .find(|item| item.id == note.knowledge_base_id)?;

            Some(crate::domain::Citation {
                knowledge_base_id: note.knowledge_base_id.clone(),
                knowledge_base_name: knowledge_base.name.clone(),
                note_id: note.id.clone(),
                title: note.title.clone(),
                path: note.path.clone(),
                snippet: extract_snippet(&note.content, prompt),
                score,
                location: None,
            })
        })
        .collect();

    citations.sort_by(|left, right| right.score.total_cmp(&left.score));
    citations.truncate(4);
    citations
}

/** 从 Markdown 内容中提取引用片段。 */
pub(crate) fn extract_snippet(content: &str, prompt: &str) -> String {
    let prompt_terms: Vec<&str> = prompt
        .split_whitespace()
        .filter(|term| !term.is_empty())
        .collect();

    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .find(|line| prompt_terms.iter().any(|term| line.contains(term)))
        .or_else(|| {
            content
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty() && !line.starts_with('#'))
        })
        .unwrap_or("命中该笔记，但暂无可展示片段。")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::{apply_index_to_transaction, IndexMode};
    use crate::domain::{KnowledgeBase, Note, WorkspaceSnapshot};
    use crate::storage::{ensure_database_schema, hash_content};
    use rusqlite::Connection;
    use tempfile::tempdir;

    fn test_note(id: &str, content: &str) -> Note {
        Note {
            id: id.to_owned(),
            knowledge_base_id: "kb-a".to_owned(),
            title: format!("笔记 {id}"),
            path: format!("{id}.md"),
            content: content.to_owned(),
            tags: Vec::new(),
            updated_at: "刚刚".to_owned(),
            backlinks: Vec::new(),
            content_hash: hash_content(content),
        }
    }

    fn test_snapshot(notes: Vec<Note>) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            knowledge_bases: vec![KnowledgeBase {
                id: "kb-a".to_owned(),
                name: "测试知识库".to_owned(),
                path: "/redacted".to_owned(),
                description: String::new(),
                status: "ready".to_owned(),
                note_count: notes.len(),
                document_count: 0,
                updated_at: "刚刚".to_owned(),
                is_default: true,
                semantic_index_enabled: false,
                scan_report: None,
            }],
            folders: Vec::new(),
            notes,
            documents: Vec::new(),
            sessions: Vec::new(),
            active_knowledge_base_id: "kb-a".to_owned(),
            active_note_id: String::new(),
            active_document_id: String::new(),
            active_session_id: String::new(),
        }
    }

    fn note_ids(connection: &Connection) -> Vec<String> {
        let mut statement = connection
            .prepare("SELECT id FROM notes ORDER BY id")
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect()
    }

    /** 过期回合快照做 upsert 时不得删掉其它会话已经写入的笔记。 */
    #[test]
    fn stale_turn_snapshot_does_not_wipe_other_session_notes_from_fts() {
        let dir = tempdir().unwrap();
        let database_path = dir.path().join("index.sqlite3");
        let mut connection = Connection::open(&database_path).unwrap();
        ensure_database_schema(&connection, &database_path).unwrap();

        let first = test_snapshot(vec![test_note("note-a", "会话 A 旧笔记")]);
        let transaction = connection.transaction().unwrap();
        apply_index_to_transaction(&transaction, &first, IndexMode::ReplaceAll).unwrap();
        transaction.commit().unwrap();

        let both = test_snapshot(vec![
            test_note("note-a", "会话 A 旧笔记"),
            test_note("note-b", "会话 B 新笔记"),
        ]);
        let transaction = connection.transaction().unwrap();
        apply_index_to_transaction(&transaction, &both, IndexMode::Upsert).unwrap();
        transaction.commit().unwrap();
        assert_eq!(note_ids(&connection), vec!["note-a", "note-b"]);

        let stale = test_snapshot(vec![test_note("note-a", "会话 A 旧笔记")]);
        let transaction = connection.transaction().unwrap();
        apply_index_to_transaction(&transaction, &stale, IndexMode::Upsert).unwrap();
        transaction.commit().unwrap();
        assert_eq!(note_ids(&connection), vec!["note-a", "note-b"]);

        let fts_count: i64 = connection
            .query_row(
                "SELECT count(*) FROM note_fts WHERE note_id = 'note-b'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fts_count, 1);
    }

    /** 删除单篇笔记索引时不得动其它笔记。 */
    #[test]
    fn remove_note_ids_does_not_wipe_other_notes() {
        let dir = tempdir().unwrap();
        let database_path = dir.path().join("index-remove.sqlite3");
        let mut connection = Connection::open(&database_path).unwrap();
        ensure_database_schema(&connection, &database_path).unwrap();

        let snapshot = test_snapshot(vec![
            test_note("note-a", "保留"),
            test_note("note-b", "删除"),
        ]);
        let transaction = connection.transaction().unwrap();
        apply_index_to_transaction(&transaction, &snapshot, IndexMode::ReplaceAll).unwrap();
        transaction.commit().unwrap();

        let transaction = connection.transaction().unwrap();
        super::remove_note_ids_from_transaction(&transaction, &["note-b".to_owned()]).unwrap();
        transaction.commit().unwrap();
        assert_eq!(note_ids(&connection), vec!["note-a"]);
    }

    /** 重扫一个知识库不得清掉另一个知识库的笔记。 */
    #[test]
    fn reindex_one_knowledge_base_keeps_other_kb_notes() {
        let dir = tempdir().unwrap();
        let database_path = dir.path().join("index-reindex.sqlite3");
        let mut connection = Connection::open(&database_path).unwrap();
        ensure_database_schema(&connection, &database_path).unwrap();

        let mut snapshot = test_snapshot(vec![test_note("note-a", "库 A")]);
        snapshot.knowledge_bases.push(crate::domain::KnowledgeBase {
            id: "kb-b".to_owned(),
            name: "知识库 B".to_owned(),
            path: "/redacted-b".to_owned(),
            description: String::new(),
            status: "ready".to_owned(),
            note_count: 1,
            document_count: 0,
            updated_at: "刚刚".to_owned(),
            is_default: false,
            semantic_index_enabled: false,
            scan_report: None,
        });
        snapshot.notes.push(Note {
            id: "note-b".to_owned(),
            knowledge_base_id: "kb-b".to_owned(),
            title: "库 B 笔记".to_owned(),
            path: "b.md".to_owned(),
            content: "库 B".to_owned(),
            tags: Vec::new(),
            updated_at: "刚刚".to_owned(),
            backlinks: Vec::new(),
            content_hash: hash_content("库 B"),
        });
        let transaction = connection.transaction().unwrap();
        apply_index_to_transaction(&transaction, &snapshot, IndexMode::ReplaceAll).unwrap();
        transaction.commit().unwrap();

        snapshot
            .notes
            .retain(|note| note.knowledge_base_id != "kb-a");
        snapshot.notes.insert(0, test_note("note-a2", "库 A 重扫"));
        let transaction = connection.transaction().unwrap();
        super::reindex_knowledge_base_notes_in_transaction(&transaction, &snapshot, "kb-a")
            .unwrap();
        transaction.commit().unwrap();
        assert_eq!(note_ids(&connection), vec!["note-a2", "note-b"]);
    }
}
