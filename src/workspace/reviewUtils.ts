import { formatLocalDateTime } from "../shared/id";
import type { ProposedChange, ReviewComment, WorkspaceSnapshot } from "../shared/types";

/** 创建审阅状态摘要，避免各入口重复计算评论数量。 */
export function buildReviewState(comments: ReviewComment[], selected?: ReviewComment) {
  return {
    selectedCommentId: selected?.id,
    selectedLineSide: selected?.lineSide,
    selectedLineNumber: selected?.lineNumber,
    commentCount: comments.length,
    submittedCommentCount: comments.filter((comment) => comment.status === "submitted").length,
    updatedAt: formatLocalDateTime(),
  };
}

/** 更新当前会话的 pending diff；调用方负责决定是否持久化。 */
export function updateActivePendingChange(snapshot: WorkspaceSnapshot, nextChange: ProposedChange) {
  return {
    ...snapshot,
    sessions: snapshot.sessions.map((session) =>
      session.id === snapshot.activeSessionId
        ? {
            ...session,
            pendingChange: nextChange,
            updatedAt: formatLocalDateTime(),
          }
        : session,
    ),
  };
}

/** 生成发送给 Agent 的审阅反馈消息，包含行号和评论正文，但不进入诊断日志。 */
export function buildReviewFeedbackPrompt(change: ProposedChange, comments: ReviewComment[]) {
  const lines = comments.map((comment, index) => {
    const sideLabel = comment.lineSide === "next" ? "建议内容" : "原文";

    return `${index + 1}. ${sideLabel} L${comment.lineNumber}: ${comment.body}`;
  });

  return [
    `请根据我对「${change.title}」的逐行审阅反馈，重新生成待确认 diff。`,
    `目标路径：${change.targetPath}`,
    "审阅反馈：",
    ...lines,
    "保持未被评论的合理改动，仍然只生成待确认 diff，不要直接写入文件。",
  ].join("\n");
}
