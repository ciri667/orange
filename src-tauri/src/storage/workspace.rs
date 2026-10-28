use super::*;

pub(crate) struct StoredKnowledgeBase {
    id: String,
    name: String,
    path: String,
    semantic_index_enabled: bool,
    updated_at: String,
}

/** Agent search 的结构化过滤条件；空集合和空字符串表示不过滤。 */
#[derive(Clone, Debug, Default)]
pub(crate) struct NoteSearchFilters {
    pub(crate) path_prefix: String,
    pub(crate) tags: Vec<String>,
    pub(crate) file_types: Vec<String>,
    pub(crate) updated_after_unix: Option<i64>,
    pub(crate) updated_before_unix: Option<i64>,
}

/** 规范化工具标签：去空值、去前导 #、去重复，避免模型传参格式影响精确匹配。 */
pub(crate) fn normalize_filter_tags(tags: &[String]) -> Vec<String> {
    let mut normalized: Vec<String> = tags
        .iter()
        .map(|tag| tag.trim().trim_start_matches('#').trim().to_owned())
        .filter(|tag| !tag.is_empty())
        .collect();
    normalized.sort();
    normalized.dedup();
    normalized
}

/** 归一化路径前缀为 / 分隔的目录或文件路径；根目录和空值表示不过滤。 */
pub(crate) fn normalize_path_prefix(prefix: &str) -> String {
    prefix
        .replace('\\', "/")
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

/** 解析 search 时间过滤；日期按本地时区展开为自然日边界，ISO 时间直接使用其 offset。 */
pub(crate) fn parse_note_search_datetime(
    value: &str,
    is_before_bound: bool,
) -> Result<Option<i64>, String> {
    if value.trim().is_empty() {
        return Ok(None);
    }

    if let Ok(date) = chrono::NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d") {
        let time = if is_before_bound {
            date.and_hms_opt(23, 59, 59)
        } else {
            date.and_hms_opt(0, 0, 0)
        };
        return Ok(time
            .and_then(|datetime| Local.from_local_datetime(&datetime).single())
            .map(|datetime| datetime.timestamp()));
    }

    if let Ok(datetime) = chrono::DateTime::parse_from_rfc3339(value.trim()) {
        return Ok(Some(datetime.timestamp()));
    }

    Err("时间过滤必须是 YYYY-MM-DD 或 ISO 8601 时间。".to_owned())
}

/** 判断笔记是否满足结构化过滤；时间字段缺失时，时间过滤会排除该笔记。 */
fn note_matches_filters(note: &Note, filters: &NoteSearchFilters) -> bool {
    if !filters.file_types.is_empty()
        && !filters
            .file_types
            .iter()
            .any(|file_type| file_type == "markdown")
    {
        return false;
    }

    if !filters.path_prefix.is_empty() {
        let prefix = format!("{}/", filters.path_prefix);
        if note.path != filters.path_prefix && !note.path.starts_with(&prefix) {
            return false;
        }
    }

    if !filters.tags.is_empty()
        && !filters
            .tags
            .iter()
            .any(|tag| note.tags.iter().any(|note_tag| note_tag == tag))
    {
        return false;
    }

    let updated_at_unix = crate::storage::parse_index_datetime(&note.updated_at);
    if filters
        .updated_after_unix
        .is_some_and(|after| updated_at_unix.is_none_or(|time| time < after))
        || filters
            .updated_before_unix
            .is_some_and(|before| updated_at_unix.is_none_or(|time| time > before))
    {
        return false;
    }

    true
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
    cleanup_semantic(&transaction)?;
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
        cleanup_semantic(&transaction)?;
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
            .execute("DELETE FROM note_chunk_fts", [])
            .map_err(|error| format!("无法清理 chunk FTS 索引：{error}"))?;
        transaction
            .execute("DELETE FROM note_chunks", [])
            .map_err(|error| format!("无法清理 chunk 索引：{error}"))?;
        transaction
            .execute("DELETE FROM notes", [])
            .map_err(|error| format!("无法清理笔记索引：{error}"))?;
        transaction
            .execute("DELETE FROM note_tags", [])
            .map_err(|error| format!("无法清理笔记标签索引：{error}"))?;
        transaction
            .execute("DELETE FROM knowledge_bases", [])
            .map_err(|error| format!("无法清理知识库索引：{error}"))?;
        transaction
            .execute("DELETE FROM document_chunk_fts", [])
            .map_err(|error| format!("无法清理文档 FTS 索引：{error}"))?;
        transaction
            .execute("DELETE FROM document_chunks", [])
            .map_err(|error| format!("无法清理文档 chunk 索引：{error}"))?;
        transaction
            .execute("DELETE FROM document_index_state", [])
            .map_err(|error| format!("无法清理文档索引状态：{error}"))?;
    }

    for knowledge_base in &snapshot.knowledge_bases {
        ensure_semantic_knowledge_base(
            transaction,
            &knowledge_base.id,
            knowledge_base.semantic_index_enabled,
        )?;
        transaction
            .execute(
                "INSERT OR REPLACE INTO knowledge_bases (id, name, path, semantic_index_enabled, updated_at) VALUES (?1, ?2, ?3, COALESCE((SELECT enabled FROM semantic_index_state WHERE knowledge_base_id=?1),?4), ?5)",
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

    for document in &snapshot.documents {
        if !matches!(document.file_type.as_str(), "docx" | "pdf") {
            continue;
        }
        let Some(knowledge_base) = snapshot
            .knowledge_bases
            .iter()
            .find(|knowledge_base| knowledge_base.id == document.knowledge_base_id)
        else {
            continue;
        };
        // 单个文档解析失败不能阻塞整个知识库索引；下次扫描会重试。
        if let Ok(extraction) = extract_document_text(Path::new(&knowledge_base.path), document) {
            if let Err(error) = upsert_document_index(transaction, document, &extraction) {
                log::warn!(
                    target: "document_index",
                    "文档索引写入失败：path={} error={}",
                    document.path,
                    error
                );
            }
        }
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
    cleanup_semantic(&transaction)?;
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
    cleanup_semantic(&transaction)?;
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
    cleanup_semantic(&transaction)?;
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
            .execute(
                "DELETE FROM note_chunk_fts WHERE note_id = ?1",
                params![note_id],
            )
            .map_err(|error| format!("无法删除 chunk FTS 索引：{error}"))?;
        transaction
            .execute(
                "DELETE FROM note_chunks WHERE note_id = ?1",
                params![note_id],
            )
            .map_err(|error| format!("无法删除 chunk 索引：{error}"))?;
        transaction
            .execute("DELETE FROM notes WHERE id = ?1", params![note_id])
            .map_err(|error| format!("无法删除笔记索引：{error}"))?;
        transaction
            .execute("DELETE FROM note_tags WHERE note_id = ?1", params![note_id])
            .map_err(|error| format!("无法删除笔记标签索引：{error}"))?;
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
            "DELETE FROM note_chunk_fts WHERE knowledge_base_id = ?1",
            params![knowledge_base_id],
        )
        .map_err(|error| format!("无法删除知识库 chunk FTS 索引：{error}"))?;
    transaction
        .execute(
            "DELETE FROM note_chunks WHERE knowledge_base_id = ?1",
            params![knowledge_base_id],
        )
        .map_err(|error| format!("无法删除知识库 chunk 索引：{error}"))?;
    transaction
        .execute(
            "DELETE FROM notes WHERE knowledge_base_id = ?1",
            params![knowledge_base_id],
        )
        .map_err(|error| format!("无法删除知识库笔记索引：{error}"))?;
    transaction
        .execute(
            "DELETE FROM document_chunk_fts WHERE knowledge_base_id = ?1",
            params![knowledge_base_id],
        )
        .map_err(|error| format!("无法删除知识库文档 FTS：{error}"))?;
    transaction
        .execute(
            "DELETE FROM document_chunks WHERE knowledge_base_id = ?1",
            params![knowledge_base_id],
        )
        .map_err(|error| format!("无法删除知识库文档 chunk：{error}"))?;
    transaction
        .execute(
            "DELETE FROM document_index_state WHERE knowledge_base_id = ?1",
            params![knowledge_base_id],
        )
        .map_err(|error| format!("无法删除知识库文档索引状态：{error}"))?;
    transaction
        .execute(
            "DELETE FROM note_tags WHERE knowledge_base_id = ?1",
            params![knowledge_base_id],
        )
        .map_err(|error| format!("无法删除知识库标签索引：{error}"))?;
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
                "INSERT OR REPLACE INTO knowledge_bases (id, name, path, semantic_index_enabled, updated_at) VALUES (?1, ?2, ?3, COALESCE((SELECT enabled FROM semantic_index_state WHERE knowledge_base_id=?1),?4), ?5)",
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
    for document in snapshot.documents.iter().filter(|document| {
        document.knowledge_base_id == knowledge_base_id
            && matches!(document.file_type.as_str(), "docx" | "pdf")
    }) {
        if let Some(knowledge_base) = snapshot
            .knowledge_bases
            .iter()
            .find(|knowledge_base| knowledge_base.id == document.knowledge_base_id)
        {
            if let Ok(extraction) =
                extract_document_text(Path::new(&knowledge_base.path), document)
            {
                let _ = upsert_document_index(transaction, document, &extraction);
            }
        }
    }
    Ok(())
}

/** 文档索引的中间 chunk，保留页码和结构块范围。 */
struct DocumentChunkInput {
    page: Option<usize>,
    block_start: usize,
    block_end: usize,
    heading_path: String,
    content: String,
}

const MAX_DOCUMENT_CHUNK_CHARS: usize = 1_200;

/** 把 DOCX/PDF block 按 1200 字符、页码和标题路径切成检索 chunk。 */
fn chunk_document_blocks(
    document: &WorkspaceDocument,
    extraction: &DocumentTextExtraction,
) -> Vec<DocumentChunkInput> {
    let mut chunks = Vec::new();
    let mut current_text = String::new();
    let mut current_page = None;
    let mut block_start = 0usize;
    let mut block_end = 0usize;
    let mut heading_stack: Vec<(u8, String)> = Vec::new();
    let mut current_heading = String::new();

    let flush = |chunks: &mut Vec<DocumentChunkInput>,
                 text: &mut String,
                 page: Option<usize>,
                 start: usize,
                 end: usize,
                 heading: &str| {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            chunks.push(DocumentChunkInput {
                page,
                block_start: start,
                block_end: end,
                heading_path: heading.to_owned(),
                content: trimmed.to_owned(),
            });
        }
        text.clear();
    };

    for block in &extraction.blocks {
        if block
            .role
            .as_deref()
            .is_some_and(|role| matches!(role, "header" | "footer" | "comment" | "footnote" | "endnote"))
        {
            continue;
        }
        if block.r#type == "heading" {
            let level = block.heading_level.unwrap_or(1);
            heading_stack.retain(|(existing_level, _)| *existing_level < level);
            heading_stack.push((level, block.text.clone()));
            current_heading = heading_stack
                .iter()
                .map(|(_, title)| title.as_str())
                .collect::<Vec<_>>()
                .join(" / ");
            flush(
                &mut chunks,
                &mut current_text,
                current_page,
                block_start,
                block_end,
                &current_heading,
            );
        }
        if current_text.is_empty() {
            current_page = block.page;
            block_start = block.index;
            block_end = block.index;
        }
        let prefix = if current_text.is_empty() {
            String::new()
        } else {
            "\n\n".to_owned()
        };
        let addition = format!("{prefix}{}", block.text);
        let projected = current_text.chars().count() + addition.chars().count();
        if !current_text.is_empty()
            && (block.page != current_page || projected > MAX_DOCUMENT_CHUNK_CHARS)
        {
            flush(
                &mut chunks,
                &mut current_text,
                current_page,
                block_start,
                block_end,
                &current_heading,
            );
            current_page = block.page;
            block_start = block.index;
        }
        if current_text.is_empty() {
            current_text.push_str(&block.text);
        } else {
            current_text.push_str(&addition);
        }
        block_end = block.index;
    }
    flush(
        &mut chunks,
        &mut current_text,
        current_page,
        block_start,
        block_end,
        &current_heading,
    );

    // 单页/单段超长时按字符切分，仍保留同一页和结构块定位。
    let mut bounded = Vec::new();
    for chunk in chunks {
        if chunk.content.chars().count() <= MAX_DOCUMENT_CHUNK_CHARS {
            bounded.push(chunk);
            continue;
        }
        let characters: Vec<char> = chunk.content.chars().collect();
        for (part_index, part) in characters.chunks(MAX_DOCUMENT_CHUNK_CHARS).enumerate() {
            bounded.push(DocumentChunkInput {
                page: chunk.page,
                block_start: chunk.block_start,
                block_end: chunk.block_end,
                heading_path: if part_index == 0 {
                    chunk.heading_path.clone()
                } else {
                    format!("{} / 续", chunk.heading_path)
                },
                content: part.iter().collect(),
            });
        }
    }

    log::debug!(
        target: "document_index",
        "文档 chunk 完成：path={} type={} blocks={} chunks={}",
        document.path,
        document.file_type,
        extraction.blocks.len(),
        bounded.len()
    );
    bounded
}

/** 写入单个文档的 chunk/FTS；内容 hash 和解析器版本未变时跳过。 */
fn upsert_document_index(
    transaction: &rusqlite::Transaction<'_>,
    document: &WorkspaceDocument,
    extraction: &DocumentTextExtraction,
) -> Result<(), String> {
    let extractor_version = crate::storage::DOCUMENT_EXTRACTOR_VERSION;
    let previous: Option<(String, String)> = transaction
        .query_row(
            "SELECT content_hash, extractor_version FROM document_index_state WHERE document_id=?1",
            [&document.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|_| "无法读取文档索引状态。")?;
    if previous.as_ref().is_some_and(|(hash, version)| {
        hash == &extraction.content_hash && version == extractor_version
    }) {
        return Ok(());
    }

    transaction
        .execute(
            "DELETE FROM document_chunk_fts WHERE document_id=?1",
            [&document.id],
        )
        .map_err(|error| format!("无法删除旧文档 FTS：{error}"))?;
    transaction
        .execute(
            "DELETE FROM document_chunks WHERE document_id=?1",
            [&document.id],
        )
        .map_err(|error| format!("无法删除旧文档 chunk：{error}"))?;

    let updated_at_unix = crate::storage::parse_index_datetime(&document.updated_at);
    for (index, chunk) in chunk_document_blocks(document, extraction)
        .into_iter()
        .enumerate()
    {
        let chunk_id = format!("document:{}:{index}", document.id);
        transaction
            .execute(
                "INSERT OR REPLACE INTO document_chunks
                 (chunk_id, document_id, knowledge_base_id, document_title, document_path, chunk_index, page, block_start, block_end, heading_path, content, content_hash, updated_at_unix)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    &chunk_id,
                    &document.id,
                    &document.knowledge_base_id,
                    &document.title,
                    &document.path,
                    index as i64,
                    chunk.page.map(|page| page as i64),
                    chunk.block_start as i64,
                    chunk.block_end as i64,
                    &chunk.heading_path,
                    &chunk.content,
                    &extraction.content_hash,
                    updated_at_unix,
                ],
            )
            .map_err(|error| format!("无法写入文档 chunk：{error}"))?;
        transaction
            .execute(
                "INSERT INTO document_chunk_fts
                 (chunk_id, document_id, knowledge_base_id, title_tokens, path_tokens, heading_tokens, body_tokens)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    &chunk_id,
                    &document.id,
                    &document.knowledge_base_id,
                    crate::storage::build_fts_tokens(&document.title),
                    crate::storage::build_fts_tokens(&document.path),
                    crate::storage::build_fts_tokens(&chunk.heading_path),
                    crate::storage::build_fts_tokens(&chunk.content),
                ],
            )
            .map_err(|error| format!("无法写入文档 FTS：{error}"))?;
    }

    transaction
        .execute(
            "INSERT OR REPLACE INTO document_index_state
             (document_id, knowledge_base_id, content_hash, extractor_version, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                &document.id,
                &document.knowledge_base_id,
                &extraction.content_hash,
                extractor_version,
                &document.updated_at
            ],
        )
        .map_err(|error| format!("无法写入文档索引状态：{error}"))?;
    Ok(())
}

/** 在当前授权 scope 内检索已索引的 DOCX/PDF chunk，返回带页码/块号的引用。 */
pub(crate) fn search_documents(
    app: &AppHandle,
    snapshot: &WorkspaceSnapshot,
    knowledge_base_ids: &[String],
    query: &str,
    limit: usize,
    filters: &NoteSearchFilters,
) -> Result<Vec<crate::domain::Citation>, String> {
    if knowledge_base_ids.is_empty() || query.trim().is_empty() {
        return Ok(Vec::new());
    }
    // fileTypes 只包含 markdown 时，文档检索没有可检索范围。
    if !filters.file_types.is_empty()
        && !filters
            .file_types
            .iter()
            .any(|file_type| matches!(file_type.as_str(), "pdf" | "docx"))
    {
        return Ok(Vec::new());
    }
    let connection = open_database(app)?;
    search_documents_with_connection(
        &connection,
        snapshot,
        knowledge_base_ids,
        query,
        limit,
        filters,
    )
}

/** 在调用方连接上执行文档 FTS，便于单元测试直接复用 SQL 过滤。 */
pub(crate) fn search_documents_with_connection(
    connection: &rusqlite::Connection,
    snapshot: &WorkspaceSnapshot,
    knowledge_base_ids: &[String],
    query: &str,
    limit: usize,
    filters: &NoteSearchFilters,
) -> Result<Vec<crate::domain::Citation>, String> {
    let tokens = crate::storage::tokenize_text(query);
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let fts_query = tokens
        .iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let placeholders = knowledge_base_ids
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");
    let mut clauses = vec![
        "document_chunk_fts MATCH ?1".to_owned(),
        format!("c.knowledge_base_id IN ({placeholders})"),
    ];
    let mut bindings: Vec<rusqlite::types::Value> =
        vec![rusqlite::types::Value::Text(fts_query)];
    bindings.extend(
        knowledge_base_ids
            .iter()
            .map(|id| rusqlite::types::Value::Text(id.clone())),
    );
    if !filters.path_prefix.is_empty() {
        let like_prefix = filters
            .path_prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        clauses.push("(c.document_path = ? OR c.document_path LIKE ? ESCAPE '\\')".to_owned());
        bindings.push(rusqlite::types::Value::Text(filters.path_prefix.clone()));
        bindings.push(rusqlite::types::Value::Text(format!("{like_prefix}/%")));
    }
    if let Some(after) = filters.updated_after_unix {
        clauses.push("c.updated_at_unix >= ?".to_owned());
        bindings.push(rusqlite::types::Value::Integer(after));
    }
    if let Some(before) = filters.updated_before_unix {
        clauses.push("c.updated_at_unix <= ?".to_owned());
        bindings.push(rusqlite::types::Value::Integer(before));
    }
    let sql = format!(
        "SELECT c.chunk_id, c.document_id, c.knowledge_base_id, c.document_title,
                c.document_path, c.page, c.block_start, c.heading_path, c.content,
                bm25(document_chunk_fts, 8.0, 4.0, 6.0, 1.0)
         FROM document_chunk_fts
         JOIN document_chunks c ON c.chunk_id = document_chunk_fts.chunk_id
         WHERE {}
         ORDER BY bm25(document_chunk_fts, 8.0, 4.0, 6.0, 1.0)
         LIMIT 128",
        clauses.join(" AND ")
    );
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| format!("无法准备文档 FTS 检索：{error}"))?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(bindings), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, f64>(9)?,
            ))
        })
        .map_err(|error| format!("无法执行文档 FTS 检索：{error}"))?;

    let mut citations = Vec::new();
    let mut seen_chunks = HashSet::new();
    for row in rows {
        let (
            chunk_id,
            document_id,
            knowledge_base_id,
            title,
            path,
            page,
            block_start,
            heading_path,
            content,
            rank,
        ) = row.map_err(|error| format!("无法读取文档检索结果：{error}"))?;
        if !seen_chunks.insert(chunk_id) {
            continue;
        }
        let Some(document) = snapshot.documents.iter().find(|document| {
            document.id == document_id && document.knowledge_base_id == knowledge_base_id
        }) else {
            continue;
        };
        if !filters.file_types.is_empty()
            && !filters
                .file_types
                .iter()
                .any(|file_type| file_type == &document.file_type)
        {
            continue;
        }
        let Some(knowledge_base) = snapshot
            .knowledge_bases
            .iter()
            .find(|knowledge_base| knowledge_base.id == knowledge_base_id)
        else {
            continue;
        };
        citations.push(crate::domain::Citation {
            knowledge_base_id,
            knowledge_base_name: knowledge_base.name.clone(),
            note_id: document_id,
            title,
            path,
            snippet: extract_chunk_snippet(&content, &tokens),
            score: 1.0 / (1.0 + rank.abs()),
            location: Some(
                page.map(|page| format!("第 {page} 页"))
                    .unwrap_or_else(|| format!("结构块 {block_start}")),
            ),
            kind: None,
            url: None,
            published_at: None,
            source_type: Some("document".to_owned()),
            page: page.map(|page| page as usize),
            block_index: Some(block_start as usize),
            heading_path: (!heading_path.is_empty()).then_some(heading_path),
        });
    }
    citations.sort_by(|left, right| right.score.total_cmp(&left.score));
    citations.truncate(limit);
    Ok(citations)
}

fn upsert_note_index(transaction: &rusqlite::Transaction<'_>, note: &Note) -> Result<(), String> {
    let signature = hash_content(
        &serde_json::to_string(&(
            &note.title,
            &note.path,
            &note.tags,
            &note.content_hash,
            &note.updated_at,
        ))
        .map_err(|_| "无法计算笔记索引签名。")?,
    );
    let previous: Option<String> = transaction
        .query_row(
            "SELECT signature FROM semantic_note_signatures WHERE note_id=?1",
            [&note.id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| "无法读取笔记索引签名。")?;
    if previous.as_deref() == Some(signature.as_str())
        && transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM note_chunks WHERE note_id=?1)",
                [&note.id],
                |r| r.get::<_, bool>(0),
            )
            .unwrap_or(false)
    {
        return Ok(());
    }
    transaction
        .execute(
            "INSERT OR REPLACE INTO semantic_note_signatures(note_id,signature) VALUES (?1,?2)",
            params![note.id, signature],
        )
        .map_err(|_| "无法保存笔记索引签名。")?;

    transaction
        .execute("DELETE FROM note_fts WHERE note_id = ?1", params![&note.id])
        .map_err(|error| format!("无法更新 FTS 索引：{error}"))?;
    transaction
        .execute(
            "DELETE FROM note_chunk_fts WHERE note_id = ?1",
            params![&note.id],
        )
        .map_err(|error| format!("无法更新 chunk FTS 索引：{error}"))?;
    transaction
        .execute(
            "DELETE FROM note_chunks WHERE note_id = ?1",
            params![&note.id],
        )
        .map_err(|error| format!("无法更新 chunk 索引：{error}"))?;
    transaction
        .execute(
            "INSERT OR REPLACE INTO notes (id, knowledge_base_id, title, path, file_type, updated_at_unix, content_hash, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                &note.id,
                &note.knowledge_base_id,
                &note.title,
                &note.path,
                "markdown",
                crate::storage::parse_index_datetime(&note.updated_at),
                &note.content_hash,
                &note.updated_at
            ],
        )
        .map_err(|error| format!("无法写入笔记索引：{error}"))?;
    transaction
        .execute(
            "DELETE FROM note_tags WHERE note_id = ?1",
            params![&note.id],
        )
        .map_err(|error| format!("无法更新笔记标签索引：{error}"))?;
    for tag in &note.tags {
        transaction
            .execute(
                "INSERT OR IGNORE INTO note_tags (note_id, knowledge_base_id, tag) VALUES (?1, ?2, ?3)",
                params![&note.id, &note.knowledge_base_id, tag],
            )
            .map_err(|error| format!("无法写入笔记标签索引：{error}"))?;
    }
    transaction
        .execute(
            "INSERT INTO note_fts (note_id, knowledge_base_id, title, path, body) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![&note.id, &note.knowledge_base_id, &note.title, &note.path, &note.content],
        )
        .map_err(|error| format!("无法写入 FTS 索引：{error}"))?;

    for chunk in crate::storage::chunk_markdown(&note.content, &note.tags) {
        index_semantic_input(transaction, note, &chunk)?;
        let chunk_id = format!("{}:{}", note.id, chunk.chunk_index);
        let heading_tokens = crate::storage::build_fts_tokens(&chunk.heading_path);
        let body_tokens =
            crate::storage::build_fts_tokens(&format!("{} {}", note.tags.join(" "), chunk.content));

        transaction
            .execute(
                "INSERT OR REPLACE INTO note_chunks (chunk_id, note_id, knowledge_base_id, chunk_index, heading_path, content, line_start, line_end, content_hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    &chunk_id,
                    &note.id,
                    &note.knowledge_base_id,
                    chunk.chunk_index as i64,
                    &chunk.heading_path,
                    &chunk.content,
                    chunk.line_start as i64,
                    chunk.line_end as i64,
                    &note.content_hash,
                ],
            )
            .map_err(|error| format!("无法写入 chunk 索引：{error}"))?;
        transaction
            .execute(
                "INSERT INTO note_chunk_fts (chunk_id, note_id, knowledge_base_id, title_tokens, path_tokens, heading_tokens, body_tokens) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    &chunk_id,
                    &note.id,
                    &note.knowledge_base_id,
                    crate::storage::build_fts_tokens(&note.title),
                    crate::storage::build_fts_tokens(&note.path),
                    heading_tokens,
                    body_tokens,
                ],
            )
            .map_err(|error| format!("无法写入 chunk FTS 索引：{error}"))?;
    }

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
                        unsupported_files: Vec::new(),
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
    filters: &NoteSearchFilters,
) -> Result<Vec<crate::domain::Citation>, String> {
    let limit = limit.max(1);
    let selected_ids: HashSet<&str> = knowledge_base_ids.iter().map(String::as_str).collect();

    if selected_ids.is_empty() {
        return Ok(Vec::new());
    }

    let query_tokens = crate::storage::tokenize_text(prompt);
    let mut citations = Vec::new();

    if !query_tokens.is_empty() {
        citations =
            search_note_chunk_fts(app, snapshot, &selected_ids, &query_tokens, None, filters)?;

        // 长句 OR 查询失败时，用最高权重的前三个 token 做一次更短查询。
        if citations.is_empty() {
            citations = search_note_chunk_fts(
                app,
                snapshot,
                &selected_ids,
                &query_tokens,
                Some(3),
                filters,
            )?;
        }
    }

    let mut seen_note_ids: HashSet<String> = citations
        .iter()
        .map(|citation| citation.note_id.clone())
        .collect();

    // 快照兜底只在 FTS 两轮都失败后启用，避免低质量子串结果挤掉索引命中。
    let fallback_citations = if citations.is_empty() {
        search_snapshot_notes(snapshot, &selected_ids, prompt, filters)
    } else {
        Vec::new()
    };

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

/** 执行 chunk FTS 查询，每个笔记只保留排名最高的 chunk。 */
pub(crate) fn search_note_chunk_fts(
    app: &AppHandle,
    snapshot: &WorkspaceSnapshot,
    selected_ids: &HashSet<&str>,
    query_tokens: &[String],
    token_limit: Option<usize>,
    filters: &NoteSearchFilters,
) -> Result<Vec<crate::domain::Citation>, String> {
    let connection = open_database(app)?;
    search_note_chunk_fts_with_connection(
        &connection,
        snapshot,
        selected_ids,
        query_tokens,
        token_limit,
        filters,
    )
}

/** 在调用方提供的事务/连接上执行 chunk FTS，便于复用和测试 SQL 过滤。 */
pub(crate) fn search_note_chunk_fts_with_connection(
    connection: &rusqlite::Connection,
    snapshot: &WorkspaceSnapshot,
    selected_ids: &HashSet<&str>,
    query_tokens: &[String],
    token_limit: Option<usize>,
    filters: &NoteSearchFilters,
) -> Result<Vec<crate::domain::Citation>, String> {
    let mut seen = HashSet::new();
    Ok(search_note_chunk_candidates(
        connection,
        snapshot,
        selected_ids,
        query_tokens,
        token_limit,
        filters,
    )?
    .into_iter()
    .filter(|item| seen.insert(item.citation.note_id.clone()))
    .map(|item| item.citation)
    .collect())
}

/** 把 chunk 定位转成用户可读引用位置。 */
pub(crate) fn format_citation_location(
    heading_path: &str,
    line_start: i64,
    line_end: i64,
) -> String {
    let display_heading = if heading_path.is_empty() {
        "开头"
    } else {
        heading_path
    };
    format!("{display_heading} · L{line_start}-L{line_end}")
}

/** 从 chunk 原文提取命中词附近片段，保证引用不是 FTS token 拼接结果。 */
pub(crate) fn extract_chunk_snippet(content: &str, terms: &[String]) -> String {
    if content.trim().is_empty() {
        return "命中笔记元数据，暂无可展示片段。".to_owned();
    }

    let lower_content = content.to_lowercase();
    let hit_byte = terms
        .iter()
        .filter_map(|term| lower_content.find(term.as_str()))
        .min();
    let Some(hit_byte) = hit_byte else {
        return content
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("命中该笔记，但暂无可展示片段。")
            .chars()
            .take(240)
            .collect();
    };

    let hit_char = lower_content[..hit_byte].chars().count();
    let start_char = hit_char.saturating_sub(40);
    let chars: Vec<char> = content.chars().collect();
    let end_char = (start_char + 240).min(chars.len());
    let mut snippet = String::new();
    if start_char > 0 {
        snippet.push('…');
    }
    snippet.extend(chars[start_char..end_char].iter());
    if end_char < chars.len() {
        snippet.push('…');
    }
    snippet
}

/** 快照级子串检索，作为 FTS5 无命中或中文分词不足时的本地降级方案。 */
pub(crate) fn search_snapshot_notes(
    snapshot: &WorkspaceSnapshot,
    selected_ids: &HashSet<&str>,
    prompt: &str,
    filters: &NoteSearchFilters,
) -> Vec<crate::domain::Citation> {
    let prompt_terms = crate::storage::tokenize_text(prompt);
    let mut citations: Vec<crate::domain::Citation> = Vec::new();
    for note in snapshot
        .notes
        .iter()
        .filter(|note| selected_ids.contains(note.knowledge_base_id.as_str()))
        .filter(|note| note_matches_filters(note, filters))
        .filter(|note| !crate::storage::is_root_project_instruction_path(&note.path))
    {
        let knowledge_base = match snapshot
            .knowledge_bases
            .iter()
            .find(|item| item.id == note.knowledge_base_id)
        {
            Some(knowledge_base) => knowledge_base,
            None => continue,
        };

        for chunk in crate::storage::chunk_markdown(&note.content, &note.tags) {
            let searchable_text = format!(
                "{} {} {} {} {}",
                note.title,
                note.path,
                note.tags.join(" "),
                chunk.heading_path,
                chunk.content
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
                continue;
            }

            citations.push(crate::domain::Citation {
                knowledge_base_id: note.knowledge_base_id.clone(),
                knowledge_base_name: knowledge_base.name.clone(),
                note_id: note.id.clone(),
                title: note.title.clone(),
                path: note.path.clone(),
                snippet: extract_chunk_snippet(&chunk.content, &prompt_terms),
                score,
                location: Some(format_citation_location(
                    &chunk.heading_path,
                    chunk.line_start as i64,
                    chunk.line_end as i64,
                )),
                kind: None,
                url: None,
                published_at: None,
                ..Default::default()
            });
        }
    }

    citations.sort_by(|left, right| right.score.total_cmp(&left.score));
    let mut seen_note_ids: HashSet<String> = HashSet::new();
    citations.retain(|citation| seen_note_ids.insert(citation.note_id.clone()));
    citations.truncate(4);
    citations
}

#[cfg(test)]
mod tests {
    use super::{
        apply_index_to_transaction, search_note_chunk_fts_with_connection, IndexMode,
        NoteSearchFilters,
    };
    use crate::domain::{KnowledgeBase, Note, WorkspaceSnapshot};
    use crate::storage::{ensure_database_schema, hash_content};
    use rusqlite::Connection;
    use std::collections::HashSet;
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

    /** 构造带路径、标签和真实更新时间的笔记，便于覆盖结构化过滤。 */
    fn filtered_note(id: &str, path: &str, tags: &[&str], updated_at: &str, content: &str) -> Note {
        let mut note = test_note(id, content);
        note.path = path.to_owned();
        note.tags = tags.iter().map(|tag| (*tag).to_owned()).collect();
        note.updated_at = updated_at.to_owned();
        note
    }

    /** 旧库迁移必须补齐过滤列，并只回填可解析的真实时间。 */
    #[test]
    fn retrieval_filter_columns_migrate_legacy_notes_idempotently() {
        let dir = tempdir().unwrap();
        let connection = Connection::open(dir.path().join("legacy.sqlite3")).unwrap();
        connection
            .execute_batch(
                r#"
                CREATE TABLE notes (
                  id TEXT PRIMARY KEY,
                  knowledge_base_id TEXT NOT NULL,
                  title TEXT NOT NULL,
                  path TEXT NOT NULL,
                  content_hash TEXT NOT NULL,
                  updated_at TEXT NOT NULL
                );
                INSERT INTO notes VALUES ('note-old', 'kb-a', '旧', 'old.md', 'hash', '2026/01/02 10:00');
                INSERT INTO notes VALUES ('note-placeholder', 'kb-a', '新', 'new.md', 'hash', '刚刚');
                "#,
            )
            .unwrap();

        crate::storage::ensure_retrieval_filter_columns(&connection).unwrap();
        crate::storage::ensure_retrieval_filter_columns(&connection).unwrap();

        let filed_time: Option<i64> = connection
            .query_row(
                "SELECT updated_at_unix FROM notes WHERE id = 'note-old'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let placeholder_time: Option<i64> = connection
            .query_row(
                "SELECT updated_at_unix FROM notes WHERE id = 'note-placeholder'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let file_type: String = connection
            .query_row(
                "SELECT file_type FROM notes WHERE id = 'note-old'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(filed_time.is_some());
        assert_eq!(placeholder_time, None);
        assert_eq!(file_type, "markdown");
    }

    /** 笔记 upsert 必须同步重建标签和时间索引，删除时也必须一起清理。 */
    #[test]
    fn note_index_maintains_tags_and_filter_metadata() {
        let dir = tempdir().unwrap();
        let database_path = dir.path().join("filters-index.sqlite3");
        let mut connection = Connection::open(&database_path).unwrap();
        ensure_database_schema(&connection, &database_path).unwrap();
        let note = filtered_note(
            "note-a",
            "docs/architecture/sync.md",
            &["sync", "产品"],
            "2026/01/02 10:00",
            "# 数据同步\n\n冲突处理\n",
        );

        let transaction = connection.transaction().unwrap();
        apply_index_to_transaction(
            &transaction,
            &test_snapshot(vec![note.clone()]),
            IndexMode::Upsert,
        )
        .unwrap();
        transaction.commit().unwrap();

        let tag_count: i64 = connection
            .query_row(
                "SELECT count(*) FROM note_tags WHERE note_id = 'note-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let (file_type, updated_at_unix): (String, Option<i64>) = connection
            .query_row(
                "SELECT file_type, updated_at_unix FROM notes WHERE id = 'note-a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(tag_count, 2);
        assert_eq!(file_type, "markdown");
        assert!(updated_at_unix.is_some());

        let transaction = connection.transaction().unwrap();
        super::remove_note_ids_from_transaction(&transaction, &["note-a".to_owned()]).unwrap();
        transaction.commit().unwrap();
        let tag_count: i64 = connection
            .query_row(
                "SELECT count(*) FROM note_tags WHERE note_id = 'note-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tag_count, 0);
    }

    /** 结构化过滤必须下推 SQL：只有同时满足关键词和元数据条件的笔记返回。 */
    #[test]
    fn chunk_fts_applies_structured_filters_in_sql() {
        let dir = tempdir().unwrap();
        let database_path = dir.path().join("filters-query.sqlite3");
        let mut connection = Connection::open(&database_path).unwrap();
        ensure_database_schema(&connection, &database_path).unwrap();
        let matching = filtered_note(
            "note-a",
            "docs/architecture/sync.md",
            &["sync"],
            "2026/01/02 10:00",
            "# 数据同步\n\n冲突处理\n",
        );
        let excluded = filtered_note(
            "note-b",
            "meeting.md",
            &["meeting"],
            "2025/01/01 10:00",
            "# 数据同步\n\n会议细节\n",
        );
        let transaction = connection.transaction().unwrap();
        apply_index_to_transaction(
            &transaction,
            &test_snapshot(vec![matching.clone(), excluded.clone()]),
            IndexMode::ReplaceAll,
        )
        .unwrap();
        transaction.commit().unwrap();

        let selected_ids: HashSet<&str> = ["kb-a"].into_iter().collect();
        let query_tokens = crate::storage::tokenize_text("数据同步");
        let run = |filters: NoteSearchFilters| {
            search_note_chunk_fts_with_connection(
                &connection,
                &test_snapshot(vec![matching.clone(), excluded.clone()]),
                &selected_ids,
                &query_tokens,
                None,
                &filters,
            )
            .unwrap()
            .into_iter()
            .map(|citation| citation.note_id)
            .collect::<Vec<_>>()
        };

        let after = super::parse_note_search_datetime("2026-01-01", false)
            .unwrap()
            .unwrap();
        let before = super::parse_note_search_datetime("2025-12-31", true)
            .unwrap()
            .unwrap();
        let path_only = NoteSearchFilters {
            path_prefix: "docs".to_owned(),
            ..Default::default()
        };
        let tags_only = NoteSearchFilters {
            tags: vec!["meeting".to_owned(), "sync".to_owned()],
            ..Default::default()
        };
        let time_after = NoteSearchFilters {
            updated_after_unix: Some(after),
            ..Default::default()
        };
        let time_before = NoteSearchFilters {
            updated_before_unix: Some(before),
            ..Default::default()
        };
        let markdown_only = NoteSearchFilters {
            file_types: vec!["markdown".to_owned()],
            ..Default::default()
        };

        assert_eq!(run(NoteSearchFilters::default()).len(), 2);
        assert_eq!(run(path_only), vec!["note-a".to_owned()]);
        assert_eq!(run(tags_only).len(), 2);
        assert_eq!(run(time_after), vec!["note-a".to_owned()]);
        assert_eq!(run(time_before), vec!["note-b".to_owned()]);
        assert_eq!(run(markdown_only).len(), 2);
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

    /** chunk 索引必须和 notes/note_fts 同步维护，中文无空格词能直接命中对应章节。 */
    #[test]
    fn chunk_index_roundtrip_matches_chinese_heading_content() {
        let dir = tempdir().unwrap();
        let database_path = dir.path().join("chunk-roundtrip.sqlite3");
        let mut connection = Connection::open(&database_path).unwrap();
        ensure_database_schema(&connection, &database_path).unwrap();

        let content = "# 技术方案\n\n## 数据同步\n\n本地写入必须先做冲突处理。\n";
        let snapshot = test_snapshot(vec![test_note("note-a", content)]);
        let transaction = connection.transaction().unwrap();
        apply_index_to_transaction(&transaction, &snapshot, IndexMode::ReplaceAll).unwrap();
        transaction.commit().unwrap();

        let chunk_count: i64 = connection
            .query_row("SELECT count(*) FROM note_chunks", [], |row| row.get(0))
            .unwrap();
        let fts_count: i64 = connection
            .query_row("SELECT count(*) FROM note_chunk_fts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(chunk_count, 2);
        assert_eq!(fts_count, 2);

        let heading: String = connection
            .query_row(
                "SELECT c.heading_path
                 FROM note_chunk_fts f
                 JOIN note_chunks c ON c.chunk_id = f.chunk_id
                 WHERE note_chunk_fts MATCH '\"冲突\"'
                 LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(heading, "技术方案 / 数据同步");
    }

    /** chunk upsert 必须覆盖旧章节，不能让同一笔记留下重复索引。 */
    #[test]
    fn upsert_note_index_replaces_old_chunks() {
        let dir = tempdir().unwrap();
        let database_path = dir.path().join("chunk-upsert.sqlite3");
        let mut connection = Connection::open(&database_path).unwrap();
        ensure_database_schema(&connection, &database_path).unwrap();

        let first = test_note("note-a", "# 旧标题\n\n旧内容\n");
        let transaction = connection.transaction().unwrap();
        apply_index_to_transaction(&transaction, &test_snapshot(vec![first]), IndexMode::Upsert)
            .unwrap();
        transaction.commit().unwrap();

        let next = test_note("note-a", "# 新标题\n\n新内容\n");
        let transaction = connection.transaction().unwrap();
        apply_index_to_transaction(&transaction, &test_snapshot(vec![next]), IndexMode::Upsert)
            .unwrap();
        transaction.commit().unwrap();

        let old_hits: i64 = connection
            .query_row(
                "SELECT count(*) FROM note_chunk_fts WHERE body_tokens MATCH '\"旧\"'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let new_hits: i64 = connection
            .query_row(
                "SELECT count(*) FROM note_chunk_fts WHERE body_tokens MATCH '\"新\"'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old_hits, 0);
        assert_eq!(new_hits, 1);
    }

    /** FTS miss 时快照兜底仍应返回 chunk 定位，而不是退回整篇笔记。 */
    #[test]
    fn snapshot_fallback_returns_chunk_location() {
        let snapshot = test_snapshot(vec![test_note(
            "note-a",
            "# 研究方法\n\n用户访谈覆盖了本地检索体验。\n",
        )]);
        let selected_ids: std::collections::HashSet<&str> = ["kb-a"].into_iter().collect();

        let citations = super::search_snapshot_notes(
            &snapshot,
            &selected_ids,
            "访谈 检索",
            &super::NoteSearchFilters::default(),
        );

        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0].location.as_deref(), Some("研究方法 · L1-L3"));
        assert!(citations[0].snippet.contains("用户访谈"));
    }

    /** FTS 空结果后的快照兜底也必须遵守结构化过滤，不能绕过元数据边界。 */
    #[test]
    fn snapshot_fallback_applies_structured_filters() {
        let matching = filtered_note(
            "note-a",
            "docs/sync.md",
            &["sync"],
            "2026/01/02 10:00",
            "# 数据同步\n\n冲突处理\n",
        );
        let excluded = filtered_note(
            "note-b",
            "meeting.md",
            &["meeting"],
            "2025/01/01 10:00",
            "# 数据同步\n\n会议细节\n",
        );
        let selected_ids: HashSet<&str> = ["kb-a"].into_iter().collect();
        let filters = NoteSearchFilters {
            path_prefix: "docs".to_owned(),
            tags: vec!["sync".to_owned()],
            ..Default::default()
        };

        let citations = super::search_snapshot_notes(
            &test_snapshot(vec![matching, excluded]),
            &selected_ids,
            "数据同步",
            &filters,
        );

        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0].note_id, "note-a");
    }

    /** 删除单篇笔记时，chunk 和 chunk FTS 必须一起清理。 */
    #[test]
    fn remove_note_ids_cleans_chunk_index() {
        let dir = tempdir().unwrap();
        let database_path = dir.path().join("chunk-remove.sqlite3");
        let mut connection = Connection::open(&database_path).unwrap();
        ensure_database_schema(&connection, &database_path).unwrap();

        let snapshot = test_snapshot(vec![test_note("note-a", "# 标题\n\n内容\n")]);
        let transaction = connection.transaction().unwrap();
        apply_index_to_transaction(&transaction, &snapshot, IndexMode::ReplaceAll).unwrap();
        transaction.commit().unwrap();

        let transaction = connection.transaction().unwrap();
        super::remove_note_ids_from_transaction(&transaction, &["note-a".to_owned()]).unwrap();
        transaction.commit().unwrap();

        let chunk_count: i64 = connection
            .query_row("SELECT count(*) FROM note_chunks", [], |row| row.get(0))
            .unwrap();
        let fts_count: i64 = connection
            .query_row("SELECT count(*) FROM note_chunk_fts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(chunk_count, 0);
        assert_eq!(fts_count, 0);
    }

    /** 写入包含标题和正文的最小 DOCX，供文档 chunk 索引测试使用。 */
    fn write_indexable_docx(path: &std::path::Path) {
        use std::io::Write;
        use zip::write::SimpleFileOptions;
        use zip::ZipWriter;

        let document_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>技术方案</w:t></w:r></w:p>
<w:p><w:r><w:t>本地写入必须先做冲突处理。</w:t></w:r></w:p>
</w:body></w:document>"#;
        let styles_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:style>
</w:styles>"#;

        let file = std::fs::File::create(path).unwrap();
        let options = SimpleFileOptions::default();
        let mut archive = ZipWriter::new(file);
        archive.start_file("word/document.xml", options).unwrap();
        archive.write_all(document_xml.as_bytes()).unwrap();
        archive.start_file("word/styles.xml", options).unwrap();
        archive.write_all(styles_xml.as_bytes()).unwrap();
        archive.finish().unwrap();
    }

    /** 构造带一个 DOCX 文档的快照；知识库根目录指向临时目录。 */
    fn document_snapshot(root: &std::path::Path, file_type: &str, path: &str) -> WorkspaceSnapshot {
        let mut snapshot = test_snapshot(Vec::new());
        snapshot.knowledge_bases[0].path = root.to_string_lossy().to_string();
        snapshot.documents.push(crate::domain::WorkspaceDocument {
            id: "doc-a".to_owned(),
            knowledge_base_id: "kb-a".to_owned(),
            title: "技术方案".to_owned(),
            path: path.to_owned(),
            file_type: file_type.to_owned(),
            updated_at: "2026/01/02 10:00".to_owned(),
            content_hash: String::new(),
            content: None,
            preview_available: true,
        });
        snapshot
    }

    /** DOCX 索引后应能按正文命中，并带出块号与标题路径引用。 */
    #[test]
    fn document_index_roundtrip_searches_docx_chunks() {
        let dir = tempdir().unwrap();
        write_indexable_docx(&dir.path().join("spec.docx"));
        let database_path = dir.path().join("document-index.sqlite3");
        let mut connection = Connection::open(&database_path).unwrap();
        ensure_database_schema(&connection, &database_path).unwrap();

        let snapshot = document_snapshot(dir.path(), "docx", "spec.docx");
        let transaction = connection.transaction().unwrap();
        apply_index_to_transaction(&transaction, &snapshot, IndexMode::ReplaceAll).unwrap();
        transaction.commit().unwrap();

        let chunk_count: i64 = connection
            .query_row("SELECT count(*) FROM document_chunks", [], |row| row.get(0))
            .unwrap();
        assert!(chunk_count >= 1, "DOCX 正文应写入 document_chunks");

        let citations = super::search_documents_with_connection(
            &connection,
            &snapshot,
            &["kb-a".to_owned()],
            "冲突处理",
            4,
            &NoteSearchFilters::default(),
        )
        .unwrap();

        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0].note_id, "doc-a");
        assert_eq!(citations[0].source_type.as_deref(), Some("document"));
        assert!(citations[0].snippet.contains("冲突处理"));
        assert!(citations[0].location.as_deref().unwrap().contains("结构块"));
        assert_eq!(citations[0].heading_path.as_deref(), Some("技术方案"));
    }

    /** fileTypes 只允许 markdown 时，文档检索必须返回空，不能绕过过滤。 */
    #[test]
    fn document_search_respects_file_type_filters() {
        let dir = tempdir().unwrap();
        write_indexable_docx(&dir.path().join("spec.docx"));
        let database_path = dir.path().join("document-filter.sqlite3");
        let mut connection = Connection::open(&database_path).unwrap();
        ensure_database_schema(&connection, &database_path).unwrap();

        let snapshot = document_snapshot(dir.path(), "docx", "spec.docx");
        let transaction = connection.transaction().unwrap();
        apply_index_to_transaction(&transaction, &snapshot, IndexMode::ReplaceAll).unwrap();
        transaction.commit().unwrap();

        let markdown_only = NoteSearchFilters {
            file_types: vec!["markdown".to_owned()],
            ..Default::default()
        };
        assert!(super::search_documents_with_connection(
            &connection,
            &snapshot,
            &["kb-a".to_owned()],
            "冲突处理",
            4,
            &markdown_only,
        )
        .unwrap()
        .is_empty());

        let docx_only = NoteSearchFilters {
            file_types: vec!["docx".to_owned()],
            ..Default::default()
        };
        assert_eq!(
            super::search_documents_with_connection(
                &connection,
                &snapshot,
                &["kb-a".to_owned()],
                "冲突处理",
                4,
                &docx_only,
            )
            .unwrap()
            .len(),
            1
        );
    }

    /** 移除知识库授权时，文档 chunk/FTS/索引状态必须一并清理。 */
    #[test]
    fn removing_knowledge_base_cleans_document_index() {
        let dir = tempdir().unwrap();
        write_indexable_docx(&dir.path().join("spec.docx"));
        let database_path = dir.path().join("document-remove.sqlite3");
        let mut connection = Connection::open(&database_path).unwrap();
        ensure_database_schema(&connection, &database_path).unwrap();

        let snapshot = document_snapshot(dir.path(), "docx", "spec.docx");
        let transaction = connection.transaction().unwrap();
        apply_index_to_transaction(&transaction, &snapshot, IndexMode::ReplaceAll).unwrap();
        transaction.commit().unwrap();

        let transaction = connection.transaction().unwrap();
        super::remove_knowledge_base_from_transaction(&transaction, "kb-a").unwrap();
        transaction.commit().unwrap();

        for table in [
            "document_chunks",
            "document_chunk_fts",
            "document_index_state",
        ] {
            let count: i64 = connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table} 应被清理");
        }
    }
}
