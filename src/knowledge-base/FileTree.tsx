import { ChevronDown, ChevronRight, File, FilePenLine, FileText, FileType, FolderOpen, FolderPlus, Plus, Trash2 } from "lucide-react";
import { useState } from "react";
import type { FileTreeNode } from "../shared/types";

/** 本地文件树组件，递归展示文件夹、Markdown、txt、docx 和 pdf 文件。 */
export function FileTree({
  nodes,
  activeNoteId,
  activeDocumentId,
  collapsedFolderPaths,
  depth = 0,
  onToggleFolder,
  onSelectNote,
  onSelectDocument,
  onRenameNote,
  onDeleteNote,
  onRenameDocument,
  onDeleteDocument,
  onCreateMarkdown,
  onCreateText,
  onCreateFolder,
}: {
  nodes: FileTreeNode[];
  activeNoteId: string;
  activeDocumentId: string;
  collapsedFolderPaths: Set<string>;
  depth?: number;
  onToggleFolder: (folderPath: string) => void;
  onSelectNote: (noteId: string) => void;
  onSelectDocument: (documentId: string) => void;
  onRenameNote: (noteId: string) => void;
  onDeleteNote: (noteId: string) => void;
  onRenameDocument: (documentId: string) => void;
  onDeleteDocument: (documentId: string) => void;
  onCreateMarkdown: (parentPath: string) => void;
  onCreateText: (parentPath: string) => void;
  onCreateFolder: (parentPath: string) => void;
}) {
  const [openCreateMenuPath, setOpenCreateMenuPath] = useState<string | null>(null);

  if (!nodes.length && depth === 0) {
    return <p className="file-tree-empty">没有匹配的支持文档</p>;
  }

  return (
    <ul className={`file-tree-list ${depth === 0 ? "root" : ""}`} role={depth === 0 ? "tree" : "group"}>
      {nodes.map((node) => {
        const isCollapsed = collapsedFolderPaths.has(node.path);

        // 文件夹节点只控制展开状态，不直接打开笔记。
        if (node.type === "folder") {
          return (
            <li key={node.id}>
              <div
                className={`file-tree-row folder ${node.isRoot ? "root-folder" : ""}`}
                style={{ paddingLeft: depth * 14 + 6 }}
              >
                <button
                  className="file-tree-open-button"
                  type="button"
                  aria-expanded={!isCollapsed}
                  onClick={() => onToggleFolder(node.path)}
                >
                  {isCollapsed ? <ChevronRight size={14} /> : <ChevronDown size={14} />}
                  <FolderOpen size={15} />
                  <span className="file-tree-name">{node.name}</span>
                </button>
                <span className="file-tree-count">{node.children.length}</span>
                <div className="file-tree-actions">
                  <button
                    className="file-action-button"
                    type="button"
                    title={`在「${node.name}」中新建`}
                    aria-haspopup="menu"
                    aria-expanded={openCreateMenuPath === node.path}
                    onClick={(event) => {
                      event.stopPropagation();
                      setOpenCreateMenuPath(openCreateMenuPath === node.path ? null : node.path);
                    }}
                  >
                    <Plus size={14} />
                  </button>
                  {openCreateMenuPath === node.path && (
                    <div className="create-action-menu" role="menu">
                      <button
                        type="button"
                        role="menuitem"
                        onClick={() => {
                          setOpenCreateMenuPath(null);
                          onCreateMarkdown(node.path);
                        }}
                      >
                        <FileText size={14} />
                        新建 Markdown
                      </button>
                      <button
                        type="button"
                        role="menuitem"
                        onClick={() => {
                          setOpenCreateMenuPath(null);
                          onCreateText(node.path);
                        }}
                      >
                        <FileType size={14} />
                        新建 TXT
                      </button>
                      <button
                        type="button"
                        role="menuitem"
                        onClick={() => {
                          setOpenCreateMenuPath(null);
                          onCreateFolder(node.path);
                        }}
                      >
                        <FolderPlus size={14} />
                        新建目录
                      </button>
                    </div>
                  )}
                </div>
              </div>
              {!isCollapsed && (
                <FileTree
                  nodes={node.children}
                  activeNoteId={activeNoteId}
                  activeDocumentId={activeDocumentId}
                  collapsedFolderPaths={collapsedFolderPaths}
                  depth={depth + 1}
                  onToggleFolder={onToggleFolder}
                  onSelectNote={onSelectNote}
                  onSelectDocument={onSelectDocument}
                  onRenameNote={onRenameNote}
                  onDeleteNote={onDeleteNote}
                  onRenameDocument={onRenameDocument}
                  onDeleteDocument={onDeleteDocument}
                  onCreateMarkdown={onCreateMarkdown}
                  onCreateText={onCreateText}
                  onCreateFolder={onCreateFolder}
                />
              )}
            </li>
          );
        }

        const noteId = node.noteId;
        const documentId = node.documentId;
        const isActiveFile = noteId === activeNoteId || documentId === activeDocumentId;
        const canRename = Boolean(node.capabilities?.canRename);
        const canDelete = Boolean(node.capabilities?.canDelete);

        return (
          <li key={node.id}>
            <div
              className={`file-tree-row file ${isActiveFile ? "active" : ""}`}
              style={{ paddingLeft: depth * 14 + 28 }}
              role="treeitem"
              aria-selected={isActiveFile}
            >
              <button
                className="file-tree-open-button"
                type="button"
                title={node.name}
                onClick={() => {
                  if (noteId) {
                    onSelectNote(noteId);
                  } else if (documentId) {
                    onSelectDocument(documentId);
                  }
                }}
              >
                <FileTreeIcon node={node} />
                <span className="file-tree-name">{node.name}</span>
              </button>
              {(canRename || canDelete) && (
                <div className="file-tree-actions">
                  {canRename && (
                    <button
                      className="file-action-button"
                      type="button"
                      title="重命名文件"
                      onClick={(event) => {
                        event.stopPropagation();
                        if (noteId) {
                          onRenameNote(noteId);
                        } else if (documentId) {
                          onRenameDocument(documentId);
                        }
                      }}
                    >
                      <FilePenLine size={14} />
                    </button>
                  )}
                  {canDelete && (
                    <button
                      className="file-action-button danger"
                      type="button"
                      title="删除文件"
                      onClick={(event) => {
                        event.stopPropagation();
                        if (noteId) {
                          onDeleteNote(noteId);
                        } else if (documentId) {
                          onDeleteDocument(documentId);
                        }
                      }}
                    >
                      <Trash2 size={14} />
                    </button>
                  )}
                </div>
              )}
            </div>
          </li>
        );
      })}
    </ul>
  );
}

/** 根据文件类型选择目录树图标，帮助用户快速区分编辑和预览文档。 */
function FileTreeIcon({ node }: { node: FileTreeNode }) {
  if (node.fileType === "txt") {
    return <FileType size={14} />;
  }

  if (node.fileType === "docx" || node.fileType === "pdf") {
    return <File size={14} />;
  }

  return <FileText size={14} />;
}
