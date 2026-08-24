use super::registry::{
    parse_limit_arg, read_window, slice_chars, tool_path_arg, truncation_hint, write_kind,
};
use super::types::*;
use crate::domain::AgentTurnRequest;
use crate::domain::{
    AgentSecurityLevel, AgentSession, Citation, ProposedChange, ProposedChangeSet,
    ProposedFileOperation, SkillExecutionRequest, WorkspaceDocument, WorkspaceSnapshot,
    AGENT_DIRECT_EXECUTION_ID, AGENT_DIRECT_SOURCE,
};
use crate::storage::{create_id, format_local_datetime, hash_content};
use crate::text_edit::{
    count_non_overlapping_matches, replace_occurrence, replace_unique, OccurrenceReplacementError,
    UniqueReplacementError,
};
use serde_json::{json, Value};
use std::collections::HashSet;
use tauri::AppHandle;

/** run_skill 只创建结构化执行请求；真正启动进程由独立审批命令和沙箱运行器负责。 */
pub(crate) struct RunSkillTool;

impl AgentTool for RunSkillTool {
    fn name(&self) -> &'static str {
        "run"
    }

    fn description(&self) -> &'static str {
        "Request execution of an enabled Skill entry script inside Orange's isolated workspace. The user may need to approve it before execution."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "skillId": { "type": "string" },
                "arguments": {
                    "type": "array",
                    "items": { "type": "string" },
                    "maxItems": 32,
                    "description": "Non-secret arguments passed as separate process arguments."
                }
            },
            "required": ["skillId"]
        })
    }

    fn execute(&self, context: &mut AgentToolContext<'_>, args: &Value) -> ToolExecutionResult {
        execute_run_skill_request(context, args)
    }
}

/** 校验 Skill 当前包 hash、运行时和会话 scope，然后生成可审计的待审批执行请求。 */
pub(crate) fn execute_run_skill_request(
    context: &mut AgentToolContext<'_>,
    args: &Value,
) -> ToolExecutionResult {
    let Some(app) = context.app else {
        return ToolExecutionResult::failed("当前环境不支持执行 Skill。");
    };
    let session = &context.snapshot.sessions[context.session_index];
    if session.im_identity.is_some() || session.security_level == "basic" {
        return ToolExecutionResult::failed("当前会话安全级别不允许执行 Skill。");
    }
    if session
        .pending_execution
        .as_ref()
        .is_some_and(|request| request.status == "pending")
    {
        return ToolExecutionResult::failed("当前会话已有待确认执行，请先处理。");
    }

    let skill_id = args
        .get("skillId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(skill_id) = skill_id else {
        return ToolExecutionResult::failed("run_skill 缺少 skillId。");
    };
    let arguments = args
        .get("arguments")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .take(32)
                .map(|value| value.chars().take(1000).collect::<String>())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let connection = match crate::storage::open_database(app) {
        Ok(connection) => connection,
        Err(error) => return ToolExecutionResult::failed(&error),
    };
    let skill = match crate::skills::load_agent_skills(app, &connection).and_then(|skills| {
        skills
            .into_iter()
            .find(|skill| skill.id == skill_id && skill.enabled)
            .ok_or_else(|| "找不到已启用的 Skill。".to_owned())
    }) {
        Ok(skill) => skill,
        Err(error) => return ToolExecutionResult::failed(&error),
    };
    let Some(manifest) = skill.runtime_manifest.as_ref() else {
        return ToolExecutionResult::failed("该 Skill 是纯指令 Skill，没有可执行入口。");
    };
    let Some(compatibility) = skill.compatibility.as_ref() else {
        return ToolExecutionResult::failed("无法确认 Skill 兼容性，已拒绝执行。");
    };
    if compatibility.status != "ready" {
        return ToolExecutionResult::failed(&format!(
            "Skill 当前不可执行：{}。",
            compatibility.status
        ));
    }

    let request = SkillExecutionRequest {
        id: create_id("execution"),
        skill_id: skill.id.clone(),
        skill_name: skill.display_name.clone(),
        package_hash: compatibility.package_hash.clone(),
        runtime: manifest.runtime.clone(),
        command_preview: format!(
            "{} {}（{} 个参数）",
            manifest.runtime,
            manifest.entry,
            arguments.len()
        ),
        args: arguments,
        knowledge_base_ids: session.knowledge_base_ids.clone(),
        network_domains: manifest.network_domains.clone(),
        credential_aliases: manifest.credential_aliases.clone(),
        status: "pending".to_owned(),
        created_at: format_local_datetime(),
    };
    context.snapshot.sessions[context.session_index].pending_execution = Some(request.clone());
    context.snapshot.sessions[context.session_index].updated_at = format_local_datetime();

    ToolExecutionResult {
        success: true,
        summary: format!("已创建 Skill「{}」执行请求。", request.skill_name),
        payload: json!({
            "executionId": request.id,
            "skillId": request.skill_id,
            "status": request.status,
            "requiresApproval": true
        }),
        citations: Vec::new(),
        audit_fragment: Some(format!(
            "Skill 执行请求：skill_id={} runtime={} scope_count={} network_domain_count={}",
            request.skill_id,
            request.runtime,
            request.knowledge_base_ids.len(),
            request.network_domains.len()
        )),
    }
}

/** 把历史 noteId 参数转换为统一 fileId，避免旧会话重试失效。 */

/** search_notes 工具，在当前会话授权知识库内执行 SQLite/FTS 检索。 */
pub(crate) struct SearchNotesTool;

impl AgentTool for SearchNotesTool {
    fn name(&self) -> &'static str {
        "search"
    }

    fn description(&self) -> &'static str {
        "Search in the selected scope. Default target=notes uses Markdown FTS and returns citations (limit 4, max 16). In full/autonomous mode, target=path with path scans UTF-8 text files under a compliant directory; this is still search, not a new grep tool. If truncated, narrow the query or raise limit."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 16,
                    "description": "Maximum citations or path hits to return. Defaults to 4."
                },
                "target": {
                    "type": "string",
                    "enum": ["notes", "path"],
                    "description": "notes searches the knowledge-base index. path scans a compliant directory in full/autonomous mode."
                },
                "path": {
                    "type": "string",
                    "description": "Directory to scan when target=path. Relative paths stay inside the knowledge base."
                },
                "knowledgeBaseId": { "type": "string" }
            },
            "required": ["query"]
        })
    }

    fn execute(&self, context: &mut AgentToolContext<'_>, args: &Value) -> ToolExecutionResult {
        execute_search(context, args)
    }
}

/** read 工具：按 id 读 scope 内文件；无 id 时读当前激活文件；DOCX/PDF 走只读抽取。 */
pub(crate) struct ReadFileTool;

impl AgentTool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read"
    }

    fn description(&self) -> &'static str {
        "Read one file in the selected scope. Omit fileId to read the current active file. Markdown and TXT return editable text; DOCX and PDF return extracted read-only text with page or structure blocks and are never edited. Default window is 6000 characters from offset 0. If truncated, call again with offset=nextOffset (or page=N for PDF). In full/autonomous mode, path may be a knowledge-base relative path or a compliant absolute filesystem path; protected system directories are rejected."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "fileId": {
                    "type": "string",
                    "description": "Note or document id. Omit to read the current active file."
                },
                "documentId": {
                    "type": "string",
                    "description": "Legacy alias for fileId when reading DOCX/PDF."
                },
                "path": {
                    "type": "string",
                    "description": "Filesystem path. Only available in full/autonomous mode. Relative paths stay inside the knowledge base; absolute or ~-prefixed paths may target a compliant location."
                },
                "knowledgeBaseId": {
                    "type": "string",
                    "description": "Target knowledge base id for relative path reads."
                },
                "offset": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Character offset to start reading. Defaults to 0. Use nextOffset from a truncated result to continue."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Maximum characters to return. Defaults to 6000 and cannot exceed 6000."
                },
                "page": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional 1-based PDF page. Ignored for Markdown/TXT."
                }
            }
        })
    }

    fn execute(&self, context: &mut AgentToolContext<'_>, args: &Value) -> ToolExecutionResult {
        execute_read(context, args)
    }
}

/** list 工具，列出当前 scope 内目录、Markdown 笔记和支持文档摘要，供模型判断下一步。 */
pub(crate) struct ListTreeTool;

impl AgentTool for ListTreeTool {
    fn name(&self) -> &'static str {
        "list"
    }

    fn description(&self) -> &'static str {
        "List folders, Markdown notes, and supported document metadata inside the selected scope. The result includes every scoped knowledge base (even empty ones) with id and name, and every item carries knowledgeBaseName so files are not attributed to another knowledge base that happens to share a folder name. It does not read non-Markdown document contents. Default item cap is 120 per list. If truncated, narrow prefix or fileType, or increase limit. In full/autonomous mode, path may list a knowledge-base relative directory or a compliant absolute filesystem path; protected system directories are rejected."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Optional directory. Omit to list the selected knowledge-base tree. In full/autonomous mode, relative paths stay inside the knowledge base; absolute or ~-prefixed paths may target a compliant location."
                },
                "knowledgeBaseId": {
                    "type": "string",
                    "description": "Target knowledge base id for relative path listings."
                },
                "prefix": {
                    "type": "string",
                    "description": "Keep items whose path starts with this prefix."
                },
                "fileType": {
                    "type": "string",
                    "description": "Optional filter: markdown, txt, docx, pdf, image, or folder."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Maximum items per list. Defaults to 120 and cannot exceed 120."
                }
            }
        })
    }

    fn execute(&self, context: &mut AgentToolContext<'_>, args: &Value) -> ToolExecutionResult {
        execute_list(context, args)
    }
}

/** edit 工具，只创建待确认 diff，不直接写 Markdown/TXT 文件。 */
pub(crate) struct ProposeFileChangeTool;

impl AgentTool for ProposeFileChangeTool {
    fn name(&self) -> &'static str {
        "edit"
    }

    fn description(&self) -> &'static str {
        "Create a pending rewrite diff for an existing editable Markdown or TXT file."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "fileId": { "type": "string" },
                "title": { "type": "string" },
                "operation": {
                    "type": "string",
                    "enum": ["replace", "append", "multi_replace"],
                    "description": "Use replace for one unique fragment, append for end-of-note additions, and multi_replace when one request needs multiple unique edits in the same note. For replace, next is only the replacement for original. For append, next is only the increment. For multi_replace, provide edits instead of a full document."
                },
                "original": { "type": "string" },
                "next": { "type": "string", "description": "Replacement text for replace, or increment-only text for append. It may be empty for deletion in replace mode." },
                "edits": {
                    "type": "array",
                    "description": "Multiple unique replacements to apply to the same note in one pending diff. Each original must match exactly once in the current note; next may be empty to delete that fragment.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "original": { "type": "string" },
                            "next": { "type": "string" },
                            "occurrence": {
                                "type": "integer",
                                "minimum": 1,
                                "description": "Optional 1-based match index. Use this only when original appears multiple times and the edit intentionally targets a specific occurrence, such as deleting duplicate paragraphs while keeping the first copy."
                            }
                        },
                        "required": ["original", "next"]
                    }
                }
            },
            "required": ["fileId"]
        })
    }

    fn execute(&self, context: &mut AgentToolContext<'_>, args: &Value) -> ToolExecutionResult {
        execute_propose_file_change(context.snapshot, context.session_index, args)
    }
}

/** write 工具，只创建待确认新建 Markdown/TXT diff，不直接落盘。 */
pub(crate) struct CreateFileDraftTool;

impl AgentTool for CreateFileDraftTool {
    fn name(&self) -> &'static str {
        "write"
    }

    fn description(&self) -> &'static str {
        "Create a pending new Markdown or TXT draft, or a pending folder. Use kind=file (default) with fileType markdown|txt and content. Use kind=folder with targetPath; basic sessions cannot create folders. Writes still require confirmation except auto-apply in full mode."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["file", "folder"],
                    "description": "file creates a Markdown/TXT draft; folder creates a directory. Basic sessions reject folder."
                },
                "knowledgeBaseId": { "type": "string" },
                "targetPath": { "type": "string" },
                "fileType": { "type": "string", "enum": ["markdown", "txt", "folder"] },
                "title": { "type": "string" },
                "content": { "type": "string" }
            },
            "required": ["targetPath"]
        })
    }

    fn execute(&self, context: &mut AgentToolContext<'_>, args: &Value) -> ToolExecutionResult {
        execute_write(context, args)
    }
}

/** 闭集 search：默认笔记 FTS；完全级别可用 target=path 扫描目录。 */
pub(crate) fn execute_search(
    context: &mut AgentToolContext<'_>,
    args: &Value,
) -> ToolExecutionResult {
    let target = args
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or("notes")
        .trim()
        .to_ascii_lowercase();
    if target == "path" || tool_path_arg(args).is_some() {
        return execute_search_path(context, args);
    }
    execute_search_notes(context, args)
}

/** 完全级别下对合规目录做内容命中，仍叫 search，不新增 grep。 */
pub(crate) fn execute_search_path(
    context: &mut AgentToolContext<'_>,
    args: &Value,
) -> ToolExecutionResult {
    let session = &context.snapshot.sessions[context.session_index];
    if session.im_identity.is_some()
        || !AgentSecurityLevel::parse(&session.security_level).allows_external_filesystem()
    {
        return ToolExecutionResult::failed("当前会话安全级别不允许对知识库外路径做内容检索。");
    }
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if query.is_empty() {
        return ToolExecutionResult::failed("path 检索需要非空 query。");
    }
    let raw_path = tool_path_arg(args).unwrap_or(".");
    let requested_knowledge_base_id = args
        .get("knowledgeBaseId")
        .or_else(|| args.get("knowledge_base_id"))
        .and_then(Value::as_str);
    let resolved = match crate::fs_guard::resolve_agent_fs_target(
        &context.snapshot.knowledge_bases,
        &session.knowledge_base_ids,
        &context.request.active_knowledge_base_id,
        &session.security_level,
        requested_knowledge_base_id,
        raw_path,
        true,
    ) {
        Ok(resolved) => resolved,
        Err(message) => return ToolExecutionResult::failed(&message),
    };
    let limit = parse_limit_arg(args, DEFAULT_SEARCH_LIMIT, MAX_SEARCH_LIMIT);
    let query_lower = query.to_ascii_lowercase();
    let mut hits = Vec::new();
    let mut scanned = 0usize;
    for entry in walkdir::WalkDir::new(&resolved.absolute_path)
        .follow_links(false)
        .max_depth(6)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        scanned += 1;
        if scanned > 400 {
            break;
        }
        let bytes = match std::fs::read(entry.path()) {
            Ok(bytes) if bytes.len() <= MAX_READ_PATH_BYTES => bytes,
            _ => continue,
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        if !text.to_ascii_lowercase().contains(&query_lower) {
            continue;
        }
        let snippet = text
            .lines()
            .find(|line| line.to_ascii_lowercase().contains(&query_lower))
            .unwrap_or(text)
            .chars()
            .take(240)
            .collect::<String>();
        hits.push(json!({
            "path": entry.path().to_string_lossy(),
            "snippet": snippet
        }));
        if hits.len() >= limit {
            break;
        }
    }
    let truncated = hits.len() >= limit;
    let hint = truncation_hint("search", truncated, None);
    ToolExecutionResult {
        success: true,
        summary: format!(
            "在路径中检索到 {} 条命中{}",
            hits.len(),
            if truncated { "（已达 limit）" } else { "" }
        ),
        payload: json!({
            "target": "path",
            "hits": hits,
            "truncated": truncated,
            "limit": limit,
            "hint": hint
        }),
        citations: Vec::new(),
        audit_fragment: Some(format!(
            "search target=path query_chars={} hits={}",
            query.chars().count(),
            hits.len()
        )),
    }
}

/** 执行 search_notes，并把引用同步给前端消息展示。 */
pub(crate) fn execute_search_notes(
    context: &mut AgentToolContext<'_>,
    args: &Value,
) -> ToolExecutionResult {
    let Some(app) = context.app else {
        return ToolExecutionResult::failed("当前运行环境无法访问本地检索索引。");
    };
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let limit = parse_limit_arg(args, DEFAULT_SEARCH_LIMIT, MAX_SEARCH_LIMIT);

    match crate::storage::search_notes(
        app,
        context.snapshot,
        &context.snapshot.sessions[context.session_index].knowledge_base_ids,
        query,
        limit,
    ) {
        Ok(citations) => {
            let truncated = citations.len() >= limit;
            let bounded_citations: Vec<Citation> =
                citations.into_iter().map(budget_citation).collect();
            let audit_titles = bounded_citations
                .iter()
                .take(4)
                .map(|citation| format!("《{}》", citation.title))
                .collect::<Vec<_>>()
                .join("、");
            let hint = truncation_hint("search", truncated, None);

            ToolExecutionResult {
                success: true,
                summary: format!(
                    "在会话允许范围内检索到 {} 条候选引用{}",
                    bounded_citations.len(),
                    if truncated {
                        "（已达 limit，可收窄 query 或提高 limit）"
                    } else {
                        ""
                    }
                ),
                payload: json!({
                    "citations": &bounded_citations,
                    "truncated": truncated,
                    "limit": limit,
                    "hint": hint
                }),
                citations: bounded_citations,
                audit_fragment: Some(format!(
                    "search_notes 查询「{}」，返回 {}",
                    truncate_chars(query, 80),
                    if audit_titles.is_empty() {
                        "空结果".to_owned()
                    } else {
                        audit_titles
                    }
                )),
            }
        }
        Err(error) => ToolExecutionResult::failed(&format!("检索失败：{error}")),
    }
}

/** 闭集 read：无 id 时读当前激活文件；带 path 时走完全级别文件系统读取。 */
pub(crate) fn execute_read(
    context: &mut AgentToolContext<'_>,
    args: &Value,
) -> ToolExecutionResult {
    if tool_path_arg(args).is_some() {
        return execute_read_path(context, args);
    }

    let mut file_id = args
        .get("fileId")
        .or_else(|| args.get("file_id"))
        .or_else(|| args.get("documentId"))
        .or_else(|| args.get("document_id"))
        .or_else(|| args.get("noteId"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();

    if file_id.is_empty() {
        file_id = if !context.snapshot.active_note_id.is_empty() {
            context.snapshot.active_note_id.clone()
        } else {
            context.snapshot.active_document_id.clone()
        };
    }

    if file_id.is_empty() {
        return ToolExecutionResult::failed(
            "没有可读取的目标：请提供 fileId，或先在界面打开一个文件。",
        );
    }

    let mut read_args = args.clone();
    if let Some(object) = read_args.as_object_mut() {
        object.insert("fileId".to_owned(), json!(file_id));
        object.insert("documentId".to_owned(), json!(file_id));
    }
    if scoped_note(context.snapshot, context.session_index, &file_id).is_some()
        || scoped_text_document(context.snapshot, context.session_index, &file_id).is_some()
    {
        return execute_read_file(context.snapshot, context.session_index, &read_args);
    }
    if scoped_readonly_document(context.snapshot, context.session_index, &file_id).is_some() {
        return execute_read_document(context.snapshot, context.session_index, &read_args);
    }

    ToolExecutionResult::failed(
        "目标文件不在当前会话允许范围内，或不是可读取的 Markdown/TXT/DOCX/PDF 文件。",
    )
}

/** 执行 read_file；TXT 不生成知识库引用，避免扩大 Markdown 检索引用语义。 */
pub(crate) fn execute_read_file(
    snapshot: &WorkspaceSnapshot,
    session_index: usize,
    args: &Value,
) -> ToolExecutionResult {
    let file_id = args
        .get("fileId")
        .or_else(|| args.get("file_id"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Some(note) = scoped_note(snapshot, session_index, file_id) {
        let knowledge_base = snapshot
            .knowledge_bases
            .iter()
            .find(|knowledge_base| knowledge_base.id == note.knowledge_base_id);
        let citation = Citation {
            knowledge_base_id: note.knowledge_base_id.clone(),
            knowledge_base_name: knowledge_base
                .map(|knowledge_base| knowledge_base.name.clone())
                .unwrap_or_else(|| "未知知识库".to_owned()),
            note_id: note.id.clone(),
            title: note.title.clone(),
            path: note.path.clone(),
            snippet: note
                .content
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty() && !line.starts_with('#'))
                .unwrap_or("已读取该笔记。")
                .to_owned(),
            score: 1.0,
            location: None,
        };
        let note_content_chars = note.content.chars().count();
        let (offset, limit) = read_window(args);
        let (bounded_content, truncated, next_offset) = slice_chars(&note.content, offset, limit);
        let hint = truncation_hint("read", truncated, next_offset);

        return ToolExecutionResult {
            success: true,
            summary: format!(
                "已读取笔记《{}》{}",
                note.title,
                if truncated {
                    "（已截断，可用 offset 续读）"
                } else {
                    ""
                }
            ),
            payload: json!({
                "truncated": truncated,
                "nextOffset": next_offset,
                "hint": hint,
                "offset": offset,
                "limit": limit,
                "note": {
                    "id": &note.id,
                    "knowledgeBaseId": &note.knowledge_base_id,
                    "title": &note.title,
                    "path": &note.path,
                    "tags": &note.tags,
                    "updatedAt": &note.updated_at,
                    "contentHash": &note.content_hash,
                    "content": bounded_content,
                    "contentChars": note_content_chars,
                    "contentTruncated": truncated
                }
            }),
            citations: vec![citation],
            audit_fragment: Some(format!(
                "read_file type=markdown path={} chars={} offset={}{}",
                note.path,
                bounded_content.chars().count(),
                offset,
                if truncated { "（已截断）" } else { "" }
            )),
        };
    }

    let Some(document) = scoped_text_document(snapshot, session_index, file_id) else {
        return ToolExecutionResult::failed(
            "目标文件不在当前会话允许范围内，或不是可编辑的 Markdown/TXT 文件。",
        );
    };
    let content = document.content.as_deref().unwrap_or_default();
    let content_chars = content.chars().count();
    let (offset, limit) = read_window(args);
    let (bounded_content, truncated, next_offset) = slice_chars(content, offset, limit);
    let hint = truncation_hint("read", truncated, next_offset);

    ToolExecutionResult {
        success: true,
        summary: format!(
            "已读取 TXT 文件《{}》{}",
            document.title,
            if truncated {
                "（已截断，可用 offset 续读）"
            } else {
                ""
            }
        ),
        payload: json!({
            "truncated": truncated,
            "nextOffset": next_offset,
            "hint": hint,
            "offset": offset,
            "limit": limit,
            "file": {
                "id": &document.id, "knowledgeBaseId": &document.knowledge_base_id,
                "title": &document.title, "path": &document.path, "fileType": "txt",
                "updatedAt": &document.updated_at, "contentHash": &document.content_hash,
                "content": bounded_content, "contentChars": content_chars,
                "contentTruncated": truncated
            }
        }),
        citations: Vec::new(),
        audit_fragment: Some(format!(
            "read_file type=txt path={} chars={} offset={}{}",
            document.path,
            bounded_content.chars().count(),
            offset,
            if truncated { "（已截断）" } else { "" }
        )),
    }
}

/** 执行只读文档读取；正文仅在本次工具结果中传给模型，不写入工作台快照或审计日志。 */
pub(crate) fn execute_read_document(
    snapshot: &WorkspaceSnapshot,
    session_index: usize,
    args: &Value,
) -> ToolExecutionResult {
    let document_id = args
        .get("documentId")
        .or_else(|| args.get("document_id"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(document) = scoped_readonly_document(snapshot, session_index, document_id) else {
        return ToolExecutionResult::failed(
            "目标文档不在当前会话允许范围内，或不是可读取的 DOCX/PDF 文件。",
        );
    };
    let Some(knowledge_base) = snapshot
        .knowledge_bases
        .iter()
        .find(|item| item.id == document.knowledge_base_id)
    else {
        return ToolExecutionResult::failed("找不到文档所属知识库。");
    };

    let extraction = match crate::storage::extract_document_text(
        std::path::Path::new(&knowledge_base.path),
        document,
    ) {
        Ok(extraction) => extraction,
        Err(error) => return ToolExecutionResult::failed(&format!("文档读取失败：{error}")),
    };
    let (offset, limit) = read_window(args);
    let requested_page = args
        .get("page")
        .and_then(Value::as_u64)
        .map(|page| page as usize);
    let mut remaining_skip = offset;
    let mut remaining_take = limit;
    let mut truncated = false;
    let source_blocks = extraction.blocks.iter().filter(|block| {
        requested_page
            .map(|page| block.page == Some(page))
            .unwrap_or(true)
    });
    let blocks = source_blocks
        .filter_map(|block| {
            if remaining_take == 0 {
                truncated = true;
                return None;
            }
            let original_chars = block.text.chars().count();
            if remaining_skip >= original_chars {
                remaining_skip -= original_chars;
                return None;
            }
            let text: String = block
                .text
                .chars()
                .skip(remaining_skip)
                .take(remaining_take)
                .collect();
            remaining_skip = 0;
            remaining_take = remaining_take.saturating_sub(text.chars().count());
            if remaining_take == 0 && offset + limit < extraction.content_chars {
                truncated = true;
            }
            Some(json!({ "index": block.index, "type": &block.r#type, "text": text, "page": block.page }))
        })
        .collect::<Vec<_>>();
    truncated |= remaining_take == 0 && extraction.content_chars > offset + limit;
    let next_offset = truncated.then_some(offset + limit);
    let hint = truncation_hint("read", truncated, next_offset);
    let first_block = extraction.blocks.first();
    let citation = Citation {
        knowledge_base_id: document.knowledge_base_id.clone(),
        knowledge_base_name: knowledge_base.name.clone(),
        note_id: document.id.clone(),
        title: document.title.clone(),
        path: document.path.clone(),
        snippet: first_block
            .map(|block| truncate_chars(&block.text, 500))
            .unwrap_or_else(|| "未提取到文本。".to_owned()),
        score: 1.0,
        location: first_block.map(|block| {
            block
                .page
                .map(|page| format!("第 {page} 页"))
                .unwrap_or_else(|| format!("结构块 {}", block.index))
        }),
    };
    ToolExecutionResult {
        success: true,
        summary: format!(
            "已读取 {} 文档《{}》{}",
            document.file_type.to_ascii_uppercase(),
            document.title,
            if truncated {
                "（已截断，可用 offset 或 page 续读）"
            } else {
                ""
            }
        ),
        payload: json!({
            "truncated": truncated,
            "nextOffset": next_offset,
            "hint": hint,
            "offset": offset,
            "limit": limit,
            "document": {
                "id": &document.id, "knowledgeBaseId": &document.knowledge_base_id,
                "title": &document.title, "path": &document.path, "fileType": &document.file_type,
                "contentHash": extraction.content_hash, "blocks": blocks,
                "contentChars": extraction.content_chars, "contentTruncated": truncated,
                "warnings": extraction.warnings
            }
        }),
        citations: vec![citation],
        audit_fragment: Some(format!(
            "read_document type={} path={} blocks={} chars={} truncated={} warnings={}",
            document.file_type,
            document.path,
            extraction.blocks.len(),
            extraction.content_chars.min(MAX_READ_NOTE_CHARS),
            truncated,
            extraction.warnings.len()
        )),
    }
}

/** 闭集 list：无 path 时列知识库树；带 path 时走完全级别目录列出。 */
pub(crate) fn execute_list(
    context: &mut AgentToolContext<'_>,
    args: &Value,
) -> ToolExecutionResult {
    if tool_path_arg(args).is_some() {
        return execute_list_path(context, args);
    }
    execute_list_tree(context.snapshot, context.session_index, args)
}

/** 闭集 write：kind=folder 建目录，否则建 Markdown/TXT 草稿。 */
pub(crate) fn execute_write(
    context: &mut AgentToolContext<'_>,
    args: &Value,
) -> ToolExecutionResult {
    if write_kind(args) == "folder" {
        return execute_create_folder(context, args);
    }
    execute_create_file_draft(
        context.snapshot,
        context.session_index,
        context.request,
        args,
    )
}

/** 执行 list_tree，只返回当前 scope 内的目录、Markdown 笔记和普通文档元数据。 */
pub(crate) fn execute_list_tree(
    snapshot: &WorkspaceSnapshot,
    session_index: usize,
    args: &Value,
) -> ToolExecutionResult {
    let session = &snapshot.sessions[session_index];
    let scope_ids = scope_id_set(session);
    let prefix = args
        .get("prefix")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let file_type_filter = args
        .get("fileType")
        .or_else(|| args.get("type"))
        .and_then(Value::as_str)
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    let item_limit = parse_limit_arg(args, MAX_TREE_ITEMS, MAX_TREE_ITEMS);
    let scoped_folders: Vec<_> = snapshot
        .folders
        .iter()
        .filter(|folder| scope_ids.contains(folder.knowledge_base_id.as_str()))
        .filter(|folder| {
            prefix
                .map(|prefix| folder.path.starts_with(prefix) || folder.name.contains(prefix))
                .unwrap_or(true)
        })
        .filter(|_| {
            file_type_filter
                .as_deref()
                .map(|file_type| file_type == "folder")
                .unwrap_or(true)
        })
        .collect();
    let scoped_notes: Vec<_> = snapshot
        .notes
        .iter()
        .filter(|note| scope_ids.contains(note.knowledge_base_id.as_str()))
        .filter(|note| {
            prefix
                .map(|prefix| note.path.starts_with(prefix) || note.title.contains(prefix))
                .unwrap_or(true)
        })
        .filter(|_| {
            file_type_filter
                .as_deref()
                .map(|file_type| file_type == "markdown" || file_type == "md")
                .unwrap_or(true)
        })
        .collect();
    let scoped_documents: Vec<_> = snapshot
        .documents
        .iter()
        .filter(|document| scope_ids.contains(document.knowledge_base_id.as_str()))
        .filter(|document| {
            prefix
                .map(|prefix| document.path.starts_with(prefix) || document.title.contains(prefix))
                .unwrap_or(true)
        })
        .filter(|document| {
            file_type_filter
                .as_deref()
                .map(|file_type| document.file_type == file_type)
                .unwrap_or(true)
        })
        .collect();
    let file_type_counts = build_list_tree_file_type_counts(scoped_notes.len(), &scoped_documents);
    let total_files = scoped_notes.len() + scoped_documents.len();
    // 按会话 scope 顺序输出知识库清单，空知识库也必须出现，避免模型把同名文件夹当成另一个知识库。
    let knowledge_bases: Vec<_> = session
        .knowledge_base_ids
        .iter()
        .filter_map(|knowledge_base_id| {
            snapshot
                .knowledge_bases
                .iter()
                .find(|knowledge_base| knowledge_base.id == *knowledge_base_id)
        })
        .map(|knowledge_base| {
            json!({
                "id": &knowledge_base.id,
                "name": &knowledge_base.name,
                "folderCount": scoped_folders
                    .iter()
                    .filter(|folder| folder.knowledge_base_id == knowledge_base.id)
                    .count(),
                "noteCount": scoped_notes
                    .iter()
                    .filter(|note| note.knowledge_base_id == knowledge_base.id)
                    .count(),
                "documentCount": scoped_documents
                    .iter()
                    .filter(|document| document.knowledge_base_id == knowledge_base.id)
                    .count()
            })
        })
        .collect();
    let folders: Vec<_> = scoped_folders
        .iter()
        .take(item_limit)
        .map(|folder| {
            json!({
                "id": folder.id,
                "name": folder.name,
                "path": folder.path,
                "knowledgeBaseId": folder.knowledge_base_id,
                "knowledgeBaseName": knowledge_base_display_name(snapshot, &folder.knowledge_base_id)
            })
        })
        .collect();
    let notes: Vec<_> = scoped_notes
        .iter()
        .take(item_limit)
        .map(|note| {
            json!({
                "id": note.id,
                "title": note.title,
                "path": note.path,
                "knowledgeBaseId": note.knowledge_base_id,
                "knowledgeBaseName": knowledge_base_display_name(snapshot, &note.knowledge_base_id)
            })
        })
        .collect();
    let documents: Vec<_> = scoped_documents
        .iter()
        .take(item_limit)
        .map(|document| {
            json!({
                "id": &document.id,
                "title": &document.title,
                "path": &document.path,
                "knowledgeBaseId": &document.knowledge_base_id,
                "knowledgeBaseName": knowledge_base_display_name(snapshot, &document.knowledge_base_id),
                "fileType": &document.file_type,
                "previewAvailable": document.preview_available,
                "agentReadable": matches!(document.file_type.as_str(), "txt" | "docx" | "pdf"),
                "readOnly": matches!(document.file_type.as_str(), "docx" | "pdf")
            })
        })
        .collect();
    let truncated = scoped_folders.len() > item_limit
        || scoped_notes.len() > item_limit
        || scoped_documents.len() > item_limit;
    let hint = truncation_hint("list", truncated, None);

    log::debug!(
        target: "agent_tools",
        "list_tree 完成：session={} folder_count={} markdown_count={} document_count={} total_files={} truncated={} type_markdown={} type_txt={} type_docx={} type_pdf={} type_image={}",
        snapshot.sessions[session_index].id,
        scoped_folders.len(),
        scoped_notes.len(),
        scoped_documents.len(),
        total_files,
        truncated,
        file_type_counts.markdown,
        file_type_counts.txt,
        file_type_counts.docx,
        file_type_counts.pdf,
        file_type_counts.image
    );

    ToolExecutionResult {
        success: true,
        summary: format!(
            "已列出 {} 个目录、{} 篇 Markdown 和 {} 个普通文档{}",
            scoped_folders.len(),
            scoped_notes.len(),
            scoped_documents.len(),
            if truncated {
                "，结果已按预算截断"
            } else {
                ""
            }
        ),
        payload: json!({
            "knowledgeBases": knowledge_bases,
            "folders": folders,
            "notes": notes,
            "documents": documents,
            "totalFolders": scoped_folders.len(),
            "totalNotes": scoped_notes.len(),
            "totalDocuments": scoped_documents.len(),
            "totalFiles": total_files,
            "fileTypeCounts": file_type_counts.to_json(),
            "truncated": truncated,
            "limit": item_limit,
            "hint": hint
        }),
        citations: Vec::new(),
        audit_fragment: Some(format!(
            "list_tree 发送 {} 个目录摘要、{} 篇 Markdown 摘要、{} 个普通文档摘要{}",
            scoped_folders.len().min(item_limit),
            scoped_notes.len().min(item_limit),
            scoped_documents.len().min(item_limit),
            if truncated { "（已截断）" } else { "" }
        )),
    }
}

/** 执行 get_session_summary，只返回当前会话工作记忆和 diff 状态摘要。 */
#[allow(dead_code)]
pub(crate) fn execute_get_session_summary(
    snapshot: &WorkspaceSnapshot,
    session_index: usize,
) -> ToolExecutionResult {
    let session = &snapshot.sessions[session_index];
    let pending_change_summary = session.pending_change.as_ref().map(|change| {
        json!({
            "id": &change.id,
            "type": &change.r#type,
            "operation": change.operation.as_deref().unwrap_or("create"),
            "title": &change.title,
            "targetPath": &change.target_path,
            "status": &change.status,
            "diffStats": &change.diff_stats,
            "originalChars": change.original.chars().count(),
            "nextChars": change.next.chars().count(),
        })
    });

    ToolExecutionResult {
        success: true,
        summary: "已读取当前会话工作记忆摘要".to_owned(),
        payload: json!({
            "contextSummary": &session.context_summary,
            "pendingChange": pending_change_summary,
            "messageCount": session.messages.len()
        }),
        citations: Vec::new(),
        audit_fragment: Some(format!(
            "get_session_summary 发送工作记忆字段 summary_present={} message_count={} pending_change={}",
            session.context_summary.is_some(),
            session.messages.len(),
            session.pending_change.as_ref().map(|change| change.status.as_str()).unwrap_or("none")
        )),
    }
}

/** 执行 get_knowledge_base_memory，只返回当前会话 scope 内已启用记忆的脱敏摘要。 */
#[allow(dead_code)]
pub(crate) fn execute_get_knowledge_base_memory(
    app: Option<&AppHandle>,
    snapshot: &WorkspaceSnapshot,
    session_index: usize,
) -> ToolExecutionResult {
    let session = &snapshot.sessions[session_index];
    let knowledge_base_ids = session.knowledge_base_ids.clone();

    // 没有持久化 app 句柄（例如单元测试）时直接返回空集，避免误以为已检索到结果。
    let mut entries = Vec::new();
    let mut kb_count = 0usize;
    let mut kb_names = Vec::new();

    if let Some(app) = app {
        for knowledge_base_id in &knowledge_base_ids {
            match crate::storage::load_knowledge_base_memory(app, knowledge_base_id) {
                Ok(Some(memory)) if memory.enabled => {
                    kb_count += 1;
                    let kb_name = snapshot
                        .knowledge_bases
                        .iter()
                        .find(|knowledge_base| knowledge_base.id == memory.knowledge_base_id)
                        .map(|knowledge_base| knowledge_base.name.clone())
                        .unwrap_or_else(|| memory.knowledge_base_id.clone());
                    kb_names.push(kb_name.clone());
                    for entry in &memory.entries {
                        if entry.content.trim().is_empty() {
                            continue;
                        }
                        if entries.len() >= MAX_KB_MEMORY_TOOL_ENTRIES {
                            break;
                        }
                        // 工具返回前再次脱敏，防止旧数据或手动改库绕过保存入口后进入模型。
                        let redacted_content =
                            crate::storage::redact_memory_secrets(entry.content.trim());
                        entries.push(json!({
                            "category": entry.category,
                            "content": truncate_chars(&redacted_content, MAX_KB_MEMORY_TOOL_ENTRY_CHARS),
                            "source": entry.source
                        }));
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    log::warn!(
                        target: "agent_tools",
                        "get_knowledge_base_memory 读取失败：knowledge_base_id_chars={} error={}",
                        knowledge_base_id.chars().count(),
                        crate::logging::sanitize_log_text(&error)
                    );
                }
            }
        }
    }

    ToolExecutionResult {
        success: true,
        summary: "已读取当前知识库跨会话记忆".to_owned(),
        payload: json!({
            "knowledgeBases": kb_names,
            "entries": entries,
        }),
        citations: Vec::new(),
        audit_fragment: Some(format!(
            "get_knowledge_base_memory 发送跨会话记忆 kb_count={} entry_count={}",
            kb_count,
            entries.len()
        )),
    }
}

/** 执行 search_session_messages，只在当前会话消息和工具摘要内做大小写不敏感匹配。 */
#[allow(dead_code)]
pub(crate) fn execute_search_session_messages(
    snapshot: &WorkspaceSnapshot,
    session_index: usize,
    args: &Value,
) -> ToolExecutionResult {
    let session = &snapshot.sessions[session_index];
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();

    if query.is_empty() {
        return ToolExecutionResult::failed("会话历史检索 query 不能为空。");
    }

    let query_lower = query.to_lowercase();
    let mut matches = session
        .messages
        .iter()
        .enumerate()
        .filter(|(_, message)| {
            session_message_search_text(message)
                .to_lowercase()
                .contains(&query_lower)
        })
        .map(|(index, message)| {
            json!({
                "index": index + 1,
                "id": &message.id,
                "role": &message.role,
                "action": message.action.as_deref(),
                "preview": truncate_chars(&message.content, 260),
            })
        })
        .collect::<Vec<_>>();
    let truncated = matches.len() > MAX_SESSION_CONTEXT_MESSAGES;
    if truncated {
        let start = matches.len() - MAX_SESSION_CONTEXT_MESSAGES;
        matches = matches.split_off(start);
    }
    let match_count = matches.len();

    ToolExecutionResult {
        success: true,
        summary: format!("会话历史检索命中 {match_count} 条消息"),
        payload: json!({ "matches": matches, "truncated": truncated, "maxReturned": MAX_SESSION_CONTEXT_MESSAGES }),
        citations: Vec::new(),
        audit_fragment: Some(format!(
            "search_session_messages query_chars={} match_count={}",
            query.chars().count(),
            match_count
        )),
    }
}

/** 执行 read_session_context，按 messageId 精确读取或按 1-based 索引读取受限范围。 */
#[allow(dead_code)]
pub(crate) fn execute_read_session_context(
    snapshot: &WorkspaceSnapshot,
    session_index: usize,
    args: &Value,
) -> ToolExecutionResult {
    let session = &snapshot.sessions[session_index];
    let selected = if let Some(message_id) = args.get("messageId").and_then(Value::as_str) {
        session
            .messages
            .iter()
            .enumerate()
            .filter(|(_, message)| message.id == message_id)
            .collect::<Vec<_>>()
    } else {
        let start_index = args
            .get("startIndex")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1) as usize;
        let end_index = args
            .get("endIndex")
            .and_then(Value::as_u64)
            .map(|value| value.max(1) as usize)
            .unwrap_or(start_index);
        let start = start_index.min(end_index).saturating_sub(1);
        let end = start_index.max(end_index).min(session.messages.len());

        session
            .messages
            .iter()
            .enumerate()
            .skip(start)
            .take(end.saturating_sub(start).min(MAX_SESSION_CONTEXT_MESSAGES))
            .collect::<Vec<_>>()
    };

    if selected.is_empty() {
        return ToolExecutionResult::failed("未找到匹配的会话历史消息。");
    }

    let messages = selected
        .iter()
        .map(|(index, message)| {
            json!({
                "index": index + 1,
                "id": &message.id,
                "role": &message.role,
                "action": message.action.as_deref(),
                "content": truncate_chars(&message.content, MAX_SESSION_CONTEXT_MESSAGE_CHARS),
                "toolSummaries": message.tool_calls.as_ref().map(|tool_calls| {
                    tool_calls.iter().map(|tool_call| {
                        json!({ "name": &tool_call.name, "status": &tool_call.status, "summary": &tool_call.summary })
                    }).collect::<Vec<_>>()
                }).unwrap_or_default(),
            })
        })
        .collect::<Vec<_>>();
    let message_count = messages.len();

    ToolExecutionResult {
        success: true,
        summary: format!("已读取 {message_count} 条会话历史消息"),
        payload: json!({ "messages": messages, "maxReturned": MAX_SESSION_CONTEXT_MESSAGES }),
        citations: Vec::new(),
        audit_fragment: Some(format!(
            "read_session_context message_count={message_count}"
        )),
    }
}

/** 构造会话历史检索文本，仅在内存中使用，不进入审计日志。 */
#[allow(dead_code)]
pub(crate) fn session_message_search_text(message: &crate::domain::AgentMessage) -> String {
    let mut parts = vec![message.content.clone()];

    if let Some(tool_calls) = &message.tool_calls {
        parts.extend(tool_calls.iter().map(|tool_call| tool_call.summary.clone()));
    }

    parts.join("\n")
}

impl ListTreeFileTypeCounts {
    /** 转成模型可读 JSON，固定输出五个支持类型 key，便于调用方稳定解析。 */
    fn to_json(&self) -> Value {
        json!({
            "markdown": self.markdown,
            "txt": self.txt,
            "docx": self.docx,
            "pdf": self.pdf,
            "image": self.image
        })
    }
}

/** 汇总 list_tree 返回范围内的文件类型数量，不读取普通文档正文或 hash。 */
pub(crate) fn build_list_tree_file_type_counts(
    markdown_count: usize,
    documents: &[&WorkspaceDocument],
) -> ListTreeFileTypeCounts {
    let mut counts = ListTreeFileTypeCounts {
        markdown: markdown_count,
        txt: 0,
        docx: 0,
        pdf: 0,
        image: 0,
    };

    for document in documents {
        // file_type 来自扫描白名单；未知历史值不进入固定计数，避免误导模型能力边界。
        match document.file_type.as_str() {
            "txt" => counts.txt += 1,
            "docx" => counts.docx += 1,
            "pdf" => counts.pdf += 1,
            "image" => counts.image += 1,
            _ => {}
        }
    }

    counts
}

/** 同一文件已有 pending 时拒绝后写覆盖先写。 */
pub(crate) fn pending_write_conflict(
    session: &AgentSession,
    file_id: Option<&str>,
    target_path: Option<&str>,
) -> Option<String> {
    let pending = session
        .pending_change
        .as_ref()
        .filter(|change| change.status == "pending")?;
    let same_id = file_id.filter(|id| !id.is_empty()).is_some_and(|id| {
        pending.target_id.as_deref() == Some(id) || pending.note_id.as_deref() == Some(id)
    });
    let same_path = target_path
        .filter(|path| !path.is_empty())
        .is_some_and(|path| pending.target_path == path);
    if same_id || same_path {
        Some("同一文件已有待确认变更，请先处理后再继续编辑。".to_owned())
    } else {
        None
    }
}

/** 执行 propose_file_change，只创建待确认 diff，不直接写 Markdown/TXT 文件。 */
pub(crate) fn execute_propose_file_change(
    snapshot: &mut WorkspaceSnapshot,
    session_index: usize,
    args: &Value,
) -> ToolExecutionResult {
    let file_id = args
        .get("fileId")
        .or_else(|| args.get("file_id"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Some(message) =
        pending_write_conflict(&snapshot.sessions[session_index], Some(file_id), None)
    {
        return ToolExecutionResult::failed(&message);
    }
    let target = if let Some(note) = scoped_note(snapshot, session_index, file_id) {
        (
            note.id.clone(),
            note.knowledge_base_id.clone(),
            note.title.clone(),
            note.path.clone(),
            note.content.clone(),
            note.content_hash.clone(),
            "note",
            "markdown",
        )
    } else if let Some(document) = scoped_text_document(snapshot, session_index, file_id) {
        (
            document.id.clone(),
            document.knowledge_base_id.clone(),
            document.title.clone(),
            document.path.clone(),
            document.content.clone().unwrap_or_default(),
            document.content_hash.clone(),
            "document",
            "txt",
        )
    } else {
        return ToolExecutionResult::failed(
            "目标文件不在当前会话允许范围内，或不是可编辑的 Markdown/TXT 文件。",
        );
    };
    let operation = args
        .get("operation")
        .or_else(|| args.get("mode"))
        .and_then(Value::as_str)
        .filter(|value| matches!(*value, "append" | "replace" | "multi_replace"))
        .unwrap_or("replace");
    let (original, next) =
        match prepare_rewrite_content(&target.4, operation, args, target.7 == "markdown") {
            Ok(prepared_change) => prepared_change,
            Err(message) => return ToolExecutionResult::failed(&message),
        };

    let change = ProposedChange {
        id: create_id("change"),
        knowledge_base_id: target.1.clone(),
        note_id: (target.6 == "note").then(|| target.0.clone()),
        target_id: Some(target.0.clone()),
        target_kind: Some(target.6.to_owned()),
        file_type: Some(target.7.to_owned()),
        r#type: "rewrite".to_owned(),
        operation: Some(operation.to_owned()),
        title: args
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("改写《{}》", target.2)),
        target_path: target.3.clone(),
        original,
        next,
        original_hash: target.5.clone(),
        status: "pending".to_owned(),
        review_comments: None,
        review_state: None,
        diff_stats: None,
    };

    snapshot.sessions[session_index].pending_change = Some(change.clone());
    let audit_fragment = Some(format!(
        "propose_file_change type={} path={} operation={} original_chars={} next_chars={}",
        target.7,
        target.3,
        operation,
        change.original.chars().count(),
        change.next.chars().count()
    ));

    ToolExecutionResult {
        success: true,
        summary: format!("已为《{}》生成待确认改写 diff", target.2),
        payload: json!({ "change": &change }),
        citations: Vec::new(),
        audit_fragment,
    }
}

/** 根据 operation 准备待审阅 diff 的原文和建议内容，不在日志或错误里回显正文。 */
pub(crate) fn prepare_rewrite_content(
    content: &str,
    operation: &str,
    args: &Value,
    is_markdown: bool,
) -> Result<(String, String), String> {
    match operation {
        "append" => prepare_append_rewrite(content, args, is_markdown),
        "multi_replace" => prepare_multi_replace_rewrite(content, args),
        _ => prepare_single_replace_rewrite(content, args),
    }
}

/** 准备单处替换，original 必须唯一命中，next 可以为空以支持删除。 */
pub(crate) fn prepare_single_replace_rewrite(
    content: &str,
    args: &Value,
) -> Result<(String, String), String> {
    let original = args
        .get("original")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| first_body_paragraph(content));
    let Some(next) = args.get("next").and_then(Value::as_str).map(str::to_owned) else {
        return Err("改写工具缺少 next 内容；如需删除，请显式传入空字符串。".to_owned());
    };

    if original.is_empty() {
        return Err("改写工具缺少 original 内容。".to_owned());
    }

    if looks_like_full_document_replacement_mismatch(content, &original, &next) {
        return Err(
            "改写工具疑似把整篇改后文档放进 next，但 original 只是一段局部内容。已拒绝生成会导致正文重复的 diff；如需文末追加，请使用 operation=append，并只把增量内容放入 next。"
                .to_owned(),
        );
    }

    validate_unique_original(content, &original).map_err(single_rewrite_validation_message)?;

    Ok((original, next))
}

/** 准备文末追加，工具层合成整篇 diff，避免模型把整篇正文塞进局部替换。 */
pub(crate) fn prepare_append_rewrite(
    content: &str,
    args: &Value,
    is_markdown: bool,
) -> Result<(String, String), String> {
    let addition = args
        .get("next")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();

    if addition.trim().is_empty() {
        return Err("文末追加工具缺少增量内容。".to_owned());
    }

    // TXT 不修剪或注入 Markdown 分隔空行，确保 Agent 纯文本写入保持原始语义。
    let next = if is_markdown {
        append_note_content(content, &addition)
    } else {
        format!("{content}{addition}")
    };
    Ok((content.to_owned(), next))
}

/** 准备同一文件内多处替换，先按唯一片段顺序应用到内存，再生成整篇待确认 diff。 */
pub(crate) fn prepare_multi_replace_rewrite(
    content: &str,
    args: &Value,
) -> Result<(String, String), String> {
    let edits = parse_text_edits(args)?;
    let next = apply_multi_text_edits(content, &edits)?;

    if next == content {
        return Err("多处编辑没有产生内容变化，已拒绝生成空 diff。".to_owned());
    }

    Ok((content.to_owned(), next))
}

/** 从工具参数读取 edits/replacements，正文只保存在 pending diff，不进入日志。 */
pub(crate) fn parse_text_edits(args: &Value) -> Result<Vec<ProposedTextEdit>, String> {
    let Some(raw_edits_value) = args.get("edits").or_else(|| args.get("replacements")) else {
        return Err("多处编辑需要提供 edits 数组。".to_owned());
    };
    let parsed_string_edits;
    let raw_edits = if let Some(raw_edits) = raw_edits_value.as_array() {
        raw_edits
    } else if let Some(raw_edits_text) = raw_edits_value.as_str() {
        // 某些 DSML 兼容服务会把数组参数作为字符串输出；这里仅解析 JSON，不记录原文内容。
        parsed_string_edits = serde_json::from_str::<Value>(raw_edits_text)
            .map_err(|_| "多处编辑的 edits 字符串不是有效 JSON 数组。".to_owned())?;
        parsed_string_edits
            .as_array()
            .ok_or_else(|| "多处编辑的 edits 字符串不是 JSON 数组。".to_owned())?
    } else {
        return Err("多处编辑需要提供 edits 数组。".to_owned());
    };
    let mut edits = Vec::with_capacity(raw_edits.len());

    for (index, raw_edit) in raw_edits.iter().enumerate() {
        let original = raw_edit
            .get("original")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("多处编辑第 {} 处缺少 original。", index + 1))?;
        let next = raw_edit
            .get("next")
            .or_else(|| raw_edit.get("replacement"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("多处编辑第 {} 处缺少 next。", index + 1))?;
        let occurrence = raw_edit
            .get("occurrence")
            .or_else(|| raw_edit.get("matchIndex"))
            .or_else(|| raw_edit.get("match_index"))
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value > 0);

        edits.push(ProposedTextEdit {
            original,
            next,
            occurrence,
        });
    }

    if edits.is_empty() {
        Err("多处编辑需要至少包含一处 edit。".to_owned())
    } else {
        Ok(edits)
    }
}

/** 顺序应用多处唯一替换；任一处定位失败都会拒绝整次 diff。 */
pub(crate) fn apply_multi_text_edits(
    content: &str,
    edits: &[ProposedTextEdit],
) -> Result<String, String> {
    let mut next_content = content.to_owned();

    for (index, edit) in edits.iter().enumerate() {
        next_content = if let Some(occurrence) = edit.occurrence {
            replace_occurrence(&next_content, &edit.original, &edit.next, occurrence)
                .map_err(|error| occurrence_rewrite_validation_message(index + 1, error))?
        } else {
            replace_unique(&next_content, &edit.original, &edit.next)
                .map_err(|error| multi_rewrite_validation_message(index + 1, error))?
        };
    }

    Ok(next_content)
}

/** 单处替换定位失败时返回给模型的错误，禁止包含原文片段。 */
pub(crate) fn single_rewrite_validation_message(error: UniqueReplacementError) -> String {
    match error {
        UniqueReplacementError::NotFound => {
            "改写工具的 original 未命中目标笔记，已拒绝生成不可应用 diff。".to_owned()
        }
        UniqueReplacementError::Ambiguous { .. } => {
            "改写工具的 original 在目标笔记中出现多次，已拒绝生成模糊 diff。请提供更长、更唯一的原文片段。"
                .to_owned()
        }
        UniqueReplacementError::EmptyOriginal => "改写工具缺少 original 内容。".to_owned(),
    }
}

/** 多处替换定位失败时带上序号，方便模型重试但不回显正文。 */
pub(crate) fn multi_rewrite_validation_message(
    index: usize,
    error: UniqueReplacementError,
) -> String {
    match error {
        UniqueReplacementError::NotFound => {
            format!("多处编辑第 {index} 处 original 未命中目标笔记，已拒绝生成 diff。")
        }
        UniqueReplacementError::Ambiguous { .. } => {
            format!(
                "多处编辑第 {index} 处 original 在目标笔记中出现多次，请提供更长、更唯一的片段。"
            )
        }
        UniqueReplacementError::EmptyOriginal => format!("多处编辑第 {index} 处缺少 original。"),
    }
}

/** occurrence 定位失败时返回可操作提示，不回显目标正文。 */
pub(crate) fn occurrence_rewrite_validation_message(
    index: usize,
    error: OccurrenceReplacementError,
) -> String {
    match error {
        OccurrenceReplacementError::OccurrenceOutOfRange { requested, count } => format!(
            "多处编辑第 {index} 处指定第 {requested} 次命中，但当前只命中 {count} 次，已拒绝生成 diff。"
        ),
        OccurrenceReplacementError::EmptyOriginal => format!("多处编辑第 {index} 处缺少 original。"),
    }
}

/** 校验原文片段是否能唯一定位到一处待改写内容。 */
pub(crate) fn validate_unique_original(
    content: &str,
    original: &str,
) -> Result<(), UniqueReplacementError> {
    if original.is_empty() {
        return Err(UniqueReplacementError::EmptyOriginal);
    }

    match count_non_overlapping_matches(content, original) {
        0 => Err(UniqueReplacementError::NotFound),
        1 => Ok(()),
        count => Err(UniqueReplacementError::Ambiguous { count }),
    }
}

/** 判断模型是否把整篇改后文档误塞进局部替换 next，避免确认后出现正文重复。 */
pub(crate) fn looks_like_full_document_replacement_mismatch(
    content: &str,
    original: &str,
    next: &str,
) -> bool {
    let content_trimmed = content.trim();
    let original_trimmed = original.trim();
    let next_trimmed = next.trim();

    if content_trimmed.is_empty() || original_trimmed.is_empty() || next_trimmed.is_empty() {
        return false;
    }

    if original_trimmed == content_trimmed {
        return false;
    }

    next_trimmed.starts_with(content_trimmed)
}

/** 将增量内容追加到笔记末尾，统一保留一个空行作为 Markdown 分隔。 */
pub(crate) fn append_note_content(content: &str, addition: &str) -> String {
    let trimmed_addition = addition.trim();

    if content.trim().is_empty() {
        return trimmed_addition.to_owned();
    }

    format!("{}\n\n{}", content.trim_end(), trimmed_addition)
}

/** 执行 create_file_draft，只创建待确认新建 Markdown/TXT diff。 */
pub(crate) fn execute_create_file_draft(
    snapshot: &mut WorkspaceSnapshot,
    session_index: usize,
    request: &AgentTurnRequest,
    args: &Value,
) -> ToolExecutionResult {
    let scope_ids = scope_id_set(&snapshot.sessions[session_index]);
    let requested_knowledge_base_id = args
        .get("knowledgeBaseId")
        .or_else(|| args.get("knowledge_base_id"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let knowledge_base_id = if let Some(requested_knowledge_base_id) = requested_knowledge_base_id {
        if !scope_ids.contains(requested_knowledge_base_id.as_str()) {
            return ToolExecutionResult::failed(
                "目标知识库不在当前会话允许范围内，已拒绝创建草稿。",
            );
        }

        requested_knowledge_base_id
    } else if scope_ids.contains(request.active_knowledge_base_id.as_str()) {
        request.active_knowledge_base_id.clone()
    } else {
        snapshot.sessions[session_index]
            .knowledge_base_ids
            .first()
            .cloned()
            .unwrap_or_default()
    };
    let target_path = args
        .get("targetPath")
        .or_else(|| args.get("target_path"))
        .and_then(Value::as_str)
        .unwrap_or("00-Inbox/Agent 草稿.md")
        .trim()
        .to_owned();
    if let Some(message) = pending_write_conflict(
        &snapshot.sessions[session_index],
        None,
        Some(target_path.as_str()),
    ) {
        return ToolExecutionResult::failed(&message);
    }
    let file_type = args
        .get("fileType")
        .or_else(|| args.get("file_type"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();

    if knowledge_base_id.is_empty()
        || content.is_empty()
        || !matches!(file_type, "markdown" | "txt")
    {
        return ToolExecutionResult::failed(
            "新建草稿工具缺少目标知识库、正文或有效 fileType（markdown/txt）。",
        );
    }

    let valid_extension = match file_type {
        "markdown" => matches!(
            std::path::Path::new(&target_path)
                .extension()
                .and_then(|value| value.to_str()),
            Some("md") | Some("markdown")
        ),
        "txt" => {
            std::path::Path::new(&target_path)
                .extension()
                .and_then(|value| value.to_str())
                == Some("txt")
        }
        _ => false,
    };
    if !valid_extension {
        return ToolExecutionResult::failed("目标路径扩展名必须与 fileType 匹配。");
    }

    if !snapshot
        .knowledge_bases
        .iter()
        .any(|knowledge_base| knowledge_base.id == knowledge_base_id)
    {
        return ToolExecutionResult::failed("目标知识库不存在，已拒绝创建草稿。");
    }

    let change = ProposedChange {
        id: create_id("change"),
        knowledge_base_id,
        note_id: None,
        target_id: None,
        target_kind: Some(if file_type == "markdown" {
            "note".to_owned()
        } else {
            "document".to_owned()
        }),
        file_type: Some(file_type.to_owned()),
        r#type: "create".to_owned(),
        operation: None,
        title: args
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| "创建 Agent 草稿".to_owned()),
        target_path,
        original: String::new(),
        next: content,
        original_hash: hash_content(""),
        status: "pending".to_owned(),
        review_comments: None,
        review_state: None,
        diff_stats: None,
    };

    snapshot.sessions[session_index].pending_change = Some(change.clone());
    let audit_fragment = Some(format!(
        "create_file_draft type={} path={} chars={}",
        file_type,
        change.target_path,
        change.next.chars().count()
    ));

    ToolExecutionResult {
        success: true,
        summary: format!("已生成 {} 的待确认新建 diff", change.target_path),
        payload: json!({ "change": &change }),
        citations: Vec::new(),
        audit_fragment,
    }
}

/** 执行 create_folder：生成待确认的 create_folder 变更集操作，不直接落盘。 */
pub(crate) fn execute_create_folder(
    context: &mut AgentToolContext<'_>,
    args: &Value,
) -> ToolExecutionResult {
    let session = &context.snapshot.sessions[context.session_index];
    if session.im_identity.is_some()
        || !AgentSecurityLevel::parse(&session.security_level).allows_general_fs_tools()
    {
        return ToolExecutionResult::failed("当前会话安全级别不允许创建文件夹。");
    }
    // Skill 变更集必须先处理完，避免把 Agent 文件夹操作混入 Skill 隔离区差异比对结果。
    if let Some(existing) = session.pending_change_set.as_ref() {
        if existing.status == "pending" && existing.execution_id != AGENT_DIRECT_EXECUTION_ID {
            return ToolExecutionResult::failed(
                "当前已有待确认的 Skill 变更集，请先处理后再创建文件夹。",
            );
        }
    }

    let requested_knowledge_base_id = args
        .get("knowledgeBaseId")
        .or_else(|| args.get("knowledge_base_id"))
        .and_then(Value::as_str);
    let raw_target_path = args
        .get("targetPath")
        .or_else(|| args.get("target_path"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let resolved = match crate::fs_guard::resolve_agent_fs_target(
        &context.snapshot.knowledge_bases,
        &session.knowledge_base_ids,
        &context.request.active_knowledge_base_id,
        &session.security_level,
        requested_knowledge_base_id,
        raw_target_path,
        false,
    ) {
        Ok(resolved) => resolved,
        Err(message) => return ToolExecutionResult::failed(&message),
    };
    if resolved.stored_path.is_empty() {
        return ToolExecutionResult::failed("目标文件夹路径不能为空。");
    }

    let operation = ProposedFileOperation {
        id: create_id("op"),
        knowledge_base_id: resolved.knowledge_base_id.clone(),
        operation: "create_folder".to_owned(),
        source_path: None,
        target_path: resolved.stored_path.clone(),
        file_type: "folder".to_owned(),
        original_hash: String::new(),
        original: None,
        next: None,
        selected: true,
        binary: false,
        byte_size: 0,
        staged_path: None,
    };

    let summary = format!(
        "在 {} 新建文件夹 {}",
        resolved.knowledge_base_id, resolved.stored_path
    );
    let session = &mut context.snapshot.sessions[context.session_index];
    let mut change_set = session
        .pending_change_set
        .clone()
        .unwrap_or_else(|| ProposedChangeSet {
            id: create_id("change-set"),
            execution_id: AGENT_DIRECT_EXECUTION_ID.to_owned(),
            skill_id: AGENT_DIRECT_SOURCE.to_owned(),
            status: "pending".to_owned(),
            summary: String::new(),
            operations: Vec::new(),
            warnings: Vec::new(),
            created_at: format_local_datetime(),
        });
    change_set.operations.push(operation.clone());
    change_set.summary = summary.clone();
    change_set.status = "pending".to_owned();
    change_set.created_at = format_local_datetime();
    session.pending_change_set = Some(change_set.clone());
    session.updated_at = format_local_datetime();

    ToolExecutionResult {
        success: true,
        summary: format!("已生成待确认的文件夹变更：{}", resolved.stored_path),
        payload: json!({ "changeSet": &change_set, "operation": &operation }),
        citations: Vec::new(),
        audit_fragment: Some(format!(
            "create_folder knowledge_base_id={} target_path={}",
            resolved.knowledge_base_id, resolved.stored_path
        )),
    }
}

/** 执行 list_path：列出完全级别下合规目录的一层内容，不跟随符号链接。 */
pub(crate) fn execute_list_path(
    context: &mut AgentToolContext<'_>,
    args: &Value,
) -> ToolExecutionResult {
    let session = &context.snapshot.sessions[context.session_index];
    if session.im_identity.is_some()
        || !AgentSecurityLevel::parse(&session.security_level).allows_external_filesystem()
    {
        return ToolExecutionResult::failed("当前会话安全级别不允许列出知识库外路径。");
    }

    let requested_knowledge_base_id = args
        .get("knowledgeBaseId")
        .or_else(|| args.get("knowledge_base_id"))
        .and_then(Value::as_str);
    let raw_path = args.get("path").and_then(Value::as_str).unwrap_or_default();
    let resolved = match crate::fs_guard::resolve_agent_fs_target(
        &context.snapshot.knowledge_bases,
        &session.knowledge_base_ids,
        &context.request.active_knowledge_base_id,
        &session.security_level,
        requested_knowledge_base_id,
        raw_path,
        true,
    ) {
        Ok(resolved) => resolved,
        Err(message) => return ToolExecutionResult::failed(&message),
    };
    let metadata = match std::fs::symlink_metadata(&resolved.absolute_path) {
        Ok(metadata) => metadata,
        Err(error) => return ToolExecutionResult::failed(&format!("无法读取目标路径：{error}")),
    };
    if metadata.file_type().is_symlink() {
        return ToolExecutionResult::failed("目标路径是符号链接，已拒绝访问。");
    }
    if !metadata.is_dir() {
        return ToolExecutionResult::failed("目标路径不是目录。");
    }

    let mut entries = Vec::new();
    let read_dir = match std::fs::read_dir(&resolved.absolute_path) {
        Ok(read_dir) => read_dir,
        Err(error) => return ToolExecutionResult::failed(&format!("无法列出目录：{error}")),
    };
    for entry in read_dir {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if file_type.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let size = if file_type.is_file() {
            entry.metadata().map(|metadata| metadata.len()).unwrap_or(0)
        } else {
            0
        };
        entries.push(json!({
            "name": name,
            "kind": if file_type.is_dir() { "folder" } else { "file" },
            "size": size
        }));
    }
    entries.sort_by(|left, right| {
        left.get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                right
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
    });
    let total = entries.len();
    let item_limit = parse_limit_arg(args, MAX_LIST_PATH_ENTRIES, MAX_LIST_PATH_ENTRIES);
    let truncated = total > item_limit;
    entries.truncate(item_limit);
    let hint = truncation_hint("list", truncated, None);

    ToolExecutionResult {
        success: true,
        summary: format!(
            "已列出 {}，共 {} 项{}",
            resolved.stored_path,
            total,
            if truncated { "（已截断）" } else { "" }
        ),
        payload: json!({
            "path": resolved.stored_path,
            "knowledgeBaseId": resolved.knowledge_base_id,
            "external": resolved.is_external(),
            "entries": entries,
            "total": total,
            "truncated": truncated,
            "limit": item_limit,
            "hint": hint
        }),
        citations: Vec::new(),
        audit_fragment: Some(format!(
            "list_path knowledge_base_id={} path={} total={}",
            resolved.knowledge_base_id, resolved.stored_path, total
        )),
    }
}

/** 执行 read_path：读取完全级别下合规 UTF-8 文本文件，二进制和超大文件会被拒绝或截断。 */
pub(crate) fn execute_read_path(
    context: &mut AgentToolContext<'_>,
    args: &Value,
) -> ToolExecutionResult {
    let session = &context.snapshot.sessions[context.session_index];
    if session.im_identity.is_some()
        || !AgentSecurityLevel::parse(&session.security_level).allows_external_filesystem()
    {
        return ToolExecutionResult::failed("当前会话安全级别不允许读取知识库外路径。");
    }

    let requested_knowledge_base_id = args
        .get("knowledgeBaseId")
        .or_else(|| args.get("knowledge_base_id"))
        .and_then(Value::as_str);
    let raw_path = args.get("path").and_then(Value::as_str).unwrap_or_default();
    let resolved = match crate::fs_guard::resolve_agent_fs_target(
        &context.snapshot.knowledge_bases,
        &session.knowledge_base_ids,
        &context.request.active_knowledge_base_id,
        &session.security_level,
        requested_knowledge_base_id,
        raw_path,
        true,
    ) {
        Ok(resolved) => resolved,
        Err(message) => return ToolExecutionResult::failed(&message),
    };
    let metadata = match std::fs::symlink_metadata(&resolved.absolute_path) {
        Ok(metadata) => metadata,
        Err(error) => return ToolExecutionResult::failed(&format!("无法读取目标文件：{error}")),
    };
    if metadata.file_type().is_symlink() {
        return ToolExecutionResult::failed("目标路径是符号链接，已拒绝读取。");
    }
    if !metadata.is_file() {
        return ToolExecutionResult::failed("目标路径不是文件。");
    }

    let bytes = match std::fs::read(&resolved.absolute_path) {
        Ok(bytes) => bytes,
        Err(error) => return ToolExecutionResult::failed(&format!("无法读取目标文件：{error}")),
    };
    let truncated_bytes = bytes.len() > MAX_READ_PATH_BYTES;
    let limited = if truncated_bytes {
        bytes.get(..MAX_READ_PATH_BYTES).unwrap_or(&bytes)
    } else {
        bytes.as_slice()
    };
    let Ok(text) = std::str::from_utf8(limited) else {
        return ToolExecutionResult::failed("目标文件不是有效 UTF-8 文本，已拒绝读取。");
    };
    let (offset, limit) = read_window(args);
    let (content, window_truncated, next_offset) = slice_chars(text, offset, limit);
    let truncated = truncated_bytes || window_truncated;
    let hint = truncation_hint("read", truncated, next_offset);

    ToolExecutionResult {
        success: true,
        summary: format!(
            "已读取 {}，{} 字符{}",
            resolved.stored_path,
            content.chars().count(),
            if truncated {
                "（已截断，可用 offset 续读）"
            } else {
                ""
            }
        ),
        payload: json!({
            "path": resolved.stored_path,
            "knowledgeBaseId": resolved.knowledge_base_id,
            "external": resolved.is_external(),
            "content": content,
            "truncated": truncated,
            "nextOffset": next_offset,
            "hint": hint,
            "offset": offset,
            "limit": limit
        }),
        citations: Vec::new(),
        audit_fragment: Some(format!(
            "read_path knowledge_base_id={} path={} truncated={}",
            resolved.knowledge_base_id, resolved.stored_path, truncated
        )),
    }
}

/** 执行 organize 建议工具，该工具首版不写入文件。 */
#[allow(dead_code)]
pub(crate) fn execute_suggest_organization(args: &Value) -> ToolExecutionResult {
    let suggestion = args
        .get("suggestion")
        .and_then(Value::as_str)
        .unwrap_or("建议补充稳定标签、标题层级和相关链接。");

    ToolExecutionResult {
        success: true,
        summary: "已生成整理建议；该工具不会直接写入文件".to_owned(),
        payload: json!({ "suggestion": suggestion }),
        citations: Vec::new(),
        audit_fragment: Some("suggest_organization 未发送笔记正文".to_owned()),
    }
}

/** 获取会话 scope 内的笔记。 */
pub(crate) fn scoped_note<'a>(
    snapshot: &'a WorkspaceSnapshot,
    session_index: usize,
    note_id: &str,
) -> Option<&'a crate::domain::Note> {
    let scope_ids = scope_id_set(&snapshot.sessions[session_index]);

    snapshot
        .notes
        .iter()
        .find(|note| note.id == note_id && scope_ids.contains(note.knowledge_base_id.as_str()))
}

/** 返回会话授权范围内可被 Agent 读取和改写的 TXT；其它普通文档始终拒绝。 */
pub(crate) fn scoped_text_document<'a>(
    snapshot: &'a WorkspaceSnapshot,
    session_index: usize,
    file_id: &str,
) -> Option<&'a WorkspaceDocument> {
    let scope_ids = scope_id_set(&snapshot.sessions[session_index]);

    snapshot.documents.iter().find(|document| {
        document.id == file_id
            && document.file_type == "txt"
            && scope_ids.contains(document.knowledge_base_id.as_str())
    })
}

/** 返回当前 scope 内可由 Agent 按需读取、但绝不可写入的 DOCX/PDF。 */
pub(crate) fn scoped_readonly_document<'a>(
    snapshot: &'a WorkspaceSnapshot,
    session_index: usize,
    document_id: &str,
) -> Option<&'a WorkspaceDocument> {
    let scope_ids = scope_id_set(&snapshot.sessions[session_index]);
    snapshot.documents.iter().find(|document| {
        document.id == document_id
            && matches!(document.file_type.as_str(), "docx" | "pdf")
            && scope_ids.contains(document.knowledge_base_id.as_str())
    })
}

/** 把会话知识库范围转成 HashSet，统一工具权限校验。 */
pub(crate) fn scope_id_set(session: &AgentSession) -> HashSet<&str> {
    session
        .knowledge_base_ids
        .iter()
        .map(String::as_str)
        .collect()
}

/** 按知识库 id 取展示名，缺失时给出明确占位，避免模型把文件归到错误的知识库。 */
pub(crate) fn knowledge_base_display_name(
    snapshot: &WorkspaceSnapshot,
    knowledge_base_id: &str,
) -> String {
    snapshot
        .knowledge_bases
        .iter()
        .find(|knowledge_base| knowledge_base.id == knowledge_base_id)
        .map(|knowledge_base| knowledge_base.name.clone())
        .unwrap_or_else(|| "未知知识库".to_owned())
}

/** 提取首个可改写正文段落。 */
pub(crate) fn first_body_paragraph(content: &str) -> String {
    content
        .lines()
        .map(str::trim)
        .find(|line| line.len() > 18 && !line.starts_with('#') && !line.starts_with('-'))
        .unwrap_or("")
        .to_owned()
}

/** 把字符串裁剪到指定字符预算，保留明确截断标记。 */
pub(crate) fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }

    let truncated = value.chars().take(max_chars).collect::<String>();

    format!("{truncated}\n\n[内容已按上下文预算截断]")
}

/** 裁剪引用片段，避免单条引用把模型上下文撑大。 */
pub(crate) fn budget_citation(mut citation: Citation) -> Citation {
    citation.snippet = truncate_chars(&citation.snippet, 500);
    citation
}
