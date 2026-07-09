import { useState } from "react";
import { PanelRightOpen } from "lucide-react";
import { AgentPanel } from "../agent/AgentPanel";
import { DocumentPane } from "../editor/DocumentPane";
import { EditorPane } from "../editor/EditorPane";
import { buildFileTree } from "../knowledge-base/treeUtils";
import { KnowledgeBaseSidebar } from "../knowledge-base/KnowledgeBaseSidebar";
import { SettingsDrawer } from "../settings/SettingsDrawer";
import { ConfirmDialog, type ConfirmDialogConfig } from "../shared/ConfirmDialog";
import { buildMarkdownDiff } from "../diff/markdownDiff";
import { createContentHash, createLocalId, formatLocalDateTime } from "../shared/id";
import { logError, logInfo, logWarn } from "../shared/logger";
import { decodeModelSelection } from "../shared/modelSelection";
import {
  getActiveKnowledgeBase,
  getActiveDocument,
  getActiveNote,
} from "../shared/selectors";
import {
  acceptProposedChange,
  attachKnowledgeBase,
  compactAgentContext,
  createDocument,
  createFolder,
  createNote,
  deleteDocument,
  deleteNote,
  exportCurrentFile,
  deleteSession,
  loadAppEventLogs,
  loadRequestAuditLogs,
  removeKnowledgeBase,
  renameDocument,
  renameNote,
  rejectProposedChange,
  rescanKnowledgeBase,
  restoreSessionContext,
  runAgentTurn,
  saveDocumentContent,
  saveNoteContent,
  saveNoteImageAttachments,
  saveSession,
  selectKnowledgeBaseDirectory,
  updateSessionScope,
} from "../shared/tauriApi";
import type {
  AgentActionType,
  AgentMessage,
  AgentSession,
  DocumentHistoryTargetKind,
  ExportFormat,
  KnowledgeBase,
  MarkdownViewMode,
  NoteImageAttachmentInput,
  ProposedChange,
  ReviewComment,
  WorkspaceSnapshot,
} from "../shared/types";
import { DocumentHistoryDialog } from "./DocumentHistoryDialog";
import type { ReviewCommentDraft } from "../diff/DiffPanel";
import { TopBar } from "./TopBar";
import { useAgentTurnDraft } from "./useAgentTurnDraft";
import { useDocumentPreview } from "./useDocumentPreview";
import { useReviewChangeLogger } from "./useReviewChangeLogger";
import { useResizableWorkspaceLayout } from "./useResizableWorkspaceLayout";
import { useWorkspaceBootData } from "./useWorkspaceBootData";
import { useWorkspaceDrafts } from "./useWorkspaceDrafts";
import { useWorkspaceSettingsActions } from "./useWorkspaceSettingsActions";

/** 将未知异常统一转换为可展示文案，避免启动错误页渲染空对象。 */
function formatErrorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

/** 等待用户确认的工作台操作，确认后才执行真实文件或会话变更。 */
interface PendingConfirmation extends ConfirmDialogConfig {
  onConfirm: () => Promise<void> | void;
}

/** 前端单张图片预检查上限，和 Rust 存储层限制保持一致。 */
const MAX_PASTE_IMAGE_BYTES = 20 * 1024 * 1024;

/** 前端单次粘贴总大小预检查上限，减少无意义 base64 读取和 IPC 成本。 */
const MAX_PASTE_IMAGE_BATCH_BYTES = 50 * 1024 * 1024;

/** 空白会话的默认标题；首条用户输入提交后会替换为用户原始输入。 */
const DEFAULT_SESSION_TITLE = "新会话";

/** 未持久化的占位会话 ID，只用于没有当前知识库会话时驱动侧栏展示。 */
const DRAFT_SESSION_ID = "__draft-session__";

/** 将图片文件读成 base64 主体；调用方负责限制大小和记录脱敏日志。 */
function readImageFileAsBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();

    reader.onerror = () => reject(new Error("无法读取剪贴板图片。"));
    reader.onload = () => {
      const result = typeof reader.result === "string" ? reader.result : "";
      const dataSeparatorIndex = result.indexOf(",");

      resolve(dataSeparatorIndex >= 0 ? result.slice(dataSeparatorIndex + 1) : result);
    };
    reader.readAsDataURL(file);
  });
}

/** 将 Markdown 图片片段插入用户粘贴时的选区，保持 textarea 原有编辑语义。 */
function insertMarkdownAtSelection(content: string, insertion: string, selectionStart: number, selectionEnd: number) {
  const start = clampTextIndex(selectionStart, content.length);
  const end = clampTextIndex(selectionEnd, content.length);
  const normalizedStart = Math.min(start, end);
  const normalizedEnd = Math.max(start, end);

  return `${content.slice(0, normalizedStart)}${insertion}${content.slice(normalizedEnd)}`;
}

/** 把 textarea selection 下标收敛到正文长度范围内，防止异步粘贴期间选区失效。 */
function clampTextIndex(index: number, length: number) {
  if (!Number.isFinite(index)) {
    return length;
  }

  return Math.max(0, Math.min(index, length));
}

/** 生成前端日志中的图片类型摘要，不记录文件名、路径或二进制内容。 */
function summarizeImageMimeTypes(files: File[]) {
  return Array.from(new Set(files.map((file) => file.type || "unknown")))
    .sort()
    .join(",");
}

/** 新建 Agent 会话对象，作为消息、检索范围和待确认 diff 的容器。 */
function buildAgentSession({
  knowledgeBase,
  title = DEFAULT_SESSION_TITLE,
  knowledgeBaseIds,
}: {
  knowledgeBase: KnowledgeBase;
  title?: string;
  knowledgeBaseIds?: string[];
}): AgentSession {
  /** 会话创建时间需要长期可辨认，避免历史列表里多个“刚刚”无法区分。 */
  const createdAt = formatLocalDateTime();

  return {
    id: createLocalId("session-knowledge-base"),
    title,
    type: "knowledge-base",
    knowledgeBaseIds: knowledgeBaseIds?.length ? knowledgeBaseIds : [knowledgeBase.id],
    pinnedNoteIds: [],
    messages: [],
    createdAt,
    updatedAt: createdAt,
  };
}

/** 构造未落库的侧栏占位会话，避免仅切换文档时隐式创建真实会话。 */
function buildDraftAgentSession(knowledgeBase: KnowledgeBase): AgentSession {
  return {
    id: DRAFT_SESSION_ID,
    title: DEFAULT_SESSION_TITLE,
    type: "knowledge-base",
    knowledgeBaseIds: [knowledgeBase.id],
    pinnedNoteIds: [],
    messages: [],
    createdAt: "未保存",
    updatedAt: "未保存",
  };
}

/** 从当前知识库解析应展示的会话；只复用已有会话，不创建新的历史记录。 */
function resolveKnowledgeBaseSessionId(snapshot: WorkspaceSnapshot, knowledgeBaseId: string) {
  const activeSession = snapshot.sessions.find((session) => session.id === snapshot.activeSessionId);

  if (activeSession?.knowledgeBaseIds.includes(knowledgeBaseId)) {
    return activeSession.id;
  }

  return snapshot.sessions.find((session) => session.knowledgeBaseIds.includes(knowledgeBaseId))?.id ?? "";
}

/** 获取当前知识库下可用的真实会话；没有时返回 undefined，由 UI 使用草稿会话展示。 */
function resolveActiveSessionForKnowledgeBase(snapshot: WorkspaceSnapshot, knowledgeBase: KnowledgeBase) {
  const sessionId = resolveKnowledgeBaseSessionId(snapshot, knowledgeBase.id);

  return snapshot.sessions.find((session) => session.id === sessionId);
}

/** 判断会话是否已经持久化在当前快照中，草稿会话不能直接提交给后端 Agent。 */
function isPersistedSession(snapshot: WorkspaceSnapshot, session: AgentSession) {
  return snapshot.sessions.some((item) => item.id === session.id);
}

/** 首条用户消息会成为会话标题；空输入不会触发提交，保留默认“新会话”。 */
function buildTitleFromFirstPrompt(prompt: string) {
  return prompt.trim() || DEFAULT_SESSION_TITLE;
}

/** 仅在空白新会话第一次发送消息前允许用用户输入替换标题。 */
function shouldUseFirstPromptAsTitle(session: AgentSession) {
  return session.title === DEFAULT_SESSION_TITLE && !session.messages.some((message) => message.role === "user");
}

/** 返回替换标题后的快照和会话对象，避免在运行 Agent 前丢失用户首条输入标题。 */
function applyFirstPromptTitle(snapshot: WorkspaceSnapshot, session: AgentSession, prompt: string) {
  const nextSession = {
    ...session,
    title: buildTitleFromFirstPrompt(prompt),
    updatedAt: formatLocalDateTime(),
  };

  return {
    snapshot: {
      ...snapshot,
      activeSessionId: nextSession.id,
      sessions: snapshot.sessions.map((item) => (item.id === nextSession.id ? nextSession : item)),
    },
    session: nextSession,
  };
}

/** 构造发送后立即展示的用户消息，后端会通过同一 ID 复用并持久化本轮记录。 */
function buildOptimisticUserMessage(prompt: string, action: AgentActionType): AgentMessage {
  return {
    id: createLocalId("user"),
    role: "user",
    content: prompt,
    action,
  };
}

/** 把用户消息追加进目标会话，确保 Agent 响应前对话框已经显示用户输入。 */
function appendUserMessageToSession(
  snapshot: WorkspaceSnapshot,
  session: AgentSession,
  message: AgentMessage,
) {
  const nextSession = {
    ...session,
    messages: [...session.messages, message],
    updatedAt: formatLocalDateTime(),
  };

  return {
    snapshot: {
      ...snapshot,
      activeSessionId: nextSession.id,
      sessions: snapshot.sessions.map((item) => (item.id === nextSession.id ? nextSession : item)),
    },
    session: nextSession,
  };
}

/** 创建审阅状态摘要，避免各入口重复计算评论数量。 */
function buildReviewState(comments: ReviewComment[], selected?: ReviewComment) {
  return {
    selectedCommentId: selected?.id,
    selectedLineSide: selected?.lineSide,
    selectedLineNumber: selected?.lineNumber,
    commentCount: comments.length,
    submittedCommentCount: comments.filter((comment) => comment.status === "submitted").length,
    updatedAt: formatLocalDateTime(),
  };
}

/** 更新当前会话的 pending diff；调用方负责决定是否持久化。 */
function updateActivePendingChange(snapshot: WorkspaceSnapshot, nextChange: ProposedChange) {
  return {
    ...snapshot,
    sessions: snapshot.sessions.map((session) =>
      session.id === snapshot.activeSessionId
        ? {
            ...session,
            pendingChange: nextChange,
            updatedAt: formatLocalDateTime(),
          }
        : session,
    ),
  };
}

/** 生成发送给 Agent 的审阅反馈消息，包含行号和评论正文，但不进入诊断日志。 */
function buildReviewFeedbackPrompt(change: ProposedChange, comments: ReviewComment[]) {
  const lines = comments.map((comment, index) => {
    const sideLabel = comment.lineSide === "next" ? "建议内容" : "原文";

    return `${index + 1}. ${sideLabel} L${comment.lineNumber}: ${comment.body}`;
  });

  return [
    `请根据我对「${change.title}」的逐行审阅反馈，重新生成待确认 diff。`,
    `目标路径：${change.targetPath}`,
    "审阅反馈：",
    ...lines,
    "保持未被评论的合理改动，仍然只生成待确认 diff，不要直接写入文件。",
  ].join("\n");
}

/** 正式工作台根组件，集中编排知识库、编辑器、Agent loop 和设置状态。 */
export function WorkspaceShell() {
  /** Agent turn 草稿 hook 维护输入框、本轮 Provider 和显式 Skill 状态。 */
  const {
    agentPrompt,
    setAgentPrompt,
    turnModelSelection,
    setTurnModelSelection,
    explicitSkillIds,
    setExplicitSkillIds,
    resetTurnSelection,
  } = useAgentTurnDraft();
  /** 左侧目录搜索词，只影响当前前端文件树过滤，不写入持久化。 */
  const [searchTerm, setSearchTerm] = useState("");
  /** 目录树折叠状态由前端维护，切换知识库、重扫或恢复会话时重置。 */
  const [collapsedFolderPaths, setCollapsedFolderPaths] = useState<Set<string>>(new Set());
  /** 会话历史浮层开关，和上下文、scope 浮层互斥。 */
  const [isSessionListOpen, setIsSessionListOpen] = useState(false);
  /** 当前会话上下文浮层开关，避免长消息列表挤占主输入区。 */
  const [isSessionContextOpen, setIsSessionContextOpen] = useState(false);
  /** 会话工具范围选择器开关，用于多知识库 scope 管理。 */
  const [isScopeSelectorOpen, setIsScopeSelectorOpen] = useState(false);
  /** 桌面端手动折叠 Agent 协作区，窄窗口断点仍由 CSS 自动接管。 */
  const [isAgentPanelCollapsed, setIsAgentPanelCollapsed] = useState(false);
  /** 设置抽屉打开状态，打开时会刷新非阻塞诊断日志。 */
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  /** 全局忙碌状态覆盖文件、会话、设置和日志刷新操作。 */
  const [isBusy, setIsBusy] = useState(false);
  /** 忙碌状态文案只展示当前操作类型，不包含路径、密钥或请求内容。 */
  const [busyLabel, setBusyLabel] = useState("");
  /** 顶部/侧栏轻量通知，展示用户操作结果和可恢复错误。 */
  const [notice, setNotice] = useState("");
  /** 编辑草稿 hook 维护 dirty 集合和保存基准 hash，文件写入仍由原有 Tauri API 执行。 */
  const {
    editingBaseHashes,
    editingBaseDocumentHashes,
    dirtyNoteIds,
    setDirtyNoteIds,
    dirtyDocumentIds,
    setDirtyDocumentIds,
    initializeDraftBaselines,
    commitDraftSnapshot,
  } = useWorkspaceDrafts();
  /** Markdown 编辑区视图模式，保持编辑/预览切换不影响文件内容。 */
  const [markdownViewMode, setMarkdownViewMode] = useState<MarkdownViewMode>("edit");
  /** 当前打开的文档历史弹窗目标；只允许 Markdown 和 TXT。 */
  const [historyDialog, setHistoryDialog] = useState<{ targetKind: DocumentHistoryTargetKind; targetId: string } | null>(null);
  /** 文件重命名弹窗草稿，同时支持 Markdown 和可编辑 TXT。 */
  const [renameDialog, setRenameDialog] = useState<{ kind: "note" | "document"; id: string; fileName: string } | null>(null);
  /** 目录树新建弹窗草稿，创建位置由被点击的父目录决定。 */
  const [createDialog, setCreateDialog] = useState<{
    kind: "markdown" | "text" | "folder";
    knowledgeBaseId: string;
    parentPath: string;
    name: string;
  } | null>(null);
  /** 待确认的危险操作，使用应用内弹窗替代 window.confirm，避免 Tauri dialog 权限依赖。 */
  const [pendingConfirmation, setPendingConfirmation] = useState<PendingConfirmation | null>(null);
  /** 主工作台三栏布局偏好，负责拖拽分隔条、键盘调整和本机持久化。 */
  const { workspaceRef, gridTemplateColumns, resizingPane, getSeparatorProps } = useResizableWorkspaceLayout();
  /** 启动和诊断数据 hook 返回工作台全局状态及刷新入口，根组件继续负责业务动作。 */
  const {
    snapshot,
    setSnapshot,
    userSettings,
    setUserSettings,
    imSettings,
    setImSettings,
    agentSkills,
    setAgentSkills,
    modelApiKeyStatuses,
    setModelApiKeyStatuses,
    feishuCredentialStatus,
    setFeishuCredentialStatus,
    feishuGatewayStatus,
    setFeishuGatewayStatus,
    providerTemplates,
    isBooting,
    bootError,
    knowledgeBaseMemories,
    setKnowledgeBaseMemories,
    auditLogs,
    setAuditLogs,
    appEventLogs,
    setAppEventLogs,
    loadInitialData,
    loadInitialDiagnosticLogs,
  } = useWorkspaceBootData({
    onSnapshotInitialized: initializeDraftBaselines,
    onNoticeChange: setNotice,
  });
  useReviewChangeLogger(snapshot);
  /** 只读文档预览 hook 负责异步加载和错误状态，TXT 仍由可编辑正文面板处理。 */
  const { documentPreview, documentPreviewError, isDocumentPreviewLoading } = useDocumentPreview(snapshot);
  /** 设置动作 hook 统一处理保存、凭证、Skills 和诊断日志刷新，复用原有 Tauri API。 */
  const {
    handleSaveSettings,
    handleSaveImSettings,
    handleSaveKnowledgeBaseMemory,
    handleDeleteKnowledgeBaseMemory,
    handleSaveFeishuSecret,
    handleStartFeishuGateway,
    handleStopFeishuGateway,
    handleRefreshFeishuStatus,
    handleSaveSkill,
    handleInstallSkill,
    handleToggleSkill,
    handleDeleteSkill,
    handleOpenUserSkillsFolder,
    handleSaveApiKey,
    handleRefreshProviderModels,
    handleRefreshAuditLogs,
    handleRefreshAppEventLogs,
    handleClearAppEventLogs,
    handleOpenAppLogFolder,
  } = useWorkspaceSettingsActions({
    beginBusy,
    endBusy,
    setNotice,
    imSettings,
    feishuCredentialStatus,
    feishuGatewayStatus,
    setUserSettings,
    setImSettings,
    setAgentSkills,
    setModelApiKeyStatuses,
    setFeishuCredentialStatus,
    setFeishuGatewayStatus,
    setKnowledgeBaseMemories,
    setAuditLogs,
    setAppEventLogs,
  });

  if (isBooting) {
    return (
      <main className="loading-shell">
        <div className="brand-mark">
          <img className="brand-logo" src="/orange-logo.svg" alt="" />
        </div>
        <p>正在加载本地知识库工作台...</p>
      </main>
    );
  }

  if (!snapshot || !userSettings || !imSettings) {
    const errorMessage = bootError || "工作台初始化未完成，请重试。";

    return (
      <main className="loading-shell boot-error-shell">
        <div className="brand-mark">
          <img className="brand-logo" src="/orange-logo.svg" alt="" />
        </div>
        <p>本地知识库工作台加载失败</p>
        <p className="boot-error-message">{errorMessage}</p>
        <button className="primary-button compact" type="button" onClick={() => void loadInitialData()}>
          重试
        </button>
      </main>
    );
  }

  /** 已加载的工作台快照，供事件闭包使用，避免 nullable state 进入业务逻辑。 */
  const currentSnapshot = snapshot;

  if (!currentSnapshot.knowledgeBases.length) {
    return (
      <main className="empty-shell">
        <div className="brand-mark">
          <img className="brand-logo" src="/orange-logo.svg" alt="" />
        </div>
        <h1>连接一个支持文档目录，开始使用知识库 Agent 助手。</h1>
        <p>目录树会展示 Markdown、TXT、DOCX、PDF 和图片；Agent 写入仍只作用于确认后的 Markdown diff。</p>
        {(busyLabel || notice) && (
          <p className={`operation-notice ${notice.includes("失败") || notice.includes("阻止") ? "error" : ""}`}>
            {busyLabel || notice}
          </p>
        )}
        <button className="primary-button" type="button" onClick={handleAddKnowledgeBase} disabled={isBusy}>
          添加第一个知识库
        </button>
      </main>
    );
  }

  const activeKnowledgeBase = getActiveKnowledgeBase(currentSnapshot);
  const persistedActiveSession = resolveActiveSessionForKnowledgeBase(currentSnapshot, activeKnowledgeBase);
  const activeSession = persistedActiveSession ?? buildDraftAgentSession(activeKnowledgeBase);
  const activeDocument = getActiveDocument(currentSnapshot);
  const activeNote = getActiveNote(currentSnapshot);
  const isActiveNoteDirty = activeNote ? dirtyNoteIds.has(activeNote.id) : false;
  const isActiveDocumentDirty = activeDocument ? dirtyDocumentIds.has(activeDocument.id) : false;
  const fileTree = buildFileTree({
    knowledgeBase: activeKnowledgeBase,
    folders: currentSnapshot.folders,
    notes: currentSnapshot.notes,
    documents: currentSnapshot.documents,
    searchTerm,
  });
  const historyNote = historyDialog?.targetKind === "note"
    ? currentSnapshot.notes.find((note) => note.id === historyDialog.targetId)
    : undefined;
  const historyDocument = historyDialog?.targetKind === "document"
    ? currentSnapshot.documents.find((document) => document.id === historyDialog.targetId && document.fileType === "txt")
    : undefined;
  const historyTarget = historyNote
    ? {
        targetKind: "note" as const,
        targetId: historyNote.id,
        title: historyNote.title,
        content: historyNote.content,
        contentHash: historyNote.contentHash,
        isDirty: dirtyNoteIds.has(historyNote.id),
      }
    : historyDocument
      ? {
          targetKind: "document" as const,
          targetId: historyDocument.id,
          title: historyDocument.title,
          content: historyDocument.content ?? "",
          contentHash: historyDocument.contentHash,
          isDirty: dirtyDocumentIds.has(historyDocument.id),
        }
      : null;

  /** 统一进入忙碌状态，附带可展示的操作说明。 */
  function beginBusy(label: string) {
    setIsBusy(true);
    setBusyLabel(label);
    setNotice("");
  }

  /** 统一结束忙碌状态，避免扫描或保存结束后残留旧提示。 */
  function endBusy() {
    setIsBusy(false);
    setBusyLabel("");
  }

  /** 打开应用内确认弹窗，调用方只在用户确认后执行真实副作用。 */
  function requestConfirmation(config: ConfirmDialogConfig, onConfirm: () => Promise<void> | void) {
    setPendingConfirmation({
      cancelLabel: "取消",
      tone: "danger",
      ...config,
      onConfirm,
    });
  }

  /** 执行确认动作并关闭弹窗；业务错误仍由原动作内部写入 notice。 */
  async function handleConfirmDialogConfirm() {
    const confirmation = pendingConfirmation;

    if (!confirmation) {
      return;
    }

    setPendingConfirmation(null);
    await confirmation.onConfirm();
  }

  /** 写入新快照时同步保存基准 hash，清理已经不存在的草稿标记。 */
  function commitSnapshot(
    nextSnapshot: WorkspaceSnapshot,
    dirtyNotesToKeep = dirtyNoteIds,
    dirtyDocumentsToKeep = dirtyDocumentIds,
  ) {
    setSnapshot(nextSnapshot);
    commitDraftSnapshot(nextSnapshot, dirtyNotesToKeep, dirtyDocumentsToKeep);
  }

  /** 选择知识库时只切换浏览焦点；会话最多切到该知识库已有会话，不再隐式创建。 */
  async function handleSelectKnowledgeBase(knowledgeBaseId: string) {
    const nextKnowledgeBase = currentSnapshot.knowledgeBases.find((knowledgeBase) => knowledgeBase.id === knowledgeBaseId);
    const nextNotes = currentSnapshot.notes.filter((note) => note.knowledgeBaseId === knowledgeBaseId);
    const nextDocuments = currentSnapshot.documents.filter((document) => document.knowledgeBaseId === knowledgeBaseId);

    if (!nextKnowledgeBase) {
      return;
    }

    const nextActiveSessionId = resolveKnowledgeBaseSessionId(currentSnapshot, nextKnowledgeBase.id);
    const activatedSnapshot = {
      ...currentSnapshot,
      activeKnowledgeBaseId: knowledgeBaseId,
      activeNoteId: nextNotes[0]?.id ?? "",
      activeDocumentId: nextNotes[0] ? "" : nextDocuments[0]?.id ?? "",
      activeSessionId: nextActiveSessionId,
    };

    logInfo("切换知识库浏览焦点。", {
      category: "frontend",
      event: "select_knowledge_base",
      status: "completed",
      metadata: {
        hasExistingSession: Boolean(nextActiveSessionId),
        noteCount: nextNotes.length,
        documentCount: nextDocuments.length,
      },
    });
    commitSnapshot(activatedSnapshot);
    setSearchTerm("");
    setCollapsedFolderPaths(new Set());
  }

  /** 添加知识库时走 Tauri 目录选择器，浏览器开发态使用 mock 目录。 */
  async function handleAddKnowledgeBase() {
    beginBusy("正在选择并扫描知识库...");

    try {
      const selection = await selectKnowledgeBaseDirectory(currentSnapshot.knowledgeBases.length);
      setNotice(`正在扫描「${selection.name}」中的支持文档...`);
      const nextSnapshot = await attachKnowledgeBase(currentSnapshot, selection);
      commitSnapshot(nextSnapshot);
      setSearchTerm("");
      setCollapsedFolderPaths(new Set());
      setNotice(buildScanNotice(nextSnapshot, selection.id));
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 展开或折叠文件夹节点，模拟本地文件管理器的目录树操作。 */
  function handleToggleFolder(folderPath: string) {
    setCollapsedFolderPaths((currentFolderPaths) => {
      const nextFolderPaths = new Set(currentFolderPaths);

      // 同一个文件夹再次点击时恢复展开，其他文件夹状态不受影响。
      if (nextFolderPaths.has(folderPath)) {
        nextFolderPaths.delete(folderPath);
      } else {
        nextFolderPaths.add(folderPath);
      }

      return nextFolderPaths;
    });
  }

  /** 确保指定目录路径处于展开状态，让新建结果立即可见。 */
  function expandFolderPaths(folderPaths: string[]) {
    setCollapsedFolderPaths((currentFolderPaths) => {
      const nextFolderPaths = new Set(currentFolderPaths);

      folderPaths.forEach((folderPath) => {
        nextFolderPaths.delete(folderPath);
      });

      return nextFolderPaths;
    });
  }

  /** 打开 Markdown 文件只切换编辑器焦点；会话保持知识库级别，不再跟随文档切换。 */
  async function handleSelectNote(noteId: string) {
    const nextNote = currentSnapshot.notes.find((note) => note.id === noteId);

    if (!nextNote) {
      return;
    }

    const nextKnowledgeBase =
      currentSnapshot.knowledgeBases.find((knowledgeBase) => knowledgeBase.id === nextNote.knowledgeBaseId) ?? activeKnowledgeBase;
    const nextActiveSessionId = resolveKnowledgeBaseSessionId(currentSnapshot, nextKnowledgeBase.id);
    const activatedSnapshot = {
      ...currentSnapshot,
      activeKnowledgeBaseId: nextKnowledgeBase.id,
      activeNoteId: noteId,
      activeDocumentId: "",
      activeSessionId: nextActiveSessionId,
    };

    logInfo("切换 Markdown 浏览焦点。", {
      category: "frontend",
      event: "select_note",
      status: "completed",
      metadata: { hasExistingSession: Boolean(nextActiveSessionId) },
    });
    commitSnapshot(activatedSnapshot);
  }

  /** 打开普通文档只切换编辑器焦点；没有同库会话时也不创建默认会话。 */
  async function handleSelectDocument(documentId: string) {
    const nextDocument = currentSnapshot.documents.find((document) => document.id === documentId);

    if (!nextDocument) {
      return;
    }

    const nextKnowledgeBase =
      currentSnapshot.knowledgeBases.find((knowledgeBase) => knowledgeBase.id === nextDocument.knowledgeBaseId) ??
      activeKnowledgeBase;
    const nextActiveSessionId = resolveKnowledgeBaseSessionId(currentSnapshot, nextKnowledgeBase.id);
    const activatedSnapshot = {
      ...currentSnapshot,
      activeKnowledgeBaseId: nextKnowledgeBase.id,
      activeNoteId: "",
      activeDocumentId: documentId,
      activeSessionId: nextActiveSessionId,
    };

    logInfo("切换普通文档浏览焦点。", {
      category: "frontend",
      event: "select_document",
      status: "completed",
      metadata: { hasExistingSession: Boolean(nextActiveSessionId) },
    });
    commitSnapshot(activatedSnapshot);
  }

  /** 更新当前笔记正文，只修改内存草稿；保存时才写回本地 Markdown 文件。 */
  function handleContentChange(content: string) {
    if (!activeNote) {
      return;
    }

    setSnapshot({
      ...currentSnapshot,
      notes: currentSnapshot.notes.map((note) =>
        note.id === activeNote.id ? { ...note, content, updatedAt: "刚刚", contentHash: createContentHash(content) } : note,
      ),
    });
    setDirtyNoteIds((currentNoteIds) => new Set(currentNoteIds).add(activeNote.id));
  }

  /** 保存粘贴图片附件并把后端返回的标准 Markdown 图片语法插入当前草稿。 */
  async function handlePasteImages(files: File[], selectionStart: number, selectionEnd: number) {
    if (!activeNote || !files.length) {
      return;
    }

    if (isBusy) {
      setNotice("当前操作进行中，请稍后再粘贴图片。");
      return;
    }

    const startedAt = performance.now();
    const totalBytes = files.reduce((sum, file) => sum + file.size, 0);
    const mimeTypes = summarizeImageMimeTypes(files);
    const logMetadata = {
      imageCount: files.length,
      totalBytes,
      mimeTypes,
    };

    logInfo("开始处理粘贴图片。", {
      category: "frontend",
      event: "paste_image",
      status: "started",
      metadata: logMetadata,
    });

    if (files.some((file) => file.size > MAX_PASTE_IMAGE_BYTES)) {
      const message = "单张图片超过 20MB，已阻止保存。";

      logWarn("粘贴图片超过单张大小限制。", {
        category: "frontend",
        event: "paste_image",
        status: "blocked",
        metadata: { ...logMetadata, reason: "single_limit" },
      });
      setNotice(message);
      return;
    }

    if (totalBytes > MAX_PASTE_IMAGE_BATCH_BYTES) {
      const message = "单次粘贴图片总大小超过 50MB，已阻止保存。";

      logWarn("粘贴图片超过批量大小限制。", {
        category: "frontend",
        event: "paste_image",
        status: "blocked",
        metadata: { ...logMetadata, reason: "batch_limit" },
      });
      setNotice(message);
      return;
    }

    beginBusy("正在保存粘贴图片...");

    try {
      // todo: 后续补充图片压缩、EXIF 清理、附件管理和孤立附件清理；首版保持原图本地落盘。
      const imageInputs: NoteImageAttachmentInput[] = await Promise.all(
        files.map(async (file) => ({
          mimeType: file.type,
          bytesBase64: await readImageFileAsBase64(file),
        })),
      );
      const savedAttachments = await saveNoteImageAttachments(currentSnapshot, activeNote.id, imageInputs);
      const markdownInsertion = savedAttachments.map((attachment) => attachment.markdown).join("\n");
      const nextContent = insertMarkdownAtSelection(activeNote.content, markdownInsertion, selectionStart, selectionEnd);

      handleContentChange(nextContent);
      setNotice(`已保存 ${savedAttachments.length} 张图片，正文仍需保存草稿。`);
      logInfo("粘贴图片处理完成。", {
        category: "frontend",
        event: "paste_image",
        status: "completed",
        durationMs: performance.now() - startedAt,
        metadata: {
          ...logMetadata,
          savedCount: savedAttachments.length,
        },
      });
    } catch (error) {
      setNotice(formatErrorMessage(error));
      logError("粘贴图片处理失败。", {
        category: "frontend",
        event: "paste_image",
        status: "failed",
        durationMs: performance.now() - startedAt,
        error,
        metadata: logMetadata,
      });
    } finally {
      endBusy();
    }
  }

  /** 更新当前 txt 文档正文，只修改内存草稿；保存时才写回本地文件。 */
  function handleDocumentContentChange(content: string) {
    if (!activeDocument || activeDocument.fileType !== "txt") {
      return;
    }

    setSnapshot({
      ...currentSnapshot,
      documents: currentSnapshot.documents.map((document) =>
        document.id === activeDocument.id
          ? { ...document, content, contentHash: createContentHash(content) }
          : document,
      ),
    });
    setDirtyDocumentIds((currentDocumentIds) => new Set(currentDocumentIds).add(activeDocument.id));
  }

  /** 打开 Markdown 历史记录弹窗；允许存在草稿，但恢复动作会在弹窗内禁用。 */
  function openNoteHistory(noteId = activeNote?.id ?? "") {
    const note = currentSnapshot.notes.find((item) => item.id === noteId);

    if (!note) {
      return;
    }

    setHistoryDialog({ targetKind: "note", targetId: note.id });
  }

  /** 打开 TXT 历史记录弹窗；DOCX/PDF/图片不暴露该入口。 */
  function openDocumentHistory(documentId = activeDocument?.id ?? "") {
    const document = currentSnapshot.documents.find((item) => item.id === documentId);

    if (!document || document.fileType !== "txt") {
      return;
    }

    setHistoryDialog({ targetKind: "document", targetId: document.id });
  }

  /** 应用历史回档返回的新快照，并清理该文件的草稿状态。 */
  function handleHistoryRestored(nextSnapshot: WorkspaceSnapshot) {
    const nextDirtyNoteIds = new Set(dirtyNoteIds);
    const nextDirtyDocumentIds = new Set(dirtyDocumentIds);

    if (historyDialog?.targetKind === "note") {
      nextDirtyNoteIds.delete(historyDialog.targetId);
    } else if (historyDialog?.targetKind === "document") {
      nextDirtyDocumentIds.delete(historyDialog.targetId);
    }

    commitSnapshot(nextSnapshot, nextDirtyNoteIds, nextDirtyDocumentIds);
  }

  /** 打开目录树新建弹窗；创建位置完全由被点击的目录节点决定。 */
  function openCreateDialog(kind: "markdown" | "text" | "folder", parentPath: string) {
    const defaultName =
      kind === "markdown"
        ? getNextAvailableMarkdownName(currentSnapshot, activeKnowledgeBase.id, parentPath)
        : kind === "text"
          ? getNextAvailableTextDocumentName(currentSnapshot, activeKnowledgeBase.id, parentPath)
          : getNextAvailableFolderName(currentSnapshot, activeKnowledgeBase.id, parentPath);

    setRenameDialog(null);
    setCreateDialog({
      kind,
      knowledgeBaseId: activeKnowledgeBase.id,
      parentPath,
      name: defaultName,
    });
  }

  /** 提交目录树新建弹窗，文档创建后自动打开，目录创建后保持当前文档不变。 */
  async function handleSubmitCreate() {
    if (!createDialog) {
      return;
    }

    const nextName = createDialog.name.trim();

    if (!nextName) {
      return;
    }

    beginBusy(
      createDialog.kind === "markdown"
        ? "正在新建 Markdown..."
        : createDialog.kind === "text"
          ? "正在新建 TXT..."
          : "正在新建目录...",
    );

    try {
      if (createDialog.kind === "markdown") {
        const nextSnapshot = await createNote(
          currentSnapshot,
          createDialog.knowledgeBaseId,
          createDialog.parentPath,
          nextName,
        );
        const nextNote = getActiveNote(nextSnapshot);
        const nextKnowledgeBase = getActiveKnowledgeBase(nextSnapshot);
        const activatedSnapshot = nextNote
          ? {
              ...nextSnapshot,
              activeKnowledgeBaseId: nextKnowledgeBase.id,
              activeNoteId: nextNote.id,
              activeDocumentId: "",
              activeSessionId: resolveKnowledgeBaseSessionId(nextSnapshot, nextKnowledgeBase.id),
            }
          : nextSnapshot;

        commitSnapshot(activatedSnapshot);
        setSearchTerm("");
        expandFolderPaths([createDialog.parentPath]);
        setMarkdownViewMode("edit");
        setCreateDialog(null);
        setNotice(nextNote ? `已新建「${nextNote.title}」。` : "已新建 Markdown。");
        return;
      }

      if (createDialog.kind === "text") {
        const nextSnapshot = await createDocument(
          currentSnapshot,
          createDialog.knowledgeBaseId,
          createDialog.parentPath,
          nextName,
        );
        const nextDocument = getActiveDocument(nextSnapshot);

        commitSnapshot(nextSnapshot);
        setSearchTerm("");
        expandFolderPaths([createDialog.parentPath]);
        setCreateDialog(null);
        setNotice(nextDocument ? `已新建「${nextDocument.title}」。` : "已新建 TXT。");
        return;
      }

      const nextSnapshot = await createFolder(
        currentSnapshot,
        createDialog.knowledgeBaseId,
        createDialog.parentPath,
        nextName,
      );
      const createdFolderPath = joinRelativePath(createDialog.parentPath, nextName);

      commitSnapshot(nextSnapshot);
      setSearchTerm("");
      expandFolderPaths([createDialog.parentPath, createdFolderPath]);
      setCreateDialog(null);
      setNotice(`已新建目录「${nextName}」。`);
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 保存当前笔记草稿，后端会用开始编辑时的 hash 检测外部编辑器冲突。 */
  async function handleSaveActiveNote() {
    if (!activeNote || !isActiveNoteDirty) {
      return;
    }

    const expectedHash = editingBaseHashes[activeNote.id] ?? activeNote.contentHash;

    beginBusy("正在保存当前 Markdown...");

    try {
      const nextSnapshot = await saveNoteContent(currentSnapshot, activeNote.id, activeNote.content, expectedHash);
      const nextDirtyNoteIds = new Set(dirtyNoteIds);

      nextDirtyNoteIds.delete(activeNote.id);
      commitSnapshot(nextSnapshot, nextDirtyNoteIds);
      setNotice(`已保存「${activeNote.title}」。`);
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 保存当前 txt 草稿，后端会用开始编辑时的 hash 检测外部编辑器冲突。 */
  async function handleSaveActiveDocument() {
    if (!activeDocument || activeDocument.fileType !== "txt" || !isActiveDocumentDirty) {
      return;
    }

    const expectedHash = editingBaseDocumentHashes[activeDocument.id] ?? activeDocument.contentHash;

    beginBusy("正在保存当前 TXT...");

    try {
      const nextSnapshot = await saveDocumentContent(
        currentSnapshot,
        activeDocument.id,
        activeDocument.content ?? "",
        expectedHash,
      );
      const nextDirtyDocumentIds = new Set(dirtyDocumentIds);

      nextDirtyDocumentIds.delete(activeDocument.id);
      commitSnapshot(nextSnapshot, dirtyNoteIds, nextDirtyDocumentIds);
      setNotice(`已保存「${activeDocument.title}」。`);
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 导出前保存当前脏草稿；保存冲突会抛出错误并阻止后续导出。 */
  async function saveCurrentDirtyFileBeforeExport(targetKind: "note" | "document", targetId: string) {
    let snapshotForExport = currentSnapshot;

    if (targetKind === "note") {
      const noteForExport = snapshotForExport.notes.find((note) => note.id === targetId);

      if (!noteForExport) {
        throw new Error("找不到要导出的 Markdown 笔记。");
      }

      if (dirtyNoteIds.has(targetId)) {
        const expectedHash = editingBaseHashes[targetId] ?? noteForExport.contentHash;
        const nextDirtyNoteIds = new Set(dirtyNoteIds);

        // 导出必须基于本地磁盘版本，先复用现有保存命令执行 hash 冲突检测和原子写入。
        snapshotForExport = await saveNoteContent(snapshotForExport, targetId, noteForExport.content, expectedHash);
        nextDirtyNoteIds.delete(targetId);
        commitSnapshot(snapshotForExport, nextDirtyNoteIds, dirtyDocumentIds);
      }

      return snapshotForExport;
    }

    const documentForExport = snapshotForExport.documents.find((document) => document.id === targetId);

    if (!documentForExport) {
      throw new Error("找不到要导出的文档。");
    }

    if (documentForExport.fileType === "txt" && dirtyDocumentIds.has(targetId)) {
      const expectedHash = editingBaseDocumentHashes[targetId] ?? documentForExport.contentHash;
      const nextDirtyDocumentIds = new Set(dirtyDocumentIds);

      // 只有 TXT 可编辑；DOCX/PDF 是只读源文件，不需要也不能执行保存命令。
      snapshotForExport = await saveDocumentContent(
        snapshotForExport,
        targetId,
        documentForExport.content ?? "",
        expectedHash,
      );
      nextDirtyDocumentIds.delete(targetId);
      commitSnapshot(snapshotForExport, dirtyNoteIds, nextDirtyDocumentIds);
    }

    return snapshotForExport;
  }

  /** 导出当前打开文件；保存对话框取消返回 null，前端只给普通提示不报错。 */
  async function handleExportActiveFile(format: ExportFormat) {
    const targetKind = activeDocument ? "document" : "note";
    const targetId = activeDocument?.id ?? activeNote?.id ?? "";
    const sourceType = activeDocument?.fileType ?? "markdown";

    if (!targetId) {
      return;
    }

    const startedAt = performance.now();
    const logMetadata = {
      format,
      targetKind,
      sourceType,
      dirtyBeforeExport:
        targetKind === "note" ? Boolean(activeNote && dirtyNoteIds.has(activeNote.id)) : Boolean(activeDocument && dirtyDocumentIds.has(activeDocument.id)),
    };

    logInfo("开始导出当前文件。", {
      category: "frontend",
      event: "export_file",
      status: "started",
      metadata: logMetadata,
    });
    beginBusy("正在导出当前文件...");

    try {
      const snapshotForExport = await saveCurrentDirtyFileBeforeExport(targetKind, targetId);
      const result = await exportCurrentFile(snapshotForExport, targetKind, targetId, format);

      if (!result) {
        setNotice("已取消导出。");
        logInfo("当前文件导出已取消。", {
          category: "frontend",
          event: "export_file",
          status: "cancelled",
          durationMs: performance.now() - startedAt,
          metadata: logMetadata,
        });
        return;
      }

      setNotice(`已导出「${result.fileName}」。`);
      logInfo("当前文件导出完成。", {
        category: "frontend",
        event: "export_file",
        status: "completed",
        durationMs: performance.now() - startedAt,
        metadata: {
          ...logMetadata,
          byteSize: result.byteSize,
        },
      });
    } catch (error) {
      setNotice(formatErrorMessage(error));
      logError("当前文件导出失败。", {
        category: "frontend",
        event: "export_file",
        status: "failed",
        durationMs: performance.now() - startedAt,
        error,
        metadata: logMetadata,
      });
    } finally {
      endBusy();
    }
  }

  /** 打开重命名弹窗；存在未保存草稿时先阻止，避免本地文件版本语义不清。 */
  function openRenameDialog(noteId = activeNote?.id ?? "") {
    const note = currentSnapshot.notes.find((item) => item.id === noteId);

    if (!note) {
      return;
    }

    if (dirtyNoteIds.size > 0 || dirtyDocumentIds.size > 0) {
      setNotice("请先保存当前草稿，再重命名。");
      return;
    }

    setRenameDialog({ kind: "note", id: note.id, fileName: getFileNameFromPath(note.path) });
  }

  /** 打开 txt 重命名弹窗；存在未保存草稿时先阻止，避免本地文件版本语义不清。 */
  function openRenameDocumentDialog(documentId = activeDocument?.id ?? "") {
    const document = currentSnapshot.documents.find((item) => item.id === documentId);

    if (!document || document.fileType !== "txt") {
      return;
    }

    if (dirtyNoteIds.size > 0 || dirtyDocumentIds.size > 0) {
      setNotice("请先保存当前草稿，再重命名。");
      return;
    }

    setRenameDialog({ kind: "document", id: document.id, fileName: getFileNameFromPath(document.path) });
  }

  /** 提交重命名弹窗中的新文件名，真实桌面端最终由 Tauri 校验并执行 fs::rename。 */
  async function handleSubmitRename() {
    if (!renameDialog) {
      return;
    }

    if (renameDialog.kind === "document") {
      await handleSubmitRenameDocument();
      return;
    }

    const note = currentSnapshot.notes.find((item) => item.id === renameDialog.id);

    if (!note) {
      setRenameDialog(null);
      return;
    }

    const currentFileName = getFileNameFromPath(note.path);
    const nextFileName = renameDialog.fileName.trim();

    if (!nextFileName || nextFileName === currentFileName) {
      setRenameDialog(null);
      return;
    }

    beginBusy("正在重命名 Markdown...");

    try {
      const nextSnapshot = await renameNote(currentSnapshot, note.id, nextFileName);

      commitSnapshot(nextSnapshot, new Set());
      setCollapsedFolderPaths(new Set());
      setRenameDialog(null);
      setNotice(`已重命名为「${nextFileName}」。`);
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 提交 txt 文档重命名，后端会拒绝非 txt、越界路径和重名目标。 */
  async function handleSubmitRenameDocument() {
    if (!renameDialog || renameDialog.kind !== "document") {
      return;
    }

    const document = currentSnapshot.documents.find((item) => item.id === renameDialog.id);

    if (!document) {
      setRenameDialog(null);
      return;
    }

    const currentFileName = getFileNameFromPath(document.path);
    const nextFileName = renameDialog.fileName.trim();

    if (!nextFileName || nextFileName === currentFileName) {
      setRenameDialog(null);
      return;
    }

    beginBusy("正在重命名 TXT...");

    try {
      const nextSnapshot = await renameDocument(currentSnapshot, document.id, nextFileName);

      commitSnapshot(nextSnapshot, dirtyNoteIds, new Set());
      setCollapsedFolderPaths(new Set());
      setRenameDialog(null);
      setNotice(`已重命名为「${nextFileName}」。`);
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 删除指定 Markdown 文件到系统回收站；删除前二次确认并携带保存基准 hash。 */
  async function handleDeleteNote(noteId = activeNote?.id ?? "") {
    const note = currentSnapshot.notes.find((item) => item.id === noteId);

    if (!note) {
      return;
    }

    if (dirtyNoteIds.size > 0 || dirtyDocumentIds.size > 0) {
      setNotice("请先保存当前草稿，再删除。");
      return;
    }

    requestConfirmation(
      {
        title: "移入回收站",
        message: `将「${note.title}」移入系统回收站？这会从当前工作区移除索引和会话引用。`,
        confirmLabel: "移入回收站",
      },
      async () => {
        const expectedHash = editingBaseHashes[note.id] ?? note.contentHash;

        beginBusy("正在删除 Markdown...");

        try {
          const nextSnapshot = await deleteNote(currentSnapshot, note.id, expectedHash);

          commitSnapshot(nextSnapshot, new Set(), dirtyDocumentIds);
          setNotice("已移入系统回收站。");
        } catch (error) {
          setNotice(error instanceof Error ? error.message : String(error));
        } finally {
          endBusy();
        }
      },
    );
  }

  /** 删除指定 txt 文档到系统回收站；删除前二次确认并携带保存基准 hash。 */
  async function handleDeleteDocument(documentId = activeDocument?.id ?? "") {
    const document = currentSnapshot.documents.find((item) => item.id === documentId);

    if (!document || document.fileType !== "txt") {
      return;
    }

    if (dirtyNoteIds.size > 0 || dirtyDocumentIds.size > 0) {
      setNotice("请先保存当前草稿，再删除。");
      return;
    }

    requestConfirmation(
      {
        title: "移入回收站",
        message: `将「${document.title}」移入系统回收站？这会从当前工作区移除该 TXT 文档引用。`,
        confirmLabel: "移入回收站",
      },
      async () => {
        const expectedHash = editingBaseDocumentHashes[document.id] ?? document.contentHash;

        beginBusy("正在删除 TXT...");

        try {
          const nextSnapshot = await deleteDocument(currentSnapshot, document.id, expectedHash);

          commitSnapshot(nextSnapshot, dirtyNoteIds, new Set());
          setNotice("已移入系统回收站。");
        } catch (error) {
          setNotice(error instanceof Error ? error.message : String(error));
        } finally {
          endBusy();
        }
      },
    );
  }

  /** 新建一个空白知识库会话；标题等到首条用户输入后再确定。 */
  async function handleCreateSession() {
    logInfo("创建空白会话。", {
      category: "frontend",
      event: "create_session",
      status: "started",
      metadata: {
        knowledgeBaseId: activeKnowledgeBase.id,
      },
    });

    const nextSession = buildAgentSession({
      knowledgeBase: activeKnowledgeBase,
    });
    const nextSnapshot = {
      ...currentSnapshot,
      sessions: [nextSession, ...currentSnapshot.sessions],
      activeSessionId: nextSession.id,
    };

    beginBusy("正在创建 Agent 会话...");

    try {
      commitSnapshot(await saveSession(nextSnapshot, nextSession));
      logInfo("空白会话已创建。", {
        category: "frontend",
        event: "create_session",
        status: "completed",
        metadata: {
          knowledgeBaseId: activeKnowledgeBase.id,
        },
      });
      setIsSessionListOpen(false);
      setIsSessionContextOpen(false);
      setIsScopeSelectorOpen(false);
      resetTurnSelection();
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 切换会话时恢复它绑定的知识库和工具范围；文件焦点不再被默认会话推着走。 */
  async function handleSelectSession(sessionId: string) {
    const nextSession = currentSnapshot.sessions.find((session) => session.id === sessionId);

    if (!nextSession) {
      return;
    }

    beginBusy("正在恢复 Agent 会话...");

    try {
      const nextSnapshot = await restoreSessionContext(currentSnapshot, sessionId);

      commitSnapshot(nextSnapshot);
      setSearchTerm("");
      setCollapsedFolderPaths(new Set());
      setIsSessionListOpen(false);
      setIsSessionContextOpen(false);
      setIsScopeSelectorOpen(false);
      resetTurnSelection();
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 逻辑删除 Agent 会话；删除当前会话时由后端选择下一条可用会话。 */
  async function handleDeleteSession(sessionId: string) {
    const session = currentSnapshot.sessions.find((item) => item.id === sessionId);

    if (!session) {
      return;
    }

    requestConfirmation(
      {
        title: "删除 Agent 会话",
        message: `删除会话「${session.title}」？本地文档和请求审计记录不会被删除。`,
        confirmLabel: "删除会话",
      },
      async () => {
        beginBusy("正在删除 Agent 会话...");

        try {
          const nextSnapshot = await deleteSession(currentSnapshot, sessionId);

          commitSnapshot(nextSnapshot);
          setIsSessionListOpen(true);
          setIsSessionContextOpen(false);
          setIsScopeSelectorOpen(false);
          setNotice("已删除会话。");
        } catch (error) {
          setNotice(error instanceof Error ? error.message : String(error));
        } finally {
          endBusy();
        }
      },
    );
  }

  /** 为当前会话勾选或取消额外知识库，当前激活知识库始终保留。 */
  async function handleToggleScopeKnowledgeBase(knowledgeBaseId: string) {
    if (!isPersistedSession(currentSnapshot, activeSession)) {
      setNotice("请先新建或发送一条消息创建会话，再调整工具范围。");
      return;
    }

    const selectedIds = new Set(activeSession.knowledgeBaseIds.length ? activeSession.knowledgeBaseIds : [activeKnowledgeBase.id]);

    selectedIds.add(activeKnowledgeBase.id);

    // 当前激活知识库是默认工具范围边界，不能在本会话中取消。
    if (knowledgeBaseId !== activeKnowledgeBase.id) {
      if (selectedIds.has(knowledgeBaseId)) {
        selectedIds.delete(knowledgeBaseId);
      } else {
        selectedIds.add(knowledgeBaseId);
      }
    }

    beginBusy("正在更新工具范围...");

    try {
      commitSnapshot(
        await updateSessionScope(currentSnapshot, activeSession.id, Array.from(selectedIds), activeKnowledgeBase.id),
      );
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 设置当前会话的默认 provider/model；传入空字符串表示跟随全局默认模型。 */
  async function handleSetSessionModelSelection(selection: string) {
    if (!isPersistedSession(currentSnapshot, activeSession)) {
      setNotice("请先新建或发送一条消息创建会话，再设置会话默认模型。");
      return;
    }

    const decodedSelection = decodeModelSelection(selection);
    const nextSession: AgentSession = {
      ...activeSession,
      modelProviderId: decodedSelection.providerId || undefined,
      modelId: decodedSelection.modelId || undefined,
    };

    beginBusy("正在更新会话默认模型...");

    try {
      commitSnapshot(await saveSession(currentSnapshot, nextSession));
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 手动整理当前会话工作记忆；日志只记录消息数量和摘要状态，不写入正文。 */
  async function handleCompactAgentContext() {
    if (!isPersistedSession(currentSnapshot, activeSession)) {
      setNotice("请先新建或发送一条消息创建会话，再整理上下文。");
      return;
    }

    beginBusy("正在整理上下文...");

    try {
      logInfo("开始手动整理会话上下文。", {
        category: "frontend",
        event: "compact_agent_context",
        status: "started",
        metadata: {
          sessionId: activeSession.id,
          messageCount: activeSession.messages.length,
          hasContextSummary: Boolean(activeSession.contextSummary),
          hasActivePendingChange: activeSession.pendingChange?.status === "pending",
        },
      });
      commitSnapshot(await compactAgentContext(currentSnapshot, activeSession.id));
      setNotice("已整理上下文。");
      logInfo("手动整理会话上下文完成。", {
        category: "frontend",
        event: "compact_agent_context",
        status: "completed",
        metadata: {
          sessionId: activeSession.id,
          messageCount: activeSession.messages.length,
          hasActivePendingChange: activeSession.pendingChange?.status === "pending",
        },
      });
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
      logWarn("手动整理会话上下文失败。", {
        category: "frontend",
        event: "compact_agent_context",
        status: "failed",
        metadata: {
          sessionId: activeSession.id,
          messageCount: activeSession.messages.length,
        },
      });
    } finally {
      endBusy();
    }
  }

  /** 切换会话历史浮层；日志只记录状态和数量，不写入会话标题、消息正文或路径。 */
  function handleToggleSessionList() {
    const nextOpen = !isSessionListOpen;

    logInfo("切换会话历史浮层。", {
      category: "frontend",
      event: "toggle_session_list",
      status: nextOpen ? "opened" : "closed",
      metadata: {
        sessionCount: currentSnapshot.sessions.length,
        hasActivePendingChange: activeSession.pendingChange?.status === "pending",
      },
    });
    setIsSessionListOpen(nextOpen);
    setIsSessionContextOpen(false);
    setIsScopeSelectorOpen(false);
  }

  /** 切换上下文浮层；日志只记录数量和状态，不写入标题、正文、知识库名称或路径。 */
  function handleToggleSessionContext() {
    const nextOpen = !isSessionContextOpen;

    logInfo("切换上下文浮层。", {
      category: "frontend",
      event: "toggle_session_context",
      status: nextOpen ? "opened" : "closed",
      metadata: {
        messageCount: activeSession.messages.length,
        selectedScopeCount: activeSession.knowledgeBaseIds.length || 1,
        hasActivePendingChange: activeSession.pendingChange?.status === "pending",
      },
    });
    setIsSessionContextOpen(nextOpen);
    setIsSessionListOpen(false);
    setIsScopeSelectorOpen(false);
  }

  /** 切换工具范围浮层；日志只记录数量和状态，不写入知识库名称或本地路径。 */
  function handleToggleScopeSelector() {
    const nextOpen = !isScopeSelectorOpen;

    logInfo("切换工具范围浮层。", {
      category: "frontend",
      event: "toggle_scope_selector",
      status: nextOpen ? "opened" : "closed",
      metadata: {
        knowledgeBaseCount: currentSnapshot.knowledgeBases.length,
        selectedScopeCount: activeSession.knowledgeBaseIds.length || 1,
      },
    });
    setIsScopeSelectorOpen(nextOpen);
    setIsSessionListOpen(false);
    setIsSessionContextOpen(false);
  }

  /** 切换右侧 Agent 协作区显隐，保留编辑区优先的桌面工作流。 */
  function handleToggleAgentPanelCollapsed() {
    const nextCollapsedState = !isAgentPanelCollapsed;

    logInfo("切换 Agent 协作区显隐。", {
      category: "frontend",
      event: "agent_panel_visibility_toggle",
      status: nextCollapsedState ? "collapsed" : "expanded",
      metadata: {
        messageCount: activeSession.messages.length,
        hasActivePendingChange: activeSession.pendingChange?.status === "pending",
      },
    });
    setIsAgentPanelCollapsed(nextCollapsedState);
  }

  /** 提交 Agent 输入，运行时会自行决定是否调用检索工具。 */
  async function handleSubmitPrompt(action: AgentActionType = "ask", presetPrompt?: string, sourceSnapshot = currentSnapshot) {
    const prompt = (presetPrompt ?? agentPrompt).trim();
    const turnExplicitSkillIds = presetPrompt ? [] : explicitSkillIds;
    const sourceActiveSession = sourceSnapshot.sessions.find((session) => session.id === sourceSnapshot.activeSessionId) ?? activeSession;
    const sourceActiveKnowledgeBase =
      sourceSnapshot.knowledgeBases.find((knowledgeBase) => knowledgeBase.id === sourceSnapshot.activeKnowledgeBaseId) ?? activeKnowledgeBase;
    const sourceActiveNote = sourceSnapshot.notes.find((note) => note.id === sourceSnapshot.activeNoteId) ?? activeNote;
    const sourceActiveDocument =
      sourceSnapshot.documents.find((document) => document.id === sourceSnapshot.activeDocumentId) ?? activeDocument;

    // 空输入不创建消息，避免侧栏出现无意义的对话记录。
    if (!prompt) {
      return;
    }

    const optimisticMessage = buildOptimisticUserMessage(prompt, action);
    const promptBeforeSubmit = agentPrompt;
    let didPersistOptimisticMessage = false;

    beginBusy("Agent 正在处理...");

    try {
      let snapshotForTurn = sourceSnapshot;
      let sessionForTurn = sourceActiveSession;

      if (!isPersistedSession(sourceSnapshot, sourceActiveSession)) {
        sessionForTurn = buildAgentSession({
          knowledgeBase: sourceActiveKnowledgeBase,
          title: buildTitleFromFirstPrompt(prompt),
        });
        snapshotForTurn = {
          ...sourceSnapshot,
          sessions: [sessionForTurn, ...sourceSnapshot.sessions],
          activeSessionId: sessionForTurn.id,
        };
        logInfo("准备创建草稿会话。", {
          category: "frontend",
          event: "bootstrap_session",
          status: "started",
          metadata: {
            knowledgeBaseId: sourceActiveKnowledgeBase.id,
            promptLength: prompt.length,
            explicitSkillCount: turnExplicitSkillIds.length,
          },
        });
      } else if (shouldUseFirstPromptAsTitle(sourceActiveSession)) {
        const titled = applyFirstPromptTitle(sourceSnapshot, sourceActiveSession, prompt);

        sessionForTurn = titled.session;
        snapshotForTurn = titled.snapshot;
        logInfo("会话标题已由首条输入确定。", {
          category: "frontend",
          event: "title_session",
          status: "completed",
          metadata: {
            knowledgeBaseId: sourceActiveKnowledgeBase.id,
            promptLength: prompt.length,
            explicitSkillCount: turnExplicitSkillIds.length,
          },
        });
      }

      const optimisticTurn = appendUserMessageToSession(snapshotForTurn, sessionForTurn, optimisticMessage);

      sessionForTurn = optimisticTurn.session;
      snapshotForTurn = optimisticTurn.snapshot;
      // 先提交本地快照，让用户发送的消息立即出现在对话框中，再等待 Agent 慢任务。
      commitSnapshot(snapshotForTurn);
      setAgentPrompt("");
      snapshotForTurn = await saveSession(snapshotForTurn, sessionForTurn);
      didPersistOptimisticMessage = true;
      logInfo("用户消息已乐观落库。", {
        category: "frontend",
        event: "persist_user_message",
        status: "completed",
        metadata: {
          knowledgeBaseId: activeKnowledgeBase.id,
          sessionId: sessionForTurn.id,
          promptLength: prompt.length,
          explicitSkillCount: turnExplicitSkillIds.length,
        },
      });

      const turnSnapshot = {
        ...snapshotForTurn,
        activeSessionId: sessionForTurn.id,
        activeKnowledgeBaseId: sourceActiveKnowledgeBase.id,
        activeNoteId: sourceActiveNote?.id ?? "",
        activeDocumentId: sourceActiveDocument?.id ?? "",
      };
      const decodedTurnModelSelection = decodeModelSelection(turnModelSelection);
      const result = await runAgentTurn(
        turnSnapshot,
        prompt,
        action,
        optimisticMessage.id,
        decodedTurnModelSelection.providerId || undefined,
        decodedTurnModelSelection.modelId || undefined,
        turnExplicitSkillIds,
      );

      commitSnapshot(result.snapshot);
      if (!presetPrompt) {
        setExplicitSkillIds([]);
      }
      const [nextAuditLogs, nextAppEventLogs] = await Promise.all([loadRequestAuditLogs(), loadAppEventLogs()]);

      setAuditLogs(nextAuditLogs);
      setAppEventLogs(nextAppEventLogs);
    } catch (error) {
      if (!didPersistOptimisticMessage) {
        commitSnapshot(sourceSnapshot);
        setAgentPrompt(promptBeforeSubmit);
      }
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 为当前待写入 diff 添加行评论；日志只记录行号、侧别和计数，不记录评论正文。 */
  async function handleAddReviewComment(commentDraft: ReviewCommentDraft) {
    const pendingChange = activeSession.pendingChange;

    if (!pendingChange || pendingChange.status !== "pending" || !isPersistedSession(currentSnapshot, activeSession)) {
      return;
    }

    const nextComment: ReviewComment = {
      id: createLocalId("review-comment"),
      changeId: pendingChange.id,
      lineSide: commentDraft.lineSide,
      lineNumber: commentDraft.lineNumber,
      lineTextPreview: commentDraft.lineTextPreview,
      body: commentDraft.body,
      status: "draft",
      createdAt: formatLocalDateTime(),
    };
    const nextComments = [...(pendingChange.reviewComments ?? []), nextComment];
    const nextChange: ProposedChange = {
      ...pendingChange,
      reviewComments: nextComments,
      reviewState: buildReviewState(nextComments, nextComment),
      diffStats: pendingChange.diffStats ?? buildMarkdownDiff(pendingChange.original, pendingChange.next).stats,
    };
    const nextSession = {
      ...activeSession,
      pendingChange: nextChange,
      updatedAt: formatLocalDateTime(),
    };
    const nextSnapshot = updateActivePendingChange(currentSnapshot, nextChange);

    commitSnapshot(nextSnapshot);
    logInfo("添加 diff 行评论。", {
      category: "frontend",
      event: "review_comment_add",
      status: "completed",
      metadata: {
        changeId: pendingChange.id,
        sessionId: activeSession.id,
        lineSide: commentDraft.lineSide,
        lineNumber: commentDraft.lineNumber,
        commentCount: nextComments.length,
      },
    });

    try {
      commitSnapshot(await saveSession(nextSnapshot, nextSession));
    } catch (error) {
      logWarn("保存 diff 行评论失败。", {
        category: "frontend",
        event: "review_comment_save",
        status: "failed",
        error,
        metadata: {
          changeId: pendingChange.id,
          sessionId: activeSession.id,
        },
      });
      setNotice(error instanceof Error ? error.message : String(error));
    }
  }

  /** 把待发送审阅评论转成用户消息，让 Agent 基于定位反馈重新生成 pending diff。 */
  async function handleSubmitReviewComments() {
    const pendingChange = activeSession.pendingChange;
    const draftComments = pendingChange?.reviewComments?.filter((comment) => comment.status === "draft") ?? [];

    if (!pendingChange || pendingChange.status !== "pending" || !draftComments.length || !isPersistedSession(currentSnapshot, activeSession)) {
      return;
    }

    const submittedAt = formatLocalDateTime();
    const nextComments = (pendingChange.reviewComments ?? []).map((comment) =>
      comment.status === "draft" ? { ...comment, status: "submitted" as const, createdAt: comment.createdAt || submittedAt } : comment,
    );
    const nextChange: ProposedChange = {
      ...pendingChange,
      reviewComments: nextComments,
      reviewState: buildReviewState(nextComments),
      diffStats: pendingChange.diffStats ?? buildMarkdownDiff(pendingChange.original, pendingChange.next).stats,
    };
    const nextSession = {
      ...activeSession,
      pendingChange: nextChange,
      updatedAt: submittedAt,
    };
    const nextSnapshot = updateActivePendingChange(currentSnapshot, nextChange);

    commitSnapshot(nextSnapshot);
    logInfo("提交 diff 审阅评论给 Agent。", {
      category: "frontend",
      event: "review_comments_submit",
      status: "started",
      metadata: {
        changeId: pendingChange.id,
        sessionId: activeSession.id,
        commentCount: draftComments.length,
      },
    });

    try {
      const savedSnapshot = await saveSession(nextSnapshot, nextSession);

      commitSnapshot(savedSnapshot);
      await handleSubmitPrompt(
        pendingChange.type === "create" ? "create" : "rewrite",
        buildReviewFeedbackPrompt(pendingChange, draftComments),
        savedSnapshot,
      );
      logInfo("diff 审阅评论已交给 Agent。", {
        category: "frontend",
        event: "review_comments_submit",
        status: "completed",
        metadata: {
          changeId: pendingChange.id,
          sessionId: activeSession.id,
          commentCount: draftComments.length,
        },
      });
    } catch (error) {
      logWarn("提交 diff 审阅评论失败。", {
        category: "frontend",
        event: "review_comments_submit",
        status: "failed",
        error,
        metadata: {
          changeId: pendingChange.id,
          sessionId: activeSession.id,
        },
      });
      setNotice(error instanceof Error ? error.message : String(error));
    }
  }

  /** 重新扫描指定知识库，使用本地支持文档刷新目录树和 Markdown FTS 索引。 */
  async function handleRescanKnowledgeBase(knowledgeBaseId: string) {
    if (dirtyNoteIds.size > 0 || dirtyDocumentIds.size > 0) {
      setNotice("请先保存当前草稿，再刷新目录树。");
      return;
    }

    beginBusy("正在重新扫描知识库...");

    try {
      const nextSnapshot = await rescanKnowledgeBase(currentSnapshot, knowledgeBaseId);

      commitSnapshot(nextSnapshot, new Set(), new Set());
      setCollapsedFolderPaths(new Set());
      setNotice(buildScanNotice(nextSnapshot, knowledgeBaseId));
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 移除知识库授权和索引缓存，不删除用户选择目录中的本地文件。 */
  async function handleRemoveKnowledgeBase(knowledgeBaseId: string) {
    const knowledgeBase = currentSnapshot.knowledgeBases.find((item) => item.id === knowledgeBaseId);

    if (!knowledgeBase) {
      return;
    }

    requestConfirmation(
      {
        title: "移除知识库授权",
        message: `移除「${knowledgeBase.name}」的知识库授权？本地文件不会被删除，但索引缓存和会话范围会同步清理。`,
        confirmLabel: "移除授权",
      },
      async () => {
        beginBusy("正在移除知识库授权...");

        try {
          const nextSnapshot = await removeKnowledgeBase(currentSnapshot, knowledgeBaseId);

          commitSnapshot(nextSnapshot, new Set(), new Set());
          setCollapsedFolderPaths(new Set());
          setNotice(`已移除「${knowledgeBase.name}」授权，本地文件未被删除。`);

          if (!nextSnapshot.knowledgeBases.length) {
            setSearchTerm("");
            setIsSettingsOpen(false);
          }
        } catch (error) {
          setNotice(error instanceof Error ? error.message : String(error));
        } finally {
          endBusy();
        }
      },
    );
  }

  /** 接受 Agent diff，真实桌面版会在 Tauri 层做路径、hash 和原子写入校验。 */
  async function handleAcceptChange() {
    const pendingChange = activeSession.pendingChange;

    if (pendingChange) {
      logInfo("准备确认写入审阅 diff。", {
        category: "frontend",
        event: "review_change_accept",
        status: "started",
        metadata: {
          changeId: pendingChange.id,
          sessionId: activeSession.id,
          changeType: pendingChange.type,
          operation: pendingChange.operation ?? "replace",
          commentCount: pendingChange.reviewComments?.length ?? 0,
        },
      });
    }

    beginBusy("正在应用 diff...");

    try {
      const nextSnapshot = await acceptProposedChange(currentSnapshot);

      commitSnapshot(nextSnapshot);
      const nextAppEventLogs = await loadAppEventLogs();

      setAppEventLogs(nextAppEventLogs);
      setNotice("已应用本次 diff。");
    } catch (error) {
      if (pendingChange) {
        logWarn("确认写入审阅 diff 失败。", {
          category: "frontend",
          event: "review_change_accept",
          status: "failed",
          error,
          metadata: {
            changeId: pendingChange.id,
            sessionId: activeSession.id,
            operation: pendingChange.operation ?? "replace",
          },
        });
      }
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 取消 Agent diff，保持原始 Markdown 内容不变。 */
  async function handleRejectChange() {
    const pendingChange = activeSession.pendingChange;

    if (pendingChange) {
      logInfo("准备取消审阅 diff。", {
        category: "frontend",
        event: "review_change_reject",
        status: "started",
        metadata: {
          changeId: pendingChange.id,
          sessionId: activeSession.id,
          changeType: pendingChange.type,
          operation: pendingChange.operation ?? "replace",
          commentCount: pendingChange.reviewComments?.length ?? 0,
        },
      });
    }

    beginBusy("正在取消 diff...");

    try {
      commitSnapshot(await rejectProposedChange(currentSnapshot));
      const nextAppEventLogs = await loadAppEventLogs();

      setAppEventLogs(nextAppEventLogs);
      setNotice("已取消本次 diff。");
    } catch (error) {
      if (pendingChange) {
        logWarn("取消审阅 diff 失败。", {
          category: "frontend",
          event: "review_change_reject",
          status: "failed",
          error,
          metadata: {
            changeId: pendingChange.id,
            sessionId: activeSession.id,
            operation: pendingChange.operation ?? "replace",
          },
        });
      }
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 打开设置抽屉时刷新非阻塞诊断信息，避免展示过旧的日志列表。 */
  function handleOpenSettings() {
    setIsSettingsOpen(true);
    void loadInitialDiagnosticLogs();
  }

  return (
    <div className="app-shell">
      <TopBar
        activeKnowledgeBase={activeKnowledgeBase}
        knowledgeBaseCount={currentSnapshot.knowledgeBases.length}
        onOpenSettings={handleOpenSettings}
      />
      <main
        className={`workspace-grid ${resizingPane ? "is-resizing" : ""} ${isAgentPanelCollapsed ? "is-agent-collapsed" : ""}`}
        ref={workspaceRef}
        style={{ gridTemplateColumns }}
      >
        <KnowledgeBaseSidebar
          knowledgeBases={currentSnapshot.knowledgeBases}
          activeKnowledgeBase={activeKnowledgeBase}
          fileTree={fileTree}
          activeNoteId={activeNote?.id ?? ""}
          activeDocumentId={activeDocument?.id ?? ""}
          collapsedFolderPaths={collapsedFolderPaths}
          searchTerm={searchTerm}
          isBusy={isBusy}
          busyLabel={busyLabel}
          notice={notice}
          onSearchChange={setSearchTerm}
          onSelectKnowledgeBase={handleSelectKnowledgeBase}
          onAddKnowledgeBase={handleAddKnowledgeBase}
          onToggleFolder={handleToggleFolder}
          onSelectNote={handleSelectNote}
          onSelectDocument={handleSelectDocument}
          onRenameNote={openRenameDialog}
          onDeleteNote={handleDeleteNote}
          onOpenNoteHistory={openNoteHistory}
          onRenameDocument={openRenameDocumentDialog}
          onDeleteDocument={handleDeleteDocument}
          onOpenDocumentHistory={openDocumentHistory}
          onCreateMarkdown={(parentPath) => openCreateDialog("markdown", parentPath)}
          onCreateText={(parentPath) => openCreateDialog("text", parentPath)}
          onCreateFolder={(parentPath) => openCreateDialog("folder", parentPath)}
          onRefreshKnowledgeBase={handleRescanKnowledgeBase}
        />
        <div
          className={`workspace-resizer ${resizingPane === "sidebar" ? "active" : ""}`}
          {...getSeparatorProps("sidebar")}
        />
        {activeDocument ? (
          <DocumentPane
            document={activeDocument}
            knowledgeBase={activeKnowledgeBase}
            preview={documentPreview ?? undefined}
            previewError={documentPreviewError}
            isPreviewLoading={isDocumentPreviewLoading}
            isBusy={isBusy}
            isDirty={isActiveDocumentDirty}
            onSaveDocument={handleSaveActiveDocument}
            onContentChange={handleDocumentContentChange}
            onExportFile={handleExportActiveFile}
            onOpenHistory={() => openDocumentHistory()}
            onRenameDocument={() => openRenameDocumentDialog()}
            onDeleteDocument={() => handleDeleteDocument()}
          />
        ) : (
          <EditorPane
            note={activeNote}
            knowledgeBase={activeKnowledgeBase}
            proposedChange={activeSession.pendingChange?.status === "pending" ? activeSession.pendingChange : undefined}
            isBusy={isBusy}
            isDirty={isActiveNoteDirty}
            viewMode={markdownViewMode}
            onViewModeChange={setMarkdownViewMode}
            onSaveNote={handleSaveActiveNote}
            onContentChange={handleContentChange}
            onPasteImages={handlePasteImages}
            onExportFile={handleExportActiveFile}
            onOpenHistory={() => openNoteHistory()}
            onRenameNote={() => openRenameDialog()}
            onDeleteNote={() => handleDeleteNote()}
            onAcceptChange={handleAcceptChange}
            onRejectChange={handleRejectChange}
            onAddReviewComment={handleAddReviewComment}
            onSubmitReviewComments={handleSubmitReviewComments}
          />
        )}
        {!isAgentPanelCollapsed && (
          <div
            className={`workspace-resizer ${resizingPane === "agent" ? "active" : ""}`}
            {...getSeparatorProps("agent")}
          />
        )}
        {isAgentPanelCollapsed ? (
          <button
            className="agent-panel-reopen"
            type="button"
            title="展开 Agent 协作区"
            onClick={handleToggleAgentPanelCollapsed}
          >
            <PanelRightOpen size={17} />
            <span>Agent</span>
          </button>
        ) : (
          <AgentPanel
            sessions={currentSnapshot.sessions}
            activeSession={activeSession}
            activeKnowledgeBase={activeKnowledgeBase}
            knowledgeBases={currentSnapshot.knowledgeBases}
            notes={currentSnapshot.notes}
            prompt={agentPrompt}
            skills={agentSkills}
            selectedSkillIds={explicitSkillIds}
            modelConfig={userSettings.modelConfig}
            turnModelSelection={turnModelSelection}
            isBusy={isBusy}
            isSessionListOpen={isSessionListOpen}
            isSessionContextOpen={isSessionContextOpen}
            isScopeSelectorOpen={isScopeSelectorOpen}
            onToggleSessionList={handleToggleSessionList}
            onToggleSessionContext={handleToggleSessionContext}
            onToggleScopeSelector={handleToggleScopeSelector}
            onCollapsePanel={handleToggleAgentPanelCollapsed}
            onCreateSession={handleCreateSession}
            onSelectSession={handleSelectSession}
            onDeleteSession={handleDeleteSession}
            onToggleScopeKnowledgeBase={handleToggleScopeKnowledgeBase}
            onPromptChange={setAgentPrompt}
            onSelectedSkillIdsChange={setExplicitSkillIds}
            onSubmitPrompt={() => handleSubmitPrompt("ask")}
            onTurnModelSelectionChange={setTurnModelSelection}
            onSetSessionModelSelection={handleSetSessionModelSelection}
            onCompactAgentContext={handleCompactAgentContext}
          />
        )}
      </main>
      {isSettingsOpen && (
        <SettingsDrawer
          knowledgeBases={currentSnapshot.knowledgeBases}
          activeKnowledgeBaseId={activeKnowledgeBase.id}
          settings={userSettings}
          imSettings={imSettings}
          skills={agentSkills}
          modelApiKeyStatuses={modelApiKeyStatuses}
          feishuCredentialStatus={feishuCredentialStatus}
          feishuGatewayStatus={feishuGatewayStatus}
          providerTemplates={providerTemplates}
          auditLogs={auditLogs}
          appEventLogs={appEventLogs}
          knowledgeBaseMemories={knowledgeBaseMemories}
          isBusy={isBusy}
          onSelectKnowledgeBase={handleSelectKnowledgeBase}
          onAddKnowledgeBase={handleAddKnowledgeBase}
          onRescanKnowledgeBase={handleRescanKnowledgeBase}
          onRemoveKnowledgeBase={handleRemoveKnowledgeBase}
          onSaveSettings={handleSaveSettings}
          onSaveImSettings={handleSaveImSettings}
          onSaveKnowledgeBaseMemory={handleSaveKnowledgeBaseMemory}
          onDeleteKnowledgeBaseMemory={handleDeleteKnowledgeBaseMemory}
          onSaveSkill={handleSaveSkill}
          onInstallSkill={handleInstallSkill}
          onToggleSkill={handleToggleSkill}
          onDeleteSkill={handleDeleteSkill}
          onOpenUserSkillsFolder={handleOpenUserSkillsFolder}
          onSaveApiKey={handleSaveApiKey}
          onRefreshProviderModels={handleRefreshProviderModels}
          onSaveFeishuSecret={handleSaveFeishuSecret}
          onStartFeishuGateway={handleStartFeishuGateway}
          onStopFeishuGateway={handleStopFeishuGateway}
          onRefreshFeishuStatus={handleRefreshFeishuStatus}
          onRefreshAuditLogs={handleRefreshAuditLogs}
          onRefreshAppEventLogs={handleRefreshAppEventLogs}
          onClearAppEventLogs={handleClearAppEventLogs}
          onOpenAppLogFolder={handleOpenAppLogFolder}
          onClose={() => setIsSettingsOpen(false)}
        />
      )}
      {historyDialog && historyTarget && (
        <DocumentHistoryDialog
          snapshot={currentSnapshot}
          targetKind={historyTarget.targetKind}
          targetId={historyTarget.targetId}
          title={historyTarget.title}
          currentContent={historyTarget.content}
          currentHash={historyTarget.contentHash}
          isDirty={historyTarget.isDirty}
          isBusy={isBusy}
          onClose={() => setHistoryDialog(null)}
          onRestored={handleHistoryRestored}
          onNotice={setNotice}
        />
      )}
      {renameDialog && (
        <div className="modal-backdrop" role="presentation" onMouseDown={() => setRenameDialog(null)}>
          <form
            className="rename-dialog"
            aria-label={renameDialog.kind === "note" ? "重命名 Markdown 文件" : "重命名 TXT 文件"}
            onMouseDown={(event) => event.stopPropagation()}
            onSubmit={(event) => {
              event.preventDefault();
              handleSubmitRename();
            }}
          >
            <div className="modal-header">
              <div>
                <p className="section-label">{renameDialog.kind === "note" ? "Markdown 文件" : "TXT 文件"}</p>
                <h2>重命名</h2>
              </div>
            </div>
            <label className="rename-field">
              <span>文件名</span>
              <input
                autoFocus
                value={renameDialog.fileName}
                onChange={(event) => setRenameDialog({ ...renameDialog, fileName: event.target.value })}
                placeholder="例如：会议记录.md"
              />
            </label>
            <div className="modal-actions">
              <button className="ghost-button" type="button" onClick={() => setRenameDialog(null)} disabled={isBusy}>
                取消
              </button>
              <button className="primary-button compact" type="submit" disabled={isBusy || !renameDialog.fileName.trim()}>
                保存文件名
              </button>
            </div>
          </form>
        </div>
      )}
      {createDialog && (
        <div className="modal-backdrop" role="presentation" onMouseDown={() => setCreateDialog(null)}>
          <form
            className="rename-dialog"
            aria-label={getCreateDialogAriaLabel(createDialog.kind)}
            onMouseDown={(event) => event.stopPropagation()}
            onSubmit={(event) => {
              event.preventDefault();
              handleSubmitCreate();
            }}
          >
            <div className="modal-header">
              <div>
                <p className="section-label">{getCreateParentLabel(createDialog.parentPath)}</p>
                <h2>{getCreateDialogTitle(createDialog.kind)}</h2>
              </div>
            </div>
            <label className="rename-field">
              <span>{createDialog.kind === "folder" ? "目录名" : "文件名"}</span>
              <input
                autoFocus
                value={createDialog.name}
                onChange={(event) => setCreateDialog({ ...createDialog, name: event.target.value })}
                placeholder={getCreatePlaceholder(createDialog.kind)}
              />
            </label>
            <div className="modal-actions">
              <button className="ghost-button" type="button" onClick={() => setCreateDialog(null)} disabled={isBusy}>
                取消
              </button>
              <button className="primary-button compact" type="submit" disabled={isBusy || !createDialog.name.trim()}>
                {getCreateSubmitLabel(createDialog.kind)}
              </button>
            </div>
          </form>
        </div>
      )}
      {pendingConfirmation && (
        <ConfirmDialog
          {...pendingConfirmation}
          isBusy={isBusy}
          onCancel={() => setPendingConfirmation(null)}
          onConfirm={() => void handleConfirmDialogConfirm()}
        />
      )}
    </div>
  );
}

/** 从知识库相对路径中取最后一级文件名，用于重命名弹窗默认值。 */
function getFileNameFromPath(relativePath: string) {
  return relativePath.split("/").filter(Boolean).pop() ?? relativePath;
}

/** 收集当前知识库中已经被文件占用的路径，覆盖 Markdown 和普通文档。 */
function getExistingFilePaths(snapshot: WorkspaceSnapshot, knowledgeBaseId: string) {
  return new Set([
    ...snapshot.notes.filter((note) => note.knowledgeBaseId === knowledgeBaseId).map((note) => note.path),
    ...snapshot.documents.filter((document) => document.knowledgeBaseId === knowledgeBaseId).map((document) => document.path),
  ]);
}

/** 为新建 Markdown 生成当前父目录下不冲突的默认名称。 */
function getNextAvailableMarkdownName(snapshot: WorkspaceSnapshot, knowledgeBaseId: string, parentPath: string) {
  const existingPaths = getExistingFilePaths(snapshot, knowledgeBaseId);

  for (let index = 1; index <= 999; index += 1) {
    const fileName = index === 1 ? "未命名.md" : `未命名 ${index}.md`;

    // 默认名称只看当前目标目录，避免用户打开弹窗后马上遇到后端重名错误。
    if (!existingPaths.has(joinRelativePath(parentPath, fileName))) {
      return fileName;
    }
  }

  return "未命名.md";
}

/** 为新建 TXT 生成当前父目录下不冲突的默认名称。 */
function getNextAvailableTextDocumentName(snapshot: WorkspaceSnapshot, knowledgeBaseId: string, parentPath: string) {
  const existingPaths = getExistingFilePaths(snapshot, knowledgeBaseId);

  for (let index = 1; index <= 999; index += 1) {
    const fileName = index === 1 ? "未命名.txt" : `未命名 ${index}.txt`;

    // 默认名称只看当前目标目录，真正文件系统冲突仍由 Tauri 后端最终校验。
    if (!existingPaths.has(joinRelativePath(parentPath, fileName))) {
      return fileName;
    }
  }

  return "未命名.txt";
}

/** 为新建目录生成当前父目录下不冲突的默认名称。 */
function getNextAvailableFolderName(snapshot: WorkspaceSnapshot, knowledgeBaseId: string, parentPath: string) {
  const existingPaths = new Set([
    ...snapshot.folders.filter((folder) => folder.knowledgeBaseId === knowledgeBaseId).map((folder) => folder.path),
    ...getExistingFilePaths(snapshot, knowledgeBaseId),
  ]);

  for (let index = 1; index <= 999; index += 1) {
    const folderName = index === 1 ? "新建文件夹" : `新建文件夹 ${index}`;

    // 文件夹默认名称只根据目录节点判断，真正文件系统冲突仍由 Tauri 后端最终校验。
    if (!existingPaths.has(joinRelativePath(parentPath, folderName))) {
      return folderName;
    }
  }

  return "新建文件夹";
}

/** 拼接知识库内相对路径，根目录下只返回子名称。 */
function joinRelativePath(parentPath: string, childName: string) {
  return parentPath ? `${parentPath}/${childName}` : childName;
}

/** 弹窗中展示当前创建位置，根目录用明确名称避免路径为空带来的歧义。 */
function getCreateParentLabel(parentPath: string) {
  return parentPath ? `创建位置：${parentPath}` : "创建位置：根目录";
}

/** 返回新建弹窗标题。 */
function getCreateDialogTitle(kind: "markdown" | "text" | "folder") {
  if (kind === "markdown") {
    return "新建 Markdown";
  }

  if (kind === "text") {
    return "新建 TXT";
  }

  return "新建目录";
}

/** 返回新建弹窗无障碍标签。 */
function getCreateDialogAriaLabel(kind: "markdown" | "text" | "folder") {
  if (kind === "markdown") {
    return "新建 Markdown 文档";
  }

  if (kind === "text") {
    return "新建 TXT 文档";
  }

  return "新建目录";
}

/** 返回新建输入框占位文案。 */
function getCreatePlaceholder(kind: "markdown" | "text" | "folder") {
  if (kind === "markdown") {
    return "例如：会议记录";
  }

  if (kind === "text") {
    return "例如：灵感草稿";
  }

  return "例如：Projects";
}

/** 返回新建提交按钮文案。 */
function getCreateSubmitLabel(kind: "markdown" | "text" | "folder") {
  if (kind === "markdown") {
    return "创建 Markdown";
  }

  if (kind === "text") {
    return "创建 TXT";
  }

  return "创建目录";
}

/** 根据扫描报告生成状态提示，让空目录、失败文件和跳过目录都有可读反馈。 */
function buildScanNotice(snapshot: WorkspaceSnapshot, knowledgeBaseId: string) {
  const knowledgeBase = snapshot.knowledgeBases.find((item) => item.id === knowledgeBaseId);
  const report = knowledgeBase?.scanReport;

  if (!knowledgeBase) {
    return "";
  }

  if (knowledgeBase.status === "error") {
    return knowledgeBase.description;
  }

  if (!report) {
    return `已扫描「${knowledgeBase.name}」，发现 ${knowledgeBase.noteCount} 篇 Markdown、${knowledgeBase.documentCount} 个普通文档。`;
  }

  const skippedText = report.skippedDirectories.length ? `，跳过 ${report.skippedDirectories.length} 个依赖或隐藏目录` : "";
  const errorText = report.failedFileCount ? `，${report.failedFileCount} 个文件读取失败` : "";

  if (report.scannedFileCount === 0 && !report.failedFileCount) {
    return `「${knowledgeBase.name}」暂未发现支持文档${skippedText}。`;
  }

  return `已扫描「${knowledgeBase.name}」：${report.scannedFileCount} 个支持文档${errorText}${skippedText}。`;
}
