import type { FileTreeNode, FolderEntry, KnowledgeBase, Note } from "../shared/types";

/** 对目录树节点排序，保持文件夹在前、文件在后，并按中文路径名称稳定展示。 */
export function sortFileTreeNodes(nodes: FileTreeNode[]): FileTreeNode[] {
  return nodes
    .sort((leftNode, rightNode) => {
      // 文件夹优先可以让树更接近桌面文件管理器的浏览习惯。
      if (leftNode.type !== rightNode.type) {
        return leftNode.type === "folder" ? -1 : 1;
      }

      return leftNode.name.localeCompare(rightNode.name, "zh-CN");
    })
    .map((node) => {
      if (node.type !== "folder") {
        return node;
      }

      return { ...node, children: sortFileTreeNodes(node.children) };
    });
}

/** 根据真实目录节点和 Markdown 文件构建以知识库根目录为顶层的目录树。 */
export function buildFileTree({
  knowledgeBase,
  folders,
  notes,
  searchTerm,
}: {
  knowledgeBase: KnowledgeBase;
  folders: FolderEntry[];
  notes: Note[];
  searchTerm: string;
}): FileTreeNode[] {
  const normalizedSearchTerm = searchTerm.trim().toLowerCase();
  const activeFolders = folders.filter((folder) => folder.knowledgeBaseId === knowledgeBase.id);
  const activeNotes = notes.filter((note) => note.knowledgeBaseId === knowledgeBase.id);
  const visibleFolderPaths = getVisibleFolderPaths(activeFolders, activeNotes, normalizedSearchTerm);
  const visibleNotes = getVisibleNotes(activeNotes, activeFolders, normalizedSearchTerm);
  const rootNode: FileTreeNode = {
    id: `folder-root-${knowledgeBase.id}`,
    name: knowledgeBase.name || "根目录",
    path: "",
    type: "folder",
    isRoot: true,
    children: [],
  };
  const folderMap = new Map<string, FileTreeNode>([["", rootNode]]);

  visibleFolderPaths.forEach((folderPath) => {
    ensureFolderNode(folderPath, folderMap, rootNode);
  });

  visibleNotes.forEach((note) => {
    const pathParts = splitRelativePath(note.path);
    const fileName = pathParts[pathParts.length - 1] ?? note.title;
    const parentPath = pathParts.slice(0, -1).join("/");
    const parentNode = ensureFolderNode(parentPath, folderMap, rootNode);

    parentNode.children.push({
      id: `file-${note.id}`,
      name: fileName,
      path: note.path,
      type: "file",
      noteId: note.id,
      children: [],
    });
  });

  rootNode.children = sortFileTreeNodes(rootNode.children);

  return [rootNode];
}

/** 计算搜索状态下需要展示的目录路径集合，包含匹配目录和匹配笔记的所有父级。 */
function getVisibleFolderPaths(folders: FolderEntry[], notes: Note[], normalizedSearchTerm: string) {
  if (!normalizedSearchTerm) {
    return folders.map((folder) => folder.path);
  }

  const visiblePaths = new Set<string>();
  const matchingFolderPaths = folders
    .filter((folder) => `${folder.name} ${folder.path}`.toLowerCase().includes(normalizedSearchTerm))
    .map((folder) => folder.path);

  matchingFolderPaths.forEach((folderPath) => {
    addPathAndAncestors(visiblePaths, folderPath);

    // 搜索命中文件夹时继续展示它的子目录，避免用户只看到空父节点。
    folders
      .filter((folder) => isInsideFolder(folder.path, folderPath))
      .forEach((folder) => addPathAndAncestors(visiblePaths, folder.path));
  });

  notes
    .filter((note) => doesNoteMatchSearch(note, normalizedSearchTerm) || matchingFolderPaths.some((path) => isInsideFolder(note.path, path)))
    .forEach((note) => {
      const parentPath = splitRelativePath(note.path).slice(0, -1).join("/");

      addPathAndAncestors(visiblePaths, parentPath);
    });

  return Array.from(visiblePaths);
}

/** 计算搜索状态下需要展示的笔记；文件夹命中时展示该文件夹下的笔记。 */
function getVisibleNotes(notes: Note[], folders: FolderEntry[], normalizedSearchTerm: string) {
  if (!normalizedSearchTerm) {
    return notes;
  }

  const matchingFolderPaths = folders
    .filter((folder) => `${folder.name} ${folder.path}`.toLowerCase().includes(normalizedSearchTerm))
    .map((folder) => folder.path);

  return notes.filter(
    (note) => doesNoteMatchSearch(note, normalizedSearchTerm) || matchingFolderPaths.some((path) => isInsideFolder(note.path, path)),
  );
}

/** 创建或返回指定路径的目录节点，同时补齐缺失的祖先目录节点。 */
function ensureFolderNode(folderPath: string, folderMap: Map<string, FileTreeNode>, rootNode: FileTreeNode) {
  const normalizedFolderPath = folderPath.trim().replace(/^\/+|\/+$/g, "");

  if (!normalizedFolderPath) {
    return rootNode;
  }

  const existingNode = folderMap.get(normalizedFolderPath);

  if (existingNode) {
    return existingNode;
  }

  const pathParts = splitRelativePath(normalizedFolderPath);
  const folderName = pathParts[pathParts.length - 1] ?? "未命名目录";
  const parentPath = pathParts.slice(0, -1).join("/");
  const parentNode = ensureFolderNode(parentPath, folderMap, rootNode);
  const nextNode: FileTreeNode = {
    id: `folder-${normalizedFolderPath}`,
    name: folderName,
    path: normalizedFolderPath,
    type: "folder",
    children: [],
  };

  folderMap.set(normalizedFolderPath, nextNode);
  parentNode.children.push(nextNode);

  return nextNode;
}

/** 判断笔记标题、路径、标签或正文是否命中当前搜索词。 */
function doesNoteMatchSearch(note: Note, normalizedSearchTerm: string) {
  const searchableText = `${note.title} ${note.path} ${note.tags.join(" ")} ${note.content}`.toLowerCase();

  return searchableText.includes(normalizedSearchTerm);
}

/** 把相对路径按目录层级切分，统一过滤空片段。 */
function splitRelativePath(relativePath: string) {
  return relativePath.split("/").filter(Boolean);
}

/** 把某一路径及其所有父级目录加入可见集合。 */
function addPathAndAncestors(paths: Set<string>, folderPath: string) {
  const pathParts = splitRelativePath(folderPath);

  for (let index = 1; index <= pathParts.length; index += 1) {
    paths.add(pathParts.slice(0, index).join("/"));
  }
}

/** 判断文件或目录路径是否位于指定目录内，目录自身也算命中。 */
function isInsideFolder(candidatePath: string, folderPath: string) {
  return candidatePath === folderPath || candidatePath.startsWith(`${folderPath}/`);
}
