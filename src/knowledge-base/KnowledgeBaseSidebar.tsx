import { AlertCircle, BookOpen, Database, Loader2, Plus, RefreshCw, Search } from "lucide-react";
import { FileTree } from "./FileTree";
import { Button } from "../shared/Button";
import { ListRow } from "../shared/ListRow";
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
  onOpenNoteHistory,
  onRenameDocument,
  onDeleteDocument,
  onOpenDocumentHistory,
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
  onOpenNoteHistory: (noteId: string) => void;
  onRenameDocument: (documentId: string) => void;
  onDeleteDocument: (documentId: string) => void;
  onOpenDocumentHistory: (documentId: string) => void;
  onCreateMarkdown: (parentPath: string) => void;
  onCreateText: (parentPath: string) => void;
  onCreateFolder: (parentPath: string) => void;
  onRefreshKnowledgeBase: (knowledgeBaseId: string) => void;
}) {
  const assetCount = getKnowledgeBaseAssetCount(knowledgeBases);

  return (
    <aside className="sidebar" aria-label="知识库导航">
      <div className="flex items-center gap-2.5 px-0.5 pb-1">
        <div className="grid size-8 place-items-center rounded-control bg-accent-soft text-accent-strong">
          <BookOpen size={18} />
        </div>
        <div className="min-w-0">
          <strong className="block text-ink-strong">资料库</strong>
          <span className="block truncate text-xs text-ink-muted">
            {knowledgeBases.length} 个本地库 · {assetCount} 个文件
          </span>
        </div>
      </div>

      <section className="grid gap-[7px]" aria-label="知识库切换">
        <div className="flex items-center justify-between gap-3">
          <p className="section-label">Library</p>
          <span className="text-xs text-ink-muted">本地优先</span>
        </div>
        {knowledgeBases.map((knowledgeBase) => {
          const knowledgeBaseSummary = `${getKnowledgeBaseFileSummary(knowledgeBase)} · ${getKnowledgeBaseStatusLabel(knowledgeBase)}`;

          return (
            <ListRow
              key={knowledgeBase.id}
              active={knowledgeBase.id === activeKnowledgeBase.id}
              error={knowledgeBase.status === "error"}
              aria-label={`${knowledgeBase.name}，${knowledgeBaseSummary}`}
              onClick={() => onSelectKnowledgeBase(knowledgeBase.id)}
            >
              {knowledgeBase.status === "error" ? <AlertCircle size={15} /> : <Database size={15} />}
              <span className="min-w-0">
                <OverflowTooltipText as="strong" className="block truncate text-ink-strong" text={knowledgeBase.name} logArea="knowledge_base_row_name" />
                <OverflowTooltipText className="mt-[3px] block truncate text-xs text-ink-muted" text={knowledgeBaseSummary} logArea="knowledge_base_row_summary" />
              </span>
            </ListRow>
          );
        })}
        <Button variant="ghost" className="w-full" onClick={onAddKnowledgeBase}>
          <Plus size={15} />
          连接资料库
        </Button>
      </section>

      {(isBusy || notice) && (
        <div className={`operation-notice ${notice.includes("失败") || notice.includes("阻止") ? "error" : ""}`}>
          {isBusy && <Loader2 size={14} />}
          <span>{busyLabel || notice}</span>
        </div>
      )}

      <label className="flex min-h-[38px] items-center gap-2 rounded-control border border-border-translucent bg-surface-translucent px-2.5 text-ink-muted">
        <Search size={16} />
        <input
          className="min-w-0 w-full border-0 bg-transparent outline-0"
          value={searchTerm}
          onChange={(event) => onSearchChange(event.target.value)}
          placeholder="过滤文件和文件夹"
          type="search"
        />
      </label>

      <div className="local-tree" aria-label="本地目录树">
        <div className="flex items-center justify-between gap-3">
          <p className="section-label">Files</p>
          <div className="inline-flex items-center gap-2">
            <span className="text-xs text-ink-muted">{activeKnowledgeBase.status === "error" ? "目录失效" : "支持文档"}</span>
            <Button
              variant="ghost"
              size="compact"
              title="手动刷新目录树"
              onClick={() => onRefreshKnowledgeBase(activeKnowledgeBase.id)}
              disabled={isBusy}
            >
              <RefreshCw size={13} />
              刷新
            </Button>
          </div>
        </div>
        <OverflowTooltipText as="p" className="root-path" text={activeKnowledgeBase.path} logArea="knowledge_base_root_path" />
        <ScanReportSummary knowledgeBase={activeKnowledgeBase} />
        <FileTree
          nodes={fileTree}
          activeNoteId={activeNoteId}
          activeDocumentId={activeDocumentId}
          collapsedFolderPaths={collapsedFolderPaths}
          isFiltered={Boolean(searchTerm.trim())}
          onToggleFolder={onToggleFolder}
          onSelectNote={onSelectNote}
          onSelectDocument={onSelectDocument}
          onRenameNote={onRenameNote}
          onDeleteNote={onDeleteNote}
          onOpenNoteHistory={onOpenNoteHistory}
          onRenameDocument={onRenameDocument}
          onDeleteDocument={onDeleteDocument}
          onOpenDocumentHistory={onOpenDocumentHistory}
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
