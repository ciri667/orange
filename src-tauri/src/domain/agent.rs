use super::im::ImSessionIdentity;
use serde::{Deserialize, Serialize};

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
    /** DOCX/PDF 等只读文档的块或页码定位；Markdown 引用保持为空。 */
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
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

/** Agent 一轮中的一个过程步骤，按时间顺序记录模型思考或用户可见工具调用。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTraceStep {
    pub id: String,
    #[serde(rename = "type")]
    pub step_type: String,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
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
    /** 本条用户消息在发送时显式 @ 的文件 ID；仅用于历史回显，不会成为长期上下文。 */
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mentioned_file_ids: Vec<String>,
    /** 本轮过程时间线；旧会话没有该字段时按空轨迹兼容。 */
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trace: Vec<AgentTraceStep>,
    /** 本轮墙钟耗时，供过程区展示「已处理 12s」。 */
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_duration_ms: Option<u64>,
}

/** Agent 对可编辑文本文件提出的待确认变更；正文始终留在 diff 中，不进入日志。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposedChange {
    pub id: String,
    pub knowledge_base_id: String,
    /** 兼容历史会话的 Markdown ID；加载后由迁移逻辑补齐 target_id。 */
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_type: Option<String>,
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    pub title: String,
    pub target_path: String,
    pub original: String,
    pub next: String,
    pub original_hash: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_comments: Option<Vec<ReviewComment>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_state: Option<ProposedChangeReviewState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_stats: Option<ProposedChangeDiffStats>,
}

/** 变更集中一项待确认的文件操作；create_folder 操作的 file_type 为 "folder"，不带正文。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposedFileOperation {
    pub id: String,
    pub knowledge_base_id: String,
    pub operation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    pub target_path: String,
    pub file_type: String,
    pub original_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    pub selected: bool,
    pub binary: bool,
    pub byte_size: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged_path: Option<String>,
}

/** Agent 直接产出（非 Skill 执行）的变更集使用的执行 ID 占位符。 */
pub const AGENT_DIRECT_EXECUTION_ID: &str = "agent-direct";

/** Agent 直接产出变更集时填入 ProposedChangeSet.skill_id 的来源标记。 */
pub const AGENT_DIRECT_SOURCE: &str = "agent";

/** 完全级别下，知识库范围外的合规路径使用该 scope id，避免误绑到某个知识库。 */
pub const EXTERNAL_FILESYSTEM_SCOPE_ID: &str = "external";

/** 一次执行产生的多文件变更集；executionId 为 agent-direct 时表示由 Agent 工具直接生成，应用前始终保留待确认。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposedChangeSet {
    pub id: String,
    pub execution_id: String,
    pub skill_id: String,
    pub status: String,
    pub summary: String,
    pub operations: Vec<ProposedFileOperation>,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub created_at: String,
}

/** Skill 命令在真正运行前持久化的审批请求。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillExecutionRequest {
    pub id: String,
    pub skill_id: String,
    pub skill_name: String,
    pub package_hash: String,
    pub runtime: String,
    pub command_preview: String,
    pub args: Vec<String>,
    pub knowledge_base_ids: Vec<String>,
    #[serde(default)]
    pub network_domains: Vec<String>,
    #[serde(default)]
    pub credential_aliases: Vec<String>,
    pub status: String,
    pub created_at: String,
}

/** 审阅评论绑定到 diff 的一侧和行号，正文只随会话 payload 传递。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewComment {
    pub id: String,
    pub change_id: String,
    pub line_side: String,
    pub line_number: usize,
    pub line_text_preview: String,
    pub body: String,
    pub status: String,
    pub created_at: String,
}

/** 待写入变更的审阅状态，供前端恢复选择和评论数量。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposedChangeReviewState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_comment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_line_side: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_line_number: Option<usize>,
    pub comment_count: usize,
    pub submitted_comment_count: usize,
    pub updated_at: String,
}

/** diff 摘要统计只记录数量，不保存正文。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposedChangeDiffStats {
    pub added_lines: usize,
    pub removed_lines: usize,
    pub context_lines: usize,
    pub hunk_count: usize,
    pub original_line_count: usize,
    pub next_line_count: usize,
    pub original_char_count: usize,
    pub next_char_count: usize,
}

/** 工作记忆中被引用过的笔记摘要，只保存 id/title/reason，不保存正文。 */
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentContextTouchedNote {
    pub id: String,
    pub title: String,
    pub reason: String,
}

/** Agent 会话滚动工作记忆，压缩早期对话和工具结果以支撑长会话。 */
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentContextSummary {
    pub version: u32,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_goal: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub user_constraints: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completed_work: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_tasks: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub touched_notes: Vec<AgentContextTouchedNote>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_change_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open_questions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_summarized_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_compacted_message_id: Option<String>,
}

/** 最近一次有效的模型 usage；全零或失败响应不得覆盖。 */
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentContextUsage {
    pub model_id: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub recorded_at: String,
    /** 记账时采用的模型窗口；Provider 未提供时为空。 */
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u64>,
}

/** 最近一次发给模型的上下文预览；完整正文只留在日志目录 JSON。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPromptDump {
    pub session_id: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_context_length: Option<u64>,
    pub recorded_at: String,
    pub round: u32,
    pub kind: String,
    pub total_chars: usize,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file_path: String,
    pub outline: String,
    pub messages: Vec<AgentPromptDumpMessage>,
}

/** 单条发给模型的消息预览；content 仅写入日志文件。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPromptDumpMessage {
    pub index: usize,
    pub role: String,
    pub chars: usize,
    pub preview: String,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/** Agent 会话上下文容器。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSession {
    pub id: String,
    pub title: String,
    /** IM 来源身份；普通本地会话保持为空，避免将 provider 细节散落到 UI。 */
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub im_identity: Option<ImSessionIdentity>,
    pub r#type: String,
    pub knowledge_base_ids: Vec<String>,
    pub active_note_id: Option<String>,
    pub pinned_note_ids: Vec<String>,
    pub messages: Vec<AgentMessage>,
    pub pending_change: Option<ProposedChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_change_set: Option<ProposedChangeSet>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_execution: Option<SkillExecutionRequest>,
    /** 安全级别按会话固化；IM 会话始终由入口强制降为 basic。 */
    #[serde(default = "default_agent_security_level")]
    pub security_level: String,
    /** 会话滚动工作记忆，用于让模型在只带最近历史时仍保留早期目标和决定。 */
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_summary: Option<AgentContextSummary>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<String>,
    /** 会话默认使用的 LLM Provider；缺省时回退到全局默认 provider。 */
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider_id: Option<String>,
    /** 会话默认使用的模型 ID；和 model_provider_id 配套决定具体模型。 */
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /** 最近一次非 abort、非 error、usage>0 的成功响应记账，供界面展示占用。 */
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_usage: Option<AgentContextUsage>,
}

/** 跨会话记忆的分类枚举，覆盖计划文档列举的长期偏好类型。 */
pub const MEMORY_CATEGORY_NOTE_STRUCTURE: &str = "noteStructure";

pub const MEMORY_CATEGORY_TAG_CONVENTION: &str = "tagConvention";

pub const MEMORY_CATEGORY_ORGANIZATION: &str = "organization";

pub const MEMORY_CATEGORY_CONVENTION: &str = "convention";

pub const MEMORY_CATEGORY_OTHER: &str = "other";

/** 跨会话记忆条目的来源取值；user 表示手动录入，auto 预留给后续 Agent 自动生成路径。 */
pub const MEMORY_SOURCE_USER: &str = "user";

pub const MEMORY_SOURCE_AUTO: &str = "auto";

/** 跨会话记忆单条；只保存用户偏好与约定，保存前会做敏感信息脱敏。 */
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMemoryEntry {
    pub id: String,
    pub category: String,
    pub content: String,
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
}

/** 单个知识库的跨会话记忆集合；默认关闭，用户在设置页手动开启后注入 Runtime。 */
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeBaseMemory {
    pub knowledge_base_id: String,
    pub enabled: bool,
    #[serde(default)]
    pub entries: Vec<AgentMemoryEntry>,
    pub updated_at: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentSecurityLevel {
    Basic,
    Advanced,
    #[serde(rename = "autonomous")]
    Full,
}

impl Default for AgentSecurityLevel {
    fn default() -> Self {
        Self::Basic
    }
}

impl AgentSecurityLevel {
    /** 解析会话/设置中的安全级别字符串；未知值一律降为基础级别。 */
    pub fn parse(value: &str) -> Self {
        match value {
            "advanced" => Self::Advanced,
            "autonomous" | "full" => Self::Full,
            _ => Self::Basic,
        }
    }

    /** 进阶/完全级别可使用通用文件工具（如 create_folder）。 */
    pub fn allows_general_fs_tools(self) -> bool {
        !matches!(self, Self::Basic)
    }

    /** 完全级别允许对知识库根目录之外的合规绝对路径进行操作。 */
    pub fn allows_external_filesystem(self) -> bool {
        matches!(self, Self::Full)
    }

    /** 完全级别允许校验通过后自动落盘，不再中途打断用户确认。 */
    pub fn allows_auto_apply(self) -> bool {
        matches!(self, Self::Full)
    }
}

fn default_agent_security_level() -> String {
    "basic".to_owned()
}

fn default_agent_resource_limits() -> AgentResourceLimits {
    AgentResourceLimits {
        timeout_seconds: 120,
        max_memory_mb: 512,
        max_processes: 20,
        max_artifact_mb: 100,
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentResourceLimits {
    pub timeout_seconds: u64,
    pub max_memory_mb: u64,
    pub max_processes: u32,
    pub max_artifact_mb: u64,
}

impl Default for AgentResourceLimits {
    fn default() -> Self {
        default_agent_resource_limits()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedSkillGrant {
    pub skill_id: String,
    pub package_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSecuritySettings {
    #[serde(default = "default_agent_security_level")]
    pub default_level: String,
    #[serde(default)]
    pub advanced_execution_enabled: bool,
    #[serde(default)]
    pub autonomous_mode_enabled: bool,
    #[serde(default = "default_agent_resource_limits")]
    pub resource_limits: AgentResourceLimits,
    #[serde(default)]
    pub trusted_skill_grants: Vec<TrustedSkillGrant>,
    #[serde(default)]
    pub allowed_network_domains: Vec<String>,
}

impl Default for AgentSecuritySettings {
    fn default() -> Self {
        Self {
            default_level: default_agent_security_level(),
            advanced_execution_enabled: false,
            autonomous_mode_enabled: false,
            resource_limits: default_agent_resource_limits(),
            trusted_skill_grants: Vec::new(),
            allowed_network_domains: Vec::new(),
        }
    }
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
    /** 本轮显式选择的 Provider；优先级高于会话默认和全局默认。 */
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider_id: Option<String>,
    /** 本轮显式选择的模型 ID；和 model_provider_id 一起决定具体模型。 */
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /** 本轮通过 slash picker 显式激活的 Skill ID；默认空数组兼容历史请求。 */
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub explicit_skill_ids: Vec<String>,
    /** 本轮用户显式 @ 的文件 ID；Runtime 会在会话 scope 内重新校验。 */
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mentioned_file_ids: Vec<String>,
}
