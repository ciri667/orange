import { Check, Database, FileText, Layers3, MessageSquareText, ShieldAlert, Sparkles, Trash2, X } from "lucide-react";
import { useEffect, useRef } from "react";
import ReactMarkdown from "react-markdown";
import rehypeSanitize from "rehype-sanitize";
import remarkGfm from "remark-gfm";
import { Button } from "../shared/Button";
import { MarkdownLink } from "../shared/MarkdownLink";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import {
  getScopeSummaryLabel,
  getImSessionRecentMessageLabel,
  getImSessionSourceLabel,
  getSessionKnowledgeBaseLabel,
  getSessionRecoveryNoteLabel,
  getSessionTypeLabel,
} from "../shared/selectors";
import {
  encodeModelSelection,
  FOLLOW_DEFAULT_MODEL_SELECTION,
  getProviderModelSelectionLabel,
} from "../shared/modelSelection";
import { ModelCascadeSelector } from "../shared/ModelCascadeSelector";
import type {
  AgentMessage,
  AgentSecuritySettings,
  AgentSession,
  AgentTurnProgressEvent,
  KnowledgeBase,
  ModelConfig,
  Note,
  WorkspaceDocument,
} from "../shared/types";
import { CitationList } from "./CitationList";
import { AgentTurnTrace } from "./AgentTurnTrace";
import { ToolCallList } from "./ToolCallList";

/** 会话摘要条只保留当前文件和待确认写入，避免和输入条、范围入口重复。 */
export function AgentSessionSummary({
  activeSession,
  currentFileLabel,
}: {
  activeSession: AgentSession;
  /** 工作台当前焦点文件；它是本轮默认编辑目标，独立于会话恢复锚点。 */
  currentFileLabel: string;
}) {
  const isPendingWrite = activeSession.pendingChange?.status === "pending";

  return (
    <div className="session-summary" aria-label="当前会话摘要">
      <span className="agent-file-chip">
        <FileText size={13} />
        <OverflowTooltipText text={currentFileLabel} logArea="agent_session_current_file_summary" />
      </span>
      {isPendingWrite && (
        <OverflowTooltipText
          className="session-write-status pending"
          text="待确认 diff"
          logArea="agent_session_write_status"
        />
      )}
    </div>
  );
}

/** 三级权限是用户愿意把多少执行权交给 Agent。 */
export const AGENT_SECURITY_LEVEL_COPY = {
  basic: {
    label: "基础",
    description: "Agent只在选中的知识库文档里工作。",
  },
  advanced: {
    label: "进阶",
    description: "Agent可以做更多事，仍需手动确认。",
  },
  autonomous: {
    label: "完全",
    description: "可不受限制地访问你的电脑上任何文件。",
  },
} as const;

/** 钉在输入条上的三级权限开关；IM 会话不渲染。 */
export function AgentSecurityLevelControl({
  activeSession,
  agentSecurity,
  isBusy,
  onSecurityLevelChange,
}: {
  activeSession: AgentSession;
  agentSecurity?: AgentSecuritySettings;
  isBusy: boolean;
  onSecurityLevelChange?: (level: AgentSession["securityLevel"]) => void;
}) {
  if (!agentSecurity) {
    return null;
  }

  return (
    <div className="agent-security-level-control" aria-label="当前会话权限">
      <span className="agent-security-level-label">
        <ShieldAlert size={13} />
      </span>
      <div className="agent-security-level-options" role="radiogroup" aria-label="当前会话权限级别">
        {([
          ["basic", true],
          ["advanced", agentSecurity.advancedExecutionEnabled],
          ["autonomous", agentSecurity.autonomousModeEnabled],
        ] as const).map(([level, isEnabled]) => {
          const copy = AGENT_SECURITY_LEVEL_COPY[level];

          return (
            <button
              className={activeSession.securityLevel === level ? "active" : ""}
              type="button"
              role="radio"
              aria-checked={activeSession.securityLevel === level}
              aria-label={`${copy.label}权限。${copy.description}`}
              title={`${copy.description}${isEnabled ? "" : "选择后将启用此能力。"}`}
              disabled={isBusy}
              key={level}
              onClick={() => onSecurityLevelChange?.(level)}
            >
              {copy.label}
            </button>
          );
        })}
      </div>
    </div>
  );
}

/** 会话历史浮层，展示可恢复会话并提供删除入口。 */
export function AgentSessionHistoryPopover({
  sessions,
  activeSession,
  knowledgeBases,
  onToggleSessionList,
  onSelectSession,
  onDeleteSession,
}: {
  sessions: AgentSession[];
  activeSession: AgentSession;
  knowledgeBases: KnowledgeBase[];
  onToggleSessionList: () => void;
  onSelectSession: (sessionId: string) => void;
  onDeleteSession: (sessionId: string) => void;
}) {
  return (
    <section className="session-popover" aria-label="会话历史">
      <div className="popover-header">
        <div>
          <p className="section-label">Sessions</p>
          <h3>会话历史</h3>
        </div>
        <Button variant="icon" title="关闭会话历史" onClick={onToggleSessionList}>
          <X size={15} />
        </Button>
      </div>
      <div className="session-list">
        {sessions.map((session) => (
          <div className={`session-row ${session.id === activeSession.id ? "active" : ""}`} key={session.id}>
            <button className="session-row-main" type="button" onClick={() => onSelectSession(session.id)}>
              <span className="session-row-title">
                <MessageSquareText size={14} />
                <OverflowTooltipText as="strong" text={session.title} logArea="agent_session_history_title" />
              </span>
              <span className="session-row-meta">
                {session.imIdentity ? (
                  <span className="im-session-badge">{getImSessionSourceLabel(session)}</span>
                ) : (
                  <OverflowTooltipText text={getSessionTypeLabel(session.type)} logArea="agent_session_history_type" />
                )}
                <OverflowTooltipText text={getSessionKnowledgeBaseLabel(session, knowledgeBases)} logArea="agent_session_history_scope" />
                {getImSessionRecentMessageLabel(session) && (
                  <OverflowTooltipText
                    className="session-row-recent-message"
                    text={getImSessionRecentMessageLabel(session)}
                    logArea="agent_session_history_recent_message"
                  />
                )}
                <OverflowTooltipText
                  as="time"
                  dateTime={session.createdAt}
                  text={`创建：${session.createdAt}`}
                  logArea="agent_session_history_created_at"
                />
                <OverflowTooltipText
                  as="time"
                  dateTime={session.updatedAt}
                  text={`最近：${session.updatedAt}`}
                  logArea="agent_session_history_updated_at"
                />
              </span>
              {session.pendingChange?.status === "pending" && <span className="session-pending">待确认 diff</span>}
            </button>
            <Button
              variant="icon"
              tone="danger"
              className="session-delete-button"
              title="删除会话"
              onClick={() => onDeleteSession(session.id)}
            >
              <Trash2 size={14} />
            </Button>
          </div>
        ))}
      </div>
    </section>
  );
}

/** 会话上下文浮层，集中展示工具范围、工作台文件、恢复锚点和会话默认模型。 */
export function AgentSessionContextPopover({
  activeSession,
  knowledgeBases,
  notes,
  currentFileLabel,
  modelConfig,
  isBusy,
  onToggleSessionContext,
  onSetSessionModelSelection,
  onCompactAgentContext,
}: {
  activeSession: AgentSession;
  knowledgeBases: KnowledgeBase[];
  notes: Note[];
  /** 工作台当前焦点文件；本轮 Agent 默认以它作为编辑目标。 */
  currentFileLabel: string;
  modelConfig: ModelConfig;
  isBusy: boolean;
  onToggleSessionContext: () => void;
  onSetSessionModelSelection: (selection: string) => void;
  onCompactAgentContext: () => void;
}) {
  /** 已启用的 Provider 列表；未启用的 provider 不出现在选择器中。 */
  const enabledProviders = modelConfig.providers.filter((provider) => provider.enabled);
  /** 全局默认 provider 名称，用于“跟随默认”选项的说明文案。 */
  const defaultProvider = modelConfig.providers.find((provider) => provider.id === modelConfig.defaultProviderId);
  /** 旧会话可能只保存了 providerId；此时用该 provider 的默认模型补齐选择值。 */
  const sessionProvider = activeSession.modelProviderId
    ? modelConfig.providers.find((provider) => provider.id === activeSession.modelProviderId)
    : undefined;
  /** 会话默认模型的 select value；空字符串表示跟随全局默认。 */
  const sessionModelSelection = sessionProvider
    ? encodeModelSelection(sessionProvider.id, activeSession.modelId || sessionProvider.model)
    : FOLLOW_DEFAULT_MODEL_SELECTION;
  /** 当前会话的写入状态，和摘要条使用同一语义。 */
  const writeStatus = activeSession.pendingChange?.status === "pending" ? "待确认 diff" : "写入需确认";
  /** 工作记忆状态只展示字段数和更新时间，不暴露 summary 正文。 */
  const memoryStatus = activeSession.contextSummary
    ? `${getContextSummaryFieldCount(activeSession)} 项 · ${activeSession.contextSummary.updatedAt || "刚刚"}`
    : "未整理";

  return (
    <section className="context-popover" aria-label="会话上下文">
      <div className="popover-header">
        <div>
          <p className="section-label">Context</p>
          <h3>上下文</h3>
        </div>
        <div className="popover-header-actions">
          <Button variant="icon" title="整理上下文" onClick={onCompactAgentContext} disabled={isBusy}>
            <Sparkles size={15} />
          </Button>
          <Button variant="icon" title="关闭上下文" onClick={onToggleSessionContext}>
            <X size={15} />
          </Button>
        </div>
      </div>
      <div className="context-popover-body">
        <div className="context-matrix">
          <div>
            <span>工具检索范围</span>
            <OverflowTooltipText
              as="strong"
              text={getSessionKnowledgeBaseLabel(activeSession, knowledgeBases)}
              logArea="agent_context_scope"
            />
          </div>
          <div>
            <span>当前文件</span>
            <OverflowTooltipText as="strong" text={currentFileLabel} logArea="agent_context_current_file" />
          </div>
          <div>
            <span>会话恢复笔记</span>
            <OverflowTooltipText
              as="strong"
              text={getSessionRecoveryNoteLabel(activeSession, notes)}
              logArea="agent_context_session_recovery_note"
            />
          </div>
          <div>
            <span>消息</span>
            <strong>{activeSession.messages.length} 条</strong>
          </div>
          <div>
            <span>写入</span>
            <strong>{writeStatus}</strong>
          </div>
          <div>
            <span>工作记忆</span>
            <strong>{memoryStatus}</strong>
          </div>
        </div>
        {modelConfig.enabled && (
          <label className="context-model-select">
            <span>会话默认模型</span>
            <ModelCascadeSelector
              value={sessionModelSelection}
              providers={enabledProviders}
              defaultLabel={`跟随全局默认${defaultProvider ? `（${getProviderModelSelectionLabel(defaultProvider)}）` : ""}`}
              ariaLabel="会话默认模型"
              onChange={onSetSessionModelSelection}
              variant="block"
              logArea="agent_session_model_cascade"
            />
          </label>
        )}
        <p className="context-note">
          当前文件是本轮默认编辑目标；会话恢复笔记只用于恢复旧会话位置。Agent 会按模型窗口装入尽量多的最近对话，更早内容进入工作记忆，也可按需检索会话历史和其他文件。
        </p>
      </div>
    </section>
  );
}

/** 统计工作记忆里有内容的字段数量，UI 不展示任何 summary 正文。 */
function getContextSummaryFieldCount(session: AgentSession) {
  const summary = session.contextSummary;

  if (!summary) {
    return 0;
  }

  // 后端数组字段使用 skip_serializing_if="Vec::is_empty"，空数组在 JSON 中会被省略，
  // 因此这里对每个数组字段做空值保护，避免 undefined 时崩溃。
  return [
    summary.currentGoal,
    summary.userConstraints?.length,
    summary.decisions?.length,
    summary.completedWork?.length,
    summary.pendingTasks?.length,
    summary.touchedNotes?.length,
    summary.pendingChangeSummary,
    summary.openQuestions?.length,
    summary.lastSummarizedMessageId,
    summary.lastCompactedMessageId,
  ].filter(Boolean).length;
}

/** 工具范围选择器，当前激活知识库始终保持选中。 */
export function AgentScopeSelector({
  activeSession,
  activeKnowledgeBase,
  knowledgeBases,
  isScopeSelectorOpen,
  onToggleScopeSelector,
  onToggleScopeKnowledgeBase,
}: {
  activeSession: AgentSession;
  activeKnowledgeBase: KnowledgeBase;
  knowledgeBases: KnowledgeBase[];
  isScopeSelectorOpen: boolean;
  onToggleScopeSelector: () => void;
  onToggleScopeKnowledgeBase: (knowledgeBaseId: string) => void;
}) {
  /** 当前会话选中的知识库 ID，用于驱动范围摘要和多选列表。 */
  const selectedKnowledgeBaseIds = activeSession.knowledgeBaseIds.length
    ? activeSession.knowledgeBaseIds
    : [activeKnowledgeBase.id];
  /** 当前会话的知识库集合，当前激活知识库不能被移除。 */
  const selectedKnowledgeBaseSet = new Set(selectedKnowledgeBaseIds);
  /** 当前会话范围摘要，展示 Agent 可调用检索工具的权限边界。 */
  const selectedScopeLabel = getScopeSummaryLabel(activeSession, knowledgeBases);

  return (
    <>
      <button
        className={`scope-selector ${selectedKnowledgeBaseIds.length > 1 ? "active" : ""}`}
        type="button"
        title="编辑工具范围"
        aria-label="工具范围"
        aria-expanded={isScopeSelectorOpen}
        onClick={onToggleScopeSelector}
      >
        <Layers3 size={14} />
        <OverflowTooltipText text={selectedScopeLabel} logArea="agent_scope_selector_summary" />
      </button>

      {isScopeSelectorOpen && (
        <section className="scope-popover" aria-label="选择检索知识库">
          <div className="popover-header">
            <div>
              <p className="section-label">Scope</p>
              <h3>选择工具可访问知识库</h3>
            </div>
            <div className="popover-header-actions">
              <span>
                {selectedKnowledgeBaseIds.length} / {knowledgeBases.length}
              </span>
              <Button variant="icon" title="关闭工具范围" onClick={onToggleScopeSelector}>
                <X size={15} />
              </Button>
            </div>
          </div>
          <div className="scope-option-list">
            {knowledgeBases.map((knowledgeBase) => {
              const isActiveKnowledgeBase = knowledgeBase.id === activeKnowledgeBase.id;
              const isSelected = selectedKnowledgeBaseSet.has(knowledgeBase.id) || isActiveKnowledgeBase;

              return (
                <label className={`scope-option ${isSelected ? "selected" : ""}`} key={knowledgeBase.id}>
                  <input
                    className="control-checkbox-input"
                    checked={isSelected}
                    disabled={isActiveKnowledgeBase}
                    onChange={() => onToggleScopeKnowledgeBase(knowledgeBase.id)}
                    type="checkbox"
                  />
                  <span className="scope-check">{isSelected && <Check size={12} />}</span>
                  <Database size={15} />
                  <span className="scope-option-copy">
                    <OverflowTooltipText as="strong" text={knowledgeBase.name} logArea="agent_scope_option_name" />
                    <OverflowTooltipText
                      text={isActiveKnowledgeBase ? "当前激活，默认选中" : knowledgeBase.path}
                      logArea="agent_scope_option_detail"
                    />
                  </span>
                </label>
              );
            })}
          </div>
        </section>
      )}
    </>
  );
}

/** Agent 消息列表，安全渲染 Markdown、过程轨迹和知识库引用。 */
export function AgentMessageList({
  activeSession,
  notes,
  documents,
  liveTurn,
}: {
  activeSession: AgentSession;
  notes: Note[];
  documents: WorkspaceDocument[];
  liveTurn?: AgentTurnProgressEvent | null;
}) {
  const persistedIds = new Set(activeSession.messages.map((message) => message.id));
  const showLiveTurn =
    Boolean(liveTurn) && liveTurn?.sessionId === activeSession.id && !persistedIds.has(liveTurn.liveMessageId);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    // 过程增量到达时跟到底部，避免执行中展开的步骤被旧消息挡住。
    listRef.current?.scrollTo({ top: listRef.current.scrollHeight });
  }, [activeSession.messages.length, liveTurn?.steps.length, liveTurn?.content, liveTurn?.status]);

  return (
    <div className="message-list" aria-live="polite" ref={listRef}>
      {activeSession.messages.length === 0 && !showLiveTurn && (
        <div className="message-list-empty">
          <Sparkles size={16} />
          <p>从下面开始提问</p>
          <span>@ 引用当前库里的文件，/ 选择本轮 Skill</span>
        </div>
      )}
      {activeSession.messages.map((message) => (
        <AgentMessageItem documents={documents} key={message.id} message={message} notes={notes} />
      ))}
      {showLiveTurn && liveTurn && (
        <AgentMessageItem
          documents={documents}
          liveStatus={liveTurn.status}
          message={{
            id: liveTurn.liveMessageId,
            role: "assistant",
            content: liveTurn.content ?? "",
            trace: liveTurn.steps,
            turnDurationMs: liveTurn.turnDurationMs,
          }}
          notes={notes}
        />
      )}
    </div>
  );
}

/** 单条会话消息：过程区在最终回答之前，旧消息没有 trace 时回退到扁平运行信息。 */
function AgentMessageItem({
  message,
  notes,
  documents,
  liveStatus,
}: {
  message: AgentMessage;
  notes: Note[];
  documents: WorkspaceDocument[];
  liveStatus?: AgentTurnProgressEvent["status"];
}) {
  const usesTurnTrace =
    liveStatus != null || message.turnDurationMs != null || Boolean(message.trace?.length);
  const showTurnTrace =
    message.role === "assistant" &&
    usesTurnTrace &&
    (Boolean(message.trace?.length) || liveStatus === "running" || liveStatus === "failed");

  return (
    <article className={`message ${message.role}`}>
      <div className="message-role">
        {message.role === "assistant" ? <Sparkles size={14} /> : <MessageSquareText size={14} />}
        <span>{message.role === "assistant" ? "橘记 Agent" : "你"}</span>
      </div>
      {message.mentionedFileIds?.length ? (
        <div className="message-mentioned-files" aria-label="本轮 @ 文件">
          {message.mentionedFileIds.map((fileId) => (
            <span key={fileId}>{getMentionedFileLabel(fileId, notes, documents)}</span>
          ))}
        </div>
      ) : null}
      {showTurnTrace ? (
        <AgentTurnTrace
          durationMs={message.turnDurationMs}
          status={liveStatus ?? "completed"}
          steps={message.trace ?? []}
        />
      ) : null}
      {message.content ? <MessageMarkdown content={message.content} /> : null}
      {message.role === "assistant" && !usesTurnTrace ? <ToolCallList toolCalls={message.toolCalls} /> : null}
      <CitationList citations={message.citations} />
    </article>
  );
}

/** 将历史消息中的 @ 文件 ID 转成安全展示名称；文件被删除后保留可解释的占位。 */
function getMentionedFileLabel(fileId: string, notes: Note[], documents: WorkspaceDocument[]) {
  return notes.find((note) => note.id === fileId)?.title ?? documents.find((document) => document.id === fileId)?.title ?? "已失效文件";
}

/** 安全渲染 Agent 对话中的 GFM Markdown，避免模型内容中的 HTML 被直接执行。 */
function MessageMarkdown({ content }: { content: string }) {
  return (
    <div className="message-markdown">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={[rehypeSanitize]}
        components={{
          a: (props) => <MarkdownLink {...props} source="agent_message" />,
        }}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
}
