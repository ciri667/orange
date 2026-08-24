import { invokeLogged, isTauriRuntime } from "./runtime";
import { createMockWorkspaceSnapshot } from "../mock/workspace";
import { browserMock, resetBrowserDiskContents } from "../mock/browser";
import {
  KnowledgeBase,
  Note,
  WorkspaceBootstrapState,
  WorkspaceEditorState,
  WorkspaceSnapshot,
} from "../types";

/** 从 Tauri 本地层加载工作台状态，浏览器中回退到 mock 数据。 */
export async function loadWorkspaceState(): Promise<WorkspaceBootstrapState> {
  if (!isTauriRuntime()) {
    const snapshot = createMockWorkspaceSnapshot();

    browserMock.documentHistoryEntries = [];
    browserMock.documentHistoryContents = new Map();
    resetBrowserDiskContents(snapshot);

    const activeTab = snapshot.activeDocumentId
      ? { kind: "document" as const, id: snapshot.activeDocumentId }
      : snapshot.activeNoteId
        ? { kind: "note" as const, id: snapshot.activeNoteId }
        : undefined;

    return {
      snapshot,
      editorState: {
        activeKnowledgeBaseId: snapshot.activeKnowledgeBaseId,
        openTabs: activeTab ? [activeTab] : [],
        activeTab,
        updatedAt: "浏览器开发态",
      },
    };
  }

  return invokeLogged<WorkspaceBootstrapState>("load_workspace_state");
}

/** 保存编辑器会话；浏览器开发态不落盘，桌面端由 SQLite 统一持久化。 */
export async function saveWorkspaceEditorState(state: WorkspaceEditorState): Promise<WorkspaceEditorState> {
  if (!isTauriRuntime()) {
    return state;
  }

  return invokeLogged<WorkspaceEditorState>("save_workspace_editor_state", { editorState: state });
}
