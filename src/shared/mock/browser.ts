import { createContentHash, createLocalId, formatLocalDateTime } from "../id";
import { logDebug, logInfo } from "../logger";
import type {
  AgentActionType,
  AgentContextSummary,
  AgentPromptDump,
  AgentSkill,
  AgentSession,
  AgentTurnRequest,
  AppEventLog,
  DocumentHistoryEntry,
  DocumentHistoryFileType,
  DocumentHistorySource,
  DocumentHistoryTargetKind,
  FolderEntry,
  ImGatewayStatus,
  ImIntegrationSettings,
  ImProviderSettings,
  InstallAgentSkillPayload,
  InstallAgentSkillResult,
  OnlineSkill,
  OnlineSkillPreview,
  OnlineSkillSearchResult,
  SearchOnlineSkillsPayload,
  KnowledgeBase,
  LlmProviderConfig,
  LlmProviderModel,
  Note,
  ProposedChange,
  ProviderTemplate,
  RequestAuditLog,
  UserSettings,
  WorkspaceSnapshot,
} from "../types";

/** 迁移出的默认 provider 固定 ID，和后端 MIGRATED_DEFAULT_PROVIDER_ID 保持一致。 */
export const DEFAULT_PROVIDER_ID = "default";

/** 浏览器开发态默认 provider 实例；桌面端真实设置由 SQLite 和系统 keyring 保存。 */
export const defaultBrowserProvider: LlmProviderConfig = {
  id: DEFAULT_PROVIDER_ID,
  name: "默认 Provider",
  provider: "openai-compatible",
  apiBase: "https://api.openai.com/v1",
  model: "gpt-4o-mini",
  keyReference: "orange-openai-compatible-api-key",
  enabled: false,
  supportsTools: true,
  requiresApiKey: true,
  models: [
    {
      id: "gpt-4o-mini",
      name: "gpt-4o-mini",
      enabled: true,
      source: "manual",
      contextLength: 128000,
      updatedAt: "刚刚",
    },
  ],
  createdAt: "刚刚",
  updatedAt: "刚刚",
};

/** 浏览器开发态的默认模型设置；桌面端真实设置由 SQLite 和系统 keyring 保存。 */
export const defaultBrowserUserSettings: UserSettings = {
  modelConfig: {
    enabled: false,
    defaultProviderId: DEFAULT_PROVIDER_ID,
    providers: [defaultBrowserProvider],
  },
  privacyPolicy: "allow-selected-scope",
  writeConfirmationRequired: true,
  agentSecurity: {
    defaultLevel: "basic",
    advancedExecutionEnabled: false,
    autonomousModeEnabled: false,
    resourceLimits: { timeoutSeconds: 120, maxMemoryMb: 512, maxProcesses: 20, maxArtifactMb: 100 },
    trustedSkillGrants: [],
    allowedNetworkDomains: [],
  },
};

/** 浏览器开发态默认飞书 provider；桌面端真实设置由 SQLite 和系统 keyring 保存。 */
export const defaultBrowserFeishuProvider: ImProviderSettings = {
  providerId: "feishu",
  enabled: false,
  defaultKnowledgeBaseIds: [],
  allowedUserOpenIds: [],
  allowedChatIds: [],
  discoveredUserOpenIds: [],
  discoveredChatIds: [],
  requireMention: true,
  updatedAt: "刚刚",
  config: {
    type: "feishu",
    domain: "feishu",
    appId: "",
    secretKeyReference: "orange-feishu-app-secret",
  },
};

/** 浏览器开发态默认 IM 设置；桌面端真实设置由 SQLite 和系统 keyring 保存。 */
export const defaultBrowserImSettings: ImIntegrationSettings = {
  providers: [defaultBrowserFeishuProvider],
};

/** 浏览器开发态镜像后端内置模板，只用于模拟设置页“新增 Provider”入口。 */
export const browserProviderTemplates: ProviderTemplate[] = [
  {
    templateId: "openai",
    name: "OpenAI",
    provider: "openai-compatible",
    apiBase: "https://api.openai.com/v1",
    model: "gpt-4o-mini",
    requiresApiKey: true,
  },

  {
    templateId: "deepseek",
    name: "DeepSeek",
    provider: "openai-compatible",
    apiBase: "https://api.deepseek.com/v1",
    model: "deepseek-chat",
    requiresApiKey: true,
  },

  {
    templateId: "openrouter",
    name: "OpenRouter",
    provider: "openai-compatible",
    apiBase: "https://openrouter.ai/api/v1",
    model: "openai/gpt-4o-mini",
    requiresApiKey: true,
  },

  {
    templateId: "ollama",
    name: "Ollama（本地）",
    provider: "openai-compatible",
    apiBase: "http://localhost:11434/v1",
    model: "llama3.1",
    requiresApiKey: false,
  },

  {
    templateId: "custom",
    name: "自定义兼容服务",
    provider: "openai-compatible",
    apiBase: "",
    model: "",
    requiresApiKey: true,
  },

];

/** 从 IM 设置中读取飞书 provider；浏览器态缺失时回退默认值，避免旧 mock 状态崩溃。 */
export function getFeishuProvider(settings: ImIntegrationSettings): ImProviderSettings {
  return settings.providers.find((provider) => provider.providerId === "feishu") ?? defaultBrowserFeishuProvider;
}

/** 浏览器历史捕获上下文；正文只进入内存快照 Map，不进入前端日志。 */
export interface BrowserDocumentHistoryCapture {
  targetKind: DocumentHistoryTargetKind;
  knowledgeBaseId: string;
  targetId: string;
  relativePath: string;
  title: string;
  fileType: DocumentHistoryFileType;
  content: string;
  source: DocumentHistorySource;
  sessionId?: string;
  changeId?: string;
  operationId?: string;
}

/** 浏览器历史记录每个文件保留的最大版本数，和 Rust 常量保持一致。 */
export const BROWSER_DOCUMENT_HISTORY_MAX_ENTRIES = 100;

/** 浏览器历史记录最长保留天数，正式桌面端由 Rust 层执行同一策略。 */
export const BROWSER_DOCUMENT_HISTORY_RETENTION_DAYS = 90;

/** 统计历史快照行数；空正文显示 0 行，尾随换行保留为可见逻辑行。 */
export function countBrowserHistoryLines(content: string) {
  return content ? content.split("\n").length : 0;
}

/** 判断某条浏览器历史记录是否属于指定文件。 */
export function isBrowserHistoryTarget(entry: DocumentHistoryEntry, targetKind: DocumentHistoryTargetKind, targetId: string) {
  return entry.targetKind === targetKind && entry.targetId === targetId;
}

/** 捕获浏览器开发态历史版本；相同目标最新 hash 一致时不重复写入。 */
export function captureBrowserDocumentHistory(capture: BrowserDocumentHistoryCapture) {
  const contentHash = createContentHash(capture.content);
  const latestEntry = browserMock.documentHistoryEntries.find((entry) => isBrowserHistoryTarget(entry, capture.targetKind, capture.targetId));

  if (latestEntry?.contentHash === contentHash) {
    return;
  }

  const entry: DocumentHistoryEntry = {
    id: createLocalId("history"),
    targetKind: capture.targetKind,
    knowledgeBaseId: capture.knowledgeBaseId,
    targetId: capture.targetId,
    relativePath: capture.relativePath,
    title: capture.title,
    fileType: capture.fileType,
    contentHash,
    byteSize: new TextEncoder().encode(capture.content).length,
    lineCount: countBrowserHistoryLines(capture.content),
    source: capture.source,
    sessionId: capture.sessionId,
    changeId: capture.changeId,
    operationId: capture.operationId,
    createdAt: formatLocalDateTime(),
  };

  browserMock.documentHistoryEntries = [entry, ...browserMock.documentHistoryEntries];
  browserMock.documentHistoryContents.set(entry.id, capture.content);
  pruneBrowserDocumentHistory(capture.targetKind, capture.targetId);
}

/** 对浏览器历史记录执行数量和时间保留策略，并同步清理正文 Map。 */
export function pruneBrowserDocumentHistory(targetKind: DocumentHistoryTargetKind, targetId: string) {
  const cutoffTime = Date.now() - BROWSER_DOCUMENT_HISTORY_RETENTION_DAYS * 24 * 60 * 60 * 1000;
  let keptForTarget = 0;

  browserMock.documentHistoryEntries = browserMock.documentHistoryEntries.filter((entry) => {
    if (!isBrowserHistoryTarget(entry, targetKind, targetId)) {
      return true;
    }

    const createdTime = Date.parse(entry.createdAt.replace(/\//g, "-"));
    const isExpired = Number.isFinite(createdTime) && createdTime < cutoffTime;
    const shouldKeep = !isExpired && keptForTarget < BROWSER_DOCUMENT_HISTORY_MAX_ENTRIES;

    if (shouldKeep) {
      keptForTarget += 1;
    } else {
      browserMock.documentHistoryContents.delete(entry.id);
    }

    return shouldKeep;
  });
}

/** 重命名浏览器 mock 文件后迁移历史元数据，让旧版本继续挂在新文件 ID 下。 */
export function migrateBrowserDocumentHistoryTarget(
  targetKind: DocumentHistoryTargetKind,
  previousTargetId: string,
  nextTargetId: string,
  relativePath: string,
  title: string,
) {
  browserMock.documentHistoryEntries = browserMock.documentHistoryEntries.map((entry) =>
    isBrowserHistoryTarget(entry, targetKind, previousTargetId)
      ? {
          ...entry,
          targetId: nextTargetId,
          relativePath,
          title,
        }
      : entry,
  );
}

/** 清理浏览器 mock 当前文件历史，不影响其他文件或当前文档正文。 */
export function clearBrowserDocumentHistory(targetKind: DocumentHistoryTargetKind, targetId: string) {
  const removedIds = browserMock.documentHistoryEntries
    .filter((entry) => isBrowserHistoryTarget(entry, targetKind, targetId))
    .map((entry) => entry.id);

  removedIds.forEach((entryId) => browserMock.documentHistoryContents.delete(entryId));
  browserMock.documentHistoryEntries = browserMock.documentHistoryEntries.filter((entry) => !removedIds.includes(entry.id));
}

/** 从初始 mock 快照建立磁盘正文镜像；只在浏览器 loadWorkspaceState 时整体重置。 */
export function resetBrowserDiskContents(snapshot: WorkspaceSnapshot) {
  browserMock.noteDiskContents = new Map(snapshot.notes.map((note) => [note.id, note.content]));
  browserMock.documentDiskContents = new Map(
    snapshot.documents
      .filter((document) => document.fileType === "txt")
      .map((document) => [document.id, document.content ?? ""]),
  );
}

/** 从前端本地 ID 中提取创建毫秒时间戳，用于同一分钟内的新会话稳定倒序。 */
export function getTimestampMillisFromLocalId(id: string) {
  return id
    .split("-")
    .map((part) => Number(part))
    .find((timestampMillis) => timestampMillis >= 946_684_800_000 && timestampMillis <= 4_102_444_800_000);
}

/** 将浏览器 fallback 会话时间转成排序值，无法解析时排到列表末尾。 */
export function getSessionCreatedSortKey(session: AgentSession) {
  const parsedCreatedAt = Date.parse(session.createdAt.replace(/\//g, "-"));

  return (getTimestampMillisFromLocalId(session.id) ?? parsedCreatedAt) || 0;
}

/** 按创建时间倒序排列会话历史，保持浏览器开发态与 Tauri 持久化层一致。 */
export function sortSessionsByCreatedAtDesc(sessions: AgentSession[]) {
  sessions.sort((left, right) => {
    const timeDelta = getSessionCreatedSortKey(right) - getSessionCreatedSortKey(left);

    return timeDelta || right.createdAt.localeCompare(left.createdAt);
  });
}

/** 浏览器开发态内置 skills，与 Rust 内置定义保持同名同 ID，便于前后端切换验证。 */
export const browserBuiltInSkills: AgentSkill[] = [
  {
    id: "skill-note-research",
    name: "note-research",
    displayName: "知识库研究",
    description: "基于已选知识库发现支持文档、检索和阅读 Markdown 笔记，并给出带引用的回答。",
    instructions:
      "当用户要求查找、总结、对比或引用本地知识库时，先调用 list、search 或 read 获取依据。search 只覆盖 Markdown；read 可读取授权范围内的 Markdown/TXT，省略 fileId 时读取当前文件；TXT 不产生知识库引用。DOCX/PDF 也用 read 只读抽取。",
    tags: ["研究", "检索", "引用"],
    enabled: true,
    source: "built-in",
    createdAt: "内置",
    updatedAt: "内置",
  },

  {
    id: "skill-note-rewrite",
    name: "note-rewrite",
    displayName: "笔记改写",
    description: "改写当前笔记内容，并通过待确认 diff 交给用户决定是否写入。",
    instructions:
      "当用户要求润色、改写、压缩、扩写、多处编辑或文末追加 Markdown/TXT 时，先用 read 读取目标（可省略 fileId 以读当前文件）。只能调用 edit 生成待确认 diff；TXT 必须保持纯文本。局部改写用 replace，追加用 append，多处编辑用 multi_replace 和 edits。",
    tags: ["写作", "改写", "diff"],
    enabled: true,
    source: "built-in",
    createdAt: "内置",
    updatedAt: "内置",
  },

  {
    id: "skill-draft-from-context",
    name: "draft-from-context",
    displayName: "上下文草稿",
    description: "基于已选 scope 创建新的 Markdown 草稿，写入前仍需用户确认。",
    instructions:
      "当用户要求生成新笔记、TXT、清单、总结稿或草稿时，可以先检索或读取相关文件，再调用 write。目标路径必须在当前会话允许的知识库内，fileType 必须为 markdown 或 txt 且扩展名匹配；TXT 正文是纯文本。",
    tags: ["草稿", "生成", "Markdown"],
    enabled: true,
    source: "built-in",
    createdAt: "内置",
    updatedAt: "内置",
  },

  {
    id: "skill-organize-knowledge",
    name: "organize-knowledge",
    displayName: "知识整理",
    description: "给出标签、标题、目录、支持文档和关联笔记建议，不直接移动或改写文件。",
    instructions:
      "当用户要求整理知识库、补标签、规划目录或建立关联时，优先调用 list 获取目录、Markdown 笔记和已支持普通文档结构；需要正文依据时再调用 search 或 read 读取 Markdown 笔记，然后直接在回复中给出建议。该 skill 不执行文件移动或直接写入；若要落盘请调用 edit 或 write。",
    tags: ["整理", "标签", "目录"],
    enabled: true,
    source: "built-in",
    createdAt: "内置",
    updatedAt: "内置",
  },

];

/** 浏览器开发态模拟的自定义 skill，验证 UI 能展示 SKILL.md 来源和路径。 */
export const browserCustomSkills: AgentSkill[] = [
  {
    id: "skill-custom-browser-demo",
    name: "meeting-note-polish",
    displayName: "会议纪要润色",
    description: "来自 ~/.orange/skills 的示例 SKILL.md，用于模拟自定义 skill 扫描结果。",
    instructions:
      "读取当前会议纪要上下文，保持事实和行动项不变，输出更清晰的 Markdown 结构。涉及写入时必须生成待确认 diff。",
    tags: ["自定义", "会议", "写作"],
    enabled: true,
    source: "custom",
    createdAt: "自定义",
    updatedAt: "自定义",
    path: "~/.orange/skills/meeting-note-polish/SKILL.md",
    relativePath: "meeting-note-polish/SKILL.md",
    metadata: {
      frontmatterName: "meeting-note-polish",
    },
  },

];

/** 浏览器开发态没有 Tauri asset 协议，用轻量 SVG data URL 模拟图片预览。 */
export function createMockImagePreviewDataUrl(title: string) {
  const safeTitle = title.replace(/[<>&"]/g, "");
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="960" height="540" viewBox="0 0 960 540"><rect width="960" height="540" fill="#f7f3ea"/><rect x="96" y="80" width="768" height="380" rx="18" fill="#ffffff" stroke="#d7cfc2"/><circle cx="260" cy="210" r="58" fill="#6aa6a1"/><path d="M160 410 360 270l130 92 92-70 218 118z" fill="#314452"/><text x="480" y="500" text-anchor="middle" font-family="system-ui, sans-serif" font-size="28" fill="#314452">${safeTitle || "图片预览"}</text></svg>`;

  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}

/** 浏览器开发态的轻量上下文整理，只保存短摘要和计数，避免把完整消息正文塞进 contextSummary。 */
export function buildMockContextSummary(session: AgentSession): AgentContextSummary {
  const lastUserMessage = [...session.messages].reverse().find((message) => message.role === "user");
  const lastAssistantMessage = [...session.messages].reverse().find((message) => message.role === "assistant");
  const pendingChangeSummary =
    session.pendingChange?.status === "pending"
      ? [
          `type=${session.pendingChange.type}`,
          `operation=${session.pendingChange.operation ?? "create"}`,
          `title=${session.pendingChange.title}`,
          `path=${session.pendingChange.targetPath}`,
          `status=${session.pendingChange.status}`,
          `addedLines=${session.pendingChange.diffStats?.addedLines ?? 0}`,
          `removedLines=${session.pendingChange.diffStats?.removedLines ?? 0}`,
        ].join(" ")
      : undefined;

  return {
    version: 1,
    updatedAt: formatLocalDateTime(),
    currentGoal: lastUserMessage ? truncateSummaryText(lastUserMessage.content) : session.contextSummary?.currentGoal,
    userConstraints: session.contextSummary?.userConstraints ?? [],
    decisions: session.contextSummary?.decisions ?? [],
    completedWork: lastAssistantMessage ? [truncateSummaryText(`本轮回复：${lastAssistantMessage.content}`)] : [],
    pendingTasks: pendingChangeSummary ? ["等待用户确认当前 pending diff。"] : [],
    touchedNotes: session.contextSummary?.touchedNotes ?? [],
    pendingChangeSummary,
    openQuestions: session.contextSummary?.openQuestions ?? [],
    lastSummarizedMessageId: session.messages.at(-1)?.id,
    lastCompactedMessageId: session.messages.at(-1)?.id,
  };
}

/** 折叠空白并限制单条 mock summary 长度，和 Rust 侧的脱敏预算保持同类行为。 */
export function truncateSummaryText(value: string) {
  const collapsed = value.trim().split(/\s+/).join(" ");

  return collapsed.length > 360 ? `${collapsed.slice(0, 360)}…` : collapsed;
}

/** 浏览器开发态使用的文件名校验，保持与 Rust 层正式规则一致。 */
export function validateMarkdownFileNameForMock(fileName: string) {
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
export function validateNewMarkdownFileNameForMock(fileName: string) {
  const trimmedFileName = fileName.trim();

  if (!trimmedFileName) {
    throw new Error("文件名不能为空。");
  }

  const normalizedFileName = /\.[^./\\]+$/.test(trimmedFileName) ? trimmedFileName : `${trimmedFileName}.md`;

  return validateMarkdownFileNameForMock(normalizedFileName);
}

/** 浏览器开发态的 TXT 文件名校验，只允许当前目录下的 .txt 文件。 */
export function validateTextDocumentFileNameForMock(fileName: string) {
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
export function validateNewTextDocumentFileNameForMock(fileName: string) {
  const trimmedFileName = fileName.trim();

  if (!trimmedFileName) {
    throw new Error("文件名不能为空。");
  }

  const normalizedFileName = /\.[^./\\]+$/.test(trimmedFileName) ? trimmedFileName : `${trimmedFileName}.txt`;

  return validateTextDocumentFileNameForMock(normalizedFileName);
}

/** 浏览器开发态的新建目录名校验，只允许单级普通目录名。 */
export function validateFolderNameForMock(folderName: string) {
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
export function normalizeFolderPath(folderPath: string) {
  return folderPath.trim().replace(/^\/+|\/+$/g, "");
}

/** 拼接知识库内相对路径，根目录下只返回子名称。 */
export function joinRelativePath(parentPath: string, childName: string) {
  return parentPath ? `${parentPath}/${childName}` : childName;
}

/** 浏览器 fallback 确认父目录存在，避免新建时隐式创建多级目录。 */
export function ensureParentFolderExistsForMock(snapshot: WorkspaceSnapshot, knowledgeBaseId: string, parentPath: string) {
  if (!parentPath) {
    return;
  }

  const parentExists = snapshot.folders.some((folder) => folder.knowledgeBaseId === knowledgeBaseId && folder.path === parentPath);

  if (!parentExists) {
    throw new Error("目标父目录不存在，已阻止新建。");
  }
}

/** 在相对路径中替换最后一级文件名，模拟桌面端“只改文件名”的重命名语义。 */
export function replaceFileNameInPath(relativePath: string, nextFileName: string) {
  const pathParts = relativePath.split("/");

  pathParts[pathParts.length - 1] = nextFileName;

  return pathParts.join("/");
}

/** 预览或重命名时从正文一级标题提取展示标题，没有一级标题时使用文件名 stem。 */
export function getTitleFromMarkdownOrFileName(content: string, fileName: string) {
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
export function migrateNoteReferencesAfterRename(
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
export function migratePendingChangeAfterRename(
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
export function removeNoteReferencesAfterDelete(snapshot: WorkspaceSnapshot, noteId: string) {
  snapshot.sessions = snapshot.sessions.map((session) => ({
    ...session,
    activeNoteId: session.activeNoteId === noteId ? undefined : session.activeNoteId,
    pinnedNoteIds: session.pinnedNoteIds.filter((pinnedNoteId) => pinnedNoteId !== noteId),
    pendingChange: session.pendingChange?.noteId === noteId ? undefined : session.pendingChange,
  }));
}

/** 深拷贝 skill 列表，避免浏览器 mock 状态被 React 组件直接修改。 */
export function cloneAgentSkills(skills: AgentSkill[]): AgentSkill[] {
  return skills.map((skill) => ({
    ...skill,
    tags: [...skill.tags],
    metadata: skill.metadata ? { ...skill.metadata } : undefined,
  }));
}

/** 深拷贝用户设置，保证浏览器开发态保存和读取行为接近桌面端持久化。 */
export function cloneUserSettings(settings: UserSettings): UserSettings {
  return {
    ...settings,
    agentSecurity: {
      ...settings.agentSecurity,
      resourceLimits: { ...settings.agentSecurity.resourceLimits },
      trustedSkillGrants: settings.agentSecurity.trustedSkillGrants.map((grant) => ({ ...grant })),
      allowedNetworkDomains: [...settings.agentSecurity.allowedNetworkDomains],
    },
    modelConfig: {
      ...settings.modelConfig,
      providers: settings.modelConfig.providers.map((provider) => ({
        ...provider,
        models: provider.models.map((model) => ({ ...model })),
      })),
    },
  };
}

/** 浏览器开发态按 provider 模板生成少量模型，用于验证设置页模型选择交互。 */
export function createBrowserDiscoveredModels(provider: LlmProviderConfig): LlmProviderModel[] {
  const lowerName = `${provider.name} ${provider.apiBase}`.toLowerCase();
  const modelIds = lowerName.includes("deepseek")
    ? ["deepseek-chat", "deepseek-reasoner"]
    : lowerName.includes("openrouter")
      ? ["openai/gpt-4o-mini", "anthropic/claude-sonnet-4", "google/gemini-2.5-flash"]
      : lowerName.includes("ollama") || provider.apiBase.includes("11434")
        ? ["llama3.1", "qwen2.5", "mistral"]
        : ["gpt-4o-mini", "gpt-4.1-mini", "gpt-4.1"];

  return modelIds.map((modelId) => ({
    id: modelId,
    name: modelId,
    ownedBy: lowerName.includes("ollama") ? "ollama" : provider.name,
    enabled: false,
    source: "discovered",
    contextLength: mockModelContextLength(modelId),
    updatedAt: formatLocalDateTime(),
  }));
}

/** 浏览器开发态给常见模型补一个窗口，方便上下文占用条展示。 */
function mockModelContextLength(modelId: string) {
  if (modelId.includes("gpt-4.1") || modelId.includes("gemini")) {
    return 1_048_576;
  }

  if (modelId.includes("claude")) {
    return 200_000;
  }

  if (modelId.includes("deepseek")) {
    return 65_536;
  }

  if (modelId.includes("llama") || modelId.includes("qwen") || modelId.includes("mistral")) {
    return 32_768;
  }

  return 128_000;
}

/** 浏览器开发态记住最近一次模拟发给模型的上下文，供上下文浮层查看。 */
export function rememberBrowserPromptDump(session: AgentSession, prompt: string, action: AgentActionType) {
  const systemContent = "你是橘记的本地优先知识库 Agent。浏览器 mock 不会发送真实模型请求。";
  const userContent = `界面 action 提示：${action}\n用户输入：${prompt}`;
  const messages = [
    { role: "system", content: systemContent },
    { role: "user", content: userContent },
  ];
  const dump: AgentPromptDump = {
    sessionId: session.id,
    modelId: session.contextUsage?.modelId || session.modelId || "gpt-4o-mini",
    modelContextLength: session.contextUsage?.contextLength ?? 128000,
    recordedAt: formatLocalDateTime(),
    round: 1,
    kind: "turn",
    totalChars: messages.reduce((sum, message) => sum + JSON.stringify(message).length, 0),
    filePath: "~/Library/Logs/app.orange.desktop/agent-prompts/session.json",
    outline: messages.map((message) => `${message.role}:${message.content.length}`).join(","),
    messages: messages.map((message, index) => ({
      index,
      role: message.role,
      chars: JSON.stringify(message).length,
      preview: message.content,
      truncated: false,
    })),
  };

  browserMock.promptDumps.set(session.id, dump);
}

/** 浏览器开发态模型合并逻辑，对齐桌面端：保留启用状态，并留下远端没返回的手动模型。 */
export function mergeBrowserProviderModels(
  provider: LlmProviderConfig,
  discoveredModels: LlmProviderModel[],
  fetchedAt: string,
): LlmProviderModel[] {
  const existingById = new Map(provider.models.map((model) => [model.id, model]));
  const seenModelIds = new Set<string>();
  const mergedModels: LlmProviderModel[] = [];

  for (const discoveredModel of discoveredModels) {
    const modelId = discoveredModel.id.trim();

    if (!modelId || seenModelIds.has(modelId)) {
      continue;
    }

    seenModelIds.add(modelId);
    mergedModels.push({
      ...discoveredModel,
      id: modelId,
      name: discoveredModel.name.trim() || modelId,
      enabled: existingById.get(modelId)?.enabled ?? false,
      source: "discovered",
      updatedAt: fetchedAt,
    });
  }

  for (const existingModel of provider.models) {
    if (existingModel.source !== "manual" || seenModelIds.has(existingModel.id)) {
      continue;
    }

    seenModelIds.add(existingModel.id);
    mergedModels.push(existingModel);
  }

  const defaultModelId = provider.model.trim();

  if (defaultModelId && !mergedModels.some((model) => model.id === defaultModelId)) {
    mergedModels.push({
      id: defaultModelId,
      name: defaultModelId,
      enabled: true,
      source: "manual",
      updatedAt: provider.updatedAt,
    });
  }

  if (defaultModelId) {
    return mergedModels.map((model) => (model.id === defaultModelId ? { ...model, enabled: true } : model));
  }

  if (!mergedModels.some((model) => model.enabled) && mergedModels[0]) {
    mergedModels[0] = { ...mergedModels[0], enabled: true };
  }

  return mergedModels;
}

/** 深拷贝即时通讯设置，保证浏览器开发态保存和读取行为接近桌面端持久化。 */
export function cloneImSettings(settings: ImIntegrationSettings): ImIntegrationSettings {
  return {
    providers: settings.providers.map((provider) => ({
      ...provider,
      defaultKnowledgeBaseIds: [...provider.defaultKnowledgeBaseIds],
      allowedUserOpenIds: [...provider.allowedUserOpenIds],
      allowedChatIds: [...provider.allowedChatIds],
      discoveredUserOpenIds: [...provider.discoveredUserOpenIds],
      discoveredChatIds: [...provider.discoveredChatIds],
      config: { ...provider.config },
    })),
  };
}

/** 归一化浏览器开发态用户 skill，并模拟桌面端写入 SKILL.md 后返回 custom 来源。 */
export function normalizeBrowserCustomSkill(skill: AgentSkill): AgentSkill {
  const now = formatLocalDateTime();
  const normalizedName = normalizeBrowserSkillName(skill.name || skill.displayName || skill.id);
  const relativePath = `${normalizedName}/SKILL.md`;
  const nextId = `skill-custom-browser-${normalizedName || createLocalId("skill")}`;
  const normalizedSkill: AgentSkill = {
    ...skill,
    id: nextId,
    name: normalizedName,
    displayName: skill.displayName.trim(),
    description: skill.description.trim(),
    instructions: skill.instructions.trim(),
    tags: normalizeBrowserTerms(skill.tags),
    enabled: skill.enabled,
    source: "custom",
    createdAt: skill.createdAt.trim() || now,
    updatedAt: now,
    path: `~/.orange/skills/${relativePath}`,
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
export function installBrowserMockSkill(payload: InstallAgentSkillPayload): InstallAgentSkillResult {
  const now = formatLocalDateTime();
  const sourceSummary = summarizeBrowserSkillInstallSource(payload);
  const skillName = buildBrowserInstalledSkillName(payload);
  const normalizedSkill = normalizeBrowserCustomSkill({
    id: "",
    name: skillName,
    displayName: `安装 Skill ${skillName}`,
    description: "浏览器开发态模拟安装的第三方 SKILL.md，桌面端会读取真实来源并验证 frontmatter。",
    instructions:
      "这是浏览器开发态的安装模拟能力。真实桌面端会在安装后默认停用第三方 skill，用户审阅并启用后才会进入 Runtime。",
    tags: ["安装", "模拟"],
    enabled: payload.enableAfterInstall,
    source: "custom",
    createdAt: now,
    updatedAt: now,
    metadata: {
      installSourceType: payload.sourceType,
      installSourceSummary: sourceSummary,
    },
  });
  const existingSkill = browserMock.agentSkills.find((skill) => skill.id === normalizedSkill.id || skill.name === normalizedSkill.name);

  if (existingSkill && payload.conflictStrategy === "fail") {
    throw new Error("目标 Skill 目录已存在，请勾选替换同名 Skill 后重试。");
  }

  browserMock.agentSkills = [
    ...browserMock.agentSkills.filter((skill) => skill.id !== normalizedSkill.id && skill.name !== normalizedSkill.name),
    normalizedSkill,
  ];

  const installedSkills = cloneAgentSkills([normalizedSkill]);

  return {
    installedSkills,
    skills: cloneAgentSkills(browserMock.agentSkills),
    warnings: [],
    summary: "已安装 1 个 Skill，复制 1 个文件。",
    sourceType: payload.sourceType,
    sourceSummary,
    installedCount: 1,
    fileCount: 1,
  };
}

/** 浏览器开发态的在线目录样本，覆盖写作、PDF 和不可一键安装来源。 */
const BROWSER_ONLINE_SKILLS: OnlineSkill[] = [
  {
    id: "vercel-labs/agent-skills/writing-guidelines",
    skillId: "writing-guidelines",
    name: "writing-guidelines",
    source: "vercel-labs/agent-skills",
    installs: 54516,
    pageUrl: "https://skills.sh/vercel-labs/agent-skills/writing-guidelines",
    installable: true,
    description: "写作风格与结构指南，适合润色笔记和长文。",
  },
  {
    id: "anthropics/skills/pdf",
    skillId: "pdf",
    name: "pdf",
    source: "anthropics/skills",
    installs: 186704,
    pageUrl: "https://skills.sh/anthropics/skills/pdf",
    installable: true,
    description: "处理 PDF 阅读、抽取表格和合并文档。",
  },
  {
    id: "claude-office-skills/skills/meeting-notes",
    skillId: "meeting-notes",
    name: "meeting-notes",
    source: "claude-office-skills/skills",
    installs: 4712,
    pageUrl: "https://skills.sh/claude-office-skills/skills/meeting-notes",
    installable: true,
    description: "把会议内容整理成可检索的纪要。",
  },
  {
    id: "open.feishu.cn/lark-note",
    skillId: "lark-note",
    name: "lark-note",
    source: "open.feishu.cn",
    installs: 414150,
    pageUrl: "https://skills.sh/open.feishu.cn/lark-note",
    installable: false,
    description: "飞书云文档来源，当前不支持一键安装。",
  },
];

/** 浏览器开发态模拟在线搜索，按名称、来源和简介过滤。 */
export function searchBrowserOnlineSkills(payload: SearchOnlineSkillsPayload): OnlineSkillSearchResult {
  const query = payload.query.trim().toLowerCase();
  const owner = payload.owner?.trim().toLowerCase();

  if (query.length < 2) {
    throw new Error("请输入至少两个字搜索在线 Skills。");
  }

  const skills = BROWSER_ONLINE_SKILLS.filter((skill) => {
    const searchableText = `${skill.name} ${skill.skillId} ${skill.source} ${skill.description ?? ""}`.toLowerCase();
    const matchesQuery = searchableText.includes(query) || query === "skill";
    const matchesOwner = !owner || skill.source.toLowerCase().startsWith(`${owner}/`) || skill.source.toLowerCase() === owner;

    return matchesQuery && matchesOwner;
  }).sort((left, right) => right.installs - left.installs);

  return { query: payload.query.trim(), skills };
}

/** 浏览器开发态返回样本简介，未知 id 仍给出可打开的 skills.sh 地址。 */
export function previewBrowserOnlineSkill(skillId: string): OnlineSkillPreview {
  const skill = BROWSER_ONLINE_SKILLS.find((item) => item.id === skillId);

  return {
    id: skillId,
    pageUrl: skill?.pageUrl ?? `https://skills.sh/${skillId}`,
    description: skill?.description,
  };
}

/** 根据安装来源生成稳定 mock 名称，避免浏览器开发态反复安装产生不可追踪 ID。 */
export function buildBrowserInstalledSkillName(payload: InstallAgentSkillPayload) {
  const requestedName = payload.skillNames?.find((name) => name.trim());

  if (requestedName) {
    return normalizeBrowserSkillName(requestedName) || `installed-${payload.sourceType.toLowerCase()}`;
  }

  const source = payload.source?.trim();
  const sourceTail = source ? source.split(/[\\/]/).filter(Boolean).at(-1) ?? source : payload.sourceType;
  const withoutExtension = sourceTail.replace(/\.(zip|md|markdown)$/i, "");
  const normalizedName = normalizeBrowserSkillName(withoutExtension || payload.sourceType);

  return normalizedName || `installed-${payload.sourceType.toLowerCase()}`;
}

/** 生成安装来源脱敏摘要，只保留 host、文件名或选择器类型。 */
export function summarizeBrowserSkillInstallSource(payload: InstallAgentSkillPayload) {
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
export function normalizeBrowserSkillName(name: string) {
  return name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, "-")
    .split("-")
    .filter(Boolean)
    .join("-");
}

/** 清理标签，去重并限制数量，避免 mock prompt 摘要失控。 */
export function normalizeBrowserTerms(terms: string[]) {
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

/** 浏览器 mock 记录本轮显式 skill 数量，不记录用户输入或 skill instructions。 */
export function logBrowserSkillContext(skills: AgentSkill[], request: AgentTurnRequest): void {
  logDebug("浏览器 mock 解析本轮 Skill 上下文。", {
    category: "frontend",
    event: "resolve_skill",
    status: request.explicitSkillIds?.length ? "explicit" : "catalog_only",
    metadata: {
      action: request.action,
      enabledSkillCount: skills.filter((skill) => skill.enabled).length,
      explicitSkillCount: request.explicitSkillIds?.length ?? 0,
    },
  });
}

/** 浏览器 fallback 清理会话中的失效知识库和笔记引用，模拟后端持久化入口的归一化。 */
export function normalizeMockSnapshotSessions(snapshot: WorkspaceSnapshot): WorkspaceSnapshot {
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
export function getSessionNoteId(snapshot: WorkspaceSnapshot, activeNoteId: string | undefined, knowledgeBaseId: string) {
  if (
    activeNoteId &&
    snapshot.notes.some((note) => note.id === activeNoteId && note.knowledgeBaseId === knowledgeBaseId)
  ) {
    return activeNoteId;
  }

  return "";
}

/** 无可激活 Markdown 时，用同知识库第一个普通文档填充中间面板。 */
export function getFallbackDocumentId(snapshot: WorkspaceSnapshot, knowledgeBaseId: string, activeNoteId: string) {
  if (activeNoteId) {
    return "";
  }

  return snapshot.documents.find((document) => document.knowledgeBaseId === knowledgeBaseId)?.id ?? "";
}

/** 按知识库列表顺序整理范围 ID，避免 UI 多选顺序随点击行为抖动。 */
export function orderValidKnowledgeBaseIds(selectedIds: string[], knowledgeBases: KnowledgeBase[]) {
  const selectedIdSet = new Set(selectedIds);

  return knowledgeBases.filter((knowledgeBase) => selectedIdSet.has(knowledgeBase.id)).map((knowledgeBase) => knowledgeBase.id);
}

/** 为浏览器 fallback 创建请求审计摘要，便于设置页预览 M3 审计信息。 */
export function createBrowserAuditLog(snapshot: WorkspaceSnapshot, prompt: string): RequestAuditLog {
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

/** 浏览器开发态可变状态；桌面端走 Tauri 命令，不读写这里。 */
export const browserMock: {
  userSettings: UserSettings;
  imSettings: ImIntegrationSettings;
  feishuGatewayStatus: ImGatewayStatus;
  auditLogs: RequestAuditLog[];
  appEventLogs: AppEventLog[];
  documentHistoryEntries: DocumentHistoryEntry[];
  documentHistoryContents: Map<string, string>;
  noteDiskContents: Map<string, string>;
  documentDiskContents: Map<string, string>;
  agentSkills: AgentSkill[];
  promptDumps: Map<string, AgentPromptDump>;
} = {
  userSettings: defaultBrowserUserSettings,
  imSettings: defaultBrowserImSettings,
  feishuGatewayStatus: {
    providerId: "feishu",
    running: false,
    connected: false,
    domain: "feishu",
    appIdConfigured: false,
    secretConfigured: false,
    lastError: "浏览器开发态未连接桌面长连接网关。",
  },
  auditLogs: [],
  appEventLogs: [],
  documentHistoryEntries: [],
  documentHistoryContents: new Map<string, string>(),
  noteDiskContents: new Map<string, string>(),
  documentDiskContents: new Map<string, string>(),
  agentSkills: cloneAgentSkills([...browserBuiltInSkills, ...browserCustomSkills]),
  promptDumps: new Map<string, AgentPromptDump>(),
};
