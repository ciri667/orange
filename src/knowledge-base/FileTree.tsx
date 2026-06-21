import { ChevronDown, ChevronRight, FilePenLine, FileText, FolderOpen, FolderPlus, Plus, Trash2 } from "lucide-react";
import { useState } from "react";
import type { FileTreeNode } from "../shared/types";

/** 本地文件树组件，递归展示文件夹和 Markdown 文件。 */
export function FileTree({
  nodes,
  activeNoteId,
  collapsedFolderPaths,
  depth = 0,
  onToggleFolder,
  onSelectNote,
  onRenameNote,
  onDeleteNote,
  onCreateDocument,
  onCreateFolder,
}: {
  nodes: FileTreeNode[];
  activeNoteId: string;
  collapsedFolderPaths: Set<string>;
  depth?: number;
  onToggleFolder: (folderPath: string) => void;
  onSelectNote: (noteId: string) => void;
  onRenameNote: (noteId: string) => void;
  onDeleteNote: (noteId: string) => void;
  onCreateDocument: (parentPath: string) => void;
  onCreateFolder: (parentPath: string) => void;
}) {
  const [openCreateMenuPath, setOpenCreateMenuPath] = useState<string | null>(null);

  if (!nodes.length && depth === 0) {
    return <p className="file-tree-empty">没有匹配的 Markdown 文件</p>;
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
                          onCreateDocument(node.path);
                        }}
                      >
                        <FileText size={14} />
                        新建文档
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
                  collapsedFolderPaths={collapsedFolderPaths}
                  depth={depth + 1}
                  onToggleFolder={onToggleFolder}
                  onSelectNote={onSelectNote}
                  onRenameNote={onRenameNote}
                  onDeleteNote={onDeleteNote}
                  onCreateDocument={onCreateDocument}
                  onCreateFolder={onCreateFolder}
                />
              )}
            </li>
          );
        }

        const noteId = node.noteId;

        return (
          <li key={node.id}>
            <div
              className={`file-tree-row file ${noteId === activeNoteId ? "active" : ""}`}
              style={{ paddingLeft: depth * 14 + 28 }}
              role="treeitem"
              aria-selected={noteId === activeNoteId}
            >
              <button
                className="file-tree-open-button"
                type="button"
                title={node.name}
                onClick={() => {
                  if (!noteId) {
                    return;
                  }

                  onSelectNote(noteId);
                }}
              >
                <FileText size={14} />
                <span className="file-tree-name">{node.name}</span>
              </button>
              {noteId && (
                <div className="file-tree-actions">
                  <button
                    className="file-action-button"
                    type="button"
                    title="重命名文件"
                    onClick={(event) => {
                      event.stopPropagation();
                      onRenameNote(noteId);
                    }}
                  >
                    <FilePenLine size={14} />
                  </button>
                  <button
                    className="file-action-button danger"
                    type="button"
                    title="删除文件"
                    onClick={(event) => {
                      event.stopPropagation();
                      onDeleteNote(noteId);
                    }}
                  >
                    <Trash2 size={14} />
                  </button>
                </div>
              )}
            </div>
          </li>
        );
      })}
    </ul>
  );
}
