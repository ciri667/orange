import { decodeModelSelection } from "../shared/modelSelection";
import { logInfo, logWarn } from "../shared/logger";
import { getActiveDocument, getActiveKnowledgeBase, getActiveNote } from "../shared/selectors";
import { compactAgentContext, deleteSession, restoreSessionContext, saveSession, saveUserSettings, updateSessionScope } from "../shared/tauriApi";
import type { AgentSession } from "../shared/types";
import { formatLocalDateTime } from "../shared/id";
import {
  buildAgentSession,
  buildDraftAgentSession,
  isPersistedSession,
  resolveActiveSessionForKnowledgeBase,
} from "./sessionUtils";
import type { WorkspaceChrome } from "./workspaceChrome";

interface SessionActionsOptions extends WorkspaceChrome {
  setSearchTerm: (value: string) => void;
  setCollapsedFolderPaths: (value: Set<string>) => void;
  setIsSessionListOpen: (value: boolean | ((current: boolean) => boolean)) => void;
  setIsSessionContextOpen: (value: boolean | ((current: boolean) => boolean)) => void;
  setIsScopeSelectorOpen: (value: boolean | ((current: boolean) => boolean)) => void;
  isSessionListOpen: boolean;
  isSessionContextOpen: boolean;
  isScopeSelectorOpen: boolean;
  agentOpen: boolean;
  setAgentOpen: (value: boolean) => void;
  setUserSettings: (settings: import("../shared/types").UserSettings) => void;
}

const noopAsync = async (..._args: unknown[]) => {};
const noop = (..._args: unknown[]) => {};

/** 会话创建、切换、范围、模型和权限动作。 */
export function useSessionActions(options: SessionActionsOptions) {
  const {
    snapshot,
    userSettings,
    beginBusy,
    endBusy,
    setNotice,
    commitSnapshot,
    requestConfirmation,
    setSearchTerm,
    setCollapsedFolderPaths,
    setIsSessionListOpen,
    setIsSessionContextOpen,
    setIsScopeSelectorOpen,
    isSessionListOpen,
    isSessionContextOpen,
    isScopeSelectorOpen,
    agentOpen,
    setAgentOpen,
    setUserSettings,
  } = options;

  if (!snapshot || !userSettings) {
    return {
      handleCreateSession: noopAsync,
      handleSelectSession: noopAsync,
      handleDeleteSession: noopAsync,
      handleToggleScopeKnowledgeBase: noopAsync,
      handleSetSessionModelSelection: noopAsync,
      handleCompactAgentContext: noopAsync,
      handleToggleSessionList: noop,
      handleToggleSessionContext: noop,
      handleToggleScopeSelector: noop,
      handleToggleAgentPanel: noop,
      handleSessionSecurityLevelChange: noopAsync,
    };
  }

  const currentSnapshot = snapshot;
  const currentUserSettings = userSettings;
  const activeKnowledgeBase = getActiveKnowledgeBase(currentSnapshot);
  const activeNote = getActiveNote(currentSnapshot);
  const activeDocument = getActiveDocument(currentSnapshot);
  const persistedActiveSession = resolveActiveSessionForKnowledgeBase(currentSnapshot, activeKnowledgeBase);
  const activeSession = persistedActiveSession ?? buildDraftAgentSession(activeKnowledgeBase);


  /** 新建一个空白知识库会话；标题等到首条用户输入后再确定。 */
  async function handleCreateSession() {
    logInfo("创建空白会话。", {
      category: "frontend",
      event: "create_session",
      status: "started",
      metadata: {
        knowledgeBaseId: activeKnowledgeBase.id,
      },
    });

    const nextSession = buildAgentSession({
      knowledgeBase: activeKnowledgeBase,
    });
    const nextSnapshot = {
      ...currentSnapshot,
      sessions: [nextSession, ...currentSnapshot.sessions],
      activeSessionId: nextSession.id,
    };

    beginBusy("正在创建 Agent 会话...");

    try {
      commitSnapshot(await saveSession(nextSnapshot, nextSession));
      logInfo("空白会话已创建。", {
        category: "frontend",
        event: "create_session",
        status: "completed",
        metadata: {
          knowledgeBaseId: activeKnowledgeBase.id,
        },
      });
      setIsSessionListOpen(false);
      setIsSessionContextOpen(false);
      setIsScopeSelectorOpen(false);
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 切换会话时恢复它绑定的知识库和工具范围；文件焦点不再被默认会话推着走。 */
  async function handleSelectSession(sessionId: string) {
    const nextSession = currentSnapshot.sessions.find((session) => session.id === sessionId);

    if (!nextSession) {
      return;
    }

    beginBusy("正在恢复 Agent 会话...");

    try {
      const nextSnapshot = await restoreSessionContext(currentSnapshot, sessionId);

      commitSnapshot(nextSnapshot);
      setSearchTerm("");
      setCollapsedFolderPaths(new Set());
      setIsSessionListOpen(false);
      setIsSessionContextOpen(false);
      setIsScopeSelectorOpen(false);
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 逻辑删除 Agent 会话；删除当前会话时由后端选择下一条可用会话。 */
  async function handleDeleteSession(sessionId: string) {
    const session = currentSnapshot.sessions.find((item) => item.id === sessionId);

    if (!session) {
      return;
    }

    requestConfirmation(
      {
        title: "删除 Agent 会话",
        message: `删除会话「${session.title}」？本地文档和请求审计记录不会被删除。`,
        confirmLabel: "删除会话",
      },
      async () => {
        beginBusy("正在删除 Agent 会话...");

        try {
          const nextSnapshot = await deleteSession(currentSnapshot, sessionId);

          commitSnapshot(nextSnapshot);
          setIsSessionListOpen(true);
          setIsSessionContextOpen(false);
          setIsScopeSelectorOpen(false);
          setNotice("已删除会话。");
        } catch (error) {
          setNotice(error instanceof Error ? error.message : String(error));
        } finally {
          endBusy();
        }
      },
    );
  }

  /** 为当前会话勾选或取消额外知识库，当前激活知识库始终保留。 */
  async function handleToggleScopeKnowledgeBase(knowledgeBaseId: string) {
    if (!isPersistedSession(currentSnapshot, activeSession)) {
      setNotice("请先新建或发送一条消息创建会话，再调整工具范围。");
      return;
    }

    const selectedIds = new Set(activeSession.knowledgeBaseIds.length ? activeSession.knowledgeBaseIds : [activeKnowledgeBase.id]);

    selectedIds.add(activeKnowledgeBase.id);

    // 当前激活知识库是默认工具范围边界，不能在本会话中取消。
    if (knowledgeBaseId !== activeKnowledgeBase.id) {
      if (selectedIds.has(knowledgeBaseId)) {
        selectedIds.delete(knowledgeBaseId);
      } else {
        selectedIds.add(knowledgeBaseId);
      }
    }

    beginBusy("正在更新工具范围...");

    try {
      commitSnapshot(
        await updateSessionScope(currentSnapshot, activeSession.id, Array.from(selectedIds), activeKnowledgeBase.id),
      );
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 设置当前会话的默认 provider/model；传入空字符串表示跟随全局默认模型。 */
  async function handleSetSessionModelSelection(selection: string) {
    if (!isPersistedSession(currentSnapshot, activeSession)) {
      setNotice("请先新建或发送一条消息创建会话，再设置会话默认模型。");
      return;
    }

    const decodedSelection = decodeModelSelection(selection);
    const nextSession: AgentSession = {
      ...activeSession,
      modelProviderId: decodedSelection.providerId || undefined,
      modelId: decodedSelection.modelId || undefined,
    };

    beginBusy("正在更新会话默认模型...");

    try {
      commitSnapshot(await saveSession(currentSnapshot, nextSession));
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      endBusy();
    }
  }

  /** 手动整理当前会话工作记忆；日志只记录消息数量和摘要状态，不写入正文。 */
  async function handleCompactAgentContext() {
    if (!isPersistedSession(currentSnapshot, activeSession)) {
      setNotice("请先新建或发送一条消息创建会话，再整理上下文。");
      return;
    }

    beginBusy("正在整理上下文...");

    try {
      logInfo("开始手动整理会话上下文。", {
        category: "frontend",
        event: "compact_agent_context",
        status: "started",
        metadata: {
          sessionId: activeSession.id,
          messageCount: activeSession.messages.length,
          hasContextSummary: Boolean(activeSession.contextSummary),
          hasActivePendingChange: activeSession.pendingChange?.status === "pending",
        },
      });
      commitSnapshot(await compactAgentContext(currentSnapshot, activeSession.id));
      setNotice("已整理上下文。");
      logInfo("手动整理会话上下文完成。", {
        category: "frontend",
        event: "compact_agent_context",
        status: "completed",
        metadata: {
          sessionId: activeSession.id,
          messageCount: activeSession.messages.length,
          hasActivePendingChange: activeSession.pendingChange?.status === "pending",
        },
      });
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
      logWarn("手动整理会话上下文失败。", {
        category: "frontend",
        event: "compact_agent_context",
        status: "failed",
        metadata: {
          sessionId: activeSession.id,
          messageCount: activeSession.messages.length,
        },
      });
    } finally {
      endBusy();
    }
  }

  /** 切换会话历史浮层；日志只记录状态和数量，不写入会话标题、消息正文或路径。 */
  function handleToggleSessionList() {
    const nextOpen = !isSessionListOpen;

    logInfo("切换会话历史浮层。", {
      category: "frontend",
      event: "toggle_session_list",
      status: nextOpen ? "opened" : "closed",
      metadata: {
        sessionCount: currentSnapshot.sessions.length,
        hasActivePendingChange: activeSession.pendingChange?.status === "pending",
      },
    });
    setIsSessionListOpen(nextOpen);
    setIsSessionContextOpen(false);
    setIsScopeSelectorOpen(false);
  }

  /** 切换上下文浮层；日志只记录数量和状态，不写入标题、正文、知识库名称或路径。 */
  function handleToggleSessionContext() {
    const nextOpen = !isSessionContextOpen;

    logInfo("切换上下文浮层。", {
      category: "frontend",
      event: "toggle_session_context",
      status: nextOpen ? "opened" : "closed",
      metadata: {
        messageCount: activeSession.messages.length,
        selectedScopeCount: activeSession.knowledgeBaseIds.length || 1,
        hasActivePendingChange: activeSession.pendingChange?.status === "pending",
      },
    });
    setIsSessionContextOpen(nextOpen);
    setIsSessionListOpen(false);
    setIsScopeSelectorOpen(false);
  }

  /** 切换工具范围浮层；日志只记录数量和状态，不写入知识库名称或本地路径。 */
  function handleToggleScopeSelector() {
    const nextOpen = !isScopeSelectorOpen;

    logInfo("切换工具范围浮层。", {
      category: "frontend",
      event: "toggle_scope_selector",
      status: nextOpen ? "opened" : "closed",
      metadata: {
        knowledgeBaseCount: currentSnapshot.knowledgeBases.length,
        selectedScopeCount: activeSession.knowledgeBaseIds.length || 1,
      },
    });
    setIsScopeSelectorOpen(nextOpen);
    setIsSessionListOpen(false);
    setIsSessionContextOpen(false);
  }

  /** 切换右侧 Agent 停靠栏显隐，编辑区始终留在中间。 */
  function handleToggleAgentPanel() {
    const nextOpen = !agentOpen;

    logInfo("切换 Agent 协作区显隐。", {
      category: "frontend",
      event: "agent_panel_visibility_toggle",
      status: nextOpen ? "expanded" : "collapsed",
      metadata: {
        messageCount: activeSession.messages.length,
        hasActivePendingChange: activeSession.pendingChange?.status === "pending",
      },
    });
    setAgentOpen(nextOpen);
  }

  /** 更新当前本地会话的安全级别，并将用户的显式选择同步为对应能力授权。 */
  async function handleSessionSecurityLevelChange(securityLevel: AgentSession["securityLevel"]) {
    if (!userSettings || activeSession.imIdentity || securityLevel === activeSession.securityLevel) {
      return;
    }
    const currentUserSettings = userSettings;

    /** 权限选择是用户的显式授权动作；先保存能力开关，再保存依赖它的会话级别。 */
    async function persistSecurityLevel() {
      beginBusy("正在更新 Agent 权限...");

      try {
        const requiresAdvanced = securityLevel !== "basic";
        const requiresAutonomous = securityLevel === "autonomous";
        const nextAgentSecurity = {
          ...currentUserSettings.agentSecurity,
          advancedExecutionEnabled: requiresAdvanced ? true : currentUserSettings.agentSecurity.advancedExecutionEnabled,
          autonomousModeEnabled: requiresAutonomous ? true : currentUserSettings.agentSecurity.autonomousModeEnabled,
        };

        if (
          nextAgentSecurity.advancedExecutionEnabled !== currentUserSettings.agentSecurity.advancedExecutionEnabled ||
          nextAgentSecurity.autonomousModeEnabled !== currentUserSettings.agentSecurity.autonomousModeEnabled
        ) {
          setUserSettings(await saveUserSettings({ ...currentUserSettings, agentSecurity: nextAgentSecurity }));
        }

        const nextSession = { ...activeSession, securityLevel, updatedAt: formatLocalDateTime() };
        const nextSnapshot = {
          ...currentSnapshot,
          sessions: currentSnapshot.sessions.map((session) => session.id === activeSession.id ? nextSession : session),
        };
        commitSnapshot(await saveSession(nextSnapshot, nextSession));
        setNotice(
          securityLevel === "basic"
            ? "已切回基础权限：只在选中的知识库里工作，写入需要你确认。"
            : securityLevel === "advanced"
              ? "已切换到进阶权限：可整理目录并运行 Skill，落盘前仍需确认。"
              : "已切换到完全权限：可在合规路径上读列，校验通过后可自动落盘；不是整台电脑。",
        );
      } catch (error) {
        setNotice(error instanceof Error ? error.message : String(error));
      } finally {
        endBusy();
      }
    }

    if (securityLevel === "autonomous") {
      requestConfirmation(
        {
          title: "切换到完全权限",
          message: "完全权限意味着你把连续执行权交给 Agent，好让模型一次把任务做完。校验通过的知识库文本写入会自动落盘；已授权的 Skill 可连续运行；也可以在知识库之外的合规路径上用同一套 list / read / write。这不是整台电脑：系统保护目录和隔离边界仍然生效。",
          confirmLabel: "切换到完全权限",
          tone: "default",
        },
        persistSecurityLevel,
      );
      return;
    }

    await persistSecurityLevel();
  }

  return {
    handleCreateSession,
    handleSelectSession,
    handleDeleteSession,
    handleToggleScopeKnowledgeBase,
    handleSetSessionModelSelection,
    handleCompactAgentContext,
    handleToggleSessionList,
    handleToggleSessionContext,
    handleToggleScopeSelector,
    handleToggleAgentPanel,
    handleSessionSecurityLevelChange,
  };
}
