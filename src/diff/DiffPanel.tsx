import { Check, ChevronDown, ChevronRight, MessageSquarePlus, SendHorizontal, X } from "lucide-react";
import { useMemo, useState } from "react";
import { Button } from "../shared/Button";
import { cn } from "../shared/cn";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import { fieldTextareaClassName, sectionLabelClassName } from "../shared/ui";
import {
  diffGutterClassName,
  diffHunkHeaderClassName,
  diffLineGridClassName,
  diffLineToneClassName,
  unifiedDiffFileClassName,
} from "./diffStyles";
import type { ProposedChange, ReviewComment } from "../shared/types";
import { buildMarkdownDiff } from "./markdownDiff";
import type { MarkdownDiffHunk, MarkdownDiffLine } from "./markdownDiff";

/** 行评论提交入参，调用方负责写回会话并持久化。 */
export interface ReviewCommentDraft {
  lineSide: ReviewComment["lineSide"];
  lineNumber: number;
  lineTextPreview: string;
  body: string;
}

/** Agent 变更审阅工作台，支持行级 diff、定位评论和整次确认写入。 */
export function DiffPanel({
  change,
  onAccept,
  onReject,
  onAddComment,
  onSubmitComments,
  isBusy,
}: {
  change: ProposedChange;
  onAccept: () => void;
  onReject: () => void;
  onAddComment: (comment: ReviewCommentDraft) => void;
  onSubmitComments: () => void;
  isBusy: boolean;
}) {
  const diff = useMemo(() => buildMarkdownDiff(change.original, change.next), [change.original, change.next]);
  const comments = change.reviewComments ?? [];
  const draftCommentCount = comments.filter((comment) => comment.status === "draft").length;
  const submittedCommentCount = comments.filter((comment) => comment.status === "submitted").length;
  /** 变更规模用于审阅区的风险提示，不记录正文内容。 */
  const changedLineCount = diff.stats.addedLines + diff.stats.removedLines;
  const [collapsedHunkIds, setCollapsedHunkIds] = useState<Set<string>>(new Set());
  const [selectedLine, setSelectedLine] = useState<{ side: ReviewComment["lineSide"]; lineNumber: number; text: string } | null>(null);
  const [commentBody, setCommentBody] = useState("");

  /** 选中可评论的增删行；上下文和折叠占位不生成评论锚点。 */
  function handleSelectLine(line: MarkdownDiffLine) {
    const anchor = getCommentAnchor(line);

    if (!anchor) {
      return;
    }

    setSelectedLine(anchor);
    setCommentBody("");
  }

  /** 保存当前评论草稿，正文保存在会话中但不会写入诊断日志。 */
  function handleAddComment() {
    const body = commentBody.trim();

    if (!selectedLine || !body) {
      return;
    }

    onAddComment({
      lineSide: selectedLine.side,
      lineNumber: selectedLine.lineNumber,
      lineTextPreview: selectedLine.text.slice(0, 120),
      body,
    });
    setSelectedLine(null);
    setCommentBody("");
  }

  /** 折叠或展开单个 hunk，保持大 diff 的扫描性能。 */
  function toggleHunk(hunkId: string) {
    setCollapsedHunkIds((currentIds) => {
      const nextIds = new Set(currentIds);

      if (nextIds.has(hunkId)) {
        nextIds.delete(hunkId);
      } else {
        nextIds.add(hunkId);
      }

      return nextIds;
    });
  }

  return (
    <aside className="flex max-h-[min(620px,55vh)] min-h-80 flex-col overflow-hidden rounded-panel border border-primary-border bg-[#f9f9f6]" aria-label="Agent 变更审阅工作台">
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0">
          <p className={sectionLabelClassName}>{change.type === "create" ? "Agent 新建文件建议" : "Agent 文档变更审阅"}</p>
          <OverflowTooltipText as="h3" className="mt-1 mb-0 text-xl leading-tight text-ink-strong" text={change.title} logArea="diff_change_title" />
          <OverflowTooltipText text={change.targetPath} logArea="diff_target_path" />
        </div>
        <div className="flex items-center gap-2">
          <Button variant="ghost" tone="danger" onClick={onReject} disabled={isBusy}>
            <X size={16} />
            拒绝写入
          </Button>
          <Button variant="primary" size="compact" onClick={onAccept} disabled={isBusy}>
            <Check size={16} />
            确认写入
          </Button>
        </div>
      </div>

      <div className="flex flex-wrap gap-[7px] rounded-control border border-primary-border bg-primary-wash px-2.5 py-2 text-xs text-ink-muted" aria-label="Agent 写入确认状态">
        <strong className="text-agent-strong">写入前检查</strong>
        <span className="border-l border-[rgba(59,92,204,0.16)] pl-2">{changedLineCount} 行变更</span>
        <span className="border-l border-[rgba(59,92,204,0.16)] pl-2">{draftCommentCount ? `${draftCommentCount} 条反馈待发送` : "可直接确认或评论"}</span>
        <span className="border-l border-[rgba(59,92,204,0.16)] pl-2">路径与 hash 会在确认时校验</span>
      </div>

      <div className="flex shrink-0 flex-wrap gap-2 border-y border-[rgba(230,224,214,0.86)] px-3.5 py-2 text-xs text-ink-muted" aria-label="变更摘要">
        <span className="font-extrabold text-success">+{diff.stats.addedLines}</span>
        <span className="font-extrabold text-danger">-{diff.stats.removedLines}</span>
        <span>{diff.stats.hunkCount} 个变更区域</span>
        <span>{formatOperationLabel(change.operation)}</span>
        <span>{diff.stats.originalLineCount} 行 → {diff.stats.nextLineCount} 行</span>
        <span>hash 校验会在确认写入时执行</span>
      </div>

      <div className="grid min-h-0 grid-cols-[minmax(0,1fr)_minmax(250px,30%)] max-[1100px]:grid-cols-1">
        <div className="min-w-0 overflow-auto border-r border-[rgba(230,224,214,0.86)] bg-surface max-[1100px]:border-r-0 max-[1100px]:border-b" aria-label="文本文件行级 diff">
          <div className={unifiedDiffFileClassName}>
            <OverflowTooltipText className="min-w-0 truncate" text={change.targetPath} logArea="diff_file_path" />
            <span className="min-w-0 truncate">{change.fileType === "txt" ? "TXT" : "Markdown"} · {change.type === "create" ? "new file" : "pending"}</span>
          </div>
          {diff.hunks.map((hunk) => (
            <DiffHunkView
              key={hunk.id}
              hunk={hunk}
              isCollapsed={collapsedHunkIds.has(hunk.id)}
              comments={comments}
              selectedLine={selectedLine}
              onToggle={() => toggleHunk(hunk.id)}
              onSelectLine={handleSelectLine}
            />
          ))}
        </div>

        <aside className="flex min-w-0 flex-col gap-2.5 overflow-auto bg-warm-panel p-3 max-[1100px]:max-h-[230px]" aria-label="审阅评论">
          <div className="flex min-w-0 flex-col gap-2">
            <div>
              <p className={sectionLabelClassName}>行评论</p>
              <strong className="text-[13px] text-ink-strong">{selectedLine ? formatLineLabel(selectedLine.side, selectedLine.lineNumber) : "选择一行变更"}</strong>
            </div>
            <textarea
              className={cn(fieldTextareaClassName, "min-h-[86px]")}
              value={commentBody}
              onChange={(event) => setCommentBody(event.target.value)}
              placeholder="写下给 Agent 的具体修改意见"
              disabled={!selectedLine || isBusy}
            />
            <Button variant="ghost" size="compact" onClick={handleAddComment} disabled={!selectedLine || !commentBody.trim() || isBusy}>
              <MessageSquarePlus size={15} />
              添加评论
            </Button>
          </div>

          <div className="flex min-w-0 flex-col gap-2">
            <div>
              <p className={sectionLabelClassName}>评论</p>
              <span className="mt-[5px] block text-xs text-ink-muted">{draftCommentCount} 条待发送，{submittedCommentCount} 条已发送</span>
            </div>
            {comments.length ? (
              comments.map((comment) => (
                <article
                  className={cn(
                    "rounded-control border border-border bg-surface p-2",
                    comment.status === "submitted" && "border-[#c6d7d5] bg-[#f5fbfa]",
                  )}
                  key={comment.id}
                >
                  <span className="text-[11px] font-extrabold text-ink-muted">{formatLineLabel(comment.lineSide, comment.lineNumber)}</span>
                  <p className="mt-1 mb-0 text-xs leading-normal text-[#24323c] [overflow-wrap:anywhere]">{comment.body}</p>
                </article>
              ))
            ) : (
              <p className="mt-1 mb-0 text-xs leading-normal text-ink-muted [overflow-wrap:anywhere]">点击变更行后添加具体反馈。</p>
            )}
          </div>

          <Button variant="primary" size="compact" onClick={onSubmitComments} disabled={!draftCommentCount || isBusy}>
            <SendHorizontal size={15} />
            发送给 Agent 处理
          </Button>
        </aside>
      </div>
    </aside>
  );
}

/** 渲染单个 hunk，折叠时只保留头部和隐藏行数提示。 */
function DiffHunkView({
  hunk,
  isCollapsed,
  comments,
  selectedLine,
  onToggle,
  onSelectLine,
}: {
  hunk: MarkdownDiffHunk;
  isCollapsed: boolean;
  comments: ReviewComment[];
  selectedLine: { side: ReviewComment["lineSide"]; lineNumber: number; text: string } | null;
  onToggle: () => void;
  onSelectLine: (line: MarkdownDiffLine) => void;
}) {
  const hunkCommentCount = comments.filter((comment) =>
    hunk.lines.some((line) => {
      const anchor = getCommentAnchor(line);

      return anchor?.side === comment.lineSide && anchor.lineNumber === comment.lineNumber;
    }),
  ).length;

  return (
    <section className="border-b border-[#eef2f4]">
      <button
        className={diffHunkHeaderClassName}
        type="button"
        onClick={onToggle}
      >
        {isCollapsed ? <ChevronRight size={14} /> : <ChevronDown size={14} />}
        <span>
          @@ -{hunk.oldStart || 0},{hunk.oldLines} +{hunk.newStart || 0},{hunk.newLines} @@
        </span>
        {hunkCommentCount > 0 && <em className="ml-auto font-inherit not-italic text-ink-muted">{hunkCommentCount} 条评论</em>}
      </button>
      {!isCollapsed && (
        <div className="flex flex-col">
          {hunk.hiddenBefore > 0 && <DiffPlaceholderLine hiddenCount={hunk.hiddenBefore} />}
          {hunk.lines.map((line) => (
            <DiffLineView
              key={line.id}
              line={line}
              comments={comments}
              isSelected={isLineSelected(line, selectedLine)}
              onSelect={() => onSelectLine(line)}
            />
          ))}
          {hunk.hiddenAfter > 0 && <DiffPlaceholderLine hiddenCount={hunk.hiddenAfter} />}
        </div>
      )}
    </section>
  );
}

/** 渲染真实 diff 行，变更行提供评论按钮锚点。 */
function DiffLineView({
  line,
  comments,
  isSelected,
  onSelect,
}: {
  line: MarkdownDiffLine;
  comments: ReviewComment[];
  isSelected: boolean;
  onSelect: () => void;
}) {
  const anchor = getCommentAnchor(line);
  const commentCount = anchor
    ? comments.filter((comment) => comment.lineSide === anchor.side && comment.lineNumber === anchor.lineNumber).length
    : 0;
  const marker = line.kind === "added" ? "+" : line.kind === "removed" ? "-" : " ";

  return (
    <button
      className={cn(
        diffLineGridClassName,
        "border-0 text-left disabled:cursor-default disabled:opacity-100",
        diffLineToneClassName(line.kind),
        isSelected && "border-l-agent bg-primary-wash",
        Boolean(anchor) && "enabled:hover:border-l-agent enabled:hover:bg-primary-wash",
      )}
      type="button"
      onClick={onSelect}
      disabled={!anchor}
      title={anchor ? "添加行评论" : undefined}
    >
      <span className={cn(diffGutterClassName, "diff-line-number-old")}>{line.originalLineNumber ?? ""}</span>
      <span className={cn(diffGutterClassName, "diff-line-number-new")}>{line.nextLineNumber ?? ""}</span>
      <span className={cn(diffGutterClassName, "diff-line-marker justify-center px-0")}>{marker}</span>
      <code className="min-w-0 px-2 py-0.5 whitespace-pre-wrap [overflow-wrap:anywhere]">{line.text || " "}</code>
      {commentCount > 0 && (
        <span className="mr-2 self-center rounded-full border border-[#c6d7d5] bg-white px-[7px] font-sans text-[11px] font-extrabold text-accent-strong">
          {commentCount}
        </span>
      )}
    </button>
  );
}

/** 折叠占位行只显示隐藏数量，不泄露正文内容。 */
function DiffPlaceholderLine({ hiddenCount }: { hiddenCount: number }) {
  return (
    <div className={cn(diffLineGridClassName, diffLineToneClassName("placeholder"))}>
      <span className={diffGutterClassName}>…</span>
      <span className={diffGutterClassName}>…</span>
      <span className={cn(diffGutterClassName, "justify-center px-0")}> </span>
      <code className="min-w-0 px-2 py-0.5 whitespace-pre-wrap [overflow-wrap:anywhere]">隐藏 {hiddenCount} 行未变更内容</code>
    </div>
  );
}

/** 只有新增和删除行可评论，并映射到 next/original 两侧。 */
function getCommentAnchor(line: MarkdownDiffLine) {
  if (line.kind === "added" && typeof line.nextLineNumber === "number") {
    return { side: "next" as const, lineNumber: line.nextLineNumber, text: line.text };
  }

  if (line.kind === "removed" && typeof line.originalLineNumber === "number") {
    return { side: "original" as const, lineNumber: line.originalLineNumber, text: line.text };
  }

  return null;
}

/** 判断当前行是否为评论输入框绑定的选中行。 */
function isLineSelected(line: MarkdownDiffLine, selectedLine: { side: ReviewComment["lineSide"]; lineNumber: number } | null) {
  const anchor = getCommentAnchor(line);

  return Boolean(anchor && selectedLine && anchor.side === selectedLine.side && anchor.lineNumber === selectedLine.lineNumber);
}

/** 格式化评论锚点，避免把正文重复显示在紧凑控件里。 */
function formatLineLabel(side: ReviewComment["lineSide"], lineNumber: number) {
  return `${side === "next" ? "建议" : "原文"} L${lineNumber}`;
}

/** 格式化变更操作类型，避免 UI 暴露后端枚举名。 */
function formatOperationLabel(operation: ProposedChange["operation"]) {
  if (operation === "append") {
    return "文末追加";
  }

  if (operation === "multi_replace") {
    return "多处编辑";
  }

  return "局部替换";
}
