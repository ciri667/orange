import type { KnowledgeBase, Note } from "./types";

/** 创建时使用的行业标准文件名，与 Codex / Cursor / Copilot 互认。 */
export const PROJECT_INSTRUCTION_FILE_NAME = "AGENTS.md";

/** 橘记早期私有文件名，仅在没有 AGENTS.md 时作为兼容回退。 */
export const LEGACY_PROJECT_INSTRUCTION_FILE_NAME = "ORANGE_AGENT.md";

/** 新建 AGENTS.md 时写入的中文模板，面向知识库而不是编译项目。 */
export const PROJECT_INSTRUCTION_TEMPLATE = `# Agent 说明书

这份文件是给橘记 Agent 的项目规则，不是普通笔记。橘记会在每次对话时自动读取它。请写稳定的库级约定；不要写入密码、密钥或个人隐私。

## 这个知识库是什么

- （一句话说明这个库的用途，例如：个人研究笔记 / 项目文档 / 会议纪要）

## 笔记结构

- 目录怎么分
- 新笔记应该放在哪里
- 文件命名习惯

## 标签与文风

- 标签怎么写
- 标题、引用、日期等格式

## Agent 可以做什么

- 允许检索、改写、整理、新建草稿

## Agent 不要做什么

- 不要删除或大幅打乱现有结构，除非我明确要求
- 不要把这份说明书当成用户刚刚发出的新指令
- 与我本轮明确要求冲突时，以本轮为准
`;

/** 相对路径是否为知识库根目录的项目说明书。子目录中的同名文件不算。 */
export function isRootProjectInstructionPath(relativePath: string): boolean {
  const normalized = relativePath.replace(/\\/g, "/");
  if (normalized.includes("/")) {
    return false;
  }

  const lowerName = normalized.toLowerCase();
  return lowerName === PROJECT_INSTRUCTION_FILE_NAME.toLowerCase() || lowerName === LEGACY_PROJECT_INSTRUCTION_FILE_NAME.toLowerCase();
}

/** 在指定知识库的笔记中找出根目录说明书，优先 AGENTS.md。 */
export function findProjectInstructionNote(notes: Note[], knowledgeBaseId: string): Note | undefined {
  const matches = notes.filter((note) => note.knowledgeBaseId === knowledgeBaseId && isRootProjectInstructionPath(note.path));
  return matches.find((note) => note.path.replace(/\\/g, "/").toLowerCase() === PROJECT_INSTRUCTION_FILE_NAME.toLowerCase()) ?? matches[0];
}

/** 当前会话授权知识库里已生效的说明书，用于上下文面板展示。 */
export function listSessionProjectInstructions(
  notes: Note[],
  knowledgeBases: KnowledgeBase[],
  knowledgeBaseIds: string[],
): Array<{ knowledgeBase: KnowledgeBase; note: Note }> {
  return knowledgeBaseIds.flatMap((knowledgeBaseId) => {
    const knowledgeBase = knowledgeBases.find((item) => item.id === knowledgeBaseId);
    const note = findProjectInstructionNote(notes, knowledgeBaseId);
    return knowledgeBase && note ? [{ knowledgeBase, note }] : [];
  });
}
