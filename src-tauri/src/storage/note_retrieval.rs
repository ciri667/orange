//! 两路召回复用相同的 SQL 元数据过滤，并保留 chunk 身份供融合。
use super::*;
#[derive(Clone, Debug)]
pub(crate) struct NoteChunkCandidate {
    pub chunk_id: String,
    pub citation: crate::domain::Citation,
}

pub(crate) fn append_note_filter_sql(
    filters: &NoteSearchFilters,
    where_clauses: &mut Vec<String>,
    bind_values: &mut Vec<rusqlite::types::Value>,
) {
    if !filters.file_types.is_empty() {
        let placeholders = filters
            .file_types
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");
        where_clauses.push(format!("n.file_type IN ({placeholders})"));
        bind_values.extend(
            filters
                .file_types
                .iter()
                .map(|file_type| rusqlite::types::Value::Text(file_type.clone())),
        );
    }

    if !filters.path_prefix.is_empty() {
        where_clauses.push("(n.path = ? OR n.path LIKE ? ESCAPE '\\')".to_owned());
        bind_values.push(rusqlite::types::Value::Text(filters.path_prefix.clone()));
        let like_prefix = filters
            .path_prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        bind_values.push(rusqlite::types::Value::Text(format!("{like_prefix}/%")));
    }

    if let Some(updated_after) = filters.updated_after_unix {
        where_clauses.push("n.updated_at_unix >= ?".to_owned());
        bind_values.push(rusqlite::types::Value::Integer(updated_after));
    }

    if let Some(updated_before) = filters.updated_before_unix {
        where_clauses.push("n.updated_at_unix <= ?".to_owned());
        bind_values.push(rusqlite::types::Value::Integer(updated_before));
    }

    if !filters.tags.is_empty() {
        let placeholders = filters
            .tags
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");
        where_clauses.push(format!(
            "EXISTS (SELECT 1 FROM note_tags t WHERE t.note_id = n.id AND t.tag IN ({placeholders}))"
        ));
        bind_values.extend(
            filters
                .tags
                .iter()
                .map(|tag| rusqlite::types::Value::Text(tag.clone())),
        );
    }
}

pub(crate) fn search_note_chunk_candidates(
    connection: &rusqlite::Connection,
    snapshot: &WorkspaceSnapshot,
    selected_ids: &HashSet<&str>,
    query_tokens: &[String],
    token_limit: Option<usize>,
    filters: &NoteSearchFilters,
) -> Result<Vec<NoteChunkCandidate>, String> {
    if query_tokens.is_empty() {
        return Ok(Vec::new());
    }

    let bounded_tokens: Vec<String> = match token_limit {
        Some(limit) => query_tokens.iter().take(limit).cloned().collect(),
        None => query_tokens.to_vec(),
    };
    let fts_query = bounded_tokens
        .iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let knowledge_base_placeholders = selected_ids
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");
    let mut where_clauses = vec![
        "note_chunk_fts MATCH ?1".to_owned(),
        format!("note_chunk_fts.knowledge_base_id IN ({knowledge_base_placeholders})"),
        format!("n.knowledge_base_id IN ({knowledge_base_placeholders})"),
    ];
    let mut bind_values: Vec<rusqlite::types::Value> =
        vec![rusqlite::types::Value::Text(fts_query)];
    bind_values.extend(
        selected_ids
            .iter()
            .map(|id| rusqlite::types::Value::Text((*id).to_owned())),
    );
    bind_values.extend(
        selected_ids
            .iter()
            .map(|id| rusqlite::types::Value::Text((*id).to_owned())),
    );

    append_note_filter_sql(filters, &mut where_clauses, &mut bind_values);

    let sql = format!(
        "SELECT
               c.chunk_id, c.note_id, c.knowledge_base_id, n.title, n.path,
               c.heading_path, c.content, c.line_start, c.line_end,
               bm25(note_chunk_fts, 8.0, 4.0, 6.0, 1.0)
             FROM note_chunk_fts
             JOIN note_chunks c ON c.chunk_id = note_chunk_fts.chunk_id
             JOIN notes n ON n.id = c.note_id
             WHERE {}
             ORDER BY bm25(note_chunk_fts, 8.0, 4.0, 6.0, 1.0)
             LIMIT 128",
        where_clauses.join("\n               AND ")
    );
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| format!("无法准备 FTS 检索：{error}"))?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(bind_values), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, f64>(9)?,
            ))
        })
        .map_err(|error| format!("无法执行 FTS 检索：{error}"))?;
    let mut citations = Vec::new();

    for row in rows {
        let (
            chunk_id,
            note_id,
            knowledge_base_id,
            title,
            path,
            heading_path,
            chunk_content,
            line_start,
            line_end,
            rank,
        ) = row.map_err(|error| format!("无法读取 FTS 命中结果：{error}"))?;

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
            citations.push(NoteChunkCandidate {
                chunk_id,
                citation: crate::domain::Citation {
                    knowledge_base_id,
                    knowledge_base_name: knowledge_base.name.clone(),
                    note_id,
                    title,
                    path,
                    snippet: extract_chunk_snippet(&chunk_content, &bounded_tokens),
                    score: 1.0 / (1.0 + rank.abs()),
                    location: Some(format_citation_location(
                        &heading_path,
                        line_start,
                        line_end,
                    )),
                    kind: None,
                    url: None,
                    published_at: None,
                },
            });
        }
    }

    Ok(citations)
}
