import { invokeLogged, isTauriRuntime } from "./runtime";
import { acceptMockProposedChange, rejectMockProposedChange, runMockAgentTurn } from "../mock/workspace";
import {
  browserMock,
  buildMockContextSummary,
  captureBrowserDocumentHistory,
  createBrowserAuditLog,
  logBrowserSkillContext,
  normalizeMockSnapshotSessions,
  rememberBrowserPromptDump,
} from "../mock/browser";
import {
  AgentActionType,
  AgentPromptDump,
  AgentTurnProgressEvent,
  AgentTurnRequest,
  AgentTurnResult,
  KnowledgeBase,
  Note,
  ProposedChange,
  WorkspaceSnapshot,
} from "../types";

/** 运行 Agent 单轮 loop，模型可在内部自行选择是否调用检索工具。 */
export async function runAgentTurn(
  snapshot: WorkspaceSnapshot,
  prompt: string,
  action: AgentActionType,
  clientMessageId?: string,
  modelProviderId?: string,
  modelId?: string,
  explicitSkillIds: string[] = [],
  mentionedFileIds: string[] = [],
): Promise<AgentTurnResult> {
  const request: AgentTurnRequest = {
    prompt,
    action,
    sessionId: snapshot.activeSessionId,
    activeKnowledgeBaseId: snapshot.activeKnowledgeBaseId,
    activeNoteId: snapshot.activeNoteId,
    clientMessageId,
    modelProviderId,
    modelId,
    explicitSkillIds,
    mentionedFileIds,
  };

  if (!isTauriRuntime()) {
    logBrowserSkillContext(browserMock.agentSkills, request);
    const nextSnapshot = runMockAgentTurn(snapshot, prompt, action, clientMessageId, explicitSkillIds, mentionedFileIds);
    const session = nextSnapshot.sessions.find((item) => item.id === nextSnapshot.activeSessionId);

    if (session) {
      rememberBrowserPromptDump(session, prompt, action);
    }

    browserMock.auditLogs = [createBrowserAuditLog(nextSnapshot, prompt), ...browserMock.auditLogs].slice(0, 20);

    return { snapshot: nextSnapshot };
  }

  return invokeLogged<AgentTurnResult>("run_agent_turn", { payload: { snapshot, request } });
}

/** 订阅 Agent 过程事件；浏览器开发态没有 Tauri 窗口时返回空卸载函数。 */
export async function listenAgentTurnProgress(
  onProgress: (payload: AgentTurnProgressEvent) => void,
): Promise<() => void> {
  if (!isTauriRuntime()) {
    return () => undefined;
  }

  const { listen } = await import("@tauri-apps/api/event");
  return listen<AgentTurnProgressEvent>("agent-turn-progress", (event) => {
    onProgress(event.payload);
  });
}

/** 手动整理当前 Agent 会话工作记忆，桌面端由后端决定使用模型或本地降级。 */
export async function compactAgentContext(snapshot: WorkspaceSnapshot, sessionId: string): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    return normalizeMockSnapshotSessions({
      ...snapshot,
      sessions: snapshot.sessions.map((session) =>
        session.id === sessionId ? { ...session, contextSummary: buildMockContextSummary(session) } : session,
      ),
    });
  }

  return invokeLogged<WorkspaceSnapshot>("compact_agent_context", { payload: { snapshot, sessionId } });
}

/** 读取某会话最近一次发给模型的上下文预览；没有转储时返回 null。 */
export async function loadAgentPromptDump(sessionId: string): Promise<AgentPromptDump | null> {
  if (!isTauriRuntime()) {
    return browserMock.promptDumps.get(sessionId) ?? null;
  }

  return invokeLogged<AgentPromptDump | null>("load_agent_prompt_dump", { payload: { sessionId } });
}

/** 接受当前会话的待确认变更，Tauri 环境中由本地层执行安全写入。 */
export async function acceptProposedChange(snapshot: WorkspaceSnapshot): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const pendingChange = snapshot.sessions.find((session) => session.id === snapshot.activeSessionId)?.pendingChange;
    const targetNote = pendingChange?.noteId
      ? snapshot.notes.find((note) => note.id === pendingChange.noteId)
      : undefined;

    if (pendingChange && pendingChange.type !== "create" && targetNote) {
      const diskContent = browserMock.noteDiskContents.get(targetNote.id) ?? targetNote.content;

      captureBrowserDocumentHistory({
        targetKind: "note",
        knowledgeBaseId: targetNote.knowledgeBaseId,
        targetId: targetNote.id,
        relativePath: targetNote.path,
        title: targetNote.title,
        fileType: "markdown",
        content: diskContent,
        source: "agent-change",
        sessionId: snapshot.activeSessionId,
        changeId: pendingChange.id,
      });
    }

    const nextSnapshot = normalizeMockSnapshotSessions(acceptMockProposedChange(snapshot));

    if (pendingChange?.type === "create") {
      const createdNote = nextSnapshot.notes.find((note) => note.path === pendingChange.targetPath);

      if (createdNote) {
        browserMock.noteDiskContents.set(createdNote.id, createdNote.content);
      }
    } else if (targetNote) {
      const nextNote = nextSnapshot.notes.find((note) => note.id === targetNote.id);

      if (nextNote) {
        browserMock.noteDiskContents.set(nextNote.id, nextNote.content);
      }
    }

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("apply_proposed_change", { payload: { snapshot } });
}

/** 拒绝当前会话的待确认变更，Tauri 环境中只更新会话状态。 */
export async function rejectProposedChange(snapshot: WorkspaceSnapshot): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    return normalizeMockSnapshotSessions(rejectMockProposedChange(snapshot));
  }

  return invokeLogged<WorkspaceSnapshot>("reject_proposed_change", { payload: { snapshot } });
}

/** 批准当前会话的 Skill 执行请求；浏览器开发态不模拟系统进程执行。 */
export async function approveSkillExecution(snapshot: WorkspaceSnapshot): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    throw new Error("Skill 隔离执行仅在橘记桌面端可用。");
  }

  return invokeLogged<WorkspaceSnapshot>("approve_skill_execution", { payload: { snapshot } });
}

/** 拒绝当前会话的 Skill 执行请求，不启动任何进程。 */
export async function rejectSkillExecution(snapshot: WorkspaceSnapshot): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    return {
      ...snapshot,
      sessions: snapshot.sessions.map((session) =>
        session.id === snapshot.activeSessionId && session.pendingExecution
          ? { ...session, pendingExecution: { ...session.pendingExecution, status: "rejected" } }
          : session,
      ),
    };
  }

  return invokeLogged<WorkspaceSnapshot>("reject_skill_execution", { payload: { snapshot } });
}

/** 应用 Skill 生成的多文件变更集。 */
export async function applySkillChangeSet(snapshot: WorkspaceSnapshot): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    throw new Error("Skill 文件变更集仅在橘记桌面端可用。");
  }
  return invokeLogged<WorkspaceSnapshot>("apply_skill_change_set", { payload: { snapshot } });
}

/** 拒绝 Skill 生成的多文件变更集并清理隔离副本。 */
export async function rejectSkillChangeSet(snapshot: WorkspaceSnapshot): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    return {
      ...snapshot,
      sessions: snapshot.sessions.map((session) =>
        session.id === snapshot.activeSessionId && session.pendingChangeSet
          ? { ...session, pendingChangeSet: { ...session.pendingChangeSet, status: "rejected" } }
          : session,
      ),
    };
  }
  return invokeLogged<WorkspaceSnapshot>("reject_skill_change_set", { payload: { snapshot } });
}

/** 应用 Agent 直接产出的多文件变更集（如 create_folder），无 Skill 隔离区。 */
export async function applyAgentChangeSet(snapshot: WorkspaceSnapshot): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    throw new Error("Agent 文件变更集仅在橘记桌面端可用。");
  }
  return invokeLogged<WorkspaceSnapshot>("apply_agent_change_set", { payload: { snapshot } });
}

/** 拒绝 Agent 直接产出的多文件变更集。 */
export async function rejectAgentChangeSet(snapshot: WorkspaceSnapshot): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    return {
      ...snapshot,
      sessions: snapshot.sessions.map((session) =>
        session.id === snapshot.activeSessionId && session.pendingChangeSet
          ? { ...session, pendingChangeSet: { ...session.pendingChangeSet, status: "rejected" } }
          : session,
      ),
    };
  }
  return invokeLogged<WorkspaceSnapshot>("reject_agent_change_set", { payload: { snapshot } });
}
