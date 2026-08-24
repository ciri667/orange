import { BrainCircuit, CheckCircle2, ChevronDown, ChevronRight, Search, Sparkles, Wrench } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { cn } from "../shared/cn";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import type { AgentToolCall } from "../shared/types";

/** 工具调用轨迹列表，让用户知道 Agent 本轮是否访问了知识库。 */
export function ToolCallList({ toolCalls }: { toolCalls?: AgentToolCall[] }) {
  /** 判断是否存在运行中或失败的调用，异常状态默认展开以免被折叠隐藏。 */
  const hasAttentionStatus = useMemo(
    () => toolCalls?.some((toolCall) => toolCall.status === "failed" || toolCall.status === "running") ?? false,
    [toolCalls],
  );
  /** 控制工具调用轨迹展开状态，完成态默认收起以减少对话正文干扰。 */
  const [isExpanded, setIsExpanded] = useState(hasAttentionStatus);

  useEffect(() => {
    // 工具运行中或失败时自动展开，避免异常状态被旧的收起状态遮住。
    if (hasAttentionStatus) {
      setIsExpanded(true);
    }
  }, [hasAttentionStatus]);

  if (!toolCalls?.length) {
    return null;
  }

  /** 按状态汇总调用数量，收起态也能看到本轮 Agent 是否仍在执行或失败。 */
  const statusCounts = toolCalls.reduce(
    (counts, toolCall) => ({
      completed: counts.completed + (toolCall.status === "completed" ? 1 : 0),
      failed: counts.failed + (toolCall.status === "failed" ? 1 : 0),
      running: counts.running + (toolCall.status === "running" ? 1 : 0),
    }),
    { completed: 0, failed: 0, running: 0 },
  );
  /** 汇总当前轨迹状态，用于收起态提示用户是否有异常或正在运行的工具。 */
  const statusSummary = hasAttentionStatus
    ? toolCalls.some((toolCall) => toolCall.status === "failed")
      ? "存在失败调用"
      : "工具正在运行"
    : "已完成";
  const ToggleIcon = isExpanded ? ChevronDown : ChevronRight;

  return (
    <div className="mt-2.5 grid min-w-0 gap-1.5" aria-label="Agent 工具调用轨迹">
      <button
        className="grid min-w-0 grid-cols-[auto_auto_minmax(0,max-content)_minmax(0,1fr)_auto] items-center gap-1.5 rounded-control border border-primary-border bg-primary-wash px-2 py-[7px] text-left text-agent-strong"
        type="button"
        aria-expanded={isExpanded}
        onClick={() => setIsExpanded((current) => !current)}
      >
        <ToggleIcon size={13} className="min-w-[13px]" />
        <BrainCircuit size={14} />
        <span className="min-w-0 text-xs font-bold [overflow-wrap:anywhere]">运行信息</span>
        <span className="min-w-0 text-xs text-[#626985] [overflow-wrap:anywhere]">
          {toolCalls.length} 次调用 · {statusSummary}
        </span>
        <span className="inline-flex gap-1" aria-label="工具调用状态汇总">
          {statusCounts.running > 0 && (
            <em className="grid size-[18px] min-w-[18px] place-items-center rounded-full bg-agent-soft text-[10px] font-extrabold not-italic text-agent-strong">
              {statusCounts.running}
            </em>
          )}
          {statusCounts.failed > 0 && (
            <em className="grid size-[18px] min-w-[18px] place-items-center rounded-full bg-danger-soft text-[10px] font-extrabold not-italic text-danger">
              {statusCounts.failed}
            </em>
          )}
          {statusCounts.completed > 0 && (
            <em className="grid size-[18px] min-w-[18px] place-items-center rounded-full bg-success-soft text-[10px] font-extrabold not-italic text-success">
              {statusCounts.completed}
            </em>
          )}
        </span>
      </button>

      {isExpanded && (
        <div className="grid min-w-0 gap-1.5">
          {toolCalls.map((toolCall) => {
            /** 根据调用类型选择轨迹图标，让模型请求和本地工具一眼可分辨。 */
            const Icon =
              toolCall.name === "activate_skill" || toolCall.name === "skill_context"
                ? Sparkles
                : toolCall.name === "model_request"
                ? BrainCircuit
                : toolCall.name === "search" || toolCall.name === "search_notes"
                  ? Search
                  : toolCall.status === "completed"
                    ? CheckCircle2
                    : Wrench;

            return (
              <div
                className={cn(
                  "grid min-w-0 grid-cols-[auto_minmax(0,9.5rem)_minmax(0,1fr)] items-start gap-1.5 rounded-control border border-primary-border bg-primary-wash px-2 py-[7px] text-agent-strong [overflow-wrap:anywhere] [word-break:break-word]",
                  toolCall.status === "failed" && "border-[rgba(var(--danger-rgb),0.26)] bg-danger-soft text-danger",
                  toolCall.status === "running" && "border-primary-border bg-agent-soft",
                )}
                key={toolCall.id}
              >
                <Icon size={13} className="mt-0.5 shrink-0" />
                <OverflowTooltipText className="min-w-0 font-mono text-xs font-bold leading-[1.45] [overflow-wrap:anywhere]" text={toolCall.name} logArea="agent_tool_call_name" />
                <OverflowTooltipText
                  as="p"
                  className={cn("m-0 min-w-0 text-xs leading-normal [overflow-wrap:anywhere] [word-break:break-word]", toolCall.status === "failed" ? "text-[#7f1d1d]" : "text-[#626985]")}
                  text={toolCall.summary}
                  logArea="agent_tool_call_summary"
                />
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
