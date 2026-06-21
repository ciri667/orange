use serde::{Deserialize, Serialize};

/** 知识库扫描与索引状态，对应前端 KnowledgeBaseStatus。 */
#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum KnowledgeBaseStatus {
    Idle,
    Scanning,
    Ready,
    Error,
}

/** Agent 首版支持的用户意图类型。 */
#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentActionType {
    Ask,
    Find,
    Rewrite,
    Create,
    Organize,
}

/** Agent 会话类型，决定默认上下文粒度。 */
#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentSessionType {
    Note,
    KnowledgeBase,
    Task,
}

/** Agent 工具调用状态，用于展示 loop 轨迹。 */
#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentToolCallStatus {
    Planned,
    Running,
    Completed,
    Failed,
}

/** 用户选择的本地 Markdown 知识库元信息。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeBase {
    pub id: String,
    pub name: String,
    pub path: String,
    pub description: String,
    pub status: String,
    pub note_count: usize,
    pub updated_at: String,
    pub is_default: bool,
    pub semantic_index_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_report: Option<ScanReport>,
}

/** 单次知识库扫描报告，用于向前端说明成功、失败和跳过目录。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub scanned_file_count: usize,
    pub failed_file_count: usize,
    pub skipped_directories: Vec<String>,
    pub errors: Vec<String>,
}

/** 单篇 Markdown 笔记，真实内容来自本地文件。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: String,
    pub knowledge_base_id: String,
    pub title: String,
    pub path: String,
    pub content: String,
    pub tags: Vec<String>,
    pub updated_at: String,
    pub backlinks: Vec<String>,
    pub content_hash: String,
}

/** 本地知识库中的真实目录，用于让空文件夹也能出现在目录树中。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderEntry {
    pub id: String,
    pub knowledge_base_id: String,
    pub name: String,
    pub path: String,
    pub updated_at: String,
}

/** Agent 引用来源，必须来自已执行的工具结果。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Citation {
    pub knowledge_base_id: String,
    pub knowledge_base_name: String,
    pub note_id: String,
    pub title: String,
    pub path: String,
    pub snippet: String,
    pub score: f64,
}

/** Agent loop 中的一次工具调用记录。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolCall {
    pub id: String,
    pub name: String,
    pub status: String,
    pub summary: String,
    pub args: serde_json::Value,
}

/** Agent 与用户的会话消息。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub action: Option<String>,
    pub citations: Option<Vec<Citation>>,
    pub tool_calls: Option<Vec<AgentToolCall>>,
}

/** Agent 对笔记提出的待确认变更。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposedChange {
    pub id: String,
    pub knowledge_base_id: String,
    pub note_id: Option<String>,
    pub r#type: String,
    pub title: String,
    pub target_path: String,
    pub original: String,
    pub next: String,
    pub original_hash: String,
    pub status: String,
}

/** Agent 会话上下文容器。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSession {
    pub id: String,
    pub title: String,
    pub r#type: String,
    pub knowledge_base_ids: Vec<String>,
    pub active_note_id: Option<String>,
    pub pinned_note_ids: Vec<String>,
    pub messages: Vec<AgentMessage>,
    pub pending_change: Option<ProposedChange>,
    pub created_at: String,
    pub updated_at: String,
}

/** 前后端传输的工作台快照。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSnapshot {
    pub knowledge_bases: Vec<KnowledgeBase>,
    #[serde(default)]
    pub folders: Vec<FolderEntry>,
    pub notes: Vec<Note>,
    pub sessions: Vec<AgentSession>,
    pub active_knowledge_base_id: String,
    pub active_note_id: String,
    pub active_session_id: String,
}

/** Tauri 目录选择器返回的知识库目录信息。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeBaseSelection {
    pub id: String,
    pub name: String,
    pub path: String,
    pub note_count: usize,
}

/** Agent 单轮请求，模型可在 loop 内自行选择工具。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnRequest {
    pub prompt: String,
    pub action: String,
    pub session_id: String,
    pub active_knowledge_base_id: String,
    pub active_note_id: String,
}

/** Agent 单轮返回结果。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnResult {
    pub snapshot: WorkspaceSnapshot,
}

/** 扫描知识库命令入参。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanKnowledgeBasePayload {
    pub snapshot: WorkspaceSnapshot,
    pub selection: KnowledgeBaseSelection,
}

/** 重新扫描单个已连接知识库的命令入参。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RescanKnowledgeBasePayload {
    pub snapshot: WorkspaceSnapshot,
    pub knowledge_base_id: String,
}

/** 保存当前笔记正文的命令入参，expected_hash 用于发现外部编辑器冲突。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveNoteContentPayload {
    pub snapshot: WorkspaceSnapshot,
    pub note_id: String,
    pub content: String,
    pub expected_hash: String,
}

/** 用户从目录树指定目录新建 Markdown 文档的命令入参。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNotePayload {
    pub snapshot: WorkspaceSnapshot,
    pub knowledge_base_id: String,
    #[serde(default)]
    pub parent_path: Option<String>,
    #[serde(default)]
    pub file_name: Option<String>,
}

/** 用户主动新建文件夹的命令入参，只允许在知识库内创建单级目录。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateFolderPayload {
    pub snapshot: WorkspaceSnapshot,
    pub knowledge_base_id: String,
    pub parent_path: String,
    pub folder_name: String,
}

/** 重命名当前 Markdown 文件的命令入参，只改文件名，不改正文标题。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameNotePayload {
    pub snapshot: WorkspaceSnapshot,
    pub note_id: String,
    pub next_file_name: String,
}

/** 删除 Markdown 文件的命令入参，expected_hash 用于删除前冲突检测。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteNotePayload {
    pub snapshot: WorkspaceSnapshot,
    pub note_id: String,
    pub expected_hash: String,
}

/** 移除知识库授权记录的命令入参，不会删除用户 Markdown 文件。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveKnowledgeBasePayload {
    pub snapshot: WorkspaceSnapshot,
    pub knowledge_base_id: String,
}

/** Agent loop 命令入参。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnPayload {
    pub snapshot: WorkspaceSnapshot,
    pub request: AgentTurnRequest,
}

/** diff 确认或取消命令入参。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangePayload {
    pub snapshot: WorkspaceSnapshot,
}
