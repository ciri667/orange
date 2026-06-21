import { invoke } from "@tauri-apps/api/core";
import { createContentHash, createLocalId } from "./id";
import {
  acceptMockProposedChange,
  cloneWorkspaceSnapshot,
  createMockKnowledgeBaseSelection,
  createMockWorkspaceSnapshot,
  rejectMockProposedChange,
  runMockAgentTurn,
} from "./mockWorkspace";
import type {
  AgentActionType,
  AgentTurnRequest,
  AgentTurnResult,
  FolderEntry,
  KnowledgeBase,
  KnowledgeBaseSelection,
  Note,
  ProposedChange,
  WorkspaceSnapshot,
} from "./types";

declare global {
  interface Window {
    /** Tauri 运行时注入对象，用于区分桌面环境与浏览器开发环境。 */
    __TAURI_INTERNALS__?: unknown;
  }
}

/** 判断当前是否运行在 Tauri 桌面壳中。 */
export function isTauriRuntime() {
  return typeof window !== "undefined" && Boolean(window.__TAURI_INTERNALS__);
}

/** 从 Tauri 本地层加载工作台状态，浏览器中回退到 mock 数据。 */
export async function loadWorkspaceState(): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    return createMockWorkspaceSnapshot();
  }

  return invoke<WorkspaceSnapshot>("load_workspace_state");
}

/** 通过 Tauri 目录选择器连接知识库，浏览器中创建 mock 目录。 */
export async function selectKnowledgeBaseDirectory(currentCount: number): Promise<KnowledgeBaseSelection> {
  if (!isTauriRuntime()) {
    return createMockKnowledgeBaseSelection(currentCount);
  }

  return invoke<KnowledgeBaseSelection>("select_knowledge_base");
}

/** 扫描新知识库并把它合并进当前快照，浏览器中使用模拟笔记。 */
export async function attachKnowledgeBase(
  snapshot: WorkspaceSnapshot,
  selection: KnowledgeBaseSelection,
): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const newKnowledgeBase: KnowledgeBase = {
      id: selection.id,
      name: selection.name,
      path: selection.path,
      description: "模拟新增的本地 Markdown 目录，正式版本由 Tauri 扫描真实文件。",
      status: "ready",
      noteCount: selection.noteCount,
      updatedAt: "刚刚",
      isDefault: false,
      semanticIndexEnabled: false,
      scanReport: {
        scannedFileCount: 1,
        failedFileCount: 0,
        skippedDirectories: ["node_modules"],
        errors: [],
      },
    };
    const newNote: Note = {
      id: `note-${selection.id}`,
      knowledgeBaseId: selection.id,
      title: "知识库索引",
      path: "Index/知识库索引.md",
      updatedAt: "刚刚",
      tags: ["索引", "Agent"],
      backlinks: [],
      content: `# 知识库索引

这是一个浏览器开发态模拟知识库。正式桌面版会扫描 ${selection.path} 下的 Markdown 文件。`,
      contentHash: "mock-new-note",
    };
    const newFolder: FolderEntry = {
      id: `folder-${selection.id}-index`,
      knowledgeBaseId: selection.id,
      name: "Index",
      path: "Index",
      updatedAt: "刚刚",
    };

    nextSnapshot.knowledgeBases = [...nextSnapshot.knowledgeBases, newKnowledgeBase];
    nextSnapshot.folders = [...nextSnapshot.folders, newFolder];
    nextSnapshot.notes = [newNote, ...nextSnapshot.notes];
    nextSnapshot.activeKnowledgeBaseId = newKnowledgeBase.id;
    nextSnapshot.activeNoteId = newNote.id;

    return nextSnapshot;
  }

  return invoke<WorkspaceSnapshot>("scan_knowledge_base", { payload: { snapshot, selection } });
}

/** 重新扫描已连接知识库，Tauri 环境读取真实目录，浏览器中只刷新模拟状态。 */
export async function rescanKnowledgeBase(snapshot: WorkspaceSnapshot, knowledgeBaseId: string): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);

    nextSnapshot.knowledgeBases = nextSnapshot.knowledgeBases.map((knowledgeBase) =>
      knowledgeBase.id === knowledgeBaseId ? { ...knowledgeBase, updatedAt: "刚刚", status: "ready" } : knowledgeBase,
    );

    return nextSnapshot;
  }

  return invoke<WorkspaceSnapshot>("rescan_knowledge_base", { payload: { snapshot, knowledgeBaseId } });
}

/** 在用户点击的目录下新建空白 Markdown；桌面端会立即创建真实文件。 */
export async function createNote(
  snapshot: WorkspaceSnapshot,
  knowledgeBaseId: string,
  parentPath: string,
  fileName: string,
): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const knowledgeBase = nextSnapshot.knowledgeBases.find((item) => item.id === knowledgeBaseId);

    if (!knowledgeBase) {
      return nextSnapshot;
    }

    const safeFileName = validateNewMarkdownFileNameForMock(fileName);
    const normalizedParentPath = normalizeFolderPath(parentPath);
    ensureParentFolderExistsForMock(nextSnapshot, knowledgeBaseId, normalizedParentPath);
    const existingPaths = new Set(
      nextSnapshot.notes.filter((note) => note.knowledgeBaseId === knowledgeBaseId).map((note) => note.path),
    );
    const nextPath = joinRelativePath(normalizedParentPath, safeFileName);

    // 浏览器 fallback 只模拟正式桌面行为，仍然不能覆盖已有 Markdown。
    if (existingPaths.has(nextPath)) {
      throw new Error("目标文件已存在，已阻止覆盖。");
    }

    const newNote: Note = {
      id: createLocalId("note"),
      knowledgeBaseId,
      title: safeFileName.replace(/\.(md|markdown)$/i, ""),
      path: nextPath,
      content: "",
      tags: [],
      updatedAt: "刚刚",
      backlinks: [],
      contentHash: createContentHash(""),
    };

    nextSnapshot.notes = [newNote, ...nextSnapshot.notes];
    nextSnapshot.knowledgeBases = nextSnapshot.knowledgeBases.map((item) =>
      item.id === knowledgeBaseId
        ? {
            ...item,
            noteCount: item.noteCount + 1,
            updatedAt: "刚刚",
            scanReport: item.scanReport
              ? { ...item.scanReport, scannedFileCount: item.scanReport.scannedFileCount + 1 }
              : item.scanReport,
          }
        : item,
    );
    nextSnapshot.activeKnowledgeBaseId = knowledgeBaseId;
    nextSnapshot.activeNoteId = newNote.id;

    return nextSnapshot;
  }

  return invoke<WorkspaceSnapshot>("create_note", {
    payload: { snapshot, knowledgeBaseId, parentPath, fileName },
  });
}

/** 在用户点击的目录下新建单级文件夹；桌面端会立即创建真实目录。 */
export async function createFolder(
  snapshot: WorkspaceSnapshot,
  knowledgeBaseId: string,
  parentPath: string,
  folderName: string,
): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const safeFolderName = validateFolderNameForMock(folderName);
    const normalizedParentPath = normalizeFolderPath(parentPath);
    const nextFolderPath = joinRelativePath(normalizedParentPath, safeFolderName);

    ensureParentFolderExistsForMock(nextSnapshot, knowledgeBaseId, normalizedParentPath);

    const isPathTaken =
      nextSnapshot.folders.some((folder) => folder.knowledgeBaseId === knowledgeBaseId && folder.path === nextFolderPath) ||
      nextSnapshot.notes.some((note) => note.knowledgeBaseId === knowledgeBaseId && note.path === nextFolderPath);

    // 文件和目录共用同一命名空间，模拟桌面文件系统不能同名覆盖的规则。
    if (isPathTaken) {
      throw new Error("目标文件夹已存在，已阻止覆盖。");
    }

    const folderEntry: FolderEntry = {
      id: createLocalId("folder"),
      knowledgeBaseId,
      name: safeFolderName,
      path: nextFolderPath,
      updatedAt: "刚刚",
    };

    nextSnapshot.folders = [...nextSnapshot.folders, folderEntry];
    nextSnapshot.knowledgeBases = nextSnapshot.knowledgeBases.map((knowledgeBase) =>
      knowledgeBase.id === knowledgeBaseId ? { ...knowledgeBase, updatedAt: "刚刚" } : knowledgeBase,
    );
    nextSnapshot.activeKnowledgeBaseId = knowledgeBaseId;

    return nextSnapshot;
  }

  return invoke<WorkspaceSnapshot>("create_folder", {
    payload: { snapshot, knowledgeBaseId, parentPath, folderName },
  });
}

/** 保存当前笔记正文，Tauri 环境执行路径边界和 hash 校验，浏览器中更新内存快照。 */
export async function saveNoteContent(
  snapshot: WorkspaceSnapshot,
  noteId: string,
  content: string,
  expectedHash: string,
): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);

    nextSnapshot.notes = nextSnapshot.notes.map((note) =>
      note.id === noteId ? { ...note, content, contentHash: createContentHash(content), updatedAt: "刚刚" } : note,
    );

    return nextSnapshot;
  }

  return invoke<WorkspaceSnapshot>("save_note_content", { payload: { snapshot, noteId, content, expectedHash } });
}

/** 重命名当前 Markdown 文件；桌面端调用真实 Tauri 文件系统能力，浏览器仅做开发态内存 fallback。 */
export async function renameNote(
  snapshot: WorkspaceSnapshot,
  noteId: string,
  nextFileName: string,
): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const note = nextSnapshot.notes.find((item) => item.id === noteId);

    if (!note) {
      throw new Error("找不到要重命名的笔记。");
    }

    const safeFileName = validateMarkdownFileNameForMock(nextFileName);
    const nextPath = replaceFileNameInPath(note.path, safeFileName);
    const isPathTaken = nextSnapshot.notes.some(
      (item) => item.knowledgeBaseId === note.knowledgeBaseId && item.id !== note.id && item.path === nextPath,
    );

    // 浏览器 fallback 只模拟正式桌面行为，仍然不能覆盖同目录已有 Markdown。
    if (isPathTaken) {
      throw new Error("目标文件名已存在，已阻止覆盖。");
    }

    const nextNoteId = createLocalId("note-renamed");

    nextSnapshot.notes = nextSnapshot.notes.map((item) =>
      item.id === note.id
        ? {
            ...item,
            id: nextNoteId,
            path: nextPath,
            title: getTitleFromMarkdownOrFileName(item.content, safeFileName),
            updatedAt: "刚刚",
          }
        : item,
    );
    migrateNoteReferencesAfterRename(nextSnapshot, note.id, nextNoteId, nextPath);

    return nextSnapshot;
  }

  return invoke<WorkspaceSnapshot>("rename_note", { payload: { snapshot, noteId, nextFileName } });
}

/** 删除当前 Markdown 文件到系统回收站；浏览器 fallback 只移除内存快照中的模拟笔记。 */
export async function deleteNote(
  snapshot: WorkspaceSnapshot,
  noteId: string,
  expectedHash: string,
): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const note = nextSnapshot.notes.find((item) => item.id === noteId);

    if (!note) {
      throw new Error("找不到要删除的笔记。");
    }

    // 与桌面 Tauri command 保持一致：删除前必须确认操作基于同一份文件版本。
    if (note.contentHash !== expectedHash) {
      throw new Error("目标文件已被外部修改，已阻止删除。请重新扫描后再操作。");
    }

    nextSnapshot.notes = nextSnapshot.notes.filter((item) => item.id !== noteId);
    nextSnapshot.knowledgeBases = nextSnapshot.knowledgeBases.map((knowledgeBase) =>
      knowledgeBase.id === note.knowledgeBaseId
        ? {
            ...knowledgeBase,
            noteCount: Math.max(0, knowledgeBase.noteCount - 1),
            updatedAt: "刚刚",
            scanReport: knowledgeBase.scanReport
              ? {
                  ...knowledgeBase.scanReport,
                  scannedFileCount: Math.max(0, knowledgeBase.scanReport.scannedFileCount - 1),
                }
              : knowledgeBase.scanReport,
          }
        : knowledgeBase,
    );
    removeNoteReferencesAfterDelete(nextSnapshot, noteId);

    const sameKnowledgeBaseFallback = nextSnapshot.notes.find((item) => item.knowledgeBaseId === note.knowledgeBaseId);

    if (nextSnapshot.activeNoteId === noteId || !nextSnapshot.notes.some((item) => item.id === nextSnapshot.activeNoteId)) {
      nextSnapshot.activeNoteId = sameKnowledgeBaseFallback?.id ?? "";
    }

    return nextSnapshot;
  }

  return invoke<WorkspaceSnapshot>("delete_note", { payload: { snapshot, noteId, expectedHash } });
}

/** 移除知识库授权和索引缓存；不会删除用户本地 Markdown 文件。 */
export async function removeKnowledgeBase(snapshot: WorkspaceSnapshot, knowledgeBaseId: string): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);

    nextSnapshot.knowledgeBases = nextSnapshot.knowledgeBases.filter((knowledgeBase) => knowledgeBase.id !== knowledgeBaseId);
    nextSnapshot.folders = nextSnapshot.folders.filter((folder) => folder.knowledgeBaseId !== knowledgeBaseId);
    nextSnapshot.notes = nextSnapshot.notes.filter((note) => note.knowledgeBaseId !== knowledgeBaseId);
    nextSnapshot.sessions = nextSnapshot.sessions
      .map((session) => ({
        ...session,
        knowledgeBaseIds: session.knowledgeBaseIds.filter((id) => id !== knowledgeBaseId),
        pinnedNoteIds: session.pinnedNoteIds.filter((noteId) => nextSnapshot.notes.some((note) => note.id === noteId)),
      }))
      .filter((session) => session.knowledgeBaseIds.length > 0);

    const activeKnowledgeBase = nextSnapshot.knowledgeBases.find(
      (knowledgeBase) => knowledgeBase.id === nextSnapshot.activeKnowledgeBaseId,
    );
    const fallbackKnowledgeBase = activeKnowledgeBase ?? nextSnapshot.knowledgeBases[0];

    nextSnapshot.knowledgeBases = nextSnapshot.knowledgeBases.map((knowledgeBase, index) => ({
      ...knowledgeBase,
      isDefault: index === 0,
    }));
    nextSnapshot.activeKnowledgeBaseId = fallbackKnowledgeBase?.id ?? "";
    nextSnapshot.activeNoteId = nextSnapshot.notes.find((note) => note.knowledgeBaseId === nextSnapshot.activeKnowledgeBaseId)?.id ?? "";
    nextSnapshot.activeSessionId =
      nextSnapshot.sessions.find((session) => session.knowledgeBaseIds.includes(nextSnapshot.activeKnowledgeBaseId))?.id ??
      nextSnapshot.sessions[0]?.id ??
      "";

    if (!nextSnapshot.knowledgeBases.length) {
      nextSnapshot.sessions = [];
      nextSnapshot.activeKnowledgeBaseId = "";
      nextSnapshot.activeNoteId = "";
      nextSnapshot.activeSessionId = "";
    }

    return nextSnapshot;
  }

  return invoke<WorkspaceSnapshot>("remove_knowledge_base", { payload: { snapshot, knowledgeBaseId } });
}

/** 运行 Agent 单轮 loop，模型可在内部自行选择是否调用检索工具。 */
export async function runAgentTurn(
  snapshot: WorkspaceSnapshot,
  prompt: string,
  action: AgentActionType,
): Promise<AgentTurnResult> {
  const request: AgentTurnRequest = {
    prompt,
    action,
    sessionId: snapshot.activeSessionId,
    activeKnowledgeBaseId: snapshot.activeKnowledgeBaseId,
    activeNoteId: snapshot.activeNoteId,
  };

  if (!isTauriRuntime()) {
    return { snapshot: runMockAgentTurn(snapshot, prompt, action) };
  }

  return invoke<AgentTurnResult>("run_agent_turn", { payload: { snapshot, request } });
}

/** 接受当前会话的待确认变更，Tauri 环境中由本地层执行安全写入。 */
export async function acceptProposedChange(snapshot: WorkspaceSnapshot): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    return acceptMockProposedChange(snapshot);
  }

  return invoke<WorkspaceSnapshot>("apply_proposed_change", { payload: { snapshot } });
}

/** 拒绝当前会话的待确认变更，Tauri 环境中只更新会话状态。 */
export async function rejectProposedChange(snapshot: WorkspaceSnapshot): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    return rejectMockProposedChange(snapshot);
  }

  return invoke<WorkspaceSnapshot>("reject_proposed_change", { payload: { snapshot } });
}

/** 浏览器开发态使用的文件名校验，保持与 Rust 层正式规则一致。 */
function validateMarkdownFileNameForMock(fileName: string) {
  const trimmedFileName = fileName.trim();

  if (!trimmedFileName) {
    throw new Error("文件名不能为空。");
  }

  // 重命名只改当前目录下的文件名，不能携带路径分隔符或上级目录。
  if (trimmedFileName.includes("/") || trimmedFileName.includes("\\") || trimmedFileName === "." || trimmedFileName === "..") {
    throw new Error("文件名不能包含路径或上级目录。");
  }

  if (!/\.(md|markdown)$/i.test(trimmedFileName)) {
    throw new Error("文件名必须以 .md 或 .markdown 结尾。");
  }

  return trimmedFileName;
}

/** 浏览器开发态的新建 Markdown 文件名校验；允许省略扩展名并默认补 .md。 */
function validateNewMarkdownFileNameForMock(fileName: string) {
  const trimmedFileName = fileName.trim();

  if (!trimmedFileName) {
    throw new Error("文件名不能为空。");
  }

  const normalizedFileName = /\.[^./\\]+$/.test(trimmedFileName) ? trimmedFileName : `${trimmedFileName}.md`;

  return validateMarkdownFileNameForMock(normalizedFileName);
}

/** 浏览器开发态的新建目录名校验，只允许单级普通目录名。 */
function validateFolderNameForMock(folderName: string) {
  const trimmedFolderName = folderName.trim();
  const ignoredDirectoryNames = new Set([
    ".git",
    ".hg",
    ".svn",
    ".idea",
    ".vscode",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".nuxt",
    ".turbo",
    ".cache",
  ]);

  if (!trimmedFolderName) {
    throw new Error("文件夹名不能为空。");
  }

  // 新建目录只允许单级名称，不能通过浏览器 fallback 伪造路径穿越或多级目录。
  if (
    trimmedFolderName.includes("/") ||
    trimmedFolderName.includes("\\") ||
    trimmedFolderName === "." ||
    trimmedFolderName === ".."
  ) {
    throw new Error("文件夹名不能包含路径或上级目录。");
  }

  if (trimmedFolderName.startsWith(".") || ignoredDirectoryNames.has(trimmedFolderName)) {
    throw new Error("不能创建隐藏目录或扫描忽略目录。");
  }

  return trimmedFolderName;
}

/** 规范化目录相对路径，根目录统一为空字符串。 */
function normalizeFolderPath(folderPath: string) {
  return folderPath.trim().replace(/^\/+|\/+$/g, "");
}

/** 拼接知识库内相对路径，根目录下只返回子名称。 */
function joinRelativePath(parentPath: string, childName: string) {
  return parentPath ? `${parentPath}/${childName}` : childName;
}

/** 浏览器 fallback 确认父目录存在，避免新建时隐式创建多级目录。 */
function ensureParentFolderExistsForMock(snapshot: WorkspaceSnapshot, knowledgeBaseId: string, parentPath: string) {
  if (!parentPath) {
    return;
  }

  const parentExists = snapshot.folders.some((folder) => folder.knowledgeBaseId === knowledgeBaseId && folder.path === parentPath);

  if (!parentExists) {
    throw new Error("目标父目录不存在，已阻止新建。");
  }
}

/** 在相对路径中替换最后一级文件名，模拟桌面端“只改文件名”的重命名语义。 */
function replaceFileNameInPath(relativePath: string, nextFileName: string) {
  const pathParts = relativePath.split("/");

  pathParts[pathParts.length - 1] = nextFileName;

  return pathParts.join("/");
}

/** 预览或重命名时从正文一级标题提取展示标题，没有一级标题时使用文件名 stem。 */
function getTitleFromMarkdownOrFileName(content: string, fileName: string) {
  const markdownTitle = content
    .split(/\r?\n/)
    .find((line) => line.trim().startsWith("# "))
    ?.trim()
    .replace(/^#\s+/, "")
    .trim();

  if (markdownTitle) {
    return markdownTitle;
  }

  return fileName.replace(/\.(md|markdown)$/i, "") || "未命名笔记";
}

/** 重命名后迁移当前笔记、固定笔记和待确认 diff 引用。 */
function migrateNoteReferencesAfterRename(
  snapshot: WorkspaceSnapshot,
  previousNoteId: string,
  nextNoteId: string,
  nextPath: string,
) {
  if (snapshot.activeNoteId === previousNoteId) {
    snapshot.activeNoteId = nextNoteId;
  }

  snapshot.sessions = snapshot.sessions.map((session) => {
    const nextPendingChange = migratePendingChangeAfterRename(session.pendingChange, previousNoteId, nextNoteId, nextPath);

    return {
      ...session,
      activeNoteId: session.activeNoteId === previousNoteId ? nextNoteId : session.activeNoteId,
      pinnedNoteIds: Array.from(
        new Set(session.pinnedNoteIds.map((pinnedNoteId) => (pinnedNoteId === previousNoteId ? nextNoteId : pinnedNoteId))),
      ),
      pendingChange: nextPendingChange,
    };
  });
}

/** 迁移待确认 diff 中的目标笔记和路径，避免重命名后 diff 仍指向旧文件。 */
function migratePendingChangeAfterRename(
  pendingChange: ProposedChange | undefined,
  previousNoteId: string,
  nextNoteId: string,
  nextPath: string,
) {
  if (pendingChange?.noteId !== previousNoteId) {
    return pendingChange;
  }

  return { ...pendingChange, noteId: nextNoteId, targetPath: nextPath };
}

/** 删除后清理会话中的笔记引用和待确认 diff。 */
function removeNoteReferencesAfterDelete(snapshot: WorkspaceSnapshot, noteId: string) {
  snapshot.sessions = snapshot.sessions.map((session) => ({
    ...session,
    activeNoteId: session.activeNoteId === noteId ? undefined : session.activeNoteId,
    pinnedNoteIds: session.pinnedNoteIds.filter((pinnedNoteId) => pinnedNoteId !== noteId),
    pendingChange: session.pendingChange?.noteId === noteId ? undefined : session.pendingChange,
  }));
}
