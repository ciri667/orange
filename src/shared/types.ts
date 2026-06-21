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
  | "search_notes"
  | "read_note"
  | "list_tree"
  | "get_current_note"
  | "propose_note_change"
  | "create_note_draft"
  | "suggest_organization";

/** Agent 工具调用状态，用于前端展示本轮 loop 的执行轨迹。 */
export type AgentToolCallStatus = "planned" | "running" | "completed" | "failed";

/** 用户选择的本地 Markdown 知识库元信息。 */
export interface KnowledgeBase {
  id: string;
  name: string;
  path: string;
  description: string;
  status: KnowledgeBaseStatus;
  noteCount: number;
  updatedAt: string;
  isDefault: boolean;
  semanticIndexEnabled: boolean;
  scanReport?: ScanReport;
}

/** 单次知识库扫描报告，用于展示成功、失败、跳过目录和错误信息。 */
export interface ScanReport {
  scannedFileCount: number;
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
}

/** 本地目录树节点，用于把 Markdown 路径还原成文件夹和文件层级。 */
export interface FileTreeNode {
  id: string;
  name: string;
  path: string;
  type: "folder" | "file";
  noteId?: string;
  isRoot?: boolean;
  children: FileTreeNode[];
}

/** 工作台快照，是前端和 Tauri command 之间的首版数据传输对象。 */
export interface WorkspaceSnapshot {
  knowledgeBases: KnowledgeBase[];
  folders: FolderEntry[];
  notes: Note[];
  sessions: AgentSession[];
  activeKnowledgeBaseId: string;
  activeNoteId: string;
  activeSessionId: string;
}

/** Agent 单轮请求，运行时会在 loop 内自行选择是否调用工具。 */
export interface AgentTurnRequest {
  prompt: string;
  action: AgentActionType;
  sessionId: string;
  activeKnowledgeBaseId: string;
  activeNoteId: string;
}

/** Agent 单轮返回结果，包含更新后的完整工作台状态。 */
export interface AgentTurnResult {
  snapshot: WorkspaceSnapshot;
}

/** 知识库目录选择结果，Tauri 环境中来自系统目录选择器。 */
export interface KnowledgeBaseSelection {
  id: string;
  name: string;
  path: string;
  noteCount: number;
}
