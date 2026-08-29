import { Clock, Database, FileText, FolderOpen, Gauge, Layers3, MessageSquareText, ShieldAlert, Sparkles, Trash2, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import rehypeSanitize from "rehype-sanitize";
import { Button } from "../shared/Button";
import { Checkbox } from "../shared/Checkbox";
import { Chip } from "../shared/Chip";
import { cn } from "../shared/cn";
import { listRowClassName } from "../shared/ListRow";
import {
  createMarkdownComponents,
  markdownMessageClassName,
  markdownRemarkPlugins,
  protectGfmTablePipesInInlineCode,
} from "../shared/markdown";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import { SegmentedControl, SegmentedControlItem } from "../shared/SegmentedControl";
import { agentPopoverClassName, popoverHeaderClassName, sectionLabelClassName } from "../shared/ui";
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
import { listSessionProjectInstructions } from "../shared/projectInstructions";
import { loadAgentPromptDump } from "../shared/api/agent";
import { openAppLogFolder } from "../shared/api/logs";
import {
  formatContextMeterChip,
  formatContextUsageLabel,
  formatContextWindowLabel,
  resolveContextMeter,
} from "../shared/contextUsage";
import type {
  AgentMessage,
  AgentPromptDump,
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
import {
  getTraceScrollFingerprint,
  isNearScrollBottom,
  shouldRenderTurnTrace,
} from "./agentTrace";
import { ToolCallList } from "./ToolCallList";

/** 会话摘要条只保留当前文件、上下文占用和待确认写入，避免和输入条、范围入口重复。 */
export function AgentSessionSummary({
  activeSession,
  currentFileLabel,
  modelConfig,
}: {
  activeSession: AgentSession;
  /** 工作台当前焦点文件；它是本轮默认编辑目标，独立于会话恢复锚点。 */
  currentFileLabel: string;
  modelConfig: ModelConfig;
}) {
  const isPendingWrite = activeSession.pendingChange?.status === "pending";
  const contextMeter = resolveContextMeter(activeSession, modelConfig);
  const contextMeterLabel = formatContextMeterChip(contextMeter);

  const contextMeterTitle = `窗口 ${formatContextWindowLabel(contextMeter)} · 占用 ${formatContextUsageLabel(contextMeter)}`;

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-nowrap items-center gap-1.5 overflow-hidden" aria-label="当前会话摘要">
      <Chip className="max-w-full flex-1 rounded-full border-border bg-surface py-[5px] pr-[9px] pl-[9px] font-normal text-ink-muted">
        <FileText size={13} className="shrink-0" />
        <OverflowTooltipText className="min-w-0 truncate" text={currentFileLabel} logArea="agent_session_current_file_summary" />
      </Chip>
      {contextMeterLabel && (
        <span
          className="inline-flex shrink-0"
          title={contextMeterTitle}
          aria-label={contextMeterTitle}
        >
          {/* 占用标签很短，按内容单行展示。外层若是 shrink-to-fit，子级 max-w-[n%] 会把胶囊压成竖排。 */}
          <Chip className="max-w-none rounded-full border-border bg-surface py-[5px] pr-[9px] pl-[9px] font-normal text-ink-muted">
            <Gauge size={13} className="shrink-0" />
            <OverflowTooltipText
              className="whitespace-nowrap"
              text={contextMeterLabel}
              logArea="agent_session_context_usage"
            />
          </Chip>
        </span>
      )}
      {isPendingWrite && (
        <OverflowTooltipText
          className="shrink-0 rounded-control border border-[rgba(var(--danger-rgb),0.26)] bg-danger-soft px-1.5 py-0.5 text-xs text-danger"
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
    description: "只在选中的知识库里工作，写入需确认。",
  },
  advanced: {
    label: "进阶",
    description: "可整理目录并运行 Skill，落盘前仍确认。",
  },
  autonomous: {
    label: "完全",
    description: "可在合规路径上读列，校验通过后可自动落盘；不是整台电脑。",
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
    <div className="inline-flex min-w-0 shrink items-center gap-1 bg-transparent p-0 text-xs text-agent-strong" aria-label="当前会话权限">
      <span className="inline-flex shrink-0 items-center text-ink-muted">
        <ShieldAlert size={13} />
      </span>
      <SegmentedControl
        className="ml-0 w-auto grid grid-cols-3 gap-px rounded-full"
        role="radiogroup"
        aria-label="当前会话权限级别"
      >
        {([
          ["basic", true],
          ["advanced", agentSecurity.advancedExecutionEnabled],
          ["autonomous", agentSecurity.autonomousModeEnabled],
        ] as const).map(([level, isEnabled]) => {
          const copy = AGENT_SECURITY_LEVEL_COPY[level];

          return (
            <SegmentedControlItem
              className="min-h-[26px] rounded-full px-2 text-[11px] font-bold"
              active={activeSession.securityLevel === level}
              role="radio"
              aria-checked={activeSession.securityLevel === level}
              aria-label={`${copy.label}权限。${copy.description}`}
              title={`${copy.description}${isEnabled ? "" : "选择后将启用此能力。"}`}
              disabled={isBusy}
              key={level}
              onClick={() => onSecurityLevelChange?.(level)}
            >
              {copy.label}
            </SegmentedControlItem>
          );
        })}
      </SegmentedControl>
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
    <section className={agentPopoverClassName} aria-label="会话历史">
      <div className={popoverHeaderClassName}>
        <div>
          <p className={sectionLabelClassName}>Sessions</p>
          <h3 className="m-0 text-lg text-ink-strong">会话历史</h3>
        </div>
        <Button variant="icon" title="关闭会话历史" onClick={onToggleSessionList}>
          <X size={15} />
        </Button>
      </div>
      <div className="mt-3 grid content-start gap-2 overflow-auto pr-0.5">
        {sessions.map((session) => (
          <div
            className={listRowClassName({
              active: session.id === activeSession.id,
              className: "grid grid-cols-[minmax(0,1fr)_auto] border-border-translucent bg-surface-translucent p-1.5",
            })}
            key={session.id}
          >
            <button className="grid min-w-0 gap-1 p-1 text-left" type="button" onClick={() => onSelectSession(session.id)}>
              <span className="flex min-w-0 items-center gap-1.5">
                <MessageSquareText size={14} className="shrink-0" />
                <OverflowTooltipText as="strong" className="min-w-0 truncate text-ink-strong" text={session.title} logArea="agent_session_history_title" />
              </span>
              <span className="grid min-w-0 grid-cols-[minmax(0,max-content)_minmax(0,1fr)] items-center gap-x-1.5 gap-y-[3px] text-xs text-ink-muted">
                {session.imIdentity ? (
                  <span className="max-w-full shrink-0 truncate rounded-full border border-primary-border bg-accent-soft px-1.5 py-px text-[11px] font-semibold leading-[1.4] text-accent-strong">
                    {getImSessionSourceLabel(session)}
                  </span>
                ) : (
                  <OverflowTooltipText className="min-w-0 truncate leading-[1.35]" text={getSessionTypeLabel(session.type)} logArea="agent_session_history_type" />
                )}
                <OverflowTooltipText className="min-w-0 truncate leading-[1.35]" text={getSessionKnowledgeBaseLabel(session, knowledgeBases)} logArea="agent_session_history_scope" />
                {getImSessionRecentMessageLabel(session) && (
                  <OverflowTooltipText
                    className="col-span-full min-w-0 truncate"
                    text={getImSessionRecentMessageLabel(session)}
                    logArea="agent_session_history_recent_message"
                  />
                )}
                <OverflowTooltipText
                  as="time"
                  className="col-span-full min-w-0 truncate leading-[1.35]"
                  dateTime={session.createdAt}
                  text={`创建：${session.createdAt}`}
                  logArea="agent_session_history_created_at"
                />
                <OverflowTooltipText
                  as="time"
                  className="col-span-full min-w-0 truncate leading-[1.35]"
                  dateTime={session.updatedAt}
                  text={`最近：${session.updatedAt}`}
                  logArea="agent_session_history_updated_at"
                />
              </span>
              {session.pendingChange?.status === "pending" && (
                <span className="rounded-control border border-[rgba(var(--danger-rgb),0.26)] bg-danger-soft px-1.5 py-0.5 text-xs text-danger">
                  待确认 diff
                </span>
              )}
            </button>
            <Button
              variant="icon"
              tone="danger"
              className="size-[30px] min-h-[30px] shrink-0"
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
  onOpenProjectInstruction,
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
  onOpenProjectInstruction: (noteId: string) => void;
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
  const contextMeter = resolveContextMeter(activeSession, modelConfig);
  const projectInstructions = listSessionProjectInstructions(notes, knowledgeBases, activeSession.knowledgeBaseIds);
  const projectInstructionLabel = projectInstructions.length
    ? projectInstructions.map((item) => `${item.knowledgeBase.name}/${item.note.path}`).join(" · ")
    : "未配置";
  const [promptDump, setPromptDump] = useState<AgentPromptDump | null>(null);
  const [promptDumpError, setPromptDumpError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    loadAgentPromptDump(activeSession.id)
      .then((dump) => {
        if (!cancelled) {
          setPromptDump(dump);
          setPromptDumpError(null);
        }
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          setPromptDump(null);
          setPromptDumpError(error instanceof Error ? error.message : "无法读取发给模型的上下文");
        }
      });

    return () => {
      cancelled = true;
    };
  }, [activeSession.id, activeSession.updatedAt, activeSession.contextUsage?.recordedAt]);

  return (
    <section className={agentPopoverClassName} aria-label="会话上下文">
      <div className={popoverHeaderClassName}>
        <div>
          <p className={sectionLabelClassName}>Context</p>
          <h3 className="m-0 text-lg text-ink-strong">上下文</h3>
        </div>
        <div className="inline-flex items-center gap-2 text-xs text-ink-muted">
          <Button variant="icon" title="整理上下文" onClick={onCompactAgentContext} disabled={isBusy}>
            <Sparkles size={15} />
          </Button>
          <Button variant="icon" title="关闭上下文" onClick={onToggleSessionContext}>
            <X size={15} />
          </Button>
        </div>
      </div>
      <div className="overflow-auto pr-0.5">
        <div className="mt-3 grid grid-cols-2 gap-2">
          <div className="rounded-control border border-border-translucent bg-surface-translucent p-[9px]">
            <span className="text-xs text-ink-muted">模型窗口</span>
            <strong className="mt-1 block text-[13px] text-ink-strong">{formatContextWindowLabel(contextMeter)}</strong>
            {!contextMeter.windowKnown ? (
              <span className="mt-1 block text-[11px] text-ink-muted">可在模型设置中填写上下文窗口</span>
            ) : null}
          </div>
          <div className="rounded-control border border-border-translucent bg-surface-translucent p-[9px]">
            <span className="text-xs text-ink-muted">上下文占用</span>
            <strong className="mt-1 block text-[13px] text-ink-strong">{formatContextUsageLabel(contextMeter)}</strong>
            {!contextMeter.matchesCurrentModel && contextMeter.usageModelId ? (
              <span className="mt-1 block text-[11px] text-ink-muted">上次为 {contextMeter.usageModelId}</span>
            ) : contextMeter.recordedAt ? (
              <span className="mt-1 block text-[11px] text-ink-muted">{contextMeter.recordedAt}</span>
            ) : null}
          </div>
          <div className="rounded-control border border-border-translucent bg-surface-translucent p-[9px]">
            <span className="text-xs text-ink-muted">工具检索范围</span>
            <OverflowTooltipText
              as="strong"
              className="mt-1 block text-[13px] text-ink-strong"
              text={getSessionKnowledgeBaseLabel(activeSession, knowledgeBases)}
              logArea="agent_context_scope"
            />
          </div>
          <div className="rounded-control border border-border-translucent bg-surface-translucent p-[9px]">
            <span className="text-xs text-ink-muted">当前文件</span>
            <OverflowTooltipText as="strong" className="mt-1 block text-[13px] text-ink-strong" text={currentFileLabel} logArea="agent_context_current_file" />
          </div>
          <div className="rounded-control border border-border-translucent bg-surface-translucent p-[9px]">
            <span className="text-xs text-ink-muted">会话恢复笔记</span>
            <OverflowTooltipText
              as="strong"
              className="mt-1 block text-[13px] text-ink-strong"
              text={getSessionRecoveryNoteLabel(activeSession, notes)}
              logArea="agent_context_session_recovery_note"
            />
          </div>
          <div className="rounded-control border border-border-translucent bg-surface-translucent p-[9px]">
            <span className="text-xs text-ink-muted">消息</span>
            <strong className="mt-1 block text-[13px] text-ink-strong">{activeSession.messages.length} 条</strong>
          </div>
          <div className="rounded-control border border-border-translucent bg-surface-translucent p-[9px]">
            <span className="text-xs text-ink-muted">写入</span>
            <strong className="mt-1 block text-[13px] text-ink-strong">{writeStatus}</strong>
          </div>
          <div className="rounded-control border border-border-translucent bg-surface-translucent p-[9px]">
            <span className="text-xs text-ink-muted">工作记忆</span>
            <strong className="mt-1 block text-[13px] text-ink-strong">{memoryStatus}</strong>
          </div>
          <div className="rounded-control border border-border-translucent bg-surface-translucent p-[9px]">
            <span className="text-xs text-ink-muted">项目说明书</span>
            {projectInstructions.length ? (
              <button
                className="mt-1 block min-w-0 border-0 bg-transparent p-0 text-left text-[13px] font-bold text-ink-strong"
                type="button"
                onClick={() => onOpenProjectInstruction(projectInstructions[0].note.id)}
              >
                <OverflowTooltipText as="span" className="block min-w-0" text={projectInstructionLabel} logArea="agent_context_project_instruction" />
              </button>
            ) : (
              <strong className="mt-1 block text-[13px] text-ink-strong">{projectInstructionLabel}</strong>
            )}
          </div>
        </div>
        {modelConfig.enabled && (
          <label className="mt-2.5 grid gap-1.5 text-xs font-bold text-ink-muted">
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
        <section className="mt-2.5 rounded-control border border-border-translucent bg-surface-translucent p-[9px]" aria-label="发给模型的上下文">
          <div className="flex items-start justify-between gap-2">
            <div>
              <span className="text-xs text-ink-muted">发给模型的上下文</span>
              <strong className="mt-1 block text-[13px] text-ink-strong">
                {promptDump
                  ? `第 ${promptDump.round} 轮 · ${promptDump.messages.length} 条 · ${promptDump.totalChars.toLocaleString()} 字`
                  : "尚未发送"}
              </strong>
            </div>
            <Button variant="ghost" size="compact" title="打开应用日志目录" onClick={() => void openAppLogFolder()}>
              <FolderOpen size={14} />
              日志
            </Button>
          </div>
          {promptDumpError ? <p className="mt-2 mb-0 text-xs text-danger">{promptDumpError}</p> : null}
          {promptDump ? (
            <ol className="mt-2 mb-0 grid list-none gap-1.5 p-0">
              {promptDump.messages.map((message) => (
                <li className="min-w-0 rounded-small border border-border-translucent bg-surface p-1.5" key={`${message.role}-${message.index}`}>
                  <div className="flex items-center justify-between gap-2 text-[11px] text-ink-muted">
                    <span>{message.role}</span>
                    <span>{message.chars.toLocaleString()} 字{message.truncated ? " · 已截断预览" : ""}</span>
                  </div>
                  <pre className="m-0 max-h-24 overflow-auto whitespace-pre-wrap break-words font-mono text-[11px] leading-[1.45] text-ink">
                    {message.preview}
                  </pre>
                </li>
              ))}
            </ol>
          ) : (
            <p className="mt-2 mb-0 text-xs text-ink-muted">发送一轮后可在这里查看结构。完整 JSON 写在应用日志目录的 agent-prompts 下。</p>
          )}
        </section>
        <p className="text-xs text-ink-muted">
          占用取最近一次有效模型 usage；窗口来自当前模型目录，Provider 未提供时显示未知。当前文件是本轮默认编辑目标；会话恢复笔记只用于恢复旧会话位置。Agent 会按模型窗口装入尽量多的最近对话，更早内容进入工作记忆，也可按需检索会话历史和其他文件。
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
        className={cn(
          "inline-flex w-auto max-w-[46%] min-w-0 cursor-pointer items-center gap-1.5 rounded-full border border-border-translucent bg-surface-translucent px-[9px] py-[5px] text-left text-ink",
          (selectedKnowledgeBaseIds.length > 1 || isScopeSelectorOpen) && "border-primary-border bg-primary-wash",
        )}
        type="button"
        title="编辑工具范围"
        aria-label="工具范围"
        aria-expanded={isScopeSelectorOpen}
        onClick={onToggleScopeSelector}
      >
        <Layers3 size={14} className="shrink-0 text-agent" />
        <OverflowTooltipText className="min-w-0 truncate text-xs" text={selectedScopeLabel} logArea="agent_scope_selector_summary" />
      </button>

      {isScopeSelectorOpen && (
        <section className={agentPopoverClassName} aria-label="选择检索知识库">
          <div className={popoverHeaderClassName}>
            <div>
              <p className={sectionLabelClassName}>Scope</p>
              <h3 className="m-0 text-lg text-ink-strong">选择工具可访问知识库</h3>
            </div>
            <div className="inline-flex items-center gap-2 text-xs text-ink-muted">
              <span>
                {selectedKnowledgeBaseIds.length} / {knowledgeBases.length}
              </span>
              <Button variant="icon" title="关闭工具范围" onClick={onToggleScopeSelector}>
                <X size={15} />
              </Button>
            </div>
          </div>
          <div className="mt-3 grid content-start gap-2 overflow-auto pr-0.5">
            {knowledgeBases.map((knowledgeBase) => {
              const isActiveKnowledgeBase = knowledgeBase.id === activeKnowledgeBase.id;
              const isSelected = selectedKnowledgeBaseSet.has(knowledgeBase.id) || isActiveKnowledgeBase;

              return (
                <label
                  className={listRowClassName({
                    active: isSelected,
                    className: "relative border-border-translucent bg-surface-translucent",
                  })}
                  key={knowledgeBase.id}
                >
                  <Checkbox
                    checked={isSelected}
                    disabled={isActiveKnowledgeBase}
                    onChange={() => onToggleScopeKnowledgeBase(knowledgeBase.id)}
                  />
                  <Database size={15} />
                  <span className="min-w-0">
                    <OverflowTooltipText as="strong" className="block truncate text-ink-strong" text={knowledgeBase.name} logArea="agent_scope_option_name" />
                    <OverflowTooltipText
                      className="mt-[3px] block truncate text-xs text-ink-muted"
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
  queuedFollowUp,
}: {
  activeSession: AgentSession;
  notes: Note[];
  documents: WorkspaceDocument[];
  liveTurn?: AgentTurnProgressEvent | null;
  /** 当前回合结束后才会发给模型的下一条用户指令。 */
  queuedFollowUp?: string | null;
}) {
  const persistedIds = new Set(activeSession.messages.map((message) => message.id));
  const showLiveTurn =
    Boolean(liveTurn) && liveTurn?.sessionId === activeSession.id && !persistedIds.has(liveTurn.liveMessageId);
  const listRef = useRef<HTMLDivElement>(null);
  const stickToBottomRef = useRef(true);
  const sessionIdRef = useRef(activeSession.id);
  if (sessionIdRef.current !== activeSession.id) {
    sessionIdRef.current = activeSession.id;
    stickToBottomRef.current = true;
  }
  const traceScrollFingerprint = getTraceScrollFingerprint(showLiveTurn ? (liveTurn?.steps ?? []) : []);

  useEffect(() => {
    const list = listRef.current;
    if (!list || !stickToBottomRef.current) {
      return;
    }
    list.scrollTo({ top: list.scrollHeight });
  }, [
    activeSession.id,
    activeSession.messages.length,
    liveTurn?.content,
    liveTurn?.status,
    queuedFollowUp,
    traceScrollFingerprint,
  ]);

  return (
    <div
      className="min-h-0 flex-1 overflow-auto pr-0.5"
      aria-live="polite"
      ref={listRef}
      onScroll={(event) => {
        const list = event.currentTarget;
        stickToBottomRef.current = isNearScrollBottom(list.scrollTop, list.scrollHeight, list.clientHeight);
      }}
    >
      {activeSession.messages.length === 0 && !showLiveTurn && !queuedFollowUp && (
        <div className="grid min-h-full place-content-center justify-items-center gap-1.5 px-3 py-6 text-center text-ink-muted">
          <Sparkles size={16} className="text-agent opacity-70" />
          <p className="m-0 text-[13px] font-[650] text-ink">从下面开始提问</p>
          <span className="text-xs leading-[1.45]">@ 引用当前库里的文件，/ 选择本轮 Skill</span>
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
      {queuedFollowUp ? (
        <AgentMessageItem
          documents={documents}
          message={{
            id: "queued-follow-up",
            role: "user",
            content: queuedFollowUp,
          }}
          notes={notes}
          queued
        />
      ) : null}
    </div>
  );
}

/** 单条会话消息：过程区在最终回答之前，旧消息没有 trace 时回退到扁平运行信息。 */
function AgentMessageItem({
  message,
  notes,
  documents,
  liveStatus,
  queued = false,
}: {
  message: AgentMessage;
  notes: Note[];
  documents: WorkspaceDocument[];
  liveStatus?: AgentTurnProgressEvent["status"];
  /** 尚未进入模型的排队用户消息，只存在于当前界面。 */
  queued?: boolean;
}) {
  const usesTurnTrace =
    liveStatus != null || message.turnDurationMs != null || Boolean(message.trace?.length);
  const showTurnTrace =
    message.role === "assistant" &&
    usesTurnTrace &&
    shouldRenderTurnTrace(message.trace ?? [], liveStatus ?? "completed", message.content);

  return (
    <article
      aria-label={queued ? "排队中的下一条指令" : undefined}
      className={cn(
        "min-w-0 rounded-xl border border-transparent bg-surface-translucent px-3 py-2.5 [&+&]:mt-2.5",
        message.role === "user" && "ml-[18px] bg-primary-wash text-agent-strong",
        message.role === "assistant" && "mr-2.5",
        queued && "border-dashed border-primary-border opacity-80",
      )}
    >
      <div className={cn("flex items-center gap-1.5 text-xs font-bold text-ink-muted", message.role === "assistant" && "[&_svg]:text-agent")}>
        {message.role === "assistant" ? <Sparkles size={14} /> : <MessageSquareText size={14} />}
        <span>{message.role === "assistant" ? "橘记 Agent" : "你"}</span>
        {queued ? (
          <span className="inline-flex items-center gap-1 rounded-full border border-primary-border bg-surface px-1.5 py-px text-[11px] font-semibold text-ink-muted">
            <Clock size={11} />
            排队中
          </span>
        ) : null}
      </div>
      {message.mentionedFileIds?.length ? (
        <div className="my-2 flex flex-wrap gap-[5px]" aria-label="本轮 @ 文件">
          {message.mentionedFileIds.map((fileId) => (
            <span
              key={fileId}
              className="max-w-[180px] truncate rounded-full border border-primary-border bg-primary-wash px-[7px] py-[3px] text-[11px] font-bold text-agent-strong"
            >
              {getMentionedFileLabel(fileId, notes, documents)}
            </span>
          ))}
        </div>
      ) : null}
      {showTurnTrace ? (
        <AgentTurnTrace
          durationMs={message.turnDurationMs}
          hasLiveAnswer={Boolean(message.content.trim())}
          status={liveStatus ?? "completed"}
          steps={message.trace ?? []}
        />
      ) : null}
      {message.content ? (
        <MessageMarkdown content={message.content} streaming={liveStatus === "running"} />
      ) : null}
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
function MessageMarkdown({ content, streaming = false }: { content: string; streaming?: boolean }) {
  return (
    <div className={markdownMessageClassName}>
      <ReactMarkdown
        remarkPlugins={markdownRemarkPlugins}
        rehypePlugins={[rehypeSanitize]}
        components={createMarkdownComponents("agent_message", "message")}
      >
        {protectGfmTablePipesInInlineCode(content)}
      </ReactMarkdown>
      {streaming ? (
        <span aria-hidden className="ml-0.5 inline-block h-[0.85em] w-[2px] animate-pulse bg-agent align-text-bottom" />
      ) : null}
    </div>
  );
}
