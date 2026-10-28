use crate::domain::{Citation, WorkspaceSnapshot};
use crate::storage::{self, NoteChunkCandidate, NoteSearchFilters};
use rusqlite::{params_from_iter, types::Value};
use std::collections::{HashMap, HashSet};
use tauri::AppHandle;

/** 供工具轨迹使用的检索诊断，不包含服务地址、密钥或额外知识库信息。 */
pub(crate) struct SearchResult {
    pub citations: Vec<Citation>,
    pub mode: &'static str,
    pub coverage: f64,
    pub fallback_reason: Option<String>,
}

/** SQL 在向量读取前完成授权和结构化过滤；按行读取，避免把全库向量载入内存。 */
pub(crate) fn vector_candidates(
    connection: &rusqlite::Connection,
    snapshot: &WorkspaceSnapshot,
    scope: &HashSet<&str>,
    filters: &NoteSearchFilters,
    fingerprint: &str,
    query: &[f32],
    threshold: f32,
) -> Result<Vec<NoteChunkCandidate>, String> {
    if scope.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = scope.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let mut clauses = vec![
        format!("n.knowledge_base_id IN ({placeholders})"),
        "s.enabled=1".into(),
        "v.fingerprint=?".into(),
        "v.dimensions=?".into(),
    ];
    let mut bindings = scope
        .iter()
        .map(|id| Value::Text((*id).into()))
        .collect::<Vec<_>>();
    bindings.push(Value::Text(fingerprint.into()));
    bindings.push(Value::Integer(query.len() as i64));
    storage::append_note_filter_sql(filters, &mut clauses, &mut bindings);
    let sql=format!("SELECT c.chunk_id,c.note_id,c.knowledge_base_id,n.title,n.path,c.heading_path,c.content,c.line_start,c.line_end,v.vector
        FROM semantic_inputs i JOIN semantic_vectors v ON v.knowledge_base_id=i.knowledge_base_id AND v.input_hash=i.input_hash
        JOIN note_chunks c ON c.chunk_id=i.chunk_id JOIN notes n ON n.id=c.note_id AND n.knowledge_base_id=c.knowledge_base_id
        JOIN knowledge_bases k ON k.id=n.knowledge_base_id JOIN semantic_index_state s ON s.knowledge_base_id=n.knowledge_base_id
        WHERE {}",clauses.join(" AND "));
    let mut statement = connection.prepare(&sql).map_err(|_| "无法准备向量查询。")?;
    let mut rows = statement
        .query(params_from_iter(bindings))
        .map_err(|_| "无法执行向量查询。")?;
    let mut top: Vec<NoteChunkCandidate> = Vec::new();
    while let Some(row) = rows.next().map_err(|_| "无法读取向量结果。")? {
        let path: String = row.get(4).map_err(|_| "无法读取向量来源。")?;
        if storage::is_root_project_instruction_path(&path) {
            continue;
        }
        let kb_id: String = row.get(2).map_err(|_| "无法读取向量来源。")?;
        let Some(kb) = snapshot.knowledge_bases.iter().find(|k| k.id == kb_id) else {
            continue;
        };
        let bytes = row
            .get_ref(9)
            .and_then(|v| Ok(v.as_blob()?))
            .map_err(|_| "无法读取向量数据。")?;
        let Some(similarity) = cosine_blob(bytes, query) else {
            continue;
        };
        if similarity < threshold as f64 {
            continue;
        }
        if top.len() == 32 && similarity <= top.last().unwrap().citation.score {
            continue;
        }
        let content: String = row.get(6).map_err(|_| "无法读取引用片段。")?;
        top.push(NoteChunkCandidate {
            chunk_id: row.get(0).map_err(|_| "无法读取 chunk。")?,
            citation: Citation {
                knowledge_base_id: kb_id,
                knowledge_base_name: kb.name.clone(),
                note_id: row.get(1).map_err(|_| "无法读取笔记。")?,
                title: row.get(3).map_err(|_| "无法读取标题。")?,
                path,
                snippet: storage::extract_chunk_snippet(&content, &[]),
                score: similarity,
                location: Some(storage::format_citation_location(
                    &row.get::<_, String>(5).map_err(|_| "无法读取章节。")?,
                    row.get(7).map_err(|_| "无法读取行号。")?,
                    row.get(8).map_err(|_| "无法读取行号。")?,
                )),
                kind: None,
                url: None,
                published_at: None,
                ..Default::default()
            },
        });
        top.sort_by(|a, b| {
            b.citation
                .score
                .total_cmp(&a.citation.score)
                .then(a.chunk_id.cmp(&b.chunk_id))
        });
        top.truncate(32);
    }
    Ok(top)
}

/** blob 是归一化的 little-endian f32；损坏、零值、维度不符直接跳过。 */
pub(crate) fn cosine_blob(bytes: &[u8], query: &[f32]) -> Option<f64> {
    if query.is_empty() || bytes.len() != query.len() * 4 {
        return None;
    }
    let mut dot = 0.;
    let mut norm = 0.;
    let mut qnorm = 0.;
    for (value, q) in bytes.chunks_exact(4).zip(query) {
        let n = f32::from_le_bytes(value.try_into().ok()?) as f64;
        if !n.is_finite() || !q.is_finite() {
            return None;
        }
        dot += n * (*q as f64);
        norm += n * n;
        qnorm += (*q as f64).powi(2);
    }
    if norm <= f64::EPSILON || qnorm <= f64::EPSILON {
        None
    } else {
        Some((dot / (norm * qnorm).sqrt()).clamp(-1., 1.))
    }
}

/** 两路等权 RRF 后才按笔记去重，使单篇长笔记不会挤满最终结果。 */
pub(crate) fn fuse(
    fts: Vec<NoteChunkCandidate>,
    vectors: Vec<NoteChunkCandidate>,
    limit: usize,
) -> Vec<Citation> {
    let mut scores: HashMap<String, (f64, Citation)> = HashMap::new();
    for list in [fts, vectors] {
        for (rank, item) in list.into_iter().take(32).enumerate() {
            let entry = scores.entry(item.chunk_id).or_insert((0., item.citation));
            entry.0 += 1. / (60. + (rank + 1) as f64);
        }
    }
    let mut ranked = scores.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.1 .0.total_cmp(&a.1 .0).then(a.0.cmp(&b.0)));
    let mut seen = HashSet::new();
    ranked
        .into_iter()
        .filter_map(|(_, (score, mut citation))| {
            if !seen.insert(citation.note_id.clone()) {
                return None;
            }
            citation.score = score;
            Some(citation)
        })
        .take(limit)
        .collect()
}

pub(crate) fn search(
    app: &AppHandle,
    snapshot: &WorkspaceSnapshot,
    ids: &[String],
    query: &str,
    limit: usize,
    filters: &NoteSearchFilters,
) -> Result<SearchResult, String> {
    let settings = storage::load_user_settings(app)?;
    let scope: HashSet<&str> = ids.iter().map(String::as_str).collect();
    let statuses = storage::load_semantic_index_statuses(app)?;
    let scoped = statuses
        .iter()
        .filter(|s| scope.contains(s.knowledge_base_id.as_str()) && s.enabled)
        .collect::<Vec<_>>();
    let total: usize = scoped.iter().map(|s| s.total_chunks).sum();
    let indexed: usize = scoped.iter().map(|s| s.indexed_chunks).sum();
    let coverage = if total == 0 {
        0.
    } else {
        indexed as f64 / total as f64
    };
    // 文档 chunk 与笔记共用同一份结构化过滤；合并后按分数截断，避免文档结果被笔记挤出。
    let merge_documents = |mut citations: Vec<Citation>| -> Vec<Citation> {
        match storage::search_documents(app, snapshot, ids, query, limit, filters) {
            Ok(documents) => {
                citations.extend(documents);
                citations.sort_by(|left, right| right.score.total_cmp(&left.score));
                let mut seen = HashSet::new();
                citations.retain(|citation| seen.insert(citation.note_id.clone()));
                citations.truncate(limit);
                citations
            }
            Err(error) => {
                log::warn!(target: "retrieval", "文档检索失败，仅返回笔记结果：{error}");
                citations
            }
        }
    };
    let keyword = |reason: Option<String>| -> Result<SearchResult, String> {
        Ok(SearchResult {
            citations: merge_documents(storage::search_notes(
                app, snapshot, ids, query, limit, filters,
            )?),
            mode: if reason.is_some() {
                "keyword_fallback"
            } else {
                "keyword"
            },
            coverage,
            fallback_reason: reason,
        })
    };
    if !settings.semantic_search.enabled || scoped.is_empty() {
        return keyword(None);
    }
    if let Err(reason) = storage::semantic_allowed(&settings) {
        return keyword(Some(reason));
    }
    if indexed == 0 {
        return keyword(Some("语义索引尚无可用向量，正在使用关键词检索。".into()));
    }
    if query.trim().is_empty() {
        return keyword(None);
    }
    let _priority = super::QueryGuard::new();
    let key = match storage::load_semantic_key(&settings.semantic_search) {
        Ok(key) => key,
        Err(_) => return keyword(Some("向量服务密钥未配置。".into())),
    };
    // 兼容当前同步工具接口，沿用项目联网工具的运行时桥接；不在 SQLite 事务中等待 HTTP。
    let handle = match tokio::runtime::Handle::try_current() {
        Ok(h) => h,
        Err(_) => return keyword(Some("向量服务运行时不可用。".into())),
    };
    let vectors = tokio::task::block_in_place(|| {
        handle.block_on(super::provider::embed(
            &settings.semantic_search,
            &key,
            &[query.to_owned()],
            std::time::Duration::from_secs(5),
        ))
    });
    let query_vector = match vectors {
        Ok(mut v) => v.remove(0),
        Err(error) => return keyword(Some(error.message.into())),
    };
    // 请求在途时配置/隐私或开关可能改变，读取后重新检查，禁止旧配置向量参与融合。
    let current = storage::load_user_settings(app)?;
    if storage::semantic_allowed(&current).is_err()
        || super::provider::fingerprint(&current.semantic_search)
            != super::provider::fingerprint(&settings.semantic_search)
    {
        return keyword(Some("语义检索配置已变化，已回退关键词。".into()));
    }
    let connection = storage::open_database(app)?;
    let tokens = storage::tokenize_text(query);
    let mut fts = storage::search_note_chunk_candidates(
        &connection,
        snapshot,
        &scope,
        &tokens,
        None,
        filters,
    )?;
    if fts.is_empty() {
        fts = storage::search_note_chunk_candidates(
            &connection,
            snapshot,
            &scope,
            &tokens,
            Some(3),
            filters,
        )?;
    }
    let fp = super::provider::fingerprint(&settings.semantic_search);
    let semantic = match vector_candidates(
        &connection,
        snapshot,
        &scope,
        filters,
        &fp,
        &query_vector,
        settings.semantic_search.min_similarity,
    ) {
        Ok(result) => result,
        Err(_) => return keyword(Some("向量索引暂时不可用。".into())),
    };
    if !scoped
        .iter()
        .any(|s| s.dimensions == Some(query_vector.len()))
    {
        return keyword(Some("模型向量维度已变化，请重建索引。".into()));
    }
    if fts.is_empty() && semantic.is_empty() {
        return keyword(None);
    }
    Ok(SearchResult {
        citations: merge_documents(fuse(fts, semantic, limit)),
        mode: "hybrid",
        coverage,
        fallback_reason: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cosine_rejects_corruption_zero_and_dimension_mismatch() {
        assert!(cosine_blob(&[0; 8], &[1., 0.]).is_none());
        assert!(cosine_blob(&f32::NAN.to_le_bytes(), &[1.]).is_none());
        assert!(cosine_blob(&[0; 4], &[1., 0.]).is_none());
        assert_eq!(
            cosine_blob(
                &[1_f32.to_le_bytes(), 0_f32.to_le_bytes()].concat(),
                &[1., 0.]
            ),
            Some(1.)
        );
    }
}
