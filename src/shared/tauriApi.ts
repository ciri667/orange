import { invoke, isTauri } from "@tauri-apps/api/core";
import { createContentHash, createLocalId, formatLocalDateTime } from "./id";
import { logDebug, logError, logInfo } from "./logger";
import {
  acceptMockProposedChange,
  cloneWorkspaceSnapshot,
  createMockKnowledgeBaseSelection,
  createMockWorkspaceSnapshot,
  rejectMockProposedChange,
  runMockAgentTurn,
} from "./mockWorkspace";
import type {
  AgentActionType,
  AgentSkill,
  AgentSession,
  AgentTurnRequest,
  AgentTurnResult,
  AppEventLog,
  AppEventLogCategory,
  AppEventLogLevel,
  DocumentPreview,
  ExportFileResult,
  ExportFormat,
  ExportTargetKind,
  FolderEntry,
  InstallAgentSkillPayload,
  InstallAgentSkillResult,
  KnowledgeBase,
  KnowledgeBaseSelection,
  ModelApiKeyStatus,
  Note,
  NoteImageAttachmentInput,
  ProposedChange,
  RequestAuditLog,
  SavedNoteImageAttachment,
  UserSettings,
  WorkspaceDocument,
  WorkspaceSnapshot,
} from "./types";

/** 浏览器开发态的默认模型设置；桌面端真实设置由 SQLite 和系统 keyring 保存。 */
const defaultBrowserUserSettings: UserSettings = {
  modelConfig: {
    provider: "openai-compatible",
    apiBase: "https://api.openai.com/v1",
    model: "gpt-4o-mini",
    keyReference: "cici-note-openai-compatible-api-key",
    enabled: false,
  },
  privacyPolicy: "allow-selected-scope",
  writeConfirmationRequired: true,
  skillSettings: {
    activationMode: "auto",
  },
};

/** 浏览器 fallback 的临时用户设置，仅用于 Vite 开发态模拟设置页交互。 */
let browserUserSettings: UserSettings = defaultBrowserUserSettings;

/** 浏览器 fallback 的临时审计日志，模拟桌面端模型和工具边界展示。 */
let browserAuditLogs: RequestAuditLog[] = [];

/** 浏览器 fallback 的临时应用事件日志，模拟桌面端设置页诊断列表。 */
let browserAppEventLogs: AppEventLog[] = [];

/** 带脱敏日志的 Tauri invoke 包装，只记录命令名、状态和耗时，不记录 payload。 */
async function invokeLogged<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const startedAt = performance.now();

  logDebug("调用 Tauri 命令。", {
    category: "frontend",
    event: "tauri_invoke",
    command,
    status: "started",
  });

  try {
    const result = await invoke<T>(command, args);

    logDebug("Tauri 命令完成。", {
      category: "frontend",
      event: "tauri_invoke",
      command,
      status: "completed",
      durationMs: performance.now() - startedAt,
    });

    return result;
  } catch (error) {
    logError("Tauri 命令失败。", {
      category: "frontend",
      event: "tauri_invoke",
      command,
      status: "failed",
      durationMs: performance.now() - startedAt,
      error,
    });

    throw error;
  }
}

/** 从前端本地 ID 中提取创建毫秒时间戳，用于同一分钟内的新会话稳定倒序。 */
function getTimestampMillisFromLocalId(id: string) {
  return id
    .split("-")
    .map((part) => Number(part))
    .find((timestampMillis) => timestampMillis >= 946_684_800_000 && timestampMillis <= 4_102_444_800_000);
}

/** 将浏览器 fallback 会话时间转成排序值，无法解析时排到列表末尾。 */
function getSessionCreatedSortKey(session: AgentSession) {
  const parsedCreatedAt = Date.parse(session.createdAt.replace(/\//g, "-"));

  return (getTimestampMillisFromLocalId(session.id) ?? parsedCreatedAt) || 0;
}

/** 按创建时间倒序排列会话历史，保持浏览器开发态与 Tauri 持久化层一致。 */
function sortSessionsByCreatedAtDesc(sessions: AgentSession[]) {
  sessions.sort((left, right) => {
    const timeDelta = getSessionCreatedSortKey(right) - getSessionCreatedSortKey(left);

    return timeDelta || right.createdAt.localeCompare(left.createdAt);
  });
}

/** 浏览器开发态内置 skills，与 Rust 内置定义保持同名同 ID，便于前后端切换验证。 */
const browserBuiltInSkills: AgentSkill[] = [
  {
    id: "skill-note-research",
    name: "note-research",
    displayName: "知识库研究",
    description: "基于已选知识库检索、阅读笔记，并给出带引用的回答。",
    instructions:
      "当用户要求查找、总结、对比或引用本地笔记时，先调用 search_notes、read_note 或 list_tree 获取依据。回答中只引用工具返回的材料；如果工具没有结果，明确说明未找到依据，不要编造来源。",
    tags: ["研究", "检索", "引用"],
    enabled: true,
    source: "built-in",
    allowAutoInvoke: true,
    createdAt: "内置",
    updatedAt: "内置",
  },
  {
    id: "skill-note-rewrite",
    name: "note-rewrite",
    displayName: "笔记改写",
    description: "改写当前笔记内容，并通过待确认 diff 交给用户决定是否写入。",
    instructions:
      "当用户要求润色、改写、压缩或扩写当前笔记时，先读取当前笔记或目标笔记。只能调用 propose_note_change 生成待确认 diff；不能声称已经修改文件，也不能绕过 original 唯一命中校验。",
    tags: ["写作", "改写", "diff"],
    enabled: true,
    source: "built-in",
    allowAutoInvoke: true,
    createdAt: "内置",
    updatedAt: "内置",
  },
  {
    id: "skill-draft-from-context",
    name: "draft-from-context",
    displayName: "上下文草稿",
    description: "基于已选 scope 创建新的 Markdown 草稿，写入前仍需用户确认。",
    instructions:
      "当用户要求生成新笔记、清单、总结稿或草稿时，可以先检索或读取相关笔记，再调用 create_note_draft。目标路径必须在当前会话允许的知识库内，正文应是完整 Markdown。",
    tags: ["草稿", "生成", "Markdown"],
    enabled: true,
    source: "built-in",
    allowAutoInvoke: true,
    createdAt: "内置",
    updatedAt: "内置",
  },
  {
    id: "skill-organize-knowledge",
    name: "organize-knowledge",
    displayName: "知识整理",
    description: "给出标签、标题、目录和关联笔记建议，不直接移动或改写文件。",
    instructions:
      "当用户要求整理知识库、补标签、规划目录或建立关联时，优先调用 list_tree、search_notes 或 read_note 获取结构与内容，再调用 suggest_organization 输出建议。该 skill 不执行文件移动或直接写入。",
    tags: ["整理", "标签", "目录"],
    enabled: true,
    source: "built-in",
    allowAutoInvoke: true,
    createdAt: "内置",
    updatedAt: "内置",
  },
];

/** 浏览器开发态模拟的文件式 skill，验证 UI 能展示 SKILL.md 来源和路径。 */
const browserFileSkills: AgentSkill[] = [
  {
    id: "skill-file-browser-demo",
    name: "meeting-note-polish",
    displayName: "会议纪要润色",
    description: "来自 ~/.cici-note/skills 的示例 SKILL.md，用于模拟文件式 skill 扫描结果。",
    instructions:
      "读取当前会议纪要上下文，保持事实和行动项不变，输出更清晰的 Markdown 结构。涉及写入时必须生成待确认 diff。",
    tags: ["文件", "会议", "写作"],
    enabled: true,
    source: "file",
    allowAutoInvoke: true,
    createdAt: "文件",
    updatedAt: "文件",
    path: "~/.cici-note/skills/meeting-note-polish/SKILL.md",
    relativePath: "meeting-note-polish/SKILL.md",
    metadata: {
      frontmatterName: "meeting-note-polish",
    },
  },
];

/** 浏览器 fallback 的临时 skills 状态，模拟桌面端 SQLite 持久化结果。 */
let browserAgentSkills: AgentSkill[] = cloneAgentSkills([...browserBuiltInSkills, ...browserFileSkills]);

declare global {
  interface Window {
    /** Tauri v2 运行时标记，官方 isTauri helper 会优先读取这个值。 */
    isTauri?: boolean;
    /** Tauri 运行时注入对象，用于区分桌面环境与浏览器开发环境。 */
    __TAURI_INTERNALS__?: unknown;
  }
}

/** 判断当前是否运行在 Tauri 桌面壳中。 */
export function isTauriRuntime() {
  return isTauri() || (typeof window !== "undefined" && Boolean(window.__TAURI_INTERNALS__));
}

/** 从 Tauri 本地层加载工作台状态，浏览器中回退到 mock 数据。 */
export async function loadWorkspaceState(): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    return createMockWorkspaceSnapshot();
  }

  return invokeLogged<WorkspaceSnapshot>("load_workspace_state");
}

/** 读取持久化 Agent 会话，浏览器中返回按当前快照清理后的会话列表。 */
export async function loadSessions(snapshot: WorkspaceSnapshot): Promise<AgentSession[]> {
  if (!isTauriRuntime()) {
    return normalizeMockSnapshotSessions(cloneWorkspaceSnapshot(snapshot)).sessions;
  }

  return invokeLogged<AgentSession[]>("load_sessions", { payload: { snapshot } });
}

/** 保存单个 Agent 会话，并返回后端归一化后的工作台快照。 */
export async function saveSession(snapshot: WorkspaceSnapshot, session: AgentSession): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const sessionIndex = nextSnapshot.sessions.findIndex((item) => item.id === session.id);

    if (sessionIndex >= 0) {
      nextSnapshot.sessions[sessionIndex] = session;
    } else {
      nextSnapshot.sessions = [session, ...nextSnapshot.sessions];
    }

    nextSnapshot.activeSessionId = session.id;

    return normalizeMockSnapshotSessions(nextSnapshot);
  }

  return invokeLogged<WorkspaceSnapshot>("save_session", { payload: { snapshot, session } });
}

/** 逻辑删除 Agent 会话；持久化记录保留 deletedAt，但普通会话列表不再展示。 */
export async function deleteSession(snapshot: WorkspaceSnapshot, sessionId: string): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const deletedSession = nextSnapshot.sessions.find((session) => session.id === sessionId);

    if (!deletedSession) {
      return normalizeMockSnapshotSessions(nextSnapshot);
    }

    deletedSession.deletedAt = "刚刚";
    deletedSession.updatedAt = "刚刚";
    nextSnapshot.sessions = nextSnapshot.sessions.filter((session) => !session.deletedAt);

    if (!nextSnapshot.sessions.some((session) => session.id === nextSnapshot.activeSessionId)) {
      nextSnapshot.activeSessionId =
        nextSnapshot.sessions.find((session) => session.knowledgeBaseIds.includes(nextSnapshot.activeKnowledgeBaseId))?.id ?? "";
    }

    return normalizeMockSnapshotSessions(nextSnapshot);
  }

  return invokeLogged<WorkspaceSnapshot>("delete_session", { payload: { snapshot, sessionId } });
}

/** 更新当前会话工具范围；桌面端会强制保留激活知识库。 */
export async function updateSessionScope(
  snapshot: WorkspaceSnapshot,
  sessionId: string,
  knowledgeBaseIds: string[],
  activeKnowledgeBaseId: string,
): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const validIds = new Set(nextSnapshot.knowledgeBases.map((knowledgeBase) => knowledgeBase.id));
    const selectedIds = new Set(knowledgeBaseIds.filter((knowledgeBaseId) => validIds.has(knowledgeBaseId)));

    selectedIds.add(activeKnowledgeBaseId);
    nextSnapshot.sessions = nextSnapshot.sessions.map((session) =>
      session.id === sessionId
        ? {
            ...session,
            knowledgeBaseIds: orderValidKnowledgeBaseIds(Array.from(selectedIds), nextSnapshot.knowledgeBases),
            updatedAt: "刚刚",
          }
        : session,
    );

    return normalizeMockSnapshotSessions(nextSnapshot);
  }

  return invokeLogged<WorkspaceSnapshot>("update_session_scope", {
    payload: { snapshot, sessionId, knowledgeBaseIds, activeKnowledgeBaseId },
  });
}

/** 恢复历史会话绑定的知识库和会话焦点；文件焦点只在会话仍有有效笔记引用时同步。 */
export async function restoreSessionContext(snapshot: WorkspaceSnapshot, sessionId: string): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = normalizeMockSnapshotSessions(cloneWorkspaceSnapshot(snapshot));
    const session = nextSnapshot.sessions.find((item) => item.id === sessionId);

    if (!session) {
      return nextSnapshot;
    }

    const nextKnowledgeBaseId =
      session.knowledgeBaseIds.find((knowledgeBaseId) =>
        nextSnapshot.knowledgeBases.some((knowledgeBase) => knowledgeBase.id === knowledgeBaseId),
      ) ??
      nextSnapshot.knowledgeBases[0]?.id ??
      "";
    const nextNoteId = getSessionNoteId(nextSnapshot, session.activeNoteId, nextKnowledgeBaseId);
    const shouldKeepCurrentFile = nextSnapshot.activeKnowledgeBaseId === nextKnowledgeBaseId;

    nextSnapshot.activeSessionId = session.id;
    nextSnapshot.activeKnowledgeBaseId = nextKnowledgeBaseId;

    if (nextNoteId) {
      nextSnapshot.activeNoteId = nextNoteId;
      nextSnapshot.activeDocumentId = "";
    } else if (!shouldKeepCurrentFile) {
      nextSnapshot.activeNoteId = nextSnapshot.notes.find((note) => note.knowledgeBaseId === nextKnowledgeBaseId)?.id ?? "";
      nextSnapshot.activeDocumentId = getFallbackDocumentId(nextSnapshot, nextKnowledgeBaseId, nextSnapshot.activeNoteId);
    }

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("restore_session_context", { payload: { snapshot, sessionId } });
}

/** 读取用户模型、隐私和写入设置；浏览器开发态返回内存默认值。 */
export async function loadUserSettings(): Promise<UserSettings> {
  if (!isTauriRuntime()) {
    return { ...browserUserSettings, modelConfig: { ...browserUserSettings.modelConfig } };
  }

  return invokeLogged<UserSettings>("load_user_settings");
}

/** 保存用户模型、隐私和写入设置；API key 由单独入口处理。 */
export async function saveUserSettings(settings: UserSettings): Promise<UserSettings> {
  if (!isTauriRuntime()) {
    browserUserSettings = cloneUserSettings(settings);

    return loadUserSettings();
  }

  return invokeLogged<UserSettings>("save_user_settings", { payload: { settings } });
}

/** 读取 Agent skills，桌面端来自 SQLite，浏览器开发态来自内存模拟状态。 */
export async function loadAgentSkills(): Promise<AgentSkill[]> {
  if (!isTauriRuntime()) {
    return cloneAgentSkills(browserAgentSkills);
  }

  return invokeLogged<AgentSkill[]>("load_agent_skills");
}

/** 打开 Cici Note 用户 Skills 文件夹；浏览器开发态只返回提示路径。 */
export async function openUserSkillsFolder(): Promise<string> {
  if (!isTauriRuntime()) {
    return "~/.cici-note/skills";
  }

  return invokeLogged<string>("open_user_skills_folder");
}

/** 新增或编辑用户自建 skill；桌面端会写入 ~/.cici-note/skills/<name>/SKILL.md。 */
export async function saveAgentSkill(skill: AgentSkill): Promise<AgentSkill> {
  if (!isTauriRuntime()) {
    const isBuiltInSkill = browserBuiltInSkills.some((builtInSkill) => builtInSkill.id === skill.id) || skill.source === "built-in";

    if (isBuiltInSkill) {
      throw new Error("内置 skill 不能编辑，只能启用或禁用。");
    }

    const normalizedSkill = normalizeBrowserFileSkill(skill);
    const existingIndex = browserAgentSkills.findIndex((item) => item.id === normalizedSkill.id);
    const hasNameConflict = existingIndex >= 0 && browserAgentSkills[existingIndex].id !== skill.id;
    const skillsWithoutPrevious = browserAgentSkills.filter((item) => item.id !== skill.id);

    if (hasNameConflict) {
      throw new Error("目标 Skill 目录已存在，请换一个 name。");
    }

    if (existingIndex >= 0) {
      browserAgentSkills = browserAgentSkills.map((item) => (item.id === normalizedSkill.id ? normalizedSkill : item));
    } else {
      browserAgentSkills = [...skillsWithoutPrevious, normalizedSkill];
    }

    return cloneAgentSkills([normalizedSkill])[0];
  }

  return invokeLogged<AgentSkill>("save_agent_skill", { payload: { skill } });
}

/** 启停任意 skill；启用的 skill 会以名称和描述进入 Agent system prompt。 */
export async function toggleAgentSkill(
  skillId: string,
  enabled: boolean,
): Promise<AgentSkill> {
  if (!isTauriRuntime()) {
    const skillIndex = browserAgentSkills.findIndex((skill) => skill.id === skillId);

    if (skillIndex < 0) {
      throw new Error("找不到要更新的 skill。");
    }

    const nextSkill: AgentSkill = {
      ...browserAgentSkills[skillIndex],
      enabled,
      updatedAt: formatLocalDateTime(),
    };

    browserAgentSkills[skillIndex] = nextSkill;

    return cloneAgentSkills([nextSkill])[0];
  }

  return invokeLogged<AgentSkill>("toggle_agent_skill", {
    payload: { skillId, enabled },
  });
}

/** 删除用户自建 skill；文件式 skill 会移除对应 SKILL.md 目录。 */
export async function deleteAgentSkill(skillId: string): Promise<AgentSkill[]> {
  if (!isTauriRuntime()) {
    const skill = browserAgentSkills.find((item) => item.id === skillId);

    if (!skill) {
      throw new Error("找不到可删除的用户 skill。");
    }

    if (skill.source === "built-in") {
      throw new Error("内置 skill 不能删除，请改为禁用。");
    }

    browserAgentSkills = browserAgentSkills.filter((item) => item.id !== skillId);

    return loadAgentSkills();
  }

  return invokeLogged<AgentSkill[]>("delete_agent_skill", { payload: { skillId } });
}

/** 安装标准 SKILL.md 包；第三方来源默认停用，用户审阅后再启用。 */
export async function installAgentSkill(payload: InstallAgentSkillPayload): Promise<InstallAgentSkillResult> {
  const startedAt = performance.now();

  logInfo("开始安装第三方 Skill。", {
    category: "skill",
    event: "install_agent_skill",
    status: "started",
    metadata: {
      sourceType: payload.sourceType,
      conflictStrategy: payload.conflictStrategy,
      enableAfterInstall: payload.enableAfterInstall,
      hasSource: Boolean(payload.source?.trim()),
    },
  });

  try {
    const result = isTauriRuntime()
      ? await invokeLogged<InstallAgentSkillResult>("install_agent_skill", { payload })
      : installBrowserMockSkill(payload);

    logInfo("第三方 Skill 安装完成。", {
      category: "skill",
      event: "install_agent_skill",
      status: "completed",
      durationMs: performance.now() - startedAt,
      metadata: {
        sourceType: result.sourceType,
        sourceSummary: result.sourceSummary,
        installedCount: result.installedCount,
        warningCount: result.warnings.length,
        fileCount: result.fileCount,
      },
    });

    return result;
  } catch (error) {
    logError("第三方 Skill 安装失败。", {
      category: "skill",
      event: "install_agent_skill",
      status: "failed",
      durationMs: performance.now() - startedAt,
      error,
      metadata: {
        sourceType: payload.sourceType,
        conflictStrategy: payload.conflictStrategy,
      },
    });
    throw error;
  }
}

/** 保存 BYOK 模型密钥；桌面端写入系统安全存储并返回读回校验状态。 */
export async function saveModelApiKey(apiKey: string): Promise<ModelApiKeyStatus> {
  if (!isTauriRuntime()) {
    throw new Error("浏览器开发态不能保存模型密钥，请在 Tauri 桌面端配置。");
  }

  return invokeLogged<ModelApiKeyStatus>("save_model_api_key", { payload: { apiKey } });
}

/** 读取 BYOK 模型密钥状态；不返回明文密钥。 */
export async function loadModelApiKeyStatus(): Promise<ModelApiKeyStatus> {
  if (!isTauriRuntime()) {
    return {
      keyReference: defaultBrowserUserSettings.modelConfig.keyReference,
      configured: false,
      message: "浏览器开发态未连接系统安全存储。",
    };
  }

  return invokeLogged<ModelApiKeyStatus>("load_model_api_key_status");
}

/** 读取最近请求审计日志，用于展示模型发送范围和工具调用摘要。 */
export async function loadRequestAuditLogs(): Promise<RequestAuditLog[]> {
  if (!isTauriRuntime()) {
    return browserAuditLogs;
  }

  return invokeLogged<RequestAuditLog[]>("load_request_audit_logs");
}

/** 读取最近应用事件日志，用于设置页展示运行诊断和关键操作。 */
export async function loadAppEventLogs(filters: {
  limit?: number;
  level?: AppEventLogLevel | "";
  category?: AppEventLogCategory | "";
} = {}): Promise<AppEventLog[]> {
  const payload = {
    limit: filters.limit ?? 100,
    level: filters.level || undefined,
    category: filters.category || undefined,
  };

  if (!isTauriRuntime()) {
    return browserAppEventLogs
      .filter((log) => !payload.level || log.level === payload.level)
      .filter((log) => !payload.category || log.category === payload.category)
      .slice(0, payload.limit);
  }

  return invokeLogged<AppEventLog[]>("load_app_event_logs", { payload });
}

/** 清空用户可读应用事件日志；桌面端不会删除文件诊断日志。 */
export async function clearAppEventLogs(): Promise<void> {
  if (!isTauriRuntime()) {
    browserAppEventLogs = [
      {
        id: createLocalId("event"),
        level: "info",
        category: "settings",
        event: "clear_app_event_logs",
        message: "已清空应用事件日志。",
        status: "completed",
        createdAt: formatLocalDateTime(),
      },
    ];
    return;
  }

  return invokeLogged<void>("clear_app_event_logs");
}

/** 打开 Tauri app log 目录，方便用户附带文件日志排查。 */
export async function openAppLogFolder(): Promise<string> {
  if (!isTauriRuntime()) {
    return "~/Library/Logs/app.cici-note.desktop";
  }

  return invokeLogged<string>("open_app_log_folder");
}

/** 通过 Tauri 目录选择器连接知识库，浏览器中创建 mock 目录。 */
export async function selectKnowledgeBaseDirectory(currentCount: number): Promise<KnowledgeBaseSelection> {
  if (!isTauriRuntime()) {
    return createMockKnowledgeBaseSelection(currentCount);
  }

  return invokeLogged<KnowledgeBaseSelection>("select_knowledge_base");
}

/** 扫描新知识库并把它合并进当前快照，浏览器中使用模拟文档。 */
export async function attachKnowledgeBase(
  snapshot: WorkspaceSnapshot,
  selection: KnowledgeBaseSelection,
): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const newKnowledgeBase: KnowledgeBase = {
      id: selection.id,
      name: selection.name,
      path: selection.path,
      description: "模拟新增的本地支持文档目录，正式版本由 Tauri 扫描真实文件。",
      status: "ready",
      noteCount: selection.noteCount,
      documentCount: 1,
      updatedAt: "刚刚",
      isDefault: false,
      semanticIndexEnabled: false,
      scanReport: {
        scannedFileCount: 2,
        scannedByType: {
          markdown: 1,
          txt: 1,
          docx: 0,
          pdf: 0,
        },
        failedFileCount: 0,
        skippedDirectories: ["node_modules"],
        errors: [],
      },
    };
    const newNote: Note = {
      id: `note-${selection.id}`,
      knowledgeBaseId: selection.id,
      title: "知识库索引",
      path: "Index/知识库索引.md",
      updatedAt: "刚刚",
      tags: ["索引", "Agent"],
      backlinks: [],
      content: `# 知识库索引

这是一个浏览器开发态模拟知识库。正式桌面版会扫描 ${selection.path} 下的支持文档。`,
      contentHash: "mock-new-note",
    };
    const newFolder: FolderEntry = {
      id: `folder-${selection.id}-index`,
      knowledgeBaseId: selection.id,
      name: "Index",
      path: "Index",
      updatedAt: "刚刚",
    };
    const newDocument: WorkspaceDocument = {
      id: `document-${selection.id}-readme`,
      knowledgeBaseId: selection.id,
      title: "资料说明",
      path: "Index/资料说明.txt",
      fileType: "txt",
      updatedAt: "刚刚",
      content: "这是一个浏览器开发态模拟 TXT 文档。",
      contentHash: createContentHash("这是一个浏览器开发态模拟 TXT 文档。"),
      previewAvailable: false,
    };

    nextSnapshot.knowledgeBases = [...nextSnapshot.knowledgeBases, newKnowledgeBase];
    nextSnapshot.folders = [...nextSnapshot.folders, newFolder];
    nextSnapshot.notes = [newNote, ...nextSnapshot.notes];
    nextSnapshot.documents = [newDocument, ...nextSnapshot.documents];
    nextSnapshot.activeKnowledgeBaseId = newKnowledgeBase.id;
    nextSnapshot.activeNoteId = newNote.id;
    nextSnapshot.activeDocumentId = "";

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("scan_knowledge_base", { payload: { snapshot, selection } });
}

/** 重新扫描已连接知识库，Tauri 环境读取真实目录，浏览器中只刷新模拟状态。 */
export async function rescanKnowledgeBase(snapshot: WorkspaceSnapshot, knowledgeBaseId: string): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);

    nextSnapshot.knowledgeBases = nextSnapshot.knowledgeBases.map((knowledgeBase) =>
      knowledgeBase.id === knowledgeBaseId ? { ...knowledgeBase, updatedAt: "刚刚", status: "ready" } : knowledgeBase,
    );

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("rescan_knowledge_base", { payload: { snapshot, knowledgeBaseId } });
}

/** 在用户点击的目录下新建空白 Markdown；桌面端会立即创建真实文件。 */
export async function createNote(
  snapshot: WorkspaceSnapshot,
  knowledgeBaseId: string,
  parentPath: string,
  fileName: string,
): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const knowledgeBase = nextSnapshot.knowledgeBases.find((item) => item.id === knowledgeBaseId);

    if (!knowledgeBase) {
      return nextSnapshot;
    }

    const safeFileName = validateNewMarkdownFileNameForMock(fileName);
    const normalizedParentPath = normalizeFolderPath(parentPath);
    ensureParentFolderExistsForMock(nextSnapshot, knowledgeBaseId, normalizedParentPath);
    const existingPaths = new Set([
      ...nextSnapshot.notes.filter((note) => note.knowledgeBaseId === knowledgeBaseId).map((note) => note.path),
      ...nextSnapshot.documents.filter((document) => document.knowledgeBaseId === knowledgeBaseId).map((document) => document.path),
    ]);
    const nextPath = joinRelativePath(normalizedParentPath, safeFileName);

    // 浏览器 fallback 只模拟正式桌面行为，仍然不能覆盖已有 Markdown。
    if (existingPaths.has(nextPath)) {
      throw new Error("目标文件已存在，已阻止覆盖。");
    }

    const newNote: Note = {
      id: createLocalId("note"),
      knowledgeBaseId,
      title: safeFileName.replace(/\.(md|markdown)$/i, ""),
      path: nextPath,
      content: "",
      tags: [],
      updatedAt: "刚刚",
      backlinks: [],
      contentHash: createContentHash(""),
    };

    nextSnapshot.notes = [newNote, ...nextSnapshot.notes];
    nextSnapshot.knowledgeBases = nextSnapshot.knowledgeBases.map((item) =>
      item.id === knowledgeBaseId
        ? {
            ...item,
            noteCount: item.noteCount + 1,
            updatedAt: "刚刚",
            scanReport: item.scanReport
              ? {
                  ...item.scanReport,
                  scannedFileCount: item.scanReport.scannedFileCount + 1,
                  scannedByType: {
                    ...item.scanReport.scannedByType,
                    markdown: item.scanReport.scannedByType.markdown + 1,
                  },
                }
              : item.scanReport,
          }
        : item,
    );
    nextSnapshot.activeKnowledgeBaseId = knowledgeBaseId;
    nextSnapshot.activeNoteId = newNote.id;
    nextSnapshot.activeDocumentId = "";

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("create_note", {
    payload: { snapshot, knowledgeBaseId, parentPath, fileName },
  });
}

/** 在用户点击的目录下新建空白 TXT；桌面端会立即创建真实文件。 */
export async function createDocument(
  snapshot: WorkspaceSnapshot,
  knowledgeBaseId: string,
  parentPath: string,
  fileName: string,
): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const knowledgeBase = nextSnapshot.knowledgeBases.find((item) => item.id === knowledgeBaseId);

    if (!knowledgeBase) {
      return nextSnapshot;
    }

    const safeFileName = validateNewTextDocumentFileNameForMock(fileName);
    const normalizedParentPath = normalizeFolderPath(parentPath);
    ensureParentFolderExistsForMock(nextSnapshot, knowledgeBaseId, normalizedParentPath);
    const existingPaths = new Set([
      ...nextSnapshot.notes.filter((note) => note.knowledgeBaseId === knowledgeBaseId).map((note) => note.path),
      ...nextSnapshot.documents.filter((document) => document.knowledgeBaseId === knowledgeBaseId).map((document) => document.path),
    ]);
    const nextPath = joinRelativePath(normalizedParentPath, safeFileName);

    // 浏览器 fallback 也模拟桌面文件系统的同目录不可覆盖规则。
    if (existingPaths.has(nextPath)) {
      throw new Error("目标文件已存在，已阻止覆盖。");
    }

    const newDocument: WorkspaceDocument = {
      id: createLocalId("document"),
      knowledgeBaseId,
      title: safeFileName.replace(/\.txt$/i, ""),
      path: nextPath,
      fileType: "txt",
      content: "",
      contentHash: createContentHash(""),
      updatedAt: "刚刚",
      previewAvailable: false,
    };

    nextSnapshot.documents = [newDocument, ...nextSnapshot.documents];
    nextSnapshot.knowledgeBases = nextSnapshot.knowledgeBases.map((item) =>
      item.id === knowledgeBaseId
        ? {
            ...item,
            documentCount: item.documentCount + 1,
            updatedAt: "刚刚",
            scanReport: item.scanReport
              ? {
                  ...item.scanReport,
                  scannedFileCount: item.scanReport.scannedFileCount + 1,
                  scannedByType: {
                    ...item.scanReport.scannedByType,
                    txt: item.scanReport.scannedByType.txt + 1,
                  },
                }
              : item.scanReport,
          }
        : item,
    );
    nextSnapshot.activeKnowledgeBaseId = knowledgeBaseId;
    nextSnapshot.activeNoteId = "";
    nextSnapshot.activeDocumentId = newDocument.id;

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("create_document", {
    payload: { snapshot, knowledgeBaseId, parentPath, fileName },
  });
}

/** 在用户点击的目录下新建单级文件夹；桌面端会立即创建真实目录。 */
export async function createFolder(
  snapshot: WorkspaceSnapshot,
  knowledgeBaseId: string,
  parentPath: string,
  folderName: string,
): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const safeFolderName = validateFolderNameForMock(folderName);
    const normalizedParentPath = normalizeFolderPath(parentPath);
    const nextFolderPath = joinRelativePath(normalizedParentPath, safeFolderName);

    ensureParentFolderExistsForMock(nextSnapshot, knowledgeBaseId, normalizedParentPath);

    const isPathTaken =
      nextSnapshot.folders.some((folder) => folder.knowledgeBaseId === knowledgeBaseId && folder.path === nextFolderPath) ||
      nextSnapshot.notes.some((note) => note.knowledgeBaseId === knowledgeBaseId && note.path === nextFolderPath) ||
      nextSnapshot.documents.some((document) => document.knowledgeBaseId === knowledgeBaseId && document.path === nextFolderPath);

    // 文件和目录共用同一命名空间，模拟桌面文件系统不能同名覆盖的规则。
    if (isPathTaken) {
      throw new Error("目标文件夹已存在，已阻止覆盖。");
    }

    const folderEntry: FolderEntry = {
      id: createLocalId("folder"),
      knowledgeBaseId,
      name: safeFolderName,
      path: nextFolderPath,
      updatedAt: "刚刚",
    };

    nextSnapshot.folders = [...nextSnapshot.folders, folderEntry];
    nextSnapshot.knowledgeBases = nextSnapshot.knowledgeBases.map((knowledgeBase) =>
      knowledgeBase.id === knowledgeBaseId ? { ...knowledgeBase, updatedAt: "刚刚" } : knowledgeBase,
    );
    nextSnapshot.activeKnowledgeBaseId = knowledgeBaseId;

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("create_folder", {
    payload: { snapshot, knowledgeBaseId, parentPath, folderName },
  });
}

/** 保存当前笔记正文，Tauri 环境执行路径边界和 hash 校验，浏览器中更新内存快照。 */
export async function saveNoteContent(
  snapshot: WorkspaceSnapshot,
  noteId: string,
  content: string,
  expectedHash: string,
): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);

    nextSnapshot.notes = nextSnapshot.notes.map((note) =>
      note.id === noteId ? { ...note, content, contentHash: createContentHash(content), updatedAt: "刚刚" } : note,
    );

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("save_note_content", { payload: { snapshot, noteId, content, expectedHash } });
}

/** 保存当前 Markdown 粘贴图片附件；只写图片文件，不自动保存正文草稿。 */
export async function saveNoteImageAttachments(
  snapshot: WorkspaceSnapshot,
  noteId: string,
  images: NoteImageAttachmentInput[],
): Promise<SavedNoteImageAttachment[]> {
  if (!isTauriRuntime()) {
    throw new Error("浏览器开发态不能保存本地图片附件，请在 Tauri 桌面端使用粘贴图片。");
  }

  return invokeLogged<SavedNoteImageAttachment[]>("save_note_image_attachments", {
    payload: { snapshot, noteId, images },
  });
}

/** 保存当前 TXT 文档正文；桌面端会执行 hash 冲突检测和原子写入。 */
export async function saveDocumentContent(
  snapshot: WorkspaceSnapshot,
  documentId: string,
  content: string,
  expectedHash: string,
): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const document = nextSnapshot.documents.find((item) => item.id === documentId);

    if (!document) {
      throw new Error("找不到要保存的文档。");
    }

    if (document.fileType !== "txt") {
      throw new Error("只有 TXT 文档支持保存。");
    }

    if (document.contentHash !== expectedHash) {
      throw new Error("目标文件已被外部修改，已阻止保存。请重新扫描后再编辑。");
    }

    nextSnapshot.documents = nextSnapshot.documents.map((item) =>
      item.id === documentId ? { ...item, content, contentHash: createContentHash(content), updatedAt: "刚刚" } : item,
    );
    nextSnapshot.activeNoteId = "";
    nextSnapshot.activeDocumentId = documentId;

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("save_document_content", { payload: { snapshot, documentId, content, expectedHash } });
}

/** 重命名当前 Markdown 文件；桌面端调用真实 Tauri 文件系统能力，浏览器仅做开发态内存 fallback。 */
export async function renameNote(
  snapshot: WorkspaceSnapshot,
  noteId: string,
  nextFileName: string,
): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const note = nextSnapshot.notes.find((item) => item.id === noteId);

    if (!note) {
      throw new Error("找不到要重命名的笔记。");
    }

    const safeFileName = validateMarkdownFileNameForMock(nextFileName);
    const nextPath = replaceFileNameInPath(note.path, safeFileName);
    const isPathTaken =
      nextSnapshot.notes.some(
        (item) => item.knowledgeBaseId === note.knowledgeBaseId && item.id !== note.id && item.path === nextPath,
      ) ||
      nextSnapshot.documents.some((item) => item.knowledgeBaseId === note.knowledgeBaseId && item.path === nextPath);

    // 浏览器 fallback 只模拟正式桌面行为，仍然不能覆盖同目录已有 Markdown。
    if (isPathTaken) {
      throw new Error("目标文件名已存在，已阻止覆盖。");
    }

    const nextNoteId = createLocalId("note-renamed");

    nextSnapshot.notes = nextSnapshot.notes.map((item) =>
      item.id === note.id
        ? {
            ...item,
            id: nextNoteId,
            path: nextPath,
            title: getTitleFromMarkdownOrFileName(item.content, safeFileName),
            updatedAt: "刚刚",
          }
        : item,
    );
    migrateNoteReferencesAfterRename(nextSnapshot, note.id, nextNoteId, nextPath);
    nextSnapshot.activeDocumentId = "";

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("rename_note", { payload: { snapshot, noteId, nextFileName } });
}

/** 重命名当前 TXT 文档；桌面端调用真实 Tauri 文件系统能力。 */
export async function renameDocument(
  snapshot: WorkspaceSnapshot,
  documentId: string,
  nextFileName: string,
): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const document = nextSnapshot.documents.find((item) => item.id === documentId);

    if (!document) {
      throw new Error("找不到要重命名的文档。");
    }

    if (document.fileType !== "txt") {
      throw new Error("只有 TXT 文档支持重命名。");
    }

    const safeFileName = validateTextDocumentFileNameForMock(nextFileName);
    const nextPath = replaceFileNameInPath(document.path, safeFileName);
    const isPathTaken =
      nextSnapshot.notes.some((item) => item.knowledgeBaseId === document.knowledgeBaseId && item.path === nextPath) ||
      nextSnapshot.documents.some(
        (item) => item.knowledgeBaseId === document.knowledgeBaseId && item.id !== document.id && item.path === nextPath,
      );

    // 文件系统同目录不能出现同名文件，不区分它属于 note 还是 document 模型。
    if (isPathTaken) {
      throw new Error("目标文件名已存在，已阻止覆盖。");
    }

    const nextDocumentId = createLocalId("document-renamed");

    nextSnapshot.documents = nextSnapshot.documents.map((item) =>
      item.id === document.id
        ? {
            ...item,
            id: nextDocumentId,
            path: nextPath,
            title: safeFileName.replace(/\.txt$/i, ""),
            updatedAt: "刚刚",
          }
        : item,
    );

    if (nextSnapshot.activeDocumentId === document.id) {
      nextSnapshot.activeDocumentId = nextDocumentId;
      nextSnapshot.activeNoteId = "";
    }

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("rename_document", { payload: { snapshot, documentId, nextFileName } });
}

/** 删除当前 Markdown 文件到系统回收站；浏览器 fallback 只移除内存快照中的模拟笔记。 */
export async function deleteNote(
  snapshot: WorkspaceSnapshot,
  noteId: string,
  expectedHash: string,
): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const note = nextSnapshot.notes.find((item) => item.id === noteId);

    if (!note) {
      throw new Error("找不到要删除的笔记。");
    }

    // 与桌面 Tauri command 保持一致：删除前必须确认操作基于同一份文件版本。
    if (note.contentHash !== expectedHash) {
      throw new Error("目标文件已被外部修改，已阻止删除。请重新扫描后再操作。");
    }

    nextSnapshot.notes = nextSnapshot.notes.filter((item) => item.id !== noteId);
    nextSnapshot.knowledgeBases = nextSnapshot.knowledgeBases.map((knowledgeBase) =>
      knowledgeBase.id === note.knowledgeBaseId
        ? {
            ...knowledgeBase,
            noteCount: Math.max(0, knowledgeBase.noteCount - 1),
            updatedAt: "刚刚",
            scanReport: knowledgeBase.scanReport
              ? {
                  ...knowledgeBase.scanReport,
                  scannedFileCount: Math.max(0, knowledgeBase.scanReport.scannedFileCount - 1),
                  scannedByType: {
                    ...knowledgeBase.scanReport.scannedByType,
                    markdown: Math.max(0, knowledgeBase.scanReport.scannedByType.markdown - 1),
                  },
                }
              : knowledgeBase.scanReport,
          }
        : knowledgeBase,
    );
    removeNoteReferencesAfterDelete(nextSnapshot, noteId);

    const sameKnowledgeBaseFallback = nextSnapshot.notes.find((item) => item.knowledgeBaseId === note.knowledgeBaseId);

    if (nextSnapshot.activeNoteId === noteId || !nextSnapshot.notes.some((item) => item.id === nextSnapshot.activeNoteId)) {
      nextSnapshot.activeNoteId = sameKnowledgeBaseFallback?.id ?? "";
    }
    nextSnapshot.activeDocumentId = "";

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("delete_note", { payload: { snapshot, noteId, expectedHash } });
}

/** 删除当前 TXT 文档到系统回收站；浏览器 fallback 只移除内存快照中的模拟文档。 */
export async function deleteDocument(
  snapshot: WorkspaceSnapshot,
  documentId: string,
  expectedHash: string,
): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
    const document = nextSnapshot.documents.find((item) => item.id === documentId);

    if (!document) {
      throw new Error("找不到要删除的文档。");
    }

    if (document.fileType !== "txt") {
      throw new Error("只有 TXT 文档支持删除。");
    }

    // 与桌面 Tauri command 保持一致：删除前必须确认操作基于同一份文件版本。
    if (document.contentHash !== expectedHash) {
      throw new Error("目标文件已被外部修改，已阻止删除。请重新扫描后再操作。");
    }

    nextSnapshot.documents = nextSnapshot.documents.filter((item) => item.id !== documentId);
    nextSnapshot.knowledgeBases = nextSnapshot.knowledgeBases.map((knowledgeBase) =>
      knowledgeBase.id === document.knowledgeBaseId
        ? {
            ...knowledgeBase,
            documentCount: Math.max(0, knowledgeBase.documentCount - 1),
            updatedAt: "刚刚",
            scanReport: knowledgeBase.scanReport
              ? {
                  ...knowledgeBase.scanReport,
                  scannedFileCount: Math.max(0, knowledgeBase.scanReport.scannedFileCount - 1),
                  scannedByType: {
                    ...knowledgeBase.scanReport.scannedByType,
                    txt: Math.max(0, knowledgeBase.scanReport.scannedByType.txt - 1),
                  },
                }
              : knowledgeBase.scanReport,
          }
        : knowledgeBase,
    );

    if (nextSnapshot.activeDocumentId === documentId) {
      const sameKnowledgeBaseFallback = nextSnapshot.documents.find((item) => item.knowledgeBaseId === document.knowledgeBaseId);

      nextSnapshot.activeDocumentId = sameKnowledgeBaseFallback?.id ?? "";
      nextSnapshot.activeNoteId = sameKnowledgeBaseFallback ? "" : nextSnapshot.notes.find((item) => item.knowledgeBaseId === document.knowledgeBaseId)?.id ?? "";
    }

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("delete_document", { payload: { snapshot, documentId, expectedHash } });
}

/** 加载 DOCX/PDF 只读预览；TXT 直接使用快照中的 content。 */
export async function loadDocumentPreview(snapshot: WorkspaceSnapshot, documentId: string): Promise<DocumentPreview> {
  const document = snapshot.documents.find((item) => item.id === documentId);

  if (!document) {
    throw new Error("找不到要预览的文档。");
  }

  if (!isTauriRuntime()) {
    return {
      documentId: document.id,
      fileType: document.fileType,
      title: document.title,
      path: document.path,
      updatedAt: document.updatedAt,
      contentHash: document.contentHash,
      assetPath: document.fileType === "pdf" ? document.path : undefined,
      blocks:
        document.fileType === "docx"
          ? [
              { type: "heading", text: document.title },
              { type: "paragraph", text: "这是浏览器开发态模拟的 DOCX 只读预览正文。" },
            ]
          : undefined,
    };
  }

  return invokeLogged<DocumentPreview>("load_document_preview", { payload: { snapshot, documentId } });
}

/** 导出当前打开文件；真实文件写入只允许在 Tauri 桌面端通过系统保存对话框完成。 */
export async function exportCurrentFile(
  snapshot: WorkspaceSnapshot,
  targetKind: ExportTargetKind,
  targetId: string,
  format: ExportFormat,
): Promise<ExportFileResult | null> {
  if (!isTauriRuntime()) {
    throw new Error("浏览器开发态不能导出本地文件，请在 Tauri 桌面端使用导出。");
  }

  return invokeLogged<ExportFileResult | null>("export_current_file", {
    payload: { snapshot, targetKind, targetId, format },
  });
}

/** 移除知识库授权和索引缓存；不会删除用户本地文档。 */
export async function removeKnowledgeBase(snapshot: WorkspaceSnapshot, knowledgeBaseId: string): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    const nextSnapshot = cloneWorkspaceSnapshot(snapshot);

    nextSnapshot.knowledgeBases = nextSnapshot.knowledgeBases.filter((knowledgeBase) => knowledgeBase.id !== knowledgeBaseId);
    nextSnapshot.folders = nextSnapshot.folders.filter((folder) => folder.knowledgeBaseId !== knowledgeBaseId);
    nextSnapshot.notes = nextSnapshot.notes.filter((note) => note.knowledgeBaseId !== knowledgeBaseId);
    nextSnapshot.documents = nextSnapshot.documents.filter((document) => document.knowledgeBaseId !== knowledgeBaseId);
    nextSnapshot.sessions = nextSnapshot.sessions
      .map((session) => ({
        ...session,
        knowledgeBaseIds: session.knowledgeBaseIds.filter((id) => id !== knowledgeBaseId),
        pinnedNoteIds: session.pinnedNoteIds.filter((noteId) => nextSnapshot.notes.some((note) => note.id === noteId)),
      }))
      .filter((session) => session.knowledgeBaseIds.length > 0);

    const activeKnowledgeBase = nextSnapshot.knowledgeBases.find(
      (knowledgeBase) => knowledgeBase.id === nextSnapshot.activeKnowledgeBaseId,
    );
    const fallbackKnowledgeBase = activeKnowledgeBase ?? nextSnapshot.knowledgeBases[0];

    nextSnapshot.knowledgeBases = nextSnapshot.knowledgeBases.map((knowledgeBase, index) => ({
      ...knowledgeBase,
      isDefault: index === 0,
    }));
    nextSnapshot.activeKnowledgeBaseId = fallbackKnowledgeBase?.id ?? "";
    nextSnapshot.activeNoteId = nextSnapshot.notes.find((note) => note.knowledgeBaseId === nextSnapshot.activeKnowledgeBaseId)?.id ?? "";
    nextSnapshot.activeDocumentId = nextSnapshot.activeNoteId
      ? ""
      : nextSnapshot.documents.find((document) => document.knowledgeBaseId === nextSnapshot.activeKnowledgeBaseId)?.id ?? "";
    nextSnapshot.activeSessionId =
      nextSnapshot.sessions.find((session) => session.knowledgeBaseIds.includes(nextSnapshot.activeKnowledgeBaseId))?.id ??
      "";

    if (!nextSnapshot.knowledgeBases.length) {
      nextSnapshot.sessions = [];
      nextSnapshot.activeKnowledgeBaseId = "";
      nextSnapshot.activeNoteId = "";
      nextSnapshot.activeDocumentId = "";
      nextSnapshot.activeSessionId = "";
    }

    return nextSnapshot;
  }

  return invokeLogged<WorkspaceSnapshot>("remove_knowledge_base", { payload: { snapshot, knowledgeBaseId } });
}

/** 运行 Agent 单轮 loop，模型可在内部自行选择是否调用检索工具。 */
export async function runAgentTurn(
  snapshot: WorkspaceSnapshot,
  prompt: string,
  action: AgentActionType,
  clientMessageId?: string,
): Promise<AgentTurnResult> {
  const request: AgentTurnRequest = {
    prompt,
    action,
    sessionId: snapshot.activeSessionId,
    activeKnowledgeBaseId: snapshot.activeKnowledgeBaseId,
    activeNoteId: snapshot.activeNoteId,
    clientMessageId,
  };

  if (!isTauriRuntime()) {
    logBrowserSkillContext(browserAgentSkills, request);
    const nextSnapshot = runMockAgentTurn(snapshot, prompt, action, clientMessageId);

    browserAuditLogs = [createBrowserAuditLog(nextSnapshot, prompt), ...browserAuditLogs].slice(0, 20);

    return { snapshot: nextSnapshot };
  }

  return invokeLogged<AgentTurnResult>("run_agent_turn", { payload: { snapshot, request } });
}

/** 接受当前会话的待确认变更，Tauri 环境中由本地层执行安全写入。 */
export async function acceptProposedChange(snapshot: WorkspaceSnapshot): Promise<WorkspaceSnapshot> {
  if (!isTauriRuntime()) {
    return normalizeMockSnapshotSessions(acceptMockProposedChange(snapshot));
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

/** 浏览器开发态使用的文件名校验，保持与 Rust 层正式规则一致。 */
function validateMarkdownFileNameForMock(fileName: string) {
  const trimmedFileName = fileName.trim();

  if (!trimmedFileName) {
    throw new Error("文件名不能为空。");
  }

  // 重命名只改当前目录下的文件名，不能携带路径分隔符或上级目录。
  if (trimmedFileName.includes("/") || trimmedFileName.includes("\\") || trimmedFileName === "." || trimmedFileName === "..") {
    throw new Error("文件名不能包含路径或上级目录。");
  }

  if (!/\.(md|markdown)$/i.test(trimmedFileName)) {
    throw new Error("文件名必须以 .md 或 .markdown 结尾。");
  }

  return trimmedFileName;
}

/** 浏览器开发态的新建 Markdown 文件名校验；允许省略扩展名并默认补 .md。 */
function validateNewMarkdownFileNameForMock(fileName: string) {
  const trimmedFileName = fileName.trim();

  if (!trimmedFileName) {
    throw new Error("文件名不能为空。");
  }

  const normalizedFileName = /\.[^./\\]+$/.test(trimmedFileName) ? trimmedFileName : `${trimmedFileName}.md`;

  return validateMarkdownFileNameForMock(normalizedFileName);
}

/** 浏览器开发态的 TXT 文件名校验，只允许当前目录下的 .txt 文件。 */
function validateTextDocumentFileNameForMock(fileName: string) {
  const trimmedFileName = fileName.trim();

  if (!trimmedFileName) {
    throw new Error("文件名不能为空。");
  }

  // 重命名只改当前目录下的文件名，不能携带路径分隔符或上级目录。
  if (trimmedFileName.includes("/") || trimmedFileName.includes("\\") || trimmedFileName === "." || trimmedFileName === "..") {
    throw new Error("文件名不能包含路径或上级目录。");
  }

  if (!/\.txt$/i.test(trimmedFileName)) {
    throw new Error("文件名必须以 .txt 结尾。");
  }

  return trimmedFileName;
}

/** 浏览器开发态的新建 TXT 文件名校验；允许省略扩展名并默认补 .txt。 */
function validateNewTextDocumentFileNameForMock(fileName: string) {
  const trimmedFileName = fileName.trim();

  if (!trimmedFileName) {
    throw new Error("文件名不能为空。");
  }

  const normalizedFileName = /\.[^./\\]+$/.test(trimmedFileName) ? trimmedFileName : `${trimmedFileName}.txt`;

  return validateTextDocumentFileNameForMock(normalizedFileName);
}

/** 浏览器开发态的新建目录名校验，只允许单级普通目录名。 */
function validateFolderNameForMock(folderName: string) {
  const trimmedFolderName = folderName.trim();
  const ignoredDirectoryNames = new Set([
    ".git",
    ".hg",
    ".svn",
    ".idea",
    ".vscode",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".nuxt",
    ".turbo",
    ".cache",
  ]);

  if (!trimmedFolderName) {
    throw new Error("文件夹名不能为空。");
  }

  // 新建目录只允许单级名称，不能通过浏览器 fallback 伪造路径穿越或多级目录。
  if (
    trimmedFolderName.includes("/") ||
    trimmedFolderName.includes("\\") ||
    trimmedFolderName === "." ||
    trimmedFolderName === ".."
  ) {
    throw new Error("文件夹名不能包含路径或上级目录。");
  }

  if (trimmedFolderName.startsWith(".") || ignoredDirectoryNames.has(trimmedFolderName)) {
    throw new Error("不能创建隐藏目录或扫描忽略目录。");
  }

  return trimmedFolderName;
}

/** 规范化目录相对路径，根目录统一为空字符串。 */
function normalizeFolderPath(folderPath: string) {
  return folderPath.trim().replace(/^\/+|\/+$/g, "");
}

/** 拼接知识库内相对路径，根目录下只返回子名称。 */
function joinRelativePath(parentPath: string, childName: string) {
  return parentPath ? `${parentPath}/${childName}` : childName;
}

/** 浏览器 fallback 确认父目录存在，避免新建时隐式创建多级目录。 */
function ensureParentFolderExistsForMock(snapshot: WorkspaceSnapshot, knowledgeBaseId: string, parentPath: string) {
  if (!parentPath) {
    return;
  }

  const parentExists = snapshot.folders.some((folder) => folder.knowledgeBaseId === knowledgeBaseId && folder.path === parentPath);

  if (!parentExists) {
    throw new Error("目标父目录不存在，已阻止新建。");
  }
}

/** 在相对路径中替换最后一级文件名，模拟桌面端“只改文件名”的重命名语义。 */
function replaceFileNameInPath(relativePath: string, nextFileName: string) {
  const pathParts = relativePath.split("/");

  pathParts[pathParts.length - 1] = nextFileName;

  return pathParts.join("/");
}

/** 预览或重命名时从正文一级标题提取展示标题，没有一级标题时使用文件名 stem。 */
function getTitleFromMarkdownOrFileName(content: string, fileName: string) {
  const markdownTitle = content
    .split(/\r?\n/)
    .find((line) => line.trim().startsWith("# "))
    ?.trim()
    .replace(/^#\s+/, "")
    .trim();

  if (markdownTitle) {
    return markdownTitle;
  }

  return fileName.replace(/\.(md|markdown)$/i, "") || "未命名笔记";
}

/** 重命名后迁移当前笔记、固定笔记和待确认 diff 引用。 */
function migrateNoteReferencesAfterRename(
  snapshot: WorkspaceSnapshot,
  previousNoteId: string,
  nextNoteId: string,
  nextPath: string,
) {
  if (snapshot.activeNoteId === previousNoteId) {
    snapshot.activeNoteId = nextNoteId;
  }

  snapshot.sessions = snapshot.sessions.map((session) => {
    const nextPendingChange = migratePendingChangeAfterRename(session.pendingChange, previousNoteId, nextNoteId, nextPath);

    return {
      ...session,
      activeNoteId: session.activeNoteId === previousNoteId ? nextNoteId : session.activeNoteId,
      pinnedNoteIds: Array.from(
        new Set(session.pinnedNoteIds.map((pinnedNoteId) => (pinnedNoteId === previousNoteId ? nextNoteId : pinnedNoteId))),
      ),
      pendingChange: nextPendingChange,
    };
  });
}

/** 迁移待确认 diff 中的目标笔记和路径，避免重命名后 diff 仍指向旧文件。 */
function migratePendingChangeAfterRename(
  pendingChange: ProposedChange | undefined,
  previousNoteId: string,
  nextNoteId: string,
  nextPath: string,
) {
  if (pendingChange?.noteId !== previousNoteId) {
    return pendingChange;
  }

  return { ...pendingChange, noteId: nextNoteId, targetPath: nextPath };
}

/** 删除后清理会话中的笔记引用和待确认 diff。 */
function removeNoteReferencesAfterDelete(snapshot: WorkspaceSnapshot, noteId: string) {
  snapshot.sessions = snapshot.sessions.map((session) => ({
    ...session,
    activeNoteId: session.activeNoteId === noteId ? undefined : session.activeNoteId,
    pinnedNoteIds: session.pinnedNoteIds.filter((pinnedNoteId) => pinnedNoteId !== noteId),
    pendingChange: session.pendingChange?.noteId === noteId ? undefined : session.pendingChange,
  }));
}

/** 深拷贝 skill 列表，避免浏览器 mock 状态被 React 组件直接修改。 */
function cloneAgentSkills(skills: AgentSkill[]) {
  return skills.map((skill) => ({
    ...skill,
    tags: [...skill.tags],
    metadata: skill.metadata ? { ...skill.metadata } : undefined,
  }));
}

/** 深拷贝用户设置，保证浏览器开发态保存和读取行为接近桌面端持久化。 */
function cloneUserSettings(settings: UserSettings): UserSettings {
  return {
    ...settings,
    modelConfig: { ...settings.modelConfig },
    skillSettings: settings.skillSettings ? { ...settings.skillSettings } : { activationMode: "auto" },
  };
}

/** 归一化浏览器开发态用户 skill，并模拟桌面端写入 SKILL.md 后返回 file 来源。 */
function normalizeBrowserFileSkill(skill: AgentSkill): AgentSkill {
  const now = formatLocalDateTime();
  const normalizedName = normalizeBrowserSkillName(skill.name || skill.displayName || skill.id);
  const relativePath = `${normalizedName}/SKILL.md`;
  const nextId = `skill-file-browser-${normalizedName || createLocalId("skill")}`;
  const normalizedSkill: AgentSkill = {
    ...skill,
    id: nextId,
    name: normalizedName,
    displayName: skill.displayName.trim(),
    description: skill.description.trim(),
    instructions: skill.instructions.trim(),
    tags: normalizeBrowserTerms(skill.tags),
    enabled: skill.enabled,
    source: "file",
    allowAutoInvoke: true,
    createdAt: skill.createdAt.trim() || now,
    updatedAt: now,
    path: `~/.cici-note/skills/${relativePath}`,
    relativePath,
    metadata: {
      frontmatterName: normalizedName,
      ...(skill.metadata ?? {}),
    },
  };

  if (!normalizedSkill.displayName) {
    throw new Error("Skill 名称不能为空。");
  }

  if (!normalizedSkill.description) {
    throw new Error("Skill 描述不能为空。");
  }

  if (!normalizedSkill.instructions) {
    throw new Error("Skill 执行说明不能为空。");
  }

  return normalizedSkill;
}

/** 浏览器开发态模拟第三方 skill 安装，便于不启动 Tauri 时验证设置页流程。 */
function installBrowserMockSkill(payload: InstallAgentSkillPayload): InstallAgentSkillResult {
  const now = formatLocalDateTime();
  const sourceSummary = summarizeBrowserSkillInstallSource(payload);
  const skillName = buildBrowserInstalledSkillName(payload);
  const normalizedSkill = normalizeBrowserFileSkill({
    id: "",
    name: skillName,
    displayName: `安装 Skill ${skillName}`,
    description: "浏览器开发态模拟安装的第三方 SKILL.md，桌面端会读取真实来源并验证 frontmatter。",
    instructions:
      "这是浏览器开发态的安装模拟能力。真实桌面端会在安装后默认停用第三方 skill，用户审阅并启用后才会进入 Runtime。",
    tags: ["安装", "模拟"],
    enabled: payload.enableAfterInstall,
    source: "file",
    allowAutoInvoke: true,
    createdAt: now,
    updatedAt: now,
    metadata: {
      installSourceType: payload.sourceType,
      installSourceSummary: sourceSummary,
    },
  });
  const existingSkill = browserAgentSkills.find((skill) => skill.id === normalizedSkill.id || skill.name === normalizedSkill.name);

  if (existingSkill && payload.conflictStrategy === "fail") {
    throw new Error("目标 Skill 目录已存在，请勾选替换同名 Skill 后重试。");
  }

  browserAgentSkills = [
    ...browserAgentSkills.filter((skill) => skill.id !== normalizedSkill.id && skill.name !== normalizedSkill.name),
    normalizedSkill,
  ];

  const installedSkills = cloneAgentSkills([normalizedSkill]);

  return {
    installedSkills,
    skills: cloneAgentSkills(browserAgentSkills),
    warnings: [],
    summary: "已安装 1 个 Skill，复制 1 个文件。",
    sourceType: payload.sourceType,
    sourceSummary,
    installedCount: 1,
    fileCount: 1,
  };
}

/** 根据安装来源生成稳定 mock 名称，避免浏览器开发态反复安装产生不可追踪 ID。 */
function buildBrowserInstalledSkillName(payload: InstallAgentSkillPayload) {
  const source = payload.source?.trim();
  const sourceTail = source ? source.split(/[\\/]/).filter(Boolean).at(-1) ?? source : payload.sourceType;
  const withoutExtension = sourceTail.replace(/\.(zip|md|markdown)$/i, "");
  const normalizedName = normalizeBrowserSkillName(withoutExtension || payload.sourceType);

  return normalizedName || `installed-${payload.sourceType.toLowerCase()}`;
}

/** 生成安装来源脱敏摘要，只保留 host、文件名或选择器类型。 */
function summarizeBrowserSkillInstallSource(payload: InstallAgentSkillPayload) {
  const source = payload.source?.trim();

  if (payload.sourceType === "url") {
    if (!source) {
      return "url:empty";
    }

    try {
      const parsedUrl = new URL(source);

      return parsedUrl.host || "url:unknown";
    } catch {
      return "url:invalid";
    }
  }

  if (!source) {
    return payload.sourceType === "localFolder" ? "local:folder-picker" : "local:archive-picker";
  }

  const fileName = source.split(/[\\/]/).filter(Boolean).at(-1);

  return fileName ? `local:${fileName}` : "local:selected";
}

/** 把用户输入的 skill name 转成稳定标识，便于 selector 和 prompt 识别。 */
function normalizeBrowserSkillName(name: string) {
  return name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, "-")
    .split("-")
    .filter(Boolean)
    .join("-");
}

/** 清理标签，去重并限制数量，避免 mock prompt 摘要失控。 */
function normalizeBrowserTerms(terms: string[]) {
  const seenTerms = new Set<string>();

  return terms
    .map((term) => term.trim())
    .filter(Boolean)
    .filter((term) => {
      const key = term.toLowerCase();

      if (seenTerms.has(key)) {
        return false;
      }

      seenTerms.add(key);
      return true;
    })
    .slice(0, 16);
}

/** 浏览器 mock 只记录已启用 skill 数量，具体是否使用由真实模型场景自行判断。 */
function logBrowserSkillContext(skills: AgentSkill[], request: AgentTurnRequest): void {
  logDebug("浏览器 mock 未预先选择 Skill。", {
    category: "frontend",
    event: "resolve_skill",
    status: "skipped",
    metadata: {
      action: request.action,
      enabledSkillCount: skills.filter((skill) => skill.enabled).length,
    },
  });
}

/** 浏览器 fallback 清理会话中的失效知识库和笔记引用，模拟后端持久化入口的归一化。 */
function normalizeMockSnapshotSessions(snapshot: WorkspaceSnapshot): WorkspaceSnapshot {
  snapshot.documents = snapshot.documents ?? [];

  const knowledgeBaseIds = new Set(snapshot.knowledgeBases.map((knowledgeBase) => knowledgeBase.id));
  const noteIds = new Set(snapshot.notes.map((note) => note.id));
  const documentIds = new Set(snapshot.documents.map((document) => document.id));

  snapshot.sessions = snapshot.sessions
    .filter((session) => !session.deletedAt)
    .map((session) => ({
      ...session,
      knowledgeBaseIds: orderValidKnowledgeBaseIds(
        session.knowledgeBaseIds.filter((knowledgeBaseId) => knowledgeBaseIds.has(knowledgeBaseId)),
        snapshot.knowledgeBases,
      ),
      activeNoteId: session.activeNoteId && noteIds.has(session.activeNoteId) ? session.activeNoteId : undefined,
      pinnedNoteIds: session.pinnedNoteIds.filter((noteId) => noteIds.has(noteId)),
      pendingChange:
        session.pendingChange?.noteId && !noteIds.has(session.pendingChange.noteId) ? undefined : session.pendingChange,
    }))
    .filter((session) => session.knowledgeBaseIds.length > 0);
  sortSessionsByCreatedAtDesc(snapshot.sessions);

  if (!snapshot.sessions.some((session) => session.id === snapshot.activeSessionId)) {
    snapshot.activeSessionId =
      snapshot.sessions.find((session) => session.knowledgeBaseIds.includes(snapshot.activeKnowledgeBaseId))?.id ?? "";
  }

  if (snapshot.activeDocumentId && !documentIds.has(snapshot.activeDocumentId)) {
    snapshot.activeDocumentId = "";
  }

  return snapshot;
}

/** 按会话引用恢复同知识库 Markdown；引用无效时保持文件焦点由调用方决定。 */
function getSessionNoteId(snapshot: WorkspaceSnapshot, activeNoteId: string | undefined, knowledgeBaseId: string) {
  if (
    activeNoteId &&
    snapshot.notes.some((note) => note.id === activeNoteId && note.knowledgeBaseId === knowledgeBaseId)
  ) {
    return activeNoteId;
  }

  return "";
}

/** 无可激活 Markdown 时，用同知识库第一个普通文档填充中间面板。 */
function getFallbackDocumentId(snapshot: WorkspaceSnapshot, knowledgeBaseId: string, activeNoteId: string) {
  if (activeNoteId) {
    return "";
  }

  return snapshot.documents.find((document) => document.knowledgeBaseId === knowledgeBaseId)?.id ?? "";
}

/** 按知识库列表顺序整理范围 ID，避免 UI 多选顺序随点击行为抖动。 */
function orderValidKnowledgeBaseIds(selectedIds: string[], knowledgeBases: KnowledgeBase[]) {
  const selectedIdSet = new Set(selectedIds);

  return knowledgeBases.filter((knowledgeBase) => selectedIdSet.has(knowledgeBase.id)).map((knowledgeBase) => knowledgeBase.id);
}

/** 为浏览器 fallback 创建请求审计摘要，便于设置页预览 M3 审计信息。 */
function createBrowserAuditLog(snapshot: WorkspaceSnapshot, prompt: string): RequestAuditLog {
  const session = snapshot.sessions.find((item) => item.id === snapshot.activeSessionId) ?? snapshot.sessions[0];
  const scopeSummary =
    session?.knowledgeBaseIds
      .map((knowledgeBaseId) => snapshot.knowledgeBases.find((knowledgeBase) => knowledgeBase.id === knowledgeBaseId)?.name)
      .filter(Boolean)
      .join(" / ") || "未绑定知识库";
  const toolSummary =
    session?.messages
      .at(-1)
      ?.toolCalls?.map((toolCall) => toolCall.name)
      .join(", ") || "未调用工具";
  const skillSummary =
    session?.messages
      .at(-1)
      ?.toolCalls?.find((toolCall) => toolCall.name === "skill_context" || toolCall.name === "activate_skill")
      ?.summary ?? "没有 Skill 上下文";

  return {
    id: createLocalId("audit"),
    kind: "browser_mock_turn",
    sessionId: session?.id,
    scopeSummary,
    contentSummary: `浏览器 mock；${skillSummary}；输入长度 ${prompt.length} 字符`,
    toolSummary,
    createdAt: formatLocalDateTime(),
  };
}
