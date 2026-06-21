use crate::domain::{
    AgentMessage, AgentSession, FolderEntry, KnowledgeBase, KnowledgeBaseSelection, Note,
    ScanReport, WorkspaceSnapshot,
};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
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
    let connection = Connection::open(database_path(app)?)
        .map_err(|error| format!("无法打开 SQLite：{error}"))?;

    connection
        .execute_batch(
            r#"
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

            CREATE TABLE IF NOT EXISTS request_audit_logs (
              id TEXT PRIMARY KEY,
              kind TEXT NOT NULL,
              summary TEXT NOT NULL,
              created_at TEXT NOT NULL
            );
            "#,
        )
        .map_err(|error| format!("无法初始化 SQLite schema：{error}"))?;

    Ok(connection)
}

/** 将当前快照写入 SQLite FTS5 索引，供后续真实工具检索使用。 */
pub fn index_snapshot(app: &AppHandle, snapshot: &WorkspaceSnapshot) -> Result<(), String> {
    let connection = open_database(app)?;
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| format!("无法启动索引事务：{error}"))?;

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

    transaction
        .commit()
        .map_err(|error| format!("无法提交索引事务：{error}"))
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
    let mut knowledge_bases = Vec::new();
    let mut folders = Vec::new();
    let mut notes = Vec::new();

    for stored_row in stored_rows {
        let stored_knowledge_base =
            stored_row.map_err(|error| format!("无法解析知识库记录：{error}"))?;
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
    let sessions = knowledge_bases
        .first()
        .map(|knowledge_base| vec![create_default_agent_session(knowledge_base)])
        .unwrap_or_default();
    let active_session_id = sessions
        .first()
        .map(|session| session.id.clone())
        .unwrap_or_default();

    Ok(WorkspaceSnapshot {
        knowledge_bases,
        folders,
        notes,
        sessions,
        active_knowledge_base_id,
        active_note_id,
        active_session_id,
    })
}

/** 为恢复或新增知识库创建默认 Agent 会话，绑定单个知识库作为工具范围。 */
pub fn create_default_agent_session(knowledge_base: &KnowledgeBase) -> AgentSession {
    let title = format!("{}问答助手", knowledge_base.name);

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
        created_at: "刚刚".to_owned(),
        updated_at: "刚刚".to_owned(),
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
        create_stable_note_id, hash_content, rename_markdown_file,
        resolve_existing_file_inside_root, resolve_inside_root, scan_markdown_directory,
        trash_markdown_file, validate_folder_name, validate_markdown_file_name,
        validate_new_markdown_file_name,
    };
    use crate::domain::KnowledgeBaseSelection;
    use std::fs;
    use tempfile::tempdir;

    /** hash 内容变化时必须变化，用于写入冲突检测。 */
    #[test]
    fn hash_changes_when_content_changes() {
        assert_ne!(hash_content("a"), hash_content("b"));
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
