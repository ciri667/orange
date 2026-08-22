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
    <div className="agent-turn-trace" aria-label="Agent 执行过程">
      <button
        className={`agent-turn-trace-toggle ${resolvedStatus}`}
        type="button"
        aria-expanded={isExpanded}
        onClick={() => setIsExpanded((current) => !current)}
      >
        <ToggleIcon size={13} />
        <span className="agent-turn-trace-title">{title}</span>
        <span className="agent-turn-trace-meta">{metaParts.join(" · ")}</span>
        {resolvedStatus === "running" && <Loader2 className="agent-turn-trace-spinner" size={13} />}
      </button>

      {isExpanded && (
        <div className="agent-turn-trace-steps">
          {steps.length === 0 && resolvedStatus === "running" ? (
            <p className="agent-turn-thinking">正在等待模型开始本轮推理…</p>
          ) : (
            steps.map((step) =>
              step.type === "thinking" ? (
                <p className="agent-turn-thinking" key={step.id}>
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
    <div className={`agent-turn-tool ${step.status ?? "completed"} ${isExpanded && hasDetails ? "is-open" : ""}`.trim()}>
      <button
        className={`agent-turn-tool-toggle ${hasDetails ? "" : "is-static"}`.trim()}
        type="button"
        aria-expanded={isExpanded}
        onClick={() => hasDetails && setIsExpanded((current) => !current)}
      >
        {hasDetails ? <ToggleIcon size={13} /> : <span className="agent-turn-tool-spacer" />}
        <span className="agent-turn-tool-icon" aria-hidden="true">
          <Icon className={isRunning ? "agent-turn-spin" : undefined} size={13} />
        </span>
        <span className="agent-turn-tool-copy">
          <span className="agent-turn-tool-label">{getToolTraceLabel(step)}</span>
          <span className="agent-turn-tool-aside">
            <em className="agent-turn-tool-kind">{kindLabel}</em>
            {typeof step.durationMs === "number" && step.durationMs > 0 && (
              <span className="agent-turn-tool-duration">{formatTurnDuration(step.durationMs)}</span>
            )}
          </span>
        </span>
      </button>
      {isExpanded && hasDetails && (
        <div className="agent-turn-tool-details">
          {step.error && <p className="agent-turn-error">{step.error}</p>}
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
        <dl className="agent-turn-meta">
          {meta.map((field) => (
            <div className="agent-turn-meta-row" key={field.key}>
              <dt>{field.label}</dt>
              <dd>
                {field.text}
                {field.truncated && <em className="agent-turn-truncated">已截断</em>}
              </dd>
            </div>
          ))}
        </dl>
      )}
      {lists.map((field) => (
        <section className="agent-turn-list" key={field.key}>
          <strong>
            {field.label}
            {field.truncated && <em className="agent-turn-truncated">已截断</em>}
          </strong>
          <ul>
            {field.items?.map((item, index) => (
              <li key={`${field.key}-${index}`}>{item}</li>
            ))}
          </ul>
        </section>
      ))}
      {tech.length > 0 && (
        <div className="agent-turn-tech">
          <button
            className="agent-turn-tech-toggle"
            type="button"
            aria-expanded={techOpen}
            onClick={() => setTechOpen((current) => !current)}
          >
            {techOpen ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
            技术细节
          </button>
          {techOpen && (
            <dl className="agent-turn-meta is-tech">
              {tech.map((field) => (
                <div className="agent-turn-meta-row" key={field.key}>
                  <dt>{field.label}</dt>
                  <dd>
                    <code>{field.text}</code>
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
    <section className="agent-turn-excerpt-block">
      <div className="agent-turn-excerpt-head">
        <strong>{field.label}</strong>
        {field.truncated && <em className="agent-turn-truncated">已截断</em>}
        {canExpand && (
          <button className="agent-turn-excerpt-toggle" type="button" onClick={() => setIsOpen((current) => !current)}>
            {isOpen ? "收起" : "展开"}
          </button>
        )}
      </div>
      <pre className={`agent-turn-excerpt ${isOpen || !canExpand ? "is-open" : ""}`}>{field.text}</pre>
    </section>
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
