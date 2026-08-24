import type { WorkspaceSnapshot } from "../shared/types";

/** 从知识库相对路径中取最后一级文件名，用于重命名弹窗默认值。 */
export function getFileNameFromPath(relativePath: string) {
  return relativePath.split("/").filter(Boolean).pop() ?? relativePath;
}

/** 拼接知识库内相对路径，根目录下只返回子名称。 */
export function joinRelativePath(parentPath: string, childName: string) {
  return parentPath ? `${parentPath}/${childName}` : childName;
}

/** 收集当前知识库中已经被文件占用的路径，覆盖 Markdown 和普通文档。 */
export function getExistingFilePaths(snapshot: WorkspaceSnapshot, knowledgeBaseId: string) {
  return new Set([
    ...snapshot.notes.filter((note) => note.knowledgeBaseId === knowledgeBaseId).map((note) => note.path),
    ...snapshot.documents.filter((document) => document.knowledgeBaseId === knowledgeBaseId).map((document) => document.path),
  ]);
}

/** 为新建 Markdown 生成当前父目录下不冲突的默认名称。 */
export function getNextAvailableMarkdownName(snapshot: WorkspaceSnapshot, knowledgeBaseId: string, parentPath: string) {
  const existingPaths = getExistingFilePaths(snapshot, knowledgeBaseId);

  for (let index = 1; index <= 999; index += 1) {
    const fileName = index === 1 ? "未命名.md" : `未命名 ${index}.md`;

    // 默认名称只看当前目标目录，避免用户打开弹窗后马上遇到后端重名错误。
    if (!existingPaths.has(joinRelativePath(parentPath, fileName))) {
      return fileName;
    }
  }

  return "未命名.md";
}

/** 为新建 TXT 生成当前父目录下不冲突的默认名称。 */
export function getNextAvailableTextDocumentName(snapshot: WorkspaceSnapshot, knowledgeBaseId: string, parentPath: string) {
  const existingPaths = getExistingFilePaths(snapshot, knowledgeBaseId);

  for (let index = 1; index <= 999; index += 1) {
    const fileName = index === 1 ? "未命名.txt" : `未命名 ${index}.txt`;

    // 默认名称只看当前目标目录，真正文件系统冲突仍由 Tauri 后端最终校验。
    if (!existingPaths.has(joinRelativePath(parentPath, fileName))) {
      return fileName;
    }
  }

  return "未命名.txt";
}

/** 为新建目录生成当前父目录下不冲突的默认名称。 */
export function getNextAvailableFolderName(snapshot: WorkspaceSnapshot, knowledgeBaseId: string, parentPath: string) {
  const existingPaths = new Set([
    ...snapshot.folders.filter((folder) => folder.knowledgeBaseId === knowledgeBaseId).map((folder) => folder.path),
    ...getExistingFilePaths(snapshot, knowledgeBaseId),
  ]);

  for (let index = 1; index <= 999; index += 1) {
    const folderName = index === 1 ? "新建文件夹" : `新建文件夹 ${index}`;

    // 文件夹默认名称只根据目录节点判断，真正文件系统冲突仍由 Tauri 后端最终校验。
    if (!existingPaths.has(joinRelativePath(parentPath, folderName))) {
      return folderName;
    }
  }

  return "新建文件夹";
}

/** 弹窗中展示当前创建位置，根目录用明确名称避免路径为空带来的歧义。 */
export function getCreateParentLabel(parentPath: string) {
  return parentPath ? `创建位置：${parentPath}` : "创建位置：根目录";
}

/** 返回新建弹窗标题。 */
export function getCreateDialogTitle(kind: "markdown" | "text" | "folder") {
  if (kind === "markdown") {
    return "新建 Markdown";
  }

  if (kind === "text") {
    return "新建 TXT";
  }

  return "新建目录";
}

/** 返回新建弹窗无障碍标签。 */
export function getCreateDialogAriaLabel(kind: "markdown" | "text" | "folder") {
  if (kind === "markdown") {
    return "新建 Markdown 文档";
  }

  if (kind === "text") {
    return "新建 TXT 文档";
  }

  return "新建目录";
}

/** 返回新建输入框占位文案。 */
export function getCreatePlaceholder(kind: "markdown" | "text" | "folder") {
  if (kind === "markdown") {
    return "例如：会议记录";
  }

  if (kind === "text") {
    return "例如：灵感草稿";
  }

  return "例如：Projects";
}

/** 返回新建提交按钮文案。 */
export function getCreateSubmitLabel(kind: "markdown" | "text" | "folder") {
  if (kind === "markdown") {
    return "创建 Markdown";
  }

  if (kind === "text") {
    return "创建 TXT";
  }

  return "创建目录";
}
