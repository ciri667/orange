use crate::domain::{
    AgentSession, AgentToolCall, AgentTurnRequest, Citation, ProposedChange, WorkspaceDocument,
    WorkspaceSnapshot,
};
use crate::storage::{create_id, hash_content};
use crate::text_edit::{
    count_non_overlapping_matches, replace_occurrence, replace_unique, OccurrenceReplacementError,
    UniqueReplacementError,
};
use serde_json::{json, Value};
use std::collections::HashSet;
use tauri::AppHandle;

/** 单次 read_note 工具最多发送给模型的正文字符数。 */
pub(crate) const MAX_READ_NOTE_CHARS: usize = 6000;

/** list_tree 工具最多发送的目录、Markdown 和普通文档摘要数量。 */
const MAX_TREE_ITEMS: usize = 120;

/** list_tree 按支持文档类型输出的计数，避免模型把未知扩展名误认为已索引内容。 */
#[derive(Clone, Debug)]
struct ListTreeFileTypeCounts {
    markdown: usize,
    txt: usize,
    docx: usize,
    pdf: usize,
    image: usize,
}

/** Agent 一次多处编辑中的单个片段，original 必须唯一命中，next 允许为空表示删除。 */
#[derive(Clone, Debug)]
struct ProposedTextEdit {
    original: String,
    next: String,
    occurrence: Option<usize>,
}

/** Agent 工具执行时共享的受控上下文，所有工具都必须通过它访问会话 scope 和当前请求。 */
pub struct AgentToolContext<'a> {
    /** Tauri 应用句柄，只有需要 SQLite/FTS 或系统能力的工具才会读取。 */
    pub app: Option<&'a AppHandle>,
    /** 本轮可变工作台快照，写入类工具只能在这里创建 pending diff。 */
    pub snapshot: &'a mut WorkspaceSnapshot,
    /** 当前会话在 snapshot.sessions 中的位置，用于统一 scope 校验。 */
    pub session_index: usize,
    /** 用户本轮请求，提供当前笔记、知识库和 prompt 等 UI 上下文。 */
    pub request: &'a AgentTurnRequest,
}

/** 单个工具执行的标准结果，模型、UI 轨迹和审计日志都从这里派生。 */
pub struct ToolExecutionResult {
    pub success: bool,
    pub summary: String,
    pub payload: Value,
    pub citations: Vec<Citation>,
    pub audit_fragment: Option<String>,
}

impl ToolExecutionResult {
    /** 构造失败工具结果，模型会收到同一份错误摘要。 */
    pub fn failed(message: &str) -> Self {
        Self {
            success: false,
            summary: message.to_owned(),
            payload: json!({ "error": message }),
            citations: Vec::new(),
            audit_fragment: Some(format!("工具失败：{message}")),
        }
    }
}

/** 已执行工具的完整外部形态，包含 UI 轨迹、模型可读 payload、引用和审计片段。 */
pub struct ToolOutcome {
    pub call: AgentToolCall,
    pub payload: Value,
    pub citations: Vec<Citation>,
    pub audit_fragment: Option<String>,
}

/** Agent 内置工具接口，新增工具必须声明 schema 并在 execute 内完成权限校验。 */
pub trait AgentTool: Send + Sync {
    /** 工具名称，必须与模型 tool_call 中的 function.name 保持一致。 */
    fn name(&self) -> &'static str;

    /** 面向模型的工具说明，描述能力边界而不是 UI 行为。 */
    fn description(&self) -> &'static str;

    /** OpenAI-compatible function calling 参数 schema。 */
    fn parameters(&self) -> Value;

    /** 执行工具并返回标准结果，禁止绕过 context 中的 scope 和写入边界。 */
    fn execute(&self, context: &mut AgentToolContext<'_>, args: &Value) -> ToolExecutionResult;
}

/** 内置 Agent 工具注册表，统一负责 schema 输出和按名称分发工具调用。 */
pub struct ToolRegistry {
    /** 已注册工具列表；顺序稳定，便于 UI 和测试比对 schema。 */
    tools: Vec<Box<dyn AgentTool>>,
}

impl Default for ToolRegistry {
    /** 创建默认内置工具集；todo: 外部插件工具接入时复用同一注册入口并增加权限声明。 */
    fn default() -> Self {
        Self {
            tools: vec![
                Box::new(SearchNotesTool),
                Box::new(ReadNoteTool),
                Box::new(ListTreeTool),
                Box::new(GetCurrentNoteTool),
                Box::new(ProposeNoteChangeTool),
                Box::new(CreateNoteDraftTool),
                Box::new(SuggestOrganizationTool),
            ],
        }
    }
}

impl ToolRegistry {
    /** 将当前注册工具转换成 OpenAI-compatible tools schema。 */
    pub fn schemas(&self) -> Value {
        Value::Array(
            self.tools
                .iter()
                .map(|tool| function_tool(tool.name(), tool.description(), tool.parameters()))
                .collect(),
        )
    }

    /** 返回已注册工具名，主要用于测试和诊断工具集是否完整。 */
    #[cfg(test)]
    pub fn tool_names(&self) -> Vec<&'static str> {
        self.tools.iter().map(|tool| tool.name()).collect()
    }

    /** 按名称执行工具，未知工具会被显式拒绝且不会修改工作台快照。 */
    pub fn execute_named(
        &self,
        context: &mut AgentToolContext<'_>,
        name: &str,
        args: Value,
    ) -> ToolOutcome {
        let result = self
            .tools
            .iter()
            .find(|tool| tool.name() == name)
            .map(|tool| tool.execute(context, &args))
            .unwrap_or_else(|| ToolExecutionResult::failed("未知工具，已拒绝执行。"));

        tool_outcome(name, args, result)
    }

    /** 执行模型返回的 tool_call，负责解析 arguments 并复用命名工具分发。 */
    pub fn execute_model_tool_call(
        &self,
        context: &mut AgentToolContext<'_>,
        model_tool_call: &Value,
    ) -> ToolOutcome {
        let name = model_tool_call
            .get("function")
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("unknown_tool");
        let args = parse_tool_args(model_tool_call);

        self.execute_named(context, name, args)
    }
}

/** search_notes 工具，在当前会话授权知识库内执行 SQLite/FTS 检索。 */
struct SearchNotesTool;

impl AgentTool for SearchNotesTool {
    fn name(&self) -> &'static str {
        "search_notes"
    }

    fn description(&self) -> &'static str {
        "Search Markdown notes in the selected session scope."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "required": ["query"]
        })
    }

    fn execute(&self, context: &mut AgentToolContext<'_>, args: &Value) -> ToolExecutionResult {
        execute_search_notes(context, args)
    }
}

/** read_note 工具，读取当前 scope 内的一篇笔记并返回受预算限制的正文。 */
struct ReadNoteTool;

impl AgentTool for ReadNoteTool {
    fn name(&self) -> &'static str {
        "read_note"
    }

    fn description(&self) -> &'static str {
        "Read one note by id if it is inside the selected scope."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "noteId": { "type": "string" } },
            "required": ["noteId"]
        })
    }

    fn execute(&self, context: &mut AgentToolContext<'_>, args: &Value) -> ToolExecutionResult {
        execute_read_note(context.snapshot, context.session_index, args)
    }
}

/** list_tree 工具，列出当前 scope 内目录、Markdown 笔记和支持文档摘要，供模型判断下一步。 */
struct ListTreeTool;

impl AgentTool for ListTreeTool {
    fn name(&self) -> &'static str {
        "list_tree"
    }

    fn description(&self) -> &'static str {
        "List folders, Markdown notes, and supported document metadata inside the selected scope. It does not read non-Markdown document contents."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    fn execute(&self, context: &mut AgentToolContext<'_>, _args: &Value) -> ToolExecutionResult {
        execute_list_tree(context.snapshot, context.session_index)
    }
}

/** get_current_note 工具，读取 UI 当前激活笔记但仍执行 scope 校验。 */
struct GetCurrentNoteTool;

impl AgentTool for GetCurrentNoteTool {
    fn name(&self) -> &'static str {
        "get_current_note"
    }

    fn description(&self) -> &'static str {
        "Read the current active note if it is inside the selected scope."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    fn execute(&self, context: &mut AgentToolContext<'_>, _args: &Value) -> ToolExecutionResult {
        let args = json!({ "noteId": context.request.active_note_id });

        execute_read_note(context.snapshot, context.session_index, &args)
    }
}

/** propose_note_change 工具，只创建待确认 diff，不直接写 Markdown 文件。 */
struct ProposeNoteChangeTool;

impl AgentTool for ProposeNoteChangeTool {
    fn name(&self) -> &'static str {
        "propose_note_change"
    }

    fn description(&self) -> &'static str {
        "Create a pending rewrite diff for an existing note."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "noteId": { "type": "string" },
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
            "required": ["noteId"]
        })
    }

    fn execute(&self, context: &mut AgentToolContext<'_>, args: &Value) -> ToolExecutionResult {
        execute_propose_note_change(context.snapshot, context.session_index, args)
    }
}

/** create_note_draft 工具，只创建待确认新建 diff，不直接落盘。 */
struct CreateNoteDraftTool;

impl AgentTool for CreateNoteDraftTool {
    fn name(&self) -> &'static str {
        "create_note_draft"
    }

    fn description(&self) -> &'static str {
        "Create a pending new Markdown draft diff."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "knowledgeBaseId": { "type": "string" },
                "targetPath": { "type": "string" },
                "title": { "type": "string" },
                "content": { "type": "string" }
            },
            "required": ["targetPath", "content"]
        })
    }

    fn execute(&self, context: &mut AgentToolContext<'_>, args: &Value) -> ToolExecutionResult {
        execute_create_note_draft(
            context.snapshot,
            context.session_index,
            context.request,
            args,
        )
    }
}

/** suggest_organization 工具，只返回整理建议，不创建或移动文件。 */
struct SuggestOrganizationTool;

impl AgentTool for SuggestOrganizationTool {
    fn name(&self) -> &'static str {
        "suggest_organization"
    }

    fn description(&self) -> &'static str {
        "Suggest tags, title, folder or related notes without writing files."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "noteId": { "type": "string" },
                "suggestion": { "type": "string" }
            }
        })
    }

    fn execute(&self, _context: &mut AgentToolContext<'_>, args: &Value) -> ToolExecutionResult {
        execute_suggest_organization(args)
    }
}

/** 构造 OpenAI-compatible function tool 描述。 */
fn function_tool(name: &str, description: &str, parameters: Value) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": parameters
        }
    })
}

/** 解析模型 tool_call 的 arguments JSON 字符串。 */
fn parse_tool_args(model_tool_call: &Value) -> Value {
    model_tool_call
        .get("function")
        .and_then(|function| function.get("arguments"))
        .and_then(|raw_args| {
            if let Some(raw_args) = raw_args.as_str() {
                serde_json::from_str(raw_args).ok()
            } else if raw_args.is_object() {
                Some(raw_args.clone())
            } else {
                None
            }
        })
        .unwrap_or_else(|| json!({}))
}

/** 把标准执行结果转换成前端可展示的工具轨迹。 */
fn tool_outcome(name: &str, args: Value, result: ToolExecutionResult) -> ToolOutcome {
    ToolOutcome {
        call: AgentToolCall {
            id: create_id("tool"),
            name: name.to_owned(),
            status: if result.success {
                "completed".to_owned()
            } else {
                "failed".to_owned()
            },
            summary: result.summary,
            args,
        },
        payload: result.payload,
        citations: result.citations,
        audit_fragment: result.audit_fragment,
    }
}

/** 执行 search_notes，并把引用同步给前端消息展示。 */
fn execute_search_notes(context: &mut AgentToolContext<'_>, args: &Value) -> ToolExecutionResult {
    let Some(app) = context.app else {
        return ToolExecutionResult::failed("当前运行环境无法访问本地检索索引。");
    };
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default();

    match crate::storage::search_notes(
        app,
        context.snapshot,
        &context.snapshot.sessions[context.session_index].knowledge_base_ids,
        query,
    ) {
        Ok(citations) => {
            let bounded_citations: Vec<Citation> =
                citations.into_iter().map(budget_citation).collect();
            let audit_titles = bounded_citations
                .iter()
                .take(4)
                .map(|citation| format!("《{}》", citation.title))
                .collect::<Vec<_>>()
                .join("、");

            ToolExecutionResult {
                success: true,
                summary: format!(
                    "在会话允许范围内检索到 {} 条候选引用",
                    bounded_citations.len()
                ),
                payload: json!({ "citations": &bounded_citations }),
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

/** 执行 read_note，后端校验目标笔记必须属于会话 scope。 */
fn execute_read_note(
    snapshot: &WorkspaceSnapshot,
    session_index: usize,
    args: &Value,
) -> ToolExecutionResult {
    let note_id = args
        .get("noteId")
        .or_else(|| args.get("note_id"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(note) = scoped_note(snapshot, session_index, note_id) else {
        return ToolExecutionResult::failed("目标笔记不在当前会话允许范围内。");
    };
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
    };
    let note_content_chars = note.content.chars().count();
    let bounded_content = truncate_chars(&note.content, MAX_READ_NOTE_CHARS);

    ToolExecutionResult {
        success: true,
        summary: format!("已读取笔记《{}》", note.title),
        payload: json!({
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
                "contentTruncated": note_content_chars > MAX_READ_NOTE_CHARS
            }
        }),
        citations: vec![citation],
        audit_fragment: Some(format!(
            "read_note 发送《{}》{}，{} 字符{}",
            note.title,
            note.path,
            note_content_chars.min(MAX_READ_NOTE_CHARS),
            if note_content_chars > MAX_READ_NOTE_CHARS {
                "（已截断）"
            } else {
                ""
            }
        )),
    }
}

/** 执行 list_tree，只返回当前 scope 内的目录、Markdown 笔记和普通文档元数据。 */
fn execute_list_tree(snapshot: &WorkspaceSnapshot, session_index: usize) -> ToolExecutionResult {
    let scope_ids = scope_id_set(&snapshot.sessions[session_index]);
    let scoped_folders: Vec<_> = snapshot
        .folders
        .iter()
        .filter(|folder| scope_ids.contains(folder.knowledge_base_id.as_str()))
        .collect();
    let scoped_notes: Vec<_> = snapshot
        .notes
        .iter()
        .filter(|note| scope_ids.contains(note.knowledge_base_id.as_str()))
        .collect();
    let scoped_documents: Vec<_> = snapshot
        .documents
        .iter()
        .filter(|document| scope_ids.contains(document.knowledge_base_id.as_str()))
        .collect();
    let file_type_counts = build_list_tree_file_type_counts(scoped_notes.len(), &scoped_documents);
    let total_files = scoped_notes.len() + scoped_documents.len();
    let folders: Vec<_> = scoped_folders
        .iter()
        .take(MAX_TREE_ITEMS)
        .map(|folder| json!({ "id": folder.id, "name": folder.name, "path": folder.path, "knowledgeBaseId": folder.knowledge_base_id }))
        .collect();
    let notes: Vec<_> = scoped_notes
        .iter()
        .take(MAX_TREE_ITEMS)
        .map(|note| json!({ "id": note.id, "title": note.title, "path": note.path, "knowledgeBaseId": note.knowledge_base_id }))
        .collect();
    let documents: Vec<_> = scoped_documents
        .iter()
        .take(MAX_TREE_ITEMS)
        .map(|document| {
            json!({
                "id": &document.id,
                "title": &document.title,
                "path": &document.path,
                "knowledgeBaseId": &document.knowledge_base_id,
                "fileType": &document.file_type,
                "previewAvailable": document.preview_available,
                "agentReadable": false
            })
        })
        .collect();
    let truncated = scoped_folders.len() > MAX_TREE_ITEMS
        || scoped_notes.len() > MAX_TREE_ITEMS
        || scoped_documents.len() > MAX_TREE_ITEMS;

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
            "folders": folders,
            "notes": notes,
            "documents": documents,
            "totalFolders": scoped_folders.len(),
            "totalNotes": scoped_notes.len(),
            "totalDocuments": scoped_documents.len(),
            "totalFiles": total_files,
            "fileTypeCounts": file_type_counts.to_json(),
            "truncated": truncated
        }),
        citations: Vec::new(),
        audit_fragment: Some(format!(
            "list_tree 发送 {} 个目录摘要、{} 篇 Markdown 摘要、{} 个普通文档摘要{}",
            scoped_folders.len().min(MAX_TREE_ITEMS),
            scoped_notes.len().min(MAX_TREE_ITEMS),
            scoped_documents.len().min(MAX_TREE_ITEMS),
            if truncated { "（已截断）" } else { "" }
        )),
    }
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
fn build_list_tree_file_type_counts(
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

/** 执行 propose_note_change，只创建待确认 diff，不直接写文件。 */
fn execute_propose_note_change(
    snapshot: &mut WorkspaceSnapshot,
    session_index: usize,
    args: &Value,
) -> ToolExecutionResult {
    let note_id = args
        .get("noteId")
        .or_else(|| args.get("note_id"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(note) = scoped_note(snapshot, session_index, note_id).cloned() else {
        return ToolExecutionResult::failed("目标笔记不在当前会话允许范围内。");
    };
    let operation = args
        .get("operation")
        .or_else(|| args.get("mode"))
        .and_then(Value::as_str)
        .filter(|value| matches!(*value, "append" | "replace" | "multi_replace"))
        .unwrap_or("replace");
    let (original, next) = match prepare_rewrite_content(&note.content, operation, args) {
        Ok(prepared_change) => prepared_change,
        Err(message) => return ToolExecutionResult::failed(&message),
    };

    let change = ProposedChange {
        id: create_id("change"),
        knowledge_base_id: note.knowledge_base_id.clone(),
        note_id: Some(note.id.clone()),
        r#type: "rewrite".to_owned(),
        operation: Some(operation.to_owned()),
        title: args
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("改写《{}》", note.title)),
        target_path: note.path.clone(),
        original,
        next,
        original_hash: note.content_hash.clone(),
        status: "pending".to_owned(),
        review_comments: None,
        review_state: None,
        diff_stats: None,
    };

    snapshot.sessions[session_index].pending_change = Some(change.clone());
    let audit_fragment = Some(format!(
        "propose_note_change 为《{}》生成 {} diff，原文 {} 字符，建议 {} 字符",
        note.title,
        operation,
        change.original.chars().count(),
        change.next.chars().count()
    ));

    ToolExecutionResult {
        success: true,
        summary: format!("已为《{}》生成待确认改写 diff", note.title),
        payload: json!({ "change": &change }),
        citations: Vec::new(),
        audit_fragment,
    }
}

/** 根据 operation 准备待审阅 diff 的原文和建议内容，不在日志或错误里回显正文。 */
fn prepare_rewrite_content(
    content: &str,
    operation: &str,
    args: &Value,
) -> Result<(String, String), String> {
    match operation {
        "append" => prepare_append_rewrite(content, args),
        "multi_replace" => prepare_multi_replace_rewrite(content, args),
        _ => prepare_single_replace_rewrite(content, args),
    }
}

/** 准备单处替换，original 必须唯一命中，next 可以为空以支持删除。 */
fn prepare_single_replace_rewrite(content: &str, args: &Value) -> Result<(String, String), String> {
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
fn prepare_append_rewrite(content: &str, args: &Value) -> Result<(String, String), String> {
    let addition = args
        .get("next")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();

    if addition.is_empty() {
        return Err("文末追加工具缺少增量内容。".to_owned());
    }

    Ok((content.to_owned(), append_note_content(content, &addition)))
}

/** 准备同一文件内多处替换，先按唯一片段顺序应用到内存，再生成整篇待确认 diff。 */
fn prepare_multi_replace_rewrite(content: &str, args: &Value) -> Result<(String, String), String> {
    let edits = parse_text_edits(args)?;
    let next = apply_multi_text_edits(content, &edits)?;

    if next == content {
        return Err("多处编辑没有产生内容变化，已拒绝生成空 diff。".to_owned());
    }

    Ok((content.to_owned(), next))
}

/** 从工具参数读取 edits/replacements，正文只保存在 pending diff，不进入日志。 */
fn parse_text_edits(args: &Value) -> Result<Vec<ProposedTextEdit>, String> {
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
fn apply_multi_text_edits(content: &str, edits: &[ProposedTextEdit]) -> Result<String, String> {
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
fn single_rewrite_validation_message(error: UniqueReplacementError) -> String {
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
fn multi_rewrite_validation_message(index: usize, error: UniqueReplacementError) -> String {
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
fn occurrence_rewrite_validation_message(
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
fn validate_unique_original(content: &str, original: &str) -> Result<(), UniqueReplacementError> {
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
fn looks_like_full_document_replacement_mismatch(
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
fn append_note_content(content: &str, addition: &str) -> String {
    let trimmed_addition = addition.trim();

    if content.trim().is_empty() {
        return trimmed_addition.to_owned();
    }

    format!("{}\n\n{}", content.trim_end(), trimmed_addition)
}

/** 执行 create_note_draft，只创建待确认新建 diff。 */
fn execute_create_note_draft(
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
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();

    if knowledge_base_id.is_empty() || content.is_empty() {
        return ToolExecutionResult::failed("新建草稿工具缺少目标知识库或正文内容。");
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
        "create_note_draft 生成 {}，正文 {} 字符",
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

/** 执行 organize 建议工具，该工具首版不写入文件。 */
fn execute_suggest_organization(args: &Value) -> ToolExecutionResult {
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
fn scoped_note<'a>(
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

/** 把会话知识库范围转成 HashSet，统一工具权限校验。 */
fn scope_id_set(session: &AgentSession) -> HashSet<&str> {
    session
        .knowledge_base_ids
        .iter()
        .map(String::as_str)
        .collect()
}

/** 提取首个可改写正文段落。 */
fn first_body_paragraph(content: &str) -> String {
    content
        .lines()
        .map(str::trim)
        .find(|line| line.len() > 18 && !line.starts_with('#') && !line.starts_with('-'))
        .unwrap_or("")
        .to_owned()
}

/** 把字符串裁剪到指定字符预算，保留明确截断标记。 */
fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }

    let truncated = value.chars().take(max_chars).collect::<String>();

    format!("{truncated}\n\n[内容已按上下文预算截断]")
}

/** 裁剪引用片段，避免单条引用把模型上下文撑大。 */
fn budget_citation(mut citation: Citation) -> Citation {
    citation.snippet = truncate_chars(&citation.snippet, 500);
    citation
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{FolderEntry, KnowledgeBase, Note, WorkspaceDocument};

    /** 构造工具层测试使用的最小工作台快照。 */
    fn tool_test_snapshot(note_content: String) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            knowledge_bases: vec![
                KnowledgeBase {
                    id: "kb-a".to_owned(),
                    name: "主知识库".to_owned(),
                    path: "/tmp/kb-a".to_owned(),
                    description: "测试知识库".to_owned(),
                    status: "ready".to_owned(),
                    note_count: 1,
                    document_count: 0,
                    updated_at: "刚刚".to_owned(),
                    is_default: true,
                    semantic_index_enabled: false,
                    scan_report: None,
                },
                KnowledgeBase {
                    id: "kb-b".to_owned(),
                    name: "未授权知识库".to_owned(),
                    path: "/tmp/kb-b".to_owned(),
                    description: "测试知识库".to_owned(),
                    status: "ready".to_owned(),
                    note_count: 1,
                    document_count: 0,
                    updated_at: "刚刚".to_owned(),
                    is_default: false,
                    semantic_index_enabled: false,
                    scan_report: None,
                },
            ],
            folders: vec![FolderEntry {
                id: "folder-a".to_owned(),
                knowledge_base_id: "kb-a".to_owned(),
                name: "Notes".to_owned(),
                path: "Notes".to_owned(),
                updated_at: "刚刚".to_owned(),
            }],
            notes: vec![
                Note {
                    id: "note-a".to_owned(),
                    knowledge_base_id: "kb-a".to_owned(),
                    title: "授权笔记".to_owned(),
                    path: "Notes/授权笔记.md".to_owned(),
                    content_hash: hash_content(&note_content),
                    content: note_content,
                    tags: vec!["测试".to_owned()],
                    updated_at: "刚刚".to_owned(),
                    backlinks: Vec::new(),
                },
                Note {
                    id: "note-b".to_owned(),
                    knowledge_base_id: "kb-b".to_owned(),
                    title: "未授权笔记".to_owned(),
                    path: "Private/未授权笔记.md".to_owned(),
                    content_hash: hash_content("private"),
                    content: "private".to_owned(),
                    tags: Vec::new(),
                    updated_at: "刚刚".to_owned(),
                    backlinks: Vec::new(),
                },
            ],
            documents: Vec::new(),
            sessions: vec![AgentSession {
                id: "session-a".to_owned(),
                title: "测试会话".to_owned(),
                r#type: "knowledge-base".to_owned(),
                knowledge_base_ids: vec!["kb-a".to_owned()],
                active_note_id: Some("note-a".to_owned()),
                pinned_note_ids: vec!["note-a".to_owned()],
                messages: Vec::new(),
                pending_change: None,
                created_at: "刚刚".to_owned(),
                updated_at: "刚刚".to_owned(),
                deleted_at: None,
                model_provider_id: None,
                model_id: None,
            }],
            active_knowledge_base_id: "kb-a".to_owned(),
            active_note_id: "note-a".to_owned(),
            active_document_id: String::new(),
            active_session_id: "session-a".to_owned(),
        }
    }

    /** 构造工具层测试使用的 Agent 请求。 */
    fn tool_test_request(action: &str, prompt: &str) -> AgentTurnRequest {
        AgentTurnRequest {
            prompt: prompt.to_owned(),
            action: action.to_owned(),
            session_id: "session-a".to_owned(),
            active_knowledge_base_id: "kb-a".to_owned(),
            active_note_id: "note-a".to_owned(),
            client_message_id: None,
            model_provider_id: None,
            model_id: None,
            explicit_skill_ids: Vec::new(),
        }
    }

    /** 创建无 AppHandle 的纯内存工具上下文，适合测试非索引类工具。 */
    fn tool_test_context<'a>(
        snapshot: &'a mut WorkspaceSnapshot,
        request: &'a AgentTurnRequest,
    ) -> AgentToolContext<'a> {
        AgentToolContext {
            app: None,
            snapshot,
            session_index: 0,
            request,
        }
    }

    /** 构造工具层普通文档条目，测试 list_tree 元数据时不需要真实文件系统。 */
    fn tool_test_document(
        id: &str,
        knowledge_base_id: &str,
        path: &str,
        file_type: &str,
        preview_available: bool,
    ) -> WorkspaceDocument {
        WorkspaceDocument {
            id: id.to_owned(),
            knowledge_base_id: knowledge_base_id.to_owned(),
            title: path
                .rsplit('/')
                .next()
                .unwrap_or("测试文档")
                .trim_end_matches(&format!(".{file_type}"))
                .to_owned(),
            path: path.to_owned(),
            file_type: file_type.to_owned(),
            updated_at: "刚刚".to_owned(),
            content_hash: hash_content(id),
            content: (file_type == "txt").then(|| "纯文本正文不会通过 list_tree 返回。".to_owned()),
            preview_available,
        }
    }

    /** 默认 registry 必须暴露现有内置工具的 schema。 */
    #[test]
    fn registry_schema_contains_builtin_tools() {
        let registry = ToolRegistry::default();
        let schemas = registry.schemas();
        let tool_names = registry.tool_names();

        assert!(schemas.is_array());
        assert!(tool_names.contains(&"search_notes"));
        assert!(tool_names.contains(&"read_note"));
        assert!(tool_names.contains(&"propose_note_change"));
        assert!(tool_names.contains(&"create_note_draft"));
    }

    /** 未知工具调用必须失败且不能修改 pending_change。 */
    #[test]
    fn unknown_tool_is_rejected_without_pending_change() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = tool_test_request("ask", "测试未知工具");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(&mut context, "unknown_tool", json!({}));

        assert_eq!(outcome.call.status, "failed");
        assert!(context.snapshot.sessions[0].pending_change.is_none());
    }

    /** read_note 必须拒绝读取当前会话 scope 外的笔记。 */
    #[test]
    fn read_note_rejects_note_outside_scope() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = tool_test_request("ask", "读取笔记");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome =
            registry.execute_named(&mut context, "read_note", json!({ "noteId": "note-b" }));

        assert_eq!(outcome.call.status, "failed");
        assert!(outcome.payload.get("error").is_some());
    }

    /** list_tree 应返回当前 scope 内普通文档元数据，但不暴露正文和 hash。 */
    #[test]
    fn list_tree_returns_document_metadata_for_scope() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        snapshot.documents = vec![
            tool_test_document("document-txt", "kb-a", "Docs/brief.txt", "txt", false),
            tool_test_document("document-pdf", "kb-a", "Docs/spec.pdf", "pdf", true),
        ];
        let request = tool_test_request("ask", "列出文件");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(&mut context, "list_tree", json!({}));
        let documents = outcome.payload["documents"].as_array().unwrap();
        let txt_document = documents
            .iter()
            .find(|document| document["id"].as_str() == Some("document-txt"))
            .unwrap();

        assert_eq!(outcome.call.status, "completed");
        assert_eq!(documents.len(), 2);
        assert_eq!(txt_document["fileType"].as_str(), Some("txt"));
        assert_eq!(txt_document["previewAvailable"].as_bool(), Some(false));
        assert_eq!(txt_document["agentReadable"].as_bool(), Some(false));
        assert!(txt_document.get("content").is_none());
        assert!(txt_document.get("contentHash").is_none());
    }

    /** list_tree 必须按会话 scope 过滤普通文档，避免暴露未授权知识库结构。 */
    #[test]
    fn list_tree_rejects_documents_outside_scope() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        snapshot.documents = vec![
            tool_test_document("document-a", "kb-a", "Docs/allowed.txt", "txt", false),
            tool_test_document("document-b", "kb-b", "Private/hidden.pdf", "pdf", true),
        ];
        let request = tool_test_request("ask", "列出文件");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(&mut context, "list_tree", json!({}));
        let documents = outcome.payload["documents"].as_array().unwrap();

        assert_eq!(outcome.call.status, "completed");
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0]["id"].as_str(), Some("document-a"));
        assert_eq!(outcome.payload["totalDocuments"].as_u64(), Some(1));
    }

    /** list_tree 应汇总混合文件总数、类型计数和截断状态。 */
    #[test]
    fn list_tree_reports_totals_type_counts_and_truncation() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());

        snapshot.documents = vec![
            tool_test_document("document-txt-base", "kb-a", "Docs/base.txt", "txt", false),
            tool_test_document("document-docx", "kb-a", "Docs/brief.docx", "docx", true),
            tool_test_document("document-pdf", "kb-a", "Docs/spec.pdf", "pdf", true),
            tool_test_document(
                "document-image",
                "kb-a",
                "Assets/diagram.png",
                "image",
                true,
            ),
        ];

        for index in 0..(MAX_TREE_ITEMS - 3) {
            // 生成超过 list_tree 单类预算的 TXT 文档，用于验证 totals 保留真实数量而数组被截断。
            snapshot.documents.push(tool_test_document(
                &format!("document-extra-{index}"),
                "kb-a",
                &format!("Docs/extra-{index}.txt"),
                "txt",
                false,
            ));
        }

        let request = tool_test_request("ask", "列出文件");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(&mut context, "list_tree", json!({}));
        let documents = outcome.payload["documents"].as_array().unwrap();
        let file_type_counts = &outcome.payload["fileTypeCounts"];

        assert_eq!(outcome.call.status, "completed");
        assert_eq!(documents.len(), MAX_TREE_ITEMS);
        assert_eq!(outcome.payload["totalNotes"].as_u64(), Some(1));
        assert_eq!(
            outcome.payload["totalDocuments"].as_u64(),
            Some((MAX_TREE_ITEMS + 1) as u64)
        );
        assert_eq!(
            outcome.payload["totalFiles"].as_u64(),
            Some((MAX_TREE_ITEMS + 2) as u64)
        );
        assert_eq!(outcome.payload["truncated"].as_bool(), Some(true));
        assert_eq!(file_type_counts["markdown"].as_u64(), Some(1));
        assert_eq!(
            file_type_counts["txt"].as_u64(),
            Some((MAX_TREE_ITEMS - 2) as u64)
        );
        assert_eq!(file_type_counts["docx"].as_u64(), Some(1));
        assert_eq!(file_type_counts["pdf"].as_u64(), Some(1));
        assert_eq!(file_type_counts["image"].as_u64(), Some(1));
    }

    /** read_note 会按上下文预算截断长正文并保留截断标记。 */
    #[test]
    fn read_note_truncates_large_content_for_model_context() {
        let registry = ToolRegistry::default();
        let long_content = "段落内容。".repeat(MAX_READ_NOTE_CHARS);
        let mut snapshot = tool_test_snapshot(long_content);
        let request = tool_test_request("ask", "读取长文");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome =
            registry.execute_named(&mut context, "read_note", json!({ "noteId": "note-a" }));
        let content = outcome.payload["note"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_owned();

        assert_eq!(outcome.call.status, "completed");
        assert_eq!(outcome.payload["note"]["contentTruncated"], true);
        assert!(content.contains("内容已按上下文预算截断"));
    }

    /** rewrite 工具会拒绝无法命中原文的 diff，避免生成不可应用变更。 */
    #[test]
    fn propose_note_change_rejects_original_not_found() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("这是一段可以被改写的正文内容。".to_owned());
        let request = tool_test_request("rewrite", "改写当前笔记");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(
            &mut context,
            "propose_note_change",
            json!({
                "noteId": "note-a",
                "original": "不存在的原文",
                "next": "新的建议"
            }),
        );

        assert_eq!(outcome.call.status, "failed");
        assert!(context.snapshot.sessions[0].pending_change.is_none());
    }

    /** rewrite 工具必须拒绝重复出现的 original，避免生成模糊 diff。 */
    #[test]
    fn propose_note_change_rejects_ambiguous_original() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("重复段落\n其他内容\n重复段落".to_owned());
        let request = tool_test_request("rewrite", "改写当前笔记");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(
            &mut context,
            "propose_note_change",
            json!({
                "noteId": "note-a",
                "original": "重复段落",
                "next": "新的建议"
            }),
        );

        assert_eq!(outcome.call.status, "failed");
        assert!(outcome.call.summary.contains("出现多次"));
        assert!(context.snapshot.sessions[0].pending_change.is_none());
    }

    /** rewrite 工具在 original 恰好命中一次时生成待确认 diff。 */
    #[test]
    fn propose_note_change_accepts_unique_original() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("第一段\n唯一段落\n第三段".to_owned());
        let request = tool_test_request("rewrite", "改写当前笔记");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(
            &mut context,
            "propose_note_change",
            json!({
                "noteId": "note-a",
                "original": "唯一段落",
                "next": "新的建议"
            }),
        );

        assert_eq!(outcome.call.status, "completed");
        assert_eq!(
            context.snapshot.sessions[0]
                .pending_change
                .as_ref()
                .map(|change| change.original.as_str()),
            Some("唯一段落")
        );
    }

    /** 局部 original 不能搭配整篇文档 next，否则确认后会把前文重复插入。 */
    #[test]
    fn propose_note_change_rejects_full_document_next_for_partial_replace() {
        let registry = ToolRegistry::default();
        let original_content = "第一段\n第二段\n第三段";
        let mut snapshot = tool_test_snapshot(original_content.to_owned());
        let request = tool_test_request("rewrite", "在文末追加内容");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(
            &mut context,
            "propose_note_change",
            json!({
                "noteId": "note-a",
                "operation": "replace",
                "original": "第二段",
                "next": format!("{}\n\n新增段落", original_content)
            }),
        );

        assert_eq!(outcome.call.status, "failed");
        assert!(outcome.call.summary.contains("正文重复"));
        assert!(context.snapshot.sessions[0].pending_change.is_none());
    }

    /** 文末追加必须使用 append，工具会把增量内容安全合成为整篇待确认 diff。 */
    #[test]
    fn propose_note_change_append_builds_full_note_replacement() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("第一段\n第二段".to_owned());
        let request = tool_test_request("rewrite", "在文末追加内容");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(
            &mut context,
            "propose_note_change",
            json!({
                "noteId": "note-a",
                "operation": "append",
                "next": "新增段落"
            }),
        );

        let change = context.snapshot.sessions[0]
            .pending_change
            .as_ref()
            .unwrap();

        assert_eq!(outcome.call.status, "completed");
        assert_eq!(change.operation.as_deref(), Some("append"));
        assert_eq!(change.original, "第一段\n第二段");
        assert_eq!(change.next, "第一段\n第二段\n\n新增段落");
    }

    /** 多处编辑应在工具层合成为整篇待确认 diff，避免模型拆成多个后续承诺。 */
    #[test]
    fn propose_note_change_multi_replace_builds_full_note_replacement() {
        let registry = ToolRegistry::default();
        let mut snapshot =
            tool_test_snapshot("标题\n重复段落一\n正文\n重复段落二\n结尾".to_owned());
        let request = tool_test_request("rewrite", "删除文档里的重复内容");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(
            &mut context,
            "propose_note_change",
            json!({
                "noteId": "note-a",
                "operation": "multi_replace",
                "edits": [
                    { "original": "重复段落一\n", "next": "" },
                    { "original": "重复段落二\n", "next": "" }
                ]
            }),
        );
        let change = context.snapshot.sessions[0]
            .pending_change
            .as_ref()
            .unwrap();

        assert_eq!(outcome.call.status, "completed");
        assert_eq!(change.operation.as_deref(), Some("multi_replace"));
        assert_eq!(change.original, "标题\n重复段落一\n正文\n重复段落二\n结尾");
        assert_eq!(change.next, "标题\n正文\n结尾");
    }

    /** 多处编辑支持 occurrence 精确删除重复片段中的指定一次。 */
    #[test]
    fn propose_note_change_multi_replace_accepts_occurrence_for_duplicates() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("开头\n重复段落\n中间\n重复段落\n结尾".to_owned());
        let request = tool_test_request("rewrite", "删除后面的重复段落");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(
            &mut context,
            "propose_note_change",
            json!({
                "noteId": "note-a",
                "operation": "multi_replace",
                "edits": [
                    { "original": "重复段落\n", "next": "", "occurrence": 2 }
                ]
            }),
        );
        let change = context.snapshot.sessions[0]
            .pending_change
            .as_ref()
            .unwrap();

        assert_eq!(outcome.call.status, "completed");
        assert_eq!(change.operation.as_deref(), Some("multi_replace"));
        assert_eq!(change.next, "开头\n重复段落\n中间\n结尾");
    }

    /** propose_note_change 必须拒绝 scope 外笔记。 */
    #[test]
    fn propose_note_change_rejects_note_outside_scope() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("这是一段可以被改写的正文内容。".to_owned());
        let request = tool_test_request("rewrite", "改写当前笔记");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(
            &mut context,
            "propose_note_change",
            json!({ "noteId": "note-b", "next": "新的建议" }),
        );

        assert_eq!(outcome.call.status, "failed");
        assert!(context.snapshot.sessions[0].pending_change.is_none());
    }

    /** 未授权知识库不能成为 create_note_draft 的目标。 */
    #[test]
    fn create_note_draft_rejects_knowledge_base_outside_scope() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = tool_test_request("create", "生成草稿");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(
            &mut context,
            "create_note_draft",
            json!({
                "knowledgeBaseId": "kb-b",
                "targetPath": "Private/草稿.md",
                "content": "# 草稿"
            }),
        );

        assert_eq!(outcome.call.status, "failed");
        assert!(context.snapshot.sessions[0].pending_change.is_none());
    }
}
