import { invokeLogged, isTauriRuntime } from "./runtime";
import { cloneWorkspaceSnapshot } from "../mock/workspace";
import {
  getFallbackDocumentId,
  getSessionNoteId,
  normalizeMockSnapshotSessions,
  orderValidKnowledgeBaseIds,
} from "../mock/browser";
import { rewindSessionToUserMessage } from "../../workspace/sessionUtils";
import {
  AgentSession,
  KnowledgeBase,
  Note,
  WorkspaceSnapshot,
} from "../types";

/** 读取持久化 Agent 会话，浏览器中返回按当前快照清理后的会话列表。 */
export async function loadSessions(snapshot: WorkspaceSnapshot): Promise<AgentSession[]> {
  if (!isTauriRuntime()) {
    return normalizeMockSnapshotSessions(cloneWorkspaceSnapshot(snapshot)).sessions;
  }

  return invokeLogged<AgentSession[]>("load_sessions", { payload: { snapshot } });
}

/** 保存单个 Agent 会话，并返回后端归一化后的工作台快照。 */
export async function saveSession(snapshot: WorkspaceSnapshot, session: AgentSession): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const sessionIndex = nextSnapshot.sessions.findIndex((item) => item.id === session.id);

    if (sessionIndex >= 0) {
      nextSnapshot.sessions[sessionIndex] = session;
    } else {
      nextSnapshot.sessions = [session, ...nextSnapshot.sessions];
    }

    const viewerSessionId = snapshot.activeSessionId;
    if (!viewerSessionId || !nextSnapshot.sessions.some((item) => item.id === viewerSessionId)) {
      nextSnapshot.activeSessionId = session.id;
    }

    return normalizeMockSnapshotSessions(nextSnapshot);
  }

  return invokeLogged<WorkspaceSnapshot>("save_session", { payload: { snapshot, session } });
}

/** 逻辑删除 Agent 会话；持久化记录保留 deletedAt，但普通会话列表不再展示。 */
export async function deleteSession(snapshot: WorkspaceSnapshot, sessionId: string): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const deletedSession = nextSnapshot.sessions.find((session) => session.id === sessionId);

    if (!deletedSession) {
      return normalizeMockSnapshotSessions(nextSnapshot);
    }

    deletedSession.deletedAt = "刚刚";
    deletedSession.updatedAt = "刚刚";
    nextSnapshot.sessions = nextSnapshot.sessions.filter((session) => !session.deletedAt);

    if (!nextSnapshot.sessions.some((session) => session.id === nextSnapshot.activeSessionId)) {
      nextSnapshot.activeSessionId =
        nextSnapshot.sessions.find((session) => session.knowledgeBaseIds.includes(nextSnapshot.activeKnowledgeBaseId))?.id ?? "";
    }

    return normalizeMockSnapshotSessions(nextSnapshot);
  }

  return invokeLogged<WorkspaceSnapshot>("delete_session", { payload: { snapshot, sessionId } });
}

/** 更新当前会话工具范围；桌面端会强制保留激活知识库。 */
export async function updateSessionScope(
  snapshot: WorkspaceSnapshot,
  sessionId: string,
  knowledgeBaseIds: string[],
  activeKnowledgeBaseId: string,
): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const validIds = new Set(nextSnapshot.knowledgeBases.map((knowledgeBase) => knowledgeBase.id));
    const selectedIds = new Set(knowledgeBaseIds.filter((knowledgeBaseId) => validIds.has(knowledgeBaseId)));

    selectedIds.add(activeKnowledgeBaseId);
    nextSnapshot.sessions = nextSnapshot.sessions.map((session) =>
      session.id === sessionId
        ? {
            ...session,
            knowledgeBaseIds: orderValidKnowledgeBaseIds(Array.from(selectedIds), nextSnapshot.knowledgeBases),
            updatedAt: "刚刚",
          }
        : session,
    );

    return normalizeMockSnapshotSessions(nextSnapshot);
  }

  return invokeLogged<WorkspaceSnapshot>("update_session_scope", {
    payload: { snapshot, sessionId, knowledgeBaseIds, activeKnowledgeBaseId },
  });
}

/** 恢复历史会话绑定的知识库和会话焦点；文件焦点只在会话仍有有效笔记引用时同步。 */
export async function restoreSessionContext(snapshot: WorkspaceSnapshot, sessionId: string): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = normalizeMockSnapshotSessions(cloneWorkspaceSnapshot(snapshot));
    const session = nextSnapshot.sessions.find((item) => item.id === sessionId);

    if (!session) {
      return nextSnapshot;
    }

    const nextKnowledgeBaseId =
      session.knowledgeBaseIds.find((knowledgeBaseId) =>
        nextSnapshot.knowledgeBases.some((knowledgeBase) => knowledgeBase.id === knowledgeBaseId),
      ) ??
      nextSnapshot.knowledgeBases[0]?.id ??
      "";
    const nextNoteId = getSessionNoteId(nextSnapshot, session.activeNoteId, nextKnowledgeBaseId);
    const shouldKeepCurrentFile = nextSnapshot.activeKnowledgeBaseId === nextKnowledgeBaseId;

    nextSnapshot.activeSessionId = session.id;
    nextSnapshot.activeKnowledgeBaseId = nextKnowledgeBaseId;

    if (nextNoteId) {
      nextSnapshot.activeNoteId = nextNoteId;
      nextSnapshot.activeDocumentId = "";
    } else if (!shouldKeepCurrentFile) {
      nextSnapshot.activeNoteId = nextSnapshot.notes.find((note) => note.knowledgeBaseId === nextKnowledgeBaseId)?.id ?? "";
      nextSnapshot.activeDocumentId = getFallbackDocumentId(nextSnapshot, nextKnowledgeBaseId, nextSnapshot.activeNoteId);
    }

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("restore_session_context", { payload: { snapshot, sessionId } });
}

/** 截断会话到指定用户消息并删除模型 transcript，供随后用同一消息 ID 重跑。 */
export async function rewindAgentSession(
  snapshot: WorkspaceSnapshot,
  sessionId: string,
  messageId: string,
  prompt: string,
): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const sessionIndex = nextSnapshot.sessions.findIndex((session) => session.id === sessionId);
    if (sessionIndex < 0) {
      throw new Error("找不到要编辑的会话。");
    }

    nextSnapshot.sessions[sessionIndex] = rewindSessionToUserMessage(nextSnapshot.sessions[sessionIndex], messageId, prompt);
    return normalizeMockSnapshotSessions(nextSnapshot);
  }

  return invokeLogged<WorkspaceSnapshot>("rewind_agent_session", {
    payload: { snapshot, sessionId, messageId, prompt },
  });
}
