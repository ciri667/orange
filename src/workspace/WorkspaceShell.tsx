import { useEffect, useState } from "react";
import { AgentPanel } from "../agent/AgentPanel";
import { EditorPane } from "../editor/EditorPane";
import { buildFileTree } from "../knowledge-base/treeUtils";
import { KnowledgeBaseSidebar } from "../knowledge-base/KnowledgeBaseSidebar";
import { SettingsDrawer } from "../settings/SettingsDrawer";
import { createContentHash, createLocalId } from "../shared/id";
import {
  getActiveKnowledgeBase,
  getActiveNote,
  getActiveSession,
} from "../shared/selectors";
import {
  acceptProposedChange,
  attachKnowledgeBase,
  createFolder,
  createNote,
  deleteNote,
  loadWorkspaceState,
  removeKnowledgeBase,
  renameNote,
  rejectProposedChange,
  rescanKnowledgeBase,
  runAgentTurn,
  saveNoteContent,
  selectKnowledgeBaseDirectory,
} from "../shared/tauriApi";
import type {
  AgentActionType,
  AgentSession,
  AgentSessionType,
  KnowledgeBase,
  MarkdownViewMode,
  Note,
  WorkspaceSnapshot,
} from "../shared/types";
import { TopBar } from "./TopBar";

/** 将未知异常统一转换为可展示文案，避免启动错误页渲染空对象。 */
function formatErrorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

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

  return {
    id: createLocalId(`session-${type}`),
    title,
    type,
    knowledgeBaseIds: [knowledgeBase.id],
    activeNoteId: note?.id,
    pinnedNoteIds: note ? [note.id] : [],
    messages: [buildSessionIntroMessage(title, knowledgeBase, note)],
    createdAt: "刚刚",
    updatedAt: "刚刚",
  };
}

/** 按知识库列表顺序整理会话选中的知识库，保证界面和引用范围稳定。 */
function orderKnowledgeBaseIds(selectedIds: string[], knowledgeBases: KnowledgeBase[]) {
  const selectedIdSet = new Set(selectedIds);

  return knowledgeBases
    .filter((knowledgeBase) => selectedIdSet.has(knowledgeBase.id))
    .map((knowledgeBase) => knowledgeBase.id);
}

/** 正式工作台根组件，集中编排知识库、编辑器、Agent loop 和设置状态。 */
export function WorkspaceShell() {
  const [snapshot, setSnapshot] = useState<WorkspaceSnapshot | null>(null);
  /** 首屏初始化是否仍在进行，用于区分加载中和加载失败。 */
  const [isBooting, setIsBooting] = useState(true);
  /** 首屏初始化失败原因，失败后展示重试入口而不是停留在 loading。 */
  const [bootError, setBootError] = useState("");
  const [searchTerm, setSearchTerm] = useState("");
  const [agentPrompt, setAgentPrompt] = useState("");
  const [collapsedFolderPaths, setCollapsedFolderPaths] = useState<Set<string>>(new Set());
  const [isSessionListOpen, setIsSessionListOpen] = useState(false);
  const [isSessionContextOpen, setIsSessionContextOpen] = useState(false);
  const [isScopeSelectorOpen, setIsScopeSelectorOpen] = useState(false);
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  const [isBusy, setIsBusy] = useState(false);
  const [busyLabel, setBusyLabel] = useState("");
  const [notice, setNotice] = useState("");
  const [editingBaseHashes, setEditingBaseHashes] = useState<Record<string, string>>({});
  const [dirtyNoteIds, setDirtyNoteIds] = useState<Set<string>>(new Set());
  const [markdownViewMode, setMarkdownViewMode] = useState<MarkdownViewMode>("edit");
  const [renameDialog, setRenameDialog] = useState<{ noteId: string; fileName: string } | null>(null);
  const [createDialog, setCreateDialog] = useState<{
    kind: "document" | "folder";
    knowledgeBaseId: string;
    parentPath: string;
    name: string;
  } | null>(null);

  useEffect(() => {
    let isMounted = true;

    void loadInitialData(() => isMounted);

    return () => {
      isMounted = false;
    };
  }, []);

  /** 加载首屏必需的工作台快照，失败时进入可重试错误态。 */
  async function loadInitialData(shouldCommit: () => boolean = () => true) {
    setIsBooting(true);
    setBootError("");
    setNotice("");

    try {
      // 加载工作台状态时优先走 Tauri 本地层，浏览器开发态自动使用 mock 快照。
      const nextSnapshot = await loadWorkspaceState();

      if (shouldCommit()) {
        setSnapshot(nextSnapshot);
        setEditingBaseHashes(buildNoteHashMap(nextSnapshot.notes));
      }
    } catch (error) {
      if (shouldCommit()) {
        setSnapshot(null);
        setBootError(formatErrorMessage(error));
      }
    } finally {
      if (shouldCommit()) {
        setIsBooting(false);
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

  if (!snapshot) {
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
        <h1>连接一个 Markdown 目录，开始使用知识库 Agent 助手。</h1>
        <p>Agent 只能通过受控工具访问你选择的本地目录；写入会先生成 diff，确认后才落盘。</p>
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
  const activeNote = getActiveNote(currentSnapshot);
  const isActiveNoteDirty = activeNote ? dirtyNoteIds.has(activeNote.id) : false;
  const fileTree = buildFileTree({
    knowledgeBase: activeKnowledgeBase,
    folders: currentSnapshot.folders,
    notes: currentSnapshot.notes,
    searchTerm,
  });

  /** 更新当前快照中的单个会话，确保消息、范围和 diff 都绑定在同一上下文。 */
  function updateActiveSession(updater: (session: AgentSession) => AgentSession) {
    setSnapshot((currentSnapshot) => {
      if (!currentSnapshot) {
        return currentSnapshot;
      }

      return {
        ...currentSnapshot,
        sessions: currentSnapshot.sessions.map((session) =>
          session.id === currentSnapshot.activeSessionId ? updater(session) : session,
        ),
      };
    });
  }

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

  /** 写入新快照时同步保存基准 hash，清理已经不存在的草稿标记。 */
  function commitSnapshot(nextSnapshot: WorkspaceSnapshot, dirtyNotesToKeep = dirtyNoteIds) {
    const nextNoteIds = new Set(nextSnapshot.notes.map((note) => note.id));
    const nextDirtyNoteIds = new Set(Array.from(dirtyNotesToKeep).filter((noteId) => nextNoteIds.has(noteId)));

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
    setDirtyNoteIds(nextDirtyNoteIds);
  }

  /** 选择知识库时同步收窄 Agent 工具范围，避免跨库误检索。 */
  function handleSelectKnowledgeBase(knowledgeBaseId: string) {
    const nextKnowledgeBase = currentSnapshot.knowledgeBases.find((knowledgeBase) => knowledgeBase.id === knowledgeBaseId);
    const nextNotes = currentSnapshot.notes.filter((note) => note.knowledgeBaseId === knowledgeBaseId);

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

    setSnapshot({
      ...currentSnapshot,
      sessions: existingSession ? currentSnapshot.sessions : [nextSession, ...currentSnapshot.sessions],
      activeKnowledgeBaseId: knowledgeBaseId,
      activeNoteId: nextNotes[0]?.id ?? "",
      activeSessionId: nextSession.id,
    });
    setSearchTerm("");
    setCollapsedFolderPaths(new Set());
  }

  /** 添加知识库时走 Tauri 目录选择器，浏览器开发态使用 mock 目录。 */
  async function handleAddKnowledgeBase() {
    beginBusy("正在选择并扫描知识库...");

    try {
      const selection = await selectKnowledgeBaseDirectory(currentSnapshot.knowledgeBases.length);
      setNotice(`正在扫描「${selection.name}」中的 Markdown 文件...`);
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
  function handleSelectNote(noteId: string) {
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

    setSnapshot({
      ...currentSnapshot,
      sessions: existingSession ? currentSnapshot.sessions : [nextSession, ...currentSnapshot.sessions],
      activeKnowledgeBaseId: nextKnowledgeBase.id,
      activeNoteId: noteId,
      activeSessionId: nextSession.id,
    });
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

  /** 打开目录树新建弹窗；创建位置完全由被点击的目录节点决定。 */
  function openCreateDialog(kind: "document" | "folder", parentPath: string) {
    const defaultName =
      kind === "document"
        ? getNextAvailableDocumentName(currentSnapshot, activeKnowledgeBase.id, parentPath)
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

    beginBusy(createDialog.kind === "document" ? "正在新建 Markdown..." : "正在新建目录...");

    try {
      if (createDialog.kind === "document") {
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
                activeSessionId: nextSession.id,
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

  /** 打开重命名弹窗；存在未保存草稿时先阻止，避免本地文件版本语义不清。 */
  function openRenameDialog(noteId = activeNote?.id ?? "") {
    const note = currentSnapshot.notes.find((item) => item.id === noteId);

    if (!note) {
      return;
    }

    if (dirtyNoteIds.size > 0) {
      setNotice("请先保存当前草稿，再重命名。");
      return;
    }

    setRenameDialog({ noteId: note.id, fileName: getFileNameFromPath(note.path) });
  }

  /** 提交重命名弹窗中的新文件名，真实桌面端最终由 Tauri 校验并执行 fs::rename。 */
  async function handleSubmitRenameNote() {
    if (!renameDialog) {
      return;
    }

    const note = currentSnapshot.notes.find((item) => item.id === renameDialog.noteId);

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

  /** 删除指定 Markdown 文件到系统回收站；删除前二次确认并携带保存基准 hash。 */
  async function handleDeleteNote(noteId = activeNote?.id ?? "") {
    const note = currentSnapshot.notes.find((item) => item.id === noteId);

    if (!note) {
      return;
    }

    if (dirtyNoteIds.size > 0) {
      setNotice("请先保存当前草稿，再删除。");
      return;
    }

    // 删除虽然使用系统回收站，但仍会从当前工作区移除索引和会话引用，需要用户确认。
    if (!window.confirm(`将「${note.title}」移入系统回收站？`)) {
      return;
    }

    const expectedHash = editingBaseHashes[note.id] ?? note.contentHash;

    beginBusy("正在删除 Markdown...");

    try {
      const nextSnapshot = await deleteNote(currentSnapshot, note.id, expectedHash);

      commitSnapshot(nextSnapshot, new Set());
      setNotice("已移入系统回收站。");
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 新建一个任务会话，绑定当前笔记和当前知识库作为上下文起点。 */
  function handleCreateSession() {
    const nextSession = buildAgentSession({
      type: "task",
      knowledgeBase: activeKnowledgeBase,
      note: activeNote,
    });

    setSnapshot({
      ...currentSnapshot,
      sessions: [nextSession, ...currentSnapshot.sessions],
      activeSessionId: nextSession.id,
    });
    setIsSessionListOpen(true);
    setIsSessionContextOpen(false);
    setIsScopeSelectorOpen(false);
  }

  /** 切换会话时恢复它绑定的知识库、笔记和工具范围。 */
  function handleSelectSession(sessionId: string) {
    const nextSession = currentSnapshot.sessions.find((session) => session.id === sessionId);

    if (!nextSession) {
      return;
    }

    const nextKnowledgeBaseId = nextSession.knowledgeBaseIds[0] ?? activeKnowledgeBase.id;
    const nextNoteId =
      nextSession.activeNoteId ??
      currentSnapshot.notes.find((note) => note.knowledgeBaseId === nextKnowledgeBaseId)?.id ??
      activeNote?.id ??
      "";

    setSnapshot({
      ...currentSnapshot,
      activeSessionId: sessionId,
      activeKnowledgeBaseId: nextKnowledgeBaseId,
      activeNoteId: nextNoteId,
    });
    setSearchTerm("");
    setCollapsedFolderPaths(new Set());
    setIsSessionListOpen(false);
    setIsSessionContextOpen(false);
    setIsScopeSelectorOpen(false);
  }

  /** 为当前会话勾选或取消额外知识库，当前激活知识库始终保留。 */
  function handleToggleScopeKnowledgeBase(knowledgeBaseId: string) {
    updateActiveSession((session) => {
      const selectedIds = new Set(session.knowledgeBaseIds.length ? session.knowledgeBaseIds : [activeKnowledgeBase.id]);

      selectedIds.add(activeKnowledgeBase.id);

      // 当前激活知识库是默认工具范围边界，不能在本会话中取消。
      if (knowledgeBaseId !== activeKnowledgeBase.id) {
        if (selectedIds.has(knowledgeBaseId)) {
          selectedIds.delete(knowledgeBaseId);
        } else {
          selectedIds.add(knowledgeBaseId);
        }
      }

      return {
        ...session,
        knowledgeBaseIds: orderKnowledgeBaseIds(Array.from(selectedIds), currentSnapshot.knowledgeBases),
        updatedAt: "刚刚",
      };
    });
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
      const result = await runAgentTurn(currentSnapshot, prompt, action);
      commitSnapshot(result.snapshot);
      setAgentPrompt("");
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 重新扫描指定知识库，使用本地 Markdown 文件刷新目录树和 FTS 索引。 */
  async function handleRescanKnowledgeBase(knowledgeBaseId: string) {
    if (dirtyNoteIds.size > 0) {
      setNotice("请先保存当前草稿，再刷新目录树。");
      return;
    }

    beginBusy("正在重新扫描知识库...");

    try {
      const nextSnapshot = await rescanKnowledgeBase(currentSnapshot, knowledgeBaseId);

      commitSnapshot(nextSnapshot, new Set());
      setCollapsedFolderPaths(new Set());
      setNotice(buildScanNotice(nextSnapshot, knowledgeBaseId));
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 移除知识库授权和索引缓存，不删除用户选择目录中的 Markdown 文件。 */
  async function handleRemoveKnowledgeBase(knowledgeBaseId: string) {
    const knowledgeBase = currentSnapshot.knowledgeBases.find((item) => item.id === knowledgeBaseId);

    if (!knowledgeBase) {
      return;
    }

    // 移除授权会清理本地索引和会话范围，虽然不删除 Markdown 文件，仍需要用户明确确认。
    if (!window.confirm(`移除「${knowledgeBase.name}」的知识库授权？本地 Markdown 文件不会被删除。`)) {
      return;
    }

    beginBusy("正在移除知识库授权...");

    try {
      const nextSnapshot = await removeKnowledgeBase(currentSnapshot, knowledgeBaseId);

      commitSnapshot(nextSnapshot, new Set());
      setCollapsedFolderPaths(new Set());
      setNotice(`已移除「${knowledgeBase.name}」授权，本地 Markdown 文件未被删除。`);

      if (!nextSnapshot.knowledgeBases.length) {
        setSearchTerm("");
        setIsSettingsOpen(false);
      }
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
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

  return (
    <div className="app-shell">
      <TopBar
        activeKnowledgeBase={activeKnowledgeBase}
        knowledgeBaseCount={currentSnapshot.knowledgeBases.length}
        onOpenSettings={() => setIsSettingsOpen(true)}
      />
      <main className="workspace-grid">
        <KnowledgeBaseSidebar
          knowledgeBases={currentSnapshot.knowledgeBases}
          activeKnowledgeBase={activeKnowledgeBase}
          fileTree={fileTree}
          activeNoteId={activeNote?.id ?? ""}
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
          onRenameNote={openRenameDialog}
          onDeleteNote={handleDeleteNote}
          onCreateDocument={(parentPath) => openCreateDialog("document", parentPath)}
          onCreateFolder={(parentPath) => openCreateDialog("folder", parentPath)}
          onRefreshKnowledgeBase={handleRescanKnowledgeBase}
        />
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
          onRequestRewrite={() => handleSubmitPrompt("rewrite", "改写当前笔记的核心段落")}
          onRenameNote={() => openRenameDialog()}
          onDeleteNote={() => handleDeleteNote()}
          onAcceptChange={handleAcceptChange}
          onRejectChange={handleRejectChange}
        />
        <AgentPanel
          sessions={currentSnapshot.sessions}
          activeSession={activeSession}
          activeKnowledgeBase={activeKnowledgeBase}
          knowledgeBases={currentSnapshot.knowledgeBases}
          notes={currentSnapshot.notes}
          prompt={agentPrompt}
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
          onToggleScopeKnowledgeBase={handleToggleScopeKnowledgeBase}
          onPromptChange={setAgentPrompt}
          onSubmitPrompt={() => handleSubmitPrompt("ask")}
          onQuickAction={(action, prompt) => handleSubmitPrompt(action, prompt)}
        />
      </main>
      {isSettingsOpen && (
        <SettingsDrawer
          knowledgeBases={currentSnapshot.knowledgeBases}
          activeKnowledgeBaseId={activeKnowledgeBase.id}
          isBusy={isBusy}
          onSelectKnowledgeBase={handleSelectKnowledgeBase}
          onAddKnowledgeBase={handleAddKnowledgeBase}
          onRescanKnowledgeBase={handleRescanKnowledgeBase}
          onRemoveKnowledgeBase={handleRemoveKnowledgeBase}
          onClose={() => setIsSettingsOpen(false)}
        />
      )}
      {renameDialog && (
        <div className="modal-backdrop" role="presentation" onMouseDown={() => setRenameDialog(null)}>
          <form
            className="rename-dialog"
            aria-label="重命名 Markdown 文件"
            onMouseDown={(event) => event.stopPropagation()}
            onSubmit={(event) => {
              event.preventDefault();
              handleSubmitRenameNote();
            }}
          >
            <div className="modal-header">
              <div>
                <p className="section-label">Markdown 文件</p>
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
            aria-label={createDialog.kind === "document" ? "新建 Markdown 文档" : "新建目录"}
            onMouseDown={(event) => event.stopPropagation()}
            onSubmit={(event) => {
              event.preventDefault();
              handleSubmitCreate();
            }}
          >
            <div className="modal-header">
              <div>
                <p className="section-label">{getCreateParentLabel(createDialog.parentPath)}</p>
                <h2>{createDialog.kind === "document" ? "新建文档" : "新建目录"}</h2>
              </div>
            </div>
            <label className="rename-field">
              <span>{createDialog.kind === "document" ? "文件名" : "目录名"}</span>
              <input
                autoFocus
                value={createDialog.name}
                onChange={(event) => setCreateDialog({ ...createDialog, name: event.target.value })}
                placeholder={createDialog.kind === "document" ? "例如：会议记录" : "例如：Projects"}
              />
            </label>
            <div className="modal-actions">
              <button className="ghost-button" type="button" onClick={() => setCreateDialog(null)} disabled={isBusy}>
                取消
              </button>
              <button className="primary-button compact" type="submit" disabled={isBusy || !createDialog.name.trim()}>
                {createDialog.kind === "document" ? "创建文档" : "创建目录"}
              </button>
            </div>
          </form>
        </div>
      )}
    </div>
  );
}

/** 为笔记建立当前文件 hash 映射，保存草稿时用于外部修改冲突校验。 */
function buildNoteHashMap(notes: Note[]) {
  return Object.fromEntries(notes.map((note) => [note.id, note.contentHash]));
}

/** 从知识库相对路径中取最后一级文件名，用于重命名弹窗默认值。 */
function getFileNameFromPath(relativePath: string) {
  return relativePath.split("/").filter(Boolean).pop() ?? relativePath;
}

/** 为新建文档生成当前父目录下不冲突的默认名称。 */
function getNextAvailableDocumentName(snapshot: WorkspaceSnapshot, knowledgeBaseId: string, parentPath: string) {
  const existingPaths = new Set(
    snapshot.notes.filter((note) => note.knowledgeBaseId === knowledgeBaseId).map((note) => note.path),
  );

  for (let index = 1; index <= 999; index += 1) {
    const fileName = index === 1 ? "未命名.md" : `未命名 ${index}.md`;

    // 默认名称只看当前目标目录，避免用户打开弹窗后马上遇到后端重名错误。
    if (!existingPaths.has(joinRelativePath(parentPath, fileName))) {
      return fileName;
    }
  }

  return "未命名.md";
}

/** 为新建目录生成当前父目录下不冲突的默认名称。 */
function getNextAvailableFolderName(snapshot: WorkspaceSnapshot, knowledgeBaseId: string, parentPath: string) {
  const existingPaths = new Set(
    snapshot.folders.filter((folder) => folder.knowledgeBaseId === knowledgeBaseId).map((folder) => folder.path),
  );

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
    return `已扫描「${knowledgeBase.name}」，发现 ${knowledgeBase.noteCount} 篇 Markdown。`;
  }

  const skippedText = report.skippedDirectories.length ? `，跳过 ${report.skippedDirectories.length} 个依赖或隐藏目录` : "";
  const errorText = report.failedFileCount ? `，${report.failedFileCount} 个文件读取失败` : "";

  if (report.scannedFileCount === 0 && !report.failedFileCount) {
    return `「${knowledgeBase.name}」暂未发现 Markdown 文件${skippedText}。`;
  }

  return `已扫描「${knowledgeBase.name}」：${report.scannedFileCount} 篇 Markdown${errorText}${skippedText}。`;
}
