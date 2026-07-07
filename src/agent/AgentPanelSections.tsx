import { Check, Database, Layers3, MessageSquareText, Sparkles, Trash2, X } from "lucide-react";
import ReactMarkdown from "react-markdown";
import rehypeSanitize from "rehype-sanitize";
import remarkGfm from "remark-gfm";
import { MarkdownLink } from "../shared/MarkdownLink";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import {
  getScopeSummaryLabel,
  getSessionKnowledgeBaseLabel,
  getSessionNoteLabel,
  getSessionTypeLabel,
} from "../shared/selectors";
import {
  encodeModelSelection,
  FOLLOW_DEFAULT_MODEL_SELECTION,
  getProviderModelSelectionLabel,
  getSessionModelLabel,
} from "../shared/modelSelection";
import { ModelCascadeSelector } from "../shared/ModelCascadeSelector";
import type { AgentSession, KnowledgeBase, ModelConfig, Note } from "../shared/types";
import { CitationList } from "./CitationList";
import { ToolCallList } from "./ToolCallList";

/** 会话摘要条，展示工具范围、当前文件、模型和写入状态。 */
export function AgentSessionSummary({
  activeSession,
  knowledgeBases,
  notes,
  modelConfig,
}: {
  activeSession: AgentSession;
  knowledgeBases: KnowledgeBase[];
  notes: Note[];
  modelConfig: ModelConfig;
}) {
  /** 当前会话范围摘要，展示 Agent 可调用检索工具的权限边界。 */
  const selectedScopeLabel = getScopeSummaryLabel(activeSession, knowledgeBases);
  /** 当前会话的写入状态，用不可点击标签展示，避免和上下文弹窗入口混淆。 */
  const writeStatus = activeSession.pendingChange?.status === "pending" ? "待确认 diff" : "写入需确认";

  return (
    <div className="session-summary" aria-label="当前会话摘要">
      <OverflowTooltipText text={selectedScopeLabel} logArea="agent_session_scope_summary" />
      <OverflowTooltipText text={getSessionNoteLabel(activeSession, notes)} logArea="agent_session_note_summary" />
      {modelConfig.enabled && (
        <OverflowTooltipText text={getSessionModelLabel(activeSession, modelConfig)} logArea="agent_session_provider" />
      )}
      <OverflowTooltipText
        className={`session-write-status ${activeSession.pendingChange?.status === "pending" ? "pending" : ""}`}
        text={writeStatus}
        logArea="agent_session_write_status"
      />
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
        <button className="icon-button" type="button" title="关闭会话历史" onClick={onToggleSessionList}>
          <X size={15} />
        </button>
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
                <OverflowTooltipText text={getSessionTypeLabel(session.type)} logArea="agent_session_history_type" />
                <OverflowTooltipText text={getSessionKnowledgeBaseLabel(session, knowledgeBases)} logArea="agent_session_history_scope" />
                <OverflowTooltipText
                  as="time"
                  dateTime={session.createdAt}
                  text={`创建：${session.createdAt}`}
                  logArea="agent_session_history_created_at"
                />
              </span>
              {session.pendingChange?.status === "pending" && <span className="session-pending">待确认 diff</span>}
            </button>
            <button
              className="icon-button danger session-delete-button"
              type="button"
              title="删除会话"
              onClick={() => onDeleteSession(session.id)}
            >
              <Trash2 size={14} />
            </button>
          </div>
        ))}
      </div>
    </section>
  );
}

/** 会话上下文浮层，集中展示工具范围、当前文件和会话默认模型。 */
export function AgentSessionContextPopover({
  activeSession,
  knowledgeBases,
  notes,
  modelConfig,
  onToggleSessionContext,
  onSetSessionModelSelection,
}: {
  activeSession: AgentSession;
  knowledgeBases: KnowledgeBase[];
  notes: Note[];
  modelConfig: ModelConfig;
  onToggleSessionContext: () => void;
  onSetSessionModelSelection: (selection: string) => void;
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

  return (
    <section className="context-popover" aria-label="会话上下文">
      <div className="popover-header">
        <div>
          <p className="section-label">Context</p>
          <h3>上下文</h3>
        </div>
        <button className="icon-button" type="button" title="关闭上下文" onClick={onToggleSessionContext}>
          <X size={15} />
        </button>
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
            <OverflowTooltipText as="strong" text={getSessionNoteLabel(activeSession, notes)} logArea="agent_context_note" />
          </div>
          <div>
            <span>消息</span>
            <strong>{activeSession.messages.length} 条</strong>
          </div>
          <div>
            <span>写入</span>
            <strong>{writeStatus}</strong>
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
          Agent 只有调用 `search_notes` 或 `read_note` 工具后，才会展示知识库引用。
        </p>
      </div>
    </section>
  );
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
        aria-expanded={isScopeSelectorOpen}
        onClick={onToggleScopeSelector}
      >
        <Layers3 size={17} />
        <span>
          <OverflowTooltipText as="strong" text={`工具范围：${selectedScopeLabel}`} logArea="agent_scope_selector_summary" />
          <span>当前知识库默认选中，Agent 不能越权检索未选目录</span>
        </span>
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
              <button className="icon-button" type="button" title="关闭工具范围" onClick={onToggleScopeSelector}>
                <X size={15} />
              </button>
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

/** Agent 消息列表，安全渲染 Markdown、工具调用和知识库引用。 */
export function AgentMessageList({ activeSession }: { activeSession: AgentSession }) {
  return (
    <div className="message-list" aria-live="polite">
      {activeSession.messages.map((message) => (
        <article className={`message ${message.role}`} key={message.id}>
          <div className="message-role">
            {message.role === "assistant" ? <Sparkles size={14} /> : <MessageSquareText size={14} />}
            <span>{message.role === "assistant" ? "橘记 Agent" : "你"}</span>
          </div>
          <MessageMarkdown content={message.content} />
          <ToolCallList toolCalls={message.toolCalls} />
          <CitationList citations={message.citations} />
        </article>
      ))}
    </div>
  );
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
