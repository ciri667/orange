import { useEffect, useRef } from "react";
import { logInfo } from "../shared/logger";
import type { WorkspaceSnapshot } from "../shared/types";

/** 记录 pending diff 首次打开事件；只上报 ID、类型和统计，不记录正文或本地路径。 */
export function useReviewChangeLogger(snapshot: WorkspaceSnapshot | null) {
  /** 最近一次已上报打开事件的 changeId，避免保存评论等快照刷新造成日志噪声。 */
  const loggedReviewOpenChangeIdRef = useRef("");

  useEffect(() => {
    const pendingChange = snapshot?.sessions
      .find((session) => session.id === snapshot.activeSessionId)
      ?.pendingChange;

    if (!pendingChange || pendingChange.status !== "pending") {
      return;
    }

    if (loggedReviewOpenChangeIdRef.current === pendingChange.id) {
      return;
    }

    loggedReviewOpenChangeIdRef.current = pendingChange.id;
    logInfo("打开 diff 审阅工作台。", {
      category: "frontend",
      event: "review_change_open",
      status: "completed",
      metadata: {
        changeId: pendingChange.id,
        sessionId: snapshot.activeSessionId,
        changeType: pendingChange.type,
        commentCount: pendingChange.reviewComments?.length ?? 0,
        addedLines: pendingChange.diffStats?.addedLines,
        removedLines: pendingChange.diffStats?.removedLines,
      },
    });
  }, [snapshot?.activeSessionId, snapshot?.sessions]);
}
