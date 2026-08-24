import { invokeLogged, isTauriRuntime } from "./runtime";
import { createContentHash, createLocalId } from "../id";
import { cloneWorkspaceSnapshot } from "../mock/workspace";
import {
  browserMock,
  captureBrowserDocumentHistory,
  clearBrowserDocumentHistory,
  createMockImagePreviewDataUrl,
  ensureParentFolderExistsForMock,
  joinRelativePath,
  migrateBrowserDocumentHistoryTarget,
  normalizeFolderPath,
  replaceFileNameInPath,
  validateNewTextDocumentFileNameForMock,
  validateTextDocumentFileNameForMock,
} from "../mock/browser";
import {
  DocumentPreview,
  ExportFileResult,
  ExportFormat,
  ExportTargetKind,
  KnowledgeBase,
  Note,
  WorkspaceDocument,
  WorkspaceSnapshot,
} from "../types";

/** 在用户点击的目录下新建空白 TXT；桌面端会立即创建真实文件。 */
export async function createDocument(
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

    const safeFileName = validateNewTextDocumentFileNameForMock(fileName);
    const normalizedParentPath = normalizeFolderPath(parentPath);
    ensureParentFolderExistsForMock(nextSnapshot, knowledgeBaseId, normalizedParentPath);
    const existingPaths = new Set([
      ...nextSnapshot.notes.filter((note) => note.knowledgeBaseId === knowledgeBaseId).map((note) => note.path),
      ...nextSnapshot.documents.filter((document) => document.knowledgeBaseId === knowledgeBaseId).map((document) => document.path),
    ]);
    const nextPath = joinRelativePath(normalizedParentPath, safeFileName);

    // 浏览器 fallback 也模拟桌面文件系统的同目录不可覆盖规则。
    if (existingPaths.has(nextPath)) {
      throw new Error("目标文件已存在，已阻止覆盖。");
    }

    const newDocument: WorkspaceDocument = {
      id: createLocalId("document"),
      knowledgeBaseId,
      title: safeFileName.replace(/\.txt$/i, ""),
      path: nextPath,
      fileType: "txt",
      content: "",
      contentHash: createContentHash(""),
      updatedAt: "刚刚",
      previewAvailable: false,
    };

    nextSnapshot.documents = [newDocument, ...nextSnapshot.documents];
    browserMock.documentDiskContents.set(newDocument.id, newDocument.content ?? "");
    nextSnapshot.knowledgeBases = nextSnapshot.knowledgeBases.map((item) =>
      item.id === knowledgeBaseId
        ? {
            ...item,
            documentCount: item.documentCount + 1,
            updatedAt: "刚刚",
            scanReport: item.scanReport
              ? {
                  ...item.scanReport,
                  scannedFileCount: item.scanReport.scannedFileCount + 1,
                  scannedByType: {
                    ...item.scanReport.scannedByType,
                    txt: (item.scanReport.scannedByType.txt ?? 0) + 1,
                  },
                }
              : item.scanReport,
          }
        : item,
    );
    nextSnapshot.activeKnowledgeBaseId = knowledgeBaseId;
    nextSnapshot.activeNoteId = "";
    nextSnapshot.activeDocumentId = newDocument.id;

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("create_document", {
    payload: { snapshot, knowledgeBaseId, parentPath, fileName },
  });
}

/** 保存当前 TXT 文档正文；桌面端会执行 hash 冲突检测和原子写入。 */
export async function saveDocumentContent(
  snapshot: WorkspaceSnapshot,
  documentId: string,
  content: string,
  expectedHash: string,
): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const document = nextSnapshot.documents.find((item) => item.id === documentId);

    if (!document) {
      throw new Error("找不到要保存的文档。");
    }

    if (document.fileType !== "txt") {
      throw new Error("只有 TXT 文档支持保存。");
    }

    const diskContent = browserMock.documentDiskContents.get(document.id) ?? document.content ?? "";
    const diskHash = createContentHash(diskContent);

    if (diskHash !== expectedHash) {
      throw new Error("目标文件已被外部修改，已阻止保存。请重新扫描后再编辑。");
    }

    captureBrowserDocumentHistory({
      targetKind: "document",
      knowledgeBaseId: document.knowledgeBaseId,
      targetId: document.id,
      relativePath: document.path,
      title: document.title,
      fileType: "txt",
      content: diskContent,
      source: "manual-save",
    });

    browserMock.documentDiskContents.set(document.id, content);
    nextSnapshot.documents = nextSnapshot.documents.map((item) =>
      item.id === documentId ? { ...item, content, contentHash: createContentHash(content), updatedAt: "刚刚" } : item,
    );
    nextSnapshot.activeNoteId = "";
    nextSnapshot.activeDocumentId = documentId;

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("save_document_content", { payload: { snapshot, documentId, content, expectedHash } });
}

/** 重命名当前 TXT 文档；桌面端调用真实 Tauri 文件系统能力。 */
export async function renameDocument(
  snapshot: WorkspaceSnapshot,
  documentId: string,
  nextFileName: string,
): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const document = nextSnapshot.documents.find((item) => item.id === documentId);

    if (!document) {
      throw new Error("找不到要重命名的文档。");
    }

    if (document.fileType !== "txt") {
      throw new Error("只有 TXT 文档支持重命名。");
    }

    const safeFileName = validateTextDocumentFileNameForMock(nextFileName);
    const nextPath = replaceFileNameInPath(document.path, safeFileName);
    const isPathTaken =
      nextSnapshot.notes.some((item) => item.knowledgeBaseId === document.knowledgeBaseId && item.path === nextPath) ||
      nextSnapshot.documents.some(
        (item) => item.knowledgeBaseId === document.knowledgeBaseId && item.id !== document.id && item.path === nextPath,
      );

    // 文件系统同目录不能出现同名文件，不区分它属于 note 还是 document 模型。
    if (isPathTaken) {
      throw new Error("目标文件名已存在，已阻止覆盖。");
    }

    const nextDocumentId = createLocalId("document-renamed");
    const diskContent = browserMock.documentDiskContents.get(document.id) ?? document.content ?? "";
    const nextTitle = safeFileName.replace(/\.txt$/i, "");

    nextSnapshot.documents = nextSnapshot.documents.map((item) =>
      item.id === document.id
        ? {
            ...item,
            id: nextDocumentId,
            path: nextPath,
            title: nextTitle,
            updatedAt: "刚刚",
          }
        : item,
    );
    browserMock.documentDiskContents.delete(document.id);
    browserMock.documentDiskContents.set(nextDocumentId, diskContent);
    migrateBrowserDocumentHistoryTarget("document", document.id, nextDocumentId, nextPath, nextTitle);

    if (nextSnapshot.activeDocumentId === document.id) {
      nextSnapshot.activeDocumentId = nextDocumentId;
      nextSnapshot.activeNoteId = "";
    }

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("rename_document", { payload: { snapshot, documentId, nextFileName } });
}

/** 删除当前 TXT 文档到系统回收站；浏览器 fallback 只移除内存快照中的模拟文档。 */
export async function deleteDocument(
  snapshot: WorkspaceSnapshot,
  documentId: string,
  expectedHash: string,
): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const document = nextSnapshot.documents.find((item) => item.id === documentId);

    if (!document) {
      throw new Error("找不到要删除的文档。");
    }

    if (document.fileType !== "txt") {
      throw new Error("只有 TXT 文档支持删除。");
    }

    // 与桌面 Tauri command 保持一致：删除前必须确认操作基于同一份文件版本。
    const diskContent = browserMock.documentDiskContents.get(document.id) ?? document.content ?? "";

    if (createContentHash(diskContent) !== expectedHash) {
      throw new Error("目标文件已被外部修改，已阻止删除。请重新扫描后再操作。");
    }

    nextSnapshot.documents = nextSnapshot.documents.filter((item) => item.id !== documentId);
    browserMock.documentDiskContents.delete(documentId);
    clearBrowserDocumentHistory("document", documentId);
    nextSnapshot.knowledgeBases = nextSnapshot.knowledgeBases.map((knowledgeBase) =>
      knowledgeBase.id === document.knowledgeBaseId
        ? {
            ...knowledgeBase,
            documentCount: Math.max(0, knowledgeBase.documentCount - 1),
            updatedAt: "刚刚",
            scanReport: knowledgeBase.scanReport
              ? {
                  ...knowledgeBase.scanReport,
                  scannedFileCount: Math.max(0, knowledgeBase.scanReport.scannedFileCount - 1),
                  scannedByType: {
                    ...knowledgeBase.scanReport.scannedByType,
                    txt: Math.max(0, (knowledgeBase.scanReport.scannedByType.txt ?? 0) - 1),
                  },
                }
              : knowledgeBase.scanReport,
          }
        : knowledgeBase,
    );

    if (nextSnapshot.activeDocumentId === documentId) {
      const sameKnowledgeBaseFallback = nextSnapshot.documents.find((item) => item.knowledgeBaseId === document.knowledgeBaseId);

      nextSnapshot.activeDocumentId = sameKnowledgeBaseFallback?.id ?? "";
      nextSnapshot.activeNoteId = sameKnowledgeBaseFallback ? "" : nextSnapshot.notes.find((item) => item.knowledgeBaseId === document.knowledgeBaseId)?.id ?? "";
    }

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("delete_document", { payload: { snapshot, documentId, expectedHash } });
}

/** 加载 DOCX/PDF/图片只读预览；TXT 直接使用快照中的 content。 */
export async function loadDocumentPreview(snapshot: WorkspaceSnapshot, documentId: string): Promise<DocumentPreview> {
  const document = snapshot.documents.find((item) => item.id === documentId);

  if (!document) {
    throw new Error("找不到要预览的文档。");
  }

  if (!isTauriRuntime()) {
    return {
      documentId: document.id,
      fileType: document.fileType,
      title: document.title,
      path: document.path,
      updatedAt: document.updatedAt,
      contentHash: document.contentHash,
      assetPath:
        document.fileType === "pdf"
          ? document.path
          : document.fileType === "image"
            ? createMockImagePreviewDataUrl(document.title)
            : undefined,
      blocks:
        document.fileType === "docx"
          ? [
              { type: "heading", text: document.title },
              { type: "paragraph", text: "这是浏览器开发态模拟的 DOCX 只读预览正文。" },
            ]
          : undefined,
    };
  }

  return invokeLogged<DocumentPreview>("load_document_preview", { payload: { snapshot, documentId } });
}

/** 导出当前打开文件；真实文件写入只允许在 Tauri 桌面端通过系统保存对话框完成。 */
export async function exportCurrentFile(
  snapshot: WorkspaceSnapshot,
  targetKind: ExportTargetKind,
  targetId: string,
  format: ExportFormat,
): Promise<ExportFileResult | null> {
  if (!isTauriRuntime()) {
    throw new Error("浏览器开发态不能导出本地文件，请在 Tauri 桌面端使用导出。");
  }

  return invokeLogged<ExportFileResult | null>("export_current_file", {
    payload: { snapshot, targetKind, targetId, format },
  });
}
