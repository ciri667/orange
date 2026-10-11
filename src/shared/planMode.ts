import type { AgentMessage } from "./types";

/** 宿主点击「执行计划」后自动发送的执行回合消息；与计划横幅共用一个来源，避免话术漂移。 */
export const PLAN_EXECUTION_PROMPT =
  "确认，执行该计划";

/** 判断回合回复是否就是宿主要求的「执行计划」：正文包含执行计划标题字样，且至少两条编号步骤。问候、通用问答与澄清追问不满足该形状，不弹确认横幅。 */
export function assistantReplyLooksLikePlan(message: AgentMessage | undefined): boolean {
  if (!message || message.role !== "assistant" || message.interrupted) {
    return false;
  }
  if (!message.content.includes("执行计划")) {
    return false;
  }
  // 编号步骤允许 “1.”“1、”“1）” 及 markdown 加粗等前缀；标题字样已是强前置条件，误报风险低。
  const numberedSteps = message.content.match(/\b\d{1,2}\s*[.、)）]/g) ?? [];
  return numberedSteps.length >= 2;
}

/** 折叠态最多露出的计划高度。超过这个高度才提供展开，短计划保持整张卡片一次看完。 */
export const PLAN_CARD_COLLAPSED_MAX_PX = 220;

/** 用计划标题生成下载文件名。去掉 Markdown 标记和「执行计划」前缀，并清掉 Windows 文件名非法字符。 */
export function planDownloadFilename(content: string): string {
  const firstLine =
    content
      .split(/\r?\n/)
      .map((line) => line.trim())
      .find((line) => line.length > 0) ?? "";
  const heading = firstLine.replace(/^#{1,6}\s+/, "").replace(/^\*\*(.+)\*\*$/, "$1").trim();
  const titled = heading.replace(/^执行计划\s*[:：]\s*/, "").trim();
  const base = (titled || "执行计划")
    .replace(/[<>:"/\\|?*\u0000-\u001f]/g, " ")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, 80);

  return `${base || "执行计划"}.md`;
}
