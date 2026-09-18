import {
  BookOpen,
  Bot,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  FilePlus,
  FileText,
  FolderPlus,
  ListTree,
  Loader2,
  PencilLine,
  Search,
  Sparkles,
  Square,
  Wrench,
  XCircle,
} from "lucide-react";
import { Fragment, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { cn } from "../shared/cn";
import type { AgentTraceStep } from "../shared/types";
import {
  buildToolTraceDetails,
  canonicalToolName,
  formatTurnDuration,
  getToolKindLabel,
  getToolTraceLabel,
  isTraceTextStep,
  nextTurnTraceExpanded,
  shouldExpandToolStep,
  shouldShowThinkingCaret,
  type TraceDetailField,
} from "./agentTrace";

/** 过程区展示状态：执行中展开，完成后默认折叠，失败和中断保持展开。 */
export type AgentTurnTraceStatus = "running" | "completed" | "failed" | "interrupted";

/** 正文预览超过该长度或行数才显示展开，避免短字段也多一个按钮。 */
const EXCERPT_COLLAPSE_CHARS = 140;
const EXCERPT_COLLAPSE_LINES = 5;

/** 稿纸风格过程区：思考与工具沿时间线交错，展开后显示结构化字段而不是 JSON 黑盒。 */
export function AgentTurnTrace({
  steps,
  durationMs,
  status,
  hasLiveAnswer = false,
}: {
  steps: AgentTraceStep[];
  durationMs?: number;
  status: AgentTurnTraceStatus;
  /** 回答区已经有终稿文本时，思考不再带光标，末尾完成工具也不再当「刚完成」摊开。 */
  hasLiveAnswer?: boolean;
}) {
  const hasFailedStep = useMemo(
    () => steps.some((step) => step.type === "tool" && step.status === "failed"),
    [steps],
  );
  const hasAbortedStep = useMemo(
    () => steps.some((step) => step.type === "tool" && step.status === "aborted"),
    [steps],
  );
  const resolvedStatus: AgentTurnTraceStatus =
    status === "interrupted" || (hasAbortedStep && status === "completed")
      ? "interrupted"
      : hasFailedStep && status === "completed"
        ? "failed"
        : status;
  const [isExpanded, setIsExpanded] = useState(resolvedStatus !== "completed");
  const [elapsedMs, setElapsedMs] = useState(durationMs ?? 0);
  const toolCount = useMemo(() => steps.filter((step) => step.type === "tool").length, [steps]);
  const previousStatusRef = useRef(resolvedStatus);

  useEffect(() => {
    const previousStatus = previousStatusRef.current;
    previousStatusRef.current = resolvedStatus;
    setIsExpanded((current) => nextTurnTraceExpanded(previousStatus, resolvedStatus, current));
  }, [resolvedStatus]);

  useEffect(() => {
    if (resolvedStatus !== "running") {
      setElapsedMs(durationMs ?? 0);
      return;
    }

    const startedAt = Date.now() - (durationMs ?? 0);
    const tick = () => setElapsedMs(Date.now() - startedAt);

    tick();
    const timer = window.setInterval(tick, 1000);
    return () => window.clearInterval(timer);
  }, [durationMs, resolvedStatus]);

  if (!steps.length && resolvedStatus === "completed") {
    return null;
  }

  const ToggleIcon = isExpanded ? ChevronDown : ChevronRight;
  const durationLabel = formatTurnDuration(elapsedMs);
  const title =
    resolvedStatus === "running"
      ? "正在处理"
      : resolvedStatus === "failed"
        ? "处理失败"
        : resolvedStatus === "interrupted"
          ? "已停止"
          : "过程";
  const metaParts = [
    resolvedStatus === "running" ? durationLabel : `已处理 ${durationLabel}`,
    toolCount > 0 ? `${toolCount} 步` : null,
  ].filter(Boolean);

  return (
    <div className="my-1.5 mb-3 grid min-w-0 gap-2" aria-label="Agent 执行过程">
      <button
        className="flex min-w-0 cursor-pointer items-center gap-1.5 border-0 bg-transparent px-0 py-0.5 text-left text-ink-soft"
        type="button"
        aria-expanded={isExpanded}
        onClick={() => setIsExpanded((current) => !current)}
      >
        <ToggleIcon size={13} />
        <span
          className={cn(
            "min-w-0 text-xs font-medium text-ink-muted",
            resolvedStatus === "running" && "text-agent-strong",
            resolvedStatus === "failed" && "text-danger",
            resolvedStatus === "interrupted" && "text-warning",
          )}
        >
          {title}
        </span>
        <span className="min-w-0 text-[11px] font-medium text-ink-soft">{metaParts.join(" · ")}</span>
        {resolvedStatus === "running" && <Loader2 className="animate-spin" size={13} />}
      </button>

      {isExpanded && (
        <div className="ml-1.5 grid min-w-0 gap-1.5 border-l border-border pl-3">
          {steps.length === 0 && resolvedStatus === "running" ? (
            <p className="m-0 text-[12.5px] leading-[1.65] text-ink-muted italic whitespace-pre-wrap [overflow-wrap:anywhere]">
              正在思考…
              <TraceStreamingCaret />
            </p>
          ) : (
            steps.map((step, index) =>
              isTraceTextStep(step) ? (
                <TraceTextStep
                  key={step.id}
                  showCaret={shouldShowThinkingCaret(steps, index, resolvedStatus, hasLiveAnswer)}
                  step={step}
                />
              ) : (
                <TraceToolStep
                  autoExpand={shouldExpandToolStep(steps, index, resolvedStatus, hasLiveAnswer)}
                  key={step.id}
                  step={step}
                />
              ),
            )
          )}
        </div>
      )}
    </div>
  );
}

/** 思考用斜体弱化；工具前旁白用正文色，对应模型可见 content。 */
function TraceTextStep({
  step,
  showCaret,
}: {
  step: AgentTraceStep;
  showCaret: boolean;
}) {
  const isNarration = step.type === "narration";
  return (
    <p
      className={cn(
        "m-0 text-[12.5px] leading-[1.65] whitespace-pre-wrap [overflow-wrap:anywhere]",
        isNarration ? "text-ink" : "text-ink-muted italic",
      )}
    >
      {step.content}
      {showCaret ? <TraceStreamingCaret /> : null}
    </p>
  );
}

/** 单个工具步骤：收起显示友好摘要，展开显示结构化参数和结果。 */
function TraceToolStep({
  step,
  autoExpand,
}: {
  step: AgentTraceStep;
  /** 由时间线位置和回合状态决定的默认摊开；角色变化时清掉用户覆盖。 */
  autoExpand: boolean;
}) {
  const isFailed = step.status === "failed";
  const isAborted = step.status === "aborted";
  const isRunning = step.status === "running";
  const [userOverride, setUserOverride] = useState<boolean | null>(null);
  const [previousAutoExpand, setPreviousAutoExpand] = useState(autoExpand);
  if (previousAutoExpand !== autoExpand) {
    setPreviousAutoExpand(autoExpand);
    setUserOverride(null);
  }
  const isExpanded = userOverride ?? autoExpand;
  const details = useMemo(() => buildToolTraceDetails(step), [step]);

  const ToggleIcon = isExpanded ? ChevronDown : ChevronRight;
  const Icon = resolveToolIcon(step);
  const hasDetails = details.hasDetails;
  const kindLabel = details.kindLabel || getToolKindLabel(step.name);

  return (
    <div className="grid min-w-0">
      <button
        className={cn(
          "grid min-w-0 grid-cols-[auto_auto_minmax(0,1fr)] items-start gap-2 border-0 bg-transparent px-0 py-1 text-left text-ink-muted",
          hasDetails ? "cursor-pointer hover:text-ink" : "cursor-default",
          isFailed && "text-danger",
          isAborted && "text-warning",
        )}
        type="button"
        aria-expanded={isExpanded}
        onClick={() => hasDetails && setUserOverride((current) => !(current ?? autoExpand))}
      >
        {hasDetails ? <ToggleIcon size={13} /> : <span className="w-[13px]" />}
        <span
          className={cn(
            "grid size-[18px] place-items-center text-ink-soft",
            isRunning && "text-warning",
            isFailed && "text-danger",
            isAborted && "text-warning",
          )}
          aria-hidden="true"
        >
          <Icon className={isRunning ? "animate-spin" : undefined} size={13} />
        </span>
        <span className="grid min-w-0 gap-1 pt-px">
          <span className="min-w-0 text-[12.5px] font-medium leading-[1.45] [overflow-wrap:anywhere]">{getToolTraceLabel(step)}</span>
          <span className="inline-flex flex-wrap items-center justify-start gap-1.5">
            <em className="text-[10px] not-italic text-ink-soft">
              {kindLabel}
            </em>
            {typeof step.durationMs === "number" && step.durationMs > 0 && (
              <span className="text-[10px] text-ink-soft tabular-nums">{formatTurnDuration(step.durationMs)}</span>
            )}
          </span>
        </span>
      </button>
      {isExpanded && hasDetails && (
        <div className="grid min-w-0 gap-2 px-0 pb-2 pl-7">
          {step.error && <p className="m-0 rounded-lg bg-white/70 px-[9px] py-[7px] text-xs leading-normal text-danger">{step.error}</p>}
          <TraceToolFields fields={details.fields} />
          {(step.children?.length ?? 0) > 0 && (
            <div className="grid min-w-0 gap-2 border-l-[1.5px] border-border pl-2.5" aria-label="子 Agent 过程">
              {step.children?.map((child, index) =>
                isTraceTextStep(child) ? (
                  <TraceTextStep
                    key={child.id}
                    showCaret={shouldShowThinkingCaret(
                      step.children ?? [],
                      index,
                      isRunning ? "running" : "completed",
                      false,
                    )}
                    step={child}
                  />
                ) : (
                  <TraceToolStep
                    autoExpand={shouldExpandToolStep(
                      step.children ?? [],
                      index,
                      isRunning ? "running" : "completed",
                      false,
                    )}
                    key={child.id}
                    step={child}
                  />
                ),
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/** 标签列随内容变宽并封顶，长英文键在列内换行，不挤占右侧取值。 */
function TraceKeyValueFields({
  fields,
  renderValue,
}: {
  fields: TraceDetailField[];
  renderValue: (field: TraceDetailField) => ReactNode;
}) {
  return (
    <dl className="m-0 grid min-w-0 grid-cols-[fit-content(8rem)_minmax(0,1fr)] items-start gap-x-3 gap-y-1.5">
      {fields.map((field) => (
        <Fragment key={field.key}>
          <dt className="min-w-0 pt-px text-[11px] font-bold leading-[1.45] text-ink-soft [overflow-wrap:anywhere]">
            {field.label}
          </dt>
          <dd className="m-0 min-w-0">{renderValue(field)}</dd>
        </Fragment>
      ))}
    </dl>
  );
}

/** 把结构化字段分成元信息、列表、正文和技术细节，避免再次堆出两块 JSON。 */
function TraceToolFields({ fields }: { fields: TraceDetailField[] }) {
  const meta = fields.filter((field) => field.kind === "meta");
  const lists = fields.filter((field) => field.kind === "list");
  const bodies = fields.filter((field) => field.kind === "body");
  const tech = fields.filter((field) => field.kind === "tech");
  const [techOpen, setTechOpen] = useState(false);

  if (!fields.length) {
    return null;
  }

  return (
    <>
      {meta.length > 0 && (
        <TraceKeyValueFields
          fields={meta}
          renderValue={(field) => (
            <span className="text-[12.5px] leading-[1.45] text-ink [overflow-wrap:anywhere]">
              {field.text}
              {field.truncated && <TraceTruncatedBadge />}
            </span>
          )}
        />
      )}
      {lists.map((field) => (
        <section className="grid min-w-0 gap-1.5" key={field.key}>
          <strong className="text-[11px] font-bold text-ink-soft">
            {field.label}
            {field.truncated && <TraceTruncatedBadge />}
          </strong>
          <ul className="m-0 pl-[1.1em] text-[12.5px] leading-normal text-ink [&>li+li]:mt-0.5">
            {field.items?.map((item, index) => (
              <li key={`${field.key}-${index}`}>{item}</li>
            ))}
          </ul>
        </section>
      ))}
      {tech.length > 0 && (
        <div className="grid min-w-0 gap-1.5">
          <button
            className="inline-flex w-max cursor-pointer items-center gap-1 border-0 bg-transparent p-0 text-[11px] font-[650] text-ink-muted hover:text-ink"
            type="button"
            aria-expanded={techOpen}
            onClick={() => setTechOpen((current) => !current)}
          >
            {techOpen ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
            技术细节
          </button>
          {techOpen && (
            <TraceKeyValueFields
              fields={tech}
              renderValue={(field) => (
                <code className="block font-mono text-[11px] leading-[1.45] text-ink-muted [overflow-wrap:anywhere]">{field.text}</code>
              )}
            />
          )}
        </div>
      )}
      {bodies.map((field) => (
        <TraceExcerpt field={field} key={field.key} />
      ))}
    </>
  );
}

/** 长正文用稿纸摘录展示，默认折叠，需要时再展开。 */
function TraceExcerpt({ field }: { field: TraceDetailField }) {
  const canExpand =
    field.truncated || field.text.length > EXCERPT_COLLAPSE_CHARS || field.text.split("\n").length > EXCERPT_COLLAPSE_LINES;
  const [isOpen, setIsOpen] = useState(false);

  return (
    <section className="grid min-w-0 gap-1.5">
      <div className="flex items-center gap-1.5">
        <strong className="text-[11px] font-bold text-ink-soft">{field.label}</strong>
        {field.truncated && <TraceTruncatedBadge />}
        {canExpand && (
          <button
            className="ml-auto inline-flex w-max cursor-pointer items-center gap-1 border-0 bg-transparent p-0 text-[11px] font-[650] text-ink-muted hover:text-ink"
            type="button"
            onClick={() => setIsOpen((current) => !current)}
          >
            {isOpen ? "收起" : "展开"}
          </button>
        )}
      </div>
      <pre
        className={cn(
          "m-0 max-h-[8.4em] overflow-hidden rounded-r-lg border-l-[3px] border-accent bg-surface px-2.5 py-2 font-inherit text-[12.5px] leading-[1.6] text-ink whitespace-pre-wrap [overflow-wrap:anywhere]",
          (isOpen || !canExpand) && "max-h-[22em] overflow-auto",
        )}
      >
        {field.text}
      </pre>
    </section>
  );
}

/** 截断标记，避免长字段把过程区撑开。 */
function TraceTruncatedBadge() {
  return (
    <em className="ml-1.5 inline-flex align-middle rounded-full bg-warning-soft px-1.5 py-px text-[10px] font-bold not-italic text-warning">
      已截断
    </em>
  );
}

/** 与回答区相同的脉冲光标，过程区 running 时只允许出现一根。 */
function TraceStreamingCaret() {
  return (
    <span
      aria-hidden
      className="ml-0.5 inline-block h-[0.85em] w-[2px] animate-pulse bg-agent align-text-bottom"
    />
  );
}

/** 按工具名选择图标，让检索、写入和 Skill 在时间线里一眼可分辨。 */
function resolveToolIcon(step: AgentTraceStep) {
  if (step.status === "running") {
    return Loader2;
  }

  if (step.status === "failed") {
    return XCircle;
  }

  if (step.status === "aborted") {
    return Square;
  }

  const toolName = canonicalToolName(step.name);

  if (toolName === "task") {
    return Bot;
  }

  if (toolName === "search" || step.name === "search_session_messages") {
    return Search;
  }

  if (toolName === "run") {
    return Sparkles;
  }

  if (toolName === "write") {
    return FilePlus;
  }

  if (toolName === "edit") {
    return PencilLine;
  }

  if (step.name === "create_folder") {
    return FolderPlus;
  }

  if (toolName === "list" || step.name === "list_path") {
    return ListTree;
  }

  if (step.name === "read_document") {
    return BookOpen;
  }

  if (toolName === "read" || step.name === "read_path") {
    return FileText;
  }

  if (step.status === "completed") {
    return CheckCircle2;
  }

  return Wrench;
}
