import { Check, ChevronDown, Clock, Copy, Database, FolderOpen, Gauge, Layers3, ListChecks, Pencil, Sparkles, X } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import rehypeSanitize from "rehype-sanitize";
import { Button } from "../shared/Button";
import { ImageLightbox } from "../shared/ImageLightbox";
import { copyTextToClipboard } from "../shared/clipboard";
import { assistantReplyLooksLikePlan } from "../shared/planMode";
import { PlanMessageCard } from "./PlanMessageCard";
import { VoiceDictationButton } from "../shared/VoiceDictationButton";
import { Checkbox } from "../shared/Checkbox";
import { Chip } from "../shared/Chip";
import { cn } from "../shared/cn";
import { listRowClassName } from "../shared/ListRow";
import { logWarn } from "../shared/logger";
import {
  createMarkdownComponents,
  markdownMessageClassName,
  markdownRemarkPlugins,
  protectGfmTablePipesInInlineCode,
} from "../shared/markdown";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import { Menu, MenuItem, MenuPanel } from "../shared/Menu";
import { agentPopoverClassName, fieldTextareaClassName, popoverHeaderClassName, sectionLabelClassName } from "../shared/ui";
import {
  getScopeSummaryLabel,
  getSessionKnowledgeBaseLabel,
  getSessionRecoveryNoteLabel,
} from "../shared/selectors";
import { conversationImageSrc } from "./conversationImages";
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
import { sessionHasPendingWriteConflict } from "../workspace/sessionUtils";
import type {
  AgentContextCompactionReport,
  AgentMessage,
  AgentPromptDump,
  AgentSecuritySettings,
  AgentSession,
  AgentTurnProgressEvent,
  Citation,
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
import { AgentConversationStart } from "./AgentConversationStart";

/** 会话摘要条只保留上下文占用和待确认写入。知识库范围由旁边的范围入口展示，文件不在这里绑定。 */
export function AgentSessionSummary({
  activeSession,
  sessions,
  modelConfig,
}: {
  activeSession: AgentSession;
  sessions: AgentSession[];
  modelConfig: ModelConfig;
}) {
  const isPendingWrite = activeSession.pendingChange?.status === "pending" || activeSession.pendingChangeSet?.status === "pending";
  const hasWriteConflict = sessionHasPendingWriteConflict(activeSession, sessions);
  const contextMeter = resolveContextMeter(activeSession, modelConfig);
  const contextMeterLabel = formatContextMeterChip(contextMeter);

  const contextMeterTitle = `窗口 ${formatContextWindowLabel(contextMeter)} · 占用 ${formatContextUsageLabel(contextMeter)}`;

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-nowrap items-center gap-1.5 overflow-hidden" aria-label="当前会话摘要">
      {contextMeterLabel && (
        <span
          className="inline-flex shrink-0"
          title={contextMeterTitle}
          aria-label={contextMeterTitle}
        >
          {/* 占用标签很短，按内容单行展示。外层若是 shrink-to-fit，子级 max-w-[n%] 会把胶囊压成竖排。 */}
          <Chip className="max-w-none font-normal">
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
          text={hasWriteConflict ? "与其它会话改同一文件" : "待确认 diff"}
          logArea="agent_session_write_status"
        />
      )}
    </div>
  );
}

/** 四档权限同时定义操作范围与审批方式。 */
export const AGENT_SECURITY_LEVEL_COPY = {
  "workspace-readonly": { label: "工作区内只读", description: "读取和搜索会话授权知识库及已启用 Skill 资源，禁止生成或应用文件变更。" },
  "workspace-write": { label: "工作区内修改，需审批", description: "可提出工作区文件变更，审阅确认后应用。" },
  "full-confirm": { label: "完全权限，需审批", description: "可访问工作区外资源及执行真实命令；外部读取和操作逐次审批。" },
  "full-auto": { label: "完全权限，无需审批", description: "在当前系统用户权限内自动执行真实环境操作；仍校验冲突与功能设置。" },
} as const;

/** 会话权限下拉；进入完全档由会话动作统一请求确认。 */
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
  const [isOpen, setIsOpen] = useState(false);

  if (!agentSecurity) {
    return null;
  }

  const selectedCopy = AGENT_SECURITY_LEVEL_COPY[activeSession.securityLevel];
  const levels = [
    ["workspace-readonly", true],
    ["workspace-write", true],
    ["full-confirm", true],
    ["full-auto", true],
  ] as const;

  return (
    <Menu className="shrink-0" open={isOpen} onClose={() => setIsOpen(false)}>
      <button
        className={cn(
          "inline-flex h-7 items-center gap-1 rounded-control border border-border bg-surface px-2 text-[12px] font-medium text-ink",
          "hover:border-border-strong hover:bg-surface-hover",
          "focus-visible:border-border-strong focus-visible:outline-none",
          "disabled:cursor-not-allowed disabled:opacity-50",
        )}
        type="button"
        aria-haspopup="menu"
        aria-expanded={isOpen}
        aria-label={`当前会话权限：${selectedCopy.label}。${selectedCopy.description}`}
        title={selectedCopy.description}
        disabled={isBusy}
        onClick={() => setIsOpen((open) => !open)}
      >
        <span>{selectedCopy.label}</span>
        <ChevronDown size={13} className={cn("shrink-0 text-ink-muted transition-transform", isOpen && "rotate-180")} />
      </button>
      {isOpen && (
        <MenuPanel
          placement="top-end"
          className="bottom-[calc(100%+6px)] left-0 w-[248px] min-w-0 whitespace-normal"
        >
          {levels.map(([level, isEnabled]) => {
            const copy = AGENT_SECURITY_LEVEL_COPY[level];
            const active = activeSession.securityLevel === level;
            const hint = isEnabled ? copy.description : `${copy.description}选择后将启用此能力。`;

            return (
              <MenuItem
                key={level}
                role="menuitemradio"
                aria-checked={active}
                aria-label={`${copy.label}权限。${hint}`}
                title={hint}
                className="h-auto items-start gap-2 py-1.5 whitespace-normal"
                onClick={() => {
                  setIsOpen(false);
                  onSecurityLevelChange?.(level);
                }}
              >
                <span className="grid min-w-0 flex-1 gap-0.5 text-left">
                  <span className={cn("font-medium", active && "text-accent")}>{copy.label}</span>
                  {/* 短说明均衡换行，避免固定宽度下末行只剩一个汉字。 */}
                  <span className="text-balance text-[11px] leading-snug font-normal text-ink-muted">{hint}</span>
                </span>
                {active ? <Check size={14} className="mt-0.5 shrink-0 text-accent" /> : <span className="size-3.5 shrink-0" aria-hidden="true" />}
              </MenuItem>
            );
          })}
        </MenuPanel>
      )}
    </Menu>
  );
}

/** 规划模式开关：与安全级别正交，决定 Agent 是否先出计划、经用户确认后才执行。 */
export function PlanModeToggle({
  activeSession,
  isBusy,
  onPlanModeChange,
}: {
  activeSession: AgentSession;
  isBusy: boolean;
  onPlanModeChange: (planMode: boolean) => void;
}) {
  const active = activeSession.planMode;

  return (
    <button
      type="button"
      role="switch"
      aria-checked={active}
      aria-label="计划模式"
      title={active ? "Plan 已开启：仅允许工作区只读操作。关闭后恢复原权限，不会提升档位。" : "开启 Plan：工作区只读，禁用外部工具、命令和所有变更应用。"}
      disabled={isBusy}
      className={cn(
        "shrink-0 rounded-full px-1.5 py-0.5 text-[12px] font-medium",
        active
          ? "border border-[rgba(var(--accent-rgb),0.35)] bg-accent-soft text-accent"
          : "border border-transparent bg-transparent text-ink-soft hover:text-ink",
      )}
      onClick={() => onPlanModeChange(!active)}
    >
      计划模式
    </button>
  );
}

/** 规划模式下回合结束后的确认横幅：执行走宿主流程，改计划回到输入框。窄面板时按钮组自动换行，文本保持单行省略。 */
export function PlanReadyBanner({
  onExecutePlan,
  onRefinePlan,
}: {
  onExecutePlan: () => void;
  onRefinePlan: () => void;
}) {
  return (
    <div
      className="flex w-full min-w-0 flex-wrap items-center gap-x-2 gap-y-1 rounded-control border border-[rgba(var(--accent-rgb),0.3)] bg-accent-soft px-2 py-1.5"
      role="status"
      aria-label="计划确认横幅"
    >
      <span className="inline-flex min-w-[8rem] flex-1 items-center gap-1.5 text-xs text-ink">
        <ListChecks size={14} className="shrink-0 text-accent" />
        <OverflowTooltipText className="min-w-0 truncate" text="计划已就绪：确认后开始执行，或直接输入修改意见。" logArea="agent_plan_banner" />
      </span>
      <span className="ml-auto inline-flex shrink-0 items-center gap-1.5">
        <Button variant="ghost" size="compact" onClick={onRefinePlan}>
          继续修改计划
        </Button>
        <Button variant="primary" size="compact" onClick={onExecutePlan}>
          执行计划
        </Button>
      </span>
    </div>
  );
}

/** 会话上下文浮层，集中展示工具范围、恢复锚点和会话默认模型。 */
export function AgentSessionContextPopover({
  activeSession,
  knowledgeBases,
  notes,
  modelConfig,
  isBusy,
  onToggleSessionContext,
  onSetSessionModelSelection,
  onCompactAgentContext,
  contextCompaction,
  onOpenProjectInstruction,
}: {
  activeSession: AgentSession;
  knowledgeBases: KnowledgeBase[];
  notes: Note[];
  modelConfig: ModelConfig;
  isBusy: boolean;
  onToggleSessionContext: () => void;
  onSetSessionModelSelection: (selection: string) => void;
  onCompactAgentContext: () => void;
  /** 最近一次手动整理上下文的回执；只在当前会话且刚整理过时展示。 */
  contextCompaction?: AgentContextCompactionReport | null;
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
  /** 只展示当前会话的回执，避免切到别的会话后仍显示上一次整理结果。 */
  const currentCompaction =
    contextCompaction && contextCompaction.sessionId === activeSession.id
      ? contextCompaction
      : null;
  /** 整理进行中：按钮必须立刻禁用，否则用户会以为没点上而反复点击。 */
  const isCompacting = currentCompaction?.status === "running";
  const compactionNotice = currentCompaction?.notice ?? null;

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
          <Button
            variant="ghost"
            size="compact"
            title="整理上下文：把较早的对话压成检查点，只保留最近一段"
            onClick={onCompactAgentContext}
            disabled={isBusy || isCompacting}
          >
            <Sparkles size={14} />
            {isCompacting ? "整理中…" : "整理"}
          </Button>
          <Button variant="icon" title="关闭上下文" onClick={onToggleSessionContext}>
            <X size={15} />
          </Button>
        </div>
      </div>
      {compactionNotice ? (
        <div
          className={cn(
            "mt-2 whitespace-pre-wrap rounded-control border border-border-translucent bg-surface-muted px-2.5 py-1.5 text-xs leading-[1.5] text-ink-muted",
            currentCompaction?.status === "failed" && "border-[rgba(var(--danger-rgb),0.26)] bg-danger-soft text-danger",
          )}
          role="status"
          aria-live="polite"
          aria-label="上下文整理结果"
        >
          {compactionNotice}
        </div>
      ) : null}
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
          占用取最近一次有效模型 usage；窗口来自当前模型目录，Provider 未提供时显示未知。会话只绑定知识库；要让 Agent 读取或修改某个文件，在输入框用 @ 引用。会话恢复笔记只用于恢复旧会话位置。Agent 会按模型窗口装入尽量多的最近对话，更早内容进入工作记忆，也可按需检索会话历史和其他文件。
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
          "inline-flex w-auto max-w-[46%] min-w-0 cursor-pointer items-center gap-1.5 rounded-full border border-border bg-surface px-2 py-1 text-left text-ink",
          (selectedKnowledgeBaseIds.length > 1 || isScopeSelectorOpen) && "bg-surface-muted",
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
  scrollPositions,
  activeSession,
  notes,
  documents,
  liveTurn,
  queuedFollowUp,
  onEditUserMessage,
  readingNote,
  knowledgeBaseName = "当前授权知识库",
  onChoosePrompt,
  onOpenCitation,
}: {
  scrollPositions?: Map<string, { top: number; stickToBottom: boolean }>;
  activeSession: AgentSession;
  notes: Note[];
  documents: WorkspaceDocument[];
  liveTurn?: AgentTurnProgressEvent | null;
  /** 当前回合结束后才会发给模型的下一条用户指令。 */
  queuedFollowUp?: string | null;
  /** 编辑已发送的用户消息并重跑；IM 会话不提供。 */
  onEditUserMessage?: (messageId: string, prompt: string) => void;
  readingNote?: Note;
  knowledgeBaseName?: string;
  onChoosePrompt?: (prompt: string, noteId?: string) => void;
  /** 点击引用打开文件并定位。 */
  onOpenCitation?: (citation: Citation) => void;
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

  useLayoutEffect(() => {
    const list = listRef.current;
    const saved = scrollPositions?.get(activeSession.id);
    if (list && saved) { list.scrollTop = saved.top; stickToBottomRef.current = saved.stickToBottom; }
    return () => {
      if (list) scrollPositions?.set(activeSession.id, { top: list.scrollTop, stickToBottom: stickToBottomRef.current });
    };
  }, [activeSession.id, scrollPositions]);

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
      className="agent-message-list min-h-0 flex-1 overflow-auto px-4 py-4"
      aria-live="polite"
      ref={listRef}
      onScroll={(event) => {
        const list = event.currentTarget;
        stickToBottomRef.current = isNearScrollBottom(list.scrollTop, list.scrollHeight, list.clientHeight);
        scrollPositions?.set(activeSession.id, { top: list.scrollTop, stickToBottom: stickToBottomRef.current });
      }}
    >
      {activeSession.messages.length === 0 && !showLiveTurn && !queuedFollowUp && (
        <AgentConversationStart note={readingNote} knowledgeBaseName={knowledgeBaseName} onChoosePrompt={onChoosePrompt ?? (() => {})} />
      )}
      {activeSession.messages.map((message, index) => (
        <AgentMessageItem
          canEdit={Boolean(onEditUserMessage) && !activeSession.imIdentity && message.role === "user"}
          documents={documents}
          key={message.id}
          message={message}
          notes={notes}
          onEditUserMessage={onEditUserMessage}
          onOpenCitation={onOpenCitation}
          planInitiallyExpanded={activeSession.planMode && index === activeSession.messages.length - 1}
        />
      ))}
      {showLiveTurn && liveTurn && (
        <AgentMessageItem
          documents={documents}
          liveStatus={liveTurn.status}
          liveStartedAtMs={liveTurn.startedAtMs}
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
  liveStartedAtMs,
  queued = false,
  canEdit = false,
  onEditUserMessage,
  onOpenCitation,
  planInitiallyExpanded = false,
}: {
  message: AgentMessage;
  notes: Note[];
  documents: WorkspaceDocument[];
  liveStatus?: AgentTurnProgressEvent["status"];
  /** live 气泡的本轮绝对开始时间；落盘消息没有它，直接用最终耗时。 */
  liveStartedAtMs?: number;
  /** 尚未进入模型的排队用户消息，只存在于当前界面。 */
  queued?: boolean;
  canEdit?: boolean;
  onEditUserMessage?: (messageId: string, prompt: string) => void;
  /** 点击资料库引用时打开文件并跳到页码/结构块。 */
  onOpenCitation?: (citation: Citation) => void;
  /** 仍在规划模式里、且是最新一条时，计划卡片默认展开，方便确认前通读。 */
  planInitiallyExpanded?: boolean;
}) {
  const [isEditing, setIsEditing] = useState(false);
  /** 点击消息里的图片后打开的大图。 */
  const [previewImage, setPreviewImage] = useState<{ src: string; alt: string } | null>(null);
  const usesTurnTrace =
    liveStatus != null || message.turnDurationMs != null || Boolean(message.trace?.length);
  const showTurnTrace =
    message.role === "assistant" &&
    usesTurnTrace &&
    shouldRenderTurnTrace(
      message.trace ?? [],
      liveStatus ?? (message.interrupted ? "interrupted" : "completed"),
      message.content,
    );

  const statusChip = queued ? (
    <span className="inline-flex items-center gap-1 text-[11px] font-medium text-ink-soft">
      <Clock size={11} />
      排队中
    </span>
  ) : message.interrupted || liveStatus === "interrupted" ? (
    <span className="inline-flex items-center text-[11px] font-medium text-warning">已停止</span>
  ) : null;

  const mentionedFiles = message.mentionedFileIds?.length ? (
    <div className="mb-2 flex flex-wrap gap-1" aria-label="本轮 @ 文件">
      {message.mentionedFileIds.map((fileId) => (
        <span
          key={fileId}
          className="max-w-[180px] truncate rounded-full border border-border px-2 py-[3px] text-[11px] font-medium text-ink-muted"
        >
          {getMentionedFileLabel(fileId, notes, documents)}
        </span>
      ))}
    </div>
  ) : null;

  const imageBlock = message.images?.length ? (
    <div className="mb-2 flex flex-wrap gap-1.5" aria-label="本条消息的图片">
      {message.images.map((image) => {
        const src = conversationImageSrc(image);
        const alt = image.name || "图片";
        return src ? (
          <button
            key={image.id}
            type="button"
            className="inline-flex h-20 max-w-[160px] cursor-zoom-in overflow-hidden rounded-md border border-border bg-transparent p-0"
            aria-label={`放大查看 ${alt}`}
            title="点击放大查看"
            onClick={() => setPreviewImage({ src, alt })}
          >
            <img src={src} alt="" className="h-20 max-w-[160px] object-cover" />
          </button>
        ) : (
          <span
            key={image.id}
            className="inline-flex h-20 min-w-[72px] items-center justify-center rounded-md border border-border bg-surface px-2 text-[11px] text-ink-muted"
          >
            {image.name || "图片"}
          </span>
        );
      })}
    </div>
  ) : null;

  // 流式输出仍走普通 Markdown；落盘且形状符合执行计划后，才换成可折叠卡片。
  const showPlanCard = liveStatus == null && assistantReplyLooksLikePlan(message);

  // 复制 / 编辑按钮独立于正文渲染：用户消息里放在气泡外面，助手消息跟在正文后面。
  const actionButtons =
    !isEditing && !showPlanCard && (message.content.trim() || message.images?.length || (canEdit && !queued)) ? (
      <div className="mt-1.5 flex items-center gap-0.5">
        <MessageCopyButton content={message.content} messageRole={message.role} />
        {canEdit && !queued ? (
          <Button
            variant="icon"
            size="compact"
            className="text-ink-soft opacity-0 group-hover:opacity-100 focus-visible:opacity-100"
            title="编辑并重新执行"
            aria-label="编辑并重新执行"
            onClick={() => setIsEditing(true)}
          >
            <Pencil size={13} />
          </Button>
        ) : null}
      </div>
    ) : null;

  const body = isEditing ? (
    <UserMessageEditor
      initialContent={message.content}
      allowEmpty={Boolean(message.images?.length)}
      onCancel={() => setIsEditing(false)}
      onSubmit={(prompt) => {
        setIsEditing(false);
        onEditUserMessage?.(message.id, prompt);
      }}
    />
  ) : showPlanCard ? (
    <PlanMessageCard content={message.content} initiallyExpanded={planInitiallyExpanded} />
  ) : message.content ? (
    <MessageMarkdown content={message.content} streaming={liveStatus === "running"} />
  ) : null;

  const lightbox = previewImage ? (
    <ImageLightbox src={previewImage.src} alt={previewImage.alt} onClose={() => setPreviewImage(null)} />
  ) : null;

  if (message.role === "user") {
    return (
      <article
        aria-label={queued ? "排队中的下一条指令" : undefined}
        className={cn(
          "group flex min-w-0 flex-col items-end select-text [&+&]:mt-4",
          queued && "opacity-80",
        )}
      >
        <div className="max-w-[90%] rounded-xl border border-border bg-surface px-3.5 py-2.5 text-sm leading-[1.75] text-ink">
          {statusChip ? <div className="mb-1">{statusChip}</div> : null}
          {mentionedFiles}
          {imageBlock}
          {body}
        </div>
        {actionButtons}
        {lightbox}
      </article>
    );
  }

  return (
    <article className="group min-w-0 select-text [&+&]:mt-5">
      {statusChip ? <div className="mb-1">{statusChip}</div> : null}
      {mentionedFiles}
      {imageBlock}
      {showTurnTrace ? (
        <AgentTurnTrace
          durationMs={message.turnDurationMs}
          hasLiveAnswer={Boolean(message.content.trim())}
          startedAtMs={liveStartedAtMs}
          status={liveStatus ?? (message.interrupted ? "interrupted" : "completed")}
          steps={message.trace ?? []}
        />
      ) : null}
      {body}
      {actionButtons}
      {/* 2026-08-22 之前落盘的消息没有 trace / turnDurationMs，仍走扁平「运行信息」。
          旧会话不再需要兼容时，连同 ToolCallList 一起删除。 */}
      {!usesTurnTrace ? <ToolCallList toolCalls={message.toolCalls} /> : null}
      <CitationList citations={message.citations} onOpenCitation={onOpenCitation} />
      {lightbox}
    </article>
  );
}

/** 将历史消息中的 @ 文件 ID 转成安全展示名称；文件被删除后保留可解释的占位。 */
function getMentionedFileLabel(fileId: string, notes: Note[], documents: WorkspaceDocument[]) {
  return notes.find((note) => note.id === fileId)?.title ?? documents.find((document) => document.id === fileId)?.title ?? "已失效文件";
}

/** 复制成功后的短暂反馈时长，避免按钮文案一直停在「已复制」。 */
const MESSAGE_COPY_RESET_MS = 1600;

/** 用户消息气泡内编辑；发送后由会话动作截断后续历史并重跑。 */
function UserMessageEditor({
  initialContent,
  allowEmpty = false,
  onCancel,
  onSubmit,
}: {
  initialContent: string;
  allowEmpty?: boolean;
  onCancel: () => void;
  onSubmit: (prompt: string) => void;
}) {
  const [draft, setDraft] = useState(initialContent);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const trimmed = draft.trim();
  const canSubmit = Boolean(trimmed) || allowEmpty;

  useEffect(() => {
    const textarea = textareaRef.current;
    if (!textarea) {
      return;
    }

    textarea.focus();
    textarea.select();
  }, []);

  return (
    <form
      className="mt-2 grid gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        if (!canSubmit) {
          return;
        }
        onSubmit(trimmed);
      }}
    >
      <textarea
        ref={textareaRef}
        aria-label="编辑用户消息"
        className={cn(fieldTextareaClassName, "min-h-[72px] text-sm")}
        onChange={(event) => setDraft(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            onCancel();
          }
          if (event.key === "Enter" && (event.ctrlKey || event.metaKey) && canSubmit) {
            event.preventDefault();
            onSubmit(trimmed);
          }
        }}
        rows={Math.min(8, Math.max(3, draft.split("\n").length))}
        value={draft}
      />
      <div className="flex justify-end gap-2">
        <VoiceDictationButton getTextarea={() => textareaRef.current} onValueChange={setDraft} />
        <Button type="button" variant="ghost" size="compact" onClick={onCancel}>
          取消
        </Button>
        <Button type="submit" variant="primary" size="compact" disabled={!canSubmit}>
          发送
        </Button>
      </div>
    </form>
  );
}

/** 消息正文下方的复制按钮；写入系统剪贴板的是消息原文，不含引用和过程轨迹。 */
function MessageCopyButton({
  content,
  messageRole,
}: {
  content: string;
  messageRole: AgentMessage["role"];
}) {
  const [copied, setCopied] = useState(false);
  const resetTimerRef = useRef<number | null>(null);

  useEffect(() => {
    return () => {
      if (resetTimerRef.current != null) {
        window.clearTimeout(resetTimerRef.current);
      }
    };
  }, []);

  if (!content.trim()) {
    return null;
  }

  return (
    <Button
      variant="icon"
      size="compact"
      className={cn(
        "border-transparent bg-transparent text-ink-soft",
        copied
          ? "text-success opacity-100"
          : "opacity-[0.58] group-hover:opacity-100 focus-visible:opacity-100 hover:enabled:border-border-strong hover:enabled:bg-surface-hover hover:enabled:text-ink",
      )}
      title={copied ? "已复制" : "复制消息"}
      aria-label={copied ? "已复制" : "复制消息"}
      onClick={() => {
        void copyMessageContent(content, messageRole, () => {
          setCopied(true);
          if (resetTimerRef.current != null) {
            window.clearTimeout(resetTimerRef.current);
          }
          resetTimerRef.current = window.setTimeout(() => setCopied(false), MESSAGE_COPY_RESET_MS);
        });
      }}
    >
      {copied ? <Check size={13} /> : <Copy size={13} />}
    </Button>
  );
}

/** 复制消息原文；失败只记脱敏日志，不把正文写入诊断。 */
async function copyMessageContent(
  content: string,
  messageRole: AgentMessage["role"],
  onCopied: () => void,
) {
  try {
    await copyTextToClipboard(content);
    onCopied();
  } catch (error) {
    logWarn("复制消息失败", {
      category: "frontend",
      event: "copy_agent_message",
      error,
      metadata: {
        role: messageRole,
        contentLength: content.length,
      },
    });
  }
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
