use super::*;

pub(crate) fn database_path(app: &AppHandle) -> Result<PathBuf, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法获取应用数据目录：{error}"))?;

    fs::create_dir_all(&app_data_dir).map_err(|error| format!("无法创建应用数据目录：{error}"))?;
    Ok(app_data_dir.join("orange.sqlite3"))
}

/** 打开 SQLite 连接并确保 FTS5、向量缓存和会话表存在。 */
pub fn open_database(app: &AppHandle) -> Result<Connection, String> {
    let database_path = database_path(app)?;
    let connection =
        Connection::open(&database_path).map_err(|error| format!("无法打开 SQLite：{error}"))?;

    // 启动阶段多个命令可能同时打开 SQLite；等待窗口覆盖首次大知识库索引重建。
    connection
        .busy_timeout(DATABASE_BUSY_TIMEOUT)
        .map_err(|error| format!("无法配置 SQLite 忙等待：{error}"))?;

    ensure_database_schema(&connection, &database_path)?;

    Ok(connection)
}

/** 确保 SQLite schema 只在每个进程和数据库文件上初始化一次，减少启动并发 DDL 锁竞争。 */
pub(crate) fn ensure_database_schema(
    connection: &Connection,
    database_path: &Path,
) -> Result<(), String> {
    let initialized_paths = INITIALIZED_DATABASE_PATHS.get_or_init(|| Mutex::new(HashSet::new()));
    {
        let initialized_paths = initialized_paths
            .lock()
            .map_err(|_| "SQLite 初始化状态锁已损坏。".to_owned())?;

        if initialized_paths.contains(database_path) {
            return Ok(());
        }
    }

    let _write_guard = lock_database_writer()?;

    {
        let initialized_paths = initialized_paths
            .lock()
            .map_err(|_| "SQLite 初始化状态锁已损坏。".to_owned())?;

        if initialized_paths.contains(database_path) {
            return Ok(());
        }
    }

    connection
        .execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;

            CREATE TABLE IF NOT EXISTS knowledge_bases (
              id TEXT PRIMARY KEY,
              name TEXT NOT NULL,
              path TEXT NOT NULL,
              semantic_index_enabled INTEGER NOT NULL DEFAULT 0,
              updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS notes (
              id TEXT PRIMARY KEY,
              knowledge_base_id TEXT NOT NULL,
              title TEXT NOT NULL,
              path TEXT NOT NULL,
              content_hash TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS embeddings (
              note_id TEXT NOT NULL,
              chunk_index INTEGER NOT NULL,
              vector BLOB NOT NULL,
              model TEXT NOT NULL,
              updated_at TEXT NOT NULL,
              PRIMARY KEY (note_id, chunk_index)
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS note_fts USING fts5(
              note_id UNINDEXED,
              knowledge_base_id UNINDEXED,
              title,
              path,
              body
            );

            CREATE TABLE IF NOT EXISTS agent_sessions (
              id TEXT PRIMARY KEY,
              type TEXT NOT NULL,
              title TEXT NOT NULL,
              active_note_id TEXT,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL,
              payload_json TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS user_settings (
              key TEXT PRIMARY KEY,
              payload_json TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS agent_skills (
              id TEXT PRIMARY KEY,
              source TEXT NOT NULL,
              payload_json TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS knowledge_base_memories (
              knowledge_base_id TEXT PRIMARY KEY,
              payload_json TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS request_audit_logs (
              id TEXT PRIMARY KEY,
              kind TEXT NOT NULL,
              summary TEXT NOT NULL,
              created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS app_event_logs (
              id TEXT PRIMARY KEY,
              level TEXT NOT NULL,
              category TEXT NOT NULL,
              event TEXT NOT NULL,
              message TEXT NOT NULL,
              status TEXT NOT NULL,
              operation_id TEXT,
              session_id TEXT,
              knowledge_base_id TEXT,
              entity_type TEXT,
              entity_id TEXT,
              relative_path TEXT,
              duration_ms INTEGER,
              metadata_json TEXT,
              created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS document_history_entries (
              id TEXT PRIMARY KEY,
              target_kind TEXT NOT NULL,
              knowledge_base_id TEXT NOT NULL,
              target_id TEXT NOT NULL,
              relative_path TEXT NOT NULL,
              title TEXT NOT NULL,
              file_type TEXT NOT NULL,
              content_hash TEXT NOT NULL,
              byte_size INTEGER NOT NULL,
              line_count INTEGER NOT NULL,
              source TEXT NOT NULL,
              session_id TEXT,
              change_id TEXT,
              operation_id TEXT,
              created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_document_history_target
              ON document_history_entries(target_kind, target_id, created_at);

            CREATE INDEX IF NOT EXISTS idx_document_history_created_at
              ON document_history_entries(created_at);

            CREATE TABLE IF NOT EXISTS im_session_mappings (
              channel_key TEXT PRIMARY KEY,
              session_id TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS workspace_editor_state (
              singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
              payload_json TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS agent_session_transcripts (
              session_id TEXT PRIMARY KEY,
              payload_json TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            "#,
        )
        .map_err(|error| format!("无法初始化 SQLite schema：{error}"))?;
    ensure_audit_log_columns(&connection)?;

    let mut initialized_paths = initialized_paths
        .lock()
        .map_err(|_| "SQLite 初始化状态锁已损坏。".to_owned())?;
    initialized_paths.insert(database_path.to_path_buf());

    Ok(())
}

/** 获取 SQLite 写锁，避免同一 Tauri 进程内多个连接同时升级写事务导致 database is locked。 */
pub fn lock_database_writer() -> Result<MutexGuard<'static, ()>, String> {
    DATABASE_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "SQLite 写入锁已损坏。".to_owned())
}

/** 构造 FTS 索引快照签名，用于识别内容完全相同的重复后台刷新。 */
pub(crate) fn build_index_signature(snapshot: &WorkspaceSnapshot) -> String {
    let mut hasher = Sha256::new();

    for knowledge_base in &snapshot.knowledge_bases {
        hasher.update(knowledge_base.id.as_bytes());
        hasher.update(b"\0");
        hasher.update(knowledge_base.path.as_bytes());
        hasher.update(b"\0");
        hasher.update(knowledge_base.updated_at.as_bytes());
        hasher.update(b"\0");
        hasher.update(if knowledge_base.semantic_index_enabled {
            b"1"
        } else {
            b"0"
        });
        hasher.update(b"\0");
    }

    for note in &snapshot.notes {
        hasher.update(note.id.as_bytes());
        hasher.update(b"\0");
        hasher.update(note.knowledge_base_id.as_bytes());
        hasher.update(b"\0");
        hasher.update(note.title.as_bytes());
        hasher.update(b"\0");
        hasher.update(note.path.as_bytes());
        hasher.update(b"\0");
        hasher.update(note.content_hash.as_bytes());
        hasher.update(b"\0");
    }

    for document in &snapshot.documents {
        hasher.update(document.id.as_bytes());
        hasher.update(b"\0");
        hasher.update(document.knowledge_base_id.as_bytes());
        hasher.update(b"\0");
        hasher.update(document.path.as_bytes());
        hasher.update(b"\0");
        hasher.update(document.content_hash.as_bytes());
        hasher.update(b"\0");
    }

    format!("{:x}", hasher.finalize())
}

/** 为旧版审计表补齐 M3 需要的结构化列，避免已有用户数据阻塞启动。 */
pub(crate) fn ensure_audit_log_columns(connection: &Connection) -> Result<(), String> {
    let migration_columns = [
        ("session_id", "TEXT"),
        ("scope_summary", "TEXT NOT NULL DEFAULT ''"),
        ("content_summary", "TEXT NOT NULL DEFAULT ''"),
        ("tool_summary", "TEXT NOT NULL DEFAULT ''"),
    ];

    for (column_name, column_type) in migration_columns {
        let sql = format!("ALTER TABLE request_audit_logs ADD COLUMN {column_name} {column_type}");

        // SQLite 旧表已经有列时会返回 duplicate column name；这是幂等迁移的正常情况。
        if let Err(error) = connection.execute(&sql, []) {
            let message = error.to_string();

            if !message.contains("duplicate column name") {
                return Err(format!("无法迁移请求审计表：{error}"));
            }
        }
    }

    Ok(())
}
