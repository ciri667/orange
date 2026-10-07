import { Archive, ListFilter, Loader2, MessageSquareText, MoreHorizontal, Pin, Search, Trash2 } from "lucide-react";
import { useRef, useState } from "react";
import { Button } from "../shared/Button";
import { listRowClassName } from "../shared/ListRow";
import { Menu, MenuItem, MenuPanel } from "../shared/Menu";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import { cn } from "../shared/cn";
import { focusShellClassName, sectionLabelClassName } from "../shared/ui";
import { sessionHasPendingWriteConflict } from "../workspace/sessionUtils";
import type { AgentSession, KnowledgeBase } from "../shared/types";
import {
  SESSION_LIST_FACETS,
  groupSessionsForList,
  isSessionVisible,
  sessionSearchSnippet,
  sessionSubtitle,
  type SessionListFacet,
} from "./sessionListModel";

/** 侧栏和窄屏浮层共用的紧凑会话列表。 */
export function SessionList({
  sessions,
  activeSessionId,
  knowledgeBases,
  inFlightSessionIds = [],
  queuedSessionIds = [],
  activeKnowledgeBaseId,
  onSelectSession,
  onDeleteSession,
  onRenameSession,
  onTogglePinSession,
  onToggleArchiveSession,
  onOpenKnowledgeBase,
}: {
  sessions: AgentSession[];
  activeSessionId: string;
  knowledgeBases: KnowledgeBase[];
  inFlightSessionIds?: string[];
  queuedSessionIds?: string[];
  /** 正在浏览的知识库，用来筛选「含当前知识库」并标出标签。 */
  activeKnowledgeBaseId?: string;
  onSelectSession: (sessionId: string) => void;
  onDeleteSession: (sessionId: string) => void;
  onRenameSession: (sessionId: string, title: string) => void;
  onTogglePinSession: (sessionId: string) => void;
  onToggleArchiveSession: (sessionId: string) => void;
  /** 点击会话上的知识库标签时，切到该会话并浏览这个库。 */
  onOpenKnowledgeBase?: (sessionId: string, knowledgeBaseId: string) => void;
}) {
  const [searchTerm, setSearchTerm] = useState("");
  const [facet, setFacet] = useState<SessionListFacet>("all");
  const [facetMenuOpen, setFacetMenuOpen] = useState(false);
  const [menuSessionId, setMenuSessionId] = useState<string | null>(null);
  const [renamingSessionId, setRenamingSessionId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const skipRenameCommitRef = useRef(false);
  const runningIds = new Set(inFlightSessionIds);
  const queuedIds = new Set(queuedSessionIds);
  const normalizedSearch = searchTerm.trim().toLowerCase();
  const visibleSessions = sessions.filter((session) =>
    isSessionVisible(session, {
      knowledgeBases,
      term: normalizedSearch,
      facet,
      activeKnowledgeBaseId,
      isActive: session.id === activeSessionId,
      isRunning: runningIds.has(session.id),
      isQueued: queuedIds.has(session.id),
    }),
  );
  const groups = normalizedSearch ? null : groupSessionsForList(visibleSessions, new Date());
  const facetLabel = SESSION_LIST_FACETS.find((item) => item.id === facet)?.label ?? "全部";

  /** 开始改标题。空白草稿不提交，Escape 放弃。 */
  function beginRename(session: AgentSession) {
    skipRenameCommitRef.current = false;
    setMenuSessionId(null);
    setRenamingSessionId(session.id);
    setRenameDraft(session.title);
  }

  function commitRename(sessionId: string) {
    if (skipRenameCommitRef.current) {
      skipRenameCommitRef.current = false;
      setRenamingSessionId(null);
      return;
    }

    const nextTitle = renameDraft.trim();
    setRenamingSessionId(null);
    if (!nextTitle || nextTitle === sessions.find((session) => session.id === sessionId)?.title) {
      return;
    }
    onRenameSession(sessionId, nextTitle);
  }

  function cancelRename() {
    skipRenameCommitRef.current = true;
    setRenamingSessionId(null);
  }

  return (
    <div className="flex h-full min-h-0 flex-1 flex-col">
      {/* 侧栏和浮层都是 overflow-hidden，外扩的焦点描边需要自己的空隙，否则上边和左右会被裁掉。 */}
      <div className="flex shrink-0 items-center gap-1 px-1.5 pt-1.5 pb-1.5">
        <label className={cn("flex min-h-8 min-w-0 flex-1 items-center gap-2 rounded-control bg-white/70 px-2 text-ink-muted", focusShellClassName)}>
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
        <Menu open={facetMenuOpen} onClose={() => setFacetMenuOpen(false)}>
          <Button
            variant="icon"
            size="compact"
            title={facet === "all" ? "筛选会话" : `筛选：${facetLabel}`}
            aria-label="筛选会话"
            aria-haspopup="menu"
            aria-expanded={facetMenuOpen}
            className={cn(facet !== "all" && "bg-accent-soft text-accent")}
            onClick={() => setFacetMenuOpen((open) => !open)}
          >
            <ListFilter size={14} />
          </Button>
          {facetMenuOpen && (
            <MenuPanel placement="bottom-end" className="min-w-[148px]">
              {SESSION_LIST_FACETS.map((item) => (
                <MenuItem
                  key={item.id}
                  className={cn(item.id === facet && "bg-surface-muted text-ink-strong")}
                  onClick={() => {
                    setFacet(item.id);
                    setFacetMenuOpen(false);
                  }}
                >
                  {item.label}
                </MenuItem>
              ))}
            </MenuPanel>
          )}
        </Menu>
      </div>

      <div className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-auto pr-0.5" aria-label="会话列表">
        {visibleSessions.length === 0 ? (
          <p className="px-2.5 py-3 text-xs text-ink-muted">
            {sessions.length === 0 ? "还没有会话" : normalizedSearch || facet !== "all" ? "没有匹配的会话" : "没有可显示的会话"}
          </p>
        ) : groups ? (
          groups.map((group) => (
            <section key={group.id} className="grid gap-0.5" aria-label={group.label}>
              <h3 className={cn(sectionLabelClassName, "pt-1.5")}>{group.label}</h3>
              {group.sessions.map((session) => (
                <SessionRow
                  key={session.id}
                  session={session}
                  snippet={null}
                  active={session.id === activeSessionId}
                  running={runningIds.has(session.id)}
                  queued={queuedIds.has(session.id) && !runningIds.has(session.id)}
                  pendingLabel={getPendingSessionLabel(session, sessions)}
                  knowledgeBases={knowledgeBases}
                  activeKnowledgeBaseId={activeKnowledgeBaseId}
                  menuOpen={menuSessionId === session.id}
                  renaming={renamingSessionId === session.id}
                  renameDraft={renameDraft}
                  onRenameDraftChange={setRenameDraft}
                  onBeginRename={() => beginRename(session)}
                  onCommitRename={() => commitRename(session.id)}
                  onCancelRename={cancelRename}
                  onToggleMenu={() => setMenuSessionId((current) => (current === session.id ? null : session.id))}
                  onCloseMenu={() => setMenuSessionId(null)}
                  onSelectSession={onSelectSession}
                  onDeleteSession={onDeleteSession}
                  onTogglePinSession={onTogglePinSession}
                  onToggleArchiveSession={onToggleArchiveSession}
                  onOpenKnowledgeBase={onOpenKnowledgeBase}
                />
              ))}
            </section>
          ))
        ) : (
          visibleSessions.map((session) => (
            <SessionRow
              key={session.id}
              session={session}
              snippet={sessionSearchSnippet(session, normalizedSearch)}
              active={session.id === activeSessionId}
              running={runningIds.has(session.id)}
              queued={queuedIds.has(session.id) && !runningIds.has(session.id)}
              pendingLabel={getPendingSessionLabel(session, sessions)}
              knowledgeBases={knowledgeBases}
              activeKnowledgeBaseId={activeKnowledgeBaseId}
              menuOpen={menuSessionId === session.id}
              renaming={renamingSessionId === session.id}
              renameDraft={renameDraft}
              onRenameDraftChange={setRenameDraft}
              onBeginRename={() => beginRename(session)}
              onCommitRename={() => commitRename(session.id)}
              onCancelRename={() => setRenamingSessionId(null)}
              onToggleMenu={() => setMenuSessionId((current) => (current === session.id ? null : session.id))}
              onCloseMenu={() => setMenuSessionId(null)}
              onSelectSession={onSelectSession}
              onDeleteSession={onDeleteSession}
              onTogglePinSession={onTogglePinSession}
              onToggleArchiveSession={onToggleArchiveSession}
              onOpenKnowledgeBase={onOpenKnowledgeBase}
            />
          ))
        )}
      </div>
    </div>
  );
}

function SessionRow({
  session,
  snippet,
  active,
  running,
  queued,
  pendingLabel,
  knowledgeBases,
  activeKnowledgeBaseId,
  menuOpen,
  renaming,
  renameDraft,
  onRenameDraftChange,
  onBeginRename,
  onCommitRename,
  onCancelRename,
  onToggleMenu,
  onCloseMenu,
  onSelectSession,
  onDeleteSession,
  onTogglePinSession,
  onToggleArchiveSession,
  onOpenKnowledgeBase,
}: {
  session: AgentSession;
  snippet: string | null;
  active: boolean;
  running: boolean;
  queued: boolean;
  pendingLabel: string;
  knowledgeBases: KnowledgeBase[];
  activeKnowledgeBaseId?: string;
  menuOpen: boolean;
  renaming: boolean;
  renameDraft: string;
  onRenameDraftChange: (value: string) => void;
  onBeginRename: () => void;
  onCommitRename: () => void;
  onCancelRename: () => void;
  onToggleMenu: () => void;
  onCloseMenu: () => void;
  onSelectSession: (sessionId: string) => void;
  onDeleteSession: (sessionId: string) => void;
  onTogglePinSession: (sessionId: string) => void;
  onToggleArchiveSession: (sessionId: string) => void;
  onOpenKnowledgeBase?: (sessionId: string, knowledgeBaseId: string) => void;
}) {
  const boundKnowledgeBases = getBoundKnowledgeBases(session, knowledgeBases);
  const subtitle = snippet ?? sessionSubtitle(session, knowledgeBases);
  const showKnowledgeBaseChips = !snippet && !getImLabel(session) && boundKnowledgeBases.length > 1;
  const interactiveChips = showKnowledgeBaseChips && Boolean(onOpenKnowledgeBase);

  return (
    <div
      className={listRowClassName({
        active,
        className: "group relative cursor-pointer items-start py-1.5 pr-1.5 pl-2",
      })}
      onClick={() => onSelectSession(session.id)}
    >
      <button
        className="flex min-w-0 flex-1 items-start gap-2 border-0 bg-transparent p-0 text-left text-inherit"
        type="button"
        onClick={(event) => {
          event.stopPropagation();
          if (!renaming) {
            onSelectSession(session.id);
          }
        }}
      >
        {running ? (
          <Loader2 size={15} className="mt-0.5 shrink-0 animate-spin text-accent" aria-label="运行中" />
        ) : (
          <MessageSquareText size={15} className={cn("mt-0.5 shrink-0", active ? "text-ink-muted" : "text-ink-soft")} />
        )}
        <span className="grid min-w-0 flex-1 gap-0.5">
          <span className="flex min-w-0 items-center gap-1 pr-7">
            {session.pinnedAt ? <Pin size={12} className="shrink-0 text-accent" aria-label="已置顶" /> : null}
            {renaming ? (
              <input
                className="min-w-0 flex-1 border-0 bg-transparent text-[13px] font-medium text-ink-strong outline-0"
                value={renameDraft}
                aria-label="会话标题"
                autoFocus
                onChange={(event) => onRenameDraftChange(event.target.value)}
                onClick={(event) => event.stopPropagation()}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    onCommitRename();
                  }
                  if (event.key === "Escape") {
                    event.preventDefault();
                    onCancelRename();
                  }
                }}
                onBlur={onCommitRename}
              />
            ) : (
              <OverflowTooltipText
                as="strong"
                className="block min-w-0 truncate text-[13px] font-medium leading-5 text-ink-strong"
                text={session.title}
                logArea="agent_session_history_title"
              />
            )}
            {session.archivedAt ? <span className="shrink-0 text-[10px] text-ink-soft">归档</span> : null}
          </span>
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
              <OverflowTooltipText className="min-w-0 flex-1 truncate" text={subtitle} logArea="agent_session_history_scope" />
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
      <div
        className={cn(
          "pointer-events-none absolute right-1 bottom-1 z-[2] flex items-center group-hover:pointer-events-auto",
          menuOpen && "pointer-events-auto",
        )}
        onClick={(event) => event.stopPropagation()}
      >
        <Menu open={menuOpen} onClose={onCloseMenu}>
          <Button
            variant="icon"
            size="compact"
            className={cn(
              "pointer-events-none text-ink-soft opacity-0 group-hover:pointer-events-auto group-hover:opacity-100 focus-visible:pointer-events-auto focus-visible:opacity-100",
              menuOpen && "pointer-events-auto opacity-100",
            )}
            title="会话操作"
            aria-label={`${session.title}的更多操作`}
            aria-haspopup="menu"
            aria-expanded={menuOpen}
            onClick={onToggleMenu}
          >
            <MoreHorizontal size={14} />
          </Button>
          {menuOpen && (
            <MenuPanel placement="bottom-end" className="min-w-[132px]">
              <MenuItem onClick={onBeginRename}>重命名</MenuItem>
              <MenuItem
                onClick={() => {
                  onCloseMenu();
                  onTogglePinSession(session.id);
                }}
              >
                <Pin size={14} />
                {session.pinnedAt ? "取消置顶" : "置顶"}
              </MenuItem>
              <MenuItem
                onClick={() => {
                  onCloseMenu();
                  onToggleArchiveSession(session.id);
                }}
              >
                <Archive size={14} />
                {session.archivedAt ? "取消归档" : "归档"}
              </MenuItem>
            </MenuPanel>
          )}
        </Menu>
        <Button
          variant="icon"
          size="compact"
          className="pointer-events-none text-ink-soft opacity-0 group-hover:pointer-events-auto group-hover:opacity-100 focus-visible:pointer-events-auto focus-visible:opacity-100 hover:enabled:bg-danger-soft hover:enabled:text-danger"
          title="删除会话"
          aria-label={`删除${session.title}`}
          onClick={() => onDeleteSession(session.id)}
        >
          <Trash2 size={14} />
        </Button>
      </div>
    </div>
  );
}

function getImLabel(session: AgentSession) {
  return Boolean(session.imIdentity);
}

/** 按会话记录的顺序取出仍然存在的知识库，供多库标签点击。 */
function getBoundKnowledgeBases(session: AgentSession, knowledgeBases: KnowledgeBase[]) {
  return session.knowledgeBaseIds.flatMap((knowledgeBaseId) => {
    const knowledgeBase = knowledgeBases.find((item) => item.id === knowledgeBaseId);
    return knowledgeBase ? [knowledgeBase] : [];
  });
}

/** 第二行右侧的状态和时间。悬停整行时让出位置给操作按钮。 */
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
      <time className="ml-auto shrink-0 tabular-nums text-ink-soft group-hover:invisible" dateTime={updatedAt}>
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
