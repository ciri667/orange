import { CheckCircle2, Search, Wrench } from "lucide-react";
import type { AgentToolCall } from "../shared/types";

/** 工具调用轨迹列表，让用户知道 Agent 本轮是否访问了知识库。 */
export function ToolCallList({ toolCalls }: { toolCalls?: AgentToolCall[] }) {
  if (!toolCalls?.length) {
    return null;
  }

  return (
    <div className="tool-call-list" aria-label="Agent 工具调用轨迹">
      {toolCalls.map((toolCall) => {
        const Icon = toolCall.name === "search_notes" ? Search : toolCall.status === "completed" ? CheckCircle2 : Wrench;

        return (
          <div className="tool-call" key={toolCall.id}>
            <Icon size={13} />
            <span>{toolCall.name}</span>
            <p>{toolCall.summary}</p>
          </div>
        );
      })}
    </div>
  );
}
