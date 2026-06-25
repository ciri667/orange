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
  | "model_request"
  | "local_rule_agent"
  | "search_notes"
  | "read_note"
  | "list_tree"
  | "get_current_note"
  | "propose_note_change"
  | "create_note_draft"
  | "suggest_organization";

/** Agent 工具调用状态，用于前端展示本轮 loop 的执行轨迹。 */
export type AgentToolCallStatus = "planned" | "running" | "completed" | "failed";

/** 首版云端模型提供商，M3 先固定 OpenAI-compatible BYOK 协议。 */
export type ModelProvider = "openai-compatible";

/** 用户选择的隐私策略，决定模型请求是否允许携带本地笔记片段。 */
export type PrivacyPolicy = "local-only" | "allow-selected-scope";

/** 非 Markdown 文档类型，决定目录树操作权限和中间面板展示方式。 */
export type DocumentFileType = "txt" | "docx" | "pdf";

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

/** Skill 来源，内置和文件能力只能禁用，用户自建能力允许编辑和删除。 */
export type AgentSkillSource = "built-in" | "file" | "user";

/** Skill 默认触发模式，auto 允许 Runtime 根据输入轻量匹配。 */
export type AgentSkillActivationMode = "auto" | "manual";

/** Agent skill 是可启停、可显式选择、可自动匹配的指令型工作流。 */
export interface AgentSkill {
  id: string;
  name: string;
  displayName: string;
  description: string;
  instructions: string;
  tags: string[];
  triggers: string[];
  enabled: boolean;
  source: AgentSkillSource;
  allowAutoInvoke: boolean;
  createdAt: string;
  updatedAt: string;
  /** 文件式 skill 的 SKILL.md 绝对路径，内置和表单式用户 skill 为空。 */
  path?: string;
  /** 文件式 skill 相对用户 skills 根目录的路径，用于列表展示和排序。 */
  relativePath?: string;
  /** 文件式 skill 的解析元数据，首版只记录覆盖来源等轻量信息。 */
  metadata?: Record<string, string>;
}

/** Skill 全局设置，控制未显式选择时是否自动匹配。 */
export interface SkillSettings {
  activationMode: AgentSkillActivationMode;
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

/** 非 Markdown 文档的数据模型；txt 带正文，docx/pdf 只保存预览所需元数据。 */
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

/** 非 Markdown 文档预览命令返回值，pdf 返回 asset 路径，docx 返回结构化文本。 */
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
  citations?: Citation[];
  toolCalls?: AgentToolCall[];
}

/** Agent 对笔记提出的待确认变更，确认前不能修改本地 Markdown。 */
export interface ProposedChange {
  id: string;
  knowledgeBaseId: string;
  noteId?: string;
  type: "rewrite" | "create" | "organize";
  title: string;
  targetPath: string;
  original: string;
  next: string;
  originalHash: string;
  status: "pending" | "accepted" | "rejected";
}

/** Agent 会话是上下文容器，绑定知识库范围、笔记、消息和待确认写入。 */
export interface AgentSession {
  id: string;
  title: string;
  type: AgentSessionType;
  knowledgeBaseIds: string[];
  activeNoteId?: string;
  pinnedNoteIds: string[];
  messages: AgentMessage[];
  pendingChange?: ProposedChange;
  createdAt: string;
  updatedAt: string;
  /** 逻辑删除时间；有值的会话只保留在持久化记录中，不再进入普通会话列表。 */
  deletedAt?: string;
}

/** 云端模型配置只保存 key 引用，不在普通 SQLite payload 中保存明文密钥。 */
export interface ModelConfig {
  provider: ModelProvider;
  apiBase: string;
  model: string;
  keyReference: string;
  enabled: boolean;
}

/** 用户设置聚合模型、隐私和写入确认策略，供 M3 Runtime 读取。 */
export interface UserSettings {
  modelConfig: ModelConfig;
  privacyPolicy: PrivacyPolicy;
  writeConfirmationRequired: boolean;
  skillSettings: SkillSettings;
}

/** 模型密钥状态只说明是否可读取，不包含明文密钥。 */
export interface ModelApiKeyStatus {
  keyReference: string;
  configured: boolean;
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
  selectedSkillId?: string;
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
