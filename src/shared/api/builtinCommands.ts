import { invokeLogged, isTauriRuntime } from "./runtime";
import { cloneWorkspaceSnapshot } from "../mock/workspace";
import { buildMockContextSummary, normalizeMockSnapshotSessions } from "../mock/browser";
import { buildAgentSession } from "../../workspace/sessionUtils";
import { AgentSession, BuiltinCommandDescriptor, BuiltinCommandRunResult, WorkspaceSnapshot } from "../types";

/**
 * 浏览器开发态的内置指令表，必须与 `src-tauri/src/builtin_commands.rs` 的注册表保持一致。
 * 桌面端以 Rust 注册表为准；这里只是让 `npm run dev` 也能渲染 `/` 弹窗。
 */
const MOCK_BUILTIN_COMMANDS: BuiltinCommandDescriptor[] = [
  {
    id: "help",
    name: "help",
    aliases: [],
    description: "查看全部内置指令说明",
    argumentHint: "",
    scopes: ["desktop", "im"],
    requiresActiveSession: false,
  },
  {
    id: "status",
    name: "status",
    aliases: [],
    description: "查看当前会话、模型与上下文状态",
    argumentHint: "",
    scopes: ["desktop", "im"],
    requiresActiveSession: false,
  },
  {
    id: "new",
    name: "new",
    aliases: ["reset", "clear"],
    description: "开启新的 Agent 会话",
    argumentHint: "",
    scopes: ["desktop", "im"],
    requiresActiveSession: false,
  },
  {
    id: "compact",
    name: "compact",
    aliases: [],
    description: "整理当前会话上下文，压缩模型上下文并更新工作记忆",
    argumentHint: "",
    scopes: ["desktop", "im"],
    requiresActiveSession: true,
  },
  {
    id: "rename",
    name: "rename",
    aliases: ["title"],
    description: "重命名当前会话",
    argumentHint: "<标题>",
    scopes: ["desktop"],
    requiresActiveSession: true,
  },
];

/** 读取桌面端可用的内置指令，供 `/` 弹窗与 `/help` 共用。 */
export async function listBuiltinCommands(): Promise<BuiltinCommandDescriptor[]> {
  if (!isTauriRuntime()) {
    return MOCK_BUILTIN_COMMANDS.filter((command) => command.scopes.includes("desktop"));
  }

  return invokeLogged<BuiltinCommandDescriptor[]>("list_builtin_commands");
}

/**
 * 执行桌面端内置指令。
 *
 * 指令是本地动作：这里不会调用 `run_agent_turn`，命中指令时也不会产生对话消息。
 * 未知或未实现的指令由后端返回错误，调用方据此决定是否降级为普通消息。
 */
export async function runBuiltinCommand(
  snapshot: WorkspaceSnapshot,
  commandId: string,
  args: string,
): Promise<BuiltinCommandRunResult> {
  if (!isTauriRuntime()) {
    return runMockBuiltinCommand(snapshot, commandId, args);
  }

  return invokeLogged<BuiltinCommandRunResult>("run_builtin_command", {
    payload: { snapshot, commandId, args },
  });
}

/** 浏览器开发态的指令执行分支，保持与 Rust 侧相同的提示文案与快照语义。 */
function runMockBuiltinCommand(
  snapshot: WorkspaceSnapshot,
  commandId: string,
  args: string,
): BuiltinCommandRunResult {
  const descriptor = MOCK_BUILTIN_COMMANDS.find((command) => command.id === commandId);
  if (!descriptor) {
    throw new Error(`未知的内置指令：${commandId}`);
  }

  const activeSession = snapshot.sessions.find((session) => session.id === snapshot.activeSessionId);
  if (descriptor.requiresActiveSession && !activeSession) {
    throw new Error("当前还没有已保存的会话，请先发送一条消息再使用该指令。");
  }

  switch (commandId) {
    case "help": {
      const lines = MOCK_BUILTIN_COMMANDS.filter((command) => command.scopes.includes("desktop")).map(
        (command) => `/${command.name}${command.argumentHint ? ` ${command.argumentHint}` : ""}：${command.description}`,
      );

      return { commandId, notice: ["内置指令：", ...lines].join("\n") };
    }
    case "status": {
      if (!activeSession) {
        return { commandId, notice: "当前没有活跃会话。" };
      }

      // 与 Rust 侧 build_session_status_notice 保持同样的字段和换行，避免开发态与桌面端观感不一致。
      const context = activeSession.contextUsage
        ? `${activeSession.contextUsage.totalTokens} tokens（模型窗口 ${activeSession.contextUsage.contextLength ?? "未知"}）`
        : "暂无记录";

      return {
        commandId,
        notice: [
          `会话：${activeSession.title || "未命名会话"}`,
          `消息：${activeSession.messages.length} 条`,
          "模型：跟随全局默认模型",
          `上下文：${context}`,
          `安全级别：${activeSession.securityLevel}`,
          `计划模式：${activeSession.planMode ? "开启" : "关闭"}`,
        ].join("\n"),
      };
    }
    case "new": {
      const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
      const nextSession = buildAgentSession({
        knowledgeBase: nextSnapshot.knowledgeBases.find((knowledgeBase) => knowledgeBase.id === nextSnapshot.activeKnowledgeBaseId)!,
        securityLevel: "workspace-readonly",
      });

      nextSnapshot.sessions = [nextSession, ...nextSnapshot.sessions];
      nextSnapshot.activeSessionId = nextSession.id;

      return {
        commandId,
        notice: "已开启新会话，下一条消息将从新的上下文开始。",
        snapshot: normalizeMockSnapshotSessions(nextSnapshot),
      };
    }
    case "rename": {
      const title = args.trim();
      if (!title) {
        throw new Error("用法：/rename <标题>");
      }

      const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
      nextSnapshot.sessions = nextSnapshot.sessions.map((session) =>
        session.id === nextSnapshot.activeSessionId
          ? ({ ...session, title, titleCustomized: true, updatedAt: "刚刚" } as AgentSession)
          : session,
      );

      return { commandId, notice: `已将会话重命名为「${title}」。`, snapshot: normalizeMockSnapshotSessions(nextSnapshot) };
    }
    case "compact": {
      const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
      nextSnapshot.sessions = nextSnapshot.sessions.map((session) =>
        session.id === nextSnapshot.activeSessionId
          ? { ...session, contextSummary: buildMockContextSummary(session) }
          : session,
      );

      return {
        commandId,
        // 开发态没有真实 runtime transcript，不能假装压缩了模型上下文。
        notice: "已整理上下文：开发态使用本地模拟摘要，不会真正压缩模型上下文。",
        snapshot: normalizeMockSnapshotSessions(nextSnapshot),
      };
    }
    default:
      throw new Error(`内置指令 ${commandId} 尚未实现。`);
  }
}
