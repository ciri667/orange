use crate::domain::{AgentToolCall, AgentTurnRequest, Citation, WorkspaceSnapshot};
use serde_json::{json, Value};
use tauri::AppHandle;

/** 单次 read_note 工具最多发送给模型的正文字符数。 */
pub(crate) const MAX_READ_NOTE_CHARS: usize = 6000;

/** list_tree 工具最多发送的目录、Markdown 和普通文档摘要数量。 */
pub(crate) const MAX_TREE_ITEMS: usize = 120;

/** 会话历史检索最多返回的消息数量，避免旧对话一次性塞满模型上下文。 */
pub(crate) const MAX_SESSION_CONTEXT_MESSAGES: usize = 24;

/** 会话历史工具单条消息最多返回的字符数。 */
pub(crate) const MAX_SESSION_CONTEXT_MESSAGE_CHARS: usize = 4000;

/** 跨会话记忆工具最多返回的条目数量，避免一次工具结果挤占模型上下文。 */
pub(crate) const MAX_KB_MEMORY_TOOL_ENTRIES: usize = 32;

/** 跨会话记忆工具单条内容最多返回的字符数，保存层和读取层都会做长度保护。 */
pub(crate) const MAX_KB_MEMORY_TOOL_ENTRY_CHARS: usize = 800;

/** list_path 单次最多返回的目录项数量，避免把超大目录一次性塞进模型上下文。 */
pub(crate) const MAX_LIST_PATH_ENTRIES: usize = 200;

/** read_path 最多读取的字节数；超出后按字符预算再截断。 */
pub(crate) const MAX_READ_PATH_BYTES: usize = 256 * 1024;

/** search 默认返回条数；未传 limit 时与旧行为一致。 */
pub(crate) const DEFAULT_SEARCH_LIMIT: usize = 4;

/** search 单次最多返回条数，避免一次检索塞满上下文。 */
pub(crate) const MAX_SEARCH_LIMIT: usize = 16;

/** list_tree 按支持文档类型输出的计数，避免模型把未知扩展名误认为已索引内容。 */
#[derive(Clone, Debug)]
pub(crate) struct ListTreeFileTypeCounts {
    pub(crate) markdown: usize,
    pub(crate) txt: usize,
    pub(crate) docx: usize,
    pub(crate) pdf: usize,
    pub(crate) image: usize,
}

/** Agent 一次多处编辑中的单个片段，original 必须唯一命中，next 允许为空表示删除。 */
#[derive(Clone, Debug)]
pub(crate) struct ProposedTextEdit {
    pub(crate) original: String,
    pub(crate) next: String,
    pub(crate) occurrence: Option<usize>,
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
