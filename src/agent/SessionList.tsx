import { Loader2, MessageSquareText, Search, Trash2 } from "lucide-react";
import { useState } from "react";
import { Button } from "../shared/Button";
import { listRowClassName } from "../shared/ListRow";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import { cn } from "../shared/cn";
import { focusShellClassName } from "../shared/ui";
import {
  getImSessionSourceLabel,
  getSessionKnowledgeBaseLabel,
} from "../shared/selectors";
import { sessionHasPendingWriteConflict } from "../workspace/sessionUtils";
import type { AgentSession, KnowledgeBase } from "../shared/types";

/** 侧栏和窄屏浮层共用的紧凑会话列表。 */
export function SessionList({
  sessions,
  activeSessionId,
  knowledgeBases,
  inFlightSessionIds = [],
  queuedSessionIds = [],
  filterKnowledgeBaseId,
  activeKnowledgeBaseId,
  onSelectSession,
  onDeleteSession,
  onOpenKnowledgeBase,
}: {
  sessions: AgentSession[];
  activeSessionId: string;
  knowledgeBases: KnowledgeBase[];
  inFlightSessionIds?: string[];
  queuedSessionIds?: string[];
  /** 传入时只展示绑定了该知识库的会话；运行中和当前会话始终保留。 */
  filterKnowledgeBaseId?: string;
  /** 正在浏览的知识库，用来标出会话标签里的当前库。 */
  activeKnowledgeBaseId?: string;
  onSelectSession: (sessionId: string) => void;
  onDeleteSession: (sessionId: string) => void;
  /** 点击会话上的知识库标签时，切到该会话并浏览这个库。 */
  onOpenKnowledgeBase?: (sessionId: string, knowledgeBaseId: string) => void;
}) {
  const [searchTerm, setSearchTerm] = useState("");
  const runningIds = new Set(inFlightSessionIds);
  const queuedIds = new Set(queuedSessionIds);
  const normalizedSearch = searchTerm.trim().toLowerCase();
  const scopedSessions = filterKnowledgeBaseId
    ? sessions.filter((session) =>
        sessionBelongsToKnowledgeBase(session, filterKnowledgeBaseId, session.id === activeSessionId, runningIds, queuedIds),
      )
    : sessions;
  const visibleSessions = normalizedSearch
    ? scopedSessions.filter((session) => sessionMatchesSearch(session, knowledgeBases, normalizedSearch))
    : scopedSessions;

  return (
    <div className="flex h-full min-h-0 flex-1 flex-col">
      {/* 侧栏和浮层都是 overflow-hidden，外扩的焦点描边需要自己的空隙，否则上边和左右会被裁掉。 */}
      <div className="shrink-0 px-1.5 pt-1.5 pb-1.5">
        <label className={cn("flex min-h-8 items-center gap-2 rounded-control bg-white/70 px-2 text-ink-muted", focusShellClassName)}>
          <Search size={14} />
          <input
            className="min-w-0 w-full border-0 bg-transparent text-[13px] outline-0"
            value={searchTerm}
            onChange={(event) => setSearchTerm(event.target.value)}
            placeholder="搜索会话"
            type="search"
            aria-label="搜索会话"
          />
        </label>
      </div>

      <div className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-auto pr-0.5" aria-label="会话列表">
        {visibleSessions.length === 0 ? (
          <p className="px-2.5 py-3 text-xs text-ink-muted">
            {sessions.length === 0
              ? "还没有会话"
              : normalizedSearch
                ? "没有匹配的会话"
                : "当前知识库还没有会话"}
          </p>
        ) : (
          visibleSessions.map((session) => {
            const isActive = session.id === activeSessionId;
            const running = runningIds.has(session.id);
            const queued = queuedIds.has(session.id) && !running;
            const pendingLabel = getPendingSessionLabel(session, sessions);
            const boundKnowledgeBases = getBoundKnowledgeBases(session, knowledgeBases);
            const subtitle = getSessionSubtitle(session, knowledgeBases);
            const showKnowledgeBaseChips = !getImSessionSourceLabel(session) && boundKnowledgeBases.length > 1;
            // 多库标签自己是按钮，不能再套进选择按钮里。
            const interactiveChips = showKnowledgeBaseChips && Boolean(onOpenKnowledgeBase);

            return (
              <div
                key={session.id}
                className={listRowClassName({
                  active: isActive,
                  className: "group relative cursor-pointer items-start py-1.5 pr-1.5 pl-2",
                })}
                onClick={() => onSelectSession(session.id)}
              >
                {/* 未引入 Preflight，按钮必须去掉原生边框，否则标题会画出系统灰框。 */}
                <button
                  className="flex min-w-0 flex-1 items-start gap-2 border-0 bg-transparent p-0 text-left text-inherit"
                  type="button"
                  onClick={(event) => {
                    event.stopPropagation();
                    onSelectSession(session.id);
                  }}
                >
                  {running ? (
                    <Loader2 size={15} className="mt-0.5 shrink-0 animate-spin text-accent" aria-label="运行中" />
                  ) : (
                    <MessageSquareText
                      size={15}
                      className={cn("mt-0.5 shrink-0", isActive ? "text-ink-muted" : "text-ink-soft")}
                    />
                  )}
                  <span className="grid min-w-0 flex-1 gap-0.5">
                    <OverflowTooltipText
                      as="strong"
                      className="block min-w-0 truncate pr-7 text-[13px] font-medium leading-5 text-ink-strong"
                      text={session.title}
                      logArea="agent_session_history_title"
                    />
                    <span className="flex min-w-0 items-center gap-1.5 text-[11px] leading-4 text-ink-muted">
                      {showKnowledgeBaseChips ? (
                        interactiveChips ? (
                          <span className="min-w-0 flex-1" aria-hidden />
                        ) : (
                          <SessionKnowledgeBaseChips
                            sessionId={session.id}
                            knowledgeBases={boundKnowledgeBases}
                            activeKnowledgeBaseId={activeKnowledgeBaseId}
                            scheduled={Boolean(session.scheduleIdentity)}
                          />
                        )
                      ) : (
                        <OverflowTooltipText
                          className="min-w-0 flex-1 truncate"
                          text={subtitle}
                          logArea="agent_session_history_scope"
                        />
                      )}
                      <SessionMetaTrailing queued={queued} pendingLabel={pendingLabel} updatedAt={session.updatedAt} />
                    </span>
                  </span>
                </button>
                {interactiveChips ? (
                  <div className="absolute bottom-1.5 left-8 right-16 z-[1] min-w-0">
                    <SessionKnowledgeBaseChips
                      sessionId={session.id}
                      knowledgeBases={boundKnowledgeBases}
                      activeKnowledgeBaseId={activeKnowledgeBaseId}
                      scheduled={Boolean(session.scheduleIdentity)}
                      onOpenKnowledgeBase={onOpenKnowledgeBase}
                    />
                  </div>
                ) : null}
                <Button
                  variant="icon"
                  size="compact"
                  className="pointer-events-none absolute right-1 bottom-1 text-ink-soft opacity-0 group-hover:pointer-events-auto group-hover:opacity-100 focus-visible:pointer-events-auto focus-visible:opacity-100 hover:enabled:bg-danger-soft hover:enabled:text-danger"
                  title="删除会话"
                  aria-label={`删除${session.title}`}
                  onClick={(event) => {
                    event.stopPropagation();
                    onDeleteSession(session.id);
                  }}
                >
                  <Trash2 size={14} />
                </Button>
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}

/** 当前库筛选时仍露出正在看的会话，以及别的库上还在跑或排队的会话。 */
function sessionBelongsToKnowledgeBase(
  session: AgentSession,
  knowledgeBaseId: string,
  isActive: boolean,
  runningIds: Set<string>,
  queuedIds: Set<string>,
) {
  return (
    isActive ||
    runningIds.has(session.id) ||
    queuedIds.has(session.id) ||
    session.knowledgeBaseIds.includes(knowledgeBaseId)
  );
}

/** 按会话记录的顺序取出仍然存在的知识库，供多库标签点击。 */
function getBoundKnowledgeBases(session: AgentSession, knowledgeBases: KnowledgeBase[]) {
  return session.knowledgeBaseIds.flatMap((knowledgeBaseId) => {
    const knowledgeBase = knowledgeBases.find((item) => item.id === knowledgeBaseId);
    return knowledgeBase ? [knowledgeBase] : [];
  });
}

/** 第二行右侧的状态和时间。悬停整行时让出位置给删除按钮。 */
function SessionMetaTrailing({
  queued,
  pendingLabel,
  updatedAt,
}: {
  queued: boolean;
  pendingLabel: string;
  updatedAt: string;
}) {
  return (
    <>
      {queued ? <span className="shrink-0 rounded-full border border-border px-1.5 text-[10px] leading-4">排队</span> : null}
      {pendingLabel ? (
        <span className="shrink-0 rounded-full border border-[rgba(var(--danger-rgb),0.26)] bg-danger-soft px-1.5 text-[10px] leading-4 text-danger">
          {pendingLabel}
        </span>
      ) : null}
      <time
        className="ml-auto shrink-0 tabular-nums text-ink-soft group-hover:invisible"
        dateTime={updatedAt}
      >
        {formatSessionListTime(updatedAt)}
      </time>
    </>
  );
}

/** 多个知识库时用可点击标签代替挤在一行里的名称。 */
function SessionKnowledgeBaseChips({
  sessionId,
  knowledgeBases,
  activeKnowledgeBaseId,
  scheduled,
  onOpenKnowledgeBase,
}: {
  sessionId: string;
  knowledgeBases: KnowledgeBase[];
  activeKnowledgeBaseId?: string;
  scheduled: boolean;
  onOpenKnowledgeBase?: (sessionId: string, knowledgeBaseId: string) => void;
}) {
  return (
    <span className="flex min-w-0 flex-1 items-center gap-1 overflow-hidden">
      {scheduled ? <span className="shrink-0">定时</span> : null}
      {knowledgeBases.map((knowledgeBase) => {
        const isCurrent = knowledgeBase.id === activeKnowledgeBaseId;
        const className = cn(
          "max-w-[5.5rem] truncate rounded-full border px-1.5 py-px",
          isCurrent ? "border-accent/40 bg-accent-soft text-accent" : "border-border text-ink-muted",
        );

        if (!onOpenKnowledgeBase) {
          return (
            <span className={className} key={knowledgeBase.id} title={knowledgeBase.name}>
              {knowledgeBase.name}
            </span>
          );
        }

        return (
          <button
            key={knowledgeBase.id}
            className={cn(className, "cursor-pointer hover:border-accent/50")}
            type="button"
            title={`在「${knowledgeBase.name}」中查看`}
            onClick={(event) => {
              event.stopPropagation();
              onOpenKnowledgeBase(sessionId, knowledgeBase.id);
            }}
          >
            {knowledgeBase.name}
          </button>
        );
      })}
    </span>
  );
}

/** 第二行优先展示 IM 来源和最近消息，否则展示知识库范围。 */
function getSessionSubtitle(session: AgentSession, knowledgeBases: KnowledgeBase[]) {
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

/** 只在有待确认写入时给出短标签，避免每条会话都占第三行。 */
function getPendingSessionLabel(session: AgentSession, sessions: AgentSession[]) {
  const conflict = sessionHasPendingWriteConflict(session, sessions);
  if (session.pendingChange?.status === "pending") {
    return conflict ? "与其它会话改同一文件" : "待确认 diff";
  }

  if (session.pendingChangeSet?.status === "pending") {
    return conflict ? "与其它会话改同一文件" : "待确认变更集";
  }

  return "";
}

/** 搜索标题、知识库、IM 来源和最近消息，不改会话数据。 */
function sessionMatchesSearch(session: AgentSession, knowledgeBases: KnowledgeBase[], term: string) {
  const haystack = [
    session.title,
    getSessionSubtitle(session, knowledgeBases),
    session.scheduleIdentity?.jobName ?? "",
  ]
    .join(" ")
    .toLowerCase();

  return haystack.includes(term);
}

/**
 * 把 `2026/10/02 19:30` 或 `今天 14:18` 收成侧栏能放下的时间。
 * 今天只留时分，其它日期留月日和时分。
 */
function formatSessionListTime(value: string) {
  const trimmed = value.trim();
  const todayClock = trimmed.match(/^今天\s+(\d{1,2}:\d{2})$/);
  if (todayClock) {
    return todayClock[1];
  }

  const match = trimmed.match(/^(\d{4})[/.年-](\d{1,2})[/.月-](\d{1,2})日?(?:\s+(\d{1,2}:\d{2}))?/);
  if (!match) {
    return trimmed;
  }

  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  const clock = match[4] ?? "";
  const now = new Date();
  const isToday = year === now.getFullYear() && month === now.getMonth() + 1 && day === now.getDate();
  if (isToday) {
    return clock || "今天";
  }

  const monthText = String(month).padStart(2, "0");
  const dayText = String(day).padStart(2, "0");
  return clock ? `${monthText}/${dayText} ${clock}` : `${monthText}/${dayText}`;
}
