use super::*;

/** 捕获文档历史记录所需的完整上下文；正文只写入快照文件，不进入 SQLite 表。 */
pub struct DocumentHistoryCapture {
    pub target_kind: String,
    pub knowledge_base_id: String,
    pub target_id: String,
    pub relative_path: String,
    pub title: String,
    pub file_type: String,
    pub content: String,
    pub source: String,
    pub session_id: Option<String>,
    pub change_id: Option<String>,
    pub operation_id: Option<String>,
}

/** 历史清理结果；cleanup_failure_count 只用于脱敏日志，不阻止用户文件删除。 */
#[derive(Clone, Debug, Default)]
pub struct DocumentHistoryClearSummary {
    pub removed_count: usize,
    pub cleanup_failure_count: usize,
}

/** 历史捕获结果；entry 为空表示最新 hash 相同而跳过，prune_summary 用于调用层写清理失败日志。 */
#[derive(Clone, Debug, Default)]
pub struct DocumentHistoryCaptureResult {
    pub entry: Option<DocumentHistoryEntry>,
    pub prune_summary: DocumentHistoryClearSummary,
}

/** 捕获当前磁盘正文为历史版本；相同文件最新 hash 一致时跳过重复记录。 */
pub fn capture_document_history(
    app: &AppHandle,
    capture: DocumentHistoryCapture,
) -> Result<DocumentHistoryCaptureResult, String> {
    validate_document_history_capture(&capture)?;

    let content_hash = hash_content(&capture.content);
    let byte_size = capture.content.as_bytes().len();
    let line_count = count_document_history_lines(&capture.content);
    let connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    let latest_hash =
        load_latest_document_history_hash(&connection, &capture.target_kind, &capture.target_id)?;

    if latest_hash.as_deref() == Some(content_hash.as_str()) {
        return Ok(DocumentHistoryCaptureResult::default());
    }

    let snapshot_root = document_history_root(app)?;
    let entry = DocumentHistoryEntry {
        id: create_id("history"),
        target_kind: capture.target_kind,
        knowledge_base_id: capture.knowledge_base_id,
        target_id: capture.target_id,
        relative_path: capture.relative_path,
        title: capture.title,
        file_type: capture.file_type,
        content_hash,
        byte_size,
        line_count,
        source: capture.source,
        session_id: capture.session_id,
        change_id: capture.change_id,
        operation_id: capture.operation_id,
        created_at: format_local_datetime(),
    };

    write_document_history_snapshot(&snapshot_root, &entry.id, &capture.content)?;

    if let Err(error) = insert_document_history_entry(&connection, &entry) {
        let _ = remove_document_history_snapshot(&snapshot_root, &entry.id);
        return Err(error);
    }

    let prune_summary = prune_document_history_entries(
        &connection,
        &snapshot_root,
        &entry.target_kind,
        &entry.target_id,
    )?;

    Ok(DocumentHistoryCaptureResult {
        entry: Some(entry),
        prune_summary,
    })
}

/** 读取当前文件的历史版本列表，按最新创建时间倒序返回。 */
pub fn load_document_history(
    app: &AppHandle,
    target_kind: &str,
    target_id: &str,
) -> Result<Vec<DocumentHistoryEntry>, String> {
    validate_document_history_target(target_kind, target_id)?;

    let connection = open_database(app)?;
    let mut statement = connection
        .prepare(
            "SELECT id, target_kind, knowledge_base_id, target_id, relative_path, title,
                    file_type, content_hash, byte_size, line_count, source, session_id,
                    change_id, operation_id, created_at
             FROM document_history_entries
             WHERE target_kind = ?1 AND target_id = ?2
             ORDER BY rowid DESC",
        )
        .map_err(|error| format!("无法准备文档历史读取：{error}"))?;
    let rows = statement
        .query_map(
            params![target_kind, target_id],
            document_history_entry_from_row,
        )
        .map_err(|error| format!("无法查询文档历史记录：{error}"))?;
    let mut entries = Vec::new();

    for row in rows {
        entries.push(row.map_err(|error| format!("无法解析文档历史记录：{error}"))?);
    }

    Ok(entries)
}

/** 读取单条历史记录详情，并从安全快照目录加载正文内容。 */
pub fn load_document_history_entry(
    app: &AppHandle,
    entry_id: &str,
) -> Result<DocumentHistoryEntryDetail, String> {
    validate_document_history_entry_id(entry_id)?;

    let connection = open_database(app)?;
    let entry = connection
        .query_row(
            "SELECT id, target_kind, knowledge_base_id, target_id, relative_path, title,
                    file_type, content_hash, byte_size, line_count, source, session_id,
                    change_id, operation_id, created_at
             FROM document_history_entries
             WHERE id = ?1",
            params![entry_id],
            document_history_entry_from_row,
        )
        .optional()
        .map_err(|error| format!("无法读取文档历史详情：{error}"))?
        .ok_or_else(|| "找不到该历史记录。".to_owned())?;
    let snapshot_root = document_history_root(app)?;
    let content = read_document_history_snapshot(&snapshot_root, &entry.id)?;

    Ok(DocumentHistoryEntryDetail { entry, content })
}

/** 清空当前文件历史记录和对应快照；不会删除用户知识库中的文档。 */
pub fn clear_document_history(
    app: &AppHandle,
    target_kind: &str,
    target_id: &str,
) -> Result<DocumentHistoryClearSummary, String> {
    validate_document_history_target(target_kind, target_id)?;

    let connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    let snapshot_root = document_history_root(app)?;
    let entry_ids = load_document_history_ids_for_target(&connection, target_kind, target_id)?;

    connection
        .execute(
            "DELETE FROM document_history_entries WHERE target_kind = ?1 AND target_id = ?2",
            params![target_kind, target_id],
        )
        .map_err(|error| format!("无法清空文档历史记录：{error}"))?;
    let cleanup_failure_count =
        remove_document_history_snapshots_best_effort(&snapshot_root, &entry_ids);

    Ok(DocumentHistoryClearSummary {
        removed_count: entry_ids.len(),
        cleanup_failure_count,
    })
}

/** 重命名文件后迁移历史元数据，保证旧版本仍挂在新的文件 ID 上。 */
pub fn migrate_document_history_target(
    app: &AppHandle,
    target_kind: &str,
    previous_target_id: &str,
    next_target_id: &str,
    knowledge_base_id: &str,
    next_relative_path: &str,
    next_title: &str,
) -> Result<usize, String> {
    validate_document_history_target(target_kind, previous_target_id)?;
    validate_document_history_target(target_kind, next_target_id)?;
    validate_document_history_relative_path(next_relative_path)?;

    let connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    let changed_rows = connection
        .execute(
            "UPDATE document_history_entries
             SET target_id = ?1, knowledge_base_id = ?2, relative_path = ?3, title = ?4
             WHERE target_kind = ?5 AND target_id = ?6",
            params![
                next_target_id,
                knowledge_base_id,
                next_relative_path,
                next_title,
                target_kind,
                previous_target_id
            ],
        )
        .map_err(|error| format!("无法迁移文档历史记录：{error}"))?;

    Ok(changed_rows)
}

/** 校验文档历史捕获上下文，避免无效类型或越界相对路径进入持久层。 */
pub(crate) fn validate_document_history_capture(
    capture: &DocumentHistoryCapture,
) -> Result<(), String> {
    validate_document_history_target(&capture.target_kind, &capture.target_id)?;
    validate_document_history_relative_path(&capture.relative_path)?;

    if capture.knowledge_base_id.trim().is_empty() {
        return Err("文档历史记录缺少知识库 ID。".to_owned());
    }

    if !matches!(capture.file_type.as_str(), "markdown" | "txt") {
        return Err("该文件类型暂不支持历史记录。".to_owned());
    }

    if !matches!(
        capture.source.as_str(),
        "manual-save" | "agent-change" | "restore"
    ) {
        return Err("未知的文档历史来源。".to_owned());
    }

    Ok(())
}

/** 校验历史目标类型和实体 ID；首版只支持 Markdown note 与 TXT document。 */
pub(crate) fn validate_document_history_target(
    target_kind: &str,
    target_id: &str,
) -> Result<(), String> {
    if !matches!(target_kind, "note" | "document") {
        return Err("该目标类型暂不支持历史记录。".to_owned());
    }

    if target_id.trim().is_empty() {
        return Err("文档历史记录缺少目标 ID。".to_owned());
    }

    Ok(())
}

/** 校验知识库内相对路径，历史元数据不得保存绝对路径或上级目录。 */
pub(crate) fn validate_document_history_relative_path(relative_path: &str) -> Result<(), String> {
    let requested_path = Path::new(relative_path);

    if relative_path.trim().is_empty()
        || requested_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("文档历史记录路径超出知识库根目录。".to_owned());
    }

    Ok(())
}

/** 校验历史快照文件名来源 ID，防止 entryId 被拼成路径穿越。 */
pub(crate) fn validate_document_history_entry_id(entry_id: &str) -> Result<(), String> {
    if entry_id.trim().is_empty()
        || !entry_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err("历史记录 ID 不合法。".to_owned());
    }

    Ok(())
}

/** 统计正文行数；空正文显示 0 行，带尾随换行的文本按可见逻辑行计数。 */
pub(crate) fn count_document_history_lines(content: &str) -> usize {
    if content.is_empty() {
        0
    } else {
        content.split('\n').count()
    }
}

/** 返回文档历史快照根目录，创建目录失败时直接阻止写入。 */
pub(crate) fn document_history_root(app: &AppHandle) -> Result<PathBuf, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法获取应用数据目录：{error}"))?;
    let history_root = app_data_dir.join(DOCUMENT_HISTORY_SNAPSHOT_DIR);

    fs::create_dir_all(&history_root).map_err(|error| format!("无法创建文档历史目录：{error}"))?;

    Ok(history_root)
}

/** 根据安全 entryId 生成快照路径；文件名不包含用户知识库路径。 */
pub(crate) fn document_history_snapshot_path(
    root: &Path,
    entry_id: &str,
) -> Result<PathBuf, String> {
    validate_document_history_entry_id(entry_id)?;

    Ok(root.join(format!("{entry_id}.snapshot")))
}

/** 原子写入历史正文快照，确保 SQLite 元数据不会指向半截文件。 */
pub(crate) fn write_document_history_snapshot(
    root: &Path,
    entry_id: &str,
    content: &str,
) -> Result<(), String> {
    let target_path = document_history_snapshot_path(root, entry_id)?;
    let mut temp_file = NamedTempFile::new_in(root)
        .map_err(|error| format!("无法创建历史快照临时文件：{error}"))?;

    temp_file
        .write_all(content.as_bytes())
        .map_err(|error| format!("无法写入历史快照：{error}"))?;
    temp_file
        .persist(&target_path)
        .map_err(|error| format!("无法保存历史快照：{}", error.error))?;

    Ok(())
}

/** 读取历史正文快照，并校验最终路径仍位于历史目录内。 */
pub(crate) fn read_document_history_snapshot(
    root: &Path,
    entry_id: &str,
) -> Result<String, String> {
    let target_path = document_history_snapshot_path(root, entry_id)?;
    let canonical_root =
        fs::canonicalize(root).map_err(|error| format!("无法解析文档历史目录：{error}"))?;
    let canonical_target = fs::canonicalize(&target_path)
        .map_err(|error| format!("历史快照不存在或不可访问：{error}"))?;

    if !canonical_target.starts_with(&canonical_root) || !canonical_target.is_file() {
        return Err("历史快照路径不合法。".to_owned());
    }

    fs::read_to_string(&canonical_target).map_err(|error| format!("无法读取历史快照：{error}"))
}

/** 删除单个历史快照；用于 DB 插入失败后的回滚清理。 */
pub(crate) fn remove_document_history_snapshot(root: &Path, entry_id: &str) -> Result<(), String> {
    let target_path = document_history_snapshot_path(root, entry_id)?;

    match fs::remove_file(&target_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("无法删除历史快照：{error}")),
    }
}

/** 批量尽力删除历史快照，返回失败数量供调用层写脱敏日志。 */
pub(crate) fn remove_document_history_snapshots_best_effort(
    root: &Path,
    entry_ids: &[String],
) -> usize {
    entry_ids
        .iter()
        .filter(|entry_id| remove_document_history_snapshot(root, entry_id).is_err())
        .count()
}

/** 查询当前目标最新历史 hash，用于避免同一版本重复捕获。 */
pub(crate) fn load_latest_document_history_hash(
    connection: &Connection,
    target_kind: &str,
    target_id: &str,
) -> Result<Option<String>, String> {
    connection
        .query_row(
            "SELECT content_hash
             FROM document_history_entries
             WHERE target_kind = ?1 AND target_id = ?2
             ORDER BY rowid DESC
             LIMIT 1",
            params![target_kind, target_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("无法读取最新文档历史 hash：{error}"))
}

/** 写入文档历史元数据；正文内容由同 ID 的快照文件承载。 */
pub(crate) fn insert_document_history_entry(
    connection: &Connection,
    entry: &DocumentHistoryEntry,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO document_history_entries
             (id, target_kind, knowledge_base_id, target_id, relative_path, title, file_type,
              content_hash, byte_size, line_count, source, session_id, change_id,
              operation_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                &entry.id,
                &entry.target_kind,
                &entry.knowledge_base_id,
                &entry.target_id,
                &entry.relative_path,
                &entry.title,
                &entry.file_type,
                &entry.content_hash,
                entry.byte_size as i64,
                entry.line_count as i64,
                &entry.source,
                entry.session_id.as_deref(),
                entry.change_id.as_deref(),
                entry.operation_id.as_deref(),
                &entry.created_at
            ],
        )
        .map_err(|error| format!("无法写入文档历史记录：{error}"))?;

    Ok(())
}

/** 按保留策略清理当前文件历史，并同步删除被裁剪的快照文件。 */
pub(crate) fn prune_document_history_entries(
    connection: &Connection,
    snapshot_root: &Path,
    target_kind: &str,
    target_id: &str,
) -> Result<DocumentHistoryClearSummary, String> {
    let oldest_created_at = (Local::now() - ChronoDuration::days(DOCUMENT_HISTORY_RETENTION_DAYS))
        .format("%Y/%m/%d %H:%M")
        .to_string();
    let mut entry_ids_to_delete = HashSet::new();
    let expired_ids = query_document_history_ids(
        connection,
        "SELECT id
         FROM document_history_entries
         WHERE target_kind = ?1 AND target_id = ?2 AND created_at < ?3",
        params![target_kind, target_id, oldest_created_at],
    )?;
    let overflow_ids = query_document_history_ids(
        connection,
        "SELECT id
         FROM document_history_entries
         WHERE target_kind = ?1 AND target_id = ?2
           AND rowid NOT IN (
             SELECT rowid
             FROM document_history_entries
             WHERE target_kind = ?1 AND target_id = ?2
             ORDER BY rowid DESC
             LIMIT ?3
           )",
        params![
            target_kind,
            target_id,
            MAX_DOCUMENT_HISTORY_ENTRIES_PER_FILE as i64
        ],
    )?;

    entry_ids_to_delete.extend(expired_ids);
    entry_ids_to_delete.extend(overflow_ids);

    if entry_ids_to_delete.is_empty() {
        return Ok(DocumentHistoryClearSummary::default());
    }

    let entry_ids = entry_ids_to_delete.into_iter().collect::<Vec<_>>();

    for entry_id in &entry_ids {
        connection
            .execute(
                "DELETE FROM document_history_entries WHERE id = ?1",
                params![entry_id],
            )
            .map_err(|error| format!("无法删除过期文档历史记录：{error}"))?;
    }
    let cleanup_failure_count =
        remove_document_history_snapshots_best_effort(snapshot_root, &entry_ids);

    Ok(DocumentHistoryClearSummary {
        removed_count: entry_ids.len(),
        cleanup_failure_count,
    })
}

/** 查询当前目标下所有历史 ID，用于清空或删除文件后的快照清理。 */
pub(crate) fn load_document_history_ids_for_target(
    connection: &Connection,
    target_kind: &str,
    target_id: &str,
) -> Result<Vec<String>, String> {
    query_document_history_ids(
        connection,
        "SELECT id FROM document_history_entries WHERE target_kind = ?1 AND target_id = ?2",
        params![target_kind, target_id],
    )
}

/** 执行只返回 id 的历史记录查询，集中处理 SQLite row 错误。 */
pub(crate) fn query_document_history_ids<P>(
    connection: &Connection,
    sql: &str,
    params: P,
) -> Result<Vec<String>, String>
where
    P: rusqlite::Params,
{
    let mut statement = connection
        .prepare(sql)
        .map_err(|error| format!("无法准备文档历史清理查询：{error}"))?;
    let rows = statement
        .query_map(params, |row| row.get::<_, String>(0))
        .map_err(|error| format!("无法查询文档历史清理列表：{error}"))?;
    let mut entry_ids = Vec::new();

    for row in rows {
        entry_ids.push(row.map_err(|error| format!("无法解析文档历史清理列表：{error}"))?);
    }

    Ok(entry_ids)
}

/** 将 SQLite row 转成文档历史摘要模型，并把无符号数做边界收敛。 */
pub(crate) fn document_history_entry_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<DocumentHistoryEntry> {
    let byte_size = row.get::<_, i64>(8)?.max(0) as usize;
    let line_count = row.get::<_, i64>(9)?.max(0) as usize;

    Ok(DocumentHistoryEntry {
        id: row.get(0)?,
        target_kind: row.get(1)?,
        knowledge_base_id: row.get(2)?,
        target_id: row.get(3)?,
        relative_path: row.get(4)?,
        title: row.get(5)?,
        file_type: row.get(6)?,
        content_hash: row.get(7)?,
        byte_size,
        line_count,
        source: row.get(10)?,
        session_id: row.get(11)?,
        change_id: row.get(12)?,
        operation_id: row.get(13)?,
        created_at: row.get(14)?,
    })
}
