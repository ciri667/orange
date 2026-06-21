import { AlertCircle, BookOpen, Database, Loader2, Plus, RefreshCw, Search } from "lucide-react";
import { FileTree } from "./FileTree";
import type { FileTreeNode, KnowledgeBase } from "../shared/types";

/** 左侧知识库导航，包含知识库切换、搜索和本地目录树。 */
export function KnowledgeBaseSidebar({
  knowledgeBases,
  activeKnowledgeBase,
  fileTree,
  activeNoteId,
  collapsedFolderPaths,
  searchTerm,
  isBusy,
  busyLabel,
  notice,
  onSearchChange,
  onSelectKnowledgeBase,
  onAddKnowledgeBase,
  onToggleFolder,
  onSelectNote,
  onRenameNote,
  onDeleteNote,
  onCreateDocument,
  onCreateFolder,
  onRefreshKnowledgeBase,
}: {
  knowledgeBases: KnowledgeBase[];
  activeKnowledgeBase: KnowledgeBase;
  fileTree: FileTreeNode[];
  activeNoteId: string;
  collapsedFolderPaths: Set<string>;
  searchTerm: string;
  isBusy: boolean;
  busyLabel: string;
  notice: string;
  onSearchChange: (value: string) => void;
  onSelectKnowledgeBase: (knowledgeBaseId: string) => void;
  onAddKnowledgeBase: () => void;
  onToggleFolder: (folderPath: string) => void;
  onSelectNote: (noteId: string) => void;
  onRenameNote: (noteId: string) => void;
  onDeleteNote: (noteId: string) => void;
  onCreateDocument: (parentPath: string) => void;
  onCreateFolder: (parentPath: string) => void;
  onRefreshKnowledgeBase: (knowledgeBaseId: string) => void;
}) {
  return (
    <aside className="sidebar" aria-label="知识库导航">
      <div className="workspace-title">
        <div className="workspace-icon">
          <BookOpen size={18} />
        </div>
        <div>
          <strong>Cici 工作区</strong>
          <span>{knowledgeBases.length} 个本地知识库</span>
        </div>
      </div>

      <section className="kb-switcher" aria-label="知识库切换">
        <div className="section-header">
          <p className="section-label">当前知识库</p>
          <span>单库激活</span>
        </div>
        {knowledgeBases.map((knowledgeBase) => (
          <button
            className={`kb-row ${knowledgeBase.id === activeKnowledgeBase.id ? "active" : ""} ${knowledgeBase.status === "error" ? "error" : ""}`}
            key={knowledgeBase.id}
            type="button"
            onClick={() => onSelectKnowledgeBase(knowledgeBase.id)}
          >
            {knowledgeBase.status === "error" ? <AlertCircle size={15} /> : <Database size={15} />}
            <span className="kb-row-copy">
              <strong>{knowledgeBase.name}</strong>
              <span>
                {knowledgeBase.noteCount} 篇 · {getKnowledgeBaseStatusLabel(knowledgeBase)}
              </span>
            </span>
          </button>
        ))}
        <button className="add-kb-button" type="button" onClick={onAddKnowledgeBase}>
          <Plus size={15} />
          添加知识库
        </button>
      </section>

      {(isBusy || notice) && (
        <div className={`operation-notice ${notice.includes("失败") || notice.includes("阻止") ? "error" : ""}`}>
          {isBusy && <Loader2 size={14} />}
          <span>{busyLabel || notice}</span>
        </div>
      )}

      <label className="search-box">
        <Search size={16} />
        <input
          value={searchTerm}
          onChange={(event) => onSearchChange(event.target.value)}
          placeholder="搜索当前目录树"
          type="search"
        />
      </label>

      <div className="local-tree" aria-label="本地目录树">
        <div className="section-header">
          <p className="section-label">本地目录树</p>
          <div className="section-actions">
            <span>{activeKnowledgeBase.status === "error" ? "目录失效" : "Markdown"}</span>
            <button
              className="tree-refresh-button"
              type="button"
              title="手动刷新目录树"
              onClick={() => onRefreshKnowledgeBase(activeKnowledgeBase.id)}
              disabled={isBusy}
            >
              <RefreshCw size={13} />
              刷新
            </button>
          </div>
        </div>
        <p className="root-path">{activeKnowledgeBase.path}</p>
        <ScanReportSummary knowledgeBase={activeKnowledgeBase} />
        <FileTree
          nodes={fileTree}
          activeNoteId={activeNoteId}
          collapsedFolderPaths={collapsedFolderPaths}
          onToggleFolder={onToggleFolder}
          onSelectNote={onSelectNote}
          onRenameNote={onRenameNote}
          onDeleteNote={onDeleteNote}
          onCreateDocument={onCreateDocument}
          onCreateFolder={onCreateFolder}
        />
      </div>
    </aside>
  );
}

/** 把知识库状态转成侧栏短标签，帮助用户快速识别失效目录和索引状态。 */
function getKnowledgeBaseStatusLabel(knowledgeBase: KnowledgeBase) {
  if (knowledgeBase.status === "error") {
    return "目录失效";
  }

  if (knowledgeBase.status === "scanning") {
    return "扫描中";
  }

  return knowledgeBase.semanticIndexEnabled ? "语义索引" : "FTS 索引";
}

/** 展示最近一次扫描结果，覆盖空目录、坏文件和跳过目录反馈。 */
function ScanReportSummary({ knowledgeBase }: { knowledgeBase: KnowledgeBase }) {
  const report = knowledgeBase.scanReport;

  if (knowledgeBase.status === "error") {
    return <p className="scan-summary error">{knowledgeBase.description}</p>;
  }

  if (!report) {
    return null;
  }

  const skippedText = report.skippedDirectories.length ? `，跳过 ${report.skippedDirectories.length} 个目录` : "";
  const errorText = report.failedFileCount ? `，${report.failedFileCount} 个读取失败` : "";

  return (
    <p className={`scan-summary ${report.failedFileCount ? "warning" : ""}`}>
      已扫描 {report.scannedFileCount} 篇{errorText}
      {skippedText}
    </p>
  );
}
