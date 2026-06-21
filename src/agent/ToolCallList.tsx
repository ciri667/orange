import { BrainCircuit, CheckCircle2, Search, Wrench } from "lucide-react";
import type { AgentToolCall } from "../shared/types";

/** 工具调用轨迹列表，让用户知道 Agent 本轮是否访问了知识库。 */
export function ToolCallList({ toolCalls }: { toolCalls?: AgentToolCall[] }) {
  if (!toolCalls?.length) {
    return null;
  }

  return (
    <div className="tool-call-list" aria-label="Agent 工具调用轨迹">
      {toolCalls.map((toolCall) => {
        /** 根据调用类型选择轨迹图标，让模型请求和本地工具一眼可分辨。 */
        const Icon =
          toolCall.name === "model_request"
            ? BrainCircuit
            : toolCall.name === "search_notes"
              ? Search
              : toolCall.status === "completed"
                ? CheckCircle2
                : Wrench;

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
