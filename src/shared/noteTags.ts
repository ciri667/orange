/** 单个标签最长字符数，避免把整段误粘贴进标签。 */
export const MAX_NOTE_TAG_CHARS = 40;

/** 可视化编辑时的标签数量上限，防止文末标签行过长。 */
export const MAX_NOTE_TAGS = 20;

/** 从 Markdown 正文提取标签，规则与后端 `storage::extract_tags` 对齐。 */
export function extractNoteTags(content: string): string[] {
  const tags = [...parseFrontmatterTags(content), ...parseHashtagLineTags(content)];

  return uniqueSortedTags(tags);
}

/** 把当前标签写回正文：已有 frontmatter `tags` 时改 YAML，否则维护独立 `#标签` 行。 */
export function applyNoteTags(content: string, nextTags: string[]): string {
  const normalized = uniqueSortedTags(nextTags);
  const frontmatter = splitFrontmatter(content);

  if (frontmatter && hasFrontmatterTagsKey(frontmatter.yaml)) {
    const yaml = upsertFrontmatterTags(frontmatter.yaml, normalized);
    const body = stripHashtagOnlyLines(frontmatter.body, 0);

    return formatFrontmatterDocument(yaml, body);
  }

  const stripped = stripHashtagOnlyLines(content, frontmatterBodyStartLine(content.split("\n")));

  return appendTrailingHashtags(stripped, normalized);
}

/** 去掉空值、非法字符和重复项，供输入框和正文解析共用。 */
export function normalizeTagName(raw: string): string | null {
  let name = raw.trim();

  if (name.startsWith("#") && !name.startsWith("##")) {
    name = name.slice(1).trim();
  }

  name = name.replace(/^[,\.;:!?，。；：！？"'“”]+|[,\.;:!?，。；：！？"'“”]+$/g, "").trim();

  if (!name || [...name].length > MAX_NOTE_TAG_CHARS) {
    return null;
  }

  if (/[\s#[\]{},:]/.test(name)) {
    return null;
  }

  return name;
}

/** 去重并按中文习惯排序，保证信息栏和编辑器顺序稳定。 */
function uniqueSortedTags(tags: string[]): string[] {
  const unique = new Set<string>();

  for (const tag of tags) {
    const normalized = normalizeTagName(tag);

    if (normalized) {
      unique.add(normalized);
    }
  }

  return [...unique].sort((left, right) => left.localeCompare(right, "zh-CN"));
}

/** 拆出闭合 YAML frontmatter；未闭合则视为普通正文。 */
function splitFrontmatter(content: string): { yaml: string; body: string } | null {
  const normalized = content.replace(/^\uFEFF/, "");
  const lines = normalized.split("\n");

  if (trimLine(lines[0] ?? "") !== "---") {
    return null;
  }

  for (let index = 1; index < lines.length; index += 1) {
    if (trimLine(lines[index] ?? "") === "---") {
      return {
        yaml: lines.slice(1, index).join("\n"),
        body: lines.slice(index + 1).join("\n"),
      };
    }
  }

  return null;
}

/** 读取 frontmatter 中的 tags 字段。 */
function parseFrontmatterTags(content: string): string[] {
  const frontmatter = splitFrontmatter(content);

  return frontmatter ? parseFrontmatterTagsFromYaml(frontmatter.yaml) : [];
}

/** 解析 `tags: a, b`、`tags: [a, b]` 和多行 `- item`。 */
function parseFrontmatterTagsFromYaml(yaml: string): string[] {
  const lines = yaml.split("\n");
  const tags: string[] = [];

  for (let index = 0; index < lines.length; index += 1) {
    const line = stripCarriageReturn(lines[index] ?? "");

    if (!line.startsWith("tags:")) {
      continue;
    }

    const rest = line.slice("tags:".length).trim();

    if (rest) {
      tags.push(...parseInlineTagList(rest));
      continue;
    }

    index += 1;
    while (index < lines.length) {
      const itemLine = stripCarriageReturn(lines[index] ?? "");
      const item = parseYamlListItem(itemLine);

      if (item !== null) {
        tags.push(item);
        index += 1;
        continue;
      }

      if (!itemLine.trim()) {
        index += 1;
        continue;
      }

      break;
    }
  }

  return tags;
}

/** 顶层是否已经声明 `tags` 键，用来决定可视化编辑写 YAML 还是写 `#标签` 行。 */
function hasFrontmatterTagsKey(yaml: string): boolean {
  return yaml.split("\n").some((line) => stripCarriageReturn(line).startsWith("tags:"));
}

/** 更新或插入 frontmatter 的 tags 字段，其它键保持原样。 */
function upsertFrontmatterTags(yaml: string, tags: string[]): string {
  const lines = yaml.split("\n").map(stripCarriageReturn);
  const rendered = tags.length ? `tags: [${tags.join(", ")}]` : null;
  let start = -1;
  let end = -1;

  for (let index = 0; index < lines.length; index += 1) {
    if (!lines[index]?.startsWith("tags:")) {
      continue;
    }

    start = index;
    end = index + 1;
    if (!lines[index].slice("tags:".length).trim()) {
      while (end < lines.length && parseYamlListItem(lines[end] ?? "") !== null) {
        end += 1;
      }
    }
    break;
  }

  if (start >= 0) {
    const nextLines = [...lines];

    if (rendered) {
      nextLines.splice(start, end - start, rendered);
    } else {
      nextLines.splice(start, end - start);
    }

    return nextLines.join("\n");
  }

  if (!rendered) {
    return yaml;
  }

  const nextLines = [...lines];
  let insertAt = 0;

  while (insertAt < nextLines.length && !nextLines[insertAt]?.trim()) {
    insertAt += 1;
  }

  nextLines.splice(insertAt, 0, rendered);

  return nextLines.join("\n");
}

/** 读取缩进后的 YAML 列表项。 */
function parseYamlListItem(line: string): string | null {
  const trimmedEnd = line.trimEnd();

  if (!trimmedEnd.startsWith(" ") && !trimmedEnd.startsWith("\t")) {
    return null;
  }

  const rest = trimmedEnd.trim();
  if (!rest.startsWith("- ")) {
    return null;
  }

  return unquoteYamlScalar(rest.slice(2).trim());
}

/** 解析逗号分隔或 YAML 流式数组。 */
function parseInlineTagList(raw: string): string[] {
  const trimmed = raw.trim();
  const inner =
    trimmed.startsWith("[") && trimmed.endsWith("]") && trimmed.length >= 2
      ? trimmed.slice(1, -1)
      : trimmed;

  return inner
    .split(",")
    .map((part) => unquoteYamlScalar(part.trim()))
    .filter(Boolean);
}

/** 去掉 YAML 标量两端引号。 */
function unquoteYamlScalar(value: string): string {
  const trimmed = value.trim();

  if (
    trimmed.length >= 2 &&
    ((trimmed.startsWith('"') && trimmed.endsWith('"')) || (trimmed.startsWith("'") && trimmed.endsWith("'")))
  ) {
    return trimmed.slice(1, -1);
  }

  return trimmed;
}

/** 收集独立 `#标签` 行，跳过 frontmatter 和代码围栏。 */
function parseHashtagLineTags(content: string): string[] {
  const lines = content.split("\n");
  const inFence = codeFenceMask(lines);
  const start = frontmatterBodyStartLine(lines);
  const tags: string[] = [];

  for (let index = start; index < lines.length; index += 1) {
    if (inFence[index]) {
      continue;
    }

    const lineTags = parseHashtagOnlyLine(lines[index] ?? "");

    if (lineTags) {
      tags.push(...lineTags);
    }
  }

  return tags;
}

/** 正文起始行号；没有闭合 frontmatter 时从文件头开始。 */
function frontmatterBodyStartLine(lines: string[]): number {
  if (trimLine(lines[0] ?? "") !== "---") {
    return 0;
  }

  for (let index = 1; index < lines.length; index += 1) {
    if (trimLine(lines[index] ?? "") === "---") {
      return index + 1;
    }
  }

  return 0;
}

/** 标记每一行是否位于 fenced code 内。 */
function codeFenceMask(lines: string[]): boolean[] {
  let inFence = false;

  return lines.map((line) => {
    const trimmed = trimLine(line);

    if (trimmed.startsWith("```") || trimmed.startsWith("~~~")) {
      inFence = !inFence;
      return true;
    }

    return inFence;
  });
}

/** 整行都是 `#标签` 时返回标签，否则返回 null。 */
function parseHashtagOnlyLine(line: string): string[] | null {
  const trimmed = trimLine(line);

  if (!trimmed || isAtxHeading(trimmed)) {
    return null;
  }

  const tags: string[] = [];

  for (const token of trimmed.split(/\s+/)) {
    const name = hashtagTokenName(token);

    if (!name) {
      return null;
    }

    tags.push(name);
  }

  return tags.length ? tags : null;
}

/** CommonMark ATX 标题需要 `#` 后接空白。 */
function isAtxHeading(trimmed: string): boolean {
  const match = trimmed.match(/^(#{1,6})(.*)$/);

  if (!match) {
    return false;
  }

  const rest = match[2] ?? "";

  return rest.length === 0 || /^\s/.test(rest);
}

/** 解析单个 `#标签` token。 */
function hashtagTokenName(token: string): string | null {
  if (!token.startsWith("#") || token.startsWith("##")) {
    return null;
  }

  return normalizeTagName(token.slice(1));
}

/** 删除正文中的独立标签行，供可视化编辑重写前清理旧表示。 */
function stripHashtagOnlyLines(content: string, bodyStart: number): string {
  const lines = content.split("\n");
  const inFence = codeFenceMask(lines);
  const kept: string[] = [];

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index] ?? "";
    const shouldStrip = index >= bodyStart && !inFence[index] && parseHashtagOnlyLine(line);

    if (shouldStrip) {
      continue;
    }

    kept.push(line);
  }

  return collapseExtraBlankLines(kept.join("\n")).replace(/\s+$/u, "");
}

/** 把标签写到正文末尾单独一行。 */
function appendTrailingHashtags(body: string, tags: string[]): string {
  const trimmed = body.replace(/\s+$/u, "");

  if (!tags.length) {
    return trimmed ? `${trimmed}\n` : "";
  }

  const tagLine = tags.map((tag) => `#${tag}`).join(" ");

  return trimmed ? `${trimmed}\n\n${tagLine}\n` : `${tagLine}\n`;
}

/** 用更新后的 YAML 重新拼出带 frontmatter 的文档；YAML 被删空时不再保留空壳。 */
function formatFrontmatterDocument(yaml: string, body: string): string {
  const yamlTrimmed = yaml.replace(/^\n+/u, "").replace(/\n+$/u, "");
  const bodyContent = body.replace(/^\n+/u, "").replace(/\s+$/u, "");

  if (!yamlTrimmed) {
    return bodyContent ? `${bodyContent}\n` : "";
  }

  return `---\n${yamlTrimmed}\n---\n${bodyContent ? `\n${bodyContent}\n` : ""}`;
}

/** 去掉标签行后把连续空行收成最多一行间隔。 */
function collapseExtraBlankLines(content: string): string {
  return content.replace(/\n{3,}/g, "\n\n");
}

/** 去掉行尾 CR，便于同时处理 Windows 换行。 */
function stripCarriageReturn(line: string): string {
  return line.endsWith("\r") ? line.slice(0, -1) : line;
}

/** 去掉 CR 后再 trim，用于识别 `---` 分隔符。 */
function trimLine(line: string): string {
  return stripCarriageReturn(line).trim();
}
