import { useEffect, useState } from "react";
import { AgentPanel } from "../agent/AgentPanel";
import { DocumentPane } from "../editor/DocumentPane";
import { EditorPane } from "../editor/EditorPane";
import { buildFileTree } from "../knowledge-base/treeUtils";
import { KnowledgeBaseSidebar } from "../knowledge-base/KnowledgeBaseSidebar";
import { SettingsDrawer } from "../settings/SettingsDrawer";
import { ConfirmDialog, type ConfirmDialogConfig } from "../shared/ConfirmDialog";
import { createContentHash, createLocalId, formatLocalDateTime } from "../shared/id";
import { logError, logInfo, logWarn } from "../shared/logger";
import {
  getActiveKnowledgeBase,
  getActiveDocument,
  getActiveNote,
  getActiveSession,
} from "../shared/selectors";
import {
  acceptProposedChange,
  attachKnowledgeBase,
  clearAppEventLogs,
  createDocument,
  createFolder,
  createNote,
  deleteAgentSkill,
  deleteDocument,
  deleteNote,
  deleteSession,
  loadDocumentPreview,
  loadAppEventLogs,
  loadAgentSkills,
  loadModelApiKeyStatus,
  loadRequestAuditLogs,
  loadUserSettings,
  loadWorkspaceState,
  openUserSkillsFolder,
  openAppLogFolder,
  removeKnowledgeBase,
  renameDocument,
  renameNote,
  rejectProposedChange,
  rescanKnowledgeBase,
  restoreSessionContext,
  runAgentTurn,
  saveAgentSkill,
  saveDocumentContent,
  saveNoteContent,
  saveNoteImageAttachments,
  saveModelApiKey,
  saveSession,
  saveUserSettings,
  selectKnowledgeBaseDirectory,
  toggleAgentSkill,
  updateSessionScope,
} from "../shared/tauriApi";
import type {
  AgentActionType,
  AgentSkill,
  AgentSession,
  AgentSessionType,
  AppEventLog,
  AppEventLogCategory,
  AppEventLogLevel,
  DocumentPreview,
  KnowledgeBase,
  MarkdownViewMode,
  ModelApiKeyStatus,
  Note,
  NoteImageAttachmentInput,
  RequestAuditLog,
  UserSettings,
  WorkspaceDocument,
  WorkspaceSnapshot,
} from "../shared/types";
import { TopBar } from "./TopBar";
import { useResizableWorkspaceLayout } from "./useResizableWorkspaceLayout";

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

/** 根据会话绑定范围生成标题，正式版本中可由用户重命名。 */
function buildSessionTitle(type: AgentSessionType, knowledgeBase: KnowledgeBase, note?: Note) {
  if (type === "note" && note) {
    return `${note.title} · 笔记助手`;
  }

  if (type === "task") {
    return `${note?.title ?? knowledgeBase.name} · 任务助手`;
  }

  return `${knowledgeBase.name}问答助手`;
}

/** 创建一条会话开场消息，明确说明本会话绑定的上下文和工具边界。 */
function buildSessionIntroMessage(sessionTitle: string, knowledgeBase: KnowledgeBase, note?: Note) {
  return {
    id: createLocalId("assistant-session"),
    role: "assistant" as const,
    action: "find" as const,
    content: note
      ? `已开启「${sessionTitle}」。我会作为知识库 Agent 助手工作；需要依据本地内容时才调用工具。`
      : `已开启「${sessionTitle}」。检索工具默认只允许访问「${knowledgeBase.name}」。`,
    toolCalls: [],
  };
}

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
  type,
  knowledgeBase,
  note,
}: {
  type: AgentSessionType;
  knowledgeBase: KnowledgeBase;
  note?: Note;
}): AgentSession {
  const title = buildSessionTitle(type, knowledgeBase, note);
  /** 会话创建时间需要长期可辨认，避免历史列表里多个“刚刚”无法区分。 */
  const createdAt = formatLocalDateTime();

  return {
    id: createLocalId(`session-${type}`),
    title,
    type,
    knowledgeBaseIds: [knowledgeBase.id],
    activeNoteId: note?.id,
    pinnedNoteIds: note ? [note.id] : [],
    messages: [buildSessionIntroMessage(title, knowledgeBase, note)],
    createdAt,
    updatedAt: createdAt,
  };
}

/** 正式工作台根组件，集中编排知识库、编辑器、Agent loop 和设置状态。 */
export function WorkspaceShell() {
  const [snapshot, setSnapshot] = useState<WorkspaceSnapshot | null>(null);
  const [userSettings, setUserSettings] = useState<UserSettings | null>(null);
  /** Agent skills 列表由后端合并内置与用户自建定义，前端只保存展示状态。 */
  const [agentSkills, setAgentSkills] = useState<AgentSkill[]>([]);
  /** 模型密钥状态只保存是否可读，不包含明文 API key。 */
  const [modelApiKeyStatus, setModelApiKeyStatus] = useState<ModelApiKeyStatus | null>(null);
  /** 首屏初始化是否仍在进行，用于区分加载中和加载失败。 */
  const [isBooting, setIsBooting] = useState(true);
  /** 首屏初始化失败原因，失败后展示重试入口而不是停留在 loading。 */
  const [bootError, setBootError] = useState("");
  const [auditLogs, setAuditLogs] = useState<RequestAuditLog[]>([]);
  /** 用户可读运行事件日志，只在设置页展示，不阻塞首屏工作台。 */
  const [appEventLogs, setAppEventLogs] = useState<AppEventLog[]>([]);
  const [searchTerm, setSearchTerm] = useState("");
  const [agentPrompt, setAgentPrompt] = useState("");
  /** 当前输入区显式选择的 skill；空字符串表示交给 Runtime 自动匹配。 */
  const [selectedSkillId, setSelectedSkillId] = useState("");
  const [collapsedFolderPaths, setCollapsedFolderPaths] = useState<Set<string>>(new Set());
  const [isSessionListOpen, setIsSessionListOpen] = useState(false);
  const [isSessionContextOpen, setIsSessionContextOpen] = useState(false);
  const [isScopeSelectorOpen, setIsScopeSelectorOpen] = useState(false);
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  const [isBusy, setIsBusy] = useState(false);
  const [busyLabel, setBusyLabel] = useState("");
  const [notice, setNotice] = useState("");
  const [editingBaseHashes, setEditingBaseHashes] = useState<Record<string, string>>({});
  const [editingBaseDocumentHashes, setEditingBaseDocumentHashes] = useState<Record<string, string>>({});
  const [dirtyNoteIds, setDirtyNoteIds] = useState<Set<string>>(new Set());
  const [dirtyDocumentIds, setDirtyDocumentIds] = useState<Set<string>>(new Set());
  const [markdownViewMode, setMarkdownViewMode] = useState<MarkdownViewMode>("edit");
  const [documentPreview, setDocumentPreview] = useState<DocumentPreview | null>(null);
  const [documentPreviewError, setDocumentPreviewError] = useState("");
  const [isDocumentPreviewLoading, setIsDocumentPreviewLoading] = useState(false);
  const [renameDialog, setRenameDialog] = useState<{ kind: "note" | "document"; id: string; fileName: string } | null>(null);
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

  useEffect(() => {
    let isMounted = true;

    void loadInitialData(() => isMounted);

    return () => {
      isMounted = false;
    };
  }, []);

  useEffect(() => {
    if (!snapshot?.activeDocumentId) {
      setDocumentPreview(null);
      setDocumentPreviewError("");
      setIsDocumentPreviewLoading(false);
      return;
    }

    const activeDocument = snapshot.documents.find((document) => document.id === snapshot.activeDocumentId);

    if (!activeDocument || activeDocument.fileType === "txt") {
      setDocumentPreview(null);
      setDocumentPreviewError("");
      setIsDocumentPreviewLoading(false);
      return;
    }

    let isMounted = true;

    setDocumentPreview(null);
    setDocumentPreviewError("");
    setIsDocumentPreviewLoading(true);

    void loadDocumentPreview(snapshot, activeDocument.id)
      .then((preview) => {
        if (isMounted) {
          setDocumentPreview(preview);
        }
      })
      .catch((error) => {
        if (isMounted) {
          setDocumentPreviewError(formatErrorMessage(error));
        }
      })
      .finally(() => {
        if (isMounted) {
          setIsDocumentPreviewLoading(false);
        }
      });

    return () => {
      isMounted = false;
    };
  }, [snapshot?.activeDocumentId, snapshot?.documents]);

  /** 加载首屏必需数据；诊断日志失败不阻断进入工作台。 */
  async function loadInitialData(shouldCommit: () => boolean = () => true) {
    setIsBooting(true);
    setBootError("");
    setNotice("");

    try {
      // 工作台快照和用户设置是首屏必需数据，必须同时成功后才能进入主界面。
      const [nextSnapshot, nextUserSettings, nextAgentSkills, nextModelApiKeyStatus] = await Promise.all([
        loadWorkspaceState(),
        loadUserSettings(),
        loadAgentSkills(),
        loadModelApiKeyStatus().catch((error) => ({
          keyReference: "cici-note-openai-compatible-api-key",
          configured: false,
          message: formatErrorMessage(error),
        })),
      ]);

      if (!shouldCommit()) {
        return;
      }

      setSnapshot(nextSnapshot);
      setUserSettings(nextUserSettings);
      setAgentSkills(nextAgentSkills);
      setModelApiKeyStatus(nextModelApiKeyStatus);
      setEditingBaseHashes(buildNoteHashMap(nextSnapshot.notes));
      setEditingBaseDocumentHashes(buildDocumentHashMap(nextSnapshot.documents));
      setIsBooting(false);

      void loadInitialDiagnosticLogs(shouldCommit);
    } catch (error) {
      if (shouldCommit()) {
        setSnapshot(null);
        setUserSettings(null);
        setAgentSkills([]);
        setAuditLogs([]);
        setAppEventLogs([]);
        setBootError(formatErrorMessage(error));
      }
    } finally {
      if (shouldCommit()) {
        setIsBooting(false);
      }
    }
  }

  /** 后台加载非首屏必需的诊断日志，失败时降级为空列表并提示用户。 */
  async function loadInitialDiagnosticLogs(shouldCommit: () => boolean = () => true) {
    try {
      const [nextAuditLogs, nextAppEventLogs] = await Promise.all([loadRequestAuditLogs(), loadAppEventLogs()]);

      if (!shouldCommit()) {
        return;
      }

      setAuditLogs(nextAuditLogs);
      setAppEventLogs(nextAppEventLogs);
    } catch (error) {
      if (shouldCommit()) {
        setAuditLogs([]);
        setAppEventLogs([]);
        setNotice(`诊断日志加载失败：${formatErrorMessage(error)}`);
      }
    }
  }

  if (isBooting) {
    return (
      <main className="loading-shell">
        <div className="brand-mark">
          <img className="brand-logo" src="/cici-note-logo.svg" alt="" />
        </div>
        <p>正在加载本地知识库工作台...</p>
      </main>
    );
  }

  if (!snapshot || !userSettings) {
    const errorMessage = bootError || "工作台初始化未完成，请重试。";

    return (
      <main className="loading-shell boot-error-shell">
        <div className="brand-mark">
          <img className="brand-logo" src="/cici-note-logo.svg" alt="" />
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
          <img className="brand-logo" src="/cici-note-logo.svg" alt="" />
        </div>
        <h1>连接一个支持文档目录，开始使用知识库 Agent 助手。</h1>
        <p>目录树会展示 Markdown、TXT、DOCX 和 PDF；Agent 写入仍只作用于确认后的 Markdown diff。</p>
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
  const activeSession = getActiveSession(currentSnapshot);
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
    const nextNoteIds = new Set(nextSnapshot.notes.map((note) => note.id));
    const nextDirtyNoteIds = new Set(Array.from(dirtyNotesToKeep).filter((noteId) => nextNoteIds.has(noteId)));
    const nextDocumentIds = new Set(nextSnapshot.documents.map((document) => document.id));
    const nextDirtyDocumentIds = new Set(
      Array.from(dirtyDocumentsToKeep).filter((documentId) => nextDocumentIds.has(documentId)),
    );

    setSnapshot(nextSnapshot);
    setEditingBaseHashes((currentHashes) => {
      const nextHashes = { ...currentHashes };

      // 新增或成功保存后的笔记需要更新保存基准；仍处于草稿状态的笔记保留原始 hash 用于冲突校验。
      nextSnapshot.notes.forEach((note) => {
        if (!nextDirtyNoteIds.has(note.id)) {
          nextHashes[note.id] = note.contentHash;
        } else if (!nextHashes[note.id]) {
          nextHashes[note.id] = note.contentHash;
        }
      });

      Object.keys(nextHashes).forEach((noteId) => {
        if (!nextNoteIds.has(noteId)) {
          delete nextHashes[noteId];
        }
      });

      return nextHashes;
    });
    setEditingBaseDocumentHashes((currentHashes) => {
      const nextHashes = { ...currentHashes };

      // TXT 文档保存成功或重扫后更新基准 hash；仍在编辑的文档保留原始 hash 做冲突检测。
      nextSnapshot.documents.forEach((document) => {
        if (!nextDirtyDocumentIds.has(document.id)) {
          nextHashes[document.id] = document.contentHash;
        } else if (!nextHashes[document.id]) {
          nextHashes[document.id] = document.contentHash;
        }
      });

      Object.keys(nextHashes).forEach((documentId) => {
        if (!nextDocumentIds.has(documentId)) {
          delete nextHashes[documentId];
        }
      });

      return nextHashes;
    });
    setDirtyNoteIds(nextDirtyNoteIds);
    setDirtyDocumentIds(nextDirtyDocumentIds);
  }

  /** 选择知识库时同步收窄 Agent 工具范围，避免跨库误检索。 */
  async function handleSelectKnowledgeBase(knowledgeBaseId: string) {
    const nextKnowledgeBase = currentSnapshot.knowledgeBases.find((knowledgeBase) => knowledgeBase.id === knowledgeBaseId);
    const nextNotes = currentSnapshot.notes.filter((note) => note.knowledgeBaseId === knowledgeBaseId);
    const nextDocuments = currentSnapshot.documents.filter((document) => document.knowledgeBaseId === knowledgeBaseId);

    if (!nextKnowledgeBase) {
      return;
    }

    const existingSession = currentSnapshot.sessions.find(
      (session) =>
        session.type === "knowledge-base" &&
        session.knowledgeBaseIds.length === 1 &&
        session.knowledgeBaseIds[0] === nextKnowledgeBase.id,
    );
    const nextSession = existingSession ?? buildAgentSession({ type: "knowledge-base", knowledgeBase: nextKnowledgeBase });
    const activatedSnapshot = {
      ...currentSnapshot,
      sessions: existingSession ? currentSnapshot.sessions : [nextSession, ...currentSnapshot.sessions],
      activeKnowledgeBaseId: knowledgeBaseId,
      activeNoteId: nextNotes[0]?.id ?? "",
      activeDocumentId: nextNotes[0] ? "" : nextDocuments[0]?.id ?? "",
      activeSessionId: nextSession.id,
    };

    beginBusy("正在切换知识库会话...");

    try {
      // 选择知识库会创建或恢复默认知识库会话，确保会话和 scope 可重启恢复。
      const nextSnapshot = existingSession
        ? await restoreSessionContext(activatedSnapshot, nextSession.id)
        : await saveSession(activatedSnapshot, nextSession);

      commitSnapshot(nextSnapshot);
      setSearchTerm("");
      setCollapsedFolderPaths(new Set());
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
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

  /** 打开 Markdown 文件时同步激活对应笔记会话，避免 Agent 使用旧文件上下文。 */
  async function handleSelectNote(noteId: string) {
    const nextNote = currentSnapshot.notes.find((note) => note.id === noteId);

    if (!nextNote) {
      return;
    }

    const nextKnowledgeBase =
      currentSnapshot.knowledgeBases.find((knowledgeBase) => knowledgeBase.id === nextNote.knowledgeBaseId) ?? activeKnowledgeBase;
    const existingSession = currentSnapshot.sessions.find(
      (session) => session.type === "note" && session.activeNoteId === nextNote.id,
    );
    const nextSession = existingSession ?? buildAgentSession({ type: "note", knowledgeBase: nextKnowledgeBase, note: nextNote });
    const activatedSnapshot = {
      ...currentSnapshot,
      sessions: existingSession ? currentSnapshot.sessions : [nextSession, ...currentSnapshot.sessions],
      activeKnowledgeBaseId: nextKnowledgeBase.id,
      activeNoteId: noteId,
      activeDocumentId: "",
      activeSessionId: nextSession.id,
    };

    beginBusy("正在切换笔记会话...");

    try {
      // 笔记会话绑定 activeNoteId，pending diff 也会留在创建它的会话中。
      const nextSnapshot = existingSession
        ? await restoreSessionContext(activatedSnapshot, nextSession.id)
        : await saveSession(activatedSnapshot, nextSession);

      commitSnapshot(nextSnapshot);
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 打开普通文档时切换到知识库级 Agent 会话，避免旧 Markdown 笔记上下文继续生效。 */
  async function handleSelectDocument(documentId: string) {
    const nextDocument = currentSnapshot.documents.find((document) => document.id === documentId);

    if (!nextDocument) {
      return;
    }

    const nextKnowledgeBase =
      currentSnapshot.knowledgeBases.find((knowledgeBase) => knowledgeBase.id === nextDocument.knowledgeBaseId) ??
      activeKnowledgeBase;
    const existingSession = currentSnapshot.sessions.find(
      (session) =>
        session.type === "knowledge-base" &&
        session.knowledgeBaseIds.length === 1 &&
        session.knowledgeBaseIds[0] === nextKnowledgeBase.id,
    );
    const nextSession = existingSession ?? buildAgentSession({ type: "knowledge-base", knowledgeBase: nextKnowledgeBase });
    const activatedSnapshot = {
      ...currentSnapshot,
      sessions: existingSession ? currentSnapshot.sessions : [nextSession, ...currentSnapshot.sessions],
      activeKnowledgeBaseId: nextKnowledgeBase.id,
      activeNoteId: "",
      activeDocumentId: documentId,
      activeSessionId: nextSession.id,
    };

    beginBusy("正在打开文档...");

    try {
      const nextSnapshot = existingSession
        ? await restoreSessionContext(activatedSnapshot, nextSession.id)
        : await saveSession(activatedSnapshot, nextSession);

      commitSnapshot({ ...nextSnapshot, activeNoteId: "", activeDocumentId: documentId });
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
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
          ? { ...document, content, updatedAt: "刚刚", contentHash: createContentHash(content) }
          : document,
      ),
    });
    setDirtyDocumentIds((currentDocumentIds) => new Set(currentDocumentIds).add(activeDocument.id));
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
        const existingSession = nextNote
          ? nextSnapshot.sessions.find((session) => session.type === "note" && session.activeNoteId === nextNote.id)
          : undefined;
        const nextSession =
          existingSession ??
          (nextNote ? buildAgentSession({ type: "note", knowledgeBase: nextKnowledgeBase, note: nextNote }) : undefined);
        const activatedSnapshot =
          nextNote && nextSession
            ? {
                ...nextSnapshot,
                sessions: existingSession ? nextSnapshot.sessions : [nextSession, ...nextSnapshot.sessions],
                activeKnowledgeBaseId: nextKnowledgeBase.id,
                activeNoteId: nextNote.id,
                activeDocumentId: "",
                activeSessionId: nextSession.id,
              }
            : nextSnapshot;
        const persistedSnapshot = nextNote && nextSession ? await saveSession(activatedSnapshot, nextSession) : activatedSnapshot;

        commitSnapshot(persistedSnapshot);
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

  /** 新建一个任务会话，绑定当前笔记和当前知识库作为上下文起点。 */
  async function handleCreateSession() {
    const nextSession = buildAgentSession({
      type: "task",
      knowledgeBase: activeKnowledgeBase,
      note: activeNote,
    });
    const nextSnapshot = {
      ...currentSnapshot,
      sessions: [nextSession, ...currentSnapshot.sessions],
      activeSessionId: nextSession.id,
    };

    beginBusy("正在创建 Agent 会话...");

    try {
      commitSnapshot(await saveSession(nextSnapshot, nextSession));
      setIsSessionListOpen(true);
      setIsSessionContextOpen(false);
      setIsScopeSelectorOpen(false);
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 切换会话时恢复它绑定的知识库、笔记和工具范围。 */
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

  /** 提交 Agent 输入，运行时会自行决定是否调用检索工具。 */
  async function handleSubmitPrompt(action: AgentActionType = "ask", presetPrompt?: string) {
    const prompt = (presetPrompt ?? agentPrompt).trim();

    // 空输入不创建消息，避免侧栏出现无意义的对话记录。
    if (!prompt) {
      return;
    }

    beginBusy("Agent 正在处理...");

    try {
      const result = await runAgentTurn(currentSnapshot, prompt, action, selectedSkillId || undefined);
      commitSnapshot(result.snapshot);
      const [nextAuditLogs, nextAppEventLogs] = await Promise.all([loadRequestAuditLogs(), loadAppEventLogs()]);

      setAuditLogs(nextAuditLogs);
      setAppEventLogs(nextAppEventLogs);
      setAgentPrompt("");
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
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
    beginBusy("正在应用 diff...");

    try {
      const nextSnapshot = await acceptProposedChange(currentSnapshot);

      commitSnapshot(nextSnapshot);
      setNotice("已应用本次 diff。");
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 取消 Agent diff，保持原始 Markdown 内容不变。 */
  async function handleRejectChange() {
    beginBusy("正在取消 diff...");

    try {
      commitSnapshot(await rejectProposedChange(currentSnapshot));
      setNotice("已取消本次 diff。");
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 保存模型、隐私和写入设置，密钥由单独入口写入系统安全存储。 */
  async function handleSaveSettings(nextSettings: UserSettings) {
    beginBusy("正在保存 Agent 设置...");

    try {
      setUserSettings(await saveUserSettings(nextSettings));
      setNotice("已保存 Agent 设置。");
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 保存用户自建 skill 后刷新列表，保证后端归一化后的 ID、name 和时间进入 UI。 */
  async function handleSaveSkill(skill: AgentSkill) {
    beginBusy("正在保存 Skill...");

    try {
      const savedSkill = await saveAgentSkill(skill);
      const nextSkills = await loadAgentSkills();

      setAgentSkills(nextSkills);
      setSelectedSkillId((currentSkillId) => (currentSkillId === skill.id ? savedSkill.id : currentSkillId));
      setNotice("已保存 Skill。");

      return savedSkill;
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
      throw error;
    } finally {
      endBusy();
    }
  }

  /** 启停 skill 或切换自动触发状态，禁用后也会清除输入区的显式选择。 */
  async function handleToggleSkill(skillId: string, enabled: boolean, allowAutoInvoke?: boolean) {
    beginBusy("正在更新 Skill...");

    try {
      await toggleAgentSkill(skillId, enabled, allowAutoInvoke);
      const nextSkills = await loadAgentSkills();

      setAgentSkills(nextSkills);
      if (!enabled && selectedSkillId === skillId) {
        setSelectedSkillId("");
      }
      setNotice("已更新 Skill。");
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
      throw error;
    } finally {
      endBusy();
    }
  }

  /** 删除用户自建 skill；内置 skill 由后端拒绝删除并保留为可禁用项。 */
  async function handleDeleteSkill(skillId: string) {
    beginBusy("正在删除 Skill...");

    try {
      const nextSkills = await deleteAgentSkill(skillId);

      setAgentSkills(nextSkills);
      if (selectedSkillId === skillId) {
        setSelectedSkillId("");
      }
      setNotice("已删除 Skill。");
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
      throw error;
    } finally {
      endBusy();
    }
  }

  /** 打开 Cici Note 用户 Skills 文件夹；浏览器开发态只展示 mock 路径。 */
  async function handleOpenUserSkillsFolder() {
    beginBusy("正在打开用户 Skills 文件夹...");

    try {
      const skillsFolderPath = await openUserSkillsFolder();

      setNotice(`用户 Skills 文件夹：${skillsFolderPath}`);
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
      throw error;
    } finally {
      endBusy();
    }
  }

  /** 保存 BYOK API key；桌面端写入系统 keyring，避免明文进入 SQLite。 */
  async function handleSaveApiKey(apiKey: string) {
    const trimmedApiKey = apiKey.trim();

    if (!trimmedApiKey) {
      setNotice("API key 不能为空。");
      return;
    }

    beginBusy("正在保存模型密钥...");

    try {
      const nextModelApiKeyStatus = await saveModelApiKey(trimmedApiKey);

      setModelApiKeyStatus(nextModelApiKeyStatus);
      setNotice(nextModelApiKeyStatus.message);
    } catch (error) {
      const message = formatErrorMessage(error);

      setNotice(message);
      throw new Error(message);
    } finally {
      endBusy();
    }
  }

  /** 重新读取最近审计日志，便于设置页查看最新模型和工具调用边界。 */
  async function handleRefreshAuditLogs() {
    beginBusy("正在刷新审计日志...");

    try {
      setAuditLogs(await loadRequestAuditLogs());
    } catch (error) {
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

  /** 重新读取最近应用事件日志，支持设置页级别和分类筛选。 */
  async function handleRefreshAppEventLogs(filters?: { level?: AppEventLogLevel | ""; category?: AppEventLogCategory | "" }) {
    beginBusy("正在刷新运行日志...");

    try {
      setAppEventLogs(await loadAppEventLogs({ limit: 100, ...filters }));
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 清空用户可读事件日志后立即重载列表，保留桌面端文件诊断日志。 */
  async function handleClearAppEventLogs(filters?: { level?: AppEventLogLevel | ""; category?: AppEventLogCategory | "" }) {
    beginBusy("正在清空运行日志...");

    try {
      await clearAppEventLogs();
      setAppEventLogs(await loadAppEventLogs({ limit: 100, ...filters }));
      setNotice("已清空应用事件日志。");
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 打开系统 app log 目录，便于用户附带文件诊断日志排查问题。 */
  async function handleOpenAppLogFolder() {
    beginBusy("正在打开应用日志目录...");

    try {
      const logFolderPath = await openAppLogFolder();

      setNotice(`应用日志目录：${logFolderPath}`);
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  return (
    <div className="app-shell">
      <TopBar
        activeKnowledgeBase={activeKnowledgeBase}
        knowledgeBaseCount={currentSnapshot.knowledgeBases.length}
        onOpenSettings={handleOpenSettings}
      />
      <main
        className={`workspace-grid ${resizingPane ? "is-resizing" : ""}`}
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
          onRenameDocument={openRenameDocumentDialog}
          onDeleteDocument={handleDeleteDocument}
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
            onRequestRewrite={() => handleSubmitPrompt("rewrite", "改写当前笔记的核心段落")}
            onRenameNote={() => openRenameDialog()}
            onDeleteNote={() => handleDeleteNote()}
            onAcceptChange={handleAcceptChange}
            onRejectChange={handleRejectChange}
          />
        )}
        <div
          className={`workspace-resizer ${resizingPane === "agent" ? "active" : ""}`}
          {...getSeparatorProps("agent")}
        />
        <AgentPanel
          sessions={currentSnapshot.sessions}
          activeSession={activeSession}
          activeKnowledgeBase={activeKnowledgeBase}
          knowledgeBases={currentSnapshot.knowledgeBases}
          notes={currentSnapshot.notes}
          prompt={agentPrompt}
          skills={agentSkills}
          selectedSkillId={selectedSkillId}
          isBusy={isBusy}
          isSessionListOpen={isSessionListOpen}
          isSessionContextOpen={isSessionContextOpen}
          isScopeSelectorOpen={isScopeSelectorOpen}
          onToggleSessionList={() => {
            setIsSessionListOpen((current) => !current);
            setIsSessionContextOpen(false);
            setIsScopeSelectorOpen(false);
          }}
          onToggleSessionContext={() => {
            setIsSessionContextOpen((current) => !current);
            setIsSessionListOpen(false);
            setIsScopeSelectorOpen(false);
          }}
          onToggleScopeSelector={() => {
            setIsScopeSelectorOpen((current) => !current);
            setIsSessionListOpen(false);
            setIsSessionContextOpen(false);
          }}
          onCreateSession={handleCreateSession}
          onSelectSession={handleSelectSession}
          onDeleteSession={handleDeleteSession}
          onToggleScopeKnowledgeBase={handleToggleScopeKnowledgeBase}
          onPromptChange={setAgentPrompt}
          onSelectedSkillChange={setSelectedSkillId}
          onSubmitPrompt={() => handleSubmitPrompt("ask")}
        />
      </main>
      {isSettingsOpen && (
        <SettingsDrawer
          knowledgeBases={currentSnapshot.knowledgeBases}
          activeKnowledgeBaseId={activeKnowledgeBase.id}
          settings={userSettings}
          skills={agentSkills}
          modelApiKeyStatus={modelApiKeyStatus}
          auditLogs={auditLogs}
          appEventLogs={appEventLogs}
          isBusy={isBusy}
          onSelectKnowledgeBase={handleSelectKnowledgeBase}
          onAddKnowledgeBase={handleAddKnowledgeBase}
          onRescanKnowledgeBase={handleRescanKnowledgeBase}
          onRemoveKnowledgeBase={handleRemoveKnowledgeBase}
          onSaveSettings={handleSaveSettings}
          onSaveSkill={handleSaveSkill}
          onToggleSkill={handleToggleSkill}
          onDeleteSkill={handleDeleteSkill}
          onOpenUserSkillsFolder={handleOpenUserSkillsFolder}
          onSaveApiKey={handleSaveApiKey}
          onRefreshAuditLogs={handleRefreshAuditLogs}
          onRefreshAppEventLogs={handleRefreshAppEventLogs}
          onClearAppEventLogs={handleClearAppEventLogs}
          onOpenAppLogFolder={handleOpenAppLogFolder}
          onClose={() => setIsSettingsOpen(false)}
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

/** 为笔记建立当前文件 hash 映射，保存草稿时用于外部修改冲突校验。 */
function buildNoteHashMap(notes: Note[]) {
  return Object.fromEntries(notes.map((note) => [note.id, note.contentHash]));
}

/** 为普通文档建立当前文件 hash 映射，保存 txt 草稿时用于外部修改冲突校验。 */
function buildDocumentHashMap(documents: WorkspaceDocument[]) {
  return Object.fromEntries(documents.map((document) => [document.id, document.contentHash]));
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
