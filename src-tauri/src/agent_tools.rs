use crate::domain::{
    AgentSession, AgentToolCall, AgentTurnRequest, Citation, ProposedChange, WorkspaceSnapshot,
};
use crate::storage::{create_id, hash_content};
use crate::text_edit::{count_non_overlapping_matches, UniqueReplacementError};
use serde_json::{json, Value};
use std::collections::HashSet;
use tauri::AppHandle;

/** 单次 read_note 工具最多发送给模型的正文字符数。 */
pub(crate) const MAX_READ_NOTE_CHARS: usize = 6000;

/** list_tree 工具最多发送的目录和笔记摘要数量。 */
const MAX_TREE_ITEMS: usize = 120;

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

/** list_tree 工具，列出当前 scope 内目录和笔记摘要，供模型自主判断下一步。 */
struct ListTreeTool;

impl AgentTool for ListTreeTool {
    fn name(&self) -> &'static str {
        "list_tree"
    }

    fn description(&self) -> &'static str {
        "List folders and notes inside the selected scope."
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
                "original": { "type": "string" },
                "next": { "type": "string" }
            },
            "required": ["noteId", "next"]
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

/** 执行 list_tree，只返回当前 scope 内的目录和笔记摘要。 */
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
    let truncated = scoped_folders.len() > MAX_TREE_ITEMS || scoped_notes.len() > MAX_TREE_ITEMS;

    ToolExecutionResult {
        success: true,
        summary: format!(
            "已列出 {} 个目录和 {} 篇笔记{}",
            scoped_folders.len(),
            scoped_notes.len(),
            if truncated {
                "，结果已按预算截断"
            } else {
                ""
            }
        ),
        payload: json!({
            "folders": folders,
            "notes": notes,
            "totalFolders": scoped_folders.len(),
            "totalNotes": scoped_notes.len(),
            "truncated": truncated
        }),
        citations: Vec::new(),
        audit_fragment: Some(format!(
            "list_tree 发送 {} 个目录摘要、{} 篇笔记摘要{}",
            scoped_folders.len().min(MAX_TREE_ITEMS),
            scoped_notes.len().min(MAX_TREE_ITEMS),
            if truncated { "（已截断）" } else { "" }
        )),
    }
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
    let original = args
        .get("original")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| first_body_paragraph(&note.content));
    let next = args
        .get("next")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();

    if original.is_empty() || next.is_empty() {
        return ToolExecutionResult::failed("改写工具缺少 original 或 next 内容。");
    }

    match validate_unique_original(&note.content, &original) {
        Ok(()) => {}
        Err(UniqueReplacementError::NotFound) => {
            return ToolExecutionResult::failed(
                "改写工具的 original 未命中目标笔记，已拒绝生成不可应用 diff。",
            );
        }
        Err(UniqueReplacementError::Ambiguous { .. }) => {
            return ToolExecutionResult::failed(
                "改写工具的 original 在目标笔记中出现多次，已拒绝生成模糊 diff。请提供更长、更唯一的原文片段。",
            );
        }
        Err(UniqueReplacementError::EmptyOriginal) => {
            return ToolExecutionResult::failed("改写工具缺少 original 或 next 内容。");
        }
    }

    let change = ProposedChange {
        id: create_id("change"),
        knowledge_base_id: note.knowledge_base_id.clone(),
        note_id: Some(note.id.clone()),
        r#type: "rewrite".to_owned(),
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
    };

    snapshot.sessions[session_index].pending_change = Some(change.clone());
    let audit_fragment = Some(format!(
        "propose_note_change 为《{}》生成 diff，原文 {} 字符，建议 {} 字符",
        note.title,
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
    use crate::domain::{FolderEntry, KnowledgeBase, Note};

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
            selected_skill_id: None,
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
