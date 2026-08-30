import type { ConfirmDialogConfig } from "../shared/ConfirmDialog";
import { createContentHash } from "../shared/id";
import { logError, logInfo, logWarn } from "../shared/logger";
import { getActiveDocument, getActiveKnowledgeBase, getActiveNote } from "../shared/selectors";
import { findProjectInstructionNote } from "../shared/projectInstructions";
import {
  createDocument,
  createFolder,
  createNote,
  createProjectInstruction,
  deleteDocument,
  deleteNote,
  exportCurrentFile,
  renameDocument,
  renameNote,
  saveDocumentContent,
  saveNoteContent,
  saveNoteImageAttachments,
} from "../shared/tauriApi";
import type { EditorFileTab, ExportFormat, NoteImageAttachmentInput, WorkspaceSnapshot } from "../shared/types";
import {
  insertMarkdownAtSelection,
  MAX_PASTE_IMAGE_BATCH_BYTES,
  MAX_PASTE_IMAGE_BYTES,
  readImageFileAsBase64,
  summarizeImageMimeTypes,
} from "./editorPasteUtils";
import {
  getFileNameFromPath,
  getNextAvailableFolderName,
  getNextAvailableMarkdownName,
  getNextAvailableTextDocumentName,
  joinRelativePath,
} from "./fileNameUtils";
import { extractNoteTags } from "../shared/noteTags";
import { buildDraftAgentSession, resolveActiveSessionForKnowledgeBase, resolveKnowledgeBaseSessionId } from "./sessionUtils";
import type { WorkspaceChrome } from "./workspaceChrome";

function formatErrorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

interface EditorActionsOptions extends WorkspaceChrome {
  setSnapshot: (snapshot: WorkspaceSnapshot) => void;
  setDirtyNoteIds: (value: Set<string> | ((current: Set<string>) => Set<string>)) => void;
  setDirtyDocumentIds: (value: Set<string> | ((current: Set<string>) => Set<string>)) => void;
  dirtyNoteIds: Set<string>;
  dirtyDocumentIds: Set<string>;
  editingBaseHashes: Record<string, string>;
  editingBaseDocumentHashes: Record<string, string>;
  openFileTabs: EditorFileTab[];
  setOpenFileTabs: (value: EditorFileTab[] | ((current: EditorFileTab[]) => EditorFileTab[])) => void;
  historyDialog: { targetKind: "note" | "document"; targetId: string } | null;
  setHistoryDialog: (value: { targetKind: "note" | "document"; targetId: string } | null) => void;
  renameDialog: { kind: "note" | "document"; id: string; fileName: string } | null;
  setRenameDialog: (value: { kind: "note" | "document"; id: string; fileName: string } | null) => void;
  createDialog: { kind: "markdown" | "text" | "folder"; knowledgeBaseId: string; parentPath: string; name: string } | null;
  setCreateDialog: (value: { kind: "markdown" | "text" | "folder"; knowledgeBaseId: string; parentPath: string; name: string } | null) => void;
  setSearchTerm: (value: string) => void;
  setMarkdownViewMode: (value: "edit" | "preview" | "split") => void;
  setCollapsedFolderPaths: (value: Set<string> | ((current: Set<string>) => Set<string>)) => void;
  isBusy: boolean;
  expandFolderPaths: (folderPaths: string[]) => void;
}

const noopAsync = async (..._args: unknown[]) => {};
const noop = (..._args: unknown[]) => {};

/** 编辑器标签、草稿保存、新建/重命名/删除和导出动作。 */
export function useEditorActions(options: EditorActionsOptions) {
  const {
    snapshot,
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
  } = options;

  if (!snapshot) {
    return {
      activateEditorTab: noop,
      replaceEditorTabId: noop,
      handleSelectNote: noopAsync,
      handleSelectDocument: noopAsync,
      handleContentChange: noop,
      handlePasteImages: noopAsync,
      handleDocumentContentChange: noop,
      openNoteHistory: noop,
      openDocumentHistory: noop,
      handleHistoryRestored: noop,
      openCreateDialog: noop,
      handleSubmitCreate: noopAsync,
      saveDirtyEditorTab: async () => false,
      handleSaveActiveNote: noopAsync,
      handleSaveActiveDocument: noopAsync,
      closeEditorTab: noop,
      handleExportActiveFile: noopAsync,
      openRenameDialog: noop,
      openRenameDocumentDialog: noop,
      handleSubmitRename: noopAsync,
      handleSubmitRenameDocument: noopAsync,
      handleDeleteNote: noopAsync,
      handleDeleteDocument: noopAsync,
      handleCreateOrOpenProjectInstruction: noopAsync,
    };
  }

  const currentSnapshot = snapshot;
  const activeKnowledgeBase = getActiveKnowledgeBase(currentSnapshot);
  const activeNote = getActiveNote(currentSnapshot);
  const activeDocument = getActiveDocument(currentSnapshot);
  const persistedActiveSession = resolveActiveSessionForKnowledgeBase(currentSnapshot, activeKnowledgeBase);
  const activeSession = persistedActiveSession ?? buildDraftAgentSession(activeKnowledgeBase);


  /** 把指定文件加入临时标签并激活，同时保持原有知识库与 Agent 会话选择语义。 */
  function activateEditorTab(tab: EditorFileTab, source: "tree" | "tab" | "create" | "keyboard") {
    const target =
      tab.kind === "note"
        ? currentSnapshot.notes.find((note) => note.id === tab.id)
        : currentSnapshot.documents.find((document) => document.id === tab.id);

    if (!target) {
      return;
    }

    const nextKnowledgeBase =
      currentSnapshot.knowledgeBases.find((knowledgeBase) => knowledgeBase.id === target.knowledgeBaseId) ?? activeKnowledgeBase;
    const activatedSnapshot = {
      ...currentSnapshot,
      activeKnowledgeBaseId: nextKnowledgeBase.id,
      activeNoteId: tab.kind === "note" ? tab.id : "",
      activeDocumentId: tab.kind === "document" ? tab.id : "",
      activeSessionId: resolveKnowledgeBaseSessionId(currentSnapshot, nextKnowledgeBase.id),
    };

    setOpenFileTabs((currentTabs) =>
      currentTabs.some((item) => item.kind === tab.kind && item.id === tab.id) ? currentTabs : [...currentTabs, tab],
    );
    logInfo("切换编辑器文件标签。", {
      category: "frontend",
      event: source === "tree" || source === "create" ? "editor_tab_open" : "editor_tab_select",
      status: "completed",
      metadata: { fileKind: tab.kind, source, openTabCount: openFileTabs.length + 1 },
    });
    commitSnapshot(activatedSnapshot);
  }

  /** 重命名会生成新的稳定文件 ID，需要保留原标签的位置而非把它当作失效标签移除。 */
  function replaceEditorTabId(kind: EditorFileTab["kind"], previousId: string, nextId: string) {
    setOpenFileTabs((currentTabs) =>
      currentTabs.map((tab) => (tab.kind === kind && tab.id === previousId ? { ...tab, id: nextId } : tab)),
    );
  }

  /** 打开 Markdown 文件只切换编辑器焦点；会话保持知识库级别，不再跟随文档切换。 */
  async function handleSelectNote(noteId: string) {
    const nextNote = currentSnapshot.notes.find((note) => note.id === noteId);

    if (!nextNote) {
      return;
    }

    activateEditorTab({ kind: "note", id: noteId }, "tree");
  }

  /** 打开普通文档只切换编辑器焦点；没有同库会话时也不创建默认会话。 */
  async function handleSelectDocument(documentId: string) {
    const nextDocument = currentSnapshot.documents.find((document) => document.id === documentId);

    if (!nextDocument) {
      return;
    }

    activateEditorTab({ kind: "document", id: documentId }, "tree");
  }

  /** 更新当前笔记正文，只修改内存草稿；保存时才写回本地 Markdown 文件。 */
  function handleContentChange(content: string) {
    if (!activeNote) {
      return;
    }

    setSnapshot({
      ...currentSnapshot,
      notes: currentSnapshot.notes.map((note) =>
        note.id === activeNote.id
          ? { ...note, content, tags: extractNoteTags(content), updatedAt: "刚刚", contentHash: createContentHash(content) }
          : note,
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

  /** 打开或创建当前知识库根目录的 AGENTS.md，不走普通新建文件名弹窗。 */
  async function handleCreateOrOpenProjectInstruction(knowledgeBaseId = activeKnowledgeBase.id) {
    const targetKnowledgeBase = currentSnapshot.knowledgeBases.find((item) => item.id === knowledgeBaseId);

    if (!targetKnowledgeBase || targetKnowledgeBase.status === "error") {
      setNotice("当前知识库目录不可访问，无法打开项目说明书。");
      return;
    }

    const existing = findProjectInstructionNote(currentSnapshot.notes, knowledgeBaseId);

    if (existing) {
      activateEditorTab({ kind: "note", id: existing.id }, "tree");
      setNotice(`已打开「${existing.title}」。`);
      return;
    }

    if (isBusy) {
      return;
    }

    beginBusy("正在创建 Agent 说明书...");
    try {
      const nextSnapshot = await createProjectInstruction(currentSnapshot, knowledgeBaseId);
      const nextNote =
        getActiveNote(nextSnapshot) ?? findProjectInstructionNote(nextSnapshot.notes, knowledgeBaseId);
      commitSnapshot(nextSnapshot);
      if (nextNote) {
        setOpenFileTabs((currentTabs) => [
          ...currentTabs.filter((tab) => tab.kind !== "note" || tab.id !== nextNote.id),
          { kind: "note", id: nextNote.id },
        ]);
      }
      setSearchTerm("");
      setMarkdownViewMode("edit");
      setNotice("已创建 AGENTS.md。");
    } catch (error) {
      setNotice(formatErrorMessage(error));
    } finally {
      endBusy();
    }
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
        if (nextNote) {
          setOpenFileTabs((currentTabs) => [...currentTabs.filter((tab) => tab.kind !== "note" || tab.id !== nextNote.id), { kind: "note", id: nextNote.id }]);
        }
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
        if (nextDocument) {
          setOpenFileTabs((currentTabs) => [
            ...currentTabs.filter((tab) => tab.kind !== "document" || tab.id !== nextDocument.id),
            { kind: "document", id: nextDocument.id },
          ]);
        }
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

  /** 保存指定标签的草稿；返回 false 表示保存失败或目标已不存在，调用方不得继续关闭。 */
  async function saveDirtyEditorTab(tab: EditorFileTab) {
    if (tab.kind === "note") {
      const note = currentSnapshot.notes.find((item) => item.id === tab.id);

      if (!note || !dirtyNoteIds.has(tab.id)) {
        return Boolean(note);
      }

      beginBusy("正在保存 Markdown...");
      try {
        const expectedHash = editingBaseHashes[note.id] ?? note.contentHash;
        const nextSnapshot = await saveNoteContent(currentSnapshot, note.id, note.content, expectedHash);
        const nextDirtyNoteIds = new Set(dirtyNoteIds);

        nextDirtyNoteIds.delete(note.id);
        commitSnapshot(nextSnapshot, nextDirtyNoteIds);
        setNotice(`已保存「${note.title}」。`);
        return true;
      } catch (error) {
        setNotice(error instanceof Error ? error.message : String(error));
        return false;
      } finally {
        endBusy();
      }
    }

    const document = currentSnapshot.documents.find((item) => item.id === tab.id);

    if (!document || document.fileType !== "txt" || !dirtyDocumentIds.has(tab.id)) {
      return Boolean(document);
    }

    beginBusy("正在保存 TXT...");
    try {
      const expectedHash = editingBaseDocumentHashes[document.id] ?? document.contentHash;
      const nextSnapshot = await saveDocumentContent(currentSnapshot, document.id, document.content ?? "", expectedHash);
      const nextDirtyDocumentIds = new Set(dirtyDocumentIds);

      nextDirtyDocumentIds.delete(document.id);
      commitSnapshot(nextSnapshot, dirtyNoteIds, nextDirtyDocumentIds);
      setNotice(`已保存「${document.title}」。`);
      return true;
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
      return false;
    } finally {
      endBusy();
    }
  }

  /** 保存当前笔记草稿，后端会用开始编辑时的 hash 检测外部编辑器冲突。 */
  async function handleSaveActiveNote() {
    if (activeNote) {
      await saveDirtyEditorTab({ kind: "note", id: activeNote.id });
    }
  }

  /** 保存当前 txt 草稿，后端会用开始编辑时的 hash 检测外部编辑器冲突。 */
  async function handleSaveActiveDocument() {
    if (activeDocument) {
      await saveDirtyEditorTab({ kind: "document", id: activeDocument.id });
    }
  }

  /** 从标签栏移除文件；当前标签关闭时优先选择右侧标签。 */
  function closeEditorTab(tab: EditorFileTab, discardDraft = false) {
    const tabIndex = openFileTabs.findIndex((item) => item.kind === tab.kind && item.id === tab.id);

    if (tabIndex < 0) {
      return;
    }

    const isDirty = tab.kind === "note" ? dirtyNoteIds.has(tab.id) : dirtyDocumentIds.has(tab.id);

    if (isDirty && !discardDraft) {
      requestConfirmation(
        {
          title: "关闭未保存的文件",
          message: "此文件包含未保存的更改。保存后关闭，或放弃本次更改。",
          confirmLabel: "保存并关闭",
          cancelLabel: "取消",
          tone: "default",
          thirdAction: { label: "放弃更改", tone: "danger" },
        },
        async () => {
          if (await saveDirtyEditorTab(tab)) {
            closeEditorTab(tab, true);
          }
        },
        () => closeEditorTab(tab, true),
      );
      return;
    }

    const nextTabs = openFileTabs.filter((item) => item.kind !== tab.kind || item.id !== tab.id);
    const isActive = (tab.kind === "note" && currentSnapshot.activeNoteId === tab.id) ||
      (tab.kind === "document" && currentSnapshot.activeDocumentId === tab.id);

    // “放弃更改”关闭后必须清理 dirty 集合，否则后续删除/重命名会被不可见草稿错误阻止。
    if (isDirty) {
      if (tab.kind === "note") {
        setDirtyNoteIds((currentIds) => {
          const nextIds = new Set(currentIds);

          nextIds.delete(tab.id);
          return nextIds;
        });
      } else {
        setDirtyDocumentIds((currentIds) => {
          const nextIds = new Set(currentIds);

          nextIds.delete(tab.id);
          return nextIds;
        });
      }
    }
    setOpenFileTabs(nextTabs);
    logInfo("关闭编辑器文件标签。", {
      category: "frontend",
      event: "editor_tab_close",
      status: "completed",
      metadata: { fileKind: tab.kind, openTabCount: nextTabs.length, hasUnsavedDraft: isDirty },
    });

    if (!isActive) {
      return;
    }

    const nextActiveTab = nextTabs[tabIndex] ?? nextTabs[tabIndex - 1];

    if (nextActiveTab) {
      activateEditorTab(nextActiveTab, "tab");
      return;
    }

    // 关闭最后一个标签后显式清空焦点，selectors 不会再自动回退到首个文件。
    commitSnapshot({ ...currentSnapshot, activeNoteId: "", activeDocumentId: "" });
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

      if (nextSnapshot.activeNoteId) {
        replaceEditorTabId("note", note.id, nextSnapshot.activeNoteId);
      }
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

      if (nextSnapshot.activeDocumentId) {
        replaceEditorTabId("document", document.id, nextSnapshot.activeDocumentId);
      }
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

  return {
    activateEditorTab,
    replaceEditorTabId,
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
    saveDirtyEditorTab,
    handleSaveActiveNote,
    handleSaveActiveDocument,
    closeEditorTab,
    handleExportActiveFile,
    openRenameDialog,
    openRenameDocumentDialog,
    handleSubmitRename,
    handleSubmitRenameDocument,
    handleDeleteNote,
    handleDeleteDocument,
    handleCreateOrOpenProjectInstruction,
  };
}
