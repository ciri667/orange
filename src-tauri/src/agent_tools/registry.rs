use super::execute::{
    CreateFileDraftTool, ListTreeTool, ProposeFileChangeTool, ReadFileTool, RunSkillTool,
    SearchNotesTool,
};
use super::types::*;
use crate::domain::{AgentSecuritySettings, AgentSession, AgentToolCall};
use crate::storage::create_id;
use serde_json::{json, Value};

pub struct ToolRegistry {
    /** 已注册工具列表；顺序稳定，便于 UI 和测试比对 schema。 */
    tools: Vec<Box<dyn AgentTool>>,
}

impl Default for ToolRegistry {
    /** 基础闭集：search/read/list/edit/write。进阶以上的 run 与范围扩展由 for_session 追加。 */
    fn default() -> Self {
        Self {
            tools: vec![
                Box::new(SearchNotesTool),
                Box::new(ReadFileTool),
                Box::new(ListTreeTool),
                Box::new(ProposeFileChangeTool),
                Box::new(CreateFileDraftTool),
            ],
        }
    }
}

impl ToolRegistry {
    /** 按会话安全级别构造工具集；高权限工具在注册层不可见，而不是执行时才软拒绝。 */
    pub fn for_session(session: &AgentSession, settings: &AgentSecuritySettings) -> Self {
        let mut registry = Self::default();
        let local_session = session.im_identity.is_none();
        let can_run_skills = local_session
            && session.security_level != "basic"
            && settings.advanced_execution_enabled
            && (session.security_level != "autonomous" || settings.autonomous_mode_enabled);

        if can_run_skills {
            registry.tools.push(Box::new(RunSkillTool));
        }

        registry
    }

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
        // 兼容已经持久化的旧模型调用；schema 只暴露闭集短名，不再引导模型使用别名。
        let (canonical_name, canonical_args) = remap_tool_call(name, args);
        if let Some(message) = retired_host_tool_message(canonical_name) {
            return tool_outcome(
                canonical_name,
                canonical_args,
                ToolExecutionResult::failed(message),
            );
        }
        let result = self
            .tools
            .iter()
            .find(|tool| tool.name() == canonical_name)
            .map(|tool| tool.execute(context, &canonical_args))
            .unwrap_or_else(|| ToolExecutionResult::failed("未知工具，已拒绝执行。"));

        tool_outcome(canonical_name, canonical_args, result)
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

pub(crate) fn remap_tool_call(name: &str, args: Value) -> (&str, Value) {
    match name {
        "search_notes" => ("search", args),
        "read_file" | "read_note" | "read_document" | "get_current_file" => {
            ("read", remap_read_args(name, args))
        }
        "list_tree" => ("list", args),
        "propose_file_change" | "propose_note_change" => ("edit", remap_legacy_file_id(args)),
        "create_file_draft" | "create_note_draft" => ("write", remap_legacy_markdown_draft(args)),
        "create_folder" => ("write", remap_write_folder_args(args)),
        "list_path" => ("list", args),
        "read_path" => ("read", args),
        "run_skill" => ("run", args),
        _ => (name, args),
    }
}

/** 已降级为宿主注入的工具：旧名调用返回结构化失败，避免旧模型缓存空转。 */
pub(crate) fn retired_host_tool_message(name: &str) -> Option<&'static str> {
    match name {
        "get_session_summary" | "get_knowledge_base_memory" | "search_session_messages"
        | "read_session_context" => Some(
            "该信息已由宿主注入当前上下文，请直接使用 system 中的工作记忆、待确认变更或跨会话记忆，不要再调用此工具。",
        ),
        "suggest_organization" => {
            Some("请直接在回复中给出整理建议；若要落盘请调用 edit 或 write。")
        }
        _ => None,
    }
}

/** 把旧读取参数收成 read 可识别的 fileId/documentId。 */
pub(crate) fn remap_read_args(original_name: &str, args: Value) -> Value {
    if original_name == "read_note" {
        return remap_legacy_file_id(args);
    }
    args
}

/** 旧 create_folder 收成 write + kind=folder。 */
pub(crate) fn remap_write_folder_args(mut args: Value) -> Value {
    if let Some(object) = args.as_object_mut() {
        object.insert("kind".to_owned(), Value::String("folder".to_owned()));
    }
    args
}

/** 参数里是否带了文件系统 path，用于 list/read 在完全级别走外部路径。 */
pub(crate) fn tool_path_arg(args: &Value) -> Option<&str> {
    args.get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/** write 是建文件还是建文件夹。 */
pub(crate) fn write_kind(args: &Value) -> &str {
    let kind = args
        .get("kind")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if kind.eq_ignore_ascii_case("folder") {
        return "folder";
    }
    let file_type = args
        .get("fileType")
        .or_else(|| args.get("file_type"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if file_type.eq_ignore_ascii_case("folder") {
        "folder"
    } else {
        "file"
    }
}

/** 读取可选正整数参数，缺省或非法时回退 default，并夹在 1..=max。 */
pub(crate) fn parse_limit_arg(args: &Value, default: usize, max: usize) -> usize {
    args.get("limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(default)
        .clamp(1, max)
}

/** read 的字符窗口：offset 从 0 起，limit 不超过单次正文预算。 */
pub(crate) fn read_window(args: &Value) -> (usize, usize) {
    let offset = args
        .get("offset")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(0);
    let limit = parse_limit_arg(args, MAX_READ_NOTE_CHARS, MAX_READ_NOTE_CHARS);
    (offset, limit)
}

/** 按字符 offset/limit 切片正文，返回切片、是否截断、下一 offset。 */
pub(crate) fn slice_chars(
    value: &str,
    offset: usize,
    limit: usize,
) -> (String, bool, Option<usize>) {
    let total = value.chars().count();
    if offset >= total {
        return (String::new(), false, None);
    }
    let sliced: String = value.chars().skip(offset).take(limit).collect();
    let end = offset + sliced.chars().count();
    let truncated = end < total;
    let next_offset = truncated.then_some(end);
    (sliced, truncated, next_offset)
}

/** 截断时给模型的下一步；未截断不返回 hint。 */
pub(crate) fn truncation_hint(
    kind: &str,
    truncated: bool,
    next_offset: Option<usize>,
) -> Option<String> {
    if !truncated {
        return None;
    }
    match kind {
        "read" => next_offset
            .map(|offset| format!("Use offset={offset} to continue reading from this character.")),
        "list" => Some(
            "Narrow prefix or fileType, or increase limit (max 120) to see more items.".to_owned(),
        ),
        "search" => {
            Some("Narrow the query or increase limit (max 16) to see more citations.".to_owned())
        }
        _ => Some("The result was truncated; narrow the request or raise limit.".to_owned()),
    }
}

pub(crate) fn remap_legacy_file_id(mut args: Value) -> Value {
    if let Some(object) = args.as_object_mut() {
        if !object.contains_key("fileId") {
            if let Some(note_id) = object.get("noteId").cloned() {
                object.insert("fileId".to_owned(), note_id);
            }
        }
    }
    args
}

/** 为历史新建 Markdown 草稿补齐统一工具所需的类型字段。 */
pub(crate) fn remap_legacy_markdown_draft(mut args: Value) -> Value {
    if let Some(object) = args.as_object_mut() {
        object
            .entry("fileType")
            .or_insert_with(|| Value::String("markdown".to_owned()));
    }
    args
}

/** 构造 OpenAI-compatible function tool 描述。 */
pub(crate) fn function_tool(name: &str, description: &str, parameters: Value) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": parameters
        }
    })
}

/** 读取模型 tool_call 的 function.name，缺失时回退为 unknown_tool。 */
pub(crate) fn model_tool_call_name(model_tool_call: &Value) -> String {
    model_tool_call
        .get("function")
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("unknown_tool")
        .to_owned()
}

/** 解析模型 tool_call 的 arguments JSON 字符串。 */
pub(crate) fn parse_tool_args(model_tool_call: &Value) -> Value {
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
pub(crate) fn tool_outcome(name: &str, args: Value, result: ToolExecutionResult) -> ToolOutcome {
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
