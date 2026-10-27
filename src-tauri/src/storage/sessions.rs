use super::*;

pub(crate) fn is_created_at_placeholder(created_at: &str) -> bool {
    let trimmed_created_at = created_at.trim();

    matches!(trimmed_created_at, "" | "刚刚" | "未保存")
}

/** 从前端 createLocalId 生成的会话或消息 ID 中提取 Date.now 毫秒时间戳。 */
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

/** 恢复旧会话的真实时间；展示、分组、排序和后续持久化统一使用恢复后的值。 */
pub(crate) fn normalize_session_timestamps(session: &mut AgentSession) {
    let missing_created_at = is_created_at_placeholder(&session.created_at);
    let missing_updated_at = is_created_at_placeholder(&session.updated_at);
    if !missing_created_at && !missing_updated_at {
        return;
    }

    // 旧版甚至持久化过 __draft-session__，不能只依赖会话 ID，消息和嵌套轨迹也要参与恢复。
    let mut message_times = Vec::new();
    for message in &session.messages {
        message_times.extend(timestamp_millis_from_session_id(&message.id));
        let mut steps: Vec<_> = message.trace.iter().collect();
        while let Some(step) = steps.pop() {
            message_times.extend(parse_local_datetime_millis(&step.timestamp));
            steps.extend(&step.children);
        }
    }
    let activity_times: Vec<_> = [
        session
            .context_summary
            .as_ref()
            .map(|summary| summary.updated_at.as_str()),
        session
            .context_usage
            .as_ref()
            .map(|usage| usage.recorded_at.as_str()),
        session
            .pending_change_set
            .as_ref()
            .map(|change| change.created_at.as_str()),
    ]
    .into_iter()
    .flatten()
    .filter_map(parse_local_datetime_millis)
    .collect();

    if missing_created_at {
        session.created_at = created_at_from_session_id(&session.id)
            .or_else(|| {
                message_times
                    .iter()
                    .min()
                    .copied()
                    .and_then(format_local_datetime_from_millis)
            })
            .or_else(|| {
                parse_local_datetime_millis(&session.updated_at)
                    .or_else(|| activity_times.iter().min().copied())
                    .and_then(format_local_datetime_from_millis)
            })
            // 没有历史证据时保持未知，不能把每次加载时间伪装成创建时间。
            .unwrap_or_default();
    }
    if missing_updated_at {
        session.updated_at = message_times
            .into_iter()
            .chain(activity_times)
            .chain(parse_local_datetime_millis(&session.created_at))
            .chain(timestamp_millis_from_session_id(&session.id))
            .max()
            .and_then(format_local_datetime_from_millis)
            .unwrap_or_default();
    }
}

/** 兼容历史本地展示时间及带时区的上下文时间，无法解析时返回 None。 */
pub(crate) fn parse_local_datetime_millis(value: &str) -> Option<i64> {
    let value = value.trim();
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|datetime| datetime.timestamp_millis())
        .or_else(|| {
            ["%Y/%m/%d %H:%M", "%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"]
                .iter()
                .find_map(|format| {
                    let naive = NaiveDateTime::parse_from_str(value, format).ok()?;
                    // 按本地时区解释 UI 展示时间，与 format_local_datetime 保持一致。
                    Local
                        .from_local_datetime(&naive)
                        .single()
                        .map(|datetime| datetime.timestamp_millis())
                })
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

#[cfg(test)]
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

        normalize_session_timestamps(&mut session);

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

    normalize_session_timestamps(&mut session);

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

/** 只 upsert 给定会话，不删除表中其它行。Agent 回合和单会话保存必须走这条路径。 */
pub(crate) fn persist_session_records_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    sessions: &[AgentSession],
) -> Result<(), String> {
    for session in sessions {
        persist_session_in_transaction(transaction, session)?;
    }

    Ok(())
}

/** 整表重写路径仅用于测试，证明过期快照会删掉其它会话；生产路径必须 upsert。 */
#[cfg(test)]
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
#[cfg(test)]
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

/** 删除会话级模型 transcript；没有记录时是空操作。 */
pub fn delete_agent_session_transcript(app: &AppHandle, session_id: &str) -> Result<(), String> {
    let mut connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| format!("无法启动 transcript 删除事务：{error}"))?;
    delete_agent_session_transcript_on_connection(&transaction, session_id)?;
    transaction
        .commit()
        .map_err(|error| format!("无法提交 transcript 删除事务：{error}"))
}

/** 在已有连接或事务中删除 transcript。 */
pub(crate) fn delete_agent_session_transcript_on_connection(
    connection: &Connection,
    session_id: &str,
) -> Result<(), String> {
    connection
        .execute(
            "DELETE FROM agent_session_transcripts WHERE session_id = ?1",
            params![session_id],
        )
        .map_err(|error| format!("无法删除会话 transcript：{error}"))?;

    Ok(())
}

/** 截断会话并删除 transcript，避免下一轮把已丢弃历史继续喂给模型。 */
pub fn rewind_agent_session(
    app: &AppHandle,
    mut snapshot: WorkspaceSnapshot,
    session_id: &str,
    message_id: &str,
    prompt: &str,
) -> Result<WorkspaceSnapshot, String> {
    apply_rewind_to_snapshot(&mut snapshot, session_id, message_id, prompt)?;
    normalize_sessions_for_snapshot(&mut snapshot);

    let session = snapshot
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .ok_or_else(|| "找不到要编辑的会话。".to_owned())?
        .clone();

    let mut connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| format!("无法启动会话回退事务：{error}"))?;
    persist_session_in_transaction(&transaction, &session)?;
    delete_agent_session_transcript_on_connection(&transaction, session_id)?;
    transaction
        .commit()
        .map_err(|error| format!("无法提交会话回退事务：{error}"))?;

    Ok(snapshot)
}

/** 保存某个会话时保留调用方正在查看的会话焦点，避免后台落库把用户切走。 */
pub(crate) fn retain_viewer_session_id(snapshot: &mut WorkspaceSnapshot, saved_session_id: &str) {
    let viewer_session_id = snapshot.active_session_id.clone();
    if !viewer_session_id.is_empty()
        && snapshot
            .sessions
            .iter()
            .any(|session| session.id == viewer_session_id)
    {
        return;
    }

    snapshot.active_session_id = saved_session_id.to_owned();
}

/** 只回写快照中的一个会话，避免 Agent 回合用过期列表删掉或打回其它会话。 */
pub fn save_snapshot_session(
    app: &AppHandle,
    snapshot: &WorkspaceSnapshot,
    session_id: &str,
) -> Result<(), String> {
    let session = snapshot.sessions.iter().find(|s|s.id==session_id).ok_or("找不到要保存的会话")?;
    let mut trusted = session.clone();
    let mut connection=open_database(app)?;
    let _guard=lock_database_writer()?;
    let transaction=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e|e.to_string())?;
    let payload: Option<String>=transaction.query_row("SELECT payload_json FROM agent_sessions WHERE id=?1",params![session_id],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
    if let Some(payload)=payload {
        let mut current: AgentSession=serde_json::from_str(&payload).map_err(|e|e.to_string())?;
        migrate_session_authority(&mut current);
        if !crate::permissions::same_authority(&current,session) {
            trusted.security_level=current.security_level; trusted.permission_version=current.permission_version;
            trusted.plan_mode=current.plan_mode; trusted.knowledge_base_ids=current.knowledge_base_ids;
            trusted.pending_change=current.pending_change; trusted.pending_change_set=current.pending_change_set;
        }
    }
    persist_session_in_transaction(&transaction,&trusted)?;
    transaction.commit().map_err(|e|e.to_string())

}

/** 保存单个会话，并返回已经写入快照的下一版工作台状态。 */
pub fn save_session(
    app: &AppHandle,
    mut snapshot: WorkspaceSnapshot,
    mut session: AgentSession,
) -> Result<WorkspaceSnapshot, String> {
    if let Some(existing) = load_persisted_session(app, &session.id)? {
        // 普通会话保存不能改写高权限审批载荷；仅允许按稳定 operation id 更新勾选状态。
        session.security_level = existing.security_level;
        session.permission_version = existing.permission_version;
        session.plan_mode = existing.plan_mode;
        session.schedule_identity = existing.schedule_identity;
        session.pending_change = match (existing.pending_change,session.pending_change) {
            (Some(mut trusted),Some(draft)) if trusted.id==draft.id => { trusted.review_comments=draft.review_comments; trusted.review_state=draft.review_state; Some(trusted) },
            (trusted,_)=>trusted,
        };
        session.knowledge_base_ids = existing.knowledge_base_ids;
        session.im_identity = existing.im_identity;
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
    } else {
        // 新会话只能继承当前默认权限，不能用普通保存伪造审批、变更或远程身份。
        session.security_level = load_user_settings(app)?.agent_security.default_level;
        session.permission_version = 0;
        session.plan_mode = false;
        session.im_identity = None;
        session.schedule_identity = None;
        session.pending_change = None;
        session.pending_change_set = None;
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

    retain_viewer_session_id(&mut snapshot, &session.id);
    normalize_sessions_for_snapshot(&mut snapshot);
    save_snapshot_session(app, &snapshot, &session.id)?;

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
            serde_json::from_str::<AgentSession>(&payload)
                .map(|mut session| {
                    normalize_session_timestamps(&mut session);
                    migrate_session_authority(&mut session);
                    session
                })
                .map_err(|error| format!("无法解析持久化会话：{error}"))
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

    let deleted_at = format_local_datetime();
    deleted_session.deleted_at = Some(deleted_at.clone());
    deleted_session.updated_at = deleted_at;

    if snapshot.active_session_id == session_id
        || !snapshot
            .sessions
            .iter()
            .any(|session| session.id == snapshot.active_session_id)
    {
        ensure_visible_session_after_delete(&mut snapshot);
    }

    // 只把该会话标记为逻辑删除并 upsert，不能整表重写，否则并发回合会丢掉其它会话。
    save_session_records(app, std::slice::from_ref(&deleted_session))?;

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

        normalize_session_timestamps(&mut session);

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

        if !session.title_customized {
            session.title = crate::im::format_im_session_title(&identity);
        }
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

    persist_session_records_in_transaction(&transaction, sessions)?;

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
    let viewer = snapshot.active_session_id.clone();
    snapshot = load_workspace_snapshot(app)?;
    snapshot.active_session_id=viewer;
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

    let trusted = load_persisted_session(app, session_id)?.ok_or("找不到持久化会话")?;
    *session = trusted;
    if session.knowledge_base_ids != next_ids {
        invalidate_authority(session);
        session.knowledge_base_ids = next_ids;
    }
    session.updated_at = format_local_datetime();
    let session_id = session.id.clone();
    normalize_sessions_for_snapshot(&mut snapshot);
    let changed=snapshot.sessions.iter().find(|s|s.id==session_id).ok_or("会话不存在")?;
    save_session_records(app,std::slice::from_ref(changed))?;

    Ok(snapshot)
}

/** 恢复历史会话绑定的知识库；文件焦点只在会话仍有有效笔记引用时同步。 */
pub fn restore_session_context(
    app: &AppHandle,
    mut snapshot: WorkspaceSnapshot,
    session_id: &str,
    restore_editor_context: bool,
) -> Result<WorkspaceSnapshot, String> {
    // 切会话时以 SQLite 为准合并会话列表，避免带回过期回合快照、丢掉其它线程的新消息。
    let persisted_sessions = load_sessions_for_snapshot(app, &snapshot)?;
    if !persisted_sessions.is_empty() {
        snapshot.sessions = persisted_sessions;
    }
    normalize_sessions_for_snapshot(&mut snapshot);
    let session = snapshot
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .cloned()
        .ok_or_else(|| "找不到要恢复的会话".to_owned())?;
    if !restore_editor_context {
        snapshot.active_session_id = session.id;
        return Ok(snapshot);
    }
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
    normalize_session_timestamps(session);

    migrate_session_authority(session);
    crate::file_changes::migrate_legacy_pending(session, &snapshot.knowledge_bases);

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

#[cfg(test)]
mod tests {
    use super::{
        migrate_session_authority, normalize_session_for_snapshot, normalize_session_timestamps,
        persist_session_in_transaction, retain_viewer_session_id,
    };
    use crate::domain::{AgentSession, WorkspaceSnapshot};

    fn test_session(id: &str) -> AgentSession {
        AgentSession {
            id: id.to_owned(),
            title: id.to_owned(),
            im_identity: None,
            schedule_identity: None,
            r#type: "knowledge-base".to_owned(),
            knowledge_base_ids: vec!["kb-a".to_owned()],
            active_note_id: None,
            pinned_note_ids: Vec::new(),
            messages: Vec::new(),
            pending_change: None,
            pending_change_set: None,
            security_level: "basic".to_owned(),
            permission_version: 0,
            plan_mode: false,
            context_summary: None,
            created_at: "t".to_owned(),
            updated_at: "t".to_owned(),
            deleted_at: None,
            model_provider_id: None,
            model_id: None,
            context_usage: None,
            title_customized: false,
            pinned_at: None,
            archived_at: None,
        }
    }

    /** 有真实更新时间的会话不能因历史消息或重新加载被改写。 */
    #[test]
    fn timestamp_migration_preserves_explicit_session_times() {
        let mut session = test_session("session-old");
        session.created_at = "2026/08/19 22:39".to_owned();
        session.updated_at = "2026/10/24 18:51".to_owned();
        let before = serde_json::to_value(&session).unwrap();

        normalize_session_timestamps(&mut session);

        assert_eq!(serde_json::to_value(&session).unwrap(), before);
    }

    /** 对应截图中“刚刚”的会话：最后活动晚于创建时间，迁移必须保留该差异。 */
    #[test]
    fn timestamp_migration_recovers_latest_activity_instead_of_creation_time() {
        let created_millis = super::parse_local_datetime_millis("2026/10/24 16:59").unwrap();
        let mut session = test_session(&format!("session-knowledge-base-{created_millis}-legacy"));
        session.created_at = "2026/10/24 16:59".to_owned();
        session.updated_at = "刚刚".to_owned();
        session.context_summary = Some(crate::domain::AgentContextSummary {
            updated_at: "2026/10/24 18:51".to_owned(),
            ..Default::default()
        });

        normalize_session_timestamps(&mut session);

        assert_eq!(session.created_at, "2026/10/24 16:59");
        assert_eq!(session.updated_at, "2026/10/24 18:51");
    }

    /** 旧版误存的草稿没有会话时间戳，仍应从首条消息及上下文恢复。 */
    #[test]
    fn timestamp_migration_recovers_persisted_draft_and_is_idempotent() {
        let mut session = test_session("__draft-session__");
        session.created_at = "未保存".to_owned();
        session.updated_at = "刚刚".to_owned();
        session.messages = serde_json::from_value(serde_json::json!([
            {"id": "user-1787150386036-4346qi", "role": "user", "content": "旧消息"}
        ]))
        .unwrap();
        session.context_summary = Some(crate::domain::AgentContextSummary {
            updated_at: "2026/08/19 22:41".to_owned(),
            ..Default::default()
        });

        normalize_session_timestamps(&mut session);

        assert_eq!(
            session.created_at,
            super::format_local_datetime_from_millis(1_787_150_386_036).unwrap()
        );
        assert_eq!(session.updated_at, "2026/08/19 22:41");
        let migrated = serde_json::to_value(&session).unwrap();
        normalize_session_timestamps(&mut session);
        assert_eq!(serde_json::to_value(&session).unwrap(), migrated);
    }

    /** 嵌套工具轨迹和带时区的记账时间都属于可恢复的最后活动证据。 */
    #[test]
    fn timestamp_migration_uses_latest_nested_trace_and_context_usage() {
        let mut session = test_session("session-old-uuid");
        session.created_at = "2026/10/24 16:59".to_owned();
        session.updated_at = "刚刚".to_owned();
        session.messages = serde_json::from_value(serde_json::json!([
            {"id": "assistant-uuid", "role": "assistant", "content": "旧消息", "trace": [
                {"id": "task", "type": "tool", "timestamp": "2026/10/24 17:00", "children": [
                    {"id": "child", "type": "tool", "timestamp": "2026/10/24 18:51"}
                ]}
            ]}
        ]))
        .unwrap();
        session.context_summary = Some(crate::domain::AgentContextSummary {
            updated_at: "2026-10-24T09:30:00+08:00".to_owned(),
            ..Default::default()
        });
        normalize_session_timestamps(&mut session);
        assert_eq!(session.updated_at, "2026/10/24 18:51");

        session.updated_at.clear();
        // 先构造本地 20:00 对应的 RFC3339，避免测试依赖运行机器恰好处于东八区。
        let recorded_millis = super::parse_local_datetime_millis("2026/10/24 20:00").unwrap();
        let recorded_at = chrono::DateTime::from_timestamp_millis(recorded_millis)
            .unwrap()
            .to_rfc3339();
        session.context_usage = Some(
            serde_json::from_value(serde_json::json!({
                "modelId": "test", "promptTokens": 1, "completionTokens": 1,
                "totalTokens": 2, "recordedAt": recorded_at
            }))
            .unwrap(),
        );
        normalize_session_timestamps(&mut session);
        assert_eq!(session.updated_at, "2026/10/24 20:00");
    }

    /** 无消息时回退到已知创建时间；完全无证据时不能伪造为今天。 */
    #[test]
    fn timestamp_migration_falls_back_to_known_time_and_keeps_unknown_time_empty() {
        let mut session = test_session("session-old-uuid");
        session.created_at = "2026/08/19 22:39".to_owned();
        session.updated_at = "刚刚".to_owned();
        normalize_session_timestamps(&mut session);
        assert_eq!(session.updated_at, session.created_at);

        session.created_at = "未保存".to_owned();
        session.updated_at = "刚刚".to_owned();
        normalize_session_timestamps(&mut session);
        assert!(session.created_at.is_empty());
        assert!(session.updated_at.is_empty());
    }

    /** 恢复后的列值与完整 payload 一起落库，重新读取后无需依赖加载时刻。 */
    #[test]
    fn session_persistence_writes_recovered_times_to_columns_and_payload() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("session-timestamps.sqlite3");
        let mut connection = rusqlite::Connection::open(&database_path).unwrap();
        super::ensure_database_schema(&connection, &database_path).unwrap();
        let transaction = connection.transaction().unwrap();
        let created_millis = super::parse_local_datetime_millis("2026/10/24 16:59").unwrap();
        let mut session = test_session(&format!("session-knowledge-base-{created_millis}-legacy"));
        session.created_at = "刚刚".to_owned();
        session.updated_at = "刚刚".to_owned();
        session.context_summary = Some(crate::domain::AgentContextSummary {
            updated_at: "2026/10/24 18:51".to_owned(),
            ..Default::default()
        });

        persist_session_in_transaction(&transaction, &session).unwrap();
        transaction.commit().unwrap();
        let (created_at, updated_at, payload): (String, String, String) = connection
            .query_row(
                "SELECT created_at, updated_at, payload_json FROM agent_sessions WHERE id = ?1",
                [&session.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let stored: AgentSession = serde_json::from_str(&payload).unwrap();
        assert_eq!(updated_at, "2026/10/24 18:51");
        assert_eq!(stored.created_at, created_at);
        assert_eq!(stored.updated_at, updated_at);
        assert_eq!(
            created_at,
            super::created_at_from_session_id(&session.id).unwrap()
        );
        assert_eq!(stored.title, session.title);
    }

    #[test]
    fn legacy_change_set_expires_idempotently_without_losing_records() {
        for status in ["pending", "partial"] {
            let mut session = test_session("legacy-migration");
            session.pending_change_set = Some(serde_json::from_value(serde_json::json!({
                "id":"artifacts-old", "executionId":"legacy", "skillId":"agent", "status":status,
                "summary":"保留副本", "operations":[], "createdAt":"t"
            })).unwrap());
            migrate_session_authority(&mut session);
            assert_eq!(session.security_level,"workspace-write");
            assert_eq!(session.pending_change_set.as_ref().unwrap().status,"expired");
            assert_eq!(session.pending_change_set.as_ref().unwrap().id,"artifacts-old");
            let migrated = serde_json::to_value(&session).unwrap();
            migrate_session_authority(&mut session);
            assert_eq!(serde_json::to_value(&session).unwrap(),migrated);
        }
    }

    #[test]
    fn old_session_json_defaults_title_pin_and_archive() {
        let session: AgentSession = serde_json::from_str(
            r#"{
                "id": "session-old",
                "title": "旧会话",
                "type": "knowledge-base",
                "knowledgeBaseIds": ["kb-a"],
                "activeNoteId": null,
                "pinnedNoteIds": [],
                "messages": [],
                "pendingChange": null,
                "securityLevel": "basic",
                "createdAt": "2026/01/01 10:00",
                "updatedAt": "2026/01/01 10:00"
            }"#,
        )
        .expect("旧会话 JSON 应能反序列化");

        assert!(!session.title_customized);
        assert!(session.pinned_at.is_none());
        assert!(session.archived_at.is_none());
        assert!(!session.plan_mode);
    }

    /** IM 与定时会话强制关闭规划模式，无论前端或历史 payload 如何声明。 */
    #[test]
    fn remote_sessions_cannot_keep_plan_mode() {
        let snapshot = WorkspaceSnapshot {
            knowledge_bases: Vec::new(),
            folders: Vec::new(),
            notes: Vec::new(),
            documents: Vec::new(),
            sessions: Vec::new(),
            active_knowledge_base_id: String::new(),
            active_note_id: String::new(),
            active_document_id: String::new(),
            active_session_id: String::new(),
        };

        let mut im_session = test_session("session-im");
        im_session.plan_mode = true;
        im_session.im_identity = Some(crate::domain::ImSessionIdentity {
            provider_id: "feishu".to_owned(),
            conversation_kind: "direct".to_owned(),
            channel_hash: "redacted".to_owned(),
            initial_message_preview: "IM".to_owned(),
            last_message_preview: "IM".to_owned(),
        });
        normalize_session_for_snapshot(&mut im_session, &snapshot);
        assert!(!im_session.plan_mode);

        let mut schedule_session = test_session("session-schedule");
        schedule_session.plan_mode = true;
        schedule_session.schedule_identity = Some(crate::domain::ScheduleSessionIdentity {
            job_id: "job-a".to_owned(),
            job_name: "每日整理".to_owned(),
        });
        normalize_session_for_snapshot(&mut schedule_session, &snapshot);
        assert!(!schedule_session.plan_mode);
    }

    #[test]
    fn save_session_keeps_viewer_focus_when_persisting_another_session() {
        let mut snapshot = WorkspaceSnapshot {
            knowledge_bases: Vec::new(),
            folders: Vec::new(),
            notes: Vec::new(),
            documents: Vec::new(),
            sessions: vec![test_session("session-a"), test_session("session-b")],
            active_knowledge_base_id: String::new(),
            active_note_id: String::new(),
            active_document_id: String::new(),
            active_session_id: "session-a".to_owned(),
        };

        retain_viewer_session_id(&mut snapshot, "session-b");
        assert_eq!(snapshot.active_session_id, "session-a");
    }

    #[test]
    fn save_session_adopts_saved_id_when_viewer_is_missing() {
        let mut snapshot = WorkspaceSnapshot {
            knowledge_bases: Vec::new(),
            folders: Vec::new(),
            notes: Vec::new(),
            documents: Vec::new(),
            sessions: vec![test_session("session-b")],
            active_knowledge_base_id: String::new(),
            active_note_id: String::new(),
            active_document_id: String::new(),
            active_session_id: "session-a".to_owned(),
        };

        retain_viewer_session_id(&mut snapshot, "session-b");
        assert_eq!(snapshot.active_session_id, "session-b");
    }
}

/** 幂等迁移仅更新元数据，不触碰真实知识库文件与旧产物副本。 */
pub(crate) fn migrate_session_authority(session: &mut AgentSession) {
    session.security_level = crate::permissions::migrate_level(&session.security_level);
    if session.im_identity.is_some() { session.security_level = "workspace-write".into(); session.plan_mode = false; }
    if session.schedule_identity.is_some() { session.security_level = "workspace-readonly".into(); session.plan_mode = false; }
    if let Some(set) = session.pending_change_set.as_mut() {
        if set.execution_id != crate::domain::AGENT_DIRECT_EXECUTION_ID && matches!(set.status.as_str(), "pending" | "partial") {
            set.status = "expired".into();
            set.warnings.push("历史来源的变更集已失效；副本保留，请重新提出操作。".into());
        }
    }
}

fn invalidate_authority(session: &mut AgentSession) {
    session.permission_version = session.permission_version.saturating_add(1);
    crate::operation_approval::cancel_session(&session.id);
    if crate::runtime::is_session_turn_active(&session.id) { crate::runtime::request_agent_abort(&session.id); }
}

/** 只有显式权限入口可以切档；进入完全档必须带对应的用户确认。 */
pub fn update_session_authority(app: &AppHandle, mut snapshot: WorkspaceSnapshot, session_id: &str,
    level: &str, plan_mode: bool, confirmed_full_access: bool) -> Result<WorkspaceSnapshot, String> {
    let mut session = load_persisted_session(app, session_id)?.ok_or("找不到持久化会话")?;
    if session.im_identity.is_some() || session.schedule_identity.is_some() { return Err("远程或定时会话不能修改本地执行权限。".into()); }
    if !matches!(level, "workspace-readonly" | "workspace-write" | "full-confirm" | "full-auto") { return Err("未知权限档位。".into()); }
    if session.security_level != level && matches!(level, "full-confirm" | "full-auto") && !confirmed_full_access {
        return Err("进入完全权限需要确认真实环境访问能力；无需审批档还需要确认自动执行。".into());
    }
    if session.security_level != level || session.plan_mode != plan_mode {
        invalidate_authority(&mut session);
        session.security_level = level.to_owned(); session.plan_mode = plan_mode;
    }
    save_session_records(app, std::slice::from_ref(&session))?;
    if let Some(existing) = snapshot.sessions.iter_mut().find(|item| item.id == session_id) { *existing = session; }
    Ok(snapshot)
}
