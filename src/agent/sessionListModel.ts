import {
  getImSessionSourceLabel,
  getSessionKnowledgeBaseLabel,
} from "../shared/selectors";
import type { AgentSession, KnowledgeBase, ProposedChange, ProposedChangeSet, ProposedFileOperation } from "../shared/types";

/** 会话列表一次只能叠一个筛选；关键词另外生效。 */
export type SessionListFacet = "all" | "current-knowledge-base" | "pending" | "local" | "im" | "schedule" | "archived";

/** 无关键词时的分组。置顶单独成组，其余按 updatedAt 落到时间桶。 */
export type SessionTimeGroupId = "pinned" | "today" | "yesterday" | "week" | "earlier";

export interface SessionListGroup {
  id: SessionTimeGroupId;
  label: string;
  sessions: AgentSession[];
}

/** 从文件反查会话时使用的身份。笔记和普通文档都用同一组字段。 */
export interface SessionFileRef {
  id: string;
  knowledgeBaseId: string;
  path: string;
}

/** 筛选菜单的固定顺序。 */
export const SESSION_LIST_FACETS: Array<{ id: SessionListFacet; label: string }> = [
  { id: "all", label: "全部" },
  { id: "current-knowledge-base", label: "含当前知识库" },
  { id: "pending", label: "待确认" },
  { id: "local", label: "本地" },
  { id: "im", label: "即时通讯" },
  { id: "schedule", label: "定时" },
  { id: "archived", label: "已归档" },
];

const TIME_GROUP_ORDER: SessionTimeGroupId[] = ["pinned", "today", "yesterday", "week", "earlier"];

const TIME_GROUP_LABELS: Record<SessionTimeGroupId, string> = {
  pinned: "置顶",
  today: "今天",
  yesterday: "昨天",
  week: "近 7 天",
  earlier: "更早",
};

/** 摘录在关键词两侧各留的字数。侧栏第二行放不下整段消息。 */
const SNIPPET_RADIUS = 18;

/** 第二行在没有摘录时展示的来源和知识库范围。 */
export function sessionSubtitle(session: AgentSession, knowledgeBases: KnowledgeBase[]) {
  const imLabel = getImSessionSourceLabel(session);
  if (imLabel) {
    const preview = session.imIdentity?.lastMessagePreview;
    return preview ? `${imLabel} · ${preview}` : imLabel;
  }

  const knowledgeBaseLabel = getSessionKnowledgeBaseLabel(session, knowledgeBases);
  if (session.scheduleIdentity) {
    return `定时任务 · ${knowledgeBaseLabel}`;
  }

  return knowledgeBaseLabel;
}

/** 关键词是否命中标题、范围、正文、引用、工具摘要或待确认路径。 */
export function sessionMatchesQuery(session: AgentSession, knowledgeBases: KnowledgeBase[], term: string) {
  const needle = term.trim().toLowerCase();
  if (!needle) {
    return true;
  }

  const header = [session.title, sessionSubtitle(session, knowledgeBases), session.scheduleIdentity?.jobName ?? ""]
    .join(" ")
    .toLowerCase();
  if (header.includes(needle)) {
    return true;
  }

  return searchableParts(session).some((part) => part.text.toLowerCase().includes(needle));
}

/**
 * 命中正文、引用或路径时返回摘录。
 * 只命中标题或知识库名时返回空，列表继续显示原来的第二行。
 */
export function sessionSearchSnippet(session: AgentSession, term: string) {
  const needle = term.trim().toLowerCase();
  if (!needle) {
    return null;
  }

  const parts = searchableParts(session);
  const userHit = parts.find((part) => part.kind === "user" && part.text.toLowerCase().includes(needle));
  const hit = userHit ?? parts.find((part) => part.text.toLowerCase().includes(needle));
  if (!hit) {
    return null;
  }

  return clipSnippet(hit.text, needle);
}

/** 当前筛选是否命中。归档的显隐由 isSessionVisible 单独处理。 */
export function sessionMatchesFacet(session: AgentSession, facet: SessionListFacet, activeKnowledgeBaseId?: string) {
  switch (facet) {
    case "current-knowledge-base":
      if (!activeKnowledgeBaseId) {
        return false;
      }
      return session.knowledgeBaseIds.includes(activeKnowledgeBaseId);
    case "pending":
      return hasPendingWrite(session);
    case "local":
      return !session.imIdentity && !session.scheduleIdentity;
    case "im":
      return Boolean(session.imIdentity);
    case "schedule":
      return Boolean(session.scheduleIdentity);
    case "all":
    case "archived":
      return true;
    default:
      return true;
  }
}

/** 关键词、筛选和归档一起决定这一行出不出现。运行中、排队和当前会话不被筛选藏起来。 */
export function isSessionVisible(
  session: AgentSession,
  options: {
    knowledgeBases: KnowledgeBase[];
    term: string;
    facet: SessionListFacet;
    activeKnowledgeBaseId?: string;
    isActive: boolean;
    isRunning: boolean;
    isQueued: boolean;
  },
) {
  const term = options.term.trim().toLowerCase();
  if (term && !sessionMatchesQuery(session, options.knowledgeBases, term)) {
    return false;
  }

  const exception = options.isActive || options.isRunning || options.isQueued;
  const archived = Boolean(session.archivedAt);

  if (options.facet === "archived") {
    return archived || exception;
  }

  // 归档默认收起。正在搜时仍可命中，避免归档之后再也找不到。
  if (archived && !term && !exception) {
    return false;
  }

  if (exception) {
    return true;
  }

  return sessionMatchesFacet(session, options.facet, options.activeKnowledgeBaseId);
}

/** 按置顶和时间把已经筛过的会话分组。调用方需先按 updatedAt 降序排好。 */
export function groupSessionsForList(sessions: AgentSession[], now: Date) {
  const buckets = new Map<SessionTimeGroupId, AgentSession[]>();
  for (const session of sessions) {
    const groupId = session.pinnedAt ? "pinned" : sessionTimeGroup(session.updatedAt, now);
    const bucket = buckets.get(groupId) ?? [];
    bucket.push(session);
    buckets.set(groupId, bucket);
  }

  return TIME_GROUP_ORDER.flatMap((id) => {
    const grouped = buckets.get(id);
    if (!grouped?.length) {
      return [];
    }
    return [{ id, label: TIME_GROUP_LABELS[id], sessions: grouped }];
  });
}

/** 把 updatedAt 收成列表分组。认「刚刚」、`今天 HH:MM` 和 `YYYY/MM/DD HH:MM`。 */
export function sessionTimeGroup(value: string, now: Date): Exclude<SessionTimeGroupId, "pinned"> {
  const parsed = sessionUpdatedAtDate(value, now);
  if (!parsed) {
    return "earlier";
  }

  const diffDays = Math.round((startOfDay(now).getTime() - startOfDay(parsed).getTime()) / 86_400_000);
  if (diffDays <= 0) {
    return "today";
  }
  if (diffDays === 1) {
    return "yesterday";
  }
  if (diffDays < 7) {
    return "week";
  }
  return "earlier";
}

/** 找出引用、@ 或待确认变更碰到这个文件的会话，保留传入顺序。 */
export function sessionsTouchingFile(sessions: AgentSession[], file: SessionFileRef) {
  return sessions.filter((session) => sessionTouchesFile(session, file));
}

function sessionTouchesFile(session: AgentSession, file: SessionFileRef) {
  for (const message of session.messages) {
    if (file.id && message.mentionedFileIds?.includes(file.id)) {
      return true;
    }
    for (const citation of message.citations ?? []) {
      if (file.id && citation.noteId === file.id) {
        return true;
      }
      if (citation.knowledgeBaseId === file.knowledgeBaseId && samePath(citation.path, file.path)) {
        return true;
      }
    }
  }

  if (changeTouchesFile(session.pendingChange, file)) {
    return true;
  }

  return (session.pendingChangeSet?.operations ?? []).some((operation) => operationTouchesFile(operation, file));
}

function changeTouchesFile(change: ProposedChange | undefined, file: SessionFileRef) {
  if (!change) {
    return false;
  }
  if (file.id && (change.targetId === file.id || change.noteId === file.id)) {
    return true;
  }
  return change.knowledgeBaseId === file.knowledgeBaseId && samePath(change.targetPath, file.path);
}

function operationTouchesFile(operation: ProposedFileOperation, file: SessionFileRef) {
  if (operation.knowledgeBaseId !== file.knowledgeBaseId) {
    return false;
  }
  return samePath(operation.targetPath, file.path) || samePath(operation.sourcePath ?? "", file.path);
}

function samePath(left: string, right: string) {
  const normalize = (value: string) => value.trim().replace(/\\/g, "/");
  const normalizedLeft = normalize(left);
  const normalizedRight = normalize(right);
  return normalizedLeft.length > 0 && normalizedLeft === normalizedRight;
}

function hasPendingWrite(session: AgentSession) {
  return session.pendingChange?.status === "pending" || session.pendingChangeSet?.status === "pending";
}

type SearchPart = { kind: "user" | "assistant" | "other"; text: string };

function searchableParts(session: AgentSession) {
  const parts: SearchPart[] = [];
  for (const message of session.messages) {
    if (message.content.trim()) {
      parts.push({ kind: message.role === "user" ? "user" : "assistant", text: message.content });
    }
    for (const citation of message.citations ?? []) {
      pushText(parts, citation.title);
      pushText(parts, citation.path);
    }
    for (const toolCall of message.toolCalls ?? []) {
      pushText(parts, toolCall.summary);
    }
  }

  pushChangeText(parts, session.pendingChange);
  pushChangeSetText(parts, session.pendingChangeSet);
  return parts;
}

function pushChangeText(parts: SearchPart[], change: ProposedChange | undefined) {
  if (!change) {
    return;
  }
  pushText(parts, change.title);
  pushText(parts, change.targetPath);
}

function pushChangeSetText(parts: SearchPart[], changeSet: ProposedChangeSet | undefined) {
  if (!changeSet) {
    return;
  }
  pushText(parts, changeSet.summary);
  for (const operation of changeSet.operations) {
    pushText(parts, operation.targetPath);
    pushText(parts, operation.sourcePath ?? "");
  }
}

function pushText(parts: SearchPart[], text: string) {
  if (text.trim()) {
    parts.push({ kind: "other", text });
  }
}

function clipSnippet(text: string, needle: string) {
  const normalized = text.replace(/\s+/g, " ").trim();
  const index = normalized.toLowerCase().indexOf(needle.toLowerCase());
  if (index < 0) {
    return null;
  }

  const start = Math.max(0, index - SNIPPET_RADIUS);
  const end = Math.min(normalized.length, index + needle.length + SNIPPET_RADIUS);
  const prefix = start > 0 ? "…" : "";
  const suffix = end < normalized.length ? "…" : "";
  return `${prefix}${normalized.slice(start, end)}${suffix}`;
}

function sessionUpdatedAtDate(value: string, now: Date) {
  const trimmed = value.trim();
  if (trimmed === "刚刚" || trimmed === "未保存") {
    return now;
  }

  const todayClock = trimmed.match(/^今天\s+(\d{1,2}):(\d{2})$/);
  if (todayClock) {
    const date = new Date(now);
    date.setHours(Number(todayClock[1]), Number(todayClock[2]), 0, 0);
    return date;
  }

  const match = trimmed.match(/^(\d{4})[/.年-](\d{1,2})[/.月-](\d{1,2})日?(?:\s+(\d{1,2}):(\d{2}))?/);
  if (!match) {
    return null;
  }

  return new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]), Number(match[4] ?? 0), Number(match[5] ?? 0));
}

function startOfDay(date: Date) {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}
