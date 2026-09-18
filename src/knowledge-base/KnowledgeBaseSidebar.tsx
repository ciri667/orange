import { AlertCircle, Database, MessageSquarePlus, Plus, RefreshCw, Search, Settings } from "lucide-react";
import { FileTree } from "./FileTree";
import { Button } from "../shared/Button";
import { cn } from "../shared/cn";
import { ListRow } from "../shared/ListRow";
import { OperationNotice } from "../shared/OperationNotice";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import { sectionLabelClassName } from "../shared/ui";
import type { FileTreeNode, KnowledgeBase } from "../shared/types";

/** 生成单个资料库文件数量摘要，用于 tooltip，不占侧栏两行。 */
function getKnowledgeBaseFileSummary(knowledgeBase: KnowledgeBase) {
  const fileCount = knowledgeBase.noteCount + knowledgeBase.documentCount;

  return `${fileCount} 个文件 · ${knowledgeBase.noteCount} 个 Markdown`;
}

/** 左侧知识库导航，包含品牌、新对话、知识库切换、搜索和本地目录树。 */
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
  onCreateProjectInstruction,
  onRefreshKnowledgeBase,
  onCreateSession,
  onOpenSettings,
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
  onCreateProjectInstruction: () => void;
  onRefreshKnowledgeBase: (knowledgeBaseId: string) => void;
  onCreateSession: () => void;
  onOpenSettings: () => void;
}) {
  return (
    <aside className="sidebar" aria-label="知识库导航">
      <div className="flex items-center gap-2 px-2 py-1">
        <div className="grid size-7 place-items-center overflow-hidden rounded-md">
          <img className="block size-full object-contain" src="/orange-logo.svg" alt="" />
        </div>
        <strong className="text-[15px] font-semibold text-ink-strong">橘记</strong>
      </div>

      <Button variant="ghost" className="w-full justify-start gap-2 px-2.5 text-[13px]" onClick={onCreateSession}>
        <MessageSquarePlus size={16} />
        新对话
      </Button>

      <section className="grid gap-0.5" aria-label="知识库切换">
        <p className={sectionLabelClassName}>知识库</p>
        {knowledgeBases.map((knowledgeBase) => {
          const knowledgeBaseSummary = `${getKnowledgeBaseFileSummary(knowledgeBase)} · ${getKnowledgeBaseStatusLabel(knowledgeBase)}`;

          return (
            <ListRow
              key={knowledgeBase.id}
              active={knowledgeBase.id === activeKnowledgeBase.id}
              error={knowledgeBase.status === "error"}
              className="py-1.5"
              aria-label={`${knowledgeBase.name}，${knowledgeBaseSummary}`}
              title={knowledgeBaseSummary}
              onClick={() => onSelectKnowledgeBase(knowledgeBase.id)}
            >
              {knowledgeBase.status === "error" ? <AlertCircle size={15} /> : <Database size={15} />}
              <OverflowTooltipText as="span" className="min-w-0 truncate text-[13px]" text={knowledgeBase.name} logArea="knowledge_base_row_name" />
            </ListRow>
          );
        })}
        <Button variant="ghost" className="w-full justify-start px-2.5 text-[13px] text-ink-muted" onClick={onAddKnowledgeBase}>
          <Plus size={15} />
          连接资料库
        </Button>
      </section>

      <OperationNotice isBusy={isBusy} busyLabel={busyLabel} notice={notice} />

      <label className="mx-1 flex min-h-8 items-center gap-2 rounded-control bg-white/70 px-2 text-ink-muted">
        <Search size={14} />
        <input
          className="min-w-0 w-full border-0 bg-transparent text-[13px] outline-0"
          value={searchTerm}
          onChange={(event) => onSearchChange(event.target.value)}
          placeholder="过滤文件"
          type="search"
        />
      </label>

      <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-auto pt-1 pr-0.5" aria-label="本地目录树">
        <div className="flex items-center justify-between gap-2 px-1">
          <p className={sectionLabelClassName}>文件</p>
          <Button
            variant="icon"
            size="compact"
            title="手动刷新目录树"
            onClick={() => onRefreshKnowledgeBase(activeKnowledgeBase.id)}
            disabled={isBusy}
          >
            <RefreshCw size={13} />
          </Button>
        </div>
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
          onCreateProjectInstruction={onCreateProjectInstruction}
        />
      </div>

      <Button variant="ghost" className="mt-auto w-full justify-start px-2.5 text-[13px] text-ink-muted" onClick={onOpenSettings}>
        <Settings size={16} />
        设置
      </Button>
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
    return <p className="mb-1.5 px-2.5 text-xs leading-normal text-danger">{knowledgeBase.description}</p>;
  }

  if (!report) {
    return null;
  }

  const skippedText = report.skippedDirectories.length ? `，跳过 ${report.skippedDirectories.length} 个目录` : "";
  const errorText = report.failedFileCount ? `，${report.failedFileCount} 个读取失败` : "";

  return (
    <p className={cn("mb-1.5 px-2.5 text-[11px] leading-normal text-ink-soft", report.failedFileCount && "text-warning")}>
      已扫描 {report.scannedFileCount} 个文档{errorText}
      {skippedText}
    </p>
  );
}
