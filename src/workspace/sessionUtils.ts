import type { AgentMentionFile } from "../agent/AgentInput";
import { createLocalId, formatLocalDateTime } from "../shared/id";
import type { AgentActionType, AgentMessage, AgentSession, KnowledgeBase, WorkspaceSnapshot } from "../shared/types";

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
export function appendUserMessageToSession(snapshot: WorkspaceSnapshot, session: AgentSession, message: AgentMessage) {
  const nextSession = {
    ...session,
    messages: [...session.messages, message],
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
