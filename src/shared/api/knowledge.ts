import { invokeLogged, isTauriRuntime } from "./runtime";
import { createContentHash, createLocalId } from "../id";
import { extractNoteTags } from "../noteTags";
import { cloneWorkspaceSnapshot, createMockKnowledgeBaseSelection } from "../mock/workspace";
import {
  browserMock,
  ensureParentFolderExistsForMock,
  joinRelativePath,
  normalizeFolderPath,
  validateFolderNameForMock,
} from "../mock/browser";
import {
  FolderEntry,
  KnowledgeBase,
  KnowledgeBaseMemory,
  KnowledgeBaseSelection,
  Note,
  WorkspaceDocument,
  WorkspaceSnapshot,
} from "../types";

/** 读取全部知识库的跨会话记忆；浏览器开发态返回空列表。 */
export async function loadKnowledgeBaseMemories(): Promise<KnowledgeBaseMemory[]> {
  if (!isTauriRuntime()) {
    return [];
  }

  return invokeLogged<KnowledgeBaseMemory[]>("load_knowledge_base_memories");
}

/** 保存单个知识库的跨会话记忆；桌面端写入前会做敏感信息脱敏并返回归一化结果。 */
export async function saveKnowledgeBaseMemory(
  knowledgeBaseId: string,
  memory: KnowledgeBaseMemory,
): Promise<KnowledgeBaseMemory> {
  if (!isTauriRuntime()) {
    return { ...memory, knowledgeBaseId, updatedAt: new Date().toISOString() };
  }

  return invokeLogged<KnowledgeBaseMemory>("save_knowledge_base_memory", {
    payload: { knowledgeBaseId, memory },
  });
}

/** 删除单个知识库的跨会话记忆。 */
export async function deleteKnowledgeBaseMemory(knowledgeBaseId: string): Promise<void> {
  if (!isTauriRuntime()) {
    return;
  }

  await invokeLogged<void>("delete_knowledge_base_memory", { payload: { knowledgeBaseId } });
}

/** 通过 Tauri 目录选择器连接知识库，浏览器中创建 mock 目录。 */
export async function selectKnowledgeBaseDirectory(currentCount: number): Promise<KnowledgeBaseSelection> {
  if (!isTauriRuntime()) {
    return createMockKnowledgeBaseSelection(currentCount);
  }

  return invokeLogged<KnowledgeBaseSelection>("select_knowledge_base");
}

/** 扫描新知识库并把它合并进当前快照，浏览器中使用模拟文档。 */
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
      description: "模拟新增的本地支持文档目录，正式版本由 Tauri 扫描真实文件。",
      status: "ready",
      noteCount: selection.noteCount,
      documentCount: 1,
      updatedAt: "刚刚",
      isDefault: false,
      semanticIndexEnabled: false,
      scanReport: {
        scannedFileCount: 2,
        scannedByType: {
          markdown: 1,
          txt: 1,
          docx: 0,
          pdf: 0,
          image: 0,
        },
        failedFileCount: 0,
        skippedDirectories: ["node_modules"],
        errors: [],
      },
    };
    const newNoteContent = `# 知识库索引

这是一个浏览器开发态模拟知识库。正式桌面版会扫描 ${selection.path} 下的支持文档。

#索引 #Agent
`;
    const newNote: Note = {
      id: `note-${selection.id}`,
      knowledgeBaseId: selection.id,
      title: "知识库索引",
      path: "Index/知识库索引.md",
      updatedAt: "刚刚",
      tags: extractNoteTags(newNoteContent),
      backlinks: [],
      content: newNoteContent,
      contentHash: "mock-new-note",
    };
    const newFolder: FolderEntry = {
      id: `folder-${selection.id}-index`,
      knowledgeBaseId: selection.id,
      name: "Index",
      path: "Index",
      updatedAt: "刚刚",
    };
    const newDocument: WorkspaceDocument = {
      id: `document-${selection.id}-readme`,
      knowledgeBaseId: selection.id,
      title: "资料说明",
      path: "Index/资料说明.txt",
      fileType: "txt",
      updatedAt: "刚刚",
      content: "这是一个浏览器开发态模拟 TXT 文档。",
      contentHash: createContentHash("这是一个浏览器开发态模拟 TXT 文档。"),
      previewAvailable: false,
    };

    nextSnapshot.knowledgeBases = [...nextSnapshot.knowledgeBases, newKnowledgeBase];
    nextSnapshot.folders = [...nextSnapshot.folders, newFolder];
    nextSnapshot.notes = [newNote, ...nextSnapshot.notes];
    nextSnapshot.documents = [newDocument, ...nextSnapshot.documents];
    browserMock.noteDiskContents.set(newNote.id, newNote.content);
    browserMock.documentDiskContents.set(newDocument.id, newDocument.content ?? "");
    nextSnapshot.activeKnowledgeBaseId = newKnowledgeBase.id;
    nextSnapshot.activeNoteId = newNote.id;
    nextSnapshot.activeDocumentId = "";

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("scan_knowledge_base", { payload: { snapshot, selection } });
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

  return invokeLogged<WorkspaceSnapshot>("rescan_knowledge_base", { payload: { snapshot, knowledgeBaseId } });
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
      nextSnapshot.notes.some((note) => note.knowledgeBaseId === knowledgeBaseId && note.path === nextFolderPath) ||
      nextSnapshot.documents.some((document) => document.knowledgeBaseId === knowledgeBaseId && document.path === nextFolderPath);

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

  return invokeLogged<WorkspaceSnapshot>("create_folder", {
    payload: { snapshot, knowledgeBaseId, parentPath, folderName },
  });
}

/** 移除知识库授权和索引缓存；不会删除用户本地文档。 */
export async function removeKnowledgeBase(snapshot: WorkspaceSnapshot, knowledgeBaseId: string): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);

    nextSnapshot.knowledgeBases = nextSnapshot.knowledgeBases.filter((knowledgeBase) => knowledgeBase.id !== knowledgeBaseId);
    nextSnapshot.folders = nextSnapshot.folders.filter((folder) => folder.knowledgeBaseId !== knowledgeBaseId);
    nextSnapshot.notes = nextSnapshot.notes.filter((note) => note.knowledgeBaseId !== knowledgeBaseId);
    nextSnapshot.documents = nextSnapshot.documents.filter((document) => document.knowledgeBaseId !== knowledgeBaseId);
    browserMock.documentHistoryEntries
      .filter((entry) => entry.knowledgeBaseId === knowledgeBaseId)
      .forEach((entry) => browserMock.documentHistoryContents.delete(entry.id));
    browserMock.documentHistoryEntries = browserMock.documentHistoryEntries.filter((entry) => entry.knowledgeBaseId !== knowledgeBaseId);
    Array.from(browserMock.noteDiskContents.keys()).forEach((noteId) => {
      if (!nextSnapshot.notes.some((note) => note.id === noteId)) {
        browserMock.noteDiskContents.delete(noteId);
      }
    });
    Array.from(browserMock.documentDiskContents.keys()).forEach((documentId) => {
      if (!nextSnapshot.documents.some((document) => document.id === documentId)) {
        browserMock.documentDiskContents.delete(documentId);
      }
    });
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
    nextSnapshot.activeDocumentId = nextSnapshot.activeNoteId
      ? ""
      : nextSnapshot.documents.find((document) => document.knowledgeBaseId === nextSnapshot.activeKnowledgeBaseId)?.id ?? "";
    nextSnapshot.activeSessionId =
      nextSnapshot.sessions.find((session) => session.knowledgeBaseIds.includes(nextSnapshot.activeKnowledgeBaseId))?.id ??
      "";

    if (!nextSnapshot.knowledgeBases.length) {
      nextSnapshot.sessions = [];
      nextSnapshot.activeKnowledgeBaseId = "";
      nextSnapshot.activeNoteId = "";
      nextSnapshot.activeDocumentId = "";
      nextSnapshot.activeSessionId = "";
    }

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("remove_knowledge_base", { payload: { snapshot, knowledgeBaseId } });
}
