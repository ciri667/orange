import { Check, ChevronDown, ChevronRight, MessageSquarePlus, SendHorizontal, X } from "lucide-react";
import { useMemo, useState } from "react";
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
    <aside className="diff-panel review-workbench" aria-label="Agent 变更审阅工作台">
      <div className="diff-header review-header">
        <div className="review-title-block">
          <p className="section-label">{change.type === "create" ? "Agent 新建文件建议" : "Agent 文档变更审阅"}</p>
          <h3>{change.title}</h3>
          <span>{change.targetPath}</span>
        </div>
        <div className="diff-actions">
          <button className="ghost-button" type="button" onClick={onReject} disabled={isBusy}>
            <X size={16} />
            取消
          </button>
          <button className="primary-button compact" type="button" onClick={onAccept} disabled={isBusy}>
            <Check size={16} />
            确认写入
          </button>
        </div>
      </div>

      <div className="review-summary" aria-label="变更摘要">
        <span className="review-stat added">+{diff.stats.addedLines}</span>
        <span className="review-stat removed">-{diff.stats.removedLines}</span>
        <span>{diff.stats.hunkCount} 个变更区域</span>
        <span>{formatOperationLabel(change.operation)}</span>
        <span>{diff.stats.originalLineCount} 行 → {diff.stats.nextLineCount} 行</span>
        <span>hash 校验会在确认写入时执行</span>
      </div>

      <div className="review-body">
        <div className="unified-diff" aria-label="Markdown 行级 diff">
          <div className="unified-diff-file">
            <span>{change.targetPath}</span>
            <span>{change.type === "create" ? "new file" : "pending"}</span>
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

        <aside className="review-sidebar" aria-label="审阅评论">
          <div className="review-comment-composer">
            <div>
              <p className="section-label">行评论</p>
              <strong>{selectedLine ? formatLineLabel(selectedLine.side, selectedLine.lineNumber) : "选择一行变更"}</strong>
            </div>
            <textarea
              value={commentBody}
              onChange={(event) => setCommentBody(event.target.value)}
              placeholder="写下给 Agent 的具体修改意见"
              disabled={!selectedLine || isBusy}
            />
            <button className="ghost-button compact" type="button" onClick={handleAddComment} disabled={!selectedLine || !commentBody.trim() || isBusy}>
              <MessageSquarePlus size={15} />
              添加评论
            </button>
          </div>

          <div className="review-comment-list">
            <div className="review-comment-list-header">
              <p className="section-label">评论</p>
              <span>{draftCommentCount} 条待发送，{submittedCommentCount} 条已发送</span>
            </div>
            {comments.length ? (
              comments.map((comment) => (
                <article className={`review-comment status-${comment.status}`} key={comment.id}>
                  <span>{formatLineLabel(comment.lineSide, comment.lineNumber)}</span>
                  <p>{comment.body}</p>
                </article>
              ))
            ) : (
              <p className="review-comment-empty">点击变更行后添加具体反馈。</p>
            )}
          </div>

          <button className="primary-button compact" type="button" onClick={onSubmitComments} disabled={!draftCommentCount || isBusy}>
            <SendHorizontal size={15} />
            发送给 Agent 处理
          </button>
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
    <section className="diff-hunk">
      <button className="diff-hunk-header" type="button" onClick={onToggle}>
        {isCollapsed ? <ChevronRight size={14} /> : <ChevronDown size={14} />}
        <span>
          @@ -{hunk.oldStart || 0},{hunk.oldLines} +{hunk.newStart || 0},{hunk.newLines} @@
        </span>
        {hunkCommentCount > 0 && <em>{hunkCommentCount} 条评论</em>}
      </button>
      {!isCollapsed && (
        <div className="diff-lines">
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
      className={`diff-line line-${line.kind}${isSelected ? " selected" : ""}`}
      type="button"
      onClick={onSelect}
      disabled={!anchor}
      title={anchor ? "添加行评论" : undefined}
    >
      <span className="line-number old">{line.originalLineNumber ?? ""}</span>
      <span className="line-number new">{line.nextLineNumber ?? ""}</span>
      <span className="line-marker">{marker}</span>
      <code>{line.text || " "}</code>
      {commentCount > 0 && <span className="line-comment-count">{commentCount}</span>}
    </button>
  );
}

/** 折叠占位行只显示隐藏数量，不泄露正文内容。 */
function DiffPlaceholderLine({ hiddenCount }: { hiddenCount: number }) {
  return (
    <div className="diff-line line-placeholder">
      <span className="line-number old">…</span>
      <span className="line-number new">…</span>
      <span className="line-marker"> </span>
      <code>隐藏 {hiddenCount} 行未变更内容</code>
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
