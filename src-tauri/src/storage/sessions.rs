use super::*;

pub(crate) fn is_created_at_placeholder(created_at: &str) -> bool {
    let trimmed_created_at = created_at.trim();

    trimmed_created_at.is_empty() || trimmed_created_at == "刚刚"
}

/** 从前端 createLocalId 生成的 session ID 中提取 Date.now 毫秒时间戳。 */
pub(crate) fn timestamp_millis_from_session_id(session_id: &str) -> Option<i64> {
    session_id
        .split('-')
        .filter_map(|part| part.parse::<i64>().ok())
        // 只接受常见 Unix 毫秒时间戳范围，避免把会话类型或随机片段误当时间。
        .find(|timestamp_millis| {
            *timestamp_millis >= 946_684_800_000 && *timestamp_millis <= 4_102_444_800_000
        })
}

/** 从前端 createLocalId 生成的 session ID 中恢复可展示创建时间。 */
pub(crate) fn created_at_from_session_id(session_id: &str) -> Option<String> {
    timestamp_millis_from_session_id(session_id).and_then(format_local_datetime_from_millis)
}

/** 归一化会话创建时间，避免历史列表永久显示旧版“刚刚”占位值。 */
pub(crate) fn normalize_session_created_at(session: &mut AgentSession) {
    if !is_created_at_placeholder(&session.created_at) {
        return;
    }

    session.created_at = created_at_from_session_id(&session.id)
        .or_else(|| {
            // 如果 updated_at 已经是明确时间，用它作为旧记录迁移的次优来源。
            (!is_created_at_placeholder(&session.updated_at)).then(|| session.updated_at.clone())
        })
        .unwrap_or_else(format_local_datetime);
}

/** 将本地展示时间字符串（%Y/%m/%d %H:%M）解析为毫秒时间戳，无法解析时返回 None。 */
pub(crate) fn parse_local_datetime_millis(value: &str) -> Option<i64> {
    NaiveDateTime::parse_from_str(value.trim(), "%Y/%m/%d %H:%M")
        .ok()
        .and_then(|naive| {
            // 按本地时区解释 UI 展示时间，保证与 format_local_datetime 的来源一致。
            Local
                .from_local_datetime(&naive)
                .single()
                .map(|datetime| datetime.timestamp_millis())
        })
}

/**
 * 将会话“最后使用时间”转换为可排序时间戳。
 *
 * `updated_at` 在前端每次发消息或改 diff 时刷新为 `%Y/%m/%d %H:%M`，因此优先解析它；
 * 旧版记录仍是“刚刚”占位值时，退回从会话 ID 里提取创建毫秒时间戳，避免在派生字段全空时整体失序。
 */
pub(crate) fn session_updated_sort_key(session: &AgentSession) -> i64 {
    if !is_created_at_placeholder(&session.updated_at) {
        if let Some(updated_at_millis) = parse_local_datetime_millis(&session.updated_at) {
            return updated_at_millis;
        }
    }

    timestamp_millis_from_session_id(&session.id)
        .or_else(|| parse_local_datetime_millis(&session.created_at))
        .unwrap_or(0)
}

/** 按“最后使用时间”倒序整理会话历史，相同时间再回落到创建时间，保持稳定。 */
pub(crate) fn sort_sessions_by_updated_at_desc(sessions: &mut [AgentSession]) {
    sessions.sort_by(|left, right| {
        session_updated_sort_key(right)
            .cmp(&session_updated_sort_key(left))
            .then_with(|| right.updated_at.cmp(&left.updated_at))
            .then_with(|| right.created_at.cmp(&left.created_at))
    });
}

pub(crate) fn load_deleted_sessions_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<Vec<AgentSession>, String> {
    let mut statement = transaction
        .prepare("SELECT payload_json FROM agent_sessions ORDER BY rowid")
        .map_err(|error| format!("无法准备已删除会话读取：{error}"))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("无法查询已删除会话：{error}"))?;
    let mut sessions = Vec::new();

    for row in rows {
        let payload_json = row.map_err(|error| format!("无法读取已删除会话记录：{error}"))?;
        let mut session: AgentSession = serde_json::from_str(&payload_json)
            .map_err(|error| format!("无法解析已删除会话记录：{error}"))?;

        normalize_session_created_at(&mut session);

        if session.deleted_at.is_some() {
            sessions.push(session);
        }
    }

    Ok(sessions)
}

/** 在已有 SQLite 事务中写入单条会话记录，payload_json 保留完整上下文。 */
pub(crate) fn persist_session_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    session: &AgentSession,
) -> Result<(), String> {
    let mut session = session.clone();

    normalize_session_created_at(&mut session);

    let payload_json =
        serde_json::to_string(&session).map_err(|error| format!("无法序列化会话：{error}"))?;

    transaction
        .execute(
            "INSERT OR REPLACE INTO agent_sessions
             (id, type, title, active_note_id, created_at, updated_at, payload_json)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                &session.id,
                &session.r#type,
                &session.title,
                session.active_note_id.as_deref(),
                &session.created_at,
                &session.updated_at,
                payload_json
            ],
        )
        .map_err(|error| format!("无法持久化会话：{error}"))?;

    Ok(())
}

/** 在已有 SQLite 事务中持久化当前快照的完整可见会话列表，同时保留逻辑删除记录。 */
pub(crate) fn persist_sessions_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    snapshot: &WorkspaceSnapshot,
) -> Result<(), String> {
    let deleted_sessions = load_deleted_sessions_in_transaction(transaction)?;
    let snapshot_session_ids: HashSet<&str> = snapshot
        .sessions
        .iter()
        .map(|session| session.id.as_str())
        .collect();

    transaction
        .execute("DELETE FROM agent_sessions", [])
        .map_err(|error| format!("无法清理会话表：{error}"))?;

    for session in deleted_sessions
        .iter()
        .filter(|session| !snapshot_session_ids.contains(session.id.as_str()))
    {
        persist_session_in_transaction(transaction, session)?;
    }

    for session in &snapshot.sessions {
        persist_session_in_transaction(transaction, session)?;
    }

    cleanup_orphan_session_transcripts(
        transaction,
        snapshot
            .sessions
            .iter()
            .map(|session| session.id.as_str())
            .chain(deleted_sessions.iter().map(|session| session.id.as_str())),
    )?;

    Ok(())
}

/** 删除已不存在会话的模型 transcript，避免 UI 会话列表收缩后残留大 JSON。 */
pub(crate) fn cleanup_orphan_session_transcripts<'a>(
    transaction: &rusqlite::Transaction<'_>,
    keep_session_ids: impl IntoIterator<Item = &'a str>,
) -> Result<(), String> {
    let keep_session_ids: HashSet<&str> = keep_session_ids.into_iter().collect();
    let mut statement = transaction
        .prepare("SELECT session_id FROM agent_session_transcripts")
        .map_err(|error| format!("无法准备 transcript 清理：{error}"))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("无法查询 transcript 列表：{error}"))?;
    let mut orphan_ids = Vec::new();

    for row in rows {
        let session_id = row.map_err(|error| format!("无法读取 transcript 会话 ID：{error}"))?;
        if !keep_session_ids.contains(session_id.as_str()) {
            orphan_ids.push(session_id);
        }
    }
    drop(statement);

    for session_id in orphan_ids {
        transaction
            .execute(
                "DELETE FROM agent_session_transcripts WHERE session_id = ?1",
                params![session_id],
            )
            .map_err(|error| format!("无法删除过期 transcript：{error}"))?;
    }

    Ok(())
}

/** 读取会话级模型 transcript；没有记录时返回 None，由 runtime seed。 */
pub fn load_agent_session_transcript(
    app: &AppHandle,
    session_id: &str,
) -> Result<Option<Vec<Value>>, String> {
    let connection = open_database(app)?;
    load_agent_session_transcript_from_connection(&connection, session_id)
}

/** 保存会话级模型 transcript，不写入前端会话 payload。 */
pub fn save_agent_session_transcript(
    app: &AppHandle,
    session_id: &str,
    messages: &[Value],
) -> Result<(), String> {
    let mut connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| format!("无法启动 transcript 事务：{error}"))?;
    persist_agent_session_transcript(&transaction, session_id, messages)?;
    transaction
        .commit()
        .map_err(|error| format!("无法提交 transcript 事务：{error}"))
}

/** 从已打开的连接读取 transcript，供 runtime 和单测复用。 */
pub(crate) fn load_agent_session_transcript_from_connection(
    connection: &Connection,
    session_id: &str,
) -> Result<Option<Vec<Value>>, String> {
    let payload = connection
        .query_row(
            "SELECT payload_json FROM agent_session_transcripts WHERE session_id = ?1",
            params![session_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("无法读取会话 transcript：{error}"))?;

    payload
        .map(|payload| {
            serde_json::from_str(&payload)
                .map_err(|error| format!("无法解析会话 transcript：{error}"))
        })
        .transpose()
}

/** 在已有连接或事务中写入 transcript。 */
pub(crate) fn persist_agent_session_transcript(
    connection: &Connection,
    session_id: &str,
    messages: &[Value],
) -> Result<(), String> {
    let payload_json = serde_json::to_string(messages)
        .map_err(|error| format!("无法序列化会话 transcript：{error}"))?;

    connection
        .execute(
            "INSERT OR REPLACE INTO agent_session_transcripts
             (session_id, payload_json, updated_at)
             VALUES (?1, ?2, ?3)",
            params![session_id, payload_json, format_local_datetime()],
        )
        .map_err(|error| format!("无法持久化会话 transcript：{error}"))?;

    Ok(())
}

/** 保存当前快照的完整会话列表，供前端会话操作和 Agent loop 后同步状态。 */
pub fn save_sessions(app: &AppHandle, snapshot: &WorkspaceSnapshot) -> Result<(), String> {
    let mut connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| format!("无法启动会话事务：{error}"))?;

    persist_sessions_in_transaction(&transaction, snapshot)?;
    transaction
        .commit()
        .map_err(|error| format!("无法提交会话事务：{error}"))
}

/** 保存单个会话，并返回已经写入快照的下一版工作台状态。 */
pub fn save_session(
    app: &AppHandle,
    mut snapshot: WorkspaceSnapshot,
    mut session: AgentSession,
) -> Result<WorkspaceSnapshot, String> {
    if let Some(existing) = load_persisted_session(app, &session.id)? {
        // 普通会话保存不能改写高权限审批载荷；仅允许按稳定 operation id 更新勾选状态。
        session.knowledge_base_ids = existing.knowledge_base_ids;
        session.im_identity = existing.im_identity;
        session.pending_execution = existing.pending_execution;
        session.pending_change_set = match (existing.pending_change_set, session.pending_change_set)
        {
            (Some(mut trusted), Some(draft)) if trusted.id == draft.id => {
                let selected_by_id = draft
                    .operations
                    .into_iter()
                    .map(|operation| (operation.id, operation.selected))
                    .collect::<HashMap<_, _>>();
                for operation in &mut trusted.operations {
                    if let Some(selected) = selected_by_id.get(&operation.id) {
                        operation.selected = *selected;
                    }
                }
                Some(trusted)
            }
            (trusted, _) => trusted,
        };
    }

    if let Some(index) = snapshot
        .sessions
        .iter()
        .position(|existing_session| existing_session.id == session.id)
    {
        snapshot.sessions[index] = session.clone();
    } else {
        snapshot.sessions.insert(0, session.clone());
    }

    snapshot.active_session_id = session.id;
    normalize_sessions_for_snapshot(&mut snapshot);
    save_sessions(app, &snapshot)?;

    Ok(snapshot)
}

/** 按 ID 读取未经前端覆盖的持久化会话，供审批状态保护复用。 */
pub(crate) fn load_persisted_session(
    app: &AppHandle,
    session_id: &str,
) -> Result<Option<AgentSession>, String> {
    let connection = open_database(app)?;
    let payload = connection
        .query_row(
            "SELECT payload_json FROM agent_sessions WHERE id = ?1",
            params![session_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("无法读取持久化会话：{error}"))?;

    payload
        .map(|payload| {
            serde_json::from_str(&payload).map_err(|error| format!("无法解析持久化会话：{error}"))
        })
        .transpose()
}

/** 逻辑删除单个会话，保留 payload 历史但从返回快照和普通读取中隐藏。 */
pub fn delete_session(
    app: &AppHandle,
    mut snapshot: WorkspaceSnapshot,
    session_id: &str,
) -> Result<WorkspaceSnapshot, String> {
    normalize_sessions_for_snapshot(&mut snapshot);
    let session_index = snapshot
        .sessions
        .iter()
        .position(|session| session.id == session_id)
        .ok_or_else(|| "找不到要删除的会话".to_owned())?;
    let mut deleted_session = snapshot.sessions.remove(session_index);

    deleted_session.deleted_at = Some("刚刚".to_owned());
    deleted_session.updated_at = "刚刚".to_owned();

    if snapshot.active_session_id == session_id
        || !snapshot
            .sessions
            .iter()
            .any(|session| session.id == snapshot.active_session_id)
    {
        ensure_visible_session_after_delete(&mut snapshot);
    }

    let mut persisted_snapshot = snapshot.clone();

    // 持久化时带上被删除会话，UI 返回值仍只包含未删除会话。
    persisted_snapshot.sessions.insert(0, deleted_session);
    save_sessions(app, &persisted_snapshot)?;

    Ok(snapshot)
}

/** 删除当前会话后只在当前知识库内选择已有会话；没有会话时保持空状态。 */
pub(crate) fn ensure_visible_session_after_delete(snapshot: &mut WorkspaceSnapshot) {
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
}

/** 从 SQLite 读取并按当前知识库和笔记快照清理后的会话列表。 */
pub fn load_sessions_for_snapshot(
    app: &AppHandle,
    snapshot: &WorkspaceSnapshot,
) -> Result<Vec<AgentSession>, String> {
    let connection = open_database(app)?;
    let mut statement = connection
        .prepare("SELECT payload_json FROM agent_sessions")
        .map_err(|error| format!("无法准备会话读取：{error}"))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("无法查询会话列表：{error}"))?;
    let mut sessions = Vec::new();

    for row in rows {
        let payload_json = row.map_err(|error| format!("无法读取会话记录：{error}"))?;
        let mut session: AgentSession = serde_json::from_str(&payload_json)
            .map_err(|error| format!("无法解析会话记录：{error}"))?;

        normalize_session_created_at(&mut session);

        if session.deleted_at.is_some() {
            continue;
        }

        if normalize_session_for_snapshot(&mut session, snapshot) {
            sessions.push(session);
        }
    }

    sort_sessions_by_updated_at_desc(&mut sessions);

    Ok(sessions)
}

/** 懒迁移旧版“飞书会话”，从映射表恢复来源和聊天类型，并从历史消息补齐展示摘要。 */
pub fn migrate_legacy_im_session_identities(
    app: &AppHandle,
    sessions: &mut [AgentSession],
) -> Result<Vec<AgentSession>, String> {
    let connection = open_database(app)?;
    let mut migrated_sessions = Vec::new();

    for session in sessions.iter_mut().filter(|session| {
        session.im_identity.is_none() && session.title == "飞书会话" && session.deleted_at.is_none()
    }) {
        let channel_key = connection
            .query_row(
                "SELECT channel_key FROM im_session_mappings WHERE session_id = ?1 LIMIT 1",
                params![session.id],
                |row| row.get::<_, String>(0),
            )
            .ok();
        let user_messages = session
            .messages
            .iter()
            .filter(|message| message.role == "user")
            .map(|message| message.content.as_str())
            .filter(|content| !content.trim().is_empty())
            .collect::<Vec<_>>();
        let initial_message = user_messages.first().copied().unwrap_or_default();
        let last_message = user_messages.last().copied().unwrap_or(initial_message);
        let identity = crate::im::build_im_session_identity_from_channel_key(
            channel_key.as_deref(),
            initial_message,
            last_message,
        );

        session.title = crate::im::format_im_session_title(&identity);
        session.im_identity = Some(identity);
        migrated_sessions.push(session.clone());
    }

    Ok(migrated_sessions)
}

/** 单独回写被懒迁移的会话，避免一次读取覆盖并发更新的其他会话记录。 */
pub fn save_session_records(app: &AppHandle, sessions: &[AgentSession]) -> Result<(), String> {
    if sessions.is_empty() {
        return Ok(());
    }

    let mut connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| format!("无法启动 IM 会话迁移事务：{error}"))?;

    for session in sessions {
        persist_session_in_transaction(&transaction, session)?;
    }

    transaction
        .commit()
        .map_err(|error| format!("无法提交 IM 会话迁移事务：{error}"))
}

/** 更新会话知识库范围，当前激活知识库由后端强制保留。 */
pub fn update_session_scope(
    app: &AppHandle,
    mut snapshot: WorkspaceSnapshot,
    session_id: &str,
    requested_knowledge_base_ids: Vec<String>,
    active_knowledge_base_id: &str,
) -> Result<WorkspaceSnapshot, String> {
    let active_id = if snapshot
        .knowledge_bases
        .iter()
        .any(|knowledge_base| knowledge_base.id == active_knowledge_base_id)
    {
        active_knowledge_base_id
    } else {
        snapshot.active_knowledge_base_id.as_str()
    };
    let next_ids = ordered_valid_scope_ids(&snapshot, &requested_knowledge_base_ids, active_id);
    let session = snapshot
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .ok_or_else(|| "找不到要更新范围的会话".to_owned())?;

    session.knowledge_base_ids = next_ids;
    session.updated_at = "刚刚".to_owned();
    normalize_sessions_for_snapshot(&mut snapshot);
    save_sessions(app, &snapshot)?;

    Ok(snapshot)
}

/** 恢复历史会话绑定的知识库；文件焦点只在会话仍有有效笔记引用时同步。 */
pub fn restore_session_context(
    app: &AppHandle,
    mut snapshot: WorkspaceSnapshot,
    session_id: &str,
) -> Result<WorkspaceSnapshot, String> {
    normalize_sessions_for_snapshot(&mut snapshot);
    let session = snapshot
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .cloned()
        .ok_or_else(|| "找不到要恢复的会话".to_owned())?;
    let next_knowledge_base_id = session
        .knowledge_base_ids
        .iter()
        .find(|knowledge_base_id| {
            snapshot
                .knowledge_bases
                .iter()
                .any(|knowledge_base| &knowledge_base.id == *knowledge_base_id)
        })
        .cloned()
        .or_else(|| {
            snapshot
                .knowledge_bases
                .first()
                .map(|knowledge_base| knowledge_base.id.clone())
        })
        .unwrap_or_default();
    let next_note_id = resolve_session_note_id(
        &snapshot,
        session.active_note_id.as_deref(),
        &next_knowledge_base_id,
    );
    let should_keep_current_file = snapshot.active_knowledge_base_id == next_knowledge_base_id;

    snapshot.active_session_id = session.id;
    snapshot.active_knowledge_base_id = next_knowledge_base_id;
    if !next_note_id.is_empty() {
        snapshot.active_note_id = next_note_id;
        snapshot.active_document_id.clear();
    } else if !should_keep_current_file {
        snapshot.active_note_id = snapshot
            .notes
            .iter()
            .find(|note| note.knowledge_base_id == snapshot.active_knowledge_base_id)
            .map(|note| note.id.clone())
            .unwrap_or_default();
        snapshot.active_document_id = resolve_fallback_document_id(
            &snapshot,
            &snapshot.active_knowledge_base_id,
            &snapshot.active_note_id,
        );
    }
    save_sessions(app, &snapshot)?;

    Ok(snapshot)
}

/** 查询 IM 对话已绑定的 AgentSession；会话删除或失效时由调用方重新创建。 */
pub fn load_im_session_mapping(
    app: &AppHandle,
    channel_key: &str,
) -> Result<Option<String>, String> {
    let connection = open_database(app)?;
    let session_id = connection
        .query_row(
            "SELECT session_id FROM im_session_mappings WHERE channel_key = ?1",
            params![channel_key],
            |row| row.get::<_, String>(0),
        )
        .ok();

    Ok(session_id)
}

/** 保存 IM 对话到 AgentSession 的稳定映射，避免重启后丢失上下文。 */
pub fn save_im_session_mapping(
    app: &AppHandle,
    channel_key: &str,
    session_id: &str,
) -> Result<(), String> {
    let connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;

    connection
        .execute(
            "INSERT OR REPLACE INTO im_session_mappings (channel_key, session_id, updated_at) VALUES (?1, ?2, ?3)",
            params![channel_key, session_id, format_local_datetime()],
        )
        .map_err(|error| format!("无法保存 IM 会话映射：{error}"))?;

    Ok(())
}

/** 根据会话里的 note 引用恢复同知识库 Markdown；无有效引用时不再默认绑定第一篇文档。 */
pub(crate) fn resolve_session_note_id(
    snapshot: &WorkspaceSnapshot,
    session_note_id: Option<&str>,
    knowledge_base_id: &str,
) -> String {
    if let Some(note_id) = session_note_id {
        if snapshot
            .notes
            .iter()
            .any(|note| note.id == note_id && note.knowledge_base_id == knowledge_base_id)
        {
            return note_id.to_owned();
        }
    }

    String::new()
}

/** 当当前知识库没有可激活 Markdown 时，选择第一个普通文档作为中间面板展示对象。 */
pub(crate) fn resolve_fallback_document_id(
    snapshot: &WorkspaceSnapshot,
    knowledge_base_id: &str,
    active_note_id: &str,
) -> String {
    if !active_note_id.is_empty() {
        return String::new();
    }

    snapshot
        .documents
        .iter()
        .find(|document| document.knowledge_base_id == knowledge_base_id)
        .map(|document| document.id.clone())
        .unwrap_or_default()
}

/** 按当前快照清理所有会话，删除已经失去有效知识库范围的会话。 */
pub fn normalize_sessions_for_snapshot(snapshot: &mut WorkspaceSnapshot) {
    let snapshot_view = WorkspaceSnapshot {
        knowledge_bases: snapshot.knowledge_bases.clone(),
        folders: snapshot.folders.clone(),
        notes: snapshot.notes.clone(),
        documents: snapshot.documents.clone(),
        sessions: Vec::new(),
        active_knowledge_base_id: snapshot.active_knowledge_base_id.clone(),
        active_note_id: snapshot.active_note_id.clone(),
        active_document_id: snapshot.active_document_id.clone(),
        active_session_id: snapshot.active_session_id.clone(),
    };

    snapshot
        .sessions
        .retain_mut(|session| normalize_session_for_snapshot(session, &snapshot_view));
    sort_sessions_by_updated_at_desc(&mut snapshot.sessions);
}

/** 清理单个会话引用，返回 false 表示该会话已没有可访问知识库。 */
pub fn normalize_session_for_snapshot(
    session: &mut AgentSession,
    snapshot: &WorkspaceSnapshot,
) -> bool {
    normalize_session_created_at(session);

    if session.im_identity.is_some() {
        // 远程入口无论前端或历史 payload 如何声明，都只能保持基础级别和无待执行任务。
        session.security_level = "basic".to_owned();
        session.pending_execution = None;
        session.pending_change_set = None;
    } else if !matches!(
        session.security_level.as_str(),
        "basic" | "advanced" | "autonomous"
    ) {
        session.security_level = "basic".to_owned();
    }

    if session.deleted_at.is_some() {
        return false;
    }

    let knowledge_base_ids: HashSet<&str> = snapshot
        .knowledge_bases
        .iter()
        .map(|knowledge_base| knowledge_base.id.as_str())
        .collect();
    let note_ids: HashSet<&str> = snapshot.notes.iter().map(|note| note.id.as_str()).collect();

    session
        .knowledge_base_ids
        .retain(|knowledge_base_id| knowledge_base_ids.contains(knowledge_base_id.as_str()));
    session
        .pinned_note_ids
        .retain(|note_id| note_ids.contains(note_id.as_str()));

    if session
        .active_note_id
        .as_ref()
        .is_some_and(|note_id| !note_ids.contains(note_id.as_str()))
    {
        session.active_note_id = None;
    }

    if session
        .pending_change
        .as_ref()
        .and_then(|change| change.note_id.as_ref())
        .is_some_and(|note_id| !note_ids.contains(note_id.as_str()))
    {
        session.pending_change = None;
    }

    !session.knowledge_base_ids.is_empty()
}

/** 根据知识库列表稳定排序范围，并强制保留当前激活知识库。 */
pub(crate) fn ordered_valid_scope_ids(
    snapshot: &WorkspaceSnapshot,
    requested_knowledge_base_ids: &[String],
    active_knowledge_base_id: &str,
) -> Vec<String> {
    let mut selected_ids: HashSet<&str> = requested_knowledge_base_ids
        .iter()
        .map(String::as_str)
        .collect();

    selected_ids.insert(active_knowledge_base_id);

    snapshot
        .knowledge_bases
        .iter()
        .filter(|knowledge_base| selected_ids.contains(knowledge_base.id.as_str()))
        .map(|knowledge_base| knowledge_base.id.clone())
        .collect()
}
