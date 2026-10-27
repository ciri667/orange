import { useCallback, useEffect, useRef, useState } from "react";
import { AgentPanel } from "../agent/AgentPanel";
import { OperationApprovalDialog } from "../agent/OperationApprovalDialog";
import { BrowserPanel } from "../browser/BrowserPanel";
import { useBrowserWorkspace } from "../browser/useBrowserWorkspace";
import { TerminalPanel } from "../terminal/TerminalPanel";
import { useTerminals } from "../terminal/useTerminals";
import { RightPanel, type RightPanelTabItem } from "./RightPanel";
import { rightTabId } from "./rightPanelModel";
import { useRightPanelTabs } from "./useRightPanelTabs";
import { useRightPanelActions } from "./useRightPanelActions";
import { DocumentPane } from "../editor/DocumentPane";
import { EditorTabBar, type EditorTabBarItem } from "../editor/EditorTabBar";
import { EditorPane } from "../editor/EditorPane";
import { buildFileTree } from "../knowledge-base/treeUtils";
import { KnowledgeBaseSidebar } from "../knowledge-base/KnowledgeBaseSidebar";
import { SettingsDrawer } from "../settings/SettingsDrawer";
import { BootCopy, BootErrorMessage, BootScreen, BootTitle } from "../shared/BootScreen";
import { Button } from "../shared/Button";
import { ConfirmDialog, type ConfirmDialogConfig } from "../shared/ConfirmDialog";
import { ModalBackdrop, ModalForm } from "../shared/Modal";
import { OperationNotice } from "../shared/OperationNotice";
import { fieldControlClassName, fieldLabelClassName, sectionLabelClassName } from "../shared/ui";
import { logWarn } from "../shared/logger";
import {
  getActiveKnowledgeBase,
  getActiveDocument,
  getActiveNote,
} from "../shared/selectors";
import { listenScheduledJobUpdated, loadSessions, restoreSessionContext, saveWorkspaceEditorState } from "../shared/tauriApi";
import { SchedulesView } from "../schedules/SchedulesView";
import type {
  AgentContextCompactionReport,
  DocumentHistoryTargetKind,
  EditorFileTab,
  MarkdownViewMode,
  WorkspaceEditorState,
  WorkspaceSnapshot,
} from "../shared/types";
import { DocumentHistoryDialog } from "./DocumentHistoryDialog";
import { getRegion } from "./collapsibleRegions";
import type { RegionItemId } from "./collapsibleRegions";
import { RegionPreviewNote } from "./RegionPreviewNote";
import { RegionRail } from "./RegionRail";
import { WorkspaceTopBar } from "./WorkspaceTopBar";
import {
  getCreateDialogAriaLabel,
  getCreateDialogTitle,
  getCreateParentLabel,
  getCreatePlaceholder,
  getCreateSubmitLabel,
} from "./fileNameUtils";
import { DRAFT_SESSION_ID, buildDraftAgentSession, buildMentionableFiles, resolveActivePanelSession, resolveSessionKnowledgeBase, resolveNewSessionSecurityLevel } from "./sessionUtils";
import { PLAN_EXECUTION_PROMPT } from "../shared/planMode";
import { useAgentTurn } from "./useAgentTurn";
import { useAgentTurnDraft } from "./useAgentTurnDraft";
import { useDocumentPreview } from "./useDocumentPreview";
import { useEditorActions } from "./useEditorActions";
import { useKnowledgeBaseActions } from "./useKnowledgeBaseActions";
import { useReviewActions } from "./useReviewActions";
import { useReviewChangeLogger } from "./useReviewChangeLogger";
import { useResizableWorkspaceLayout } from "./useResizableWorkspaceLayout";
import { useSessionActions } from "./useSessionActions";
import { useWorkspaceBootData } from "./useWorkspaceBootData";
import { useWorkspaceDrafts } from "./useWorkspaceDrafts";
import { useWorkspaceSettingsActions } from "./useWorkspaceSettingsActions";
import { useCompactWorkspaceView } from "./useCompactWorkspaceView";

/** 将未知异常统一转换为可展示文案，避免启动错误页渲染空对象。 */
function formatErrorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

/** 等待用户确认的工作台操作，确认后才执行真实文件或会话变更。 */
interface PendingConfirmation extends ConfirmDialogConfig {
  onConfirm: () => Promise<void> | void;
  /** 第三动作用于放弃草稿等有意但非主确认的操作。 */
  onThirdAction?: () => Promise<void> | void;
}

export function WorkspaceShell() {
  const compactView = useCompactWorkspaceView();
  /** 左侧目录搜索词，只影响当前前端文件树过滤，不写入持久化。 */
  const [searchTerm, setSearchTerm] = useState("");
  /** 目录树折叠状态由前端维护，切换知识库、重扫或恢复会话时重置。 */
  const [collapsedFolderPaths, setCollapsedFolderPaths] = useState<Set<string>>(new Set());
  /** 当前会话上下文浮层开关，避免长消息列表挤占主输入区。 */
  const [isSessionContextOpen, setIsSessionContextOpen] = useState(false);
  /** 会话工具范围选择器开关，用于多知识库 scope 管理。 */
  const [isScopeSelectorOpen, setIsScopeSelectorOpen] = useState(false);
  /** 设置抽屉打开状态，打开时会刷新非阻塞诊断日志。 */
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  /** 主区切到定时任务视图时隐藏编辑器。 */
  const [isSchedulesOpen, setIsSchedulesOpen] = useState(false);
  /** 全局忙碌状态覆盖文件、会话、设置和日志刷新操作；用计数避免切会话清掉 Agent 回合。 */
  const [isBusy, setIsBusy] = useState(false);
  const busyCountRef = useRef(0);
  /** 忙碌状态文案只展示当前操作类型，不包含路径、密钥或请求内容。 */
  const [busyLabel, setBusyLabel] = useState("");
  /** 顶部/侧栏轻量通知，展示用户操作结果和可恢复错误。 */
  const [notice, setNotice] = useState("");
  /** 最近一次手动整理上下文的回执；只在上下文浮层里就地展示，不占用全局 notice。 */
  const [contextCompaction, setContextCompaction] = useState<AgentContextCompactionReport | null>(null);
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
    rememberCleanNote,
    rememberCleanDocument,
    discardWorkspaceDraft,
  } = useWorkspaceDrafts();
  /** Markdown 编辑区视图模式，保持编辑/预览切换不影响文件内容。 */
  const [markdownViewMode, setMarkdownViewMode] = useState<MarkdownViewMode>("edit");
  /** 编辑区已打开文件的展示顺序；通过独立编辑器会话恢复，不混入领域快照。 */
  const [openFileTabs, setOpenFileTabs] = useState<EditorFileTab[]>([]);
  /** 启动恢复完成后才允许写回，避免首帧空标签覆盖已持久化的编辑器会话。 */
  const [isEditorSessionInitialized, setIsEditorSessionInitialized] = useState(false);
  /** 启动回退只执行一次；避免用户在运行期关闭最后一个标签后被立刻重新打开。 */
  const isStartupFallbackDoneRef = useRef(false);
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
  /** 主工作台三栏布局：知识库、编辑区、可停靠 Agent；负责拖拽分隔条和本机持久化。 */
  const {
    workspaceRef,
    gridTemplateColumns,
    resizingPane,
    getSeparatorProps,
    regionOpen,
    activeItems,
    regionPreviewWidth,
    toggleRegion,
    setRegionOpen,
    setRegionActiveItem,
    agentOpen,
    setAgentOpen,
  } = useResizableWorkspaceLayout();
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
    imCredentialByProvider,
    setImCredentialByProvider,
    imGatewayByProvider,
    setImGatewayByProvider,
    weixinLoginStatus,
    setWeixinLoginStatus,
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
    onEditorStateInitialized: (editorState: WorkspaceEditorState) => {
      setOpenFileTabs(editorState.openTabs);
      setIsEditorSessionInitialized(true);
    },
    onNoticeChange: setNotice,
  });
  const browserEnabled = userSettings?.browser?.enabled ?? false;
  const setRightOpen = useCallback((open: boolean) => { setAgentOpen(open); if (!open && compactView.singlePane) compactView.showPane("editor"); }, [setAgentOpen, compactView.singlePane, compactView.showPane]);
  const rightTabs = useRightPanelTabs(snapshot, setSnapshot, setRightOpen);
  const selectedRightTab = rightTabs.tabs.find((tab) => rightTabId(tab) === rightTabs.activeId);
  const rightVisible = agentOpen && (!compactView.singlePane || compactView.pane === "agent");
  const browserWorkspace = useBrowserWorkspace(selectedRightTab?.kind === "browser" ? selectedRightTab.id : null, rightVisible && browserEnabled && selectedRightTab?.kind === "browser");
  const terminals = useTerminals();
  /** 显式打开内容才展开右栏；后台网页事件不会经过此入口。 */
  function revealRightPanel() { setAgentOpen(true); compactView.showPane("agent"); }
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listenScheduledJobUpdated((payload) => {
      const status = payload.job.lastStatus;
      if (status === "ok") {
        setNotice(`定时任务「${payload.job.name}」已完成。`);
      } else if (status === "failed" || status === "blocked") {
        setNotice(
          `定时任务「${payload.job.name}」未成功${payload.job.lastError ? `：${payload.job.lastError}` : "。"}`,
        );
      }
      setSnapshot((current) => {
        if (!current || !payload.sessionId) {
          return current;
        }
        void loadSessions(current).then((sessions) => {
          setSnapshot((latest) => (latest ? { ...latest, sessions } : latest));
        });
        return current;
      });
    }).then((stop) => {
      unlisten = stop;
    });
    return () => unlisten?.();
  }, [setSnapshot]);
  useReviewChangeLogger(snapshot);
  /** Agent 输入草稿按会话隔离，切走再切回时保留该会话的模型和未发送文字。 */
  const {
    messageScrollPositions,
    agentPrompt,
    setAgentPrompt,
    turnModelSelection,
    setTurnModelSelection,
    explicitSkillIds,
    setExplicitSkillIds,
    mentionedFileIds,
    setMentionedFileIds,
    draftImages,
    setDraftImages,
  } = useAgentTurnDraft(snapshot?.activeSessionId || DRAFT_SESSION_ID);
  /** 只读文档预览 hook 负责异步加载和错误状态，TXT 仍由可编辑正文面板处理。 */
  const { documentPreview, documentPreviewError, isDocumentPreviewLoading } = useDocumentPreview(snapshot);

  useEffect(() => {
    if (!snapshot || !isEditorSessionInitialized) {
      return;
    }

    const activeTab = snapshot.activeDocumentId
      ? { kind: "document" as const, id: snapshot.activeDocumentId }
      : snapshot.activeNoteId
        ? { kind: "note" as const, id: snapshot.activeNoteId }
        : undefined;
    const editorState: WorkspaceEditorState = {
      activeKnowledgeBaseId: snapshot.activeKnowledgeBaseId,
      openTabs: openFileTabs,
      activeTab,
      // 后端会覆盖该字段为可信的本地时间，前端只提供完整传输形状。
      updatedAt: "",
    };
    // 标签点击和内容编辑可能连续触发快照更新；防抖降低 SQLite 写入频率且不影响交互。
    const timer = window.setTimeout(() => {
      void saveWorkspaceEditorState(editorState).catch((error) => {
        logWarn("保存编辑器会话失败，当前会话不受影响。", {
          category: "frontend",
          event: "workspace_editor_session_save",
          status: "failed",
          metadata: { openTabCount: editorState.openTabs.length, hasActiveTab: Boolean(editorState.activeTab) },
          error,
        });
      });
    }, 500);

    return () => window.clearTimeout(timer);
  }, [isEditorSessionInitialized, openFileTabs, snapshot?.activeDocumentId, snapshot?.activeKnowledgeBaseId, snapshot?.activeNoteId]);

  useEffect(() => {
    if (!snapshot || openFileTabs.length) {
      return;
    }

    // 首次加载沿用后端给出的当前焦点，避免启动后出现与旧版不一致的空编辑区。
    const initialTab = snapshot.activeDocumentId
      ? { kind: "document" as const, id: snapshot.activeDocumentId }
      : snapshot.activeNoteId
        ? { kind: "note" as const, id: snapshot.activeNoteId }
        : null;

    if (initialTab) {
      setOpenFileTabs([initialTab]);
    }
  }, [snapshot, openFileTabs.length]);

  useEffect(() => {
    if (!snapshot || !isEditorSessionInitialized || isStartupFallbackDoneRef.current) {
      return;
    }

    isStartupFallbackDoneRef.current = true;

    // 恢复的编辑器会话没有标签时（例如上次关闭了全部标签），启动后回退打开当前知识库的首个文件。
    // 该回退只在启动时执行一次，避免用户在运行期关闭最后一个标签后被立刻重新打开。
    if (openFileTabs.length) {
      return;
    }

    const knowledgeBaseNotes = snapshot.notes.filter((note) => note.knowledgeBaseId === snapshot.activeKnowledgeBaseId);
    const knowledgeBaseDocuments = snapshot.documents.filter((document) => document.knowledgeBaseId === snapshot.activeKnowledgeBaseId);
    const fallbackNote = knowledgeBaseNotes[0];
    const fallbackDocument = fallbackNote ? undefined : knowledgeBaseDocuments[0];

    if (!fallbackNote && !fallbackDocument) {
      return;
    }

    // 编辑区正文只读快照焦点，这里必须同时提交快照，否则标签栏有内容而右侧仍是空态。
    // commitSnapshot 会把 openFileTabs 过滤成新快照里仍存在的文件，因此先加入标签再提交。
    setOpenFileTabs([fallbackNote ? { kind: "note", id: fallbackNote.id } : { kind: "document", id: fallbackDocument!.id }]);
    commitSnapshot({
      ...snapshot,
      activeNoteId: fallbackNote?.id ?? "",
      activeDocumentId: fallbackNote ? "" : fallbackDocument!.id,
    });
  }, [snapshot, isEditorSessionInitialized, openFileTabs.length]);

  /** 设置动作 hook 统一处理保存、凭证、Skills 和诊断日志刷新，复用原有 Tauri API。 */
  const {
    handleSaveSettings,
    handleSaveImSettings,
    handleSaveKnowledgeBaseMemory,
    handleDeleteKnowledgeBaseMemory,
    handleSaveImSecret,
    handleStartImGateway,
    handleStopImGateway,
    handleRefreshImStatus,
    handleStartWeixinLogin,
    handleCancelWeixinLogin,
    handleSaveSkill,
    handleInstallSkill,
    handleToggleSkill,
    handleDeleteSkill,
    handleOpenUserSkillsFolder,
    handleRevealApiKey,
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
    imCredentialByProvider,
    imGatewayByProvider,
    setUserSettings,
    setImSettings,
    setAgentSkills,
    setModelApiKeyStatuses,
    setImCredentialByProvider,
    setImGatewayByProvider,
    setWeixinLoginStatus,
    setKnowledgeBaseMemories,
    setAuditLogs,
    setAppEventLogs,
  });
  const {
    handleSelectKnowledgeBase,
    handleAddKnowledgeBase,
    handleToggleFolder,
    expandFolderPaths,
    handleRescanKnowledgeBase,
    handleRemoveKnowledgeBase,
  } = useKnowledgeBaseActions({
    snapshot,
    userSettings,
    beginBusy,
    endBusy,
    setNotice,
    commitSnapshot,
    requestConfirmation,
    setSearchTerm,
    setCollapsedFolderPaths,
    setOpenFileTabs,
    setIsSettingsOpen,
    dirtyNoteIds,
    dirtyDocumentIds,
  });
  const {
    activateEditorTab,
    handleSelectNote,
    handleSelectDocument,
    handleContentChange,
    handlePasteImages,
    handleDocumentContentChange,
    openNoteHistory,
    openDocumentHistory,
    handleHistoryRestored,
    openCreateDialog,
    handleSubmitCreate,
    handleSaveActiveNote,
    handleSaveActiveDocument,
    closeEditorTab,
    handleExportActiveFile,
    openRenameDialog,
    openRenameDocumentDialog,
    handleSubmitRename,
    handleDeleteNote,
    handleDeleteDocument,
    handleCreateOrOpenProjectInstruction,
  } = useEditorActions({
    snapshot,
    userSettings,
    beginBusy,
    endBusy,
    setNotice,
    commitSnapshot,
    requestConfirmation,
    setSnapshot,
    setDirtyNoteIds,
    setDirtyDocumentIds,
    dirtyNoteIds,
    dirtyDocumentIds,
    editingBaseHashes,
    editingBaseDocumentHashes,
    rememberCleanNote,
    rememberCleanDocument,
    discardWorkspaceDraft,
    openFileTabs,
    setOpenFileTabs,
    historyDialog,
    setHistoryDialog,
    renameDialog,
    setRenameDialog,
    createDialog,
    setCreateDialog,
    setSearchTerm,
    setMarkdownViewMode,
    setCollapsedFolderPaths,
    isBusy,
    expandFolderPaths,
  });
  const {
    liveTurn,
    queuedFollowUp,
    queuedFollowUpInList,
    isCurrentSessionBusy,
    builtinCommands,
    builtinCommandNotice,
    dismissBuiltinCommandNotice,
    runBuiltinCommand,
    inFlightSessionIds,
    queuedSessionIds,
    handleClearQueuedFollowUp,
    handleAbortTurn,
    handlePrepareSessionForDelete,
    handleSubmitPrompt,
    handleEditUserMessageAndRerun,
    handleApplyChangeSet,
    handleRejectChangeSet,
    handleToggleSkillChangeOperation,
  } = useAgentTurn({
    snapshot,
    userSettings,
    beginBusy,
    endBusy,
    setNotice,
    commitSnapshot,
    requestConfirmation,
    agentPrompt,
    setAgentPrompt,
    turnModelSelection,
    explicitSkillIds,
    setExplicitSkillIds,
    mentionedFileIds,
    setMentionedFileIds,
    draftImages,
    setDraftImages,
    setAuditLogs,
    setAppEventLogs,
    dirtyNoteIds,
    dirtyDocumentIds,
    setContextCompaction,
  });
  const {
    handleCreateSession,
    handleSelectSession,
    handleDeleteSession,
    handleToggleScopeKnowledgeBase,
    handleSetSessionModelSelection,
    handleCompactAgentContext,
    handleToggleSessionContext,
    handleToggleScopeSelector,
    handleToggleAgentPanel,
    handleSessionSecurityLevelChange,
    handleSessionPlanModeChange,
    handleRenameSession,
    handleTogglePinSession,
    handleToggleArchiveSession,
  } = useSessionActions({
    snapshot,
    userSettings,
    beginBusy,
    endBusy,
    setNotice,
    commitSnapshot,
    requestConfirmation,
    setSearchTerm,
    setCollapsedFolderPaths,
    setIsSessionContextOpen,
    setIsScopeSelectorOpen,
    isSessionContextOpen,
    isScopeSelectorOpen,
    agentOpen,
    setAgentOpen,
    setUserSettings,
    setContextCompaction,
    prepareSessionForDelete: handlePrepareSessionForDelete,
    onSessionOpened: (id, activate = true) => { rightTabs.openTab({ kind: "session", id }, activate); if (activate) revealRightPanel(); },
    capturePanelFocus: rightTabs.captureFocus,
  });
  const { handleAddReviewComment, handleSubmitReviewComments, handleAcceptChange, handleRejectChange } = useReviewActions({
    snapshot,
    userSettings,
    beginBusy,
    endBusy,
    setNotice,
    commitSnapshot,
    requestConfirmation,
    setAppEventLogs,
    handleSubmitPrompt,
  });

  const rightActions = useRightPanelActions({ panel: rightTabs, browser: browserWorkspace, terminals, snapshot, browserEnabled, reveal: revealRightPanel, showSettings: () => setIsSettingsOpen(true), selectSession: handleSelectSession, createSession: handleCreateSession, confirm: requestConfirmation, notice: setNotice });

  if (isBooting) {
    return (
      <BootScreen>
        <div className="brand-mark">
          <img className="brand-logo" src="/orange-logo.svg" alt="" />
        </div>
        <p>正在加载本地知识库工作台...</p>
      </BootScreen>
    );
  }

  if (!snapshot || !userSettings || !imSettings) {
    const errorMessage = bootError || "工作台初始化未完成，请重试。";

    return (
      <BootScreen variant="error">
        <div className="brand-mark">
          <img className="brand-logo" src="/orange-logo.svg" alt="" />
        </div>
        <p>本地知识库工作台加载失败</p>
        <BootErrorMessage>{errorMessage}</BootErrorMessage>
        <Button variant="primary" size="compact" onClick={() => void loadInitialData()}>
          重试
        </Button>
      </BootScreen>
    );
  }

  /** 已加载的工作台快照，供事件闭包使用，避免 nullable state 进入业务逻辑。 */
  const currentSnapshot = snapshot;
  /** 已加载的用户设置，同上；收起态预览会跨函数读取，需要一份非空引用。 */
  const currentSettings = userSettings;

  if (!currentSnapshot.knowledgeBases.length) {
    return (
      <BootScreen variant="empty">
        <div className="brand-mark">
          <img className="brand-logo" src="/orange-logo.svg" alt="" />
        </div>
        <BootTitle>连接一个支持文档目录，开始使用知识库 Agent 助手。</BootTitle>
        <BootCopy>目录树会展示 Markdown、TXT、DOCX、PDF 和图片。新会话默认只读，需要修改文件时可切换权限并审阅变更。</BootCopy>
        <OperationNotice className="max-w-[620px]" isBusy={isBusy} busyLabel={busyLabel} notice={notice} />
        <Button variant="primary" onClick={handleAddKnowledgeBase} disabled={isBusy}>
          添加第一个知识库
        </Button>
      </BootScreen>
    );
  }

  const activeKnowledgeBase = getActiveKnowledgeBase(currentSnapshot);
  const persistedActiveSession = resolveActivePanelSession(currentSnapshot);
  const sessionKnowledgeBase = resolveSessionKnowledgeBase(currentSnapshot, persistedActiveSession) ?? activeKnowledgeBase;
  const activeSession = persistedActiveSession ?? buildDraftAgentSession(
    activeKnowledgeBase,
    resolveNewSessionSecurityLevel(userSettings.agentSecurity.defaultLevel),
  );
  const activeDocument = getActiveDocument(currentSnapshot);
  const activeNote = getActiveNote(currentSnapshot);
  /** 当前会话可 @ 的文件仅来自既有工具授权范围，不因显式材料扩大权限。 */
  const mentionableFiles = buildMentionableFiles(currentSnapshot, activeSession);
  const isActiveNoteDirty = activeNote ? dirtyNoteIds.has(activeNote.id) : false;
  const isActiveDocumentDirty = activeDocument ? dirtyDocumentIds.has(activeDocument.id) : false;
  /** 从快照派生标签展示信息，避免在临时标签状态中缓存可能过期的标题或文件类型。 */
  const editorTabs = openFileTabs.flatMap<EditorTabBarItem>((tab) => {
    if (tab.kind === "note") {
      const note = currentSnapshot.notes.find((item) => item.id === tab.id);

      return note ? [{ ...tab, title: note.title, isDirty: dirtyNoteIds.has(note.id) }] : [];
    }

    const document = currentSnapshot.documents.find((item) => item.id === tab.id);

    return document
      ? [{ ...tab, title: document.title, fileType: document.fileType, isDirty: document.fileType === "txt" && dirtyDocumentIds.has(document.id) }]
      : [];
  });
  /** 当前激活 ID 仍是后端预览与既有编辑器的唯一来源，标签栏只负责呈现和切换。 */
  const activeEditorTab: EditorFileTab | null = activeDocument
    ? { kind: "document", id: activeDocument.id }
    : activeNote
      ? { kind: "note", id: activeNote.id }
      : null;
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
    busyCountRef.current += 1;
    setIsBusy(true);
    setBusyLabel(label);
    setNotice("");
  }

  /** 统一结束忙碌状态；有嵌套操作时保持忙碌，避免切会话清掉 Agent 回合。 */
  function endBusy() {
    busyCountRef.current = Math.max(0, busyCountRef.current - 1);
    if (busyCountRef.current === 0) {
      setIsBusy(false);
      setBusyLabel("");
    }
  }

  /** 打开应用内确认弹窗，调用方只在用户确认后执行真实副作用。 */
  function requestConfirmation(
    config: ConfirmDialogConfig,
    onConfirm: () => Promise<void> | void,
    onThirdAction?: () => Promise<void> | void,
  ) {
    setPendingConfirmation({
      cancelLabel: "取消",
      tone: "danger",
      ...config,
      onConfirm,
      onThirdAction,
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
    // 外部删除、重扫或移除知识库后，不能保留已经无法解析的标签引用。
    setOpenFileTabs((currentTabs) =>
      currentTabs.filter((tab) =>
        tab.kind === "note"
          ? nextSnapshot.notes.some((note) => note.id === tab.id)
          : nextSnapshot.documents.some((document) => document.id === tab.id),
      ),
    );
  }

  /** 打开设置抽屉时刷新非阻塞诊断信息，避免展示过旧的日志列表。 */
  function handleOpenSettings() {
    setIsSettingsOpen(true);
    // 设置抽屉不是一个可内嵌进左栏的分块，收起态下用图标高亮标出它已被打开。
    setRegionActiveItem("sidebar", "settings");
    void loadInitialDiagnosticLogs();
  }

  /**
   * 收起态图标栏点击左栏条目：只切板块，不展开左栏。
   *
   * 展开左栏是顶栏收展按钮的职责：图标栏点击后停留在「收起态 + 悬浮浮层」里，
   * 浮层里可以就地操作，指针离开就收回。
   * 「设置」和「定时任务」不是左栏内的分块，只切界面，不改左栏的「会话 / 文件」分块。
   */
  function handleSelectSidebarItem(itemId: RegionItemId) {
    if (itemId === "settings") {
      handleOpenSettings();
      return;
    }

    if (itemId === "schedules") {
      setIsSchedulesOpen(true);
      return;
    }

    setIsSchedulesOpen(false);
    setRegionActiveItem("sidebar", itemId);
  }

  /**
   * 悬浮浮层里的左栏内容。
   *
   * 会话和文件是一对互斥分块，读共享的 activeItems.sidebar 而不是悬浮的那个图标 id：
   * 在浮层内点「文件」会写共享状态，浮层就地换成文件树，和展开后的行为一致。
   * 定时任务和设置不内嵌在左栏里，各自给出对应内容。
   */
  function renderSidebarPreview(itemId: RegionItemId) {
    if (itemId === "schedules") {
      return (
        <SchedulesView
          knowledgeBases={currentSnapshot.knowledgeBases}
          activeKnowledgeBaseId={activeKnowledgeBase.id}
          isBusy={isBusy}
          onBusy={async (label, task) => {
            beginBusy(label);
            try {
              await task();
            } catch (error) {
              setNotice(formatErrorMessage(error));
            } finally {
              endBusy();
            }
          }}
          onNotice={setNotice}
          onOpenSession={(sessionId) => {
            setIsSchedulesOpen(true);
            setRegionOpen("sidebar", true);
            void handleSelectSession(sessionId);
          }}
        />
      );
    }

    if (itemId === "settings") {
      return (
        <RegionPreviewNote
          title="设置"
          description="模型与密钥、知识库、IM 接入、技能、日志和写入确认策略。点击「展开」打开设置面板。"
        />
      );
    }

    // 会话 / 文件 共用展开态侧栏组件，这里只把 pane 定到共享状态上。
    return (
      <KnowledgeBaseSidebar
        compactPreview
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
        activePane={activeItems.sidebar === "files" ? "files" : "sessions"}
        // 写回同一份布局状态：在浮层里点「文件」会切到文件树，浮层内容就地跟着变。
        onActivePaneChange={(nextPane) => setRegionActiveItem("sidebar", nextPane)}
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
        onCreateProjectInstruction={() => handleCreateOrOpenProjectInstruction()}
        onRefreshKnowledgeBase={handleRescanKnowledgeBase}
        onCreateSession={handleCreateSession}
        onOpenSettings={handleOpenSettings}
        sessions={currentSnapshot.sessions}
        activeSessionId={activeSession.id}
        inFlightSessionIds={inFlightSessionIds}
        queuedSessionIds={queuedSessionIds}
        onSelectSession={handleSelectSession}
        onDeleteSession={handleDeleteSession}
        onRenameSession={handleRenameSession}
        onTogglePinSession={handleTogglePinSession}
        onToggleArchiveSession={handleToggleArchiveSession}
      />
    );
  }

  /**
   * 悬浮浮层里的 Agent 协作区：复用展开态面板的同一组 props 渲染一份可操作的实例。
   * 浮层宽度取用户上次的停靠宽度，因此内部 UI 的换行与展开态一致。
   */
  function renderAgentPreview() {
    return renderAgentPanel();
  }

  /** 展开态和收起态浮层共用同一个面板元素，避免两处 props 漂移。 */
  function renderAgentPanel() {
    return (
      <AgentPanel
        key={activeSession.id}
        sessions={currentSnapshot.sessions}
        activeSession={activeSession}
        messageScrollPositions={messageScrollPositions}
        activeKnowledgeBase={sessionKnowledgeBase}
        knowledgeBases={currentSnapshot.knowledgeBases}
        notes={currentSnapshot.notes}
        readingNote={activeNote && activeSession.knowledgeBaseIds.includes(activeNote.knowledgeBaseId) ? activeNote : undefined}
        documents={currentSnapshot.documents}
        prompt={agentPrompt}
        skills={agentSkills}
        selectedSkillIds={explicitSkillIds}
        builtinCommands={builtinCommands}
        builtinCommandNotice={builtinCommandNotice}
        mentionedFiles={mentionableFiles}
        selectedMentionedFileIds={mentionedFileIds}
        draftImages={draftImages}
        modelConfig={currentSettings.modelConfig}
        agentSecurity={currentSettings.agentSecurity}
        turnModelSelection={turnModelSelection}
        isComposerBusy={isCurrentSessionBusy}
        liveTurn={liveTurn}
        queuedFollowUp={queuedFollowUp}
        queuedFollowUpInList={queuedFollowUpInList}
        isSessionContextOpen={isSessionContextOpen}
        isScopeSelectorOpen={isScopeSelectorOpen}
        onToggleSessionContext={handleToggleSessionContext}
        onToggleScopeSelector={handleToggleScopeSelector}
        onToggleScopeKnowledgeBase={handleToggleScopeKnowledgeBase}
        onPromptChange={setAgentPrompt}
        onSelectedSkillIdsChange={setExplicitSkillIds}
        onSelectedMentionedFileIdsChange={setMentionedFileIds}
        onDraftImagesChange={setDraftImages}
        onNotice={setNotice}
        onSubmitPrompt={() => handleSubmitPrompt("ask")}
        onRunBuiltinCommand={(command) => void runBuiltinCommand?.(command, "")}
        onDismissBuiltinCommandNotice={dismissBuiltinCommandNotice}
        onEditUserMessage={(messageId, prompt) => void handleEditUserMessageAndRerun(messageId, prompt)}
        onAbortTurn={() => void handleAbortTurn()}
        onClearQueuedFollowUp={handleClearQueuedFollowUp}
        onTurnModelSelectionChange={setTurnModelSelection}
        onSetSessionModelSelection={handleSetSessionModelSelection}
        onCompactAgentContext={handleCompactAgentContext}
        contextCompaction={contextCompaction}
        onOpenProjectInstruction={handleSelectNote}
        onApplyChangeSet={handleApplyChangeSet}
        onRejectChangeSet={handleRejectChangeSet}
        onSecurityLevelChange={handleSessionSecurityLevelChange}
        onPlanModeChange={handleSessionPlanModeChange}
        onExecutePlan={() =>
          // 批准计划是宿主流程：先持久化关闭规划模式，再用最新快照自动发起执行回合。
          void handleSessionPlanModeChange(false, (savedSnapshot) => {
            void handleSubmitPrompt("ask", PLAN_EXECUTION_PROMPT, savedSnapshot);
          })
        }
        onToggleChangeOperation={handleToggleSkillChangeOperation}
      />
    );
  }

  /** 展开态和收起态浮层共用同一个文档面板元素，避免两处 props 漂移。 */
  function renderDocumentPane() {
    if (!activeDocument) {
      return null;
    }

    return (
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
        onNotice={setNotice}
        sessions={currentSnapshot.sessions}
        onSelectSession={(sessionId) => {
          setIsSchedulesOpen(false);
          if (!agentOpen) {
            setAgentOpen(true);
          }
          void handleSelectSession(sessionId);
        }}
      />
    );
  }

  /** 展开态和收起态浮层共用同一个 Markdown 编辑器元素。 */
  function renderEditorPane() {
    return (
      <EditorPane
        note={activeNote}
        knowledgeBase={activeKnowledgeBase}
        availableTags={currentSnapshot.notes
          .filter((item) => item.knowledgeBaseId === activeKnowledgeBase.id)
          .flatMap((item) => item.tags)}
        proposedChange={activeSession.pendingChange?.status === "pending" ? activeSession.pendingChange : undefined}
        isBusy={isBusy}
        isReviewBusy={isCurrentSessionBusy}
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
        onCreateMarkdown={(parentPath) => openCreateDialog("markdown", parentPath)}
        onCreateText={(parentPath) => openCreateDialog("text", parentPath)}
        onNotice={setNotice}
        sessions={currentSnapshot.sessions}
        onSelectSession={(sessionId) => {
          setIsSchedulesOpen(false);
          if (!agentOpen) {
            setAgentOpen(true);
          }
          void handleSelectSession(sessionId);
        }}
      />
    );
  }

  return (
    <div className="app-shell">
      {/*
        顶栏常驻在内容区上方：左栏收起后仍有专门的收展按钮，
        不必再靠悬浮收起态图标栏、从浮层里点「展开」。
      */}
      <WorkspaceTopBar
        sidebarOpen={compactView.compact ? compactView.sidebarOpen : regionOpen.sidebar}
        onToggleSidebar={() => compactView.compact ? compactView.setSidebarOpen((open) => !open) : toggleRegion("sidebar")}
        compact={compactView.compact}
        compactPane={compactView.pane}
        onCompactPaneChange={(pane) => {
          compactView.showPane(pane);
          if (pane === "agent") setAgentOpen(true);
        }}
      />
      <main
        className={`workspace-grid${resizingPane ? " is-resizing" : ""}${agentOpen ? " agent-open" : ""}${compactView.compact && compactView.sidebarOpen ? " sidebar-overlay-open" : ""}`}
        data-compact-pane={compactView.pane}
        ref={workspaceRef}
        style={{ gridTemplateColumns }}
      >
        {compactView.compact && compactView.sidebarOpen && <button className="workspace-sidebar-backdrop" aria-label="关闭知识库导航" onClick={() => compactView.setSidebarOpen(false)} />}
        {regionOpen.sidebar || compactView.compact ? (
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
          activePane={activeItems.sidebar === "files" ? "files" : "sessions"}
          onActivePaneChange={(pane) => setRegionActiveItem("sidebar", pane)}
          onSearchChange={setSearchTerm}
          onSelectKnowledgeBase={(knowledgeBaseId) => {
            setIsSchedulesOpen(false);
            handleSelectKnowledgeBase(knowledgeBaseId);
          }}
          onAddKnowledgeBase={handleAddKnowledgeBase}
          onToggleFolder={handleToggleFolder}
          onSelectNote={(noteId) => {
            compactView.showPane("editor");
            setIsSchedulesOpen(false);
            handleSelectNote(noteId);
          }}
          onSelectDocument={(documentId) => {
            compactView.showPane("editor");
            setIsSchedulesOpen(false);
            handleSelectDocument(documentId);
          }}
          onRenameNote={openRenameDialog}
          onDeleteNote={handleDeleteNote}
          onOpenNoteHistory={openNoteHistory}
          onRenameDocument={openRenameDocumentDialog}
          onDeleteDocument={handleDeleteDocument}
          onOpenDocumentHistory={openDocumentHistory}
          onCreateMarkdown={(parentPath) => openCreateDialog("markdown", parentPath)}
          onCreateText={(parentPath) => openCreateDialog("text", parentPath)}
          onCreateFolder={(parentPath) => openCreateDialog("folder", parentPath)}
          onCreateProjectInstruction={() => handleCreateOrOpenProjectInstruction()}
          onRefreshKnowledgeBase={handleRescanKnowledgeBase}
          onCreateSession={() => {
            compactView.showPane("agent");
            setIsSchedulesOpen(false);
            if (!agentOpen) {
              setAgentOpen(true);
            }
            void handleCreateSession();
          }}
          onOpenSchedules={() => { compactView.showPane("editor"); setIsSchedulesOpen(true); }}
          schedulesActive={isSchedulesOpen}
          onOpenSettings={handleOpenSettings}
          sessions={currentSnapshot.sessions}
          activeSessionId={activeSession.id}
          inFlightSessionIds={inFlightSessionIds}
          queuedSessionIds={queuedSessionIds}
          onSelectSession={(sessionId) => {
            compactView.showPane("agent");
            setIsSchedulesOpen(false);
            if (!agentOpen) {
              setAgentOpen(true);
            }
            void handleSelectSession(sessionId);
          }}
          onDeleteSession={handleDeleteSession}
          onRenameSession={handleRenameSession}
          onTogglePinSession={handleTogglePinSession}
          onToggleArchiveSession={handleToggleArchiveSession}
        />
        ) : (
          <RegionRail
            edge="left"
            label={getRegion("sidebar").label}
            items={getRegion("sidebar").items}
            activeItemId={activeItems.sidebar}
            onSelectItem={(itemId) => handleSelectSidebarItem(itemId)}
            // 会话/文件共享同一份分块状态；悬浮哪个图标就把分块同步过去，
            // 这样悬浮看到的内容和图标一致，而不是停在用户上次点过的分块上。
            onHoverItem={(itemId) => {
              if (itemId === "sessions" || itemId === "files") {
                setRegionActiveItem("sidebar", itemId);
              }
            }}
            renderPreview={(itemId) => renderSidebarPreview(itemId)}
            previewWidth={regionPreviewWidth.sidebar}
          />
        )}
        {regionOpen.sidebar && (
          <div
            className={`workspace-resizer workspace-resizer-sidebar ${resizingPane === "sidebar" ? "active" : ""}`}
            {...getSeparatorProps("sidebar")}
          />
        )}
        <div className={`editor-workbench${isSchedulesOpen ? " is-schedules" : ""}`}>
          {isSchedulesOpen ? (
            <SchedulesView
              knowledgeBases={currentSnapshot.knowledgeBases}
              activeKnowledgeBaseId={activeKnowledgeBase.id}
              isBusy={isBusy}
              onBusy={async (label, task) => {
                beginBusy(label);
                try {
                  await task();
                } catch (error) {
                  setNotice(formatErrorMessage(error));
                } finally {
                  endBusy();
                }
              }}
              onNotice={setNotice}
              onOpenSession={(sessionId) => {
                setIsSchedulesOpen(false);
                if (!agentOpen) {
                  setAgentOpen(true);
                }
                void (async () => {
                  beginBusy("正在打开任务会话...");
                  try {
                    const sessions = await loadSessions(currentSnapshot);
                    commitSnapshot(await restoreSessionContext({ ...currentSnapshot, sessions }, sessionId, false));
                    rightTabs.openTab({ kind: "session", id: sessionId });
                  } catch (error) {
                    setNotice(formatErrorMessage(error));
                  } finally {
                    endBusy();
                  }
                })();
              }}
            />
          ) : (
            <>
          <EditorTabBar
            tabs={editorTabs}
            activeTab={activeEditorTab}
            onSelect={(tab) => activateEditorTab(tab, "tab")}
            onClose={closeEditorTab}
            agentOpen={agentOpen && !compactView.singlePane}
            onToggleAgent={() => {
              if (compactView.singlePane) {
                compactView.showPane("agent");
                setAgentOpen(true);
              } else {
                handleToggleAgentPanel();
              }
            }}
          />
          <div className="editor-file-panel" id="editor-file-panel" role="tabpanel" aria-label="当前文件内容">
            {activeDocument ? renderDocumentPane() : renderEditorPane()}
          </div>
            </>
          )}
        </div>
        {regionOpen.agent && (
          <div
            className={`workspace-resizer workspace-resizer-agent ${resizingPane === "agent" ? "active" : ""}`}
            {...getSeparatorProps("agent")}
          />
        )}
        <RightPanel
          items={rightTabs.tabs.map<RightPanelTabItem>((tab) => {
            if (tab.kind === "session") {
              const session = currentSnapshot.sessions.find((item) => item.id === tab.id);
              return { tab, title: session?.title ?? "会话", status: inFlightSessionIds.includes(tab.id) || queuedSessionIds.includes(tab.id) ? "运行中" : session?.pendingChange?.status === "pending" || session?.pendingChangeSet?.status === "pending" ? "待确认" : session?.messages.at(-1)?.role === "assistant" ? "已完成" : undefined };
            }
            if (tab.kind === "terminal") {
              const terminal = terminals.get(tab.id);
              return { tab, title: terminal?.descriptor?.shell ?? "终端", status: terminal?.status === "running" ? "运行中" : terminal?.status === "failed" ? "启动失败" : terminal?.status === "exited" ? "已退出" : "启动中" };
            }
            const page = browserWorkspace.state?.tabs.find((item) => item.tabId === tab.id);
            return { tab, title: page?.title || (page?.url === "about:blank" ? "空白页" : page?.url) || "浏览器", status: browserWorkspace.activity?.tabId === tab.id ? "运行中" : undefined };
          })}
          activeId={rightTabs.activeId} open={agentOpen} onExpand={revealRightPanel} onCollapse={() => setRightOpen(false)}
          onSelect={rightActions.select} onClose={rightActions.close} onCreate={rightActions.create} onReorder={rightTabs.reorder} browserEnabled={browserEnabled}
        >
          <div className="right-session-content" hidden={selectedRightTab?.kind !== "session"}>{selectedRightTab?.kind === "session" && renderAgentPanel()}</div>
          {terminals.contents.map((content) => <TerminalPanel key={content.id} content={content} active={rightVisible && selectedRightTab?.kind === "terminal" && selectedRightTab.id === content.id} onRestart={() => void terminals.restart(content.id).catch(setNotice)} />)}
          <BrowserPanel browser={browserWorkspace} tabId={selectedRightTab?.kind === "browser" ? selectedRightTab.id : null} active={selectedRightTab?.kind === "browser"} onRetry={() => selectedRightTab?.kind === "browser" && rightActions.retryBrowser(selectedRightTab.id)} />
        </RightPanel>
      </main>
      {isSettingsOpen && (
        <SettingsDrawer
          knowledgeBases={currentSnapshot.knowledgeBases}
          notes={currentSnapshot.notes}
          activeKnowledgeBaseId={activeKnowledgeBase.id}
          settings={userSettings}
          imSettings={imSettings}
          skills={agentSkills}
          modelApiKeyStatuses={modelApiKeyStatuses}
          imCredentialByProvider={imCredentialByProvider}
          imGatewayByProvider={imGatewayByProvider}
          weixinLoginStatus={weixinLoginStatus}
          providerTemplates={providerTemplates}
          auditLogs={auditLogs}
          appEventLogs={appEventLogs}
          knowledgeBaseMemories={knowledgeBaseMemories}
          isBusy={isBusy}
          onSelectKnowledgeBase={handleSelectKnowledgeBase}
          onAddKnowledgeBase={handleAddKnowledgeBase}
          onRescanKnowledgeBase={handleRescanKnowledgeBase}
          onRemoveKnowledgeBase={handleRemoveKnowledgeBase}
          onCreateOrOpenProjectInstruction={(knowledgeBaseId) => {
            setIsSettingsOpen(false);
            void handleCreateOrOpenProjectInstruction(knowledgeBaseId);
          }}
          onSaveSettings={handleSaveSettings}
          onSaveImSettings={handleSaveImSettings}
          onSaveKnowledgeBaseMemory={handleSaveKnowledgeBaseMemory}
          onDeleteKnowledgeBaseMemory={handleDeleteKnowledgeBaseMemory}
          onSaveSkill={handleSaveSkill}
          onInstallSkill={handleInstallSkill}
          onToggleSkill={handleToggleSkill}
          onDeleteSkill={handleDeleteSkill}
          onOpenUserSkillsFolder={handleOpenUserSkillsFolder}
          onRevealApiKey={handleRevealApiKey}
          onSaveApiKey={handleSaveApiKey}
          onRefreshProviderModels={handleRefreshProviderModels}
          onSaveImSecret={handleSaveImSecret}
          onStartImGateway={handleStartImGateway}
          onStopImGateway={handleStopImGateway}
          onRefreshImStatus={handleRefreshImStatus}
          onStartWeixinLogin={handleStartWeixinLogin}
          onCancelWeixinLogin={handleCancelWeixinLogin}
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
        <ModalBackdrop onClose={() => setRenameDialog(null)}>
          <ModalForm
            aria-label={renameDialog.kind === "note" ? "重命名 Markdown 文件" : "重命名 TXT 文件"}
            onSubmit={(event) => {
              event.preventDefault();
              handleSubmitRename();
            }}
          >
            <div>
              <p className={sectionLabelClassName}>{renameDialog.kind === "note" ? "Markdown 文件" : "TXT 文件"}</p>
              <h2 className="mt-1 mb-0 text-lg leading-tight text-ink-strong [overflow-wrap:anywhere]">重命名</h2>
            </div>
            <label className={fieldLabelClassName}>
              <span>文件名</span>
              <input
                className={fieldControlClassName}
                autoFocus
                value={renameDialog.fileName}
                onChange={(event) => setRenameDialog({ ...renameDialog, fileName: event.target.value })}
                placeholder="例如：会议记录.md"
              />
            </label>
            <div className="flex min-w-0 flex-wrap justify-end gap-2">
              <Button variant="ghost" onClick={() => setRenameDialog(null)} disabled={isBusy}>
                取消
              </Button>
              <Button variant="primary" size="compact" type="submit" disabled={isBusy || !renameDialog.fileName.trim()}>
                保存文件名
              </Button>
            </div>
          </ModalForm>
        </ModalBackdrop>
      )}
      {createDialog && (
        <ModalBackdrop onClose={() => setCreateDialog(null)}>
          <ModalForm
            aria-label={getCreateDialogAriaLabel(createDialog.kind)}
            onSubmit={(event) => {
              event.preventDefault();
              handleSubmitCreate();
            }}
          >
            <div>
              <p className={sectionLabelClassName}>{getCreateParentLabel(createDialog.parentPath)}</p>
              <h2 className="mt-1 mb-0 text-lg leading-tight text-ink-strong [overflow-wrap:anywhere]">
                {getCreateDialogTitle(createDialog.kind)}
              </h2>
            </div>
            <label className={fieldLabelClassName}>
              <span>{createDialog.kind === "folder" ? "目录名" : "文件名"}</span>
              <input
                className={fieldControlClassName}
                autoFocus
                value={createDialog.name}
                onChange={(event) => setCreateDialog({ ...createDialog, name: event.target.value })}
                placeholder={getCreatePlaceholder(createDialog.kind)}
              />
            </label>
            <div className="flex min-w-0 flex-wrap justify-end gap-2">
              <Button variant="ghost" onClick={() => setCreateDialog(null)} disabled={isBusy}>
                取消
              </Button>
              <Button variant="primary" size="compact" type="submit" disabled={isBusy || !createDialog.name.trim()}>
                {getCreateSubmitLabel(createDialog.kind)}
              </Button>
            </div>
          </ModalForm>
        </ModalBackdrop>
      )}
      {pendingConfirmation && (
        <ConfirmDialog
          {...pendingConfirmation}
          isBusy={isBusy}
          onCancel={() => setPendingConfirmation(null)}
          onConfirm={() => void handleConfirmDialogConfirm()}
          onThirdAction={
            pendingConfirmation.onThirdAction
              ? () => {
                  const action = pendingConfirmation.onThirdAction;

                  setPendingConfirmation(null);
                  void action?.();
                }
              : undefined
          }
        />
      )}
      {/* MCP 工具确认由 agent 模块自订阅事件，这里只负责挂载。 */}
      <OperationApprovalDialog />
    </div>
  );
}
