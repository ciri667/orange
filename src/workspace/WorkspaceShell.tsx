import { useEffect, useRef, useState } from "react";
import { AgentPanel } from "../agent/AgentPanel";
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
import { saveWorkspaceEditorState } from "../shared/tauriApi";
import type {
  DocumentHistoryTargetKind,
  EditorFileTab,
  MarkdownViewMode,
  WorkspaceEditorState,
  WorkspaceSnapshot,
} from "../shared/types";
import { DocumentHistoryDialog } from "./DocumentHistoryDialog";
import { TopBar } from "./TopBar";
import {
  getCreateDialogAriaLabel,
  getCreateDialogTitle,
  getCreateParentLabel,
  getCreatePlaceholder,
  getCreateSubmitLabel,
} from "./fileNameUtils";
import { DRAFT_SESSION_ID, buildDraftAgentSession, buildMentionableFiles, resolveActiveSessionForKnowledgeBase } from "./sessionUtils";
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
  /** 设置抽屉打开状态，打开时会刷新非阻塞诊断日志。 */
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  /** 全局忙碌状态覆盖文件、会话、设置和日志刷新操作；用计数避免切会话清掉 Agent 回合。 */
  const [isBusy, setIsBusy] = useState(false);
  const busyCountRef = useRef(0);
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
  /** 编辑区已打开文件的展示顺序；通过独立编辑器会话恢复，不混入领域快照。 */
  const [openFileTabs, setOpenFileTabs] = useState<EditorFileTab[]>([]);
  /** 启动恢复完成后才允许写回，避免首帧空标签覆盖已持久化的编辑器会话。 */
  const [isEditorSessionInitialized, setIsEditorSessionInitialized] = useState(false);
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
    onEditorStateInitialized: (editorState: WorkspaceEditorState) => {
      setOpenFileTabs(editorState.openTabs);
      setIsEditorSessionInitialized(true);
    },
    onNoticeChange: setNotice,
  });
  useReviewChangeLogger(snapshot);
  /** Agent 输入草稿按会话隔离，切走再切回时保留该会话的模型和未发送文字。 */
  const {
    agentPrompt,
    setAgentPrompt,
    turnModelSelection,
    setTurnModelSelection,
    explicitSkillIds,
    setExplicitSkillIds,
    mentionedFileIds,
    setMentionedFileIds,
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
    inFlightSessionIds,
    queuedSessionIds,
    handleClearQueuedFollowUp,
    handleAbortTurn,
    handlePrepareSessionForDelete,
    handleSubmitPrompt,
    handleEditUserMessageAndRerun,
    handleApproveSkillExecution,
    handleRejectSkillExecution,
    handleApplySkillChangeSet,
    handleRejectSkillChangeSet,
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
    setAuditLogs,
    setAppEventLogs,
    dirtyNoteIds,
    dirtyDocumentIds,
  });
  const {
    handleCreateSession,
    handleSelectSession,
    handleDeleteSession,
    handleToggleScopeKnowledgeBase,
    handleSetSessionModelSelection,
    handleCompactAgentContext,
    handleToggleSessionList,
    handleToggleSessionContext,
    handleToggleScopeSelector,
    handleToggleAgentPanel,
    handleSessionSecurityLevelChange,
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
    setIsSessionListOpen,
    setIsSessionContextOpen,
    setIsScopeSelectorOpen,
    isSessionListOpen,
    isSessionContextOpen,
    isScopeSelectorOpen,
    agentOpen,
    setAgentOpen,
    setUserSettings,
    prepareSessionForDelete: handlePrepareSessionForDelete,
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

  if (!currentSnapshot.knowledgeBases.length) {
    return (
      <BootScreen variant="empty">
        <div className="brand-mark">
          <img className="brand-logo" src="/orange-logo.svg" alt="" />
        </div>
        <BootTitle>连接一个支持文档目录，开始使用知识库 Agent 助手。</BootTitle>
        <BootCopy>目录树会展示 Markdown、TXT、DOCX、PDF 和图片；Agent 写入仍只作用于确认后的 Markdown diff。</BootCopy>
        <OperationNotice className="max-w-[620px]" isBusy={isBusy} busyLabel={busyLabel} notice={notice} />
        <Button variant="primary" onClick={handleAddKnowledgeBase} disabled={isBusy}>
          添加第一个知识库
        </Button>
      </BootScreen>
    );
  }

  const activeKnowledgeBase = getActiveKnowledgeBase(currentSnapshot);
  const persistedActiveSession = resolveActiveSessionForKnowledgeBase(currentSnapshot, activeKnowledgeBase);
  const activeSession = persistedActiveSession ?? buildDraftAgentSession(activeKnowledgeBase);
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
    void loadInitialDiagnosticLogs();
  }

  return (
    <div className="app-shell">
      <TopBar
        activeKnowledgeBase={activeKnowledgeBase}
        knowledgeBaseCount={currentSnapshot.knowledgeBases.length}
        onOpenSettings={handleOpenSettings}
        agentOpen={agentOpen}
        onToggleAgent={handleToggleAgentPanel}
      />
      <main
        className={`workspace-grid${resizingPane ? " is-resizing" : ""}${agentOpen ? " agent-open" : ""}`}
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
          onCreateProjectInstruction={() => handleCreateOrOpenProjectInstruction()}
          onRefreshKnowledgeBase={handleRescanKnowledgeBase}
        />
        <div
          className={`workspace-resizer workspace-resizer-sidebar ${resizingPane === "sidebar" ? "active" : ""}`}
          {...getSeparatorProps("sidebar")}
        />
        <div className="editor-workbench">
          <EditorTabBar
            tabs={editorTabs}
            activeTab={activeEditorTab}
            onSelect={(tab) => activateEditorTab(tab, "tab")}
            onClose={closeEditorTab}
          />
          <div className="editor-file-panel" id="editor-file-panel" role="tabpanel" aria-label="当前文件内容">
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
              />
            )}
          </div>
        </div>
        {agentOpen && (
          <div
            className={`workspace-resizer workspace-resizer-agent ${resizingPane === "agent" ? "active" : ""}`}
            {...getSeparatorProps("agent")}
          />
        )}
        {agentOpen && (
          <AgentPanel
            sessions={currentSnapshot.sessions}
            activeSession={activeSession}
            activeKnowledgeBase={activeKnowledgeBase}
            knowledgeBases={currentSnapshot.knowledgeBases}
            notes={currentSnapshot.notes}
            documents={currentSnapshot.documents}
            currentFileLabel={
              activeNote?.title ??
              (activeDocument?.fileType === "txt" ? activeDocument.title : "当前打开文件不可作为编辑目标")
            }
            prompt={agentPrompt}
            skills={agentSkills}
            selectedSkillIds={explicitSkillIds}
            mentionedFiles={mentionableFiles}
            selectedMentionedFileIds={mentionedFileIds}
            modelConfig={userSettings.modelConfig}
            agentSecurity={userSettings.agentSecurity}
            turnModelSelection={turnModelSelection}
            isComposerBusy={isCurrentSessionBusy}
            inFlightSessionIds={inFlightSessionIds}
            queuedSessionIds={queuedSessionIds}
            liveTurn={liveTurn}
            queuedFollowUp={queuedFollowUp}
            queuedFollowUpInList={queuedFollowUpInList}
            isSessionListOpen={isSessionListOpen}
            isSessionContextOpen={isSessionContextOpen}
            isScopeSelectorOpen={isScopeSelectorOpen}
            onToggleSessionList={handleToggleSessionList}
            onToggleSessionContext={handleToggleSessionContext}
            onToggleScopeSelector={handleToggleScopeSelector}
            onCollapsePanel={handleToggleAgentPanel}
            onCreateSession={handleCreateSession}
            onSelectSession={handleSelectSession}
            onDeleteSession={handleDeleteSession}
            onToggleScopeKnowledgeBase={handleToggleScopeKnowledgeBase}
            onPromptChange={setAgentPrompt}
            onSelectedSkillIdsChange={setExplicitSkillIds}
            onSelectedMentionedFileIdsChange={setMentionedFileIds}
            onSubmitPrompt={() => handleSubmitPrompt("ask")}
            onEditUserMessage={(messageId, prompt) => void handleEditUserMessageAndRerun(messageId, prompt)}
            onAbortTurn={() => void handleAbortTurn()}
            onClearQueuedFollowUp={handleClearQueuedFollowUp}
            onTurnModelSelectionChange={setTurnModelSelection}
            onSetSessionModelSelection={handleSetSessionModelSelection}
            onCompactAgentContext={handleCompactAgentContext}
            onOpenProjectInstruction={handleSelectNote}
            onApproveExecution={handleApproveSkillExecution}
            onRejectExecution={handleRejectSkillExecution}
            onApplyChangeSet={handleApplySkillChangeSet}
            onRejectChangeSet={handleRejectSkillChangeSet}
            onSecurityLevelChange={handleSessionSecurityLevelChange}
            onToggleChangeOperation={handleToggleSkillChangeOperation}
          />
        )}
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
    </div>
  );
}
