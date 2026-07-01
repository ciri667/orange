import type { ProposedChangeDiffStats } from "../shared/types";

/** diff 行展示类型；context 为未改动上下文，placeholder 为折叠占位。 */
export type MarkdownDiffLineKind = "context" | "added" | "removed" | "placeholder";

/** 单行 diff 结果，保留两侧行号用于定位评论和后续 Agent 反馈。 */
export interface MarkdownDiffLine {
  id: string;
  kind: MarkdownDiffLineKind;
  originalLineNumber?: number;
  nextLineNumber?: number;
  text: string;
}

/** 单个 hunk 是一组相邻变更及其上下文，UI 可独立折叠。 */
export interface MarkdownDiffHunk {
  id: string;
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  lines: MarkdownDiffLine[];
  hiddenBefore: number;
  hiddenAfter: number;
}

/** 前端审阅工作台使用的完整 diff 模型。 */
export interface MarkdownDiffResult {
  hunks: MarkdownDiffHunk[];
  stats: ProposedChangeDiffStats;
}

/** 折叠上下文时，每个 hunk 前后保留的未变更行数。 */
const DEFAULT_CONTEXT_RADIUS = 3;

/** LCS 表格的最大单元数量，超过后降级为整段删除/新增，避免大文档卡住 UI。 */
const MAX_LCS_CELLS = 900_000;

/** 生成稳定 Markdown 行级 diff；输入正文不会进入日志或外部副作用。 */
export function buildMarkdownDiff(original: string, next: string, contextRadius = DEFAULT_CONTEXT_RADIUS): MarkdownDiffResult {
  const originalLines = splitMarkdownLines(original);
  const nextLines = splitMarkdownLines(next);
  const operations = buildLineOperations(originalLines, nextLines);
  const hunks = buildDiffHunks(operations, contextRadius);
  const stats = buildDiffStats(original, next, originalLines, nextLines, hunks);

  return { hunks, stats };
}

/** 拆分 Markdown 真实行；空文档返回空数组，让新建文件显示为纯新增。 */
function splitMarkdownLines(content: string) {
  if (!content) {
    return [];
  }

  return content.split(/\r\n|\r|\n/);
}

/** 基于 LCS 生成行级操作；大输入降级为整段替换，保证审阅面板性能可控。 */
function buildLineOperations(originalLines: string[], nextLines: string[]) {
  if (!originalLines.length && !nextLines.length) {
    return [];
  }

  if (!originalLines.length) {
    return nextLines.map((text, index) => createAddedOperation(index + 1, text));
  }

  if (!nextLines.length) {
    return originalLines.map((text, index) => createRemovedOperation(index + 1, text));
  }

  if (originalLines.length * nextLines.length > MAX_LCS_CELLS) {
    return [
      ...originalLines.map((text, index) => createRemovedOperation(index + 1, text)),
      ...nextLines.map((text, index) => createAddedOperation(index + 1, text)),
    ];
  }

  const table = createLcsTable(originalLines, nextLines);
  const reversed = [];
  let originalIndex = originalLines.length;
  let nextIndex = nextLines.length;

  while (originalIndex > 0 || nextIndex > 0) {
    if (originalIndex > 0 && nextIndex > 0 && originalLines[originalIndex - 1] === nextLines[nextIndex - 1]) {
      reversed.push(createContextOperation(originalIndex, nextIndex, originalLines[originalIndex - 1]));
      originalIndex -= 1;
      nextIndex -= 1;
    } else if (nextIndex > 0 && (originalIndex === 0 || table[originalIndex][nextIndex - 1] >= table[originalIndex - 1][nextIndex])) {
      reversed.push(createAddedOperation(nextIndex, nextLines[nextIndex - 1]));
      nextIndex -= 1;
    } else {
      reversed.push(createRemovedOperation(originalIndex, originalLines[originalIndex - 1]));
      originalIndex -= 1;
    }
  }

  return reversed.reverse();
}

/** 创建 LCS 动态规划表；数值只用于回溯路径，不暴露给 UI。 */
function createLcsTable(originalLines: string[], nextLines: string[]) {
  const table = Array.from({ length: originalLines.length + 1 }, () => Array(nextLines.length + 1).fill(0));

  for (let originalIndex = 1; originalIndex <= originalLines.length; originalIndex += 1) {
    for (let nextIndex = 1; nextIndex <= nextLines.length; nextIndex += 1) {
      table[originalIndex][nextIndex] =
        originalLines[originalIndex - 1] === nextLines[nextIndex - 1]
          ? table[originalIndex - 1][nextIndex - 1] + 1
          : Math.max(table[originalIndex - 1][nextIndex], table[originalIndex][nextIndex - 1]);
    }
  }

  return table;
}

/** 将完整操作流按变更附近上下文切成 hunk，并记录被折叠的行数。 */
function buildDiffHunks(operations: MarkdownDiffLine[], contextRadius: number) {
  const changedIndexes = operations
    .map((operation, index) => (operation.kind === "added" || operation.kind === "removed" ? index : -1))
    .filter((index) => index >= 0);

  if (!changedIndexes.length) {
    return [
      createHunk("hunk-1", operations.slice(0, Math.max(contextRadius * 2 + 1, 1)), {
        hiddenBefore: 0,
        hiddenAfter: Math.max(operations.length - Math.max(contextRadius * 2 + 1, 1), 0),
      }),
    ];
  }

  const ranges: Array<{ start: number; end: number }> = [];

  for (const changedIndex of changedIndexes) {
    const nextRange = {
      start: Math.max(changedIndex - contextRadius, 0),
      end: Math.min(changedIndex + contextRadius, operations.length - 1),
    };
    const previousRange = ranges[ranges.length - 1];

    if (previousRange && nextRange.start <= previousRange.end + 1) {
      previousRange.end = Math.max(previousRange.end, nextRange.end);
    } else {
      ranges.push(nextRange);
    }
  }

  return ranges.map((range, index) =>
    createHunk(`hunk-${index + 1}`, operations.slice(range.start, range.end + 1), {
      hiddenBefore: index === 0 ? range.start : 0,
      hiddenAfter:
        index === ranges.length - 1 ? Math.max(operations.length - range.end - 1, 0) : Math.max(ranges[index + 1].start - range.end - 1, 0),
    }),
  );
}

/** 创建 hunk 头部元数据，行号遵循 unified diff 常见的 1-based 规则。 */
function createHunk(id: string, lines: MarkdownDiffLine[], counts: { hiddenBefore: number; hiddenAfter: number }): MarkdownDiffHunk {
  const oldLineNumbers = lines
    .map((line) => line.originalLineNumber)
    .filter((lineNumber): lineNumber is number => typeof lineNumber === "number");
  const newLineNumbers = lines
    .map((line) => line.nextLineNumber)
    .filter((lineNumber): lineNumber is number => typeof lineNumber === "number");

  return {
    id,
    oldStart: oldLineNumbers[0] ?? 0,
    oldLines: oldLineNumbers.length,
    newStart: newLineNumbers[0] ?? 0,
    newLines: newLineNumbers.length,
    lines,
    hiddenBefore: counts.hiddenBefore,
    hiddenAfter: counts.hiddenAfter,
  };
}

/** 汇总审阅头部和日志需要的数量信息。 */
function buildDiffStats(
  original: string,
  next: string,
  originalLines: string[],
  nextLines: string[],
  hunks: MarkdownDiffHunk[],
): ProposedChangeDiffStats {
  let addedLines = 0;
  let removedLines = 0;
  let contextLines = 0;

  for (const hunk of hunks) {
    for (const line of hunk.lines) {
      if (line.kind === "added") {
        addedLines += 1;
      } else if (line.kind === "removed") {
        removedLines += 1;
      } else if (line.kind === "context") {
        contextLines += 1;
      }
    }
  }

  return {
    addedLines,
    removedLines,
    contextLines,
    hunkCount: hunks.length,
    originalLineCount: originalLines.length,
    nextLineCount: nextLines.length,
    originalCharCount: original.length,
    nextCharCount: next.length,
  };
}

/** 创建未变更行操作。 */
function createContextOperation(originalLineNumber: number, nextLineNumber: number, text: string): MarkdownDiffLine {
  return {
    id: `ctx-${originalLineNumber}-${nextLineNumber}`,
    kind: "context",
    originalLineNumber,
    nextLineNumber,
    text,
  };
}

/** 创建删除行操作。 */
function createRemovedOperation(originalLineNumber: number, text: string): MarkdownDiffLine {
  return {
    id: `del-${originalLineNumber}`,
    kind: "removed",
    originalLineNumber,
    text,
  };
}

/** 创建新增行操作。 */
function createAddedOperation(nextLineNumber: number, text: string): MarkdownDiffLine {
  return {
    id: `add-${nextLineNumber}`,
    kind: "added",
    nextLineNumber,
    text,
  };
}
