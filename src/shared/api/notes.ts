import { invokeLogged, isTauriRuntime } from "./runtime";
import { createContentHash, createLocalId } from "../id";
import { cloneWorkspaceSnapshot } from "../mock/workspace";
import {
  browserMock,
  captureBrowserDocumentHistory,
  clearBrowserDocumentHistory,
  ensureParentFolderExistsForMock,
  getTitleFromMarkdownOrFileName,
  joinRelativePath,
  migrateBrowserDocumentHistoryTarget,
  migrateNoteReferencesAfterRename,
  normalizeFolderPath,
  removeNoteReferencesAfterDelete,
  replaceFileNameInPath,
  validateMarkdownFileNameForMock,
  validateNewMarkdownFileNameForMock,
} from "../mock/browser";
import {
  KnowledgeBase,
  Note,
  NoteImageAttachmentInput,
  SavedNoteImageAttachment,
  WorkspaceSnapshot,
} from "../types";

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
    const existingPaths = new Set([
      ...nextSnapshot.notes.filter((note) => note.knowledgeBaseId === knowledgeBaseId).map((note) => note.path),
      ...nextSnapshot.documents.filter((document) => document.knowledgeBaseId === knowledgeBaseId).map((document) => document.path),
    ]);
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
    browserMock.noteDiskContents.set(newNote.id, newNote.content);
    nextSnapshot.knowledgeBases = nextSnapshot.knowledgeBases.map((item) =>
      item.id === knowledgeBaseId
        ? {
            ...item,
            noteCount: item.noteCount + 1,
            updatedAt: "刚刚",
            scanReport: item.scanReport
              ? {
                  ...item.scanReport,
                  scannedFileCount: item.scanReport.scannedFileCount + 1,
                  scannedByType: {
                    ...item.scanReport.scannedByType,
                    markdown: (item.scanReport.scannedByType.markdown ?? 0) + 1,
                  },
                }
              : item.scanReport,
          }
        : item,
    );
    nextSnapshot.activeKnowledgeBaseId = knowledgeBaseId;
    nextSnapshot.activeNoteId = newNote.id;
    nextSnapshot.activeDocumentId = "";

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("create_note", {
    payload: { snapshot, knowledgeBaseId, parentPath, fileName },
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
    const note = nextSnapshot.notes.find((item) => item.id === noteId);

    if (!note) {
      throw new Error("找不到要保存的笔记。");
    }

    const diskContent = browserMock.noteDiskContents.get(note.id) ?? note.content;
    const diskHash = createContentHash(diskContent);

    if (diskHash !== expectedHash) {
      throw new Error("目标文件已被外部修改，已阻止保存。请重新扫描后再编辑。");
    }

    captureBrowserDocumentHistory({
      targetKind: "note",
      knowledgeBaseId: note.knowledgeBaseId,
      targetId: note.id,
      relativePath: note.path,
      title: note.title,
      fileType: "markdown",
      content: diskContent,
      source: "manual-save",
    });

    browserMock.noteDiskContents.set(note.id, content);
    nextSnapshot.notes = nextSnapshot.notes.map((item) =>
      item.id === noteId ? { ...item, content, contentHash: createContentHash(content), updatedAt: "刚刚" } : item,
    );

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("save_note_content", { payload: { snapshot, noteId, content, expectedHash } });
}

/** 保存当前 Markdown 粘贴图片附件；只写图片文件，不自动保存正文草稿。 */
export async function saveNoteImageAttachments(
  snapshot: WorkspaceSnapshot,
  noteId: string,
  images: NoteImageAttachmentInput[],
): Promise<SavedNoteImageAttachment[]> {
  if (!isTauriRuntime()) {
    throw new Error("浏览器开发态不能保存本地图片附件，请在 Tauri 桌面端使用粘贴图片。");
  }

  return invokeLogged<SavedNoteImageAttachment[]>("save_note_image_attachments", {
    payload: { snapshot, noteId, images },
  });
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
    const isPathTaken =
      nextSnapshot.notes.some(
        (item) => item.knowledgeBaseId === note.knowledgeBaseId && item.id !== note.id && item.path === nextPath,
      ) ||
      nextSnapshot.documents.some((item) => item.knowledgeBaseId === note.knowledgeBaseId && item.path === nextPath);

    // 浏览器 fallback 只模拟正式桌面行为，仍然不能覆盖同目录已有 Markdown。
    if (isPathTaken) {
      throw new Error("目标文件名已存在，已阻止覆盖。");
    }

    const nextNoteId = createLocalId("note-renamed");
    const diskContent = browserMock.noteDiskContents.get(note.id) ?? note.content;
    const nextTitle = getTitleFromMarkdownOrFileName(note.content, safeFileName);

    nextSnapshot.notes = nextSnapshot.notes.map((item) =>
      item.id === note.id
        ? {
            ...item,
            id: nextNoteId,
            path: nextPath,
            title: nextTitle,
            updatedAt: "刚刚",
          }
        : item,
    );
    browserMock.noteDiskContents.delete(note.id);
    browserMock.noteDiskContents.set(nextNoteId, diskContent);
    migrateBrowserDocumentHistoryTarget("note", note.id, nextNoteId, nextPath, nextTitle);
    migrateNoteReferencesAfterRename(nextSnapshot, note.id, nextNoteId, nextPath);
    nextSnapshot.activeDocumentId = "";

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("rename_note", { payload: { snapshot, noteId, nextFileName } });
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
    const diskContent = browserMock.noteDiskContents.get(note.id) ?? note.content;

    if (createContentHash(diskContent) !== expectedHash) {
      throw new Error("目标文件已被外部修改，已阻止删除。请重新扫描后再操作。");
    }

    nextSnapshot.notes = nextSnapshot.notes.filter((item) => item.id !== noteId);
    browserMock.noteDiskContents.delete(noteId);
    clearBrowserDocumentHistory("note", noteId);
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
                  scannedByType: {
                    ...knowledgeBase.scanReport.scannedByType,
                    markdown: Math.max(0, (knowledgeBase.scanReport.scannedByType.markdown ?? 0) - 1),
                  },
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
    nextSnapshot.activeDocumentId = "";

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("delete_note", { payload: { snapshot, noteId, expectedHash } });
}
