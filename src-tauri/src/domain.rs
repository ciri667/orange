use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

/** 首版云端模型提供商，M3 先固定 OpenAI-compatible BYOK 协议。 */
#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelProvider {
    OpenaiCompatible,
}

/** 用户选择的隐私策略，决定模型请求是否允许携带本地笔记片段。 */
#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PrivacyPolicy {
    LocalOnly,
    AllowSelectedScope,
}

/** Skill 启用状态类型，前端用它派生列表筛选和状态标签。 */
#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentSkillStatus {
    Enabled,
    Disabled,
}

/** Skill 来源类型，内置 skill 和文件 skill 只能禁用，用户 skill 才允许编辑或删除。 */
#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentSkillSource {
    BuiltIn,
    File,
    User,
}

/** Skill 参考模式，用于设置模型可参考能力目录还是仅显式选择。 */
#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentSkillActivationMode {
    Auto,
    Manual,
}

/** Skill 安装来源类型，URL、本地目录和本地压缩包走不同的准备流程。 */
#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SkillInstallSourceType {
    Url,
    LocalFolder,
    LocalArchive,
}

/** Skill 安装遇到同名目录时的处理策略。 */
#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SkillInstallConflictStrategy {
    Fail,
    Replace,
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
    #[serde(default)]
    pub document_count: usize,
    pub updated_at: String,
    pub is_default: bool,
    pub semantic_index_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_report: Option<ScanReport>,
}

/** 支持文档类型的扫描计数，默认补齐四类避免旧快照缺字段时报错。 */
pub fn default_scanned_by_type() -> HashMap<String, usize> {
    HashMap::from([
        ("markdown".to_owned(), 0),
        ("txt".to_owned(), 0),
        ("docx".to_owned(), 0),
        ("pdf".to_owned(), 0),
    ])
}

/** 单次知识库扫描报告，用于向前端说明成功、失败和跳过目录。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub scanned_file_count: usize,
    #[serde(default = "default_scanned_by_type")]
    pub scanned_by_type: HashMap<String, usize>,
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

/** 非 Markdown 文档，txt 带正文，docx/pdf 只存只读预览所需元数据。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDocument {
    pub id: String,
    pub knowledge_base_id: String,
    pub title: String,
    pub path: String,
    pub file_type: String,
    pub updated_at: String,
    pub content_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    pub preview_available: bool,
}

/** docx 只读预览的段落级文本块。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentPreviewBlock {
    pub r#type: String,
    pub text: String,
}

/** 非 Markdown 文档预览返回值，pdf 使用 assetPath，docx 使用 blocks。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentPreview {
    pub document_id: String,
    pub file_type: String,
    pub title: String,
    pub path: String,
    pub updated_at: String,
    pub content_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<DocumentPreviewBlock>>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<String>,
}

/** 云端模型配置只保存 key 引用，不在普通 SQLite payload 中保存明文密钥。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelConfig {
    pub provider: String,
    pub api_base: String,
    pub model: String,
    pub key_reference: String,
    pub enabled: bool,
}

/** Agent skill 是可启停的指令型工作流包，首版不携带脚本或外部命令。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkill {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub instructions: String,
    pub tags: Vec<String>,
    pub enabled: bool,
    pub source: String,
    pub allow_auto_invoke: bool,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
}

/** 旧版 Skill 全局设置兼容字段；当前运行时始终注入已启用 Skill 的名称和描述。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSettings {
    pub activation_mode: String,
}

/** 旧版用户设置缺少 skillSettings 时使用兼容默认值。 */
pub fn default_skill_settings() -> SkillSettings {
    SkillSettings {
        activation_mode: "auto".to_owned(),
    }
}

/** 用户设置聚合模型、隐私和写入确认策略，供 M3 Runtime 读取。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSettings {
    pub model_config: ModelConfig,
    pub privacy_policy: String,
    pub write_confirmation_required: bool,
    #[serde(default = "default_skill_settings")]
    pub skill_settings: SkillSettings,
}

/** 模型密钥保存状态，只暴露是否可读取，不返回明文密钥。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelApiKeyStatus {
    pub key_reference: String,
    pub configured: bool,
    pub message: String,
}

/** 模型请求和本地工具调用审计摘要，用于解释每轮 Agent 使用了哪些范围。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestAuditLog {
    pub id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub scope_summary: String,
    pub content_summary: String,
    pub tool_summary: String,
    pub created_at: String,
}

/** 用户可读的应用事件日志，和模型请求审计分开保存，避免运行诊断污染 Agent 边界说明。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppEventLog {
    pub id: String,
    pub level: String,
    pub category: String,
    pub event: String,
    pub message: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge_base_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_json: Option<String>,
    pub created_at: String,
}

/** 读取应用事件日志的筛选入参；缺省时返回最近日志。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadAppEventLogsPayload {
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
}

/** 前后端传输的工作台快照。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSnapshot {
    pub knowledge_bases: Vec<KnowledgeBase>,
    #[serde(default)]
    pub folders: Vec<FolderEntry>,
    pub notes: Vec<Note>,
    #[serde(default)]
    pub documents: Vec<WorkspaceDocument>,
    pub sessions: Vec<AgentSession>,
    pub active_knowledge_base_id: String,
    pub active_note_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub active_document_id: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_skill_id: Option<String>,
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

/** 单张待保存的粘贴图片；bytesBase64 只在命令边界传输，不能写入日志。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteImageAttachmentInput {
    pub mime_type: String,
    pub bytes_base64: String,
    #[serde(default)]
    pub original_file_name: Option<String>,
}

/** 粘贴图片保存命令入参，正文不在此命令内写回 Markdown。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveNoteImageAttachmentsPayload {
    pub snapshot: WorkspaceSnapshot,
    pub note_id: String,
    pub images: Vec<NoteImageAttachmentInput>,
}

/** 已保存的图片附件，relativePath 是相对当前 Markdown 文件的引用路径。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedNoteImageAttachment {
    pub relative_path: String,
    pub markdown: String,
    pub mime_type: String,
    pub byte_size: usize,
}

/** 保存 txt 文档正文的命令入参，expectedHash 用于发现外部编辑器冲突。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveDocumentContentPayload {
    pub snapshot: WorkspaceSnapshot,
    pub document_id: String,
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

/** 用户从目录树指定目录新建 txt 文档的命令入参。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDocumentPayload {
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

/** 重命名 txt 文档的命令入参，只改文件名，不改变正文。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameDocumentPayload {
    pub snapshot: WorkspaceSnapshot,
    pub document_id: String,
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

/** 删除 txt 文档的命令入参，expectedHash 用于删除前冲突检测。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteDocumentPayload {
    pub snapshot: WorkspaceSnapshot,
    pub document_id: String,
    pub expected_hash: String,
}

/** 加载 docx/pdf 只读预览的命令入参。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadDocumentPreviewPayload {
    pub snapshot: WorkspaceSnapshot,
    pub document_id: String,
}

/** 当前文件导出的目标类型，note 对应 Markdown，document 对应 TXT/DOCX/PDF。 */
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportTargetKind {
    Note,
    Document,
}

/** 当前文件导出的格式；original 保留源文件，markdown/pdf 执行轻量转换。 */
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportFormat {
    Original,
    Markdown,
    Pdf,
}

/** 当前文件导出命令入参；正文内容只通过 snapshot 定位，不额外跨 IPC 传输。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportCurrentFilePayload {
    pub snapshot: WorkspaceSnapshot,
    pub target_kind: ExportTargetKind,
    pub target_id: String,
    pub format: ExportFormat,
}

/** 当前文件导出结果；targetPath 只返回给前端提示，不写入后端日志。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportFileResult {
    pub format: ExportFormat,
    pub target_path: String,
    pub file_name: String,
    pub byte_size: u64,
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

/** 持久化或更新单个 Agent 会话的命令入参。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSessionPayload {
    pub snapshot: WorkspaceSnapshot,
    pub session: AgentSession,
}

/** 逻辑删除 Agent 会话的命令入参；会话 payload 会保留 deletedAt 标记。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSessionPayload {
    pub snapshot: WorkspaceSnapshot,
    pub session_id: String,
}

/** 读取会话列表时携带当前快照，用于清理失效知识库和笔记引用。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadSessionsPayload {
    pub snapshot: WorkspaceSnapshot,
}

/** 更新会话检索范围的命令入参，后端会强制保留当前激活知识库。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSessionScopePayload {
    pub snapshot: WorkspaceSnapshot,
    pub session_id: String,
    pub knowledge_base_ids: Vec<String>,
    pub active_knowledge_base_id: String,
}

/** 从历史会话恢复知识库和笔记上下文的命令入参。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreSessionContextPayload {
    pub snapshot: WorkspaceSnapshot,
    pub session_id: String,
}

/** 保存用户模型和隐私设置的命令入参。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveUserSettingsPayload {
    pub settings: UserSettings,
}

/** 保存 BYOK 模型密钥的命令入参；密钥只进入系统安全存储。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveModelApiKeyPayload {
    pub api_key: String,
}

/** 保存用户自建 skill 的命令入参；内置 skill 不能通过该入口修改。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveAgentSkillPayload {
    pub skill: AgentSkill,
}

/** 启停 skill 的命令入参，allowAutoInvoke 仅保留旧客户端兼容性。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToggleAgentSkillPayload {
    pub skill_id: String,
    pub enabled: bool,
    #[serde(default)]
    pub allow_auto_invoke: Option<bool>,
}

/** 删除用户自建 skill 的命令入参；内置 skill 只能禁用不能删除。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteAgentSkillPayload {
    pub skill_id: String,
}

/** 安装第三方 skill 的命令入参；本地来源 source 为空时由后端打开系统选择器。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallAgentSkillPayload {
    pub source_type: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub enable_after_install: bool,
    pub conflict_strategy: String,
}

/** 安装第三方 skill 后返回安装项、刷新列表和脱敏摘要。 */
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallAgentSkillResult {
    pub installed_skills: Vec<AgentSkill>,
    pub skills: Vec<AgentSkill>,
    pub warnings: Vec<String>,
    pub summary: String,
    pub source_type: String,
    pub source_summary: String,
    pub installed_count: usize,
    pub file_count: usize,
}
