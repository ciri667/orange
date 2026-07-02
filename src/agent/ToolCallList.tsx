import { BrainCircuit, CheckCircle2, ChevronDown, ChevronRight, Search, Sparkles, Wrench } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
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
    <div className="tool-call-list" aria-label="Agent 工具调用轨迹">
      <button
        className="tool-call-toggle"
        type="button"
        aria-expanded={isExpanded}
        onClick={() => setIsExpanded((current) => !current)}
      >
        <ToggleIcon size={13} />
        <BrainCircuit size={14} />
        <span className="tool-call-toggle-title">运行信息</span>
        <span className="tool-call-toggle-meta">
          {toolCalls.length} 次调用 · {statusSummary}
        </span>
        <span className="tool-call-status-strip" aria-label="工具调用状态汇总">
          {statusCounts.running > 0 && <em className="running">{statusCounts.running}</em>}
          {statusCounts.failed > 0 && <em className="failed">{statusCounts.failed}</em>}
          {statusCounts.completed > 0 && <em className="completed">{statusCounts.completed}</em>}
        </span>
      </button>

      {isExpanded && (
        <div className="tool-call-items">
          {toolCalls.map((toolCall) => {
            /** 根据调用类型选择轨迹图标，让模型请求和本地工具一眼可分辨。 */
            const Icon =
              toolCall.name === "activate_skill" || toolCall.name === "skill_context"
                ? Sparkles
                : toolCall.name === "model_request"
                ? BrainCircuit
                : toolCall.name === "search_notes"
                  ? Search
                  : toolCall.status === "completed"
                    ? CheckCircle2
                    : Wrench;

            return (
              <div className={`tool-call ${toolCall.status}`} key={toolCall.id}>
                <Icon size={13} />
                <span>{toolCall.name}</span>
                <p>{toolCall.summary}</p>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
