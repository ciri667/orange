import { useEffect, useRef, useState } from "react";
import { decodeModelSelection } from "../shared/modelSelection";
import { logInfo } from "../shared/logger";
import { getActiveDocument, getActiveKnowledgeBase, getActiveNote } from "../shared/selectors";
import {
  applyAgentChangeSet,
  applySkillChangeSet,
  approveSkillExecution,
  listenAgentTurnProgress,
  loadAppEventLogs,
  loadRequestAuditLogs,
  rejectAgentChangeSet,
  rejectSkillChangeSet,
  rejectSkillExecution,
  runAgentTurn,
  saveSession,
} from "../shared/tauriApi";
import type { AgentActionType, AgentSession, AgentTurnProgressEvent, AppEventLog, RequestAuditLog, WorkspaceSnapshot } from "../shared/types";
import { formatLocalDateTime } from "../shared/id";
import {
  applyFirstPromptTitle,
  appendUserMessageToSession,
  buildAgentSession,
  buildDraftAgentSession,
  buildOptimisticUserMessage,
  buildTitleFromFirstPrompt,
  isPersistedSession,
  resolveActiveSessionForKnowledgeBase,
  shouldUseFirstPromptAsTitle,
} from "./sessionUtils";
import type { WorkspaceChrome } from "./workspaceChrome";

interface AgentTurnOptions extends WorkspaceChrome {
  agentPrompt: string;
  setAgentPrompt: (value: string) => void;
  turnModelSelection: string;
  explicitSkillIds: string[];
  setExplicitSkillIds: (value: string[]) => void;
  mentionedFileIds: string[];
  setMentionedFileIds: (value: string[]) => void;
  setAuditLogs: (logs: RequestAuditLog[]) => void;
  setAppEventLogs: (logs: AppEventLog[]) => void;
}

/** Agent 发送、排队 follow-up、Skill/变更集确认。 */
export function useAgentTurn(options: AgentTurnOptions) {
  const {
    snapshot,
    beginBusy,
    endBusy,
    setNotice,
    commitSnapshot,
    agentPrompt,
    setAgentPrompt,
    turnModelSelection,
    explicitSkillIds,
    setExplicitSkillIds,
    mentionedFileIds,
    setMentionedFileIds,
    setAuditLogs,
    setAppEventLogs,
  } = options;
  const [liveTurn, setLiveTurn] = useState<AgentTurnProgressEvent | null>(null);
  const liveTurnActiveRef = useRef(false);
  const queuedFollowUpRef = useRef<string | null>(null);
  const [queuedFollowUp, setQueuedFollowUp] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    void listenAgentTurnProgress((payload) => {
      if (!disposed && liveTurnActiveRef.current) {
        setLiveTurn(payload);
      }
    }).then((stop) => {
      if (disposed) {
        stop();
        return;
      }

      unlisten = stop;
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const noopAsync = async (..._args: unknown[]) => {};
  const noop = (..._args: unknown[]) => {};

  if (!snapshot) {
    return {
      liveTurn,
      queuedFollowUp,
      enqueueFollowUp: noop,
      takeQueuedFollowUp: () => null as string | null,
      handleClearQueuedFollowUp: noop,
      handleSubmitPrompt: noopAsync,
      handleApproveSkillExecution: noopAsync,
      handleRejectSkillExecution: noopAsync,
      handleApplySkillChangeSet: noopAsync,
      handleRejectSkillChangeSet: noopAsync,
      handleToggleSkillChangeOperation: noopAsync,
    };
  }

  const currentSnapshot = snapshot;
  const activeKnowledgeBase = getActiveKnowledgeBase(currentSnapshot);
  const activeNote = getActiveNote(currentSnapshot);
  const activeDocument = getActiveDocument(currentSnapshot);
  const persistedActiveSession = resolveActiveSessionForKnowledgeBase(currentSnapshot, activeKnowledgeBase);
  const activeSession = persistedActiveSession ?? buildDraftAgentSession(activeKnowledgeBase);


  /** busy 时最多接受一条 follow-up；成功入队后清空输入框，让用户看到消息已离开编辑区。 */
  function enqueueFollowUp(prompt: string) {
    if (queuedFollowUpRef.current) {
      setNotice("已有一条排队指令，请等当前回合结束。");
      return;
    }

    queuedFollowUpRef.current = prompt;
    setQueuedFollowUp(prompt);
    setAgentPrompt("");
    setNotice("当前回合结束后会处理下一条指令。");
  }

  /** 取出排队指令并清掉展示态；finally 里用返回值再开一轮，避免重复发送。 */
  function takeQueuedFollowUp() {
    const prompt = queuedFollowUpRef.current;

    queuedFollowUpRef.current = null;
    setQueuedFollowUp(null);
    return prompt;
  }

  /** 用户取消尚未进入模型的排队指令，不影响当前正在跑的回合。 */
  function handleClearQueuedFollowUp() {
    if (!queuedFollowUpRef.current) {
      return;
    }

    queuedFollowUpRef.current = null;
    setQueuedFollowUp(null);
    setNotice("已取消排队指令。");
  }

  /** 提交 Agent 输入，运行时会自行决定是否调用检索工具。 */
  async function handleSubmitPrompt(action: AgentActionType = "ask", presetPrompt?: string, sourceSnapshot = currentSnapshot) {
    const prompt = (presetPrompt ?? agentPrompt).trim();
    const turnExplicitSkillIds = presetPrompt ? [] : explicitSkillIds;
    // 预设操作不继承输入框里的 @ 文件，避免审阅等系统操作意外携带上一轮材料。
    const turnMentionedFileIds = presetPrompt ? [] : mentionedFileIds;
    const sourceActiveSession = sourceSnapshot.sessions.find((session) => session.id === sourceSnapshot.activeSessionId) ?? activeSession;
    const sourceActiveKnowledgeBase =
      sourceSnapshot.knowledgeBases.find((knowledgeBase) => knowledgeBase.id === sourceSnapshot.activeKnowledgeBaseId) ?? activeKnowledgeBase;
    const sourceActiveNote = sourceSnapshot.notes.find((note) => note.id === sourceSnapshot.activeNoteId) ?? activeNote;
    const sourceActiveDocument =
      sourceSnapshot.documents.find((document) => document.id === sourceSnapshot.activeDocumentId) ?? activeDocument;

    // 空输入不创建消息，避免侧栏出现无意义的对话记录。
    if (!prompt) {
      return;
    }

    if (liveTurnActiveRef.current && !presetPrompt) {
      enqueueFollowUp(prompt);
      return;
    }

    const optimisticMessage = buildOptimisticUserMessage(prompt, action, turnMentionedFileIds);
    const promptBeforeSubmit = agentPrompt;
    let didPersistOptimisticMessage = false;
    // 排队的下一条必须带着本轮已经提交的快照；闭包里的 currentSnapshot 仍是点发送时的旧值。
    let latestSnapshot = sourceSnapshot;

    liveTurnActiveRef.current = true;
    setLiveTurn(null);
    beginBusy("Agent 正在处理...");

    try {
      let snapshotForTurn = sourceSnapshot;
      let sessionForTurn = sourceActiveSession;

      if (!isPersistedSession(sourceSnapshot, sourceActiveSession)) {
        sessionForTurn = buildAgentSession({
          knowledgeBase: sourceActiveKnowledgeBase,
          title: buildTitleFromFirstPrompt(prompt),
        });
        snapshotForTurn = {
          ...sourceSnapshot,
          sessions: [sessionForTurn, ...sourceSnapshot.sessions],
          activeSessionId: sessionForTurn.id,
        };
        logInfo("准备创建草稿会话。", {
          category: "frontend",
          event: "bootstrap_session",
          status: "started",
          metadata: {
            knowledgeBaseId: sourceActiveKnowledgeBase.id,
            promptLength: prompt.length,
            explicitSkillCount: turnExplicitSkillIds.length,
          },
        });
      } else if (shouldUseFirstPromptAsTitle(sourceActiveSession)) {
        const titled = applyFirstPromptTitle(sourceSnapshot, sourceActiveSession, prompt);

        sessionForTurn = titled.session;
        snapshotForTurn = titled.snapshot;
        logInfo("会话标题已由首条输入确定。", {
          category: "frontend",
          event: "title_session",
          status: "completed",
          metadata: {
            knowledgeBaseId: sourceActiveKnowledgeBase.id,
            promptLength: prompt.length,
            explicitSkillCount: turnExplicitSkillIds.length,
          },
        });
      }

      const optimisticTurn = appendUserMessageToSession(snapshotForTurn, sessionForTurn, optimisticMessage);

      sessionForTurn = optimisticTurn.session;
      snapshotForTurn = optimisticTurn.snapshot;
      // 先提交本地快照，让用户发送的消息立即出现在对话框中，再等待 Agent 慢任务。
      commitSnapshot(snapshotForTurn);
      latestSnapshot = snapshotForTurn;
      setAgentPrompt("");
      // 消息已携带引用 ID，发送后清空输入态；请求失败时会在 catch 中恢复，方便重试。
      setMentionedFileIds([]);
      snapshotForTurn = await saveSession(snapshotForTurn, sessionForTurn);
      latestSnapshot = snapshotForTurn;
      didPersistOptimisticMessage = true;
      logInfo("用户消息已乐观落库。", {
        category: "frontend",
        event: "persist_user_message",
        status: "completed",
        metadata: {
          knowledgeBaseId: activeKnowledgeBase.id,
          sessionId: sessionForTurn.id,
          promptLength: prompt.length,
          explicitSkillCount: turnExplicitSkillIds.length,
        },
      });

      const turnSnapshot = {
        ...snapshotForTurn,
        activeSessionId: sessionForTurn.id,
        activeKnowledgeBaseId: sourceActiveKnowledgeBase.id,
        activeNoteId: sourceActiveNote?.id ?? "",
        activeDocumentId: sourceActiveDocument?.id ?? "",
      };
      const decodedTurnModelSelection = decodeModelSelection(turnModelSelection);
      const result = await runAgentTurn(
        turnSnapshot,
        prompt,
        action,
        optimisticMessage.id,
        decodedTurnModelSelection.providerId || undefined,
        decodedTurnModelSelection.modelId || undefined,
        turnExplicitSkillIds,
        turnMentionedFileIds,
      );

      commitSnapshot(result.snapshot);
      latestSnapshot = result.snapshot;
      setLiveTurn(null);
      if (!presetPrompt) {
        setExplicitSkillIds([]);
      }
      const [nextAuditLogs, nextAppEventLogs] = await Promise.all([loadRequestAuditLogs(), loadAppEventLogs()]);

      setAuditLogs(nextAuditLogs);
      setAppEventLogs(nextAppEventLogs);
    } catch (error) {
      if (!presetPrompt) {
        setMentionedFileIds(turnMentionedFileIds);
      }
      if (!didPersistOptimisticMessage) {
        commitSnapshot(sourceSnapshot);
        latestSnapshot = sourceSnapshot;
        setAgentPrompt(promptBeforeSubmit);
      }
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      liveTurnActiveRef.current = false;
      setLiveTurn(null);
      endBusy();
      const queuedPrompt = takeQueuedFollowUp();
      if (queuedPrompt) {
        void handleSubmitPrompt("ask", queuedPrompt, latestSnapshot);
      }
    }
  }

  /** 打开设置抽屉时刷新非阻塞诊断信息，避免展示过旧的日志列表。 */
  async function handleApproveSkillExecution() {
    beginBusy("正在隔离区运行 Skill...");
    try {
      const nextSnapshot = await approveSkillExecution(currentSnapshot);
      commitSnapshot(nextSnapshot);
      setNotice(nextSnapshot.sessions.find((session) => session.id === nextSnapshot.activeSessionId)?.pendingChangeSet?.summary ?? "Skill 执行完成。");
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 拒绝待审批执行，明确不创建工作区、不启动进程。 */
  async function handleRejectSkillExecution() {
    beginBusy("正在拒绝 Skill 执行...");
    try {
      commitSnapshot(await rejectSkillExecution(currentSnapshot));
      setNotice("已拒绝 Skill 执行。");
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 应用 Skill 多文件变更集，完成后使用后端重扫结果替换当前快照。 */
  async function handleApplySkillChangeSet() {
    const isAgentChangeSet = activeSession.pendingChangeSet?.executionId === "agent-direct";
    beginBusy(isAgentChangeSet ? "正在应用 Agent 文件变更..." : "正在应用 Skill 文件变更...");
    try {
      const nextSnapshot = isAgentChangeSet
        ? await applyAgentChangeSet(currentSnapshot)
        : await applySkillChangeSet(currentSnapshot);
      commitSnapshot(nextSnapshot, new Set(), new Set());
      setNotice(isAgentChangeSet ? "已应用 Agent 文件变更集。" : "已应用 Skill 文件变更集。");
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 拒绝 Skill 或 Agent 多文件变更集；Agent 变更集只清空待确认状态，不触碰 Skill 隔离目录。 */
  async function handleRejectSkillChangeSet() {
    const isAgentChangeSet = activeSession.pendingChangeSet?.executionId === "agent-direct";
    beginBusy(isAgentChangeSet ? "正在拒绝 Agent 文件变更..." : "正在拒绝 Skill 文件变更...");
    try {
      const nextSnapshot = isAgentChangeSet
        ? await rejectAgentChangeSet(currentSnapshot)
        : await rejectSkillChangeSet(currentSnapshot);
      commitSnapshot(nextSnapshot);
      setNotice(isAgentChangeSet ? "已拒绝 Agent 文件变更集。" : "已拒绝 Skill 文件变更集。");
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 更新变更集中的单文件选择状态，并立即持久化以支持重启后继续审阅。 */
  async function handleToggleSkillChangeOperation(operationId: string, selected: boolean) {
    if (!activeSession.pendingChangeSet) {
      return;
    }
    const nextSession: AgentSession = {
      ...activeSession,
      pendingChangeSet: {
        ...activeSession.pendingChangeSet,
        operations: activeSession.pendingChangeSet.operations.map((operation) =>
          operation.id === operationId ? { ...operation, selected } : operation,
        ),
      },
      updatedAt: formatLocalDateTime(),
    };
    const nextSnapshot = {
      ...currentSnapshot,
      sessions: currentSnapshot.sessions.map((session) => session.id === activeSession.id ? nextSession : session),
    };
    try {
      commitSnapshot(await saveSession(nextSnapshot, nextSession));
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    }
  }

  return {
    liveTurn,
    queuedFollowUp,
    enqueueFollowUp,
    takeQueuedFollowUp,
    handleClearQueuedFollowUp,
    handleSubmitPrompt,
    handleApproveSkillExecution,
    handleRejectSkillExecution,
    handleApplySkillChangeSet,
    handleRejectSkillChangeSet,
    handleToggleSkillChangeOperation,
  };
}
