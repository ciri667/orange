//! 语义索引持久化。输入映射与 chunk 同事务维护，向量按实际输入哈希复用。
use super::*;
use crate::domain::{SemanticIndexAction, SemanticIndexStatus, SemanticSearchSettings};

/** 后台任务携带授权代次；任何关闭、重建、服务切换都会使旧任务失效。 */
#[derive(Clone, Debug)]
pub(crate) struct SemanticJob {
    pub chunk_id: String,
    pub knowledge_base_id: String,
    pub input_hash: String,
    pub input: String,
    pub fingerprint: String,
    pub generation: i64,
    pub attempts: i64,
}

pub(crate) fn ensure_semantic_schema(connection: &Connection) -> Result<(), String> {
    connection.execute_batch(r#"
        CREATE TABLE IF NOT EXISTS semantic_note_signatures(note_id TEXT PRIMARY KEY, signature TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS semantic_index_state (
          knowledge_base_id TEXT PRIMARY KEY, enabled INTEGER NOT NULL DEFAULT 0,
          paused INTEGER NOT NULL DEFAULT 0, generation INTEGER NOT NULL DEFAULT 0,
          fingerprint TEXT NOT NULL DEFAULT '', dimensions INTEGER, active INTEGER NOT NULL DEFAULT 0,
          last_error TEXT NOT NULL DEFAULT ''
        );
        CREATE TABLE IF NOT EXISTS semantic_inputs (
          chunk_id TEXT PRIMARY KEY, knowledge_base_id TEXT NOT NULL,
          input_hash TEXT NOT NULL, input TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_semantic_inputs_kb ON semantic_inputs(knowledge_base_id, input_hash);
        CREATE TABLE IF NOT EXISTS semantic_vectors (
          knowledge_base_id TEXT NOT NULL, fingerprint TEXT NOT NULL, input_hash TEXT NOT NULL,
          dimensions INTEGER NOT NULL, vector BLOB NOT NULL,
          PRIMARY KEY(knowledge_base_id, fingerprint, input_hash)
        );
        CREATE TABLE IF NOT EXISTS semantic_jobs (
          chunk_id TEXT PRIMARY KEY, knowledge_base_id TEXT NOT NULL, input_hash TEXT NOT NULL,
          fingerprint TEXT NOT NULL, generation INTEGER NOT NULL, attempts INTEGER NOT NULL DEFAULT 0,
          next_attempt INTEGER NOT NULL DEFAULT 0, last_error TEXT NOT NULL DEFAULT ''
        );
        CREATE TRIGGER IF NOT EXISTS semantic_chunk_deleted AFTER DELETE ON note_chunks BEGIN
          DELETE FROM semantic_inputs WHERE chunk_id = old.chunk_id;
          DELETE FROM semantic_jobs WHERE chunk_id = old.chunk_id;
        END;
        CREATE TRIGGER IF NOT EXISTS semantic_document_chunk_deleted AFTER DELETE ON document_chunks BEGIN
          DELETE FROM semantic_inputs WHERE chunk_id = old.chunk_id;
          DELETE FROM semantic_jobs WHERE chunk_id = old.chunk_id;
        END;
        INSERT OR IGNORE INTO semantic_index_state(knowledge_base_id, enabled)
          SELECT id, semantic_index_enabled FROM knowledge_bases;
    "#).map_err(|_| "无法初始化语义索引表。".into())
}

/** 只在实际 chunk 输入变化时生成新哈希；行号不是模型输入，不影响向量复用。 */
pub(crate) fn index_semantic_input(
    connection: &Connection,
    note: &Note,
    chunk: &NoteChunk,
) -> Result<(), String> {
    if is_root_project_instruction_path(&note.path) {
        return Ok(());
    }
    let mut tags = note.tags.clone();
    tags.sort();
    tags.dedup();
    let input = format!(
        "标题：{}\n路径：{}\n章节：{}\n标签：{}\n正文：{}",
        note.title,
        note.path,
        chunk.heading_path,
        tags.join(" "),
        chunk.content
    );
    connection.execute("INSERT INTO semantic_inputs(chunk_id, knowledge_base_id, input_hash, input) VALUES (?1,?2,?3,?4)
        ON CONFLICT(chunk_id) DO UPDATE SET knowledge_base_id=excluded.knowledge_base_id,input_hash=excluded.input_hash,input=excluded.input",
        params![format!("{}:{}",note.id,chunk.chunk_index),note.knowledge_base_id,hash_content(&input),input])
        .map_err(|_| "无法维护语义索引输入。".to_owned())?;
    Ok(())
}

/** DOCX/PDF chunk 的语义输入；chunk_id 与 document_chunks 保持一致（document:<id>:<index>）。 */
pub(crate) fn index_document_semantic_input(
    connection: &Connection,
    document: &WorkspaceDocument,
    chunk_id: &str,
    heading_path: &str,
    content: &str,
) -> Result<(), String> {
    let input = format!(
        "标题：{}\n路径：{}\n页码/结构：{}\n正文：{}",
        document.title, document.path, heading_path, content
    );
    connection
        .execute(
            "INSERT INTO semantic_inputs(chunk_id, knowledge_base_id, input_hash, input) VALUES (?1,?2,?3,?4)
             ON CONFLICT(chunk_id) DO UPDATE SET knowledge_base_id=excluded.knowledge_base_id,input_hash=excluded.input_hash,input=excluded.input",
            params![
                chunk_id,
                &document.knowledge_base_id,
                hash_content(&input),
                input
            ],
        )
        .map_err(|_| "无法维护文档语义索引输入。".to_owned())?;
    Ok(())
}

/** 状态表是开关的权威来源，普通快照（包括旧 Agent 回合）不能覆盖它。 */
pub(crate) fn ensure_semantic_knowledge_base(
    connection: &Connection,
    id: &str,
    enabled: bool,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT OR IGNORE INTO semantic_index_state(knowledge_base_id,enabled) VALUES (?1,?2)",
            params![id, enabled],
        )
        .map_err(|_| "无法初始化知识库语义状态。".to_owned())?;
    Ok(())
}

/** 清理只在索引事务末尾执行，确保重扫临时删除 chunk 不会丢失可复用向量。 */
pub(crate) fn cleanup_semantic(connection: &Connection) -> Result<(), String> {
    connection.execute_batch("DELETE FROM semantic_vectors WHERE NOT EXISTS (
          SELECT 1 FROM semantic_inputs i WHERE i.knowledge_base_id=semantic_vectors.knowledge_base_id AND i.input_hash=semantic_vectors.input_hash);
        UPDATE semantic_index_state SET enabled=0,paused=0,active=0,generation=generation+1
          WHERE enabled=1 AND NOT EXISTS(SELECT 1 FROM knowledge_bases k WHERE k.id=knowledge_base_id);
        DELETE FROM semantic_jobs WHERE NOT EXISTS(SELECT 1 FROM semantic_inputs i WHERE i.chunk_id=semantic_jobs.chunk_id);
        DELETE FROM semantic_note_signatures WHERE NOT EXISTS(SELECT 1 FROM notes n WHERE n.id=note_id);
        DELETE FROM semantic_vectors WHERE knowledge_base_id IN (SELECT knowledge_base_id FROM semantic_index_state WHERE enabled=0);")
        .map_err(|_| "无法清理过期语义索引。".into())
}

/** 单事务提交设置、任务指纹和代次，关闭后在途响应无法再次写回。 */
pub(crate) fn persist_semantic_settings(
    app: &AppHandle,
    settings: &UserSettings,
) -> Result<(), String> {
    let mut connection = open_database(app)?;
    let _guard = lock_database_writer()?;
    let transaction = connection
        .transaction()
        .map_err(|_| "无法启动语义配置事务。")?;
    let payload = serde_json::to_string(settings).map_err(|_| "无法序列化语义配置。")?;
    transaction
        .execute(
            "INSERT OR REPLACE INTO user_settings(key,payload_json,updated_at) VALUES (?1,?2,?3)",
            params![USER_SETTINGS_KEY, payload, format_local_datetime()],
        )
        .map_err(|_| "无法保存语义配置。")?;
    // 全局开关与隐私变化也提高代次，避免关闭再开启间旧请求复活。
    transaction
        .execute(
            "UPDATE semantic_index_state SET generation=generation+1,active=0",
            [],
        )
        .map_err(|_| "无法使旧任务失效。")?;
    reconcile_semantic(&transaction, settings)?;
    transaction
        .commit()
        .map_err(|_| "无法提交语义配置。".into())
}

/** 从数据库读取当前配置，用于持锁回写时再次校验，避免配置保存与响应回写竞争。 */
fn persisted_semantic_settings(connection: &Connection) -> Result<UserSettings, String> {
    let json: Option<String> = connection
        .query_row(
            "SELECT payload_json FROM user_settings WHERE key=?1",
            [USER_SETTINGS_KEY],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| "无法读取语义配置。")?;
    json.map(|value| {
        model_provider::parse_or_migrate_user_settings_json(&value, &format_local_datetime())
    })
    .unwrap_or_else(|| Ok(default_user_settings()))
}

pub(crate) fn semantic_allowed(settings: &UserSettings) -> Result<(), String> {
    if !settings.semantic_search.enabled {
        return Err("语义检索已关闭。".into());
    }
    crate::retrieval::provider::validate(&settings.semantic_search)?;
    let (_, remote) = crate::retrieval::provider::endpoint(&settings.semantic_search)?;
    if remote {
        if settings.privacy_policy != "allow-selected-scope" {
            return Err("仅本地隐私策略禁止云端语义索引。".into());
        }
        if settings
            .semantic_search
            .cloud_consent_fingerprint
            .as_deref()
            != Some(crate::retrieval::provider::fingerprint(&settings.semantic_search).as_str())
        {
            return Err("请确认云端语义索引的数据发送授权。".into());
        }
    }
    Ok(())
}

/** 将待处理状态持久化。相同 hash 命中缓存时直接复用，任务不会重复请求。 */
pub(crate) fn reconcile_semantic(
    connection: &Connection,
    settings: &UserSettings,
) -> Result<(), String> {
    let fingerprint = crate::retrieval::provider::fingerprint(&settings.semantic_search);
    connection.execute("UPDATE semantic_index_state SET fingerprint=?1,generation=generation+1,dimensions=NULL,active=0,last_error=''
        WHERE fingerprint<>?1",[&fingerprint]).map_err(|_| "无法更新语义索引版本。")?;
    cleanup_semantic(connection)?;
    // 切回已缓存的服务时恢复维度；否则没有待处理任务却会被误判为维度未知。
    connection.execute("UPDATE semantic_index_state SET dimensions=(SELECT v.dimensions FROM semantic_vectors v WHERE v.knowledge_base_id=semantic_index_state.knowledge_base_id AND v.fingerprint=?1 LIMIT 1) WHERE dimensions IS NULL",[&fingerprint]).map_err(|_| "无法恢复向量维度。")?;
    connection.execute("DELETE FROM semantic_jobs WHERE NOT EXISTS(SELECT 1 FROM semantic_index_state s JOIN semantic_inputs i ON i.knowledge_base_id=s.knowledge_base_id
        WHERE i.chunk_id=semantic_jobs.chunk_id AND i.input_hash=semantic_jobs.input_hash AND s.enabled=1
        AND s.generation=semantic_jobs.generation AND s.fingerprint=semantic_jobs.fingerprint)
        OR EXISTS(SELECT 1 FROM semantic_vectors v WHERE v.knowledge_base_id=semantic_jobs.knowledge_base_id AND v.fingerprint=semantic_jobs.fingerprint AND v.input_hash=semantic_jobs.input_hash)",[])
        .map_err(|_| "无法清理语义任务。")?;
    if semantic_allowed(settings).is_err() {
        return Ok(());
    }
    connection.execute("INSERT OR IGNORE INTO semantic_jobs(chunk_id,knowledge_base_id,input_hash,fingerprint,generation)
        SELECT i.chunk_id,i.knowledge_base_id,i.input_hash,s.fingerprint,s.generation FROM semantic_inputs i
        JOIN semantic_index_state s ON s.knowledge_base_id=i.knowledge_base_id
        JOIN knowledge_bases k ON k.id=i.knowledge_base_id
        WHERE s.enabled=1 AND NOT EXISTS(SELECT 1 FROM semantic_vectors v WHERE v.knowledge_base_id=i.knowledge_base_id AND v.fingerprint=s.fingerprint AND v.input_hash=i.input_hash)",[])
        .map_err(|_| "无法创建语义索引任务。")?;
    Ok(())
}

/** 网络调用前重新检查任务；单条降级时也逐条检查，撤销授权后不再发送后续文本。 */
pub(crate) fn validate_semantic_jobs(
    app: &AppHandle,
    jobs: &[SemanticJob],
) -> Result<Vec<SemanticJob>, String> {
    let connection = open_database(app)?;
    let settings = persisted_semantic_settings(&connection)?;
    if semantic_allowed(&settings).is_err() {
        return Ok(Vec::new());
    }
    let fingerprint = crate::retrieval::provider::fingerprint(&settings.semantic_search);
    let mut valid = Vec::new();
    for job in jobs {
        if fingerprint != job.fingerprint {
            continue;
        }
        let allowed:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM semantic_index_state s JOIN knowledge_bases k ON k.id=s.knowledge_base_id JOIN semantic_inputs i ON i.knowledge_base_id=k.id
            WHERE s.enabled=1 AND s.paused=0 AND s.generation=?1 AND s.fingerprint=?2 AND i.chunk_id=?3 AND i.input_hash=?4 AND k.id=?5)",params![job.generation,job.fingerprint,job.chunk_id,job.input_hash,job.knowledge_base_id],|r|r.get(0)).map_err(|_| "无法校验发送范围。")?;
        if allowed {
            valid.push(job.clone());
        }
    }
    Ok(valid)
}

pub(crate) fn next_semantic_jobs(
    app: &AppHandle,
) -> Result<(UserSettings, Vec<SemanticJob>), String> {
    let mut connection = open_database(app)?;
    let _guard = lock_database_writer()?;
    let transaction = connection
        .transaction()
        .map_err(|_| "无法启动语义任务事务。")?;
    let settings = persisted_semantic_settings(&transaction)?;
    reconcile_semantic(&transaction, &settings)?;
    let jobs = if semantic_allowed(&settings).is_ok() {
        let mut statement = transaction.prepare("SELECT j.chunk_id,j.knowledge_base_id,j.input_hash,i.input,j.fingerprint,j.generation,j.attempts
            FROM semantic_jobs j JOIN semantic_inputs i ON i.chunk_id=j.chunk_id
            JOIN semantic_index_state s ON s.knowledge_base_id=j.knowledge_base_id
            WHERE s.enabled=1 AND s.paused=0 AND s.last_error='' AND j.attempts<3 AND j.next_attempt<=unixepoch()
            ORDER BY j.next_attempt,j.rowid LIMIT 16").map_err(|_| "无法读取语义任务。")?;
        let rows = statement
            .query_map([], |r| {
                Ok(SemanticJob {
                    chunk_id: r.get(0)?,
                    knowledge_base_id: r.get(1)?,
                    input_hash: r.get(2)?,
                    input: r.get(3)?,
                    fingerprint: r.get(4)?,
                    generation: r.get(5)?,
                    attempts: r.get(6)?,
                })
            })
            .map_err(|_| "无法读取语义任务。")?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| "无法解析语义任务。")?
    } else {
        Vec::new()
    };
    for job in &jobs {
        transaction
            .execute(
                "UPDATE semantic_index_state SET active=1 WHERE knowledge_base_id=?1",
                [&job.knowledge_base_id],
            )
            .map_err(|_| "无法更新索引状态。")?;
    }
    transaction.commit().map_err(|_| "无法提交语义任务。")?;
    Ok((settings, jobs))
}

/** 原子核对代次、配置、授权与实际输入；不允许旧响应复活已删除的文本。 */
pub(crate) fn complete_semantic_job(
    connection: &Connection,
    settings: &UserSettings,
    job: &SemanticJob,
    vector: &[f32],
) -> Result<bool, String> {
    if semantic_allowed(settings).is_err()
        || crate::retrieval::provider::fingerprint(&settings.semantic_search) != job.fingerprint
    {
        return Ok(false);
    }
    let valid: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM semantic_index_state s JOIN knowledge_bases k ON k.id=s.knowledge_base_id
        JOIN semantic_inputs i ON i.knowledge_base_id=s.knowledge_base_id JOIN semantic_jobs j ON j.chunk_id=i.chunk_id
        WHERE s.knowledge_base_id=?1 AND s.enabled=1 AND s.paused=0 AND s.generation=?2 AND s.fingerprint=?3
        AND i.chunk_id=?4 AND i.input_hash=?5 AND j.generation=s.generation)",params![job.knowledge_base_id,job.generation,job.fingerprint,job.chunk_id,job.input_hash],|r|r.get(0)).map_err(|_| "无法校验语义任务。")?;
    if !valid {
        return Ok(false);
    }
    let dimension: Option<usize> = connection
        .query_row(
            "SELECT dimensions FROM semantic_index_state WHERE knowledge_base_id=?1",
            [&job.knowledge_base_id],
            |r| r.get(0),
        )
        .map_err(|_| "无法读取向量维度。")?;
    if dimension.is_some_and(|d| d != vector.len()) {
        return Err("向量维度变化，请重建索引。".into());
    }
    let bytes = vector
        .iter()
        .flat_map(|n| n.to_le_bytes())
        .collect::<Vec<_>>();
    connection.execute("INSERT OR REPLACE INTO semantic_vectors(knowledge_base_id,fingerprint,input_hash,dimensions,vector) VALUES (?1,?2,?3,?4,?5)",params![job.knowledge_base_id,job.fingerprint,job.input_hash,vector.len(),bytes]).map_err(|_| "无法保存向量。")?;
    connection
        .execute(
            "UPDATE semantic_index_state SET dimensions=?2,active=0 WHERE knowledge_base_id=?1",
            params![job.knowledge_base_id, vector.len()],
        )
        .map_err(|_| "无法保存向量维度。")?;
    connection
        .execute(
            "DELETE FROM semantic_jobs WHERE chunk_id=?1",
            [&job.chunk_id],
        )
        .map_err(|_| "无法完成索引任务。")?;
    Ok(true)
}

pub(crate) fn finish_semantic_batch(
    app: &AppHandle,
    jobs: &[SemanticJob],
    result: Result<Vec<Vec<f32>>, crate::retrieval::provider::EmbeddingError>,
) -> Result<(), String> {
    let mut connection = open_database(app)?;
    let _guard = lock_database_writer()?;
    let transaction = connection.transaction().map_err(|_| "无法启动向量回写。")?;
    let settings = persisted_semantic_settings(&transaction)?;
    for (index, job) in jobs.iter().enumerate() {
        transaction
            .execute(
                "UPDATE semantic_index_state SET active=0 WHERE knowledge_base_id=?1",
                [&job.knowledge_base_id],
            )
            .map_err(|_| "无法更新任务状态。")?;
        let failure = match &result {
            Ok(vectors) => {
                match complete_semantic_job(&transaction, &settings, job, &vectors[index]) {
                    Ok(_) => None,
                    Err(_) => Some(("向量维度变化，请重建索引。", false, true)),
                }
            }
            Err(error) => Some((error.message, error.retryable, error.configuration)),
        };
        if let Some((message, retryable, configuration)) = failure {
            // UPDATE 必须绑定任务代次；错误响应也不能暂停用户刚切换的新配置。
            transaction
                .execute(
                    "UPDATE semantic_jobs SET attempts=?2,last_error=?3,next_attempt=unixepoch()+?4
                WHERE chunk_id=?1 AND generation=?5 AND fingerprint=?6 AND input_hash=?7",
                    params![
                        job.chunk_id,
                        if retryable { job.attempts + 1 } else { 3 },
                        message,
                        if job.attempts == 0 { 2 } else { 8 },
                        job.generation,
                        job.fingerprint,
                        job.input_hash
                    ],
                )
                .map_err(|_| "无法保存任务错误。")?;
            if configuration {
                transaction.execute("UPDATE semantic_index_state SET last_error=?2 WHERE knowledge_base_id=?1 AND generation=?3 AND fingerprint=?4",params![job.knowledge_base_id,message,job.generation,job.fingerprint]).map_err(|_| "无法暂停错误配置。")?;
            }
        }
    }
    reconcile_semantic(&transaction, &settings)?;
    transaction
        .commit()
        .map_err(|_| "无法提交向量回写。".into())
}

pub(crate) fn semantic_statuses(
    connection: &Connection,
    settings: &UserSettings,
) -> Result<Vec<SemanticIndexStatus>, String> {
    let fp = crate::retrieval::provider::fingerprint(&settings.semantic_search);
    let mut statement=connection.prepare("SELECT k.id,COALESCE(s.enabled,0),COALESCE(s.paused,0),COALESCE(s.active,0),COALESCE(s.last_error,''),s.dimensions,
        (SELECT count(*) FROM semantic_inputs i WHERE i.knowledge_base_id=k.id),
        (SELECT count(*) FROM semantic_inputs i JOIN semantic_vectors v ON v.knowledge_base_id=i.knowledge_base_id AND v.input_hash=i.input_hash AND v.fingerprint=?1 WHERE i.knowledge_base_id=k.id),
        (SELECT count(*) FROM semantic_jobs j WHERE j.knowledge_base_id=k.id AND j.attempts>=3),COALESCE(s.fingerprint,'')
        FROM knowledge_bases k LEFT JOIN semantic_index_state s ON s.knowledge_base_id=k.id ORDER BY k.rowid").map_err(|_| "无法查询语义索引进度。")?;
    let rows = statement
        .query_map([&fp], |r| {
            let enabled: bool = r.get(1)?;
            let paused: bool = r.get(2)?;
            let active: bool = r.get(3)?;
            let error: String = r.get(4)?;
            let total: usize = r.get(6)?;
            let indexed: usize = r.get(7)?;
            let failed: usize = r.get(8)?;
            let state_fp: String = r.get(9)?;
            let blocked = semantic_allowed(settings).err();
            let (status, message) = if !enabled {
                ("disabled", "未开启".to_owned())
            } else if !settings.semantic_search.enabled {
                ("paused", "全局语义检索已关闭。".into())
            } else if let Some(reason) = blocked {
                ("stale", reason)
            } else if paused {
                ("paused", "已暂停生成新向量，已有向量仍可检索。".into())
            } else if !error.is_empty() {
                ("error", error)
            } else if state_fp != fp {
                ("stale", "配置已变更，等待建立新索引。".into())
            } else if failed > 0 {
                (
                    "partial_failure",
                    "部分片段建立失败，可点击继续重试。".into(),
                )
            } else if indexed == total {
                ("ready", "索引可用。".into())
            } else if active {
                ("indexing", "正在建立语义索引。".into())
            } else {
                ("pending", "等待后台建立索引。".into())
            };
            Ok(SemanticIndexStatus {
                knowledge_base_id: r.get(0)?,
                enabled,
                paused,
                status: status.into(),
                indexed_chunks: indexed,
                pending_chunks: total.saturating_sub(indexed + failed),
                failed_chunks: failed,
                total_chunks: total,
                dimensions: r.get(5)?,
                message,
            })
        })
        .map_err(|_| "无法读取语义进度。")?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| "无法解析语义进度。".into())
}

pub fn load_semantic_index_statuses(app: &AppHandle) -> Result<Vec<SemanticIndexStatus>, String> {
    let connection = open_database(app)?;
    let settings = load_user_settings(app)?;
    semantic_statuses(&connection, &settings)
}

pub fn apply_semantic_index_action(
    app: &AppHandle,
    payload: &SemanticIndexAction,
) -> Result<Vec<SemanticIndexStatus>, String> {
    let mut connection = open_database(app)?;
    let _guard = lock_database_writer()?;
    let transaction = connection.transaction().map_err(|_| "无法修改语义索引。")?;
    let settings = persisted_semantic_settings(&transaction)?;
    let statuses=apply_semantic_action_in_transaction(&transaction,&settings,payload)?;
    transaction.commit().map_err(|_| "无法提交语义索引设置。")?;
    Ok(statuses)
}


/** 与 IPC 共用的事务内操作，供生命周期测试直接验证真实状态转换。 */
pub(crate) fn apply_semantic_action_in_transaction(connection:&Connection,settings:&UserSettings,payload:&SemanticIndexAction)->Result<Vec<SemanticIndexStatus>,String> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM knowledge_bases WHERE id=?1)",
            [&payload.knowledge_base_id],
            |r| r.get(0),
        )
        .map_err(|_| "无法校验知识库。")?;
    if !exists {
        return Err("知识库已移除授权。".into());
    }
    if matches!(payload.action.as_str(), "enable" | "resume" | "rebuild") {
        semantic_allowed(&settings)?;
        if payload.action == "enable"
            && crate::retrieval::provider::endpoint(&settings.semantic_search)?.1
            && !payload.allow_cloud_upload
        {
            return Err("开启该知识库前，请确认向云端发送文本片段。".into());
        }
    }
    ensure_semantic_knowledge_base(connection, &payload.knowledge_base_id, false)?;
    match payload.action.as_str() {
        "enable" => {
            connection.execute("UPDATE semantic_index_state SET enabled=1,paused=0,generation=generation+1,last_error='' WHERE knowledge_base_id=?1",[&payload.knowledge_base_id]).map_err(|_| "无法开启语义索引。")?;
        }
        "pause" => {
            connection.execute("UPDATE semantic_index_state SET paused=1,active=0,generation=generation+1 WHERE knowledge_base_id=?1",[&payload.knowledge_base_id]).map_err(|_| "无法暂停语义索引。")?;
        }
        "resume" => {
            connection.execute("UPDATE semantic_index_state SET paused=0,last_error='' WHERE knowledge_base_id=?1",[&payload.knowledge_base_id]).map_err(|_| "无法继续语义索引。")?;
            connection.execute("UPDATE semantic_jobs SET attempts=0,next_attempt=0,last_error='' WHERE knowledge_base_id=?1",[&payload.knowledge_base_id]).map_err(|_| "无法重试语义任务。")?;
        }
        "disable" | "rebuild" => {
            connection.execute("UPDATE semantic_index_state SET enabled=?2,paused=0,active=0,generation=generation+1,dimensions=NULL,last_error='' WHERE knowledge_base_id=?1",params![payload.knowledge_base_id,payload.action=="rebuild"]).map_err(|_| "无法重置语义索引。")?;
            connection
                .execute(
                    "DELETE FROM semantic_vectors WHERE knowledge_base_id=?1",
                    [&payload.knowledge_base_id],
                )
                .map_err(|_| "无法清理知识库向量。")?;
            connection
                .execute(
                    "DELETE FROM semantic_jobs WHERE knowledge_base_id=?1",
                    [&payload.knowledge_base_id],
                )
                .map_err(|_| "无法清理知识库任务。")?;
        }
        _ => return Err("不支持的语义索引操作。".into()),
    }
    connection.execute("UPDATE knowledge_bases SET semantic_index_enabled=(SELECT enabled FROM semantic_index_state WHERE knowledge_base_id=?1) WHERE id=?1",[&payload.knowledge_base_id]).map_err(|_| "无法保存知识库索引开关。")?;
    reconcile_semantic(connection, &settings)?;
    let statuses = semantic_statuses(connection, &settings)?;
    Ok(statuses)
}

/** 密钥引用按配置指纹隔离，切换服务地址不会将旧服务的凭证发给新端点。 */
pub(crate) fn semantic_key_reference(settings: &SemanticSearchSettings) -> String {
    model_provider::key_reference_for_provider(&format!(
        "semantic-{}",
        crate::retrieval::provider::fingerprint(settings)
    ))
}

pub(crate) fn load_semantic_key(settings: &SemanticSearchSettings) -> Result<String, String> {
    if !settings.requires_api_key {
        return Ok(String::new());
    }
    load_model_api_key(&semantic_key_reference(settings))?
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| "请先保存向量服务密钥。".into())
}
