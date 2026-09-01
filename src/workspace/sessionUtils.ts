import type { AgentMentionFile } from "../agent/AgentInput";
import { createLocalId, formatLocalDateTime } from "../shared/id";
import type {
  AgentActionType,
  AgentMessage,
  AgentSession,
  FolderEntry,
  KnowledgeBase,
  Note,
  WorkspaceDocument,
  WorkspaceSnapshot,
} from "../shared/types";

/** 空白会话的默认标题；首条用户输入提交后会替换为用户原始输入。 */
export const DEFAULT_SESSION_TITLE = "新会话";

/** 未持久化的占位会话 ID，只用于没有当前知识库会话时驱动侧栏展示。 */
export const DRAFT_SESSION_ID = "__draft-session__";

/** 新建 Agent 会话对象，作为消息、检索范围和待确认 diff 的容器。 */
export function buildAgentSession({
  knowledgeBase,
  title = DEFAULT_SESSION_TITLE,
  knowledgeBaseIds,
  securityLevel = "basic",
}: {
  knowledgeBase: KnowledgeBase;
  title?: string;
  knowledgeBaseIds?: string[];
  securityLevel?: AgentSession["securityLevel"];
}): AgentSession {
  /** 会话创建时间需要长期可辨认，避免历史列表里多个“刚刚”无法区分。 */
  const createdAt = formatLocalDateTime();

  return {
    id: createLocalId("session-knowledge-base"),
    title,
    type: "knowledge-base",
    knowledgeBaseIds: knowledgeBaseIds?.length ? knowledgeBaseIds : [knowledgeBase.id],
    pinnedNoteIds: [],
    messages: [],
    securityLevel,
    createdAt,
    updatedAt: createdAt,
  };
}

/** 构造未落库的侧栏占位会话，避免仅切换文档时隐式创建真实会话。 */
export function buildDraftAgentSession(knowledgeBase: KnowledgeBase): AgentSession {
  return {
    id: DRAFT_SESSION_ID,
    title: DEFAULT_SESSION_TITLE,
    type: "knowledge-base",
    knowledgeBaseIds: [knowledgeBase.id],
    pinnedNoteIds: [],
    messages: [],
    securityLevel: "basic",
    createdAt: "未保存",
    updatedAt: "未保存",
  };
}

/** 根据会话授权范围构造 @ picker 的公开文件清单；不携带正文或绝对路径。 */
export function buildMentionableFiles(snapshot: WorkspaceSnapshot, session: AgentSession): AgentMentionFile[] {
  const scopeIds = new Set(session.knowledgeBaseIds);
  const notes = snapshot.notes
    .filter((note) => scopeIds.has(note.knowledgeBaseId))
    .map<AgentMentionFile>((note) => ({
      id: note.id,
      displayName: note.title,
      relativePath: note.path,
      kind: "markdown",
    }));
  const documents = snapshot.documents
    .filter((document) => scopeIds.has(document.knowledgeBaseId))
    .map<AgentMentionFile>((document) => ({
      id: document.id,
      displayName: document.title,
      relativePath: document.path,
      kind: document.fileType === "txt" ? "text" : document.fileType,
    }));

  // 按路径排序让同名文件的 picker 顺序稳定，避免每次渲染跳动。
  return [...notes, ...documents].sort((left, right) => left.relativePath.localeCompare(right.relativePath));
}

/** 从当前知识库解析应展示的会话；只复用已有会话，不创建新的历史记录。 */
export function resolveKnowledgeBaseSessionId(snapshot: WorkspaceSnapshot, knowledgeBaseId: string) {
  const activeSession = snapshot.sessions.find((session) => session.id === snapshot.activeSessionId);

  if (activeSession?.knowledgeBaseIds.includes(knowledgeBaseId)) {
    return activeSession.id;
  }

  return snapshot.sessions.find((session) => session.knowledgeBaseIds.includes(knowledgeBaseId))?.id ?? "";
}

/** 获取当前知识库下可用的真实会话；没有时返回 undefined，由 UI 使用草稿会话展示。 */
export function resolveActiveSessionForKnowledgeBase(snapshot: WorkspaceSnapshot, knowledgeBase: KnowledgeBase) {
  const sessionId = resolveKnowledgeBaseSessionId(snapshot, knowledgeBase.id);

  return snapshot.sessions.find((session) => session.id === sessionId);
}

/** 判断会话是否已经持久化在当前快照中，草稿会话不能直接提交给后端 Agent。 */
export function isPersistedSession(snapshot: WorkspaceSnapshot, session: AgentSession) {
  return snapshot.sessions.some((item) => item.id === session.id);
}

/** 首条用户消息会成为会话标题；空输入不会触发提交，保留默认“新会话”。 */
export function buildTitleFromFirstPrompt(prompt: string) {
  return prompt.trim() || DEFAULT_SESSION_TITLE;
}

/** 仅在空白新会话第一次发送消息前允许用用户输入替换标题。 */
export function shouldUseFirstPromptAsTitle(session: AgentSession) {
  return session.title === DEFAULT_SESSION_TITLE && !session.messages.some((message) => message.role === "user");
}

/** 返回替换标题后的快照和会话对象，避免在运行 Agent 前丢失用户首条输入标题。 */
export function applyFirstPromptTitle(snapshot: WorkspaceSnapshot, session: AgentSession, prompt: string) {
  const nextSession = {
    ...session,
    title: buildTitleFromFirstPrompt(prompt),
    updatedAt: formatLocalDateTime(),
  };

  return {
    snapshot: {
      ...snapshot,
      activeSessionId: nextSession.id,
      sessions: snapshot.sessions.map((item) => (item.id === nextSession.id ? nextSession : item)),
    },
    session: nextSession,
  };
}

/** 构造发送后立即展示的用户消息，后端会通过同一 ID 复用并持久化本轮记录。 */
export function buildOptimisticUserMessage(prompt: string, action: AgentActionType, mentionedFileIds: string[]): AgentMessage {
  return {
    id: createLocalId("user"),
    role: "user",
    content: prompt,
    action,
    mentionedFileIds: mentionedFileIds.length ? mentionedFileIds : undefined,
  };
}

/** 把用户消息追加进目标会话，确保 Agent 响应前对话框已经显示用户输入。 */
export function appendUserMessageToSession(
  snapshot: WorkspaceSnapshot,
  session: AgentSession,
  message: AgentMessage,
  options?: { activate?: boolean },
) {
  const nextSession = {
    ...session,
    messages: [...session.messages, message],
    updatedAt: formatLocalDateTime(),
  };
  const activate = options?.activate ?? true;

  return {
    snapshot: {
      ...snapshot,
      ...(activate ? { activeSessionId: nextSession.id } : {}),
      sessions: snapshot.sessions.map((item) => (item.id === nextSession.id ? nextSession : item)),
    },
    session: nextSession,
  };
}

/** 按 id 合并列表：保留当前快照里多出来的项，并按回调决定是否采用回合结果。 */
function mergeItemsById<T extends { id: string }>(
  current: T[],
  incoming: T[],
  shouldTakeIncoming: (currentItem: T | undefined, incomingItem: T) => boolean,
): T[] {
  const currentById = new Map(current.map((item) => [item.id, item]));
  const incomingById = new Map(incoming.map((item) => [item.id, item]));
  const merged: T[] = [];
  const seen = new Set<string>();

  for (const item of current) {
    const next = incomingById.get(item.id);
    merged.push(next && shouldTakeIncoming(item, next) ? next : item);
    seen.add(item.id);
  }

  for (const item of incoming) {
    if (!seen.has(item.id) && shouldTakeIncoming(currentById.get(item.id), item)) {
      merged.push(item);
    }
  }

  return merged;
}

/** 把一轮 Agent 结果合并进当前工作台，不抢焦点、不丢其它会话。 */
export function mergeSessionTurn(
  current: WorkspaceSnapshot,
  turnSnapshot: WorkspaceSnapshot,
  turnSessionId: string,
  options?: { dirtyNoteIds?: Set<string>; dirtyDocumentIds?: Set<string> },
): WorkspaceSnapshot {
  const turnSession = turnSnapshot.sessions.find((session) => session.id === turnSessionId);

  if (!turnSession) {
    return current;
  }

  const hasTurnSession = current.sessions.some((session) => session.id === turnSessionId);
  const sessions = hasTurnSession
    ? current.sessions.map((session) => (session.id === turnSessionId ? turnSession : session))
    : [turnSession, ...current.sessions];
  const dirtyNoteIds = options?.dirtyNoteIds ?? new Set<string>();
  const dirtyDocumentIds = options?.dirtyDocumentIds ?? new Set<string>();

  return {
    ...current,
    sessions,
    notes: mergeItemsById<Note>(current.notes, turnSnapshot.notes, (currentNote, incomingNote) => {
      if (currentNote && dirtyNoteIds.has(currentNote.id)) {
        return false;
      }

      return !currentNote || currentNote.contentHash !== incomingNote.contentHash;
    }),
    documents: mergeItemsById<WorkspaceDocument>(
      current.documents,
      turnSnapshot.documents,
      (currentDocument, incomingDocument) => {
        if (currentDocument && dirtyDocumentIds.has(currentDocument.id)) {
          return false;
        }

        return !currentDocument || currentDocument.contentHash !== incomingDocument.contentHash;
      },
    ),
    folders: mergeItemsById<FolderEntry>(current.folders, turnSnapshot.folders, (currentFolder) => !currentFolder),
  };
}

/** 截断到指定用户消息并替换正文；浏览器 mock 与后端不变量对齐。 */
export function rewindSessionToUserMessage(session: AgentSession, messageId: string, prompt: string): AgentSession {
  if (session.imIdentity) {
    throw new Error("即时通讯会话不支持编辑历史消息。");
  }

  const nextPrompt = prompt.trim();
  if (!nextPrompt) {
    throw new Error("消息不能为空。");
  }

  const messageIndex = session.messages.findIndex((message) => message.id === messageId);
  if (messageIndex < 0) {
    throw new Error("找不到要编辑的用户消息。");
  }
  if (session.messages[messageIndex].role !== "user") {
    throw new Error("只能编辑用户消息。");
  }

  const oldContent = session.messages[messageIndex].content;
  const isFirstUser = session.messages.find((message) => message.role === "user")?.id === messageId;
  const messages = session.messages.slice(0, messageIndex + 1).map((message, index) =>
    index === messageIndex ? { ...message, content: nextPrompt } : message,
  );
  const retainedIds = new Set(messages.map((message) => message.id));
  const compactedId = session.contextSummary?.lastCompactedMessageId;
  let contextSummary = session.contextSummary;
  if (!compactedId || !retainedIds.has(compactedId)) {
    contextSummary = undefined;
  } else if (contextSummary) {
    const lastSummarized = contextSummary.lastSummarizedMessageId;
    contextSummary = {
      ...contextSummary,
      lastSummarizedMessageId:
        lastSummarized && !retainedIds.has(lastSummarized) ? messages[messages.length - 1]?.id : lastSummarized,
      pendingChangeSummary: undefined,
    };
  }

  return {
    ...session,
    title: isFirstUser && session.title.trim() === oldContent.trim() ? nextPrompt : session.title,
    messages,
    pendingChange: undefined,
    pendingChangeSet: undefined,
    pendingExecution: undefined,
    contextSummary,
    updatedAt: formatLocalDateTime(),
  };
}

/** 从会话中移除尚未进入模型的排队用户消息，取消排队时回滚乐观写入。 */
export function removeSessionMessage(
  snapshot: WorkspaceSnapshot,
  sessionId: string,
  messageId: string,
): { snapshot: WorkspaceSnapshot; session?: AgentSession } {
  const session = snapshot.sessions.find((item) => item.id === sessionId);

  if (!session) {
    return { snapshot };
  }

  const nextSession = {
    ...session,
    messages: session.messages.filter((message) => message.id !== messageId),
    updatedAt: formatLocalDateTime(),
  };

  return {
    snapshot: {
      ...snapshot,
      sessions: snapshot.sessions.map((item) => (item.id === nextSession.id ? nextSession : item)),
    },
    session: nextSession,
  };
}
