import type { AgentSession, KnowledgeBase, Note, WorkspaceSnapshot } from "./types";

/** 获取当前激活知识库，缺失时回退到第一个知识库。 */
export function getActiveKnowledgeBase(snapshot: WorkspaceSnapshot): KnowledgeBase {
  return (
    snapshot.knowledgeBases.find((knowledgeBase) => knowledgeBase.id === snapshot.activeKnowledgeBaseId) ??
    snapshot.knowledgeBases[0]
  );
}

/** 获取当前激活笔记，缺失时回退到激活知识库的第一篇笔记；空知识库返回 undefined。 */
export function getActiveNote(snapshot: WorkspaceSnapshot): Note | undefined {
  const activeKnowledgeBase = getActiveKnowledgeBase(snapshot);
  const activeKnowledgeBaseNotes = snapshot.notes.filter((note) => note.knowledgeBaseId === activeKnowledgeBase.id);

  return (
    activeKnowledgeBaseNotes.find((note) => note.id === snapshot.activeNoteId) ??
    activeKnowledgeBaseNotes[0] ??
    undefined
  );
}

/** 获取当前激活会话，缺失时回退到第一个会话。 */
export function getActiveSession(snapshot: WorkspaceSnapshot): AgentSession {
  return snapshot.sessions.find((session) => session.id === snapshot.activeSessionId) ?? snapshot.sessions[0];
}

/** 获取当前激活知识库下的全部笔记。 */
export function getActiveKnowledgeBaseNotes(snapshot: WorkspaceSnapshot): Note[] {
  const activeKnowledgeBase = getActiveKnowledgeBase(snapshot);

  return snapshot.notes.filter((note) => note.knowledgeBaseId === activeKnowledgeBase.id);
}

/** 返回会话绑定的知识库名称摘要。 */
export function getSessionKnowledgeBaseLabel(session: AgentSession, knowledgeBases: KnowledgeBase[]) {
  const names = session.knowledgeBaseIds
    .map((knowledgeBaseId) => knowledgeBases.find((knowledgeBase) => knowledgeBase.id === knowledgeBaseId)?.name)
    .filter(Boolean);

  return names.length ? names.join(" / ") : "未绑定知识库";
}

/** 返回会话绑定的笔记名称摘要。 */
export function getSessionNoteLabel(session: AgentSession, notes: Note[]) {
  if (!session.activeNoteId) {
    return "未绑定具体笔记";
  }

  return notes.find((note) => note.id === session.activeNoteId)?.title ?? "笔记已移动";
}

/** 汇总会话检索范围，用于范围选择器按钮。 */
export function getScopeSummaryLabel(session: AgentSession, knowledgeBases: KnowledgeBase[]) {
  const selectedNames = session.knowledgeBaseIds
    .map((knowledgeBaseId) => knowledgeBases.find((knowledgeBase) => knowledgeBase.id === knowledgeBaseId)?.name)
    .filter(Boolean);

  if (selectedNames.length <= 1) {
    return selectedNames[0] ?? "未选择知识库";
  }

  return `${selectedNames.length} 个知识库`;
}

/** 把会话类型转成界面可读标签。 */
export function getSessionTypeLabel(type: AgentSession["type"]) {
  const labels: Record<AgentSession["type"], string> = {
    note: "笔记会话",
    "knowledge-base": "知识库会话",
    task: "任务会话",
  };

  return labels[type];
}
