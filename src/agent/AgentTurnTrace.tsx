import {
  BookOpen,
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
  Wrench,
  XCircle,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { cn } from "../shared/cn";
import type { AgentTraceStep } from "../shared/types";
import {
  buildToolTraceDetails,
  formatTurnDuration,
  getToolKindLabel,
  getToolTraceLabel,
  type TraceDetailField,
} from "./agentTrace";

/** 过程区展示状态：执行中展开，完成后默认折叠，失败保持展开。 */
export type AgentTurnTraceStatus = "running" | "completed" | "failed";

/** 正文预览超过该长度或行数才显示展开，避免短字段也多一个按钮。 */
const EXCERPT_COLLAPSE_CHARS = 140;
const EXCERPT_COLLAPSE_LINES = 5;

/** 稿纸风格过程区：思考与工具沿时间线交错，展开后显示结构化字段而不是 JSON 黑盒。 */
export function AgentTurnTrace({
  steps,
  durationMs,
  status,
}: {
  steps: AgentTraceStep[];
  durationMs?: number;
  status: AgentTurnTraceStatus;
}) {
  const hasFailedStep = useMemo(
    () => steps.some((step) => step.type === "tool" && step.status === "failed"),
    [steps],
  );
  const resolvedStatus: AgentTurnTraceStatus = hasFailedStep && status === "completed" ? "failed" : status;
  const [isExpanded, setIsExpanded] = useState(resolvedStatus !== "completed");
  const [elapsedMs, setElapsedMs] = useState(durationMs ?? 0);
  const toolCount = useMemo(() => steps.filter((step) => step.type === "tool").length, [steps]);

  useEffect(() => {
    // 完成后强制收起，对齐「过程」默认折叠；失败和运行中保持展开。
    setIsExpanded(resolvedStatus !== "completed");
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
    resolvedStatus === "running" ? "正在处理" : resolvedStatus === "failed" ? "处理失败" : "过程";
  const metaParts = [
    resolvedStatus === "running" ? durationLabel : `已处理 ${durationLabel}`,
    toolCount > 0 ? `${toolCount} 步` : null,
  ].filter(Boolean);

  return (
    <div className="my-1.5 mb-3 grid min-w-0 gap-2" aria-label="Agent 执行过程">
      <button
        className="flex min-w-0 cursor-pointer items-center gap-1.5 border-0 bg-transparent px-0 py-0.5 text-left text-ink-muted"
        type="button"
        aria-expanded={isExpanded}
        onClick={() => setIsExpanded((current) => !current)}
      >
        <ToggleIcon size={13} />
        <span
          className={cn(
            "min-w-0 text-xs font-bold text-ink",
            resolvedStatus === "running" && "text-agent-strong",
            resolvedStatus === "failed" && "text-danger",
          )}
        >
          {title}
        </span>
        <span className="min-w-0 text-[11px] font-medium text-ink-soft">{metaParts.join(" · ")}</span>
        {resolvedStatus === "running" && <Loader2 className="animate-spin" size={13} />}
      </button>

      {isExpanded && (
        <div className="ml-1.5 grid min-w-0 gap-2.5 border-l-[1.5px] border-border pl-3">
          {steps.length === 0 && resolvedStatus === "running" ? (
            <p className="m-0 text-[12.5px] leading-[1.65] text-ink-muted italic whitespace-pre-wrap [overflow-wrap:anywhere]">
              正在等待模型开始本轮推理…
            </p>
          ) : (
            steps.map((step) =>
              step.type === "thinking" ? (
                <p className="m-0 text-[12.5px] leading-[1.65] text-ink-muted italic whitespace-pre-wrap [overflow-wrap:anywhere]" key={step.id}>
                  {step.content}
                </p>
              ) : (
                <TraceToolStep key={step.id} step={step} />
              ),
            )
          )}
        </div>
      )}
    </div>
  );
}

/** 单个工具步骤：收起显示友好摘要，展开显示结构化参数和结果。 */
function TraceToolStep({ step }: { step: AgentTraceStep }) {
  const isFailed = step.status === "failed";
  const isRunning = step.status === "running";
  const [isExpanded, setIsExpanded] = useState(isFailed || isRunning);
  const details = useMemo(() => buildToolTraceDetails(step), [step]);

  useEffect(() => {
    if (isFailed || isRunning) {
      setIsExpanded(true);
    } else {
      setIsExpanded(false);
    }
  }, [isFailed, isRunning]);

  const ToggleIcon = isExpanded ? ChevronDown : ChevronRight;
  const Icon = resolveToolIcon(step);
  const hasDetails = details.hasDetails;
  const kindLabel = details.kindLabel || getToolKindLabel(step.name);

  return (
    <div
      className={cn(
        "grid min-w-0 overflow-hidden rounded-xl border border-border bg-surface",
        isFailed && "border-[rgba(var(--danger-rgb),0.26)] bg-danger-soft",
        isRunning && "border-[rgba(var(--warning-rgb),0.28)] bg-warning-soft",
      )}
    >
      <button
        className={cn(
          "grid min-w-0 grid-cols-[auto_auto_minmax(0,1fr)] items-start gap-[7px] border-0 bg-transparent px-2.5 py-2 text-left text-ink",
          hasDetails ? "cursor-pointer hover:bg-surface-hover" : "cursor-default",
          isFailed && "text-danger",
        )}
        type="button"
        aria-expanded={isExpanded}
        onClick={() => hasDetails && setIsExpanded((current) => !current)}
      >
        {hasDetails ? <ToggleIcon size={13} /> : <span className="w-[13px]" />}
        <span
          className={cn(
            "grid size-[22px] place-items-center rounded-full bg-surface-muted text-agent",
            isRunning && "bg-white/70 text-warning",
            isFailed && "bg-white/70 text-danger",
          )}
          aria-hidden="true"
        >
          <Icon className={isRunning ? "animate-spin" : undefined} size={13} />
        </span>
        <span className="grid min-w-0 gap-1 pt-px">
          <span className="min-w-0 text-[12.5px] font-[650] leading-[1.45] [overflow-wrap:anywhere]">{getToolTraceLabel(step)}</span>
          <span className="inline-flex flex-wrap items-center justify-start gap-1.5">
            <em className="inline-flex items-center rounded-full bg-surface-muted px-[7px] py-0.5 text-[10px] font-bold not-italic tracking-[0.02em] text-ink-muted">
              {kindLabel}
            </em>
            {typeof step.durationMs === "number" && step.durationMs > 0 && (
              <span className="text-[10px] text-ink-soft tabular-nums">{formatTurnDuration(step.durationMs)}</span>
            )}
          </span>
        </span>
      </button>
      {isExpanded && hasDetails && (
        <div className="grid min-w-0 gap-2.5 border-t border-border bg-surface-warm px-3 py-2.5 pb-3">
          {step.error && <p className="m-0 rounded-lg bg-white/70 px-[9px] py-[7px] text-xs leading-normal text-danger">{step.error}</p>}
          <TraceToolFields fields={details.fields} />
        </div>
      )}
    </div>
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
        <dl className="m-0 grid gap-1.5">
          {meta.map((field) => (
            <div className="grid min-w-0 grid-cols-[52px_minmax(0,1fr)] items-start gap-2" key={field.key}>
              <dt className="text-[11px] font-bold text-ink-soft">{field.label}</dt>
              <dd className="m-0 min-w-0 text-[12.5px] leading-[1.45] text-ink [overflow-wrap:anywhere]">
                {field.text}
                {field.truncated && <TraceTruncatedBadge />}
              </dd>
            </div>
          ))}
        </dl>
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
            <dl className="m-0 grid gap-1.5">
              {tech.map((field) => (
                <div className="grid min-w-0 grid-cols-[64px_minmax(0,1fr)] items-start gap-2" key={field.key}>
                  <dt className="pt-px text-[11px] font-bold text-ink-soft">{field.label}</dt>
                  <dd className="m-0 min-w-0">
                    <code className="block font-mono text-[11px] leading-[1.45] text-ink-muted [overflow-wrap:anywhere]">{field.text}</code>
                  </dd>
                </div>
              ))}
            </dl>
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

/** 按工具名选择图标，让检索、写入和 Skill 在时间线里一眼可分辨。 */
function resolveToolIcon(step: AgentTraceStep) {
  if (step.status === "running") {
    return Loader2;
  }

  if (step.status === "failed") {
    return XCircle;
  }

  if (step.name === "search_notes" || step.name === "search_session_messages") {
    return Search;
  }

  if (step.name === "run_skill") {
    return Sparkles;
  }

  if (step.name === "create_file_draft") {
    return FilePlus;
  }

  if (step.name === "propose_file_change") {
    return PencilLine;
  }

  if (step.name === "create_folder") {
    return FolderPlus;
  }

  if (step.name === "list_tree" || step.name === "list_path") {
    return ListTree;
  }

  if (step.name === "read_document") {
    return BookOpen;
  }

  if (step.name === "read_file" || step.name === "read_path" || step.name === "get_current_file") {
    return FileText;
  }

  if (step.status === "completed") {
    return CheckCircle2;
  }

  return Wrench;
}
