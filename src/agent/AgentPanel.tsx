import {
  ArrowRight,
  Check,
  Database,
  History,
  Layers3,
  MessageSquareText,
  PanelRightOpen,
  Plus,
  Trash2,
  X,
  Sparkles,
} from "lucide-react";
import ReactMarkdown from "react-markdown";
import rehypeSanitize from "rehype-sanitize";
import remarkGfm from "remark-gfm";
import { CitationList } from "./CitationList";
import { ToolCallList } from "./ToolCallList";
import {
  getScopeSummaryLabel,
  getSessionKnowledgeBaseLabel,
  getSessionNoteLabel,
  getSessionTypeLabel,
} from "../shared/selectors";
import type { AgentSession, AgentSkill, KnowledgeBase, Note } from "../shared/types";

/** 右侧 Agent 侧栏，承载会话、工具调用、检索范围、引用和输入框。 */
export function AgentPanel({
  sessions,
  activeSession,
  activeKnowledgeBase,
  knowledgeBases,
  notes,
  prompt,
  skills,
  isBusy,
  isSessionListOpen,
  isSessionContextOpen,
  isScopeSelectorOpen,
  onToggleSessionList,
  onToggleSessionContext,
  onToggleScopeSelector,
  onCreateSession,
  onSelectSession,
  onDeleteSession,
  onToggleScopeKnowledgeBase,
  onPromptChange,
  onSubmitPrompt,
}: {
  sessions: AgentSession[];
  activeSession: AgentSession;
  activeKnowledgeBase: KnowledgeBase;
  knowledgeBases: KnowledgeBase[];
  notes: Note[];
  prompt: string;
  skills: AgentSkill[];
  isBusy: boolean;
  isSessionListOpen: boolean;
  isSessionContextOpen: boolean;
  isScopeSelectorOpen: boolean;
  onToggleSessionList: () => void;
  onToggleSessionContext: () => void;
  onToggleScopeSelector: () => void;
  onCreateSession: () => void;
  onSelectSession: (sessionId: string) => void;
  onDeleteSession: (sessionId: string) => void;
  onToggleScopeKnowledgeBase: (knowledgeBaseId: string) => void;
  onPromptChange: (value: string) => void;
  onSubmitPrompt: () => void;
}) {
  /** 当前会话选中的知识库 ID，用于驱动范围摘要和多选列表。 */
  const selectedKnowledgeBaseIds = activeSession.knowledgeBaseIds.length
    ? activeSession.knowledgeBaseIds
    : [activeKnowledgeBase.id];
  /** 当前会话的知识库集合，当前激活知识库不能被移除。 */
  const selectedKnowledgeBaseSet = new Set(selectedKnowledgeBaseIds);
  /** 当前会话范围摘要，展示 Agent 可调用检索工具的权限边界。 */
  const selectedScopeLabel = getScopeSummaryLabel(activeSession, knowledgeBases);
  /** 当前会话的写入状态，用不可点击标签展示，避免和上下文弹窗入口混淆。 */
  const writeStatus = activeSession.pendingChange?.status === "pending" ? "待确认 diff" : "写入需确认";
  /** 已启用 skill 会以名称和描述进入 system prompt，具体是否使用交给 Agent 判断。 */
  const enabledSkillCount = skills.filter((skill) => skill.enabled).length;

  return (
    <aside className="agent-panel" aria-label="AI 侧栏">
      <header className="agent-header">
        <div>
          <p className="section-label">Agent Loop</p>
          <h2>{activeSession.title}</h2>
        </div>
        <div className="agent-header-actions">
          <button className="icon-button" type="button" title="查看上下文" onClick={onToggleSessionContext}>
            <PanelRightOpen size={17} />
          </button>
          <button className="icon-button" type="button" title="会话历史" onClick={onToggleSessionList}>
            <History size={17} />
          </button>
          <button className="icon-button" type="button" title="新建会话" onClick={onCreateSession}>
            <Plus size={17} />
          </button>
        </div>
      </header>

      <div className="session-summary" aria-label="当前会话摘要">
        <span>{getSessionTypeLabel(activeSession.type)}</span>
        <span>{selectedScopeLabel}</span>
        <span>{getSessionNoteLabel(activeSession, notes)}</span>
        <span className={`session-write-status ${activeSession.pendingChange?.status === "pending" ? "pending" : ""}`}>
          {writeStatus}
        </span>
      </div>

      {isSessionListOpen && (
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
              <div
                className={`session-row ${session.id === activeSession.id ? "active" : ""}`}
                key={session.id}
              >
                <button className="session-row-main" type="button" onClick={() => onSelectSession(session.id)}>
                  <span className="session-row-title">
                    <MessageSquareText size={14} />
                    <strong>{session.title}</strong>
                  </span>
                  <span className="session-row-meta">
                    <span>{getSessionTypeLabel(session.type)}</span>
                    <span>{getSessionKnowledgeBaseLabel(session, knowledgeBases)}</span>
                    <time dateTime={session.createdAt}>创建：{session.createdAt}</time>
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
      )}

      {isSessionContextOpen && (
        <section className="context-popover" aria-label="会话上下文包">
          <div className="popover-header">
            <div>
              <p className="section-label">Context</p>
              <h3>上下文包</h3>
            </div>
            <button className="icon-button" type="button" title="关闭上下文包" onClick={onToggleSessionContext}>
              <X size={15} />
            </button>
          </div>
          <div className="context-popover-body">
            <div className="context-matrix">
              <div>
                <span>工具检索范围</span>
                <strong>{getSessionKnowledgeBaseLabel(activeSession, knowledgeBases)}</strong>
              </div>
              <div>
                <span>当前文件</span>
                <strong>{getSessionNoteLabel(activeSession, notes)}</strong>
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
            <p className="context-note">
              Agent 只有调用 `search_notes` 或 `read_note` 工具后，才会展示知识库引用。
            </p>
          </div>
        </section>
      )}

      <button
        className={`scope-selector ${selectedKnowledgeBaseIds.length > 1 ? "active" : ""}`}
        type="button"
        aria-expanded={isScopeSelectorOpen}
        onClick={onToggleScopeSelector}
      >
        <Layers3 size={17} />
        <span>
          <strong>工具范围：{selectedScopeLabel}</strong>
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
                    checked={isSelected}
                    disabled={isActiveKnowledgeBase}
                    onChange={() => onToggleScopeKnowledgeBase(knowledgeBase.id)}
                    type="checkbox"
                  />
                  <span className="scope-check">{isSelected && <Check size={12} />}</span>
                  <Database size={15} />
                  <span className="scope-option-copy">
                    <strong>{knowledgeBase.name}</strong>
                    <span>{isActiveKnowledgeBase ? "当前激活，默认选中" : knowledgeBase.path}</span>
                  </span>
                </label>
              );
            })}
          </div>
        </section>
      )}

      <div className="message-list" aria-live="polite">
        {activeSession.messages.map((message) => (
          <article className={`message ${message.role}`} key={message.id}>
            <div className="message-role">
              {message.role === "assistant" ? <Sparkles size={14} /> : <MessageSquareText size={14} />}
              <span>{message.role === "assistant" ? "Cici Agent" : "你"}</span>
            </div>
            <MessageMarkdown content={message.content} />
            <ToolCallList toolCalls={message.toolCalls} />
            <CitationList citations={message.citations} />
          </article>
        ))}
      </div>

      <footer className="agent-input">
        <div className="agent-input-toolbar">
          <div className="skill-select" aria-label="当前启用 Skills">
            <Sparkles size={14} />
            <span>Skill</span>
            <strong>{enabledSkillCount} 个已启用</strong>
          </div>
        </div>
        <textarea
          value={prompt}
          onChange={(event) => onPromptChange(event.target.value)}
          placeholder="和知识库助手对话；需要依据本地笔记时，Agent 会自行调用工具"
          aria-label="Agent 输入"
          disabled={isBusy}
        />
        <button className="primary-button compact" type="button" onClick={onSubmitPrompt} disabled={isBusy}>
          <ArrowRight size={16} />
          发送
        </button>
      </footer>
    </aside>
  );
}

/** 安全渲染 Agent 对话中的 GFM Markdown，避免模型内容中的 HTML 被直接执行。 */
function MessageMarkdown({ content }: { content: string }) {
  return (
    <div className="message-markdown">
      <ReactMarkdown remarkPlugins={[remarkGfm]} rehypePlugins={[rehypeSanitize]}>
        {content}
      </ReactMarkdown>
    </div>
  );
}
