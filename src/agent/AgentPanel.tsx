import {
  ArrowRight,
  BrainCircuit,
  Check,
  Database,
  History,
  Layers3,
  MessageSquareText,
  PanelRightClose,
  PanelRightOpen,
  Plus,
  Trash2,
  X,
  Sparkles,
} from "lucide-react";
import { useRef, type CompositionEventHandler, type KeyboardEventHandler } from "react";
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
import { MarkdownLink } from "../shared/MarkdownLink";
import { logDebug } from "../shared/logger";
import { useDismissable } from "../shared/useDismissable";
import type { AgentSession, AgentSkill, KnowledgeBase, ModelConfig, Note } from "../shared/types";

/** 会话/本轮模型选择器统一使用的“跟随默认”占位值，不写入具体 providerId。 */
const FOLLOW_DEFAULT_VALUE = "";
/** 输入法结束组词后的短保护窗口；部分中文输入法会先触发 compositionend，再派发 Enter keydown。 */
const PROMPT_IME_ENTER_GUARD_MS = 150;

/** 右侧 Agent 侧栏，承载会话、工具调用、检索范围、引用和输入框。 */
export function AgentPanel({
  sessions,
  activeSession,
  activeKnowledgeBase,
  knowledgeBases,
  notes,
  prompt,
  skills,
  modelConfig,
  turnModelProviderId,
  isBusy,
  isSessionListOpen,
  isSessionContextOpen,
  isScopeSelectorOpen,
  onToggleSessionList,
  onToggleSessionContext,
  onToggleScopeSelector,
  onCollapsePanel,
  onCreateSession,
  onSelectSession,
  onDeleteSession,
  onToggleScopeKnowledgeBase,
  onPromptChange,
  onSubmitPrompt,
  onTurnModelProviderChange,
  onSetSessionModelProvider,
}: {
  sessions: AgentSession[];
  activeSession: AgentSession;
  activeKnowledgeBase: KnowledgeBase;
  knowledgeBases: KnowledgeBase[];
  notes: Note[];
  prompt: string;
  skills: AgentSkill[];
  modelConfig: ModelConfig;
  /** 本轮显式选择的 Provider，空字符串表示跟随会话/全局默认。 */
  turnModelProviderId: string;
  isBusy: boolean;
  isSessionListOpen: boolean;
  isSessionContextOpen: boolean;
  isScopeSelectorOpen: boolean;
  onToggleSessionList: () => void;
  onToggleSessionContext: () => void;
  onToggleScopeSelector: () => void;
  onCollapsePanel: () => void;
  onCreateSession: () => void;
  onSelectSession: (sessionId: string) => void;
  onDeleteSession: (sessionId: string) => void;
  onToggleScopeKnowledgeBase: (knowledgeBaseId: string) => void;
  onPromptChange: (value: string) => void;
  onSubmitPrompt: () => void;
  onTurnModelProviderChange: (providerId: string) => void;
  onSetSessionModelProvider: (providerId: string) => void;
}) {
  /** 已启用的 Provider 列表；未启用的 provider 不出现在选择器中。 */
  const enabledProviders = modelConfig.providers.filter((provider) => provider.enabled);
  /** 全局默认 provider 名称，用于“跟随默认”选项的说明文案。 */
  const defaultProvider = modelConfig.providers.find((provider) => provider.id === modelConfig.defaultProviderId);
  /** 会话当前设置的默认 provider（可能未设置，回退到全局默认）。 */
  const sessionProvider = activeSession.modelProviderId
    ? modelConfig.providers.find((provider) => provider.id === activeSession.modelProviderId)
    : undefined;
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
  /** 当前 Agent 输入框是否处于输入法组词状态，弥补不同浏览器 nativeEvent.isComposing 不一致的问题。 */
  const isPromptComposingRef = useRef(false);
  /** 最近一次输入法组词结束时间，用于过滤 compositionend 后紧邻的候选确认 Enter。 */
  const lastPromptCompositionEndAtRef = useRef(0);
  /** 记录已在组词阶段捕获到 Enter，避免常规事件顺序下过度拦截用户后续发送。 */
  const didHandlePromptComposingEnterRef = useRef(false);
  /** Agent 输入框开始组词时只更新本地状态，不记录正文内容。 */
  const handlePromptCompositionStart: CompositionEventHandler<HTMLTextAreaElement> = () => {
    isPromptComposingRef.current = true;
    lastPromptCompositionEndAtRef.current = 0;
    didHandlePromptComposingEnterRef.current = false;
  };
  /** Agent 输入框结束组词时开启短保护窗口，兼容输入法确认键和 keydown 顺序反转的情况。 */
  const handlePromptCompositionEnd: CompositionEventHandler<HTMLTextAreaElement> = () => {
    isPromptComposingRef.current = false;
    lastPromptCompositionEndAtRef.current = didHandlePromptComposingEnterRef.current ? 0 : Date.now();
    didHandlePromptComposingEnterRef.current = false;
  };
  /** Agent 输入框快捷键处理器；Enter 提交，Shift+Enter 继续使用 textarea 原生换行。 */
  const handlePromptKeyDown: KeyboardEventHandler<HTMLTextAreaElement> = (event) => {
    if (event.key !== "Enter" || event.shiftKey) {
      return;
    }

    const promptLength = prompt.trim().length;
    const timeSinceCompositionEnd = Date.now() - lastPromptCompositionEndAtRef.current;
    const isRecentCompositionEnd =
      lastPromptCompositionEndAtRef.current > 0 && timeSinceCompositionEnd >= 0 && timeSinceCompositionEnd <= PROMPT_IME_ENTER_GUARD_MS;
    const isImeConfirmationEnter = isPromptComposingRef.current || event.nativeEvent.isComposing || event.keyCode === 229 || isRecentCompositionEnd;

    // 输入法组词或刚结束组词时的 Enter 只用于确认候选词，不能触发消息发送。
    if (isImeConfirmationEnter) {
      didHandlePromptComposingEnterRef.current = true;

      if (isRecentCompositionEnd) {
        // compositionend 后补发的 Enter 已完成候选确认，这里阻止 textarea 额外插入换行。
        event.preventDefault();
        lastPromptCompositionEndAtRef.current = 0;
      }

      logDebug("忽略输入法确认用 Enter。", {
        category: "frontend",
        event: "agent_prompt_enter_ime_guard",
        status: "ignored",
        metadata: {
          isNativeComposing: event.nativeEvent.isComposing,
          isTrackedComposing: isPromptComposingRef.current,
          promptLength,
        },
      });
      return;
    }

    event.preventDefault();
    logDebug("通过回车快捷键提交 Agent 输入。", {
      category: "frontend",
      event: "agent_prompt_enter_submit",
      status: promptLength ? "submitted" : "ignored",
      metadata: {
        hasActivePendingChange: activeSession.pendingChange?.status === "pending",
        messageCount: activeSession.messages.length,
        promptLength,
      },
    });

    // 空输入只吞掉回车，避免产生无意义空行；真正发送仍复用按钮的同一业务入口。
    if (!promptLength) {
      return;
    }

    onSubmitPrompt();
  };

  // AgentPanel 三个 popover 共用同一个外层 aside 作为 ref 容器：
  // 点击 Agent 面板以外的区域才关闭浮层；面板内切入别的功能按钮时由各按钮的 toggle 自行处理。
  const panelRef = useRef<HTMLElement | null>(null);
  useDismissable(isSessionListOpen, onToggleSessionList, { externalRef: panelRef });
  useDismissable(isSessionContextOpen, onToggleSessionContext, { externalRef: panelRef });
  useDismissable(isScopeSelectorOpen, onToggleScopeSelector, { externalRef: panelRef });

  return (
    <aside ref={panelRef} className="agent-panel" aria-label="AI 侧栏">
      <header className="agent-header">
        <div>
          <p className="section-label">Agent</p>
          <h2>{activeSession.title}</h2>
        </div>
        <div className="agent-header-actions">
          <button className="icon-button" type="button" title="收起 Agent 协作区" onClick={onCollapsePanel}>
            <PanelRightClose size={17} />
          </button>
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
        <span>{selectedScopeLabel}</span>
        <span>{getSessionNoteLabel(activeSession, notes)}</span>
        {modelConfig.enabled && <span>{sessionProvider?.name ?? defaultProvider?.name ?? "模型未配置"}</span>}
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
            {modelConfig.enabled && (
              <label className="context-model-select">
                <span>会话默认模型</span>
                <span className="select-control">
                  <select
                    value={activeSession.modelProviderId ?? FOLLOW_DEFAULT_VALUE}
                    onChange={(event) => onSetSessionModelProvider(event.target.value)}
                  >
                    <option value={FOLLOW_DEFAULT_VALUE}>
                      跟随全局默认{defaultProvider ? `（${defaultProvider.name}）` : ""}
                    </option>
                    {enabledProviders.map((provider) => (
                      <option key={provider.id} value={provider.id}>
                        {provider.name}
                      </option>
                    ))}
                  </select>
                </span>
              </label>
            )}
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
                    className="control-checkbox-input"
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
          {modelConfig.enabled && enabledProviders.length > 0 && (
            <label className="turn-model-select" aria-label="本轮使用的模型">
              <BrainCircuit size={14} />
              <span className="select-control inline-select-control">
                <select value={turnModelProviderId} onChange={(event) => onTurnModelProviderChange(event.target.value)}>
                  <option value={FOLLOW_DEFAULT_VALUE}>
                    本轮：跟随会话默认{sessionProvider ? `（${sessionProvider.name}）` : defaultProvider ? `（${defaultProvider.name}）` : ""}
                  </option>
                  {enabledProviders.map((provider) => (
                    <option key={provider.id} value={provider.id}>
                      本轮：{provider.name}
                    </option>
                  ))}
                </select>
              </span>
            </label>
          )}
        </div>
        <textarea
          value={prompt}
          onChange={(event) => onPromptChange(event.target.value)}
          onCompositionStart={handlePromptCompositionStart}
          onCompositionEnd={handlePromptCompositionEnd}
          onKeyDown={handlePromptKeyDown}
          placeholder="和知识库助手对话；需要依据本地笔记时，Agent 会自行调用工具"
          aria-label="Agent 输入"
          disabled={isBusy}
        />
        <button className="primary-button compact agent-send-button" type="button" onClick={onSubmitPrompt} disabled={isBusy}>
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
