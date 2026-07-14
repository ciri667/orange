/** 知识库扫描与索引状态，用于区分目录授权、扫描中和可检索状态。 */
export type KnowledgeBaseStatus = "idle" | "scanning" | "ready" | "error";

/** Agent 首版支持的用户意图类型，运行时会进一步决定是否调用工具。 */
export type AgentActionType = "ask" | "find" | "rewrite" | "create" | "organize";

/** Agent 会话类型，用于区分笔记上下文、知识库上下文和临时任务上下文。 */
export type AgentSessionType = "note" | "knowledge-base" | "task";

/** Markdown 编辑器视图模式，控制编辑、预览和分屏布局。 */
export type MarkdownViewMode = "edit" | "preview" | "split";

/** Agent 工具名称，检索是可选择工具而不是固定前置流程。 */
export type AgentToolName =
  | "activate_skill"
  | "skill_context"
  | "model_request"
  | "local_rule_agent"
  | "review_change"
  | "search_notes"
  | "read_file"
  | "list_tree"
  | "get_current_file"
  | "get_session_summary"
  | "search_session_messages"
  | "read_session_context"
  | "propose_file_change"
  | "create_file_draft"
  | "suggest_organization";

/** Agent 工具调用状态，用于前端展示本轮 loop 的执行轨迹。 */
export type AgentToolCallStatus = "planned" | "running" | "completed" | "failed";

/** 首版云端模型提供商，M3 先固定 OpenAI-compatible BYOK 协议。 */
export type ModelProvider = "openai-compatible";

/** 模型条目来源：discovered 来自 provider API，manual 来自用户手填或兼容旧配置。 */
export type LlmProviderModelSource = "discovered" | "manual";

/** 用户选择的隐私策略，决定模型请求是否允许携带本地笔记片段。 */
export type PrivacyPolicy = "local-only" | "allow-selected-scope";

/** 非 Markdown 文档类型，决定目录树操作权限和中间面板展示方式。 */
export type DocumentFileType = "txt" | "docx" | "pdf" | "image";

/** 当前文件导出格式，original 保留原文件，markdown/pdf 走轻量转换。 */
export type ExportFormat = "original" | "markdown" | "pdf";

/** 当前文件导出目标类型，note 对应 Markdown，document 对应 TXT/DOCX/PDF/图片。 */
export type ExportTargetKind = "note" | "document";

/** 文档历史记录目标类型；首版只覆盖可写 Markdown 和 TXT 文件。 */
export type DocumentHistoryTargetKind = "note" | "document";

/** 文档历史记录来源，用于区分用户保存、Agent 写入和回档写入。 */
export type DocumentHistorySource = "manual-save" | "agent-change" | "restore";

/** 文档历史记录文件类型；Markdown 使用独立值，TXT 沿用普通文档能力。 */
export type DocumentHistoryFileType = "markdown" | "txt";

/** 可预览文档的正文块类型，首版 docx 只抽取段落级文本。 */
export type DocumentPreviewBlockType = "heading" | "paragraph";

/** 用户选择的本地知识库元信息。 */
export interface KnowledgeBase {
  id: string;
  name: string;
  path: string;
  description: string;
  status: KnowledgeBaseStatus;
  noteCount: number;
  documentCount: number;
  updatedAt: string;
  isDefault: boolean;
  semanticIndexEnabled: boolean;
  scanReport?: ScanReport;
}

/** Skill 启用状态，供设置页筛选和展示启用数量。 */
export type AgentSkillStatus = "enabled" | "disabled";

/** Skill 来源，内置能力只能禁用，自定义能力来自用户 Skills 目录并允许管理。 */
export type AgentSkillSource = "built-in" | "custom";

/** Skill 安装来源类型，URL、本地文件夹和本地 zip 走不同的后端准备流程。 */
export type SkillInstallSourceType = "url" | "localFolder" | "localArchive";

/** Skill 安装同名冲突策略，fail 保守失败，replace 由用户明确替换。 */
export type SkillInstallConflictStrategy = "fail" | "replace";

/** Agent skill 是可启停、可由 Agent 自主决定是否使用的指令型工作流。 */
export interface AgentSkill {
  id: string;
  name: string;
  displayName: string;
  description: string;
  instructions: string;
  tags: string[];
  enabled: boolean;
  source: AgentSkillSource;
  createdAt: string;
  updatedAt: string;
  /** 自定义 skill 的 SKILL.md 绝对路径，内置 skill 为空。 */
  path?: string;
  /** 自定义 skill 相对用户 skills 根目录的路径，用于列表展示和排序。 */
  relativePath?: string;
  /** 自定义 skill 的解析元数据，首版只记录覆盖来源等轻量信息。 */
  metadata?: Record<string, string>;
}

/** 第三方 skill 安装请求；本地来源 source 为空时桌面端会打开系统选择器。 */
export interface InstallAgentSkillPayload {
  sourceType: SkillInstallSourceType;
  source?: string;
  enableAfterInstall: boolean;
  conflictStrategy: SkillInstallConflictStrategy;
}

/** 第三方 skill 安装结果，包含刷新后的完整列表和脱敏摘要。 */
export interface InstallAgentSkillResult {
  installedSkills: AgentSkill[];
  skills: AgentSkill[];
  warnings: string[];
  summary: string;
  sourceType: SkillInstallSourceType;
  sourceSummary: string;
  installedCount: number;
  fileCount: number;
}

/** 单次知识库扫描报告，用于展示成功、失败、跳过目录和错误信息。 */
export interface ScanReport {
  scannedFileCount: number;
  scannedByType: Record<"markdown" | DocumentFileType, number>;
  failedFileCount: number;
  skippedDirectories: string[];
  errors: string[];
}

/** 单篇 Markdown 笔记的数据模型，正式桌面版应映射到本地文件。 */
export interface Note {
  id: string;
  knowledgeBaseId: string;
  title: string;
  path: string;
  content: string;
  tags: string[];
  updatedAt: string;
  backlinks: string[];
  contentHash: string;
}

/** 非 Markdown 文档的数据模型；txt 带正文，docx/pdf/图片只保存预览所需元数据。 */
export interface WorkspaceDocument {
  id: string;
  knowledgeBaseId: string;
  title: string;
  path: string;
  fileType: DocumentFileType;
  updatedAt: string;
  contentHash: string;
  content?: string;
  previewAvailable: boolean;
}

/** docx 预览中经过安全抽取的只读文本块。 */
export interface DocumentPreviewBlock {
  type: DocumentPreviewBlockType;
  text: string;
}

/** 非 Markdown 文档预览命令返回值，pdf/图片返回 asset 路径，docx 返回结构化文本。 */
export interface DocumentPreview {
  documentId: string;
  fileType: DocumentFileType;
  title: string;
  path: string;
  updatedAt: string;
  contentHash: string;
  assetPath?: string;
  blocks?: DocumentPreviewBlock[];
}

/** 单条文档历史记录摘要；正文内容只在打开详情时按需读取。 */
export interface DocumentHistoryEntry {
  id: string;
  targetKind: DocumentHistoryTargetKind;
  knowledgeBaseId: string;
  targetId: string;
  relativePath: string;
  title: string;
  fileType: DocumentHistoryFileType;
  contentHash: string;
  byteSize: number;
  lineCount: number;
  source: DocumentHistorySource;
  sessionId?: string;
  changeId?: string;
  operationId?: string;
  createdAt: string;
}

/** 文档历史记录详情，包含可用于 diff 和回档写入的正文快照。 */
export interface DocumentHistoryEntryDetail extends DocumentHistoryEntry {
  content: string;
}

/** 当前文件导出结果；targetPath 仅用于前端即时提示，不进入前端日志。 */
export interface ExportFileResult {
  format: ExportFormat;
  targetPath: string;
  fileName: string;
  byteSize: number;
}

/** 本地知识库中的真实目录节点，用于显示没有 Markdown 文件的空文件夹。 */
export interface FolderEntry {
  id: string;
  knowledgeBaseId: string;
  name: string;
  path: string;
  updatedAt: string;
}

/** Agent 回答引用的笔记来源信息，必须来自已执行的检索或读取工具。 */
export interface Citation {
  knowledgeBaseId: string;
  knowledgeBaseName: string;
  noteId: string;
  title: string;
  path: string;
  snippet: string;
  score: number;
}

/** Agent loop 中的一次工具调用记录。 */
export interface AgentToolCall {
  id: string;
  name: AgentToolName;
  status: AgentToolCallStatus;
  summary: string;
  args: Record<string, unknown>;
}

/** Agent 与用户的会话消息，可携带引用和工具调用轨迹。 */
export interface AgentMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  action?: AgentActionType;
  /** 本条用户消息显式 @ 的文件 ID；仅记录本轮材料，不作为后续会话自动上下文。 */
  mentionedFileIds?: string[];
  citations?: Citation[];
  toolCalls?: AgentToolCall[];
}

/** 审阅评论绑定到 diff 的一侧和行号，正文只进入会话消息，不进入诊断日志。 */
export interface ReviewComment {
  id: string;
  changeId: string;
  lineSide: "original" | "next";
  lineNumber: number;
  lineTextPreview: string;
  body: string;
  status: "draft" | "submitted" | "resolved";
  createdAt: string;
}

/** 待写入变更的审阅状态，只记录交互进度，不影响最终整次应用策略。 */
export interface ProposedChangeReviewState {
  selectedCommentId?: string;
  selectedLineSide?: ReviewComment["lineSide"];
  selectedLineNumber?: number;
  commentCount: number;
  submittedCommentCount: number;
  updatedAt: string;
}

/** diff 摘要统计，供审阅头部和日志使用，避免记录正文内容。 */
export interface ProposedChangeDiffStats {
  addedLines: number;
  removedLines: number;
  contextLines: number;
  hunkCount: number;
  originalLineCount: number;
  nextLineCount: number;
  originalCharCount: number;
  nextCharCount: number;
}

/** 工作记忆中被引用过的笔记摘要，只保存 id/title/reason，不保存正文。 */
export interface AgentContextTouchedNote {
  id: string;
  title: string;
  reason: string;
}

/** Agent 会话滚动工作记忆，压缩早期对话和工具结果以支撑长会话。 */
export interface AgentContextSummary {
  version: number;
  updatedAt: string;
  currentGoal?: string;
  userConstraints: string[];
  decisions: string[];
  completedWork: string[];
  pendingTasks: string[];
  touchedNotes: AgentContextTouchedNote[];
  pendingChangeSummary?: string;
  openQuestions: string[];
  lastSummarizedMessageId?: string;
  lastCompactedMessageId?: string;
}

/** 跨会话记忆单条；保存前会做敏感信息脱敏，content 中可能含 `[已脱敏]` 占位。 */
export interface AgentMemoryEntry {
  id: string;
  category: string;
  content: string;
  source: string;
  createdAt: string;
  updatedAt: string;
}

/** 单个知识库的跨会话记忆集合；默认关闭，用户在设置页手动开启后注入 Runtime。 */
export interface KnowledgeBaseMemory {
  knowledgeBaseId: string;
  enabled: boolean;
  entries: AgentMemoryEntry[];
  updatedAt: string;
}

/** Agent 对可编辑 Markdown/TXT 文件提出的待确认变更，确认前不能修改本地文件。 */
export interface ProposedChange {
  id: string;
  knowledgeBaseId: string;
  noteId?: string;
  /** 可编辑目标的统一 ID；旧会话的 noteId 会被迁移为该字段。 */
  targetId?: string;
  targetKind?: "note" | "document";
  fileType?: "markdown" | "txt";
  type: "rewrite" | "create" | "organize";
  operation?: "replace" | "append" | "multi_replace";
  title: string;
  targetPath: string;
  original: string;
  next: string;
  originalHash: string;
  status: "pending" | "accepted" | "rejected";
  reviewComments?: ReviewComment[];
  reviewState?: ProposedChangeReviewState;
  diffStats?: ProposedChangeDiffStats;
}

/** Agent 会话是上下文容器，绑定知识库范围、笔记、消息和待确认写入。 */
export interface AgentSession {
  id: string;
  title: string;
  /** IM 会话身份；本地创建的 Agent 会话不携带该字段。 */
  imIdentity?: ImSessionIdentity;
  type: AgentSessionType;
  knowledgeBaseIds: string[];
  activeNoteId?: string;
  pinnedNoteIds: string[];
  messages: AgentMessage[];
  pendingChange?: ProposedChange;
  /** 会话滚动工作记忆，用于让模型在只带最近历史时仍保留早期目标和决定。 */
  contextSummary?: AgentContextSummary;
  createdAt: string;
  updatedAt: string;
  /** 逻辑删除时间；有值的会话只保留在持久化记录中，不再进入普通会话列表。 */
  deletedAt?: string;
  /** 会话默认使用的 LLM Provider；缺省时回退到全局默认 provider。 */
  modelProviderId?: string;
  /** 会话默认使用的模型 ID；必须和 modelProviderId 指向的 provider 配套使用。 */
  modelId?: string;
}

/** IM 会话的展示身份；通道仅保留不可逆的脱敏指纹。 */
export interface ImSessionIdentity {
  providerId: string;
  conversationKind: "direct" | "group" | "unknown";
  channelHash: string;
  initialMessagePreview: string;
  lastMessagePreview: string;
}

/** 单个 provider 下可供用户启用和选择的模型条目。 */
export interface LlmProviderModel {
  id: string;
  name: string;
  ownedBy?: string;
  enabled: boolean;
  source: LlmProviderModelSource;
  contextLength?: number;
  created?: number;
  updatedAt: string;
}

/** 单个 LLM Provider 实例配置；用户可以配置多个 provider 并按需切换。 */
export interface LlmProviderConfig {
  id: string;
  name: string;
  provider: ModelProvider;
  apiBase: string;
  model: string;
  keyReference: string;
  enabled: boolean;
  supportsTools: boolean;
  /** 是否需要配置 API key；本地免鉴权服务（如 Ollama）可以关闭 key 校验。 */
  requiresApiKey: boolean;
  /** 自动发现或手动保留的模型列表；为空时仍使用 model 字段兼容旧配置。 */
  models: LlmProviderModel[];
  /** 最近一次从 provider API 获取模型列表的本地时间。 */
  modelsFetchedAt?: string;
  createdAt: string;
  updatedAt: string;
}

/** 设置页“新增 Provider”入口使用的预置模板，来自后端内置模板注册表。 */
export interface ProviderTemplate {
  templateId: string;
  name: string;
  provider: ModelProvider;
  apiBase: string;
  model: string;
  requiresApiKey: boolean;
}

/** 云端模型设置聚合多个 Provider；默认 Provider 决定未显式选择时使用哪一个。 */
export interface ModelConfig {
  enabled: boolean;
  defaultProviderId: string;
  providers: LlmProviderConfig[];
}

/** 用户设置聚合模型、隐私和写入确认策略，供 M3 Runtime 读取。 */
export interface UserSettings {
  modelConfig: ModelConfig;
  privacyPolicy: PrivacyPolicy;
  writeConfirmationRequired: boolean;
}

/** 当前内置 IM provider；新增 provider 时继续使用稳定小写 ID。 */
export type ImProviderId = "feishu";

/** 飞书/Lark 自建应用专属配置；appSecret 单独保存在系统安全存储。 */
export interface FeishuProviderConfig {
  type: "feishu";
  domain: "feishu" | "lark";
  appId: string;
  secretKeyReference: string;
}

/** IM provider 平台专属配置；新增 IM 时在这里扩展联合类型。 */
export type ImProviderConfig = FeishuProviderConfig;

/** 单个 IM provider 的通用配置；平台专属字段放在 config 中。 */
export interface ImProviderSettings {
  providerId: ImProviderId;
  enabled: boolean;
  defaultKnowledgeBaseIds: string[];
  allowedUserOpenIds: string[];
  allowedChatIds: string[];
  discoveredUserOpenIds: string[];
  discoveredChatIds: string[];
  requireMention: boolean;
  updatedAt: string;
  config: ImProviderConfig;
}

/** 即时通讯集成总设置，providers 是未来扩展多个 IM 的固定入口。 */
export interface ImIntegrationSettings {
  providers: ImProviderSettings[];
}

/** 兼容旧设置页局部类型命名；实际持久化已经使用 provider 结构。 */
export type FeishuIntegrationSettings = ImProviderSettings & {
  providerId: "feishu";
  config: FeishuProviderConfig;
};

/** IM provider secret 保存状态；不包含明文 secret。 */
export interface ImProviderCredentialStatus {
  providerId: ImProviderId;
  keyReference: string;
  configured: boolean;
  message: string;
}

/** 兼容旧飞书状态命名；实际接口已经 provider 化。 */
export type FeishuCredentialStatus = ImProviderCredentialStatus;

/** IM provider 长连接网关运行态，设置页用它展示手动启停结果。 */
export interface ImGatewayStatus {
  providerId: ImProviderId;
  running: boolean;
  connected: boolean;
  domain: "feishu" | "lark";
  appIdConfigured: boolean;
  secretConfigured: boolean;
  lastStartedAt?: string;
  lastStoppedAt?: string;
  lastError?: string;
}

/** 兼容旧飞书网关状态命名；实际接口已经 provider 化。 */
export type FeishuGatewayStatus = ImGatewayStatus;

/** 模型密钥状态只说明是否可读取，不包含明文密钥；按 providerId 隔离。 */
export interface ModelApiKeyStatus {
  providerId: string;
  keyReference: string;
  configured: boolean;
  message: string;
}

/** 刷新单个 provider 模型列表后的结果摘要，不包含密钥或响应正文。 */
export interface LlmProviderModelRefreshResult {
  settings: UserSettings;
  providerId: string;
  fetchedAt: string;
  fetchedCount: number;
  modelCount: number;
  enabledCount: number;
  message: string;
}

/** 模型请求和本地工具调用审计摘要，用于解释每轮 Agent 使用了哪些范围。 */
export interface RequestAuditLog {
  id: string;
  kind: string;
  sessionId?: string;
  scopeSummary: string;
  contentSummary: string;
  toolSummary: string;
  createdAt: string;
}

/** 应用事件日志级别，用于设置页筛选运行诊断和关键操作。 */
export type AppEventLogLevel = "debug" | "info" | "warn" | "error";

/** 应用事件日志分类，和 Rust logging.rs 中的 AppLogCategory 保持一致。 */
export type AppEventLogCategory =
  | "app"
  | "storage"
  | "knowledge_base"
  | "editor"
  | "agent"
  | "model"
  | "im"
  | "skill"
  | "settings"
  | "security"
  | "frontend";

/** 用户可读应用事件日志，和 Agent 请求审计分开展示。 */
export interface AppEventLog {
  id: string;
  level: AppEventLogLevel;
  category: AppEventLogCategory;
  event: string;
  message: string;
  status: string;
  operationId?: string;
  sessionId?: string;
  knowledgeBaseId?: string;
  entityType?: string;
  entityId?: string;
  relativePath?: string;
  durationMs?: number;
  metadataJson?: string;
  createdAt: string;
}

/** 本地目录树节点，用于把 Markdown 路径还原成文件夹和文件层级。 */
export interface FileTreeNode {
  id: string;
  name: string;
  path: string;
  type: "folder" | "file";
  fileType?: "markdown" | DocumentFileType;
  noteId?: string;
  documentId?: string;
  capabilities?: {
    canEdit: boolean;
    canRename: boolean;
    canDelete: boolean;
    canPreview: boolean;
  };
  isRoot?: boolean;
  children: FileTreeNode[];
}

/** 工作台快照，是前端和 Tauri command 之间的首版数据传输对象。 */
export interface WorkspaceSnapshot {
  knowledgeBases: KnowledgeBase[];
  folders: FolderEntry[];
  notes: Note[];
  documents: WorkspaceDocument[];
  sessions: AgentSession[];
  activeKnowledgeBaseId: string;
  activeNoteId: string;
  activeDocumentId?: string;
  activeSessionId: string;
}

/** Agent 单轮请求，运行时会在 loop 内自行选择是否调用工具。 */
export interface AgentTurnRequest {
  prompt: string;
  action: AgentActionType;
  sessionId: string;
  activeKnowledgeBaseId: string;
  activeNoteId: string;
  /** 本轮由用户显式 @ 的文件 ID；后端会再次进行 scope 与存在性校验。 */
  mentionedFileIds?: string[];
  /** 前端已乐观渲染并持久化的用户消息 ID，运行时复用它避免重复追加。 */
  clientMessageId?: string;
  /** 本轮显式选择的 Provider；优先级高于会话默认和全局默认。 */
  modelProviderId?: string;
  /** 本轮显式选择的模型 ID；和 modelProviderId 一起参与选择优先级。 */
  modelId?: string;
  /** 本轮通过 slash picker 显式激活的 Skill ID；只作用于当前 turn，不写入会话。 */
  explicitSkillIds?: string[];
}

/** Agent 单轮返回结果，包含更新后的完整工作台状态。 */
export interface AgentTurnResult {
  snapshot: WorkspaceSnapshot;
}

/** 单张待保存的粘贴图片；bytesBase64 只传给 Tauri 命令，不能进入日志。 */
export interface NoteImageAttachmentInput {
  mimeType: string;
  bytesBase64: string;
  originalFileName?: string;
}

/** 已保存的图片附件，relativePath 是相对当前 Markdown 文件的引用路径。 */
export interface SavedNoteImageAttachment {
  relativePath: string;
  markdown: string;
  mimeType: string;
  byteSize: number;
}

/** 知识库目录选择结果，Tauri 环境中来自系统目录选择器。 */
export interface KnowledgeBaseSelection {
  id: string;
  name: string;
  path: string;
  noteCount: number;
}
