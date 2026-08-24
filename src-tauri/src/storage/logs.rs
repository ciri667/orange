use super::*;

pub(crate) fn normalize_audit_log_created_at(log: &mut RequestAuditLog) {
    if !is_created_at_placeholder(&log.created_at) {
        return;
    }

    log.created_at = format_local_datetime();
}

pub fn append_request_audit_log(app: &AppHandle, log: &RequestAuditLog) -> Result<(), String> {
    let connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    let mut log = log.clone();

    normalize_audit_log_created_at(&mut log);

    let summary = format!(
        "{} | {} | {}",
        log.scope_summary, log.content_summary, log.tool_summary
    );

    connection
        .execute(
            "INSERT INTO request_audit_logs
             (id, kind, summary, session_id, scope_summary, content_summary, tool_summary, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                &log.id,
                &log.kind,
                summary,
                log.session_id.as_deref(),
                &log.scope_summary,
                &log.content_summary,
                &log.tool_summary,
                &log.created_at
            ],
        )
        .map_err(|error| format!("无法写入请求审计日志：{error}"))?;

    Ok(())
}

/** 读取最近的请求审计日志，用于设置页展示模型和工具边界。 */
pub fn load_request_audit_logs(
    app: &AppHandle,
    limit: usize,
) -> Result<Vec<RequestAuditLog>, String> {
    let connection = open_database(app)?;
    let bounded_limit = limit.clamp(1, 50);

    {
        let _write_guard = lock_database_writer()?;

        // 读取前迁移旧版占位时间，避免设置页每次打开都继续看到“刚刚”。
        connection
            .execute(
                "UPDATE request_audit_logs SET created_at = ?1 WHERE TRIM(created_at) = '' OR created_at = '刚刚'",
                params![format_local_datetime()],
            )
            .map_err(|error| format!("无法迁移请求审计时间：{error}"))?;
    }

    let mut statement = connection
        .prepare(
            "SELECT id, kind, session_id, scope_summary, content_summary, tool_summary, created_at
             FROM request_audit_logs
             ORDER BY rowid DESC
             LIMIT ?1",
        )
        .map_err(|error| format!("无法准备请求审计读取：{error}"))?;
    let rows = statement
        .query_map(params![bounded_limit as i64], |row| {
            Ok(RequestAuditLog {
                id: row.get(0)?,
                kind: row.get(1)?,
                session_id: row.get(2)?,
                scope_summary: row.get(3)?,
                content_summary: row.get(4)?,
                tool_summary: row.get(5)?,
                created_at: row.get(6)?,
            })
        })
        .map_err(|error| format!("无法查询请求审计日志：{error}"))?;
    let mut logs = Vec::new();

    for row in rows {
        let mut log = row.map_err(|error| format!("无法解析请求审计日志：{error}"))?;

        normalize_audit_log_created_at(&mut log);
        logs.push(log);
    }

    Ok(logs)
}

/** 追加一条用户可读应用事件日志，并顺带执行本地保留策略。 */
pub fn append_app_event_log(app: &AppHandle, log: &AppEventLog) -> Result<(), String> {
    let connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;

    insert_app_event_log(&connection, log)?;
    prune_app_event_logs(&connection)?;

    Ok(())
}

/** 清空用户可读应用事件日志；运行诊断文件日志不受影响。 */
pub fn clear_app_event_logs(app: &AppHandle) -> Result<(), String> {
    let connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;

    connection
        .execute("DELETE FROM app_event_logs", [])
        .map_err(|error| format!("无法清空应用事件日志：{error}"))?;

    Ok(())
}

/** 读取最近应用事件日志，支持按级别和分类筛选。 */
pub fn load_app_event_logs(
    app: &AppHandle,
    limit: usize,
    level: Option<&str>,
    category: Option<&str>,
) -> Result<Vec<AppEventLog>, String> {
    let connection = open_database(app)?;
    let bounded_limit = limit.clamp(1, 500);

    query_app_event_logs(&connection, bounded_limit, level, category)
}

/** 将应用事件日志写入当前 SQLite 连接，供生产代码和测试复用。 */
pub(crate) fn insert_app_event_log(
    connection: &Connection,
    log: &AppEventLog,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO app_event_logs
             (id, level, category, event, message, status, operation_id, session_id,
              knowledge_base_id, entity_type, entity_id, relative_path, duration_ms,
              metadata_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                &log.id,
                &log.level,
                &log.category,
                &log.event,
                &log.message,
                &log.status,
                log.operation_id.as_deref(),
                log.session_id.as_deref(),
                log.knowledge_base_id.as_deref(),
                log.entity_type.as_deref(),
                log.entity_id.as_deref(),
                log.relative_path.as_deref(),
                log.duration_ms,
                log.metadata_json.as_deref(),
                &log.created_at
            ],
        )
        .map_err(|error| format!("无法写入应用事件日志：{error}"))?;

    Ok(())
}

/** 按保留策略清理应用事件日志，先按时间，再按最新条数兜底。 */
pub(crate) fn prune_app_event_logs(connection: &Connection) -> Result<(), String> {
    let oldest_created_at = (Local::now() - ChronoDuration::days(APP_EVENT_LOG_RETENTION_DAYS))
        .format("%Y/%m/%d %H:%M")
        .to_string();

    connection
        .execute(
            "DELETE FROM app_event_logs WHERE created_at < ?1",
            params![oldest_created_at],
        )
        .map_err(|error| format!("无法清理过期应用事件日志：{error}"))?;

    connection
        .execute(
            "DELETE FROM app_event_logs
             WHERE rowid NOT IN (
               SELECT rowid FROM app_event_logs ORDER BY rowid DESC LIMIT ?1
             )",
            params![MAX_APP_EVENT_LOGS as i64],
        )
        .map_err(|error| format!("无法裁剪应用事件日志数量：{error}"))?;

    Ok(())
}

/** 使用固定 SQL 分支绑定筛选参数，避免动态拼接用户输入。 */
pub(crate) fn query_app_event_logs(
    connection: &Connection,
    limit: usize,
    level: Option<&str>,
    category: Option<&str>,
) -> Result<Vec<AppEventLog>, String> {
    let bounded_limit = limit.clamp(1, 500) as i64;

    match (
        level.filter(|value| !value.trim().is_empty()),
        category.filter(|value| !value.trim().is_empty()),
    ) {
        (Some(level), Some(category)) => query_app_event_logs_by_sql(
            connection,
            "SELECT id, level, category, event, message, status, operation_id, session_id,
                    knowledge_base_id, entity_type, entity_id, relative_path, duration_ms,
                    metadata_json, created_at
             FROM app_event_logs
             WHERE level = ?1 AND category = ?2
             ORDER BY rowid DESC
             LIMIT ?3",
            params![level, category, bounded_limit],
        ),
        (Some(level), None) => query_app_event_logs_by_sql(
            connection,
            "SELECT id, level, category, event, message, status, operation_id, session_id,
                    knowledge_base_id, entity_type, entity_id, relative_path, duration_ms,
                    metadata_json, created_at
             FROM app_event_logs
             WHERE level = ?1
             ORDER BY rowid DESC
             LIMIT ?2",
            params![level, bounded_limit],
        ),
        (None, Some(category)) => query_app_event_logs_by_sql(
            connection,
            "SELECT id, level, category, event, message, status, operation_id, session_id,
                    knowledge_base_id, entity_type, entity_id, relative_path, duration_ms,
                    metadata_json, created_at
             FROM app_event_logs
             WHERE category = ?1
             ORDER BY rowid DESC
             LIMIT ?2",
            params![category, bounded_limit],
        ),
        (None, None) => query_app_event_logs_by_sql(
            connection,
            "SELECT id, level, category, event, message, status, operation_id, session_id,
                    knowledge_base_id, entity_type, entity_id, relative_path, duration_ms,
                    metadata_json, created_at
             FROM app_event_logs
             ORDER BY rowid DESC
             LIMIT ?1",
            params![bounded_limit],
        ),
    }
}

/** 执行应用事件日志查询，并把 SQLite row 转成前端 camelCase wire 模型。 */
pub(crate) fn query_app_event_logs_by_sql<P>(
    connection: &Connection,
    sql: &str,
    params: P,
) -> Result<Vec<AppEventLog>, String>
where
    P: rusqlite::Params,
{
    let mut statement = connection
        .prepare(sql)
        .map_err(|error| format!("无法准备应用事件日志读取：{error}"))?;
    let rows = statement
        .query_map(params, |row| {
            Ok(AppEventLog {
                id: row.get(0)?,
                level: row.get(1)?,
                category: row.get(2)?,
                event: row.get(3)?,
                message: row.get(4)?,
                status: row.get(5)?,
                operation_id: row.get(6)?,
                session_id: row.get(7)?,
                knowledge_base_id: row.get(8)?,
                entity_type: row.get(9)?,
                entity_id: row.get(10)?,
                relative_path: row.get(11)?,
                duration_ms: row.get(12)?,
                metadata_json: row.get(13)?,
                created_at: row.get(14)?,
            })
        })
        .map_err(|error| format!("无法查询应用事件日志：{error}"))?;
    let mut logs = Vec::new();

    for row in rows {
        logs.push(row.map_err(|error| format!("无法解析应用事件日志：{error}"))?);
    }

    Ok(logs)
}
