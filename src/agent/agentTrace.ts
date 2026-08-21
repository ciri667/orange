import { createLocalId, formatLocalDateTime } from "../shared/id";
import type { AgentToolCall, AgentToolName, AgentTraceStep } from "../shared/types";

/** 基建工具不进入用户过程区，和后端 HIDDEN_TOOL_NAMES 保持一致。 */
const HIDDEN_TOOL_NAMES = new Set<AgentToolName>([
  "skill_context",
  "model_request",
  "activate_skill",
  "local_rule_agent",
]);

/** 工具折叠标题的中文别名，摘要缺失时作为兜底。 */
const TOOL_LABELS: Partial<Record<AgentToolName, string>> = {
  search_notes: "搜索笔记",
  read_file: "读取文件",
  read_document: "读取文档",
  list_tree: "查看目录",
  list_path: "查看路径",
  read_path: "读取路径",
  get_current_file: "获取当前文件",
  get_session_summary: "读取会话摘要",
  search_session_messages: "搜索会话消息",
  read_session_context: "读取会话上下文",
  run_skill: "运行 Skill",
  create_folder: "创建文件夹",
  propose_file_change: "编辑了文件",
  create_file_draft: "创建文件草稿",
  suggest_organization: "生成整理建议",
  review_change: "审阅变更",
};

/** 判断工具是否应出现在 Codex 风格过程区。 */
export function isUserVisibleTool(name: string): boolean {
  return !HIDDEN_TOOL_NAMES.has(name as AgentToolName);
}

/** 折叠行优先用工具摘要，没有摘要时用中文别名。 */
export function getToolTraceLabel(step: AgentTraceStep): string {
  if (step.summary?.trim()) {
    return step.summary.trim();
  }

  if (step.name && TOOL_LABELS[step.name]) {
    return TOOL_LABELS[step.name] ?? step.name;
  }

  return step.name ?? "工具调用";
}

/** 把毫秒格式化成 Codex 风格的 12s / 1m 5s / 1h 3m 50s。 */
export function formatTurnDuration(durationMs: number): string {
  const totalSeconds = Math.max(0, Math.round(durationMs / 1000));
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;

  if (hours > 0) {
    return `${hours}h ${minutes}m ${seconds}s`;
  }

  if (minutes > 0) {
    return `${minutes}m ${seconds}s`;
  }

  return `${seconds}s`;
}

/** 把工具参数或结果格式化成过程区可展开的预览文本。 */
export function formatTraceValue(value: unknown): string {
  if (typeof value === "string") {
    return value;
  }

  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

/** 浏览器 mock 和旧消息回退：从扁平 toolCalls 生成用户可见轨迹。 */
export function traceFromToolCalls(toolCalls: AgentToolCall[] = []): AgentTraceStep[] {
  return toolCalls.filter((toolCall) => isUserVisibleTool(toolCall.name)).map((toolCall) => ({
    id: toolCall.id || createLocalId("trace"),
    type: "tool" as const,
    timestamp: formatLocalDateTime(),
    name: toolCall.name,
    status: toolCall.status,
    summary: toolCall.summary,
    args: toolCall.args,
    error: toolCall.status === "failed" ? toolCall.summary : undefined,
  }));
}
