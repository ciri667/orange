import type { ReviewCommentDraft } from "../diff/DiffPanel";
import { buildMarkdownDiff } from "../diff/markdownDiff";
import { createLocalId, formatLocalDateTime } from "../shared/id";
import { logInfo, logWarn } from "../shared/logger";
import { getActiveDocument, getActiveKnowledgeBase, getActiveNote } from "../shared/selectors";
import { acceptProposedChange, loadAppEventLogs, rejectProposedChange, saveSession } from "../shared/tauriApi";
import type { AgentActionType, AppEventLog, ProposedChange, ReviewComment, WorkspaceSnapshot } from "../shared/types";
import { buildReviewFeedbackPrompt, buildReviewState, updateActivePendingChange } from "./reviewUtils";
import { buildDraftAgentSession, isPersistedSession, resolveActiveSessionForKnowledgeBase } from "./sessionUtils";
import type { WorkspaceChrome } from "./workspaceChrome";

interface ReviewActionsOptions extends WorkspaceChrome {
  setAppEventLogs: (logs: AppEventLog[]) => void;
  handleSubmitPrompt: (action?: AgentActionType, presetPrompt?: string, sourceSnapshot?: WorkspaceSnapshot) => Promise<void>;
}

const noopAsync = async (..._args: unknown[]) => {};

/** Diff 审阅评论与确认/拒绝写入。 */
export function useReviewActions(options: ReviewActionsOptions) {
  const { snapshot, beginBusy, endBusy, setNotice, commitSnapshot, setAppEventLogs, handleSubmitPrompt } = options;

  if (!snapshot) {
    return {
      handleAddReviewComment: noopAsync,
      handleSubmitReviewComments: noopAsync,
      handleAcceptChange: noopAsync,
      handleRejectChange: noopAsync,
    };
  }

  const currentSnapshot = snapshot;
  const activeKnowledgeBase = getActiveKnowledgeBase(currentSnapshot);
  const activeNote = getActiveNote(currentSnapshot);
  const activeDocument = getActiveDocument(currentSnapshot);
  const persistedActiveSession = resolveActiveSessionForKnowledgeBase(currentSnapshot, activeKnowledgeBase);
  const activeSession = persistedActiveSession ?? buildDraftAgentSession(activeKnowledgeBase);


  /** 为当前待写入 diff 添加行评论；日志只记录行号、侧别和计数，不记录评论正文。 */
  async function handleAddReviewComment(commentDraft: ReviewCommentDraft) {
    const pendingChange = activeSession.pendingChange;

    if (!pendingChange || pendingChange.status !== "pending" || !isPersistedSession(currentSnapshot, activeSession)) {
      return;
    }

    const nextComment: ReviewComment = {
      id: createLocalId("review-comment"),
      changeId: pendingChange.id,
      lineSide: commentDraft.lineSide,
      lineNumber: commentDraft.lineNumber,
      lineTextPreview: commentDraft.lineTextPreview,
      body: commentDraft.body,
      status: "draft",
      createdAt: formatLocalDateTime(),
    };
    const nextComments = [...(pendingChange.reviewComments ?? []), nextComment];
    const nextChange: ProposedChange = {
      ...pendingChange,
      reviewComments: nextComments,
      reviewState: buildReviewState(nextComments, nextComment),
      diffStats: pendingChange.diffStats ?? buildMarkdownDiff(pendingChange.original, pendingChange.next).stats,
    };
    const nextSession = {
      ...activeSession,
      pendingChange: nextChange,
      updatedAt: formatLocalDateTime(),
    };
    const nextSnapshot = updateActivePendingChange(currentSnapshot, nextChange);

    commitSnapshot(nextSnapshot);
    logInfo("添加 diff 行评论。", {
      category: "frontend",
      event: "review_comment_add",
      status: "completed",
      metadata: {
        changeId: pendingChange.id,
        sessionId: activeSession.id,
        lineSide: commentDraft.lineSide,
        lineNumber: commentDraft.lineNumber,
        commentCount: nextComments.length,
      },
    });

    try {
      commitSnapshot(await saveSession(nextSnapshot, nextSession));
    } catch (error) {
      logWarn("保存 diff 行评论失败。", {
        category: "frontend",
        event: "review_comment_save",
        status: "failed",
        error,
        metadata: {
          changeId: pendingChange.id,
          sessionId: activeSession.id,
        },
      });
      setNotice(error instanceof Error ? error.message : String(error));
    }
  }

  /** 把待发送审阅评论转成用户消息，让 Agent 基于定位反馈重新生成 pending diff。 */
  async function handleSubmitReviewComments() {
    const pendingChange = activeSession.pendingChange;
    const draftComments = pendingChange?.reviewComments?.filter((comment) => comment.status === "draft") ?? [];

    if (!pendingChange || pendingChange.status !== "pending" || !draftComments.length || !isPersistedSession(currentSnapshot, activeSession)) {
      return;
    }

    const submittedAt = formatLocalDateTime();
    const nextComments = (pendingChange.reviewComments ?? []).map((comment) =>
      comment.status === "draft" ? { ...comment, status: "submitted" as const, createdAt: comment.createdAt || submittedAt } : comment,
    );
    const nextChange: ProposedChange = {
      ...pendingChange,
      reviewComments: nextComments,
      reviewState: buildReviewState(nextComments),
      diffStats: pendingChange.diffStats ?? buildMarkdownDiff(pendingChange.original, pendingChange.next).stats,
    };
    const nextSession = {
      ...activeSession,
      pendingChange: nextChange,
      updatedAt: submittedAt,
    };
    const nextSnapshot = updateActivePendingChange(currentSnapshot, nextChange);

    commitSnapshot(nextSnapshot);
    logInfo("提交 diff 审阅评论给 Agent。", {
      category: "frontend",
      event: "review_comments_submit",
      status: "started",
      metadata: {
        changeId: pendingChange.id,
        sessionId: activeSession.id,
        commentCount: draftComments.length,
      },
    });

    try {
      const savedSnapshot = await saveSession(nextSnapshot, nextSession);

      commitSnapshot(savedSnapshot);
      await handleSubmitPrompt(
        pendingChange.type === "create" ? "create" : "rewrite",
        buildReviewFeedbackPrompt(pendingChange, draftComments),
        savedSnapshot,
      );
      logInfo("diff 审阅评论已交给 Agent。", {
        category: "frontend",
        event: "review_comments_submit",
        status: "completed",
        metadata: {
          changeId: pendingChange.id,
          sessionId: activeSession.id,
          commentCount: draftComments.length,
        },
      });
    } catch (error) {
      logWarn("提交 diff 审阅评论失败。", {
        category: "frontend",
        event: "review_comments_submit",
        status: "failed",
        error,
        metadata: {
          changeId: pendingChange.id,
          sessionId: activeSession.id,
        },
      });
      setNotice(error instanceof Error ? error.message : String(error));
    }
  }

  /** 接受 Agent diff，真实桌面版会在 Tauri 层做路径、hash 和原子写入校验。 */
  async function handleAcceptChange() {
    const pendingChange = activeSession.pendingChange;

    if (pendingChange) {
      logInfo("准备确认写入审阅 diff。", {
        category: "frontend",
        event: "review_change_accept",
        status: "started",
        metadata: {
          changeId: pendingChange.id,
          sessionId: activeSession.id,
          changeType: pendingChange.type,
          operation: pendingChange.operation ?? "replace",
          commentCount: pendingChange.reviewComments?.length ?? 0,
        },
      });
    }

    beginBusy("正在应用 diff...");

    try {
      const nextSnapshot = await acceptProposedChange(currentSnapshot);

      commitSnapshot(nextSnapshot);
      const nextAppEventLogs = await loadAppEventLogs();

      setAppEventLogs(nextAppEventLogs);
      setNotice("已应用本次 diff。");
    } catch (error) {
      if (pendingChange) {
        logWarn("确认写入审阅 diff 失败。", {
          category: "frontend",
          event: "review_change_accept",
          status: "failed",
          error,
          metadata: {
            changeId: pendingChange.id,
            sessionId: activeSession.id,
            operation: pendingChange.operation ?? "replace",
          },
        });
      }
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 取消 Agent diff，保持原始 Markdown 内容不变。 */
  async function handleRejectChange() {
    const pendingChange = activeSession.pendingChange;

    if (pendingChange) {
      logInfo("准备取消审阅 diff。", {
        category: "frontend",
        event: "review_change_reject",
        status: "started",
        metadata: {
          changeId: pendingChange.id,
          sessionId: activeSession.id,
          changeType: pendingChange.type,
          operation: pendingChange.operation ?? "replace",
          commentCount: pendingChange.reviewComments?.length ?? 0,
        },
      });
    }

    beginBusy("正在取消 diff...");

    try {
      commitSnapshot(await rejectProposedChange(currentSnapshot));
      const nextAppEventLogs = await loadAppEventLogs();

      setAppEventLogs(nextAppEventLogs);
      setNotice("已取消本次 diff。");
    } catch (error) {
      if (pendingChange) {
        logWarn("取消审阅 diff 失败。", {
          category: "frontend",
          event: "review_change_reject",
          status: "failed",
          error,
          metadata: {
            changeId: pendingChange.id,
            sessionId: activeSession.id,
            operation: pendingChange.operation ?? "replace",
          },
        });
      }
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  return {
    handleAddReviewComment,
    handleSubmitReviewComments,
    handleAcceptChange,
    handleRejectChange,
  };
}
