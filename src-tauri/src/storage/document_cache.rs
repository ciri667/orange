//! 文档解析结果的持久缓存。
//!
//! 进程内缓存只在一次运行里有效；重启后仍要重新解析整篇 PDF/DOCX。这里把抽取结果
//! 按 `(document_id, content_hash, extractor_version)` 落到 `document_extractions`，
//! 文件变化或解析器升级自动失效。预览与 Agent 读取共用同一份缓存。

use super::*;
use crate::domain::{DocumentTextExtraction, WorkspaceDocument};

/** 一次缓存命中的诊断信息，供日志和性能基线使用。 */
#[derive(Clone, Debug)]
pub(crate) struct CachedExtraction {
    pub extraction: DocumentTextExtraction,
    pub cache_hit: bool,
    pub duration_ms: u64,
}

/**
 * 读取文档文本，优先命中持久缓存。
 *
 * 首次调用会流式 hash 文件（不整文件读入），未命中时再走完整解析并写回缓存。
 */
pub(crate) fn extract_document_text_cached(
    app: &AppHandle,
    root: &Path,
    document: &WorkspaceDocument,
) -> Result<CachedExtraction, String> {
    let started = std::time::Instant::now();
    let target_path = resolve_existing_file_inside_root(root, &document.path)?;
    let metadata = fs::metadata(&target_path)
        .map_err(|error| format!("无法读取文档元数据：{error}"))?;
    let content_hash = hash_file(&target_path)
        .map_err(|error| format!("无法计算文档内容 hash：{error}"))?;

    if let Some(extraction) = load_document_extraction(app, &document.id, &content_hash) {
        return Ok(CachedExtraction {
            extraction,
            cache_hit: true,
            duration_ms: started.elapsed().as_millis() as u64,
        });
    }

    let extraction = extract_document_text(root, document)?;
    // 缓存写入失败不应让一次成功读取失败，只记录 warn。
    if let Err(error) = store_document_extraction(
        app,
        &extraction,
        metadata.len(),
        started.elapsed().as_millis() as u64,
    ) {
        log::warn!(
            target: "document_cache",
            "写入解析缓存失败：document={} error={}",
            document.id,
            error
        );
    }
    Ok(CachedExtraction {
        extraction,
        cache_hit: false,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

/// 按文档 id + 内容 hash + 解析器版本读取缓存；旧版本或内容变化时返回 None。
pub(crate) fn load_document_extraction(
    app: &AppHandle,
    document_id: &str,
    content_hash: &str,
) -> Option<DocumentTextExtraction> {
    let connection = open_database(app).ok()?;
    let row: Option<(i64, String, String, String, String)> = connection
        .query_row(
            "SELECT page_count, blocks_json, warnings_json, status, file_type FROM document_extractions
             WHERE document_id=?1 AND content_hash=?2 AND extractor_version=?3",
            params![document_id, content_hash, DOCUMENT_EXTRACTOR_VERSION],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .ok()?;
    let (page_count, blocks_json, warnings_json, status, file_type) = row?;
    let blocks: Vec<crate::domain::DocumentTextBlock> = serde_json::from_str(&blocks_json).ok()?;
    let warnings: Vec<String> = serde_json::from_str(&warnings_json).ok()?;
    let content_chars = blocks.iter().map(|block| block.text.chars().count()).sum();
    Some(DocumentTextExtraction {
        document_id: document_id.to_owned(),
        file_type,
        content_hash: content_hash.to_owned(),
        blocks,
        content_chars,
        page_count: page_count as usize,
        status,
        warnings,
    })
}

/// 写入或覆盖解析缓存；调用方保证 extraction 与 content_hash 对应。
pub(crate) fn store_document_extraction(
    app: &AppHandle,
    extraction: &DocumentTextExtraction,
    byte_size: u64,
    duration_ms: u64,
) -> Result<(), String> {
    let connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    let blocks_json =
        serde_json::to_string(&extraction.blocks).map_err(|_| "无法序列化文档结构块。")?;
    let warnings_json =
        serde_json::to_string(&extraction.warnings).map_err(|_| "无法序列化文档警告。")?;
    connection
        .execute(
            "INSERT OR REPLACE INTO document_extractions
             (document_id, content_hash, extractor_version, file_type, page_count, blocks_json, warnings_json, status, byte_size, duration_ms, extracted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                &extraction.document_id,
                &extraction.content_hash,
                DOCUMENT_EXTRACTOR_VERSION,
                &extraction.file_type,
                extraction.page_count as i64,
                blocks_json,
                warnings_json,
                &extraction.status,
                byte_size as i64,
                duration_ms as i64,
                format_local_datetime(),
            ],
        )
        .map_err(|error| format!("无法写入解析缓存：{error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 缓存表结构必须允许重复写入同一文档（INSERT OR REPLACE），并保留解析耗时字段。
    #[test]
    fn document_extractions_table_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let database_path = dir.path().join("cache.sqlite3");
        let connection = Connection::open(&database_path).unwrap();
        ensure_database_schema(&connection, &database_path).unwrap();
        connection
            .execute(
                "INSERT OR REPLACE INTO document_extractions
                 (document_id, content_hash, extractor_version, file_type, page_count, blocks_json, warnings_json, status, byte_size, duration_ms, extracted_at)
                 VALUES ('doc-a','hash-1','doc-reading-v2','pdf',2,'[]','[]','ok',1234,42,'2026-01-01 10:00:00')",
                [],
            )
            .unwrap();
        let (page_count, status, duration_ms): (i64, String, i64) = connection
            .query_row(
                "SELECT page_count, status, duration_ms FROM document_extractions WHERE document_id='doc-a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(page_count, 2);
        assert_eq!(status, "ok");
        assert_eq!(duration_ms, 42);
    }
}