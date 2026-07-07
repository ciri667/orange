import { AlertCircle, BookOpen, Database, Loader2, Plus, RefreshCw, Search } from "lucide-react";
import { FileTree } from "./FileTree";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import type { FileTreeNode, KnowledgeBase } from "../shared/types";

/** 汇总当前资料库文档数量，用于侧栏标题中的低噪音概览。 */
function getKnowledgeBaseAssetCount(knowledgeBases: KnowledgeBase[]) {
  return knowledgeBases.reduce((total, knowledgeBase) => total + knowledgeBase.noteCount + knowledgeBase.documentCount, 0);
}

/** 生成单个资料库文件数量摘要，总数优先，Markdown 数量作为类型补充。 */
function getKnowledgeBaseFileSummary(knowledgeBase: KnowledgeBase) {
  const fileCount = knowledgeBase.noteCount + knowledgeBase.documentCount;

  return `${fileCount} 个文件 · ${knowledgeBase.noteCount} 个 Markdown`;
}

/** 左侧知识库导航，包含知识库切换、搜索和本地目录树。 */
export function KnowledgeBaseSidebar({
  knowledgeBases,
  activeKnowledgeBase,
  fileTree,
  activeNoteId,
  activeDocumentId,
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
  onSelectDocument,
  onRenameNote,
  onDeleteNote,
  onRenameDocument,
  onDeleteDocument,
  onCreateMarkdown,
  onCreateText,
  onCreateFolder,
  onRefreshKnowledgeBase,
}: {
  knowledgeBases: KnowledgeBase[];
  activeKnowledgeBase: KnowledgeBase;
  fileTree: FileTreeNode[];
  activeNoteId: string;
  activeDocumentId: string;
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
  onSelectDocument: (documentId: string) => void;
  onRenameNote: (noteId: string) => void;
  onDeleteNote: (noteId: string) => void;
  onRenameDocument: (documentId: string) => void;
  onDeleteDocument: (documentId: string) => void;
  onCreateMarkdown: (parentPath: string) => void;
  onCreateText: (parentPath: string) => void;
  onCreateFolder: (parentPath: string) => void;
  onRefreshKnowledgeBase: (knowledgeBaseId: string) => void;
}) {
  const assetCount = getKnowledgeBaseAssetCount(knowledgeBases);

  return (
    <aside className="sidebar" aria-label="知识库导航">
      <div className="workspace-title">
        <div className="workspace-icon">
          <BookOpen size={18} />
        </div>
        <div>
          <strong>资料库</strong>
          <span>
            {knowledgeBases.length} 个本地库 · {assetCount} 个文件
          </span>
        </div>
      </div>

      <section className="kb-switcher" aria-label="知识库切换">
        <div className="section-header">
          <p className="section-label">Library</p>
          <span>本地优先</span>
        </div>
        {knowledgeBases.map((knowledgeBase) => {
          const knowledgeBaseSummary = `${getKnowledgeBaseFileSummary(knowledgeBase)} · ${getKnowledgeBaseStatusLabel(knowledgeBase)}`;

          return (
            <button
              className={`kb-row ${knowledgeBase.id === activeKnowledgeBase.id ? "active" : ""} ${knowledgeBase.status === "error" ? "error" : ""}`}
              key={knowledgeBase.id}
              type="button"
              aria-label={`${knowledgeBase.name}，${knowledgeBaseSummary}`}
              onClick={() => onSelectKnowledgeBase(knowledgeBase.id)}
            >
              {knowledgeBase.status === "error" ? <AlertCircle size={15} /> : <Database size={15} />}
              <span className="kb-row-copy">
                <OverflowTooltipText as="strong" text={knowledgeBase.name} logArea="knowledge_base_row_name" />
                <OverflowTooltipText text={knowledgeBaseSummary} logArea="knowledge_base_row_summary" />
              </span>
            </button>
          );
        })}
        <button className="add-kb-button" type="button" onClick={onAddKnowledgeBase}>
          <Plus size={15} />
          连接资料库
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
          placeholder="过滤文件和文件夹"
          type="search"
        />
      </label>

      <div className="local-tree" aria-label="本地目录树">
        <div className="section-header">
          <p className="section-label">Files</p>
          <div className="section-actions">
            <span>{activeKnowledgeBase.status === "error" ? "目录失效" : "支持文档"}</span>
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
        <OverflowTooltipText as="p" className="root-path" text={activeKnowledgeBase.path} logArea="knowledge_base_root_path" />
        <ScanReportSummary knowledgeBase={activeKnowledgeBase} />
        <FileTree
          nodes={fileTree}
          activeNoteId={activeNoteId}
          activeDocumentId={activeDocumentId}
          collapsedFolderPaths={collapsedFolderPaths}
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
      已扫描 {report.scannedFileCount} 个支持文档{errorText}
      {skippedText}
    </p>
  );
}
