/** 知识库扫描与索引状态，用于区分目录授权、扫描中和可检索状态。 */
export type KnowledgeBaseStatus = "idle" | "scanning" | "ready" | "error";

/** Agent 首版支持的用户意图类型，运行时会进一步决定是否调用工具。 */
export type AgentActionType = "ask" | "find" | "rewrite" | "create" | "organize";

/** Agent 会话类型，用于区分笔记上下文、知识库上下文和临时任务上下文。 */
export type AgentSessionType = "note" | "knowledge-base" | "task";

/** Agent 安全等级决定工具范围和确认策略：基础由用户盯紧，进阶扩大能力仍需确认，完全在校验后连续执行。提高等级只能由用户主动操作。 */
export type AgentSecurityLevel = "workspace-readonly" | "workspace-write" | "full-confirm" | "full-auto";

/** Markdown 编辑器视图模式，控制编辑、预览和分屏布局。 */
export type MarkdownViewMode = "edit" | "preview" | "split";

/** Agent 工具名称。闭集短名是 schema 对外名；旧名保留以便历史会话轨迹仍能展示。 */
export type AgentToolName =
  | "activate_skill"
  | "skill_context"
  | "model_request"
  | "local_rule_agent"
  | "review_change"
  | "search"
  | "read"
  | "list"
  | "edit"
  | "write"
  | "exec"
  | "browser"
  | "run"
  | "task"
  | "search_notes"
  | "read_file"
  | "read_document"
  | "list_tree"
  | "get_current_file"
  | "get_session_summary"
  | "search_session_messages"
  | "read_session_context"
  | "run_skill"
  | "create_folder"
  | "list_path"
  | "read_path"
  | "propose_file_change"
  | "create_file_draft"
  | "suggest_organization";

/** Agent 工具调用状态，用于前端展示本轮 loop 的执行轨迹。 */
export type AgentToolCallStatus = "planned" | "running" | "completed" | "failed" | "aborted";

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
export type DocumentPreviewBlockType = "heading" | "paragraph" | "table";

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

/** 内置斜杠指令的可用入口；同一条指令可以同时服务桌面端与 IM。 */
export type BuiltinCommandScope = "desktop" | "im";

/**
 * 内置斜杠指令描述符，由后端注册表统一下发。
 * 与 `AgentSkill` 不同：指令是本地动作，执行时绝不进入模型上下文。
 */
export interface BuiltinCommandDescriptor {
  /** 稳定 ID，用于执行与埋点，不随文案调整变化。 */
  id: string;
  /** 主命令名，不含前导斜杠。 */
  name: string;
  /** 兼容别名，解析时与主名等价，弹窗中不重复展示。 */
  aliases: string[];
  description: string;
  /** 参数占位提示，空串表示不接受参数。 */
  argumentHint: string;
  scopes: BuiltinCommandScope[];
  /** 是否必须有活跃的持久化会话。 */
  requiresActiveSession: boolean;
}

/** 内置指令执行结果；snapshot 仅在指令改动了工作台时回传。 */
export interface BuiltinCommandRunResult {
  commandId: string;
  notice: string;
  snapshot?: WorkspaceSnapshot;
}

/** 手动整理上下文的结果；notice 说明摘要来源和实际丢弃的历史。 */
export interface CompactAgentContextResult {
  snapshot: WorkspaceSnapshot;
  notice: string;
}

/**
 * 最近一次手动整理上下文的回执，供上下文浮层就地展示。
 *
 * `at` 是毫秒时间戳而不是格式化时间：后端时间戳只精确到分钟，同一分钟内连续整理两次
 * 时无法用它触发浮层刷新，这里必须用一个每次都变化的值。
 */
export interface AgentContextCompactionReport {
  sessionId: string;
  notice: string;
  at: number;
  /** running 表示整理仍在进行，此时 notice 是进度文案而不是结果。 */
  status: "running" | "done" | "failed";
}

/** Agent skill 是可启停、可由 Agent 自主决定是否使用的指令型工作流。 */
export interface AgentSkill {
  id: string;
  name: string;
  displayName: string;
  description: string;
  instructions: string;
  resources?: string[];
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
  /** 历史运行声明，只作为包内参考，不赋予权限。 */
  runtimeManifest?: SkillRuntimeManifest;
  /** 安装包和本机运行时共同决定的兼容性结果。 */
  compatibility?: SkillCompatibilityReport;
}

export type SkillRuntimeKind = "python" | "node" | "powershell" | "bash" | "executable";

/** agents/orange-runtime.yaml 的稳定运行声明。 */
export interface SkillRuntimeManifest {
  runtime: SkillRuntimeKind;
  entry: string;
  args: string[];
  networkDomains: string[];
  credentialAliases: string[];
  artifactPatterns: string[];
}

export interface SkillRuntimeStatus {
  runtime: SkillRuntimeKind;
  available: boolean;
  executablePath?: string;
  version?: string;
  message: string;
}

export type SkillCompatibilityStatus = "instruction-only" | "ready" | "missing-runtime" | "approval-required" | "partial" | "unsupported";

export interface SkillCompatibilityReport {
  status: SkillCompatibilityStatus;
  packageHash: string;
  runtime?: SkillRuntimeStatus;
  warnings: string[];
}

/** 第三方 skill 安装请求；本地来源 source 为空时桌面端会打开系统选择器。 */
export interface InstallAgentSkillPayload {
  sourceType: SkillInstallSourceType;
  source?: string;
  enableAfterInstall: boolean;
  conflictStrategy: SkillInstallConflictStrategy;
  /** 非空时只安装名称匹配的 Skill，避免发现页把整个仓库装进来。 */
  skillNames?: string[];
}

/** skills.sh 目录中的一条可发现 Skill。 */
export interface OnlineSkill {
  id: string;
  skillId: string;
  name: string;
  source: string;
  installs: number;
  pageUrl: string;
  installable: boolean;
  description?: string;
}

/** 在线 Skill 搜索结果。 */
export interface OnlineSkillSearchResult {
  query: string;
  skills: OnlineSkill[];
}

/** 在线 Skill 预览；简介缺失时仍可展示名称和来源。 */
export interface OnlineSkillPreview {
  id: string;
  pageUrl: string;
  description?: string;
}

/** 在线搜索 Agent Skills 的请求。 */
export interface SearchOnlineSkillsPayload {
  query: string;
  owner?: string;
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
  page?: number;
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

/** Agent 回答引用的来源。笔记引用不带 url；网页引用 kind 为 web。 */
export interface Citation {
  knowledgeBaseId: string;
  knowledgeBaseName: string;
  noteId: string;
  title: string;
  path: string;
  snippet: string;
  score: number;
  location?: string;
  kind?: "note" | "web";
  url?: string;
  publishedAt?: string;
}

/** 联网搜索设置。密钥本身不在这里，只保留是否配过和最近一次探测状态。 */
export interface WebSearchSettings {
  enabled: boolean;
  provider: "tavily" | "brave" | "exa" | "serper";
  keyReference: string;
  credentialStatus: "untested" | "valid" | "invalid_credentials" | "rate_limited" | "network_error" | "not_configured";
}

/** 设置页读取的联网搜索密钥状态，不含明文。 */
export interface WebSearchCredentialStatus {
  configured: boolean;
  credentialStatus: WebSearchSettings["credentialStatus"];
  message: string;
}

/** Agent loop 中的一次工具调用记录。 */
export interface AgentToolCall {
  id: string;
  name: AgentToolName;
  status: AgentToolCallStatus;
  summary: string;
  args: Record<string, unknown>;
}

/** 过程时间线步骤类型：思考、工具前旁白或用户可见工具调用。 */
export type AgentTraceStepType = "thinking" | "narration" | "tool";

/** 一轮 Agent 中的一个过程步骤，按时间顺序交错思考、旁白和工具结果。 */
export interface AgentTraceStep {
  id: string;
  type: AgentTraceStepType;
  timestamp: string;
  content?: string;
  name?: AgentToolName;
  status?: AgentToolCallStatus;
  summary?: string;
  /** 折叠行上的步骤标题；完成后仍保留，不拿结果摘要覆盖。 */
  title?: string;
  args?: Record<string, unknown>;
  resultPreview?: string;
  error?: string;
  durationMs?: number;
  /** 子 Agent 嵌套过程；仅 task 步骤使用。 */
  children?: AgentTraceStep[];
  /** task 步骤的内置角色名，例如 explore / researcher。 */
  agent?: string;
  /** 可续跑的子 Agent id。 */
  taskId?: string;
}

/** 后端在 turn 执行中推送的过程快照，live 气泡和最终助手消息共用 liveMessageId。 */
export interface AgentTurnProgressEvent {
  sessionId: string;
  liveMessageId: string;
  status: "running" | "completed" | "failed" | "interrupted";
  steps: AgentTraceStep[];
  /** 后端已耗时；仅反映事件发出那一刻，不能拿来反推起点。 */
  turnDurationMs: number;
  /** 本轮开始的绝对时间（Unix 毫秒）；切换会话导致过程区重挂载时靠它继续计时。 */
  startedAtMs: number;
  content?: string;
}

/** 对话图片的持久引用；二进制留在附件目录，会话 JSON 只保存元数据。 */
export interface ConversationImageAttachment {
  id: string;
  mimeType: string;
  byteSize: number;
  width?: number;
  height?: number;
  name?: string;
  /** 本机绝对路径或浏览器 mock 的 data URL，仅供预览。 */
  absolutePath: string;
}

/** 前端提交给准入命令的图片字节；不得写入日志。 */
export interface ConversationImageInput {
  mimeType: string;
  bytesBase64: string;
  originalFileName?: string;
}

/** Agent 与用户的会话消息，可携带引用和工具调用轨迹。 */
export interface AgentMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  action?: AgentActionType;
  /** 本条用户消息显式 @ 的文件 ID；仅记录本轮材料，不作为后续会话自动上下文。 */
  mentionedFileIds?: string[];
  /** 本条用户消息上传的对话图片；旧消息缺省为空。 */
  images?: ConversationImageAttachment[];
  citations?: Citation[];
  toolCalls?: AgentToolCall[];
  /** 本轮过程时间线；旧消息没有该字段时回退到扁平 toolCalls。 */
  trace?: AgentTraceStep[];
  /** 本轮墙钟耗时，过程区展示「已处理 12s」。 */
  turnDurationMs?: number;
  /** 用户中断时保留已流出前缀；旧消息缺省为 false。 */
  interrupted?: boolean;
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

/** 最近一次有效的模型 usage；全零或失败响应不得覆盖。 */
export interface AgentContextUsage {
  modelId: string;
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  recordedAt: string;
  /** 记账时采用的模型窗口；Provider 未提供时为空。 */
  contextLength?: number;
}

/** 最近一次发给模型的上下文预览；完整正文只留在应用日志目录。 */
export interface AgentPromptDump {
  sessionId: string;
  modelId: string;
  modelContextLength?: number;
  recordedAt: string;
  round: number;
  kind: string;
  totalChars: number;
  filePath: string;
  outline: string;
  messages: AgentPromptDumpMessage[];
}

/** 单条发给模型的消息预览，不含完整正文。 */
export interface AgentPromptDumpMessage {
  index: number;
  role: string;
  chars: number;
  preview: string;
  truncated: boolean;
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
  /** replace 时的 1-based 命中序号；缺省表示 original 必须唯一。 */
  occurrence?: number;
  reviewComments?: ReviewComment[];
  reviewState?: ProposedChangeReviewState;
  diffStats?: ProposedChangeDiffStats;
}

export type ProposedFileOperationType = "create" | "modify" | "delete" | "move" | "rename" | "create_folder";

/** Skill 一次执行产生的单个文件影响。 */
export interface ProposedFileOperation {
  id: string;
  knowledgeBaseId: string;
  operation: ProposedFileOperationType;
  sourcePath?: string;
  targetPath: string;
  fileType: string;
  originalHash: string;
  original?: string;
  next?: string;
  selected: boolean;
  binary: boolean;
  byteSize: number;
  stagedPath?: string;
  status?: "pending" | "completed" | "failed";
  error?: string;
  affectedFiles?: number;
}

/** 通用文件操作的统一审批容器。 */
export interface ProposedChangeSet {
  id: string;
  executionId: string;
  skillId: string;
  summary: string;
  operations: ProposedFileOperation[];
  status: "pending" | "partial" | "applied" | "rejected" | "conflict" | "expired";
  warnings: string[];
  createdAt: string;
}

export type SkillExecutionStatus = "pending" | "running" | "completed" | "failed" | "rejected" | "expired";

/** 历史 Skill 执行请求，仅用于显示过期记录，不再执行。 */
export interface SkillExecutionRequest {
  id: string;
  skillId: string;
  skillName: string;
  packageHash: string;
  runtime: SkillRuntimeKind;
  commandPreview: string;
  args: string[];
  knowledgeBaseIds: string[];
  networkDomains: string[];
  credentialAliases: string[];
  status: SkillExecutionStatus;
  createdAt: string;
}

export interface AgentResourceLimits {
  timeoutSeconds: number;
  maxMemoryMb: number;
  maxProcesses: number;
  maxArtifactMb: number;
}

export interface TrustedSkillGrant {
  skillId: string;
  packageHash: string;
  expiresAt?: string;
}

export interface AgentSecuritySettings {
  defaultLevel: AgentSecurityLevel;
  advancedExecutionEnabled: boolean;
  autonomousModeEnabled: boolean;
  resourceLimits: AgentResourceLimits;
  trustedSkillGrants: TrustedSkillGrant[];
  allowedNetworkDomains: string[];
}

/** Agent 会话是上下文容器，绑定知识库范围、笔记、消息和待确认写入。 */
export interface AgentSession {
  id: string;
  title: string;
  /** 用户改过标题后为 true，首条消息和 IM 标题不再覆盖。 */
  titleCustomized?: boolean;
  /** 有值表示置顶。使用和 updatedAt 相同的本地时间字符串。 */
  pinnedAt?: string;
  /** 有值表示从默认列表收起。搜索和「已归档」仍能找到。 */
  archivedAt?: string;
  /** IM 会话身份；本地创建的 Agent 会话不携带该字段。 */
  imIdentity?: ImSessionIdentity;
  /** 定时任务身份；普通本地会话和 IM 会话不携带该字段。 */
  scheduleIdentity?: ScheduleSessionIdentity;
  type: AgentSessionType;
  knowledgeBaseIds: string[];
  activeNoteId?: string;
  pinnedNoteIds: string[];
  messages: AgentMessage[];
  pendingChange?: ProposedChange;
  /** 新执行链使用批量变更集；旧 pendingChange 保留兼容单文件审阅。 */
  pendingChangeSet?: ProposedChangeSet;
  pendingExecution?: SkillExecutionRequest;
  securityLevel: AgentSecurityLevel;
  permissionVersion?: number;
  /** 规划模式开关：开启后 Agent 只用只读工具先出执行计划，用户确认后才执行。IM/定时会话恒为 false。 */
  planMode: boolean;
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
  /** 最近一次有效 usage，供界面展示占用。 */
  contextUsage?: AgentContextUsage;
}

/** 定时任务绑定到 Agent 会话的身份。 */
export interface ScheduleSessionIdentity {
  jobId: string;
  jobName: string;
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
  agentSecurity: AgentSecuritySettings;
  webSearch: WebSearchSettings;
  semanticSearch: SemanticSearchSettings;
  browser: BrowserFeatureSettings;
}

/** Embedding 独立设置；密钥由系统 keyring 保存。 */
export interface SemanticSearchSettings {
  enabled: boolean;
  provider: "ollama" | "lm-studio" | "openai-compatible";
  apiBase: string;
  model: string;
  requiresApiKey: boolean;
  minSimilarity: number;
  cloudConsentFingerprint?: string | null;
}

export interface SemanticIndexStatus {
  knowledgeBaseId: string;
  enabled: boolean;
  paused: boolean;
  status: "disabled" | "pending" | "indexing" | "ready" | "paused" | "partial_failure" | "error" | "stale";
  indexedChunks: number;
  pendingChunks: number;
  failedChunks: number;
  totalChunks: number;
  dimensions?: number | null;
  message: string;
}

export type SemanticIndexAction = "enable" | "disable" | "pause" | "resume" | "rebuild";

/** 当前内置 IM provider；新增 provider 时继续使用稳定小写 ID。 */
export type ImProviderId = "feishu" | "qq" | "weixin" | "wecom";

/** 飞书/Lark 自建应用专属配置；appSecret 单独保存在系统安全存储。 */
export interface FeishuProviderConfig {
  type: "feishu";
  domain: "feishu" | "lark";
  appId: string;
  secretKeyReference: string;
}

/** QQ 官方机器人专属配置；AppSecret 单独保存在系统安全存储。 */
export interface QqProviderConfig {
  type: "qq";
  appId: string;
  secretKeyReference: string;
}

/** 个人微信 iLink 专属配置；token 单独保存在系统安全存储。 */
export interface WeixinProviderConfig {
  type: "weixin";
  accountId: string;
  baseUrl: string;
  secretKeyReference: string;
}

/** 企业微信智能机器人专属配置；Secret 单独保存在系统安全存储。 */
export interface WecomProviderConfig {
  type: "wecom";
  botId: string;
  robotName: string;
  wsUrl: string;
  secretKeyReference: string;
}

/** IM provider 平台专属配置；新增 IM 时在这里扩展联合类型。 */
export type ImProviderConfig = FeishuProviderConfig | QqProviderConfig | WeixinProviderConfig | WecomProviderConfig;

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

export type QqIntegrationSettings = ImProviderSettings & {
  providerId: "qq";
  config: QqProviderConfig;
};

export type WeixinIntegrationSettings = ImProviderSettings & {
  providerId: "weixin";
  config: WeixinProviderConfig;
};

export type WecomIntegrationSettings = ImProviderSettings & {
  providerId: "wecom";
  config: WecomProviderConfig;
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
  domain: string;
  appIdConfigured: boolean;
  secretConfigured: boolean;
  lastStartedAt?: string;
  lastStoppedAt?: string;
  lastError?: string;
}

/** 个人微信扫码登录状态；二维码只短暂存在于设置页会话。 */
export interface ImLoginStatus {
  providerId: ImProviderId | string;
  status: string;
  qrImageBase64?: string;
  accountId?: string;
  message: string;
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

/** 用户主动查看时返回的模型密钥；只在设置抽屉会话内短暂持有。 */
export interface RevealedModelApiKey {
  providerId: string;
  apiKey: string;
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

/** 编辑区打开文件的稳定引用；持久化在独立编辑器会话中，不写入工作区快照。 */
export interface EditorFileTab {
  kind: "note" | "document";
  id: string;
}

/** 右侧内容使用稳定引用；终端和网页仅存在于当前应用运行期。 */
export type RightPanelTab =
  | { kind: "session"; id: string }
  | { kind: "terminal"; id: string }
  | { kind: "browser"; id: string };

/** 终端进程描述，不含输入或输出正文。 */
export interface TerminalDescriptor {
  terminalId: string;
  shell: string;
  cwd: string;
}

/** 使用独立 IPC Channel 传输高频输出，字节流由前端持续解码。 */
export type TerminalEvent =
  | { kind: "output"; data: number[] }
  | { kind: "exit"; code: number | null }
  | { kind: "error"; message: string };

/** 编辑器会话状态；只保存稳定 ID 和标签顺序，不保存文件标题、路径或正文。 */
export interface WorkspaceEditorState {
  activeKnowledgeBaseId: string;
  openTabs: EditorFileTab[];
  activeTab?: EditorFileTab;
  updatedAt: string;
}

/** 工作台启动结果，分离领域快照和仅属于编辑器的本机会话状态。 */
export interface WorkspaceBootstrapState {
  snapshot: WorkspaceSnapshot;
  editorState: WorkspaceEditorState;
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
  /** 本轮已准入的对话图片 ID；后端按附件目录重新解析。 */
  imageIds?: string[];
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

/** 与 Codex 对齐的星期枚举。 */
export type JobWeekday = "MO" | "TU" | "WE" | "TH" | "FR" | "SA" | "SU";

/** 结构化周期，不做 crontab。 */
export type JobSchedule =
  | { type: "hourly"; intervalHours: number; days?: JobWeekday[] }
  | { type: "daily"; time: string }
  | { type: "weekdays"; time: string }
  | { type: "weekly"; days: JobWeekday[]; time: string }
  | { type: "once"; at: string };

export type ScheduleOutputMode = "session" | "inbox";
export type ScheduleSessionPolicy = "newEachRun" | "continue";

/** 一条持久化定时任务。 */
export interface ScheduledJob {
  id: string;
  name: string;
  prompt: string;
  schedule: JobSchedule;
  scheduleDisplay: string;
  timezone: string;
  knowledgeBaseIds: string[];
  outputMode: ScheduleOutputMode | string;
  inboxKnowledgeBaseId?: string;
  inboxFolder?: string;
  sessionPolicy: ScheduleSessionPolicy | string;
  continuedSessionId?: string;
  modelProviderId?: string;
  modelId?: string;
  explicitSkillIds?: string[];
  enabled: boolean;
  nextRunAt?: string;
  lastRunAt?: string;
  lastStatus?: string;
  lastError?: string;
  lastSessionId?: string;
  failureStreak: number;
  createdAt: string;
  updatedAt: string;
}

export interface ScheduledJobRun {
  id: string;
  jobId: string;
  scheduledInstant: string;
  startedAt: string;
  finishedAt?: string;
  status: string;
  sessionId?: string;
  error?: string;
}

export interface ScheduleBlueprint {
  key: string;
  name: string;
  description: string;
  prompt: string;
  schedule: JobSchedule;
  scheduleDisplay: string;
  sessionPolicy: string;
  outputMode: string;
  needsTopic: boolean;
}

export interface CreateScheduledJobPayload {
  name: string;
  prompt: string;
  schedule: JobSchedule;
  knowledgeBaseIds: string[];
  outputMode?: string;
  inboxKnowledgeBaseId?: string;
  inboxFolder?: string;
  sessionPolicy?: string;
  modelProviderId?: string;
  modelId?: string;
  explicitSkillIds?: string[];
  enabled?: boolean;
}

export interface UpdateScheduledJobPayload {
  jobId: string;
  name?: string;
  prompt?: string;
  schedule?: JobSchedule;
  knowledgeBaseIds?: string[];
  outputMode?: string;
  inboxKnowledgeBaseId?: string;
  inboxFolder?: string;
  sessionPolicy?: string;
  modelProviderId?: string;
  modelId?: string;
  explicitSkillIds?: string[];
  enabled?: boolean;
}

export interface ScheduledJobUpdatedEvent {
  job: ScheduledJob;
  sessionId?: string;
}

/** 服务器连接状态，与 Rust Slot.state 对齐：disabled / connecting / ready / failed。 */
export type McpServerState = "disabled" | "connecting" | "ready" | "failed";

/** 传输类型，与 Rust McpServerView.transport 对齐。 */
export type McpTransportKind = "stdio" | "http";

/** 设置页看到的一台 MCP 服务器；密钥不在这里。 */
export interface McpServerView {
  name: string;
  enabled: boolean;
  transport: McpTransportKind | string;
  state: McpServerState | string;
  command: string;
  args: string[];
  url: string;
  cwd: string;
  timeoutSeconds: number;
  hasAuthorization: boolean;
  error: string | null;
  stderrTail: string;
  exposedToolCount: number;
  tools: McpToolView[];
}

/** 服务器上的一个工具，以及它现在会不会进入模型工具列表。 */
export interface McpToolView {
  rawName: string;
  publicName: string;
  description: string;
  readOnly: boolean;
  exposed: boolean;
}

/** 配置文件里被跳过的条目。 */
export interface McpConfigIssue {
  serverName: string;
  message: string;
}

/** 设置页一次读取的 MCP 概览。 */
export interface McpOverview {
  configPath: string;
  servers: McpServerView[];
  issues: McpConfigIssue[];
}

/**
 * 设置页保存一台服务器的草稿。
 * authorizationToken：undefined 保持原令牌，空字符串清除，其余文本覆盖。
 */
export interface McpServerDraft {
  name: string;
  enabled: boolean;
  transport: McpTransportKind;
  command: string;
  /** 每行一个参数。 */
  argsText: string;
  url: string;
  cwd: string;
  timeoutSeconds: number;
  /** 每行 KEY=VALUE，支持 ${ENV} 插值。 */
  envText: string;
  /** 每行 KEY=VALUE，仅 HTTP 传输使用。 */
  headersText: string;
  disabledTools: string[];
  authorizationToken?: string | null;
}

/** 测试连接的结果，不改变启用状态。 */
export interface McpProbeResult {
  serverName: string;
  ok: boolean;
  message: string;
  toolNames: string[];
}

/** 回合中等待用户确认的一次 MCP 调用。 */
export interface McpApprovalRequest {
  approvalId: string;
  sessionId: string;
  serverName: string;
  toolName: string;
  argumentsPreview: string;
}

/** 前端对确认弹窗的决定。 */
export interface McpApprovalDecision {
  approvalId: string;
  approved: boolean;
}

/** 浏览器功能设置；默认关闭，启用后需下载固定版本 Chromium。 */
export interface BrowserFeatureSettings {
  enabled: boolean;
}

/** 面板标签页视图。 */
export interface BrowserTabView {
  tabId: string;
  url: string;
  title: string;
  active: boolean;
  visible: boolean;
}

/** 控制租约状态：谁在操作浏览器。 */
export interface BrowserControlState {
  running: boolean;
  sessionId?: string | null;
  status: string;
  revision: number;
  elapsedMs: number;
}

/** 浏览器整体状态，`browser-state-changed` 事件与状态命令共用。 */
export interface BrowserState {
  available: boolean;
  enabled: boolean;
  chromiumInstalled: boolean;
  launchError?: string | null;
  tabs: BrowserTabView[];
  activeTabId?: string | null;
  visibleTabId?: string | null;
  control: BrowserControlState;
}

/** 面板实时画面帧；data 是 JPEG 的 base64。 */
export interface BrowserFrame {
  tabId: string;
  data: string;
  width: number;
  height: number;
}

/** 接管后的面板输入，坐标使用浏览器视口 CSS 像素。 */
export type BrowserUserInput =
  | { kind: "click"; x: number; y: number }
  | { kind: "scroll"; x: number; y: number; deltaX: number; deltaY: number }
  | { kind: "text"; text: string }
  | { kind: "key"; key: string; modifiers: number };

/** 面板活动提示：Agent 正在做什么。 */
export interface BrowserActivity {
  activity: string;
  tabId?: string | null;
}

/** 操作凭证：动作前保存的目标区域快照，只发面板。 */
export interface BrowserReceipt {
  receiptId: string;
  tabId: string;
  operation: string;
  x: number;
  y: number;
  url: string;
  createdAt: number;
}

/** 进阶级别下等待用户确认的浏览器操作请求。 */
export interface BrowserApprovalRequest {
  approvalId: string;
  sessionId: string;
  operation: string;
  tabId?: string | null;
  url?: string | null;
  elementSummary?: string | null;
}

/** 前端对浏览器操作确认弹窗的决定。 */
export interface BrowserApprovalDecision {
  approvalId: string;
  approved: boolean;
}

/** Chromium 安装状态，供设置页展示。 */
export interface ChromiumInstallStatus {
  installed: boolean;
  version?: string | null;
  downloading: boolean;
  phase: "idle" | "connecting" | "downloading" | "extracting" | "installed" | "failed";
  downloaded: number;
  total: number | null;
  error: string | null;
}
