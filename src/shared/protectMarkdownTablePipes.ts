/**
 * GFM 会保留表格单元格里行内代码中的 `|`，但 micromark 的 GFM 表格
 * 会把未转义竖线一律当成列分隔。解析前把代码里的 `|` 换成占位符，解析后再还原。
 */
const GFM_TABLE_PIPE_PLACEHOLDER = "\uE000";

/** 把行内代码中的 `|` 换成占位符，避免 GFM 表格在解析阶段拆列。 */
export function protectGfmTablePipesInInlineCode(markdown: string) {
  if (!markdown.includes("|") || !markdown.includes("`")) {
    return markdown;
  }

  return markdown.replace(/[^\n\r]+/g, protectInlineCodePipesInLine);
}

/** 扫描单行里成对的 backtick，只改写代码 span 内部的竖线。 */
function protectInlineCodePipesInLine(line: string) {
  if (!line.includes("|") || !line.includes("`")) {
    return line;
  }

  let result = "";
  let index = 0;

  while (index < line.length) {
    if (line[index] !== "`") {
      result += line[index];
      index += 1;
      continue;
    }

    let openLength = 0;
    while (index + openLength < line.length && line[index + openLength] === "`") {
      openLength += 1;
    }

    const closerIndex = findMatchingBackticks(line, index + openLength, openLength);
    if (closerIndex < 0) {
      result += line.slice(index, index + openLength);
      index += openLength;
      continue;
    }

    const content = line.slice(index + openLength, closerIndex);
    result += `${line.slice(index, index + openLength)}${content.split("|").join(GFM_TABLE_PIPE_PLACEHOLDER)}${line.slice(closerIndex, closerIndex + openLength)}`;
    index = closerIndex + openLength;
  }

  return result;
}

/** 按 CommonMark 规则寻找等长的闭合 backtick；找不到则这段不是代码 span。 */
function findMatchingBackticks(line: string, fromIndex: number, openLength: number) {
  let index = fromIndex;

  while (index < line.length) {
    if (line[index] !== "`") {
      index += 1;
      continue;
    }

    let closeLength = 0;
    while (index + closeLength < line.length && line[index + closeLength] === "`") {
      closeLength += 1;
    }

    if (closeLength === openLength) {
      return index;
    }

    index += closeLength;
  }

  return -1;
}

/** remark 插件：把表格解析后的占位符还原成 `|`。 */
export function remarkRestoreProtectedTablePipes() {
  return (tree: unknown) => {
    restoreProtectedPipes(tree);
  };
}

/** 递归还原文本、行内代码和代码块节点里的竖线占位符。 */
function restoreProtectedPipes(node: unknown) {
  if (!node || typeof node !== "object") {
    return;
  }

  const record = node as { value?: unknown; children?: unknown[] };
  if (typeof record.value === "string" && record.value.includes(GFM_TABLE_PIPE_PLACEHOLDER)) {
    record.value = record.value.split(GFM_TABLE_PIPE_PLACEHOLDER).join("|");
  }

  if (!Array.isArray(record.children)) {
    return;
  }

  for (const child of record.children) {
    restoreProtectedPipes(child);
  }
}
