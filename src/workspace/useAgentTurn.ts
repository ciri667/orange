import { useEffect, useRef, useState } from "react";
import { decodeModelSelection } from "../shared/modelSelection";
import { logInfo } from "../shared/logger";
import { getActiveDocument, getActiveKnowledgeBase, getActiveNote } from "../shared/selectors";
import {
  applyAgentChangeSet,
  applySkillChangeSet,
  approveSkillExecution,
  abortAgentTurn,
  clearAgentTurnPlaceholder,
  listenAgentTurnProgress,
  listActiveAgentSessionIds,
  loadAppEventLogs,
  loadRequestAuditLogs,
  rejectAgentChangeSet,
  rejectSkillChangeSet,
  rejectSkillExecution,
  loadSessions,
  rewindAgentSession,
  runAgentTurn,
  saveSession,
} from "../shared/tauriApi";
import type { AgentActionType, AgentMessage, AgentSession, AgentTurnProgressEvent, AppEventLog, RequestAuditLog, WorkspaceSnapshot } from "../shared/types";
import { formatLocalDateTime } from "../shared/id";
import {
  applyFirstPromptTitle,
  appendUserMessageToSession,
  buildAgentSession,
  buildDraftAgentSession,
  buildOptimisticUserMessage,
  buildTitleFromFirstPrompt,
  collectTouchedFileIds,
  isPersistedSession,
  mergeSessionTurn,
  removeSessionMessage,
  resolveActiveSessionForKnowledgeBase,
  MAX_PARALLEL_AGENT_TURNS,
  resolveAgentTurnStart,
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
  dirtyNoteIds: Set<string>;
  dirtyDocumentIds: Set<string>;
}

/** 绑定到某个会话的排队指令；跨会话发送时会先乐观落库，同会话 follow-up 仍只留在界面。 */
interface QueuedFollowUp {
  sessionId: string;
  prompt: string;
  action: AgentActionType;
  modelSelection: string;
  explicitSkillIds: string[];
  mentionedFileIds: string[];
  clientMessageId?: string;
}

/** Agent 发送、按会话排队 follow-up、Skill/变更集确认。 */
export function useAgentTurn(options: AgentTurnOptions) {
  const {
    snapshot,
    beginBusy,
    endBusy,
    setNotice,
    commitSnapshot,
    requestConfirmation,
    agentPrompt,
    setAgentPrompt,
    turnModelSelection,
    explicitSkillIds,
    setExplicitSkillIds,
    mentionedFileIds,
    setMentionedFileIds,
    setAuditLogs,
    setAppEventLogs,
    dirtyNoteIds,
    dirtyDocumentIds,
  } = options;
  const [liveTurns, setLiveTurns] = useState<Record<string, AgentTurnProgressEvent>>({});
  const inFlightSessionIdsRef = useRef(new Set<string>());
  const [inFlightSessionIds, setInFlightSessionIds] = useState<string[]>([]);
  const observedRunningSessionIdsRef = useRef(new Set<string>());
  const [observedRunningSessionIds, setObservedRunningSessionIds] = useState<string[]>([]);
  const queuedBySessionRef = useRef(new Map<string, QueuedFollowUp>());
  const [queuedBySession, setQueuedBySession] = useState<Record<string, QueuedFollowUp>>({});
  const hydrateExternalSessionRef = useRef<(sessionId: string) => Promise<void>>(async () => {});
  const snapshotRef = useRef(snapshot);
  const dirtyNoteIdsRef = useRef(dirtyNoteIds);
  const dirtyDocumentIdsRef = useRef(dirtyDocumentIds);
  const submitPromptRef = useRef<(
    action?: AgentActionType,
    presetPrompt?: string,
    sourceSnapshot?: WorkspaceSnapshot,
    replay?: QueuedFollowUp,
  ) => Promise<void>>(async () => {});

  snapshotRef.current = snapshot;
  dirtyNoteIdsRef.current = dirtyNoteIds;
  dirtyDocumentIdsRef.current = dirtyDocumentIds;

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    void listenAgentTurnProgress((payload) => {
      if (disposed) {
        return;
      }

      setLiveTurns((current) => ({ ...current, [payload.sessionId]: payload }));
      const owned = inFlightSessionIdsRef.current.has(payload.sessionId);
      if (payload.status === "running") {
        if (!owned && !observedRunningSessionIdsRef.current.has(payload.sessionId)) {
          observedRunningSessionIdsRef.current.add(payload.sessionId);
          setObservedRunningSessionIds(Array.from(observedRunningSessionIdsRef.current));
        }
        return;
      }

      if (observedRunningSessionIdsRef.current.delete(payload.sessionId)) {
        setObservedRunningSessionIds(Array.from(observedRunningSessionIdsRef.current));
      }
      if (!owned) {
        void hydrateExternalSessionRef.current(payload.sessionId);
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

  useEffect(() => {
    let disposed = false;

    async function syncObservedRunningSessions() {
      try {
        const activeIds = await listActiveAgentSessionIds();
        if (disposed) {
          return;
        }

        const nextObserved = new Set<string>();
        for (const sessionId of activeIds) {
          if (!inFlightSessionIdsRef.current.has(sessionId)) {
            nextObserved.add(sessionId);
          }
        }
        observedRunningSessionIdsRef.current = nextObserved;
        setObservedRunningSessionIds(Array.from(nextObserved));
        setLiveTurns((current) => {
          const next = { ...current };
          for (const sessionId of Object.keys(next)) {
            if (
              !inFlightSessionIdsRef.current.has(sessionId) &&
              !nextObserved.has(sessionId) &&
              next[sessionId]?.status !== "running"
            ) {
              delete next[sessionId];
            }
          }
          return next;
        });
      } catch {
        return;
      }
    }

    void syncObservedRunningSessions();
    const timer = window.setInterval(() => {
      void syncObservedRunningSessions();
    }, 2000);

    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, []);

  const noopAsync = async (..._args: unknown[]) => {};
  const noop = (..._args: unknown[]) => {};
  const activeSessionId = snapshot?.activeSessionId ?? "";
  const liveTurn = liveTurns[activeSessionId] ?? null;
  const activeQueuedFollowUp = queuedBySession[activeSessionId] ?? null;
  const queuedFollowUp = activeQueuedFollowUp?.prompt ?? null;
  const queuedFollowUpInList = activeQueuedFollowUp && !activeQueuedFollowUp.clientMessageId ? activeQueuedFollowUp.prompt : null;
  const runningSessionIds = Array.from(new Set([...inFlightSessionIds, ...observedRunningSessionIds]));
  const isCurrentSessionBusy =
    runningSessionIds.includes(activeSessionId) || Boolean(activeQueuedFollowUp) || liveTurn?.status === "running";

  /** 同步进行中的会话集合，让当前会话的输入条立即进入忙碌态。 */
  function syncInFlightSessionIds() {
    setInFlightSessionIds(Array.from(inFlightSessionIdsRef.current));
  }

  if (!snapshot) {
    return {
      liveTurn,
      queuedFollowUp,
      queuedFollowUpInList,
      isCurrentSessionBusy,
      inFlightSessionIds: runningSessionIds,
      queuedSessionIds: Object.keys(queuedBySession),
      enqueueFollowUp: noop,
      takeQueuedFollowUp: () => null as string | null,
      handleClearQueuedFollowUp: noop,
      handleAbortTurn: noopAsync,
      handlePrepareSessionForDelete: noopAsync,
      handleSubmitPrompt: noopAsync,
      handleEditUserMessageAndRerun: noopAsync,
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

  /** 把 ref 中的排队表同步到展示态，供当前会话过滤气泡和输入条。 */
  function syncQueuedFollowUps() {
    setQueuedBySession(Object.fromEntries(queuedBySessionRef.current));
  }

  /** 立即提交工作台快照，并同步 ref，避免回合结束时读到切会话前的旧列表。 */
  function commitTurnSnapshot(
    nextSnapshot: WorkspaceSnapshot,
    dirtyNotesToKeep?: Set<string>,
    dirtyDocumentsToKeep?: Set<string>,
  ) {
    snapshotRef.current = nextSnapshot;
    commitSnapshot(nextSnapshot, dirtyNotesToKeep, dirtyDocumentsToKeep);
  }

  /** 当前会话已在跑或并行已满时入队；每个会话最多一条。 */
  function enqueueFollowUp(queued: QueuedFollowUp, reason: "same-session" | "capacity" = "same-session") {
    if (queuedBySessionRef.current.has(queued.sessionId)) {
      setNotice("已有一条排队指令，请等当前回合结束。");
      return false;
    }

    queuedBySessionRef.current.set(queued.sessionId, queued);
    syncQueuedFollowUps();
    if (!queued.clientMessageId) {
      setAgentPrompt("");
    }
    setNotice(
      reason === "capacity"
        ? "已有 3 个任务在运行，结束后会开始这条。"
        : "当前回合结束后会处理下一条指令。",
    );
    return true;
  }

  /** 优先取出刚结束会话的排队，否则按入队顺序取下一个会话。 */
  function takeNextQueuedFollowUp(preferSessionId: string) {
    const preferred = queuedBySessionRef.current.get(preferSessionId);
    if (preferred) {
      queuedBySessionRef.current.delete(preferSessionId);
      syncQueuedFollowUps();
      return preferred;
    }

    const nextEntry = queuedBySessionRef.current.entries().next();
    if (nextEntry.done) {
      return null;
    }

    queuedBySessionRef.current.delete(nextEntry.value[0]);
    syncQueuedFollowUps();
    return nextEntry.value[1];
  }

  hydrateExternalSessionRef.current = async (sessionId: string) => {
    const sourceSnapshot = snapshotRef.current;
    if (!sourceSnapshot) {
      return;
    }

    try {
      const loadedSessions = await loadSessions(sourceSnapshot);
      const incoming = loadedSessions.find((session) => session.id === sessionId);
      const latest = snapshotRef.current ?? sourceSnapshot;
      if (incoming) {
        const sessions = latest.sessions.some((session) => session.id === sessionId)
          ? latest.sessions.map((session) => (session.id === sessionId ? incoming : session))
          : [incoming, ...latest.sessions];
        commitTurnSnapshot({ ...latest, sessions });
      }
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    }

    setLiveTurns((current) => {
      const next = { ...current };
      delete next[sessionId];
      return next;
    });

    const queued = takeNextQueuedFollowUp(sessionId);
    if (queued && resolveAgentTurnStart(queued.sessionId, [
      ...inFlightSessionIdsRef.current,
      ...observedRunningSessionIdsRef.current,
    ]) === "start") {
      void submitPromptRef.current(queued.action, queued.prompt, snapshotRef.current ?? sourceSnapshot, queued);
    } else if (queued) {
      queuedBySessionRef.current.set(queued.sessionId, queued);
      syncQueuedFollowUps();
    }
  };

  /** 用户取消尚未进入模型的排队指令，不影响当前正在跑的回合。 */
  async function handleClearQueuedFollowUp() {
    const queued = queuedBySessionRef.current.get(activeSession.id);
    if (!queued) {
      return;
    }

    queuedBySessionRef.current.delete(activeSession.id);
    syncQueuedFollowUps();

    if (queued.clientMessageId && snapshotRef.current) {
      const removed = removeSessionMessage(snapshotRef.current, queued.sessionId, queued.clientMessageId);
      if (removed.session) {
        try {
          commitTurnSnapshot(await saveSession(removed.snapshot, removed.session));
        } catch (error) {
          setNotice(error instanceof Error ? error.message : String(error));
          return;
        }
      }
    }

    setNotice("已取消排队指令。");
  }

  /** 中断当前会话正在跑的回合，并取消尚未进入模型的排队指令。 */
  async function handleAbortTurn() {
    const sessionId = activeSession.id;
    const queued = queuedBySessionRef.current.get(sessionId);
    if (queued) {
      queuedBySessionRef.current.delete(sessionId);
      syncQueuedFollowUps();
      if (queued.clientMessageId && snapshotRef.current) {
        const removed = removeSessionMessage(snapshotRef.current, queued.sessionId, queued.clientMessageId);
        if (removed.session) {
          try {
            commitTurnSnapshot(await saveSession(removed.snapshot, removed.session));
          } catch (error) {
            setNotice(error instanceof Error ? error.message : String(error));
          }
        }
      }
    }

    if (!inFlightSessionIdsRef.current.has(sessionId)) {
      if (queued) {
        setNotice("已取消排队指令。");
      }
      return;
    }

    try {
      await abortAgentTurn(sessionId);
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    }
  }

  /** 删除会话前先丢掉排队并停止该会话回合，避免回合结束又把会话 merge 回来。 */
  async function handlePrepareSessionForDelete(sessionId: string) {
    dropQueuedFollowUp(sessionId);
    let backendActive = false;
    try {
      backendActive = (await listActiveAgentSessionIds()).includes(sessionId);
    } catch {
      backendActive = false;
    }
    if (!inFlightSessionIdsRef.current.has(sessionId) && !backendActive) {
      return;
    }

    await abortAgentTurn(sessionId);
    await waitForSessionIdle(sessionId);
  }

  /** 提交 Agent 输入；对话线程始终绑定目标会话，不借用正在跑的其它会话。 */
  async function handleSubmitPrompt(
    action: AgentActionType = "ask",
    presetPrompt?: string,
    sourceSnapshot = currentSnapshot,
    replay?: QueuedFollowUp,
  ) {
    const prompt = (presetPrompt ?? replay?.prompt ?? agentPrompt).trim();
    const turnExplicitSkillIds = replay?.explicitSkillIds ?? (presetPrompt ? [] : explicitSkillIds);
    const turnMentionedFileIds = replay?.mentionedFileIds ?? (presetPrompt ? [] : mentionedFileIds);
    const turnModelSelectionForRun = replay?.modelSelection ?? turnModelSelection;
    const turnAction = replay?.action ?? action;
    const sourceActiveKnowledgeBase =
      sourceSnapshot.knowledgeBases.find((knowledgeBase) => knowledgeBase.id === sourceSnapshot.activeKnowledgeBaseId) ??
      activeKnowledgeBase;
    const sourceActiveNote = sourceSnapshot.notes.find((note) => note.id === sourceSnapshot.activeNoteId) ?? activeNote;
    const sourceActiveDocument =
      sourceSnapshot.documents.find((document) => document.id === sourceSnapshot.activeDocumentId) ?? activeDocument;
    const requestedSessionId = replay?.sessionId ?? sourceSnapshot.activeSessionId;
    const sourceActiveSession =
      sourceSnapshot.sessions.find((session) => session.id === requestedSessionId) ??
      sourceSnapshot.sessions.find((session) => session.id === sourceSnapshot.activeSessionId) ??
      activeSession;

    if (!prompt) {
      return;
    }

    if (!replay) {
      const startDecision = resolveAgentTurnStart(sourceActiveSession.id, [
        ...inFlightSessionIdsRef.current,
        ...observedRunningSessionIdsRef.current,
      ]);
      if (startDecision === "queue-same-session") {
        enqueueFollowUp({
          sessionId: sourceActiveSession.id,
          prompt,
          action: turnAction,
          modelSelection: turnModelSelectionForRun,
          explicitSkillIds: turnExplicitSkillIds,
          mentionedFileIds: turnMentionedFileIds,
        });
        return;
      }
    }

    const optimisticMessage = replay?.clientMessageId
      ? sourceActiveSession.messages.find((message) => message.id === replay.clientMessageId) ??
        buildOptimisticUserMessage(prompt, turnAction, turnMentionedFileIds)
      : buildOptimisticUserMessage(prompt, turnAction, turnMentionedFileIds);
    const promptBeforeSubmit = agentPrompt;
    let didPersistOptimisticMessage = Boolean(replay?.clientMessageId);
    let latestSnapshot = sourceSnapshot;
    let sessionForTurn = sourceActiveSession;
    let snapshotForTurn = sourceSnapshot;
    let startedTurn = false;
    let reservedSessionId: string | null = null;
    const viewerSessionId = snapshotRef.current?.activeSessionId ?? sourceSnapshot.activeSessionId;

    if (!replay) {
      const startDecision = resolveAgentTurnStart(sourceActiveSession.id, [
        ...inFlightSessionIdsRef.current,
        ...observedRunningSessionIdsRef.current,
      ]);
      if (startDecision === "queue-capacity") {
        // 先把用户消息落到目标会话，再排队等空位。
      } else if (startDecision === "start") {
        inFlightSessionIdsRef.current.add(sourceActiveSession.id);
        reservedSessionId = sourceActiveSession.id;
        startedTurn = true;
        syncInFlightSessionIds();
      }
    } else if (
      !inFlightSessionIdsRef.current.has(sourceActiveSession.id) &&
      !observedRunningSessionIdsRef.current.has(sourceActiveSession.id) &&
      inFlightSessionIdsRef.current.size + observedRunningSessionIdsRef.current.size < MAX_PARALLEL_AGENT_TURNS
    ) {
      inFlightSessionIdsRef.current.add(sourceActiveSession.id);
      reservedSessionId = sourceActiveSession.id;
      startedTurn = true;
      syncInFlightSessionIds();
    }

    try {
      if (!replay?.clientMessageId) {
        if (!isPersistedSession(sourceSnapshot, sourceActiveSession)) {
          sessionForTurn = buildAgentSession({
            knowledgeBase: sourceActiveKnowledgeBase,
            title: buildTitleFromFirstPrompt(prompt),
          });
          snapshotForTurn = {
            ...sourceSnapshot,
            sessions: [sessionForTurn, ...sourceSnapshot.sessions],
            activeSessionId: viewerSessionId === sourceActiveSession.id || !viewerSessionId ? sessionForTurn.id : viewerSessionId,
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

        const optimisticTurn = appendUserMessageToSession(snapshotForTurn, sessionForTurn, optimisticMessage, {
          activate: viewerSessionId === sessionForTurn.id || viewerSessionId === sourceActiveSession.id,
        });
        sessionForTurn = optimisticTurn.session;
        snapshotForTurn = optimisticTurn.snapshot;
        if (reservedSessionId && reservedSessionId !== sessionForTurn.id) {
          inFlightSessionIdsRef.current.delete(reservedSessionId);
          inFlightSessionIdsRef.current.add(sessionForTurn.id);
          reservedSessionId = sessionForTurn.id;
          syncInFlightSessionIds();
        }
        commitTurnSnapshot(snapshotForTurn);
        latestSnapshot = snapshotForTurn;
        setAgentPrompt("");
        setMentionedFileIds([]);
        snapshotForTurn = await saveSession(snapshotForTurn, sessionForTurn);
        latestSnapshot = mergeSessionTurn(snapshotRef.current ?? snapshotForTurn, snapshotForTurn, sessionForTurn.id, {
          dirtyNoteIds: dirtyNoteIdsRef.current,
          dirtyDocumentIds: dirtyDocumentIdsRef.current,
        });
        commitTurnSnapshot(latestSnapshot);
        snapshotForTurn = latestSnapshot;
        didPersistOptimisticMessage = true;
        logInfo("用户消息已乐观落库。", {
          category: "frontend",
          event: "persist_user_message",
          status: "completed",
          metadata: {
            knowledgeBaseId: sourceActiveKnowledgeBase.id,
            sessionId: sessionForTurn.id,
            promptLength: prompt.length,
            explicitSkillCount: turnExplicitSkillIds.length,
          },
        });
      }

      if (reservedSessionId && reservedSessionId !== sessionForTurn.id) {
        inFlightSessionIdsRef.current.delete(reservedSessionId);
        inFlightSessionIdsRef.current.add(sessionForTurn.id);
        reservedSessionId = sessionForTurn.id;
        syncInFlightSessionIds();
      }

      if (!startedTurn) {
        enqueueFollowUp(
          {
            sessionId: sessionForTurn.id,
            prompt,
            action: turnAction,
            modelSelection: turnModelSelectionForRun,
            explicitSkillIds: turnExplicitSkillIds,
            mentionedFileIds: turnMentionedFileIds,
            clientMessageId: optimisticMessage.id,
          },
          "capacity",
        );
        return;
      }

      await clearAgentTurnPlaceholder(sessionForTurn.id);
      const backendActiveIds = await listActiveAgentSessionIds();
      if (backendActiveIds.includes(sessionForTurn.id)) {
        setNotice("该会话已有进行中的回合。");
        return;
      }

      setLiveTurns((current) => {
        const next = { ...current };
        delete next[sessionForTurn.id];
        return next;
      });

      const turnSnapshot = {
        ...snapshotForTurn,
        activeSessionId: sessionForTurn.id,
        activeKnowledgeBaseId: sourceActiveKnowledgeBase.id,
        activeNoteId: sourceActiveNote?.id ?? "",
        activeDocumentId: sourceActiveDocument?.id ?? "",
      };
      const decodedTurnModelSelection = decodeModelSelection(turnModelSelectionForRun);
      const result = await runAgentTurn(
        turnSnapshot,
        prompt,
        turnAction,
        optimisticMessage.id,
        decodedTurnModelSelection.providerId || undefined,
        decodedTurnModelSelection.modelId || undefined,
        turnExplicitSkillIds,
        turnMentionedFileIds,
      );
      const viewerSnapshot = snapshotRef.current ?? result.snapshot;
      const touched = collectTouchedFileIds(snapshotForTurn, result.snapshot);
      const mergedSnapshot = mergeSessionTurn(viewerSnapshot, result.snapshot, sessionForTurn.id, {
        dirtyNoteIds: dirtyNoteIdsRef.current,
        dirtyDocumentIds: dirtyDocumentIdsRef.current,
        touchedNoteIds: touched.touchedNoteIds,
        touchedDocumentIds: touched.touchedDocumentIds,
      });

      commitTurnSnapshot(mergedSnapshot);
      latestSnapshot = mergedSnapshot;
      setLiveTurns((current) => {
        const next = { ...current };
        delete next[sessionForTurn.id];
        return next;
      });
      if (!presetPrompt && !replay) {
        setExplicitSkillIds([]);
      }
      const [nextAuditLogs, nextAppEventLogs] = await Promise.all([loadRequestAuditLogs(), loadAppEventLogs()]);

      setAuditLogs(nextAuditLogs);
      setAppEventLogs(nextAppEventLogs);
    } catch (error) {
      if (!presetPrompt && !replay) {
        setMentionedFileIds(turnMentionedFileIds);
      }
      if (!didPersistOptimisticMessage) {
        commitTurnSnapshot(sourceSnapshot);
        latestSnapshot = sourceSnapshot;
        setAgentPrompt(promptBeforeSubmit);
      }
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      if (startedTurn) {
        inFlightSessionIdsRef.current.delete(reservedSessionId ?? sessionForTurn.id);
        inFlightSessionIdsRef.current.delete(sessionForTurn.id);
        syncInFlightSessionIds();
        setLiveTurns((current) => {
          const next = { ...current };
          delete next[sessionForTurn.id];
          return next;
        });
        const queued = takeNextQueuedFollowUp(sessionForTurn.id);
        if (queued && resolveAgentTurnStart(queued.sessionId, inFlightSessionIdsRef.current) === "start") {
          const replaySnapshot = snapshotRef.current ?? latestSnapshot;
          void submitPromptRef.current(queued.action, queued.prompt, replaySnapshot, queued);
        } else if (queued) {
          queuedBySessionRef.current.set(queued.sessionId, queued);
          syncQueuedFollowUps();
        }
      }
    }
  }

  submitPromptRef.current = handleSubmitPrompt;

  /** 等到该会话的 runAgentTurn 走完 finally，避免 rewind 后被 mergeSessionTurn 写回旧消息。 */
  function waitForSessionIdle(sessionId: string, timeoutMs = 30_000) {
    return new Promise<void>((resolve, reject) => {
      const startedAt = Date.now();
      const tick = async () => {
        const frontendIdle = !inFlightSessionIdsRef.current.has(sessionId);
        let backendIdle = true;
        try {
          const activeIds = await listActiveAgentSessionIds();
          backendIdle = !activeIds.includes(sessionId);
        } catch {
          backendIdle = frontendIdle;
        }
        if (frontendIdle && backendIdle) {
          resolve();
          return;
        }
        if (Date.now() - startedAt > timeoutMs) {
          reject(new Error("等待当前回合结束超时。"));
          return;
        }
        window.setTimeout(() => {
          void tick();
        }, 50);
      };
      void tick();
    });
  }

  /** 丢掉该会话尚未进入模型的排队，避免 abort 收尾后又把旧指令跑起来。 */
  function dropQueuedFollowUp(sessionId: string) {
    if (!queuedBySessionRef.current.has(sessionId)) {
      return;
    }

    queuedBySessionRef.current.delete(sessionId);
    syncQueuedFollowUps();
  }

  /** 编辑一条已发送的用户消息，截断其后历史并立刻重跑。 */
  async function handleEditUserMessageAndRerun(messageId: string, prompt: string) {
    const nextPrompt = prompt.trim();
    if (!nextPrompt) {
      return;
    }

    const session = snapshotRef.current?.sessions.find((item) => item.id === activeSession.id) ?? activeSession;
    const messageIndex = session.messages.findIndex((message) => message.id === messageId);
    const target = messageIndex >= 0 ? session.messages[messageIndex] : undefined;
    if (!target || target.role !== "user") {
      setNotice("找不到要编辑的用户消息。");
      return;
    }
    if (session.imIdentity) {
      setNotice("即时通讯会话不支持编辑历史消息。");
      return;
    }

    const removedCount = session.messages.length - messageIndex - 1;
    const hasPending = Boolean(session.pendingChange || session.pendingChangeSet || session.pendingExecution);
    const isTurnRunning = inFlightSessionIdsRef.current.has(session.id);
    const runRewind = () => void executeEditUserMessageAndRerun(session.id, target, nextPrompt);

    if (removedCount > 0 || hasPending || isTurnRunning) {
      const details = [
        removedCount > 0 ? `将删除这条之后的 ${removedCount} 条对话。` : "",
        hasPending ? "未确认的写入和 Skill 会被放弃。" : "",
        isTurnRunning ? "当前正在生成的回复会先停止。" : "",
        "已经应用到知识库的文件不会自动还原。",
      ]
        .filter(Boolean)
        .join("");

      requestConfirmation(
        {
          title: "重新执行这条消息？",
          message: details,
          confirmLabel: "删除并重新执行",
        },
        runRewind,
      );
      return;
    }

    await executeEditUserMessageAndRerun(session.id, target, nextPrompt);
  }

  /** abort → 等空闲 → rewind → 用同一 clientMessageId 重跑，不再追加新的用户消息。 */
  async function executeEditUserMessageAndRerun(sessionId: string, target: AgentMessage, prompt: string) {
    beginBusy("正在重新执行…");
    try {
      dropQueuedFollowUp(sessionId);
      if (inFlightSessionIdsRef.current.has(sessionId)) {
        await abortAgentTurn(sessionId);
        await waitForSessionIdle(sessionId);
      }

      const sourceSnapshot = snapshotRef.current ?? currentSnapshot;
      const rewoundSnapshot = await rewindAgentSession(sourceSnapshot, sessionId, target.id, prompt);
      commitTurnSnapshot(rewoundSnapshot);
      setLiveTurns((current) => {
        const next = { ...current };
        delete next[sessionId];
        return next;
      });

      await handleSubmitPrompt(target.action ?? "ask", prompt, rewoundSnapshot, {
        sessionId,
        prompt,
        action: target.action ?? "ask",
        modelSelection: turnModelSelection,
        explicitSkillIds: [],
        mentionedFileIds: target.mentionedFileIds ?? [],
        clientMessageId: target.id,
      });
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 打开设置抽屉时刷新非阻塞诊断信息，避免展示过旧的日志列表。 */
  async function handleApproveSkillExecution() {
    beginBusy("正在隔离区运行 Skill...");
    try {
      const nextSnapshot = await approveSkillExecution(currentSnapshot);
      commitTurnSnapshot(nextSnapshot);
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
      commitTurnSnapshot(await rejectSkillExecution(currentSnapshot));
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
      commitTurnSnapshot(nextSnapshot, new Set(), new Set());
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
      commitTurnSnapshot(nextSnapshot);
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
      commitTurnSnapshot(await saveSession(nextSnapshot, nextSession));
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    }
  }

  return {
    liveTurn,
    queuedFollowUp,
    queuedFollowUpInList,
    isCurrentSessionBusy,
    inFlightSessionIds: runningSessionIds,
    queuedSessionIds: Object.keys(queuedBySession),
    enqueueFollowUp: (prompt: string) => {
      enqueueFollowUp({
        sessionId: activeSession.id,
        prompt,
        action: "ask",
        modelSelection: turnModelSelection,
        explicitSkillIds,
        mentionedFileIds,
      });
    },
    takeQueuedFollowUp: () => takeNextQueuedFollowUp(activeSession.id)?.prompt ?? null,
    handleClearQueuedFollowUp,
    handleAbortTurn,
    handlePrepareSessionForDelete,
    handleSubmitPrompt,
    handleEditUserMessageAndRerun,
    handleApproveSkillExecution,
    handleRejectSkillExecution,
    handleApplySkillChangeSet,
    handleRejectSkillChangeSet,
    handleToggleSkillChangeOperation,
  };
}
