use crate::domain::{
    AgentMessage, AgentSession, FolderEntry, KnowledgeBase, KnowledgeBaseSelection,
    ModelApiKeyStatus, ModelConfig, Note, RequestAuditLog, ScanReport, UserSettings,
    WorkspaceSnapshot,
};
use chrono::{Local, NaiveDateTime, TimeZone};
use rusqlite::{params, Connection, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tempfile::NamedTempFile;
use uuid::Uuid;
use walkdir::{DirEntry, WalkDir};

/** 扫描时跳过的大型或生成目录，避免用户选到项目根目录后长时间遍历依赖和构建产物。 */
const IGNORED_DIRECTORY_NAMES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".idea",
    ".vscode",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".nuxt",
    ".turbo",
    ".cache",
];

/** 用户设置表中的默认记录 key，首版只有一个本机用户配置。 */
const USER_SETTINGS_KEY: &str = "default";

/** SQLite 被其他连接占用时的等待时长，覆盖大知识库索引重建的正常耗时窗口。 */
const DATABASE_BUSY_TIMEOUT: Duration = Duration::from_secs(30);

/** 系统安全存储中的模型密钥引用，SQLite 只保存这个引用而不保存明文 key。 */
pub const MODEL_KEY_REFERENCE: &str = "cici-note-openai-compatible-api-key";

/** 当前桌面进程内的模型密钥缓存，用于减少同一会话内反复访问系统安全存储。 */
static MODEL_API_KEY_CACHE: OnceLock<Mutex<Option<String>>> = OnceLock::new();

/** 当前桌面进程内的 SQLite 写锁，串行化索引刷新、会话保存和轻量迁移。 */
static DATABASE_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/** 已完成 schema 初始化的 SQLite 文件路径，避免每次读命令都重复执行 DDL。 */
static INITIALIZED_DATABASE_PATHS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

/** 最近一次已完成的 FTS 快照签名，用于跳过 StrictMode/reload 的重复索引任务。 */
static COMPLETED_INDEX_SIGNATURE: OnceLock<Mutex<Option<String>>> = OnceLock::new();

/** SQLite 中持久化的知识库授权记录，用于启动时重新扫描真实目录。 */
struct StoredKnowledgeBase {
    id: String,
    name: String,
    path: String,
    semantic_index_enabled: bool,
    updated_at: String,
}

/** 计算 Markdown 内容 hash，用于确认写入前发现文件是否已被外部修改。 */
pub fn hash_content(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/** 生成本地唯一 ID，Rust 层用于新知识库、新笔记和工具调用记录。 */
pub fn create_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

/** 生成本地可读日期时间，用于长期展示的会话和审计记录时间。 */
pub(crate) fn format_local_datetime() -> String {
    Local::now().format("%Y/%m/%d %H:%M").to_string()
}

/** 将毫秒时间戳格式化为本地可读日期时间，用于迁移前端旧会话 ID 中的创建时间。 */
fn format_local_datetime_from_millis(timestamp_millis: i64) -> Option<String> {
    Local
        .timestamp_millis_opt(timestamp_millis)
        .single()
        .map(|datetime| datetime.format("%Y/%m/%d %H:%M").to_string())
}

/** 判断创建时间是否仍是旧版占位值，需要在持久化边界迁移。 */
fn is_created_at_placeholder(created_at: &str) -> bool {
    let trimmed_created_at = created_at.trim();

    trimmed_created_at.is_empty() || trimmed_created_at == "刚刚"
}

/** 从前端 createLocalId 生成的 session ID 中提取 Date.now 毫秒时间戳。 */
fn timestamp_millis_from_session_id(session_id: &str) -> Option<i64> {
    session_id
        .split('-')
        .filter_map(|part| part.parse::<i64>().ok())
        // 只接受常见 Unix 毫秒时间戳范围，避免把会话类型或随机片段误当时间。
        .find(|timestamp_millis| {
            *timestamp_millis >= 946_684_800_000 && *timestamp_millis <= 4_102_444_800_000
        })
}

/** 从前端 createLocalId 生成的 session ID 中恢复可展示创建时间。 */
fn created_at_from_session_id(session_id: &str) -> Option<String> {
    timestamp_millis_from_session_id(session_id).and_then(format_local_datetime_from_millis)
}

/** 归一化会话创建时间，避免历史列表永久显示旧版“刚刚”占位值。 */
fn normalize_session_created_at(session: &mut AgentSession) {
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

/** 将会话创建时间转换为可排序时间戳，无法解析时放到列表末尾。 */
fn session_created_sort_key(session: &AgentSession) -> i64 {
    if let Some(timestamp_millis) = timestamp_millis_from_session_id(&session.id) {
        return timestamp_millis;
    }

    NaiveDateTime::parse_from_str(&session.created_at, "%Y/%m/%d %H:%M")
        .ok()
        .and_then(|created_at| {
            // 按本地时区解释 UI 展示时间，保证与 format_local_datetime 的来源一致。
            Local
                .from_local_datetime(&created_at)
                .single()
                .map(|datetime| datetime.timestamp_millis())
        })
        .unwrap_or(0)
}

/** 按创建时间倒序整理会话历史，避免 SQLite rowid 或数组插入顺序影响展示。 */
fn sort_sessions_by_created_at_desc(sessions: &mut [AgentSession]) {
    sessions.sort_by(|left, right| {
        session_created_sort_key(right)
            .cmp(&session_created_sort_key(left))
            .then_with(|| right.created_at.cmp(&left.created_at))
    });
}

/** 归一化请求审计创建时间，避免设置页永久显示旧版“刚刚”占位值。 */
fn normalize_audit_log_created_at(log: &mut RequestAuditLog) {
    if !is_created_at_placeholder(&log.created_at) {
        return;
    }

    log.created_at = format_local_datetime();
}

/** 返回用户设置默认值；模型默认关闭，直到用户显式保存 BYOK 配置。 */
pub fn default_user_settings() -> UserSettings {
    UserSettings {
        model_config: ModelConfig {
            provider: "openai-compatible".to_owned(),
            api_base: "https://api.openai.com/v1".to_owned(),
            model: "gpt-4o-mini".to_owned(),
            key_reference: MODEL_KEY_REFERENCE.to_owned(),
            enabled: false,
        },
        privacy_policy: "allow-selected-scope".to_owned(),
        write_confirmation_required: true,
        skill_settings: crate::domain::default_skill_settings(),
    }
}

/** 根据知识库和相对路径生成稳定笔记 ID，避免重新扫描后会话引用全部失效。 */
pub fn create_stable_note_id(knowledge_base_id: &str, relative_path: &str) -> String {
    let mut hasher = Sha256::new();

    // 知识库 ID 与路径共同参与 hash，同名文件在不同知识库中不会冲突。
    hasher.update(knowledge_base_id.as_bytes());
    hasher.update(b":");
    hasher.update(relative_path.as_bytes());

    let digest = format!("{:x}", hasher.finalize());

    format!("note-{}", &digest[..24])
}

/** 根据知识库和相对目录路径生成稳定目录 ID，让空目录在重扫后仍能保持稳定节点。 */
pub fn create_stable_folder_id(knowledge_base_id: &str, relative_path: &str) -> String {
    let mut hasher = Sha256::new();

    // 目录 ID 使用独立前缀，避免与同名 Markdown 文件的稳定 ID 混淆。
    hasher.update(knowledge_base_id.as_bytes());
    hasher.update(b":folder:");
    hasher.update(relative_path.as_bytes());

    let digest = format!("{:x}", hasher.finalize());

    format!("folder-{}", &digest[..24])
}

/** 判断 WalkDir 是否应继续进入某个目录，统一约束统计和扫描的遍历范围。 */
pub fn should_walk_entry(entry: &DirEntry) -> bool {
    if entry.depth() == 0 || !entry.file_type().is_dir() {
        return true;
    }

    let Some(name) = entry.file_name().to_str() else {
        return true;
    };

    // 隐藏目录和常见构建产物通常不是用户知识内容，跳过可以明显降低误选大目录时的卡顿。
    !name.starts_with('.') && !IGNORED_DIRECTORY_NAMES.contains(&name)
}

/** 获取 SQLite 数据库路径，索引和向量都作为本地缓存保存。 */
fn database_path(app: &AppHandle) -> Result<PathBuf, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法获取应用数据目录：{error}"))?;

    fs::create_dir_all(&app_data_dir).map_err(|error| format!("无法创建应用数据目录：{error}"))?;
    Ok(app_data_dir.join("cici-note.sqlite3"))
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
fn ensure_database_schema(connection: &Connection, database_path: &Path) -> Result<(), String> {
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

            CREATE TABLE IF NOT EXISTS request_audit_logs (
              id TEXT PRIMARY KEY,
              kind TEXT NOT NULL,
              summary TEXT NOT NULL,
              created_at TEXT NOT NULL
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
fn build_index_signature(snapshot: &WorkspaceSnapshot) -> String {
    let mut hasher = Sha256::new();

    for knowledge_base in &snapshot.knowledge_bases {
        hasher.update(knowledge_base.id.as_bytes());
        hasher.update(b"\0");
        hasher.update(knowledge_base.path.as_bytes());
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

    format!("{:x}", hasher.finalize())
}

/** 为旧版审计表补齐 M3 需要的结构化列，避免已有用户数据阻塞启动。 */
fn ensure_audit_log_columns(connection: &Connection) -> Result<(), String> {
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

/** 将当前快照写入 SQLite FTS5 索引，供后续真实工具检索使用。 */
pub fn index_snapshot(app: &AppHandle, snapshot: &WorkspaceSnapshot) -> Result<(), String> {
    let index_signature = build_index_signature(snapshot);
    let mut connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    let should_rebuild_index = {
        let completed_signature = COMPLETED_INDEX_SIGNATURE
            .get_or_init(|| Mutex::new(None))
            .lock()
            .map_err(|_| "FTS 索引签名锁已损坏。".to_owned())?;

        completed_signature.as_deref() != Some(index_signature.as_str())
    };
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| format!("无法启动索引事务：{error}"))?;

    if should_rebuild_index {
        transaction
            .execute("DELETE FROM note_fts", [])
            .map_err(|error| format!("无法清理 FTS 索引：{error}"))?;
        transaction
            .execute("DELETE FROM notes", [])
            .map_err(|error| format!("无法清理笔记索引：{error}"))?;
        transaction
            .execute("DELETE FROM knowledge_bases", [])
            .map_err(|error| format!("无法清理知识库索引：{error}"))?;

        for knowledge_base in &snapshot.knowledge_bases {
            transaction
                .execute(
                    "INSERT OR REPLACE INTO knowledge_bases (id, name, path, semantic_index_enabled, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
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
            transaction
                .execute(
                    "INSERT OR REPLACE INTO notes (id, knowledge_base_id, title, path, content_hash, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![&note.id, &note.knowledge_base_id, &note.title, &note.path, &note.content_hash, &note.updated_at],
                )
                .map_err(|error| format!("无法写入笔记索引：{error}"))?;
            transaction
                .execute(
                    "INSERT INTO note_fts (note_id, knowledge_base_id, title, path, body) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![&note.id, &note.knowledge_base_id, &note.title, &note.path, &note.content],
                )
                .map_err(|error| format!("无法写入 FTS 索引：{error}"))?;
        }
    }

    persist_sessions_in_transaction(&transaction, snapshot)?;

    transaction
        .commit()
        .map_err(|error| format!("无法提交索引事务：{error}"))?;

    if should_rebuild_index {
        let mut completed_signature = COMPLETED_INDEX_SIGNATURE
            .get_or_init(|| Mutex::new(None))
            .lock()
            .map_err(|_| "FTS 索引签名锁已损坏。".to_owned())?;

        *completed_signature = Some(index_signature);
    }

    Ok(())
}

/** 在已有 SQLite 事务中读取已逻辑删除会话，供后续保存保留历史 payload。 */
fn load_deleted_sessions_in_transaction(
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
fn persist_session_in_transaction(
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
fn persist_sessions_in_transaction(
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
    session: AgentSession,
) -> Result<WorkspaceSnapshot, String> {
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

/** 删除当前会话后确保 UI 仍有一个可激活会话，必要时创建当前知识库默认会话。 */
fn ensure_visible_session_after_delete(snapshot: &mut WorkspaceSnapshot) {
    if snapshot.sessions.is_empty() {
        if let Some(knowledge_base) = snapshot
            .knowledge_bases
            .iter()
            .find(|knowledge_base| knowledge_base.id == snapshot.active_knowledge_base_id)
            .or_else(|| snapshot.knowledge_bases.first())
        {
            snapshot
                .sessions
                .push(create_default_agent_session(knowledge_base));
        }
    }

    if let Some(session) = snapshot.sessions.first() {
        snapshot.active_session_id = session.id.clone();
        snapshot.active_knowledge_base_id = session
            .knowledge_base_ids
            .first()
            .cloned()
            .unwrap_or_else(|| snapshot.active_knowledge_base_id.clone());
        snapshot.active_note_id = session.active_note_id.clone().unwrap_or_else(|| {
            snapshot
                .notes
                .iter()
                .find(|note| note.knowledge_base_id == snapshot.active_knowledge_base_id)
                .map(|note| note.id.clone())
                .unwrap_or_default()
        });
    } else {
        snapshot.active_session_id.clear();
    }
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

    sort_sessions_by_created_at_desc(&mut sessions);

    Ok(sessions)
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

/** 恢复历史会话绑定的知识库和笔记上下文。 */
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
    let next_note_id = session
        .active_note_id
        .as_ref()
        .filter(|note_id| snapshot.notes.iter().any(|note| &note.id == *note_id))
        .cloned()
        .or_else(|| {
            snapshot
                .notes
                .iter()
                .find(|note| note.knowledge_base_id == next_knowledge_base_id)
                .map(|note| note.id.clone())
        })
        .unwrap_or_default();

    snapshot.active_session_id = session.id;
    snapshot.active_knowledge_base_id = next_knowledge_base_id;
    snapshot.active_note_id = next_note_id;
    save_sessions(app, &snapshot)?;

    Ok(snapshot)
}

/** 按当前快照清理所有会话，删除已经失去有效知识库范围的会话。 */
pub fn normalize_sessions_for_snapshot(snapshot: &mut WorkspaceSnapshot) {
    let snapshot_view = WorkspaceSnapshot {
        knowledge_bases: snapshot.knowledge_bases.clone(),
        folders: snapshot.folders.clone(),
        notes: snapshot.notes.clone(),
        sessions: Vec::new(),
        active_knowledge_base_id: snapshot.active_knowledge_base_id.clone(),
        active_note_id: snapshot.active_note_id.clone(),
        active_session_id: snapshot.active_session_id.clone(),
    };

    snapshot
        .sessions
        .retain_mut(|session| normalize_session_for_snapshot(session, &snapshot_view));
    sort_sessions_by_created_at_desc(&mut snapshot.sessions);
}

/** 清理单个会话引用，返回 false 表示该会话已没有可访问知识库。 */
pub fn normalize_session_for_snapshot(
    session: &mut AgentSession,
    snapshot: &WorkspaceSnapshot,
) -> bool {
    normalize_session_created_at(session);

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
fn ordered_valid_scope_ids(
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

/** 从 SQLite 读取用户设置，缺失时返回默认未启用模型配置。 */
pub fn load_user_settings(app: &AppHandle) -> Result<UserSettings, String> {
    let connection = open_database(app)?;
    let payload_json = connection
        .query_row(
            "SELECT payload_json FROM user_settings WHERE key = ?1",
            params![USER_SETTINGS_KEY],
            |row| row.get::<_, String>(0),
        )
        .ok();

    match payload_json {
        Some(payload_json) => serde_json::from_str(&payload_json)
            .map_err(|error| format!("无法解析用户设置：{error}")),
        None => Ok(default_user_settings()),
    }
}

/** 保存用户模型和隐私设置；密钥本身由单独命令写入系统安全存储。 */
pub fn save_user_settings(app: &AppHandle, settings: &UserSettings) -> Result<(), String> {
    let connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    let payload_json =
        serde_json::to_string(settings).map_err(|error| format!("无法序列化用户设置：{error}"))?;

    connection
        .execute(
            "INSERT OR REPLACE INTO user_settings (key, payload_json, updated_at) VALUES (?1, ?2, ?3)",
            params![USER_SETTINGS_KEY, payload_json, "刚刚"],
        )
        .map_err(|error| format!("无法保存用户设置：{error}"))?;

    Ok(())
}

/** 把 BYOK 模型密钥保存到系统安全存储，避免明文进入 SQLite。 */
pub fn save_model_api_key(api_key: &str) -> Result<ModelApiKeyStatus, String> {
    ensure_persistent_model_keyring()?;

    let entry = keyring::Entry::new("Cici Note", MODEL_KEY_REFERENCE)
        .map_err(|error| format!("无法打开系统安全存储：{error}"))?;

    entry
        .set_password(api_key)
        .map_err(|error| format!("无法保存模型密钥：{error}"))?;

    let saved_api_key = entry
        .get_password()
        .map_err(|error| format!("模型密钥已提交但读回校验失败：{error}"))?;

    // 读回校验只比较是否为空，避免在错误信息或日志中暴露完整密钥内容。
    if saved_api_key.trim().is_empty() {
        return Err("模型密钥已提交但系统安全存储返回空值。".to_owned());
    }

    store_model_api_key_in_cache(&saved_api_key)?;

    Ok(ModelApiKeyStatus {
        key_reference: MODEL_KEY_REFERENCE.to_owned(),
        configured: true,
        message: "模型密钥已保存、读回校验通过，并已载入当前桌面进程。".to_owned(),
    })
}

/** 从系统安全存储读取 BYOK 模型密钥；缺失时返回 None。 */
pub fn load_model_api_key() -> Result<Option<String>, String> {
    ensure_persistent_model_keyring()?;

    if let Some(api_key) = load_model_api_key_from_cache()? {
        return Ok(Some(api_key));
    }

    let entry = keyring::Entry::new("Cici Note", MODEL_KEY_REFERENCE)
        .map_err(|error| format!("无法打开系统安全存储：{error}"))?;

    match entry.get_password() {
        Ok(api_key) if !api_key.trim().is_empty() => {
            store_model_api_key_in_cache(&api_key)?;
            Ok(Some(api_key))
        }
        Ok(_) => Ok(None),
        Err(error) => {
            let message = error.to_string();

            // 不同平台的 keyring 缺失错误文案不同，首版只把缺失视为未配置，其他错误继续暴露。
            if is_missing_keyring_entry_error(&message) {
                Ok(None)
            } else {
                Err(format!("无法读取模型密钥：{error}"))
            }
        }
    }
}

/** 查询模型密钥是否已经可读取；不会返回明文密钥。 */
pub fn load_model_api_key_status() -> Result<ModelApiKeyStatus, String> {
    let configured = load_model_api_key()?.is_some();
    let message = if configured {
        "系统安全存储中已找到模型密钥。"
    } else {
        "系统安全存储中尚未找到模型密钥。"
    };

    Ok(ModelApiKeyStatus {
        key_reference: MODEL_KEY_REFERENCE.to_owned(),
        configured,
        message: message.to_owned(),
    })
}

/** 确认当前 keyring 构建使用可跨进程持久化的系统安全存储。 */
fn ensure_persistent_model_keyring() -> Result<(), String> {
    if model_keyring_persists_until_delete() {
        return Ok(());
    }

    Err("当前构建未启用系统安全存储，模型密钥无法跨重启保存。请为 keyring 启用平台后端 feature 后重新构建。".to_owned())
}

/** 判断默认 keyring 后端是否会把密钥保存到磁盘级安全存储。 */
fn model_keyring_persists_until_delete() -> bool {
    matches!(
        keyring::default::default_credential_builder().persistence(),
        keyring::credential::CredentialPersistence::UntilDelete
    )
}

/** 把已验证密钥放入进程内缓存，避免同一桌面会话内反复访问 keychain。 */
fn store_model_api_key_in_cache(api_key: &str) -> Result<(), String> {
    let cache = MODEL_API_KEY_CACHE.get_or_init(|| Mutex::new(None));
    let mut cached_api_key = cache
        .lock()
        .map_err(|_| "模型密钥缓存已损坏。".to_owned())?;

    // 缓存只优化当前进程的重复读取，真实持久化仍完全依赖系统安全存储。
    *cached_api_key = Some(api_key.to_owned());

    Ok(())
}

/** 从进程内缓存读取模型密钥；不命中时再访问系统安全存储。 */
fn load_model_api_key_from_cache() -> Result<Option<String>, String> {
    let cache = MODEL_API_KEY_CACHE.get_or_init(|| Mutex::new(None));
    let cached_api_key = cache
        .lock()
        .map_err(|_| "模型密钥缓存已损坏。".to_owned())?;

    Ok(cached_api_key.clone())
}

/** 识别不同系统 keyring 后端返回的“条目不存在”错误文案。 */
fn is_missing_keyring_entry_error(message: &str) -> bool {
    let normalized_message = message.to_lowercase();

    normalized_message.contains("no entry found")
        || normalized_message.contains("no matching entry")
        || normalized_message.contains("not found")
        || normalized_message.contains("could not be found")
}

/** 追加一次模型请求或本地工具执行审计摘要。 */
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

/** 从 SQLite 恢复已连接知识库，并重新扫描 Markdown 文件生成可用工作台快照。 */
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

    for stored_knowledge_base in stored_knowledge_bases {
        let selection = KnowledgeBaseSelection {
            id: stored_knowledge_base.id.clone(),
            name: stored_knowledge_base.name.clone(),
            path: stored_knowledge_base.path.clone(),
            note_count: 0,
        };

        // 启动时以本地 Markdown 文件为准重新扫描，避免 SQLite 缓存覆盖用户在外部编辑器中的修改。
        match scan_markdown_directory(&selection) {
            Ok((mut knowledge_base, scanned_folders, scanned_notes)) => {
                knowledge_base.semantic_index_enabled =
                    stored_knowledge_base.semantic_index_enabled;
                knowledge_base.updated_at = stored_knowledge_base.updated_at;
                knowledge_base.is_default = knowledge_bases.is_empty();
                knowledge_base.note_count = scanned_notes.len();
                folders.extend(scanned_folders);
                notes.extend(scanned_notes);
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
                    updated_at: stored_knowledge_base.updated_at,
                    is_default: knowledge_bases.is_empty(),
                    semantic_index_enabled: stored_knowledge_base.semantic_index_enabled,
                    scan_report: Some(ScanReport {
                        scanned_file_count: 0,
                        failed_file_count: 1,
                        skipped_directories: Vec::new(),
                        errors: vec![error_message],
                    }),
                });
            }
        }
    }

    let active_knowledge_base_id = knowledge_bases
        .first()
        .map(|knowledge_base| knowledge_base.id.clone())
        .unwrap_or_default();
    let active_note_id = notes
        .iter()
        .find(|note| note.knowledge_base_id == active_knowledge_base_id)
        .or_else(|| notes.first())
        .map(|note| note.id.clone())
        .unwrap_or_default();
    let mut snapshot = WorkspaceSnapshot {
        knowledge_bases,
        folders,
        notes,
        sessions: Vec::new(),
        active_knowledge_base_id,
        active_note_id,
        active_session_id: String::new(),
    };
    snapshot.sessions = load_sessions_for_snapshot(app, &snapshot)?;

    if snapshot.sessions.is_empty() {
        snapshot.sessions = snapshot
            .knowledge_bases
            .first()
            .map(|knowledge_base| vec![create_default_agent_session(knowledge_base)])
            .unwrap_or_default();
    }

    let restored_session = snapshot.sessions.first().cloned();

    if let Some(session) = restored_session {
        snapshot.active_session_id = session.id;
        snapshot.active_knowledge_base_id = session
            .knowledge_base_ids
            .first()
            .cloned()
            .unwrap_or_else(|| snapshot.active_knowledge_base_id.clone());
        snapshot.active_note_id = session.active_note_id.unwrap_or_else(|| {
            snapshot
                .notes
                .iter()
                .find(|note| note.knowledge_base_id == snapshot.active_knowledge_base_id)
                .map(|note| note.id.clone())
                .unwrap_or_default()
        });
    }

    Ok(snapshot)
}

/** 为恢复或新增知识库创建默认 Agent 会话，绑定单个知识库作为工具范围。 */
pub fn create_default_agent_session(knowledge_base: &KnowledgeBase) -> AgentSession {
    let title = format!("{}问答助手", knowledge_base.name);
    let created_at = format_local_datetime();

    AgentSession {
        id: create_id("session-knowledge-base"),
        title: title.clone(),
        r#type: "knowledge-base".to_owned(),
        knowledge_base_ids: vec![knowledge_base.id.clone()],
        active_note_id: None,
        pinned_note_ids: Vec::new(),
        messages: vec![AgentMessage {
            id: create_id("assistant-session"),
            role: "assistant".to_owned(),
            content: format!(
                "已开启「{title}」。检索工具默认只允许访问「{}」。",
                knowledge_base.name
            ),
            action: Some("find".to_owned()),
            citations: None,
            tool_calls: Some(Vec::new()),
        }],
        pending_change: None,
        created_at: created_at.clone(),
        updated_at: created_at,
        deleted_at: None,
    }
}

/** 使用 SQLite/FTS5 索引检索会话允许范围内的笔记，失败时由 Agent 层决定是否降级。 */
pub fn search_notes(
    app: &AppHandle,
    snapshot: &WorkspaceSnapshot,
    knowledge_base_ids: &[String],
    prompt: &str,
) -> Result<Vec<crate::domain::Citation>, String> {
    let selected_ids: HashSet<&str> = knowledge_base_ids.iter().map(String::as_str).collect();

    if selected_ids.is_empty() {
        return Ok(Vec::new());
    }

    let mut citations = search_note_fts(app, snapshot, &selected_ids, prompt)?;
    let fallback_citations = search_snapshot_notes(snapshot, &selected_ids, prompt);
    let mut seen_note_ids: HashSet<String> = citations
        .iter()
        .map(|citation| citation.note_id.clone())
        .collect();

    // FTS5 对中文长句可能命中较少，补充快照子串检索保证首版中文体验可用。
    for citation in fallback_citations {
        if seen_note_ids.insert(citation.note_id.clone()) {
            citations.push(citation);
        }

        if citations.len() >= 4 {
            break;
        }
    }

    citations.sort_by(|left, right| right.score.total_cmp(&left.score));
    citations.truncate(4);

    Ok(citations)
}

/** 执行 FTS5 查询，并把索引结果转换成 Agent 可展示的引用来源。 */
fn search_note_fts(
    app: &AppHandle,
    snapshot: &WorkspaceSnapshot,
    selected_ids: &HashSet<&str>,
    prompt: &str,
) -> Result<Vec<crate::domain::Citation>, String> {
    let fts_terms = build_fts_terms(prompt);

    if fts_terms.is_empty() {
        return Ok(Vec::new());
    }

    let connection = open_database(app)?;
    let fts_query = fts_terms
        .iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let mut statement = connection
        .prepare(
            "SELECT note_id, knowledge_base_id, title, path, snippet(note_fts, 4, '', '', '...', 32), bm25(note_fts)
             FROM note_fts
             WHERE note_fts MATCH ?1
             ORDER BY bm25(note_fts)
             LIMIT 16",
        )
        .map_err(|error| format!("无法准备 FTS 检索：{error}"))?;
    let rows = statement
        .query_map(params![fts_query], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, f64>(5)?,
            ))
        })
        .map_err(|error| format!("无法执行 FTS 检索：{error}"))?;
    let mut citations = Vec::new();

    for row in rows {
        let (note_id, knowledge_base_id, title, path, snippet, rank) =
            row.map_err(|error| format!("无法读取 FTS 命中结果：{error}"))?;

        // 会话 scope 是工具权限边界，FTS 命中后仍要按本轮允许知识库过滤。
        if !selected_ids.contains(knowledge_base_id.as_str()) {
            continue;
        }

        if let Some(knowledge_base) = snapshot
            .knowledge_bases
            .iter()
            .find(|item| item.id == knowledge_base_id)
        {
            citations.push(crate::domain::Citation {
                knowledge_base_id,
                knowledge_base_name: knowledge_base.name.clone(),
                note_id,
                title,
                path,
                snippet,
                score: 1.0 / (1.0 + rank.abs()),
            });
        }
    }

    Ok(citations)
}

/** 将用户输入拆成 FTS5 查询词，避免把标点和空白带进 MATCH 语法。 */
fn build_fts_terms(prompt: &str) -> Vec<String> {
    prompt
        .split(|character: char| {
            character.is_whitespace()
                || character.is_ascii_punctuation()
                || "，。！？；：、（）《》「」".contains(character)
        })
        .map(str::trim)
        .filter(|term| term.chars().count() > 1)
        .take(8)
        .map(str::to_owned)
        .collect()
}

/** 快照级子串检索，作为 FTS5 无命中或中文分词不足时的本地降级方案。 */
fn search_snapshot_notes(
    snapshot: &WorkspaceSnapshot,
    selected_ids: &HashSet<&str>,
    prompt: &str,
) -> Vec<crate::domain::Citation> {
    let prompt_terms: Vec<String> = prompt
        .split_whitespace()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase)
        .collect();
    let mut citations: Vec<crate::domain::Citation> = snapshot
        .notes
        .iter()
        .filter(|note| selected_ids.contains(note.knowledge_base_id.as_str()))
        .filter_map(|note| {
            let searchable_text = format!(
                "{} {} {} {}",
                note.title,
                note.path,
                note.tags.join(" "),
                note.content
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
                return None;
            }

            let knowledge_base = snapshot
                .knowledge_bases
                .iter()
                .find(|item| item.id == note.knowledge_base_id)?;

            Some(crate::domain::Citation {
                knowledge_base_id: note.knowledge_base_id.clone(),
                knowledge_base_name: knowledge_base.name.clone(),
                note_id: note.id.clone(),
                title: note.title.clone(),
                path: note.path.clone(),
                snippet: extract_snippet(&note.content, prompt),
                score,
            })
        })
        .collect();

    citations.sort_by(|left, right| right.score.total_cmp(&left.score));
    citations.truncate(4);
    citations
}

/** 从 Markdown 内容中提取引用片段。 */
fn extract_snippet(content: &str, prompt: &str) -> String {
    let prompt_terms: Vec<&str> = prompt
        .split_whitespace()
        .filter(|term| !term.is_empty())
        .collect();

    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .find(|line| prompt_terms.iter().any(|term| line.contains(term)))
        .or_else(|| {
            content
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty() && !line.starts_with('#'))
        })
        .unwrap_or("命中该笔记，但暂无可展示片段。")
        .to_owned()
}

/** 从 Markdown 内容中提取首个一级标题，缺失时使用文件名。 */
pub fn extract_markdown_title(path: &Path, content: &str) -> String {
    content
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("# ")
                .map(str::trim)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("未命名笔记")
                .to_owned()
        })
}

/** 从 Markdown 内容中提取简单标签，首版支持 frontmatter tags 和正文 #tag。 */
fn extract_tags(content: &str) -> Vec<String> {
    let mut tags = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // frontmatter 中的 tags: a, b 用于兼容常见 Markdown 笔记格式。
        if let Some(raw_tags) = trimmed.strip_prefix("tags:") {
            tags.extend(
                raw_tags
                    .split(',')
                    .map(str::trim)
                    .filter(|tag| !tag.is_empty())
                    .map(str::to_owned),
            );
        }

        for token in trimmed.split_whitespace() {
            if token.starts_with('#') && token.len() > 1 {
                tags.push(token.trim_start_matches('#').trim_matches(',').to_owned());
            }
        }
    }

    tags.sort();
    tags.dedup();
    tags
}

/** 扫描用户选择的 Markdown 目录，并生成知识库、真实目录与笔记快照。 */
pub fn scan_markdown_directory(
    selection: &KnowledgeBaseSelection,
) -> Result<(KnowledgeBase, Vec<FolderEntry>, Vec<Note>), String> {
    let root = fs::canonicalize(&selection.path)
        .map_err(|error| format!("无法访问知识库目录：{error}"))?;
    let mut folders = Vec::new();
    let mut notes = Vec::new();
    let mut errors = Vec::new();
    let mut skipped_directory_set = HashSet::new();
    let root_for_filter = root.clone();

    for entry in WalkDir::new(&root).into_iter().filter_entry(|entry| {
        let should_walk = should_walk_entry(entry);

        if !should_walk {
            // 被跳过的目录写入扫描报告，帮助用户理解项目根目录为何只索引 Markdown 内容区。
            let skipped_path = entry
                .path()
                .strip_prefix(&root_for_filter)
                .unwrap_or_else(|_| entry.path())
                .to_string_lossy()
                .replace('\\', "/");

            skipped_directory_set.insert(skipped_path);
        }

        should_walk
    }) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                errors.push(format!("遍历目录失败：{error}"));
                continue;
            }
        };
        let path = entry.path();

        if path.is_dir() && entry.depth() > 0 {
            let relative_path = path
                .strip_prefix(&root)
                .map_err(|error| format!("无法计算目录相对路径：{error}"))?
                .to_string_lossy()
                .replace('\\', "/");
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("未命名目录")
                .to_owned();

            folders.push(FolderEntry {
                id: create_stable_folder_id(&selection.id, &relative_path),
                knowledge_base_id: selection.id.clone(),
                name,
                path: relative_path,
                updated_at: "刚刚".to_owned(),
            });
            continue;
        }

        // 只索引 Markdown 文件，目录已经作为真实目录节点记录，其他格式暂不进入工作区。
        if !path.is_file() || !is_markdown_file(path) {
            continue;
        }

        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(error) => {
                errors.push(format!(
                    "无法读取 Markdown 文件 {}：{error}",
                    path.display()
                ));
                continue;
            }
        };
        let relative_path = path
            .strip_prefix(&root)
            .map_err(|error| format!("无法计算相对路径：{error}"))?
            .to_string_lossy()
            .replace('\\', "/");
        let title = extract_markdown_title(path, &content);
        let tags = extract_tags(&content);

        notes.push(Note {
            id: create_stable_note_id(&selection.id, &relative_path),
            knowledge_base_id: selection.id.clone(),
            title,
            path: relative_path,
            content_hash: hash_content(&content),
            content,
            tags,
            updated_at: "刚刚".to_owned(),
            backlinks: Vec::new(),
        });
    }

    folders.sort_by(|left, right| left.path.cmp(&right.path));
    notes.sort_by(|left, right| left.path.cmp(&right.path));

    let mut skipped_directories: Vec<String> = skipped_directory_set.into_iter().collect();
    skipped_directories.sort();
    let scan_report = ScanReport {
        scanned_file_count: notes.len(),
        failed_file_count: errors.len(),
        skipped_directories,
        errors,
    };

    let knowledge_base = KnowledgeBase {
        id: selection.id.clone(),
        name: selection.name.clone(),
        path: root.to_string_lossy().to_string(),
        description: if scan_report.failed_file_count > 0 {
            format!(
                "已扫描 {} 篇 Markdown，{} 个文件失败。",
                scan_report.scanned_file_count, scan_report.failed_file_count
            )
        } else {
            "通过 Tauri 扫描的本地 Markdown 知识库。".to_owned()
        },
        status: "ready".to_owned(),
        note_count: notes.len(),
        updated_at: "刚刚".to_owned(),
        is_default: false,
        semantic_index_enabled: false,
        scan_report: Some(scan_report),
    };

    Ok((knowledge_base, folders, notes))
}

/** 判断路径是否为 Markdown 文件。 */
pub fn is_markdown_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("md") | Some("markdown") | Some("MD") | Some("MARKDOWN")
    )
}

/** 校验用户输入的新文件名，只允许当前目录下的 Markdown 文件名。 */
pub fn validate_markdown_file_name(file_name: &str) -> Result<String, String> {
    let trimmed_file_name = file_name.trim();

    if trimmed_file_name.is_empty() {
        return Err("文件名不能为空。".to_owned());
    }

    let requested_path = Path::new(trimmed_file_name);

    // 重命名只允许改当前文件名，不能携带路径分隔符或特殊路径组件。
    if requested_path.components().count() != 1
        || trimmed_file_name.contains('/')
        || trimmed_file_name.contains('\\')
        || requested_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("文件名不能包含路径或上级目录。".to_owned());
    }

    let extension = requested_path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);

    if !matches!(extension.as_deref(), Some("md") | Some("markdown")) {
        return Err("文件名必须以 .md 或 .markdown 结尾。".to_owned());
    }

    Ok(trimmed_file_name.to_owned())
}

/** 校验新建 Markdown 文件名；允许省略扩展名，省略时默认补 .md。 */
pub fn validate_new_markdown_file_name(file_name: &str) -> Result<String, String> {
    let trimmed_file_name = file_name.trim();

    if trimmed_file_name.is_empty() {
        return Err("文件名不能为空。".to_owned());
    }

    let normalized_file_name = if Path::new(trimmed_file_name).extension().is_none() {
        format!("{trimmed_file_name}.md")
    } else {
        trimmed_file_name.to_owned()
    };

    validate_markdown_file_name(&normalized_file_name)
}

/** 校验新建文件夹名，只允许单级普通目录名，并拒绝扫描忽略目录。 */
pub fn validate_folder_name(folder_name: &str) -> Result<String, String> {
    let trimmed_folder_name = folder_name.trim();

    if trimmed_folder_name.is_empty() {
        return Err("文件夹名不能为空。".to_owned());
    }

    let requested_path = Path::new(trimmed_folder_name);

    // 新建目录只允许单级名称，不能通过分隔符或特殊组件创建多级/越界路径。
    if requested_path.components().count() != 1
        || trimmed_folder_name.contains('/')
        || trimmed_folder_name.contains('\\')
        || matches!(trimmed_folder_name, "." | "..")
        || requested_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("文件夹名不能包含路径或上级目录。".to_owned());
    }

    if trimmed_folder_name.starts_with('.')
        || IGNORED_DIRECTORY_NAMES.contains(&trimmed_folder_name)
    {
        return Err("不能创建隐藏目录或扫描忽略目录。".to_owned());
    }

    Ok(trimmed_folder_name.to_owned())
}

/** 校验目标文件必须位于知识库根目录内，防止路径穿越或越权写入。 */
pub fn resolve_inside_root(root: &Path, relative_path: &str) -> Result<PathBuf, String> {
    let requested_path = Path::new(relative_path);

    // 先做纯路径组件检查，再创建父目录，避免路径穿越时在知识库外生成目录。
    if requested_path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err("目标路径超出知识库根目录，已阻止写入".to_owned());
    }

    let canonical_root =
        fs::canonicalize(root).map_err(|error| format!("无法解析知识库根目录：{error}"))?;
    let joined_path = canonical_root.join(requested_path);
    let parent = joined_path
        .parent()
        .ok_or_else(|| "目标路径缺少父目录".to_owned())?;

    fs::create_dir_all(parent).map_err(|error| format!("无法创建目标父目录：{error}"))?;
    let canonical_parent =
        fs::canonicalize(parent).map_err(|error| format!("无法解析目标父目录：{error}"))?;

    // canonicalize 目标文件本身在新建文件时会失败，所以只校验父目录边界。
    if !canonical_parent.starts_with(&canonical_root) {
        return Err("目标路径超出知识库根目录，已阻止写入".to_owned());
    }

    Ok(joined_path)
}

/** 校验已存在文件位于知识库根目录内，保存已有笔记时不创建任何新目录。 */
pub fn resolve_existing_file_inside_root(
    root: &Path,
    relative_path: &str,
) -> Result<PathBuf, String> {
    let requested_path = Path::new(relative_path);

    // 保存已有笔记只接受普通相对路径，防止前端快照被篡改后指向根目录外文件。
    if requested_path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err("目标路径超出知识库根目录，已阻止写入".to_owned());
    }

    let canonical_root =
        fs::canonicalize(root).map_err(|error| format!("无法解析知识库根目录：{error}"))?;
    let canonical_target = fs::canonicalize(canonical_root.join(requested_path))
        .map_err(|error| format!("无法解析目标 Markdown 文件：{error}"))?;

    // canonicalize 目标文件可以拦截指向根目录外的符号链接，确保保存不会越权。
    if !canonical_target.starts_with(&canonical_root) || !canonical_target.is_file() {
        return Err("目标路径超出知识库根目录，已阻止写入".to_owned());
    }

    Ok(canonical_target)
}

/** 校验父目录必须是知识库内已经存在的目录，避免新建操作隐式创建多级路径。 */
pub fn resolve_existing_directory_inside_root(
    root: &Path,
    relative_path: &str,
) -> Result<PathBuf, String> {
    let trimmed_relative_path = relative_path.trim().trim_matches('/');
    let requested_path = Path::new(trimmed_relative_path);

    if requested_path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err("目标目录超出知识库根目录，已阻止新建。".to_owned());
    }

    let canonical_root =
        fs::canonicalize(root).map_err(|error| format!("无法解析知识库根目录：{error}"))?;
    let target_path = if trimmed_relative_path.is_empty() {
        canonical_root.clone()
    } else {
        fs::canonicalize(canonical_root.join(requested_path))
            .map_err(|error| format!("无法解析目标目录：{error}"))?
    };

    // canonicalize 可以拦截指向根目录外的符号链接，确保新建不会越权。
    if !target_path.starts_with(&canonical_root) || !target_path.is_dir() {
        return Err("目标目录超出知识库根目录，已阻止新建。".to_owned());
    }

    Ok(target_path)
}

/** 在指定目录创建不覆盖已有文件的空白 Markdown，并返回相对路径。 */
pub fn create_blank_markdown_file(
    root: &Path,
    parent_relative_path: &str,
    requested_file_name: Option<&str>,
) -> Result<String, String> {
    let canonical_root =
        fs::canonicalize(root).map_err(|error| format!("无法解析知识库根目录：{error}"))?;
    let parent_path =
        resolve_existing_directory_inside_root(&canonical_root, parent_relative_path)?;
    let file_name = match requested_file_name {
        Some(file_name) => validate_new_markdown_file_name(file_name)?,
        None => next_available_markdown_file_name(&parent_path)?,
    };
    let target_path = parent_path.join(file_name);

    if target_path.exists() {
        return Err("目标文件已存在，已阻止覆盖。".to_owned());
    }

    atomic_write_markdown(&target_path, "")?;

    let canonical_target =
        fs::canonicalize(&target_path).map_err(|error| format!("无法解析新建文件：{error}"))?;

    canonical_target
        .strip_prefix(&canonical_root)
        .map_err(|error| format!("无法计算新建文件相对路径：{error}"))
        .map(|path| path.to_string_lossy().replace('\\', "/"))
}

/** 在指定目录创建单级文件夹，并返回相对路径。 */
pub fn create_folder(
    root: &Path,
    parent_relative_path: &str,
    requested_folder_name: &str,
) -> Result<String, String> {
    let canonical_root =
        fs::canonicalize(root).map_err(|error| format!("无法解析知识库根目录：{error}"))?;
    let parent_path =
        resolve_existing_directory_inside_root(&canonical_root, parent_relative_path)?;
    let folder_name = validate_folder_name(requested_folder_name)?;
    let target_path = parent_path.join(folder_name);

    if target_path.exists() {
        return Err("目标文件夹已存在，已阻止覆盖。".to_owned());
    }

    fs::create_dir(&target_path).map_err(|error| format!("无法创建文件夹：{error}"))?;

    let canonical_target =
        fs::canonicalize(&target_path).map_err(|error| format!("无法解析新建文件夹：{error}"))?;

    canonical_target
        .strip_prefix(&canonical_root)
        .map_err(|error| format!("无法计算新建文件夹相对路径：{error}"))
        .map(|path| path.to_string_lossy().replace('\\', "/"))
}

/** 生成指定目录下可用的默认 Markdown 文件名。 */
fn next_available_markdown_file_name(parent_path: &Path) -> Result<String, String> {
    for index in 1..=999 {
        let file_name = if index == 1 {
            "未命名.md".to_owned()
        } else {
            format!("未命名 {index}.md")
        };

        // 用户主动新建笔记不能覆盖已有 Markdown，遇到重名就继续寻找下一个可用文件名。
        if parent_path.join(&file_name).exists() {
            continue;
        }

        return Ok(file_name);
    }

    Err("无法生成未命名笔记路径，请清理过多未命名文件后重试。".to_owned())
}

/** 重命名已有 Markdown 文件，只修改文件名并返回新相对路径、当前正文和 hash。 */
pub fn rename_markdown_file(
    root: &Path,
    current_relative_path: &str,
    next_file_name: &str,
) -> Result<(String, String, String), String> {
    let canonical_root =
        fs::canonicalize(root).map_err(|error| format!("无法解析知识库根目录：{error}"))?;
    let current_path = resolve_existing_file_inside_root(&canonical_root, current_relative_path)?;

    if !is_markdown_file(&current_path) {
        return Err("只能重命名 Markdown 文件。".to_owned());
    }

    let safe_file_name = validate_markdown_file_name(next_file_name)?;
    let target_path = current_path.with_file_name(safe_file_name);
    let target_parent = target_path
        .parent()
        .ok_or_else(|| "目标路径缺少父目录".to_owned())?;
    let canonical_target_parent =
        fs::canonicalize(target_parent).map_err(|error| format!("无法解析目标父目录：{error}"))?;

    // 目标父目录必须仍在知识库内，防止通过异常路径或符号链接逃逸。
    if !canonical_target_parent.starts_with(&canonical_root) {
        return Err("目标路径超出知识库根目录，已阻止重命名。".to_owned());
    }

    if target_path.exists() {
        return Err("目标文件名已存在，已阻止覆盖。".to_owned());
    }

    fs::rename(&current_path, &target_path)
        .map_err(|error| format!("无法重命名 Markdown 文件：{error}"))?;

    let current_content = fs::read_to_string(&target_path)
        .map_err(|error| format!("无法读取重命名后的 Markdown 文件：{error}"))?;
    let canonical_target = fs::canonicalize(&target_path)
        .map_err(|error| format!("无法解析重命名后的文件：{error}"))?;
    let next_relative_path = canonical_target
        .strip_prefix(&canonical_root)
        .map_err(|error| format!("无法计算重命名后的相对路径：{error}"))?
        .to_string_lossy()
        .replace('\\', "/");
    let current_hash = hash_content(&current_content);

    Ok((next_relative_path, current_content, current_hash))
}

/** 将 Markdown 文件移入系统回收站，删除前用 hash 确认没有外部修改。 */
pub fn trash_markdown_file(
    root: &Path,
    relative_path: &str,
    expected_hash: &str,
) -> Result<(), String> {
    trash_markdown_file_with(root, relative_path, expected_hash, |target_path| {
        trash::delete(target_path).map_err(|error| format!("无法移入系统回收站：{error}"))
    })
}

/** 执行删除前统一校验，真实运行时注入系统回收站删除器，测试中注入可控删除器。 */
fn trash_markdown_file_with<F>(
    root: &Path,
    relative_path: &str,
    expected_hash: &str,
    delete_file: F,
) -> Result<(), String>
where
    F: FnOnce(&Path) -> Result<(), String>,
{
    let target_path = resolve_existing_file_inside_root(root, relative_path)?;

    if !is_markdown_file(&target_path) {
        return Err("只能删除 Markdown 文件。".to_owned());
    }

    let current_content = fs::read_to_string(&target_path)
        .map_err(|error| format!("无法读取待删除 Markdown 文件：{error}"))?;
    let current_hash = hash_content(&current_content);

    // 删除是破坏性操作，即使进入回收站也要先确认文件版本没有被外部编辑器改动。
    if current_hash != expected_hash {
        return Err("目标文件已被外部修改，已阻止删除。请重新扫描后再操作。".to_owned());
    }

    delete_file(&target_path)
}

/** 原子写入 Markdown 文件，避免写到一半时破坏用户数据。 */
pub fn atomic_write_markdown(path: &Path, content: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "目标路径缺少父目录".to_owned())?;
    let mut temp_file =
        NamedTempFile::new_in(parent).map_err(|error| format!("无法创建临时文件：{error}"))?;

    temp_file
        .write_all(content.as_bytes())
        .map_err(|error| format!("无法写入临时文件：{error}"))?;
    temp_file
        .persist(path)
        .map_err(|error| format!("无法替换 Markdown 文件：{}", error.error))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::trash_markdown_file_with;
    use super::{
        atomic_write_markdown, create_blank_markdown_file, create_folder, create_id,
        create_stable_note_id, ensure_persistent_model_keyring, format_local_datetime_from_millis,
        hash_content, is_missing_keyring_entry_error, load_model_api_key_from_cache,
        model_keyring_persists_until_delete, normalize_audit_log_created_at,
        normalize_session_created_at, rename_markdown_file, resolve_existing_file_inside_root,
        resolve_inside_root, scan_markdown_directory, sort_sessions_by_created_at_desc,
        store_model_api_key_in_cache, trash_markdown_file, validate_folder_name,
        validate_markdown_file_name, validate_new_markdown_file_name,
    };
    use crate::domain::{AgentSession, KnowledgeBaseSelection, RequestAuditLog};
    use std::fs;
    use tempfile::tempdir;

    /** 构造测试用 Agent 会话，避免排序和迁移测试重复铺开完整结构。 */
    fn test_agent_session(id: &str, created_at: &str) -> AgentSession {
        AgentSession {
            id: id.to_owned(),
            title: "测试会话".to_owned(),
            r#type: "task".to_owned(),
            knowledge_base_ids: vec!["kb-a".to_owned()],
            active_note_id: None,
            pinned_note_ids: Vec::new(),
            messages: Vec::new(),
            pending_change: None,
            created_at: created_at.to_owned(),
            updated_at: created_at.to_owned(),
            deleted_at: None,
        }
    }

    /** hash 内容变化时必须变化，用于写入冲突检测。 */
    #[test]
    fn hash_changes_when_content_changes() {
        assert_ne!(hash_content("a"), hash_content("b"));
    }

    /** keyring 后端的缺失条目错误应被识别为未配置，而不是模型读取故障。 */
    #[test]
    fn keyring_missing_entry_errors_are_detected() {
        assert!(is_missing_keyring_entry_error(
            "No matching entry found in secure storage"
        ));
        assert!(is_missing_keyring_entry_error(
            "The specified item could not be found in the keychain"
        ));
        assert!(!is_missing_keyring_entry_error(
            "User interaction is not allowed"
        ));
    }

    /** keyring 默认后端必须是系统级持久化存储，防止 API key 重启后丢失。 */
    #[test]
    fn model_keyring_uses_persistent_backend() {
        assert!(model_keyring_persists_until_delete());
        assert!(ensure_persistent_model_keyring().is_ok());
    }

    /** 读回校验后的密钥会进入进程缓存，供同一桌面会话内的 Agent turn 复用。 */
    #[test]
    fn model_api_key_cache_round_trips_inside_process() {
        store_model_api_key_in_cache("test-key-from-cache").unwrap();

        assert_eq!(
            load_model_api_key_from_cache().unwrap(),
            Some("test-key-from-cache".to_owned())
        );
    }

    /** 旧版会话如果把创建时间保存成“刚刚”，应优先从前端会话 ID 的时间戳恢复。 */
    #[test]
    fn normalize_session_created_at_uses_timestamp_from_frontend_id() {
        let timestamp_millis = 1_700_000_000_000;
        let mut session =
            test_agent_session(&format!("session-task-{timestamp_millis}-abc123"), "刚刚");
        let expected_created_at = format_local_datetime_from_millis(timestamp_millis).unwrap();

        normalize_session_created_at(&mut session);

        assert_eq!(session.created_at, expected_created_at);
    }

    /** 会话历史必须按创建时间倒序展示，同一分钟内依赖 ID 毫秒时间戳保持稳定。 */
    #[test]
    fn sort_sessions_by_created_at_desc_uses_id_timestamp() {
        let mut sessions = vec![
            test_agent_session("session-task-1700000000000-old", "2023/11/14 22:13"),
            test_agent_session("session-task-1700000030000-new", "2023/11/14 22:13"),
            test_agent_session("session-task-1699999940000-earliest", "2023/11/14 22:12"),
        ];

        sort_sessions_by_created_at_desc(&mut sessions);

        let session_ids = sessions
            .iter()
            .map(|session| session.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            session_ids,
            vec![
                "session-task-1700000030000-new",
                "session-task-1700000000000-old",
                "session-task-1699999940000-earliest",
            ]
        );
    }

    /** 旧版审计日志如果保存成“刚刚”，读取或写入前应改成具体本地时间。 */
    #[test]
    fn normalize_audit_log_created_at_replaces_placeholder() {
        let mut log = RequestAuditLog {
            id: "audit-a".to_owned(),
            kind: "model_turn".to_owned(),
            session_id: Some("session-a".to_owned()),
            scope_summary: "测试知识库".to_owned(),
            content_summary: "模型请求".to_owned(),
            tool_summary: "model_request".to_owned(),
            created_at: "刚刚".to_owned(),
        };

        normalize_audit_log_created_at(&mut log);

        assert_ne!(log.created_at, "刚刚");
        assert!(!log.created_at.trim().is_empty());
    }

    /** 路径穿越必须被阻止，防止 Agent 写出知识库根目录。 */
    #[test]
    fn reject_path_outside_root() {
        let dir = tempdir().unwrap();
        let result = resolve_inside_root(dir.path(), "../outside.md");

        assert!(result.is_err());
    }

    /** 路径穿越被拒绝时不应提前创建知识库外部目录。 */
    #[test]
    fn reject_path_outside_root_without_creating_parent() {
        let dir = tempdir().unwrap();
        let outside_name = format!("cici-note-outside-parent-{}", create_id("test"));
        let outside_parent = dir.path().parent().unwrap().join(&outside_name);
        let result = resolve_inside_root(dir.path(), &format!("../{outside_name}/outside.md"));

        assert!(result.is_err());
        assert!(!outside_parent.exists());
    }

    /** 原子写入应在目标路径生成完整文件。 */
    #[test]
    fn atomic_write_creates_markdown_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("note.md");

        atomic_write_markdown(&path, "# Title").unwrap();

        assert_eq!(fs::read_to_string(path).unwrap(), "# Title");
    }

    /** 保存已有笔记的路径解析不应创建缺失父目录。 */
    #[test]
    fn existing_file_resolver_does_not_create_missing_parent() {
        let dir = tempdir().unwrap();
        let result = resolve_existing_file_inside_root(dir.path(), "missing/note.md");

        assert!(result.is_err());
        assert!(!dir.path().join("missing").exists());
    }

    /** 新建 Markdown 文档应生成唯一文件名，不覆盖已有未命名文件。 */
    #[test]
    fn create_blank_markdown_file_uses_unique_path() {
        let dir = tempdir().unwrap();
        let first_path = create_blank_markdown_file(dir.path(), "", None).unwrap();
        let second_path = create_blank_markdown_file(dir.path(), "", None).unwrap();

        assert_eq!(first_path, "未命名.md");
        assert_eq!(second_path, "未命名 2.md");
        assert_eq!(fs::read_to_string(dir.path().join(first_path)).unwrap(), "");
        assert_eq!(
            fs::read_to_string(dir.path().join(second_path)).unwrap(),
            ""
        );
    }

    /** 根目录新建文档允许省略扩展名，并默认补齐 .md。 */
    #[test]
    fn create_markdown_file_in_root_appends_default_extension() {
        let dir = tempdir().unwrap();
        let relative_path = create_blank_markdown_file(dir.path(), "", Some("Root Note")).unwrap();

        assert_eq!(relative_path, "Root Note.md");
        assert_eq!(
            fs::read_to_string(dir.path().join("Root Note.md")).unwrap(),
            ""
        );
    }

    /** 子目录新建文档必须落在用户点击的目录下，不再由当前笔记上下文推断。 */
    #[test]
    fn create_markdown_file_in_child_directory() {
        let dir = tempdir().unwrap();

        fs::create_dir(dir.path().join("Child")).unwrap();

        let relative_path =
            create_blank_markdown_file(dir.path(), "Child", Some("Nested.md")).unwrap();

        assert_eq!(relative_path, "Child/Nested.md");
        assert!(dir.path().join("Child").join("Nested.md").exists());
    }

    /** 新建文档应拒绝路径穿越、重复名称和非 Markdown 扩展名。 */
    #[test]
    fn create_markdown_file_rejects_invalid_or_existing_targets() {
        let dir = tempdir().unwrap();

        fs::write(dir.path().join("taken.md"), "# Taken").unwrap();

        assert!(validate_new_markdown_file_name("../x.md").is_err());
        assert!(validate_new_markdown_file_name("").is_err());
        assert!(validate_new_markdown_file_name("note.txt").is_err());
        assert!(create_blank_markdown_file(dir.path(), "", Some("taken.md")).is_err());
        assert!(create_blank_markdown_file(dir.path(), "../outside", Some("x.md")).is_err());
    }

    /** 根目录新建文件夹成功后返回相对于知识库根目录的路径。 */
    #[test]
    fn create_folder_in_root_directory() {
        let dir = tempdir().unwrap();
        let relative_path = create_folder(dir.path(), "", "New Folder").unwrap();

        assert_eq!(relative_path, "New Folder");
        assert!(dir.path().join("New Folder").is_dir());
    }

    /** 子目录新建文件夹只创建单级子目录，并保留父目录结构。 */
    #[test]
    fn create_folder_in_child_directory() {
        let dir = tempdir().unwrap();

        fs::create_dir(dir.path().join("Parent")).unwrap();

        let relative_path = create_folder(dir.path(), "Parent", "Child").unwrap();

        assert_eq!(relative_path, "Parent/Child");
        assert!(dir.path().join("Parent").join("Child").is_dir());
    }

    /** 新建文件夹必须拒绝路径穿越、隐藏目录、扫描忽略目录和重复名称。 */
    #[test]
    fn create_folder_rejects_invalid_or_existing_targets() {
        let dir = tempdir().unwrap();

        fs::create_dir(dir.path().join("taken")).unwrap();

        assert!(validate_folder_name("../x").is_err());
        assert!(validate_folder_name("").is_err());
        assert!(validate_folder_name(".hidden").is_err());
        assert!(validate_folder_name("node_modules").is_err());
        assert!(create_folder(dir.path(), "", "taken").is_err());
        assert!(create_folder(dir.path(), "../outside", "x").is_err());
    }

    /** 重命名应拒绝路径穿越、空名和非 Markdown 扩展名。 */
    #[test]
    fn rename_rejects_invalid_file_names() {
        assert!(validate_markdown_file_name("../x.md").is_err());
        assert!(validate_markdown_file_name("").is_err());
        assert!(validate_markdown_file_name("note.txt").is_err());
    }

    /** 重命名不能覆盖同目录下已有 Markdown 文件。 */
    #[test]
    fn rename_rejects_existing_target() {
        let dir = tempdir().unwrap();

        fs::write(dir.path().join("old.md"), "# Old").unwrap();
        fs::write(dir.path().join("taken.md"), "# Taken").unwrap();

        let result = rename_markdown_file(dir.path(), "old.md", "taken.md");

        assert!(result.is_err());
        assert!(dir.path().join("old.md").exists());
        assert_eq!(
            fs::read_to_string(dir.path().join("taken.md")).unwrap(),
            "# Taken"
        );
    }

    /** 重命名成功后原路径消失，新路径保留原始正文和 hash。 */
    #[test]
    fn rename_preserves_content_and_hash() {
        let dir = tempdir().unwrap();
        let old_path = dir.path().join("old.md");

        fs::write(&old_path, "# Old\n\n正文").unwrap();

        let (next_relative_path, content, content_hash) =
            rename_markdown_file(dir.path(), "old.md", "new.md").unwrap();

        assert_eq!(next_relative_path, "new.md");
        assert_eq!(content, "# Old\n\n正文");
        assert_eq!(content_hash, hash_content("# Old\n\n正文"));
        assert!(!old_path.exists());
        assert_eq!(
            fs::read_to_string(dir.path().join("new.md")).unwrap(),
            "# Old\n\n正文"
        );
    }

    /** 删除 hash 不一致时必须拒绝，避免误删外部编辑器刚改过的文件。 */
    #[test]
    fn delete_rejects_hash_mismatch() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("note.md");

        fs::write(&path, "# Changed").unwrap();

        let result = trash_markdown_file(dir.path(), "note.md", &hash_content("# Original"));

        assert!(result.is_err());
        assert!(path.exists());
    }

    /** 删除路径越界必须拒绝。 */
    #[test]
    fn delete_rejects_path_outside_root() {
        let dir = tempdir().unwrap();
        let result = trash_markdown_file(dir.path(), "../outside.md", &hash_content(""));

        assert!(result.is_err());
    }

    /** 删除成功后文件应离开原路径，由系统回收站负责恢复能力。 */
    #[test]
    fn delete_moves_file_out_of_original_path() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("note.md");
        let content = "# Trash me";

        fs::write(&path, content).unwrap();
        trash_markdown_file_with(
            dir.path(),
            "note.md",
            &hash_content(content),
            |target_path| {
                fs::remove_file(target_path).map_err(|error| format!("测试删除失败：{error}"))
            },
        )
        .unwrap();

        assert!(!path.exists());
    }

    /** 稳定笔记 ID 必须只由知识库和路径决定，确保重扫后引用仍可匹配。 */
    #[test]
    fn stable_note_id_uses_knowledge_base_and_path() {
        let first_id = create_stable_note_id("kb-a", "A/Note.md");
        let second_id = create_stable_note_id("kb-a", "A/Note.md");
        let other_knowledge_base_id = create_stable_note_id("kb-b", "A/Note.md");

        assert_eq!(first_id, second_id);
        assert_ne!(first_id, other_knowledge_base_id);
    }

    /** 扫描应跳过大型依赖目录，并把坏 Markdown 文件作为报告错误而不是整库失败。 */
    #[test]
    fn scan_reports_failed_files_and_skipped_directories() {
        let dir = tempdir().unwrap();
        let valid_path = dir.path().join("notes").join("ok.md");
        let invalid_path = dir.path().join("broken.md");
        let skipped_path = dir.path().join("node_modules").join("ignored.md");

        fs::create_dir_all(valid_path.parent().unwrap()).unwrap();
        fs::create_dir_all(skipped_path.parent().unwrap()).unwrap();
        fs::write(&valid_path, "# 可读笔记\n\n正文").unwrap();
        fs::write(&invalid_path, [0xff, 0xfe, 0xfd]).unwrap();
        fs::write(&skipped_path, "# 忽略").unwrap();

        let selection = KnowledgeBaseSelection {
            id: "kb-test".to_owned(),
            name: "测试库".to_owned(),
            path: dir.path().to_string_lossy().to_string(),
            note_count: 0,
        };
        let (knowledge_base, folders, notes) = scan_markdown_directory(&selection).unwrap();
        let report = knowledge_base.scan_report.unwrap();

        assert_eq!(notes.len(), 1);
        assert!(folders.iter().any(|folder| folder.path == "notes"));
        assert_eq!(report.scanned_file_count, 1);
        assert_eq!(report.failed_file_count, 1);
        assert_eq!(report.skipped_directories, vec!["node_modules"]);
        assert!(report.errors[0].contains("broken.md"));
    }

    /** 扫描应返回没有 Markdown 文件的空目录，让前端目录树能显示真实空文件夹。 */
    #[test]
    fn scan_returns_empty_folder_nodes() {
        let dir = tempdir().unwrap();

        fs::create_dir(dir.path().join("Empty")).unwrap();

        let selection = KnowledgeBaseSelection {
            id: "kb-empty".to_owned(),
            name: "空目录测试库".to_owned(),
            path: dir.path().to_string_lossy().to_string(),
            note_count: 0,
        };
        let (_knowledge_base, folders, notes) = scan_markdown_directory(&selection).unwrap();

        assert!(notes.is_empty());
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].path, "Empty");
    }
}
