import { logInfo } from "../shared/logger";
import { attachKnowledgeBase, removeKnowledgeBase, rescanKnowledgeBase, selectKnowledgeBaseDirectory } from "../shared/tauriApi";
import type { EditorFileTab } from "../shared/types";
import { buildScanNotice } from "./knowledgeUtils";
import { resolveKnowledgeBaseSessionId } from "./sessionUtils";
import type { WorkspaceChrome } from "./workspaceChrome";

interface KnowledgeBaseActionsOptions extends WorkspaceChrome {
  setSearchTerm: (value: string) => void;
  setCollapsedFolderPaths: (value: Set<string> | ((current: Set<string>) => Set<string>)) => void;
  setOpenFileTabs: (value: EditorFileTab[] | ((current: EditorFileTab[]) => EditorFileTab[])) => void;
  setIsSettingsOpen: (value: boolean) => void;
  dirtyNoteIds: Set<string>;
  dirtyDocumentIds: Set<string>;
}

const noopAsync = async () => {};
const noop = (..._args: unknown[]) => {};

/** 知识库授权、扫描和目录树折叠动作。 */
export function useKnowledgeBaseActions({
  snapshot,
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
}: KnowledgeBaseActionsOptions) {
  if (!snapshot) {
    return {
      handleSelectKnowledgeBase: noopAsync,
      handleAddKnowledgeBase: noopAsync,
      handleToggleFolder: noop,
      expandFolderPaths: noop,
      handleRescanKnowledgeBase: noopAsync,
      handleRemoveKnowledgeBase: noopAsync,
    };
  }

  const currentSnapshot = snapshot;

  /** 选择知识库时只切换浏览焦点；会话最多切到该知识库已有会话，不再隐式创建。 */
  async function handleSelectKnowledgeBase(knowledgeBaseId: string) {
    const nextKnowledgeBase = currentSnapshot.knowledgeBases.find((knowledgeBase) => knowledgeBase.id === knowledgeBaseId);
    const nextNotes = currentSnapshot.notes.filter((note) => note.knowledgeBaseId === knowledgeBaseId);
    const nextDocuments = currentSnapshot.documents.filter((document) => document.knowledgeBaseId === knowledgeBaseId);

    if (!nextKnowledgeBase) {
      return;
    }

    const nextActiveSessionId = resolveKnowledgeBaseSessionId(currentSnapshot, nextKnowledgeBase.id);
    const activatedSnapshot = {
      ...currentSnapshot,
      activeKnowledgeBaseId: knowledgeBaseId,
      activeNoteId: nextNotes[0]?.id ?? "",
      activeDocumentId: nextNotes[0] ? "" : nextDocuments[0]?.id ?? "",
      activeSessionId: nextActiveSessionId,
    };

    logInfo("切换知识库浏览焦点。", {
      category: "frontend",
      event: "select_knowledge_base",
      status: "completed",
      metadata: {
        hasExistingSession: Boolean(nextActiveSessionId),
        noteCount: nextNotes.length,
        documentCount: nextDocuments.length,
      },
    });
    commitSnapshot(activatedSnapshot);
    const activatedTab = activatedSnapshot.activeDocumentId
      ? { kind: "document" as const, id: activatedSnapshot.activeDocumentId }
      : activatedSnapshot.activeNoteId
        ? { kind: "note" as const, id: activatedSnapshot.activeNoteId }
        : null;

    if (activatedTab) {
      setOpenFileTabs((currentTabs) =>
        currentTabs.some((tab) => tab.kind === activatedTab.kind && tab.id === activatedTab.id)
          ? currentTabs
          : [...currentTabs, activatedTab],
      );
    }
    setSearchTerm("");
    setCollapsedFolderPaths(new Set());
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

  return {
    handleSelectKnowledgeBase,
    handleAddKnowledgeBase,
    handleToggleFolder,
    expandFolderPaths,
    handleRescanKnowledgeBase,
    handleRemoveKnowledgeBase,
  };
}
