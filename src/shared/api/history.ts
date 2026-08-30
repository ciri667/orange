import { invokeLogged, isTauriRuntime } from "./runtime";
import { createContentHash } from "../id";
import { extractNoteTags } from "../noteTags";
import { cloneWorkspaceSnapshot } from "../mock/workspace";
import {
  browserMock,
  captureBrowserDocumentHistory,
  clearBrowserDocumentHistory,
  isBrowserHistoryTarget,
} from "../mock/browser";
import {
  DocumentHistoryEntry,
  DocumentHistoryEntryDetail,
  DocumentHistoryTargetKind,
  Note,
  WorkspaceSnapshot,
} from "../types";

/** 读取当前 Markdown/TXT 文件历史列表；DOCX/PDF/图片不会调用该接口。 */
export async function loadDocumentHistory(
  snapshot: WorkspaceSnapshot,
  targetKind: DocumentHistoryTargetKind,
  targetId: string,
): Promise<DocumentHistoryEntry[]> {
  if (!isTauriRuntime()) {
    return browserMock.documentHistoryEntries
      .filter((entry) => isBrowserHistoryTarget(entry, targetKind, targetId))
      .map((entry) => ({ ...entry }));
  }

  return invokeLogged<DocumentHistoryEntry[]>("load_document_history", {
    payload: { snapshot, targetKind, targetId },
  });
}

/** 读取单条历史记录正文快照，供恢复 diff 预览使用。 */
export async function loadDocumentHistoryEntry(entryId: string): Promise<DocumentHistoryEntryDetail> {
  if (!isTauriRuntime()) {
    const entry = browserMock.documentHistoryEntries.find((item) => item.id === entryId);

    if (!entry) {
      throw new Error("找不到该历史记录。");
    }

    const content = browserMock.documentHistoryContents.get(entryId);

    if (typeof content !== "string") {
      throw new Error("历史快照不存在或不可访问。");
    }

    return { ...entry, content };
  }

  return invokeLogged<DocumentHistoryEntryDetail>("load_document_history_entry", {
    payload: { entryId },
  });
}

/** 恢复指定历史版本；恢复前会先保存当前磁盘版本为历史记录。 */
export async function restoreDocumentHistoryEntry(
  snapshot: WorkspaceSnapshot,
  entryId: string,
  expectedHash: string,
): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const detail = await loadDocumentHistoryEntry(entryId);
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);

    if (detail.targetKind === "note") {
      const note = nextSnapshot.notes.find((item) => item.id === detail.targetId);

      if (!note) {
        throw new Error("找不到要恢复的 Markdown 笔记。");
      }

      const diskContent = browserMock.noteDiskContents.get(note.id) ?? note.content;

      if (createContentHash(diskContent) !== expectedHash) {
        throw new Error("目标文件已被外部修改，已阻止恢复。请重新扫描后再操作。");
      }

      captureBrowserDocumentHistory({
        targetKind: "note",
        knowledgeBaseId: note.knowledgeBaseId,
        targetId: note.id,
        relativePath: note.path,
        title: note.title,
        fileType: "markdown",
        content: diskContent,
        source: "restore",
      });
      browserMock.noteDiskContents.set(note.id, detail.content);
      nextSnapshot.notes = nextSnapshot.notes.map((item) =>
        item.id === note.id
          ? {
              ...item,
              content: detail.content,
              tags: extractNoteTags(detail.content),
              contentHash: createContentHash(detail.content),
              updatedAt: "刚刚",
            }
          : item,
      );
      nextSnapshot.activeNoteId = note.id;
      nextSnapshot.activeDocumentId = "";

      return nextSnapshot;
    }

    const document = nextSnapshot.documents.find((item) => item.id === detail.targetId);

    if (!document || document.fileType !== "txt") {
      throw new Error("找不到要恢复的 TXT 文档。");
    }

    const diskContent = browserMock.documentDiskContents.get(document.id) ?? document.content ?? "";

    if (createContentHash(diskContent) !== expectedHash) {
      throw new Error("目标文件已被外部修改，已阻止恢复。请重新扫描后再操作。");
    }

    captureBrowserDocumentHistory({
      targetKind: "document",
      knowledgeBaseId: document.knowledgeBaseId,
      targetId: document.id,
      relativePath: document.path,
      title: document.title,
      fileType: "txt",
      content: diskContent,
      source: "restore",
    });
    browserMock.documentDiskContents.set(document.id, detail.content);
    nextSnapshot.documents = nextSnapshot.documents.map((item) =>
      item.id === document.id
        ? { ...item, content: detail.content, contentHash: createContentHash(detail.content), updatedAt: "刚刚" }
        : item,
    );
    nextSnapshot.activeNoteId = "";
    nextSnapshot.activeDocumentId = document.id;

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("restore_document_history_entry", {
    payload: { snapshot, entryId, expectedHash },
  });
}

/** 清空当前文件历史记录；不会删除用户文档正文。 */
export async function clearDocumentHistory(
  snapshot: WorkspaceSnapshot,
  targetKind: DocumentHistoryTargetKind,
  targetId: string,
): Promise<void> {
  if (!isTauriRuntime()) {
    clearBrowserDocumentHistory(targetKind, targetId);
    return;
  }

  await invokeLogged<void>("clear_document_history", {
    payload: { snapshot, targetKind, targetId },
  });
}
