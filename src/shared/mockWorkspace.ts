import { createContentHash, createLocalId } from "./id";
import { buildMarkdownDiff } from "../diff/markdownDiff";
import type {
  AgentActionType,
  AgentMessage,
  AgentSession,
  AgentToolCall,
  Citation,
  FolderEntry,
  KnowledgeBase,
  KnowledgeBaseSelection,
  Note,
  ProposedChange,
  WorkspaceDocument,
  WorkspaceSnapshot,
} from "./types";

/** 浏览器开发态内置知识库，模拟用户连接多个本地支持文档目录。 */
const initialKnowledgeBases: KnowledgeBase[] = [
  {
    id: "kb-personal",
    name: "个人知识库",
    path: "/Users/me/Documents/Knowledge",
    description: "默认知识库，沉淀产品想法、会议纪要和个人研究。",
    status: "ready",
    noteCount: 4,
    documentCount: 4,
    updatedAt: "今天 14:18",
    isDefault: true,
    semanticIndexEnabled: false,
    scanReport: {
      scannedFileCount: 7,
      scannedByType: {
        markdown: 4,
        txt: 1,
        docx: 1,
        pdf: 1,
        image: 1,
      },
      failedFileCount: 0,
      skippedDirectories: [".git", "node_modules"],
      errors: [],
    },
  },
  {
    id: "kb-work",
    name: "工作项目库",
    path: "/Users/me/Work/Project-A/docs",
    description: "项目文档、上线计划和技术决策记录。",
    status: "ready",
    noteCount: 2,
    documentCount: 1,
    updatedAt: "今天 11:32",
    isDefault: false,
    semanticIndexEnabled: false,
    scanReport: {
      scannedFileCount: 3,
      scannedByType: {
        markdown: 2,
        txt: 1,
        docx: 0,
        pdf: 0,
        image: 0,
      },
      failedFileCount: 0,
      skippedDirectories: ["dist"],
      errors: [],
    },
  },
  {
    id: "kb-reading",
    name: "阅读资料库",
    path: "/Users/me/Library/Reading",
    description: "书摘、论文笔记和长期主题研究材料。",
    status: "ready",
    noteCount: 2,
    documentCount: 1,
    updatedAt: "昨天 22:10",
    isDefault: false,
    semanticIndexEnabled: true,
    scanReport: {
      scannedFileCount: 3,
      scannedByType: {
        markdown: 2,
        txt: 0,
        docx: 0,
        pdf: 1,
        image: 0,
      },
      failedFileCount: 0,
      skippedDirectories: [],
      errors: [],
    },
  },
];

/** 浏览器开发态内置笔记，覆盖问答、检索、隐私和写入确认场景。 */
const initialNotes: Note[] = [
  {
    id: "note-product-brief",
    knowledgeBaseId: "kb-personal",
    title: "Agent 笔记应用立项",
    path: "00-Inbox/Agent 笔记应用立项.md",
    updatedAt: "今天 14:16",
    tags: ["产品", "MVP", "Agent"],
    backlinks: ["note-research-loop", "note-privacy"],
    content: `# Agent 笔记应用立项

## 产品定位
面向个人知识工作者的本地优先 Agent 笔记应用。用户选择本地 Markdown 目录作为知识库，在熟悉的笔记编辑界面中，让助手完成查找、问答、改写、生成新笔记和整理知识等操作。

## MVP 范围
- 选择本地知识库目录并扫描 Markdown。
- 在编辑器中创建、编辑和保存笔记草稿。
- 在 Agent 侧栏中基于知识库问答，并展示引用来源。
- Agent 修改笔记前必须展示 diff，由用户确认后写入。`,
    contentHash: "",
  },
  {
    id: "note-research-loop",
    knowledgeBaseId: "kb-personal",
    title: "个人知识循环",
    path: "01-Research/个人知识循环.md",
    updatedAt: "昨天 21:40",
    tags: ["研究", "知识管理"],
    backlinks: ["note-product-brief"],
    content: `# 个人知识循环

## 捕获
把会议、阅读、灵感和问题快速放入 Inbox，避免一开始就分类。

## 整理
每周把 Inbox 中的材料改写成可复用的永久笔记，并补充来源、标签和相关链接。

## 复用
写作或决策时让 Agent 先从知识库中检索上下文，再给出带引用的总结。`,
    contentHash: "",
  },
  {
    id: "note-privacy",
    knowledgeBaseId: "kb-personal",
    title: "本地优先与隐私边界",
    path: "02-Architecture/本地优先与隐私边界.md",
    updatedAt: "周二 09:22",
    tags: ["隐私", "架构", "Tauri"],
    backlinks: ["note-product-brief"],
    content: `# 本地优先与隐私边界

## 文件所有权
Markdown 文件保存在用户选择的目录中，应用只负责读取、索引和在确认后写入。

## 云端模型
默认使用云端模型以获得更好的问答与改写质量。发送前需要明确提示用户：哪些片段会被提交给模型，为什么需要提交。

## 写入权限
Agent 不能静默修改本地文件。所有写入动作都必须先生成变更预览，并允许用户接受或取消。`,
    contentHash: "",
  },
  {
    id: "note-meeting",
    knowledgeBaseId: "kb-personal",
    title: "原型评审会议纪要",
    path: "03-Meetings/原型评审会议纪要.md",
    updatedAt: "周一 17:05",
    tags: ["会议", "原型"],
    backlinks: [],
    content: `# 原型评审会议纪要

## 结论
首版需要直接展示可工作的桌面工具界面，不做营销首页。

## 待确认
- 是否需要支持多知识库切换。
- 是否在侧栏中提供 Agent 操作历史。
- 是否展示 Markdown 预览模式。`,
    contentHash: "",
  },
  {
    id: "note-release-plan",
    knowledgeBaseId: "kb-work",
    title: "Project-A 上线计划",
    path: "Release/Project-A 上线计划.md",
    updatedAt: "今天 11:20",
    tags: ["项目", "上线", "检查清单"],
    backlinks: ["note-tech-decision"],
    content: `# Project-A 上线计划

## 发布目标
在不影响现有用户工作流的前提下，完成知识库导入、检索索引和基础写入确认流程。

## 风险
- 本地文件权限需要给出明确提示。
- 云端模型请求必须显示发送范围。
- 跨知识库检索需要由用户显式开启。`,
    contentHash: "",
  },
  {
    id: "note-tech-decision",
    knowledgeBaseId: "kb-work",
    title: "桌面端技术选型",
    path: "Architecture/桌面端技术选型.md",
    updatedAt: "昨天 18:45",
    tags: ["Tauri", "架构", "本地文件"],
    backlinks: ["note-release-plan"],
    content: `# 桌面端技术选型

## 建议
正式产品采用 Tauri + React + TypeScript。桌面层负责本地目录授权、文件系统读写和安全确认。

## 约束
Web 原型只模拟目录选择和写入行为，不直接访问真实文件系统。`,
    contentHash: "",
  },
  {
    id: "note-reading-llm",
    knowledgeBaseId: "kb-reading",
    title: "LLM 知识管理摘录",
    path: "Books/LLM 知识管理摘录.md",
    updatedAt: "昨天 22:04",
    tags: ["阅读", "LLM", "知识管理"],
    backlinks: ["note-reading-search"],
    content: `# LLM 知识管理摘录

## 摘录
好的知识工具应该帮助用户把散乱材料转化为可复用结构，而不是只保存原始片段。

## 启发
Agent 需要给出来源和不确定性，避免让用户无法判断回答依据。`,
    contentHash: "",
  },
  {
    id: "note-reading-search",
    knowledgeBaseId: "kb-reading",
    title: "语义检索笔记",
    path: "Papers/语义检索笔记.md",
    updatedAt: "周日 15:28",
    tags: ["检索", "RAG", "引用"],
    backlinks: ["note-reading-llm"],
    content: `# 语义检索笔记

## 检索边界
默认检索范围应该尽可能小，跨集合检索需要让用户明确知道范围扩大。

## 引用
回答必须保留来源笔记、路径和命中片段，方便用户回到原文验证。`,
    contentHash: "",
  },
];

/** 浏览器开发态内置普通文档，覆盖 TXT 编辑和 DOCX/PDF/图片只读预览场景。 */
const initialDocuments: WorkspaceDocument[] = [
  {
    id: "document-product-plain",
    knowledgeBaseId: "kb-personal",
    title: "快速想法",
    path: "00-Inbox/快速想法.txt",
    fileType: "txt",
    updatedAt: "今天 13:20",
    content: "先把零散想法放到 TXT，后续再整理成 Markdown 笔记。",
    contentHash: "",
    previewAvailable: false,
  },
  {
    id: "document-product-brief-docx",
    knowledgeBaseId: "kb-personal",
    title: "访谈摘要",
    path: "01-Research/访谈摘要.docx",
    fileType: "docx",
    updatedAt: "昨天 19:10",
    contentHash: "mock-docx-brief",
    previewAvailable: true,
  },
  {
    id: "document-product-spec-pdf",
    knowledgeBaseId: "kb-personal",
    title: "桌面端规格",
    path: "02-Architecture/桌面端规格.pdf",
    fileType: "pdf",
    updatedAt: "周二 16:00",
    contentHash: "mock-pdf-spec",
    previewAvailable: true,
  },
  {
    id: "document-product-image",
    knowledgeBaseId: "kb-personal",
    title: "界面截图",
    path: "02-Architecture/界面截图.svg",
    fileType: "image",
    updatedAt: "今天 09:30",
    contentHash: "mock-image-preview",
    previewAvailable: true,
  },
  {
    id: "document-work-notes",
    knowledgeBaseId: "kb-work",
    title: "发布临时记录",
    path: "Release/发布临时记录.txt",
    fileType: "txt",
    updatedAt: "今天 10:45",
    content: "发布前临时记录：确认回滚预案、客服公告和监控面板。",
    contentHash: "",
    previewAvailable: false,
  },
  {
    id: "document-reading-paper",
    knowledgeBaseId: "kb-reading",
    title: "RAG 论文原文",
    path: "Papers/RAG 论文原文.pdf",
    fileType: "pdf",
    updatedAt: "周日 15:00",
    contentHash: "mock-reading-pdf",
    previewAvailable: true,
  },
];

/** 浏览器开发态内置真实目录，覆盖普通目录和没有 Markdown 文件的空目录。 */
const initialFolders: FolderEntry[] = [
  {
    id: "folder-personal-inbox",
    knowledgeBaseId: "kb-personal",
    name: "00-Inbox",
    path: "00-Inbox",
    updatedAt: "今天 14:18",
  },
  {
    id: "folder-personal-research",
    knowledgeBaseId: "kb-personal",
    name: "01-Research",
    path: "01-Research",
    updatedAt: "昨天 21:40",
  },
  {
    id: "folder-personal-architecture",
    knowledgeBaseId: "kb-personal",
    name: "02-Architecture",
    path: "02-Architecture",
    updatedAt: "周二 09:22",
  },
  {
    id: "folder-personal-meetings",
    knowledgeBaseId: "kb-personal",
    name: "03-Meetings",
    path: "03-Meetings",
    updatedAt: "周一 17:05",
  },
  {
    id: "folder-personal-empty",
    knowledgeBaseId: "kb-personal",
    name: "04-Archive",
    path: "04-Archive",
    updatedAt: "今天 10:00",
  },
  {
    id: "folder-work-release",
    knowledgeBaseId: "kb-work",
    name: "Release",
    path: "Release",
    updatedAt: "今天 11:20",
  },
  {
    id: "folder-work-architecture",
    knowledgeBaseId: "kb-work",
    name: "Architecture",
    path: "Architecture",
    updatedAt: "昨天 18:45",
  },
  {
    id: "folder-reading-books",
    knowledgeBaseId: "kb-reading",
    name: "Books",
    path: "Books",
    updatedAt: "昨天 22:04",
  },
  {
    id: "folder-reading-papers",
    knowledgeBaseId: "kb-reading",
    name: "Papers",
    path: "Papers",
    updatedAt: "周日 15:28",
  },
];

/** 为内置笔记补齐内容 hash，确保 mock 写入也走冲突校验路径。 */
function hydrateNoteHashes(notes: Note[]) {
  return notes.map((note) => ({ ...note, contentHash: createContentHash(note.content) }));
}

/** 为内置 TXT 文档补齐内容 hash；二进制预览文档保留模拟 hash。 */
function hydrateDocumentHashes(documents: WorkspaceDocument[]) {
  return documents.map((document) =>
    document.fileType === "txt" ? { ...document, contentHash: createContentHash(document.content ?? "") } : document,
  );
}

/** 创建初始 Agent 会话，开场消息强调工具化检索和写入确认边界。 */
function createInitialSessions(): AgentSession[] {
  return [
    {
      id: "session-product-brief",
      title: "新会话",
      type: "knowledge-base",
      knowledgeBaseIds: ["kb-personal"],
      pinnedNoteIds: [],
      createdAt: "今天 14:18",
      updatedAt: "今天 14:18",
      messages: [],
    },
  ];
}

/** 返回浏览器开发态的完整工作台快照。 */
export function createMockWorkspaceSnapshot(): WorkspaceSnapshot {
  return {
    knowledgeBases: initialKnowledgeBases,
    folders: initialFolders,
    notes: hydrateNoteHashes(initialNotes),
    documents: hydrateDocumentHashes(initialDocuments),
    sessions: createInitialSessions(),
    activeKnowledgeBaseId: "kb-personal",
    activeNoteId: "note-product-brief",
    activeDocumentId: "",
    activeSessionId: "session-product-brief",
  };
}

/** 深拷贝工作台快照，避免 mock runtime 修改 React state 中的旧引用。 */
export function cloneWorkspaceSnapshot(snapshot: WorkspaceSnapshot): WorkspaceSnapshot {
  return JSON.parse(JSON.stringify(snapshot)) as WorkspaceSnapshot;
}

/** 创建浏览器开发态知识库选择结果，模拟 Tauri 目录选择器返回值。 */
export function createMockKnowledgeBaseSelection(count: number): KnowledgeBaseSelection {
  return {
    id: createLocalId("kb-added"),
    name: `客户资料库 ${count + 1}`,
    path: `/Users/me/Clients/Client-${count + 1}`,
    noteCount: 1,
  };
}

/** 浏览器 mock 没有真实模型，只响应显式工具入口，避免假装 Agent 已完成自主判断。 */
function shouldUseSearchTool(action: AgentActionType) {
  return action === "find";
}

/** 根据当前会话范围执行 mock 检索工具，返回可追溯引用。 */
function searchNotes(snapshot: WorkspaceSnapshot, session: AgentSession, prompt: string): Citation[] {
  const selectedKnowledgeBaseIds = new Set(session.knowledgeBaseIds);
  const promptTerms = prompt
    .toLowerCase()
    .split(/\s+/)
    .map((term) => term.trim())
    .filter(Boolean);

  return snapshot.notes
    .filter((note) => selectedKnowledgeBaseIds.has(note.knowledgeBaseId))
    .map((note) => {
      const knowledgeBase = snapshot.knowledgeBases.find((item) => item.id === note.knowledgeBaseId);
      const searchableText = `${note.title} ${note.path} ${note.tags.join(" ")} ${note.content}`.toLowerCase();
      const score = promptTerms.reduce((currentScore, term) => currentScore + (searchableText.includes(term) ? 2 : 0), 0);
      const keywordScore = ["写入", "隐私", "检索", "Agent", "本地"].reduce(
        (currentScore, term) => currentScore + (searchableText.includes(term.toLowerCase()) ? 1 : 0),
        score,
      );

      return {
        knowledgeBaseId: note.knowledgeBaseId,
        knowledgeBaseName: knowledgeBase?.name ?? "未知知识库",
        noteId: note.id,
        title: note.title,
        path: note.path,
        snippet: extractSnippet(note.content, prompt),
        score: keywordScore,
      };
    })
    .filter((citation) => citation.score > 0)
    .sort((left, right) => right.score - left.score)
    .slice(0, 4);
}

/** 从 Markdown 内容中提取可展示片段，作为检索工具结果摘要。 */
function extractSnippet(content: string, prompt: string) {
  const lines = content
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line && !line.startsWith("#"));
  const promptTokens = prompt.split(/\s+/).filter(Boolean);
  const matchedLine = lines.find((line) => promptTokens.some((token) => line.includes(token)));

  return matchedLine ?? lines[0] ?? "命中该笔记，但暂无可展示片段。";
}

/** 从 Markdown 内容中提取首个正文段落，用于 Agent 改写建议。 */
function getFirstBodyParagraph(content: string) {
  const lines = content.split("\n");

  // 跳过标题、空行和列表项，优先选择可以独立改写的正文段落。
  return (
    lines.find((line) => {
      const trimmedLine = line.trim();

      return trimmedLine.length > 18 && !trimmedLine.startsWith("#") && !trimmedLine.startsWith("-");
    }) ?? ""
  );
}

/** 生成改写建议正文，mock 环境中用确定性文案模拟云端模型返回。 */
function buildRewriteText(original: string) {
  return `这款产品面向长期处理资料、灵感和项目文档的个人知识工作者。它以本地 Markdown 目录作为可信数据源，在保留用户文件所有权的前提下，让 Agent 负责检索、总结、改写和生成笔记；任何写入都会先展示变更预览，并在用户确认后才落盘。\n\n原段落要点：${original}`;
}

/** 判断用户是否要求追加到文末，浏览器 mock 用它选择 append 语义。 */
function shouldAppendToNote(prompt: string) {
  return /追加|文末|末尾|最后|插入到.*(文末|末尾|最后)|append/i.test(prompt);
}

/** 判断用户是否要求同一文件多处编辑，浏览器 mock 用确定性策略模拟后端 multi_replace。 */
function shouldMultiEditNote(prompt: string) {
  return /多处|批量|全部|重复|去重|multi/i.test(prompt);
}

/** 生成浏览器 mock 的追加内容，避免把整篇原文误塞进 next。 */
function buildAppendText(prompt: string) {
  return `## 补充内容\n\n${prompt.trim() || "这里是 Agent 追加的补充内容。"}`;
}

/** 创建工具调用记录，帮助用户看清 Agent loop 做过哪些动作。 */
function createToolCall(name: AgentToolCall["name"], summary: string, args: Record<string, unknown>): AgentToolCall {
  return {
    id: createLocalId("tool"),
    name,
    status: "completed",
    summary,
    args,
  };
}

/** 根据会话绑定范围生成可读标签，用于 Agent 回复文案。 */
function getScopeLabel(snapshot: WorkspaceSnapshot, session: AgentSession) {
  const selectedNames = session.knowledgeBaseIds
    .map((knowledgeBaseId) => snapshot.knowledgeBases.find((knowledgeBase) => knowledgeBase.id === knowledgeBaseId)?.name)
    .filter(Boolean);

  if (selectedNames.length <= 1) {
    return `「${selectedNames[0] ?? "未选择知识库"}」`;
  }

  return `${selectedNames.length} 个已选知识库`;
}

/** 执行浏览器开发态 Agent loop；无真实模型时只模拟显式工具动作，不预先推断自然语言意图。 */
export function runMockAgentTurn(
  snapshot: WorkspaceSnapshot,
  prompt: string,
  action: AgentActionType,
  clientMessageId?: string,
  explicitSkillIds: string[] = [],
): WorkspaceSnapshot {
  const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
  const session = nextSnapshot.sessions.find((item) => item.id === nextSnapshot.activeSessionId) ?? nextSnapshot.sessions[0];
  const activeNote = nextSnapshot.activeNoteId
    ? nextSnapshot.notes.find((note) => note.id === nextSnapshot.activeNoteId)
    : undefined;
  const activeKnowledgeBase =
    nextSnapshot.knowledgeBases.find((knowledgeBase) => knowledgeBase.id === nextSnapshot.activeKnowledgeBaseId) ??
    nextSnapshot.knowledgeBases[0];

  if (!session) {
    throw new Error("当前没有可用 Agent 会话。");
  }

  const userMessage: AgentMessage = {
    id: clientMessageId ?? createLocalId("user"),
    role: "user",
    content: prompt,
    action,
  };
  /** 去重后的显式 Skill ID；mock 环境只有 ID，不读取或保存 Skill 正文。 */
  const explicitSkillIdList = Array.from(new Set(explicitSkillIds.map((skillId) => skillId.trim()).filter(Boolean))).slice(0, 3);
  const toolCalls: AgentToolCall[] = [
    createToolCall(
      "skill_context",
      explicitSkillIdList.length
        ? `浏览器 mock 已收到 ${explicitSkillIdList.length} 个显式 Skill；真实桌面端会向模型注入完整 instructions。`
        : "浏览器 mock 未显式指定 Skill；真实模型场景会按已启用 Skills 的名称和描述自主判断。",
      { explicit: explicitSkillIdList.length > 0, explicitSkillCount: explicitSkillIdList.length },
    ),
    ...explicitSkillIdList.map((skillId) =>
      createToolCall("activate_skill", `浏览器开发态已模拟显式激活 Skill：${skillId}`, {
        skillId,
        explicit: true,
        instructionChars: null,
      }),
    ),
  ];
  let citations: Citation[] = [];
  let content = "";

  const hasExistingClientMessage = Boolean(
    clientMessageId && session.messages.some((message) => message.id === clientMessageId && message.role === "user"),
  );

  if (
    session.title.trim() === "新会话" &&
    !session.messages.some((message) => message.role === "user" && message.id !== clientMessageId)
  ) {
    session.title = prompt.trim() || "新会话";
  }

  // 前端会在发送瞬间先落库并渲染用户消息，mock loop 只在没有该消息时补齐。
  if (!hasExistingClientMessage) {
    session.messages.push(userMessage);
  }

  if (action === "rewrite") {
    if (!activeNote) {
      content = "当前没有可改写的 Markdown 笔记。";
    } else {
      const isAppend = shouldAppendToNote(prompt);
      const isMultiEdit = !isAppend && shouldMultiEditNote(prompt);
      const original = isAppend ? activeNote.content : getFirstBodyParagraph(activeNote.content);

      if (!original) {
        content = "我没有找到适合改写的正文段落。你可以先补充内容，再让我生成改写建议。";
      } else {
        const appendText = isAppend ? buildAppendText(prompt) : "";
        const nextContent = isMultiEdit
          ? activeNote.content.replace(original, buildRewriteText(original)).replace(/重复/g, "")
          : isAppend
            ? `${activeNote.content.trimEnd()}\n\n${appendText}`
            : buildRewriteText(original);
        const nextChange: ProposedChange = {
          id: createLocalId("change"),
          knowledgeBaseId: activeKnowledgeBase.id,
          noteId: activeNote.id,
          type: "rewrite",
          operation: isAppend ? "append" : isMultiEdit ? "multi_replace" : "replace",
          title: isAppend ? `追加到《${activeNote.title}》文末` : isMultiEdit ? `多处编辑《${activeNote.title}》` : `改写《${activeNote.title}》的核心段落`,
          targetPath: activeNote.path,
          original: isMultiEdit ? activeNote.content : original,
          next: nextContent,
          originalHash: activeNote.contentHash,
          status: "pending",
        };
        nextChange.diffStats = buildMarkdownDiff(nextChange.original, nextChange.next).stats;
        toolCalls.push(
          createToolCall("propose_note_change", `已为《${activeNote.title}》生成待确认改写 diff`, {
            noteId: activeNote.id,
            targetPath: activeNote.path,
            operation: nextChange.operation,
          }),
        );
        session.pendingChange = nextChange;
        session.activeNoteId = activeNote.id;
        session.pinnedNoteIds = Array.from(new Set([...session.pinnedNoteIds, activeNote.id]));
        content = "我已经生成一份改写建议。它现在只是待确认 diff，确认前不会修改本地 Markdown 文件。";
      }
    }
  } else if (action === "create") {
    const targetPath = activeKnowledgeBase.id === "kb-work" ? "Release/上线检查清单.md" : "00-Inbox/上线检查清单.md";
    const nextChange: ProposedChange = {
      id: createLocalId("change"),
      knowledgeBaseId: activeKnowledgeBase.id,
      type: "create",
      operation: undefined,
      title: "创建《上线检查清单》草稿",
      targetPath,
      original: "",
      next: `# 上线检查清单

## 产品体验
- 首次启动可以完成默认知识库目录选择。
- 主工作台可以切换多个本地知识库。
- Agent 回答包含工具轨迹和引用来源。

## 写入安全
- 所有 Agent 写入都先展示 diff。
- 用户确认后才更新当前知识库中的 Markdown。
- 取消写入时原文保持不变。`,
      originalHash: "",
      status: "pending",
    };
    nextChange.diffStats = buildMarkdownDiff(nextChange.original, nextChange.next).stats;
    toolCalls.push(createToolCall("create_note_draft", `已生成 ${targetPath} 的待确认新建 diff`, { targetPath }));
    session.pendingChange = nextChange;
    content = "我已经生成新笔记草稿，但它还没有写入本地目录。确认 diff 后才会创建 Markdown 文件。";
  } else if (action === "organize") {
    if (!activeNote) {
      content = "当前知识库没有可整理的 Markdown 笔记。";
    } else {
      toolCalls.push(
        createToolCall("suggest_organization", `已基于当前笔记生成整理建议`, {
          noteId: activeNote.id,
          knowledgeBaseId: activeKnowledgeBase.id,
        }),
      );
      content = `建议继续把《${activeNote.title}》保留在「${activeKnowledgeBase.name}」中，并补充更稳定的标签和相关链接。该建议不涉及写入。`;
    }
  } else if (shouldUseSearchTool(action)) {
    citations = searchNotes(nextSnapshot, session, prompt);
    toolCalls.push(
      createToolCall("search_notes", `在 ${getScopeLabel(nextSnapshot, session)} 中检索到 ${citations.length} 条候选引用`, {
        query: prompt,
        knowledgeBaseIds: session.knowledgeBaseIds,
      }),
    );

    if (citations[0]) {
      toolCalls.push(
        createToolCall("read_note", `已读取最相关笔记《${citations[0].title}》用于组织回答`, {
          noteId: citations[0].noteId,
        }),
      );
    }

    content = citations.length
      ? `我调用了检索工具，并只在 ${getScopeLabel(nextSnapshot, session)} 范围内组织回答：本地优先的关键是把 Markdown 文件作为用户拥有的主数据源，索引和模型请求都只是辅助层；写入必须先形成 diff，确认后才落盘。`
      : `我调用了检索工具，但在 ${getScopeLabel(nextSnapshot, session)} 中没有找到足够相关的笔记。`;
  } else {
    content = explicitSkillIdList.length
      ? `浏览器开发态未调用真实模型，但已模拟显式 Skill 激活（${explicitSkillIdList.length} 个）。桌面端会把所选 Skill instructions 注入本轮模型上下文。`
      : "浏览器开发态没有真实模型，因此不会替 Agent 预先选择工具。桌面端启用模型后，Agent 会自行判断是否检索、读取或生成待确认 diff。";
  }

  session.messages.push({
    id: createLocalId("assistant"),
    role: "assistant",
    content,
    action,
    citations,
    toolCalls,
  });
  session.updatedAt = "刚刚";

  return nextSnapshot;
}

/** 接受待确认变更，浏览器开发态只更新内存快照。 */
export function acceptMockProposedChange(snapshot: WorkspaceSnapshot): WorkspaceSnapshot {
  const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
  const session = nextSnapshot.sessions.find((item) => item.id === nextSnapshot.activeSessionId) ?? nextSnapshot.sessions[0];
  const pendingChange = session.pendingChange;

  if (!pendingChange) {
    return nextSnapshot;
  }

  if (pendingChange.type === "create") {
    const newNote: Note = {
      id: createLocalId("note"),
      knowledgeBaseId: pendingChange.knowledgeBaseId,
      title: pendingChange.title.replace(/^创建《|》草稿$/g, "") || "Agent 新建草稿",
      path: pendingChange.targetPath,
      content: pendingChange.next,
      tags: ["Agent", "草稿"],
      updatedAt: "刚刚",
      backlinks: [],
      contentHash: createContentHash(pendingChange.next),
    };
    nextSnapshot.notes = [newNote, ...nextSnapshot.notes];
    nextSnapshot.activeNoteId = newNote.id;
    nextSnapshot.activeDocumentId = "";
    session.activeNoteId = newNote.id;
    session.pinnedNoteIds = Array.from(new Set([...session.pinnedNoteIds, newNote.id]));
  } else if (pendingChange.noteId) {
    nextSnapshot.notes = nextSnapshot.notes.map((note) => {
      // 只更新 diff 指向的笔记，避免用户切换后误写其他文件。
      if (note.id !== pendingChange.noteId) {
        return note;
      }

      const nextContent =
        pendingChange.operation === "append" || pendingChange.operation === "multi_replace"
          ? applyAppendForMock(note.content, pendingChange.original, pendingChange.next)
          : replaceUniqueForMock(note.content, pendingChange.original, pendingChange.next);

      return {
        ...note,
        content: nextContent,
        updatedAt: "刚刚",
        contentHash: createContentHash(nextContent),
      };
    });
    nextSnapshot.activeDocumentId = "";
  }

  session.pendingChange = { ...pendingChange, status: "accepted" };
  session.messages.push({
    id: createLocalId("assistant"),
    role: "assistant",
    content: "已根据你的确认应用这次变更。正式桌面版会在这里完成路径校验、hash 校验和原子写入。",
    action: pendingChange.type,
    toolCalls: [
      createToolCall("review_change", "用户已确认审阅 diff，mock 环境已更新内存中的笔记内容", {
        changeId: pendingChange.id,
      }),
    ],
  });

  return nextSnapshot;
}

/** 浏览器开发态执行整篇追加 diff，保持与 Tauri 层 append 写入规则一致。 */
function applyAppendForMock(content: string, original: string, next: string) {
  if (content !== original) {
    throw new Error("目标文件已变化，已阻止追加写入。请重新生成 diff。");
  }

  return next;
}

/** 浏览器开发态执行单处替换，保持与 Tauri 层 rewrite 写入规则一致。 */
function replaceUniqueForMock(content: string, original: string, next: string) {
  const matchCount = countNonOverlappingMatches(content, original);

  if (!original) {
    throw new Error("待写入 diff 缺少原文片段，已阻止写入。请重新生成 diff。");
  }

  if (matchCount === 0) {
    throw new Error("待写入 diff 的原文片段未命中当前文件，已阻止写入。请重新生成 diff。");
  }

  if (matchCount > 1) {
    throw new Error("待写入 diff 的原文片段在当前文件中出现多次，已阻止写入。请重新生成更精确的 diff。");
  }

  return content.replace(original, next);
}

/** 统计非重叠文本命中次数，用于 mock 写入前发现模糊定位。 */
function countNonOverlappingMatches(content: string, needle: string) {
  if (!needle) {
    return 0;
  }

  let count = 0;
  let fromIndex = 0;

  while (fromIndex <= content.length) {
    const matchIndex = content.indexOf(needle, fromIndex);

    if (matchIndex < 0) {
      break;
    }

    count += 1;
    fromIndex = matchIndex + needle.length;
  }

  return count;
}

/** 拒绝待确认变更，保留原始 Markdown 内容不变。 */
export function rejectMockProposedChange(snapshot: WorkspaceSnapshot): WorkspaceSnapshot {
  const nextSnapshot = cloneWorkspaceSnapshot(snapshot);
  const session = nextSnapshot.sessions.find((item) => item.id === nextSnapshot.activeSessionId) ?? nextSnapshot.sessions[0];
  const pendingChange = session.pendingChange;

  if (!pendingChange) {
    return nextSnapshot;
  }

  session.pendingChange = { ...pendingChange, status: "rejected" };
  session.messages.push({
    id: createLocalId("assistant"),
    role: "assistant",
    content: "已取消本次写入建议，原始 Markdown 内容保持不变。",
    action: pendingChange.type,
    toolCalls: [],
  });

  return nextSnapshot;
}
