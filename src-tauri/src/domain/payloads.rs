use super::agent::{AgentSession, AgentTurnRequest, KnowledgeBaseMemory};
use super::im::ImIntegrationSettings;
use super::knowledge::{KnowledgeBaseSelection, NoteImageAttachmentInput};
use super::session::WorkspaceSnapshot;
use super::settings::UserSettings;
use super::skills::AgentSkill;
use serde::{Deserialize, Serialize};

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

/** 粘贴图片保存命令入参，正文不在此命令内写回 Markdown。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveNoteImageAttachmentsPayload {
    pub snapshot: WorkspaceSnapshot,
    pub note_id: String,
    pub images: Vec<NoteImageAttachmentInput>,
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

/** 在知识库根目录创建 AGENTS.md 项目说明书的命令入参。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectInstructionPayload {
    pub snapshot: WorkspaceSnapshot,
    pub knowledge_base_id: String,
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

/** 加载 docx/pdf/图片只读预览的命令入参。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadDocumentPreviewPayload {
    pub snapshot: WorkspaceSnapshot,
    pub document_id: String,
}

/** 读取当前文件历史列表的命令入参；targetKind 只接受 note/document。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadDocumentHistoryPayload {
    pub snapshot: WorkspaceSnapshot,
    pub target_kind: String,
    pub target_id: String,
}

/** 读取单条历史正文快照的命令入参；entryId 会在后端再次做文件名安全校验。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadDocumentHistoryEntryPayload {
    pub entry_id: String,
}

/** 回档写入命令入参，expectedHash 用于发现外部编辑器冲突。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreDocumentHistoryEntryPayload {
    pub snapshot: WorkspaceSnapshot,
    pub entry_id: String,
    pub expected_hash: String,
}

/** 清空当前文件历史记录的命令入参；只删除历史快照，不删除用户文档。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClearDocumentHistoryPayload {
    pub snapshot: WorkspaceSnapshot,
    pub target_kind: String,
    pub target_id: String,
}

/** 当前文件导出的目标类型，note 对应 Markdown，document 对应 TXT/DOCX/PDF/图片。 */
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

/** 用户手动中断当前 Agent 回合；可与 `run_agent_turn` 并发。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AbortAgentTurnPayload {
    pub session_id: String,
}

/** diff 确认或取消命令入参。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangePayload {
    pub snapshot: WorkspaceSnapshot,
}

/** 手动整理当前 Agent 会话上下文的命令入参。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactAgentContextPayload {
    pub snapshot: WorkspaceSnapshot,
    pub session_id: String,
}

/** 读取某会话最近一次发给模型的上下文预览。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadAgentPromptDumpPayload {
    pub session_id: String,
}

/** 保存或更新单个知识库跨会话记忆的命令入参。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveKnowledgeBaseMemoryPayload {
    pub knowledge_base_id: String,
    pub memory: KnowledgeBaseMemory,
}

/** 删除单个知识库跨会话记忆的命令入参。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteKnowledgeBaseMemoryPayload {
    pub knowledge_base_id: String,
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

/** 保存即时通讯设置的命令入参；敏感凭证必须走单独命令进入 keyring。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveImSettingsPayload {
    pub settings: ImIntegrationSettings,
}

/** 保存 BYOK 模型密钥的命令入参；密钥只进入系统安全存储，按 providerId 隔离。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveModelApiKeyPayload {
    pub provider_id: String,
    pub api_key: String,
}

/** 用户主动查看模型密钥的命令入参；只接受 providerId，不接受 keyReference。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevealModelApiKeyPayload {
    pub provider_id: String,
}

/** 刷新指定 LLM provider 模型列表的命令入参；不包含明文 API key。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshLlmProviderModelsPayload {
    pub provider_id: String,
}

/** 保存 IM provider 密钥的命令入参；明文只进入系统安全存储。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveImProviderSecretPayload {
    pub provider_id: String,
    pub secret: String,
}

/** 指定 IM provider 的命令入参；启停和状态读取都通过 providerId 路由。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImProviderPayload {
    pub provider_id: String,
}

/** 保存飞书 appSecret 的兼容命令入参；明文只进入系统安全存储。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveFeishuSecretPayload {
    pub app_secret: String,
}

/** 保存用户自建 skill 的命令入参；内置 skill 不能通过该入口修改。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveAgentSkillPayload {
    pub skill: AgentSkill,
}

/** 启停 skill 的命令入参；启用的 skill 会进入 Agent 可参考目录。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToggleAgentSkillPayload {
    pub skill_id: String,
    pub enabled: bool,
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
    /** 非空时只安装名称匹配的 Skill，避免发现页把整个仓库装进来。 */
    #[serde(default)]
    pub skill_names: Vec<String>,
}

/** 在线搜索 Agent Skills 的命令入参；owner 用于官方等来源收窄。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchOnlineSkillsPayload {
    pub query: String,
    #[serde(default)]
    pub owner: Option<String>,
}

/** 预览在线 Skill 简介的命令入参，id 为 skills.sh 的 owner/repo/skill 路径。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewOnlineSkillPayload {
    pub id: String,
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
