import { createLocalId, formatLocalDateTime } from "../shared/id";
import type { AgentToolCall, AgentToolName, AgentTraceStep } from "../shared/types";

/** 基建工具不进入用户过程区，和后端 HIDDEN_TOOL_NAMES 保持一致。 */
const HIDDEN_TOOL_NAMES = new Set<AgentToolName>([
  "skill_context",
  "model_request",
  "activate_skill",
  "local_rule_agent",
]);

/** 历史工具名收到闭集短名，供图标和标签复用。 */
const CANONICAL_TOOL_NAMES: Partial<Record<string, AgentToolName>> = {
  search_notes: "search",
  read_file: "read",
  read_note: "read",
  read_document: "read",
  get_current_file: "read",
  list_tree: "list",
  propose_file_change: "edit",
  propose_note_change: "edit",
  create_file_draft: "write",
  create_note_draft: "write",
  create_folder: "write",
  list_path: "list",
  read_path: "read",
  run_skill: "run",
};

/** 把持久化轨迹里的旧工具名收成闭集短名。 */
export function canonicalToolName(name?: string): string {
  if (!name) {
    return "";
  }

  return CANONICAL_TOOL_NAMES[name] ?? name;
}

/** 工具折叠标题的中文别名，摘要缺失时作为兜底。 */
const TOOL_LABELS: Partial<Record<string, string>> = {
  search: "搜索笔记",
  search_notes: "搜索笔记",
  read: "读取文件",
  read_file: "读取文件",
  read_document: "读取文档",
  list: "查看目录",
  list_tree: "查看目录",
  list_path: "查看路径",
  read_path: "读取路径",
  get_current_file: "获取当前文件",
  get_session_summary: "读取会话摘要",
  search_session_messages: "搜索会话消息",
  read_session_context: "读取会话上下文",
  run: "运行 Skill",
  run_skill: "运行 Skill",
  create_folder: "创建文件夹",
  edit: "编辑了文件",
  propose_file_change: "编辑了文件",
  write: "创建文件草稿",
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

  if (step.name) {
    const canonical = canonicalToolName(step.name);
    return TOOL_LABELS[canonical] ?? TOOL_LABELS[step.name] ?? step.name;
  }

  return step.name ?? "工具调用";
}

/** 与 AgentTurnTrace / liveTurn.status 相同的三态，避免纯函数依赖组件模块。 */
export type AgentTraceTurnStatus = "running" | "completed" | "failed";

/** 贴底跟滚阈值：大约两行，不做成设置项。 */
export const TRACE_SCROLL_BOTTOM_PX = 48;

/** 空过程区是否还要画出来。有步骤或失败必画；running 且还没有终稿也画（正在思考）；纯问答一旦有 content 且无步骤则不画。 */
export function shouldRenderTurnTrace(
  steps: AgentTraceStep[],
  status: AgentTraceTurnStatus,
  content: string,
): boolean {
  if (steps.length > 0) {
    return true;
  }
  if (status === "failed") {
    return true;
  }
  return status === "running" && !content.trim();
}

/** 整段过程区在状态跳转时的下一展开值。running→completed 收一次；转入 failed 展开一次；其余保持用户当前选择。 */
export function nextTurnTraceExpanded(
  previousStatus: AgentTraceTurnStatus,
  nextStatus: AgentTraceTurnStatus,
  currentExpanded: boolean,
): boolean {
  if (previousStatus === "running" && nextStatus === "completed") {
    return false;
  }
  if (nextStatus === "failed" && previousStatus !== "failed") {
    return true;
  }
  return currentExpanded;
}

/**
 * 工具详情是否默认摊开。
 * 失败始终摊开。running 时：当前 running 步、以及时间线末尾刚完成且终稿还没开始的步摊开。
 * 终稿已经开始流（hasLiveAnswer）时，末尾完成步改为一行摘要，把光标让给回答区。
 */
export function shouldExpandToolStep(
  steps: AgentTraceStep[],
  index: number,
  turnStatus: AgentTraceTurnStatus,
  hasLiveAnswer = false,
): boolean {
  const step = steps[index];
  if (!step || step.type !== "tool") {
    return false;
  }
  if (step.status === "failed") {
    return true;
  }
  if (turnStatus !== "running") {
    return false;
  }
  if (step.status === "running") {
    return true;
  }
  if (hasLiveAnswer) {
    return false;
  }
  return index === steps.length - 1 && step.status === "completed";
}

/** 思考段是否带脉冲光标：仅 running、该段是时间线末尾、且回答区还没有终稿。 */
export function shouldShowThinkingCaret(
  steps: AgentTraceStep[],
  index: number,
  turnStatus: AgentTraceTurnStatus,
  hasLiveAnswer = false,
): boolean {
  if (hasLiveAnswer || turnStatus !== "running") {
    return false;
  }
  return steps[index]?.type === "thinking" && index === steps.length - 1;
}

/** 跟滚依赖指纹：思考变长或当前工具状态变化时必须变，不能只用 steps.length。 */
export function getTraceScrollFingerprint(steps: AgentTraceStep[]): string {
  const lastThinking = [...steps].reverse().find((step) => step.type === "thinking");
  const runningTool = steps.find((step) => step.type === "tool" && step.status === "running");
  const lastStep = steps[steps.length - 1];
  return [
    String(steps.length),
    lastThinking?.content ?? "",
    runningTool?.id ?? "",
    runningTool?.status ?? "",
    runningTool?.summary ?? "",
    lastStep?.id ?? "",
    lastStep?.status ?? "",
  ].join("\0");
}

/** 离底部不超过 thresholdPx 视为贴底，才允许自动 scrollTo。 */
export function isNearScrollBottom(
  scrollTop: number,
  scrollHeight: number,
  clientHeight: number,
  thresholdPx = TRACE_SCROLL_BOTTOM_PX,
): boolean {
  return scrollHeight - scrollTop - clientHeight <= thresholdPx;
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

/** 过程区长文本的截断标记，和后端 truncate_trace_text 保持一致。 */
export const TRACE_TRUNCATION_MARK = "…[已截断]";

/** 折叠行右侧的短类型标签，避免把英文工具名直接铺在稿纸上。 */
const TOOL_KIND_LABELS: Partial<Record<string, string>> = {
  search: "检索",
  search_notes: "检索",
  read: "读取",
  read_file: "读取",
  read_document: "文档",
  list: "目录",
  list_tree: "目录",
  list_path: "浏览",
  read_path: "读取",
  get_current_file: "当前",
  get_session_summary: "摘要",
  search_session_messages: "会话",
  read_session_context: "会话",
  run: "Skill",
  run_skill: "Skill",
  create_folder: "建夹",
  edit: "编辑",
  propose_file_change: "编辑",
  write: "新建",
  create_file_draft: "新建",
  suggest_organization: "整理",
  review_change: "审阅",
};

/** 参数/结果字段的中文标签，未知键回退为原字段名。 */
const TRACE_FIELD_LABELS: Record<string, string> = {
  title: "标题",
  targetPath: "路径",
  path: "路径",
  fileName: "文件名",
  fileType: "类型",
  query: "检索",
  operation: "操作",
  type: "动作",
  status: "状态",
  skillId: "Skill",
  targetKind: "对象",
  content: "正文",
  next: "新稿",
  original: "原文",
  snippet: "摘录",
  text: "文本",
  body: "正文",
  markdown: "正文",
  contextSummary: "会话摘要",
  citations: "命中",
  notes: "笔记",
  documents: "文档",
  folders: "目录",
  matches: "匹配",
  messages: "消息",
  edits: "多处编辑",
  knowledgeBases: "知识库",
  totalNotes: "笔记数",
  totalDocuments: "文档数",
  totalFiles: "文件数",
  totalFolders: "目录数",
  messageCount: "消息数",
  hits: "命中数",
  count: "数量",
  truncated: "已截断",
  name: "名称",
  summary: "摘要",
  diffStats: "变更",
  fileTypeCounts: "类型分布",
  knowledgeBaseId: "知识库",
  fileId: "文件 ID",
  noteId: "笔记 ID",
  targetId: "目标 ID",
  originalHash: "原文哈希",
  contentHash: "内容哈希",
  id: "ID",
};

/** 作为正文预览展示的长文本字段，不再丢进 JSON 黑盒。 */
const BODY_FIELD_KEYS = new Set([
  "content",
  "next",
  "original",
  "snippet",
  "text",
  "body",
  "markdown",
  "contextSummary",
]);

/** 作为列表展示的数组字段。 */
const LIST_FIELD_KEYS = new Set([
  "citations",
  "notes",
  "documents",
  "folders",
  "matches",
  "messages",
  "edits",
  "knowledgeBases",
]);

/** 默认收进「技术细节」的内部标识，避免 UUID 抢视线。 */
const TECH_FIELD_KEYS = new Set([
  "id",
  "knowledgeBaseId",
  "fileId",
  "noteId",
  "targetId",
  "originalHash",
  "contentHash",
  "sessionId",
  "liveMessageId",
]);

/** 工具结果里常见的单层包装，解开后才能读到标题和正文。 */
const RESULT_WRAPPER_KEYS = ["change", "file", "document", "note", "suggestion", "payload"] as const;

/** 截断 JSON 里仍值得尽力抽出的字段。 */
const EXTRACTABLE_JSON_KEYS = [
  "title",
  "targetPath",
  "path",
  "fileType",
  "query",
  "operation",
  "type",
  "status",
  "content",
  "next",
  "original",
] as const;

/** 元信息阅读顺序：先看标题和路径，再看类型与状态。 */
const META_FIELD_ORDER = [
  "title",
  "query",
  "targetPath",
  "path",
  "fileName",
  "fileType",
  "operation",
  "status",
  "diffStats",
];

/** 过程区展开后的结构化字段，按稿纸卡片而不是原始 JSON 渲染。 */
export type TraceDetailFieldKind = "meta" | "body" | "list" | "tech";

export interface TraceDetailField {
  key: string;
  label: string;
  kind: TraceDetailFieldKind;
  text: string;
  truncated: boolean;
  items?: string[];
}

export interface ToolTraceDetails {
  kindLabel: string;
  fields: TraceDetailField[];
  hasDetails: boolean;
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

/** 折叠行右侧的中文类型胶囊，摘要已经足够时不再重复英文工具名。 */
export function getToolKindLabel(name?: AgentToolName | string): string {
  if (!name) {
    return "工具";
  }

  const canonical = canonicalToolName(name);
  return TOOL_KIND_LABELS[canonical] ?? TOOL_KIND_LABELS[name] ?? name;
}

/** 去掉截断标记，让正文预览按普通笔记片段展示。 */
export function stripTraceTruncation(value: string): { text: string; truncated: boolean } {
  if (value.endsWith(TRACE_TRUNCATION_MARK)) {
    return {
      text: value.slice(0, -TRACE_TRUNCATION_MARK.length),
      truncated: true,
    };
  }

  if (value.includes(TRACE_TRUNCATION_MARK)) {
    return {
      text: value.split(TRACE_TRUNCATION_MARK).join(""),
      truncated: true,
    };
  }

  return { text: value, truncated: false };
}

/** 把工具步骤的参数和结果收成稿纸卡片字段，重复正文只保留一份。 */
export function buildToolTraceDetails(step: AgentTraceStep): ToolTraceDetails {
  const fields: TraceDetailField[] = [];
  const seenKeys = new Set<string>();
  const seenBodies = new Set<string>();

  ingestTraceRecord(asTraceRecord(step.args), fields, seenKeys, seenBodies);
  ingestTraceRecord(asTraceRecord(parseTracePayload(step.resultPreview)), fields, seenKeys, seenBodies);

  if (typeof step.resultPreview === "string" && step.resultPreview.trim() && !fields.length) {
    const stripped = stripTraceTruncation(humanizeTraceText(step.resultPreview));
    fields.push({
      key: "result",
      label: "结果",
      kind: "body",
      text: stripped.text.trim(),
      truncated: stripped.truncated,
    });
  }

  const kindLabel = getToolKindLabel(step.name);
  const visibleFields = sortTraceFields(
    fields.filter((field) => !(field.key === "type" && field.text === kindLabel)),
  );

  return {
    kindLabel,
    fields: visibleFields,
    hasDetails: visibleFields.length > 0 || Boolean(step.error),
  };
}

/** 元信息按阅读顺序排列，正文始终放在卡片底部。 */
function sortTraceFields(fields: TraceDetailField[]): TraceDetailField[] {
  const kindRank: Record<TraceDetailFieldKind, number> = {
    meta: 0,
    list: 1,
    tech: 2,
    body: 3,
  };

  return [...fields].sort((left, right) => {
    const kindDelta = kindRank[left.kind] - kindRank[right.kind];
    if (kindDelta !== 0) {
      return kindDelta;
    }

    if (left.kind !== "meta") {
      return 0;
    }

    const leftIndex = META_FIELD_ORDER.indexOf(left.key);
    const rightIndex = META_FIELD_ORDER.indexOf(right.key);
    return (leftIndex < 0 ? 50 : leftIndex) - (rightIndex < 0 ? 50 : rightIndex);
  });
}

/** 尝试把结果预览解析成对象；截断 JSON 则尽力抽出标题、路径和正文。 */
function parseTracePayload(raw?: string): unknown {
  if (!raw?.trim()) {
    return undefined;
  }

  const trimmed = raw.trim();
  try {
    return JSON.parse(trimmed);
  } catch {
    if (!trimmed.startsWith("{") && !trimmed.startsWith("[")) {
      return trimmed;
    }

    const extracted: Record<string, unknown> = {};
    for (const key of EXTRACTABLE_JSON_KEYS) {
      const value = extractJsonStringField(trimmed, key);
      if (value) {
        extracted[key] = value;
      }
    }

    return Object.keys(extracted).length ? extracted : trimmed;
  }
}

/** 从可能被截断的 JSON 文本中抽出一个字符串字段，并还原换行。 */
function extractJsonStringField(raw: string, key: string): string | undefined {
  const marker = `"${key}"`;
  const keyIndex = raw.indexOf(marker);
  if (keyIndex < 0) {
    return undefined;
  }

  const colon = raw.indexOf(":", keyIndex + marker.length);
  if (colon < 0) {
    return undefined;
  }

  const rest = raw.slice(colon + 1).trimStart();
  if (!rest.startsWith('"')) {
    const literal = rest.match(/^(true|false|null|-?\d+(?:\.\d+)?)/);
    return literal?.[0];
  }

  let text = "";
  for (let index = 1; index < rest.length; index += 1) {
    const character = rest[index];
    if (character === "\\") {
      const next = rest[index + 1];
      if (next === "n") {
        text += "\n";
      } else if (next === "t") {
        text += "\t";
      } else if (next === '"') {
        text += '"';
      } else if (next === "\\") {
        text += "\\";
      } else if (next) {
        text += next;
      }
      index += 1;
      continue;
    }

    if (character === '"') {
      break;
    }

    text += character;
  }

  return text || undefined;
}

/** 把转义换行还原成可读正文，供 JSON 解析失败时的兜底预览使用。 */
function humanizeTraceText(value: string): string {
  return value
    .replace(/\\n/g, "\n")
    .replace(/\\t/g, "  ")
    .replace(/\\"/g, '"')
    .replace(/\\\\/g, "\\");
}

/** 把未知值收成可遍历对象，并解开 change/file 等单层包装。 */
function asTraceRecord(value: unknown): Record<string, unknown> | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return undefined;
  }

  const record = value as Record<string, unknown>;
  for (const wrapper of RESULT_WRAPPER_KEYS) {
    const nested = record[wrapper];
    if (nested && typeof nested === "object" && !Array.isArray(nested)) {
      const rest = { ...record };
      delete rest[wrapper];
      return { ...rest, ...(nested as Record<string, unknown>) };
    }
  }

  return record;
}

/** 按字段语义写入卡片：元信息、正文、列表或技术细节。 */
function ingestTraceRecord(
  record: Record<string, unknown> | undefined,
  fields: TraceDetailField[],
  seenKeys: Set<string>,
  seenBodies: Set<string>,
) {
  if (!record) {
    return;
  }

  for (const [key, value] of Object.entries(record)) {
    ingestTraceValue(key, value, fields, seenKeys, seenBodies);
  }
}

function ingestTraceValue(
  key: string,
  value: unknown,
  fields: TraceDetailField[],
  seenKeys: Set<string>,
  seenBodies: Set<string>,
) {
  if (value == null || value === "" || key === "reviewComments" || key === "reviewState") {
    return;
  }

  if (key === "truncated" && value === false) {
    return;
  }

  if (key === "diffStats") {
    const text = formatDiffStats(value);
    if (text && !seenKeys.has(key)) {
      seenKeys.add(key);
      fields.push({ key, label: TRACE_FIELD_LABELS.diffStats, kind: "meta", text, truncated: false });
    }
    return;
  }

  if (key === "fileTypeCounts") {
    const text = formatFileTypeCounts(value);
    if (text && !seenKeys.has(key)) {
      seenKeys.add(key);
      fields.push({ key, label: TRACE_FIELD_LABELS.fileTypeCounts, kind: "meta", text, truncated: false });
    }
    return;
  }

  if (Array.isArray(value)) {
    if (seenKeys.has(key) || value.length === 0) {
      return;
    }

    seenKeys.add(key);
    const items = value.map(listItemLabel).filter((item): item is string => Boolean(item));
    if (!items.length) {
      fields.push({
        key,
        label: fieldLabel(key),
        kind: "meta",
        text: `${value.length} 项`,
        truncated: false,
      });
      return;
    }

    fields.push({
      key,
      label: fieldLabel(key),
      kind: "list",
      text: `${items.length} 项`,
      truncated: items.length > 6,
      items: items.slice(0, 6),
    });
    return;
  }

  if (typeof value === "object") {
    const nested = value as Record<string, unknown>;
    const entries = Object.entries(nested);
    const allScalar = entries.every(([, child]) => child == null || ["string", "number", "boolean"].includes(typeof child));
    if (allScalar && entries.length > 0 && entries.length <= 8) {
      ingestTraceRecord(nested, fields, seenKeys, seenBodies);
      return;
    }

    if (!seenKeys.has(key)) {
      seenKeys.add(key);
      fields.push({
        key,
        label: fieldLabel(key),
        kind: "tech",
        text: formatTraceValue(value),
        truncated: false,
      });
    }
    return;
  }

  const formatted = formatTraceFieldValue(key, value);
  if (!formatted.text) {
    return;
  }

  const kind = classifyTraceField(key, formatted.text, value);
  if (kind === "body") {
    const fingerprint = bodyFingerprint(formatted.text);
    if (!fingerprint || isDuplicateBody(fingerprint, seenBodies)) {
      return;
    }
    seenBodies.add(fingerprint);
  } else if (seenKeys.has(key)) {
    return;
  }

  seenKeys.add(key);
  fields.push({
    key,
    label: fieldLabel(key),
    kind,
    text: formatted.text,
    truncated: formatted.truncated,
  });
}

function classifyTraceField(key: string, text: string, value: unknown): TraceDetailFieldKind {
  if (TECH_FIELD_KEYS.has(key) || looksLikeIdentifier(key, value)) {
    return "tech";
  }

  if (BODY_FIELD_KEYS.has(key) || (typeof value === "string" && text.includes("\n")) || text.length > 160) {
    return "body";
  }

  if (LIST_FIELD_KEYS.has(key)) {
    return "list";
  }

  return "meta";
}

function fieldLabel(key: string): string {
  return TRACE_FIELD_LABELS[key] ?? key;
}

function formatTraceFieldValue(key: string, value: unknown): { text: string; truncated: boolean } {
  if (typeof value === "boolean") {
    return { text: value ? "是" : "否", truncated: false };
  }

  if (typeof value === "number") {
    return { text: String(value), truncated: false };
  }

  if (typeof value !== "string") {
    return { text: formatTraceValue(value), truncated: false };
  }

  const stripped = stripTraceTruncation(value);
  let text = stripped.text.trim();

  if (key === "fileType") {
    text = formatFileType(text);
  } else if (key === "operation") {
    text = ({ replace: "替换", append: "追加", multi_replace: "多处替换" } as Record<string, string>)[text] ?? text;
  } else if (key === "type") {
    text = ({ create: "新建", rewrite: "改写", organize: "整理" } as Record<string, string>)[text] ?? text;
  } else if (key === "targetKind") {
    text = ({ note: "笔记", document: "文档", folder: "文件夹" } as Record<string, string>)[text] ?? text;
  } else if (key === "status") {
    text = ({ pending: "待确认", completed: "已完成", failed: "失败", running: "进行中" } as Record<string, string>)[text] ?? text;
  }

  return { text, truncated: stripped.truncated };
}

function formatFileType(value: string): string {
  const labels: Record<string, string> = {
    markdown: "Markdown",
    md: "Markdown",
    txt: "纯文本",
    docx: "Word",
    pdf: "PDF",
    image: "图片",
  };
  return labels[value] ?? value;
}

function formatDiffStats(value: unknown): string | undefined {
  if (!value || typeof value !== "object") {
    return undefined;
  }

  const stats = value as Record<string, unknown>;
  const added = stats.added ?? stats.insertions ?? stats.addedLines;
  const removed = stats.removed ?? stats.deletions ?? stats.removedLines;
  if (typeof added !== "number" && typeof removed !== "number") {
    return undefined;
  }

  return `+${typeof added === "number" ? added : 0} / -${typeof removed === "number" ? removed : 0}`;
}

function formatFileTypeCounts(value: unknown): string | undefined {
  if (!value || typeof value !== "object") {
    return undefined;
  }

  const parts = Object.entries(value as Record<string, unknown>)
    .filter(([, count]) => typeof count === "number" && count > 0)
    .map(([type, count]) => `${formatFileType(type)} ${count}`);

  return parts.length ? parts.join(" · ") : undefined;
}

function listItemLabel(item: unknown): string | undefined {
  if (typeof item === "string") {
    const stripped = stripTraceTruncation(item).text.trim();
    return stripped || undefined;
  }

  if (!item || typeof item !== "object") {
    return undefined;
  }

  const record = item as Record<string, unknown>;
  for (const key of ["title", "path", "name", "query", "summary", "targetPath", "fileName"]) {
    const value = record[key];
    if (typeof value === "string" && value.trim()) {
      return stripTraceTruncation(value).text.trim();
    }
  }

  return undefined;
}

function looksLikeIdentifier(key: string, value: unknown): boolean {
  if (/(^id$|Id$|Hash$|_id$)/.test(key)) {
    return true;
  }

  return typeof value === "string" && /^[a-z]+-[0-9a-f-]{8,}$/i.test(value.trim());
}

function bodyFingerprint(text: string): string {
  return text.replace(/\s+/g, " ").trim().slice(0, 240);
}

function isDuplicateBody(fingerprint: string, seenBodies: Set<string>): boolean {
  for (const existing of seenBodies) {
    if (existing.startsWith(fingerprint) || fingerprint.startsWith(existing)) {
      return true;
    }
  }

  return false;
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
