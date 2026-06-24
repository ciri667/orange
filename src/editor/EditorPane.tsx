import { Clock3, Columns2, Eye, FilePenLine, PencilLine, Save, Tags, Trash2, Wand2 } from "lucide-react";
import type { RefObject, UIEventHandler } from "react";
import ReactMarkdown from "react-markdown";
import rehypeSanitize from "rehype-sanitize";
import remarkGfm from "remark-gfm";
import { DiffPanel } from "../diff/DiffPanel";
import type { KnowledgeBase, MarkdownViewMode, Note, ProposedChange } from "../shared/types";
import { useSyncedMarkdownScroll } from "./useSyncedMarkdownScroll";

/** 编辑器视图切换按钮配置，集中维护标签、图标和 aria 文案。 */
const MARKDOWN_VIEW_OPTIONS: Array<{ mode: MarkdownViewMode; label: string; title: string; icon: typeof PencilLine }> = [
  { mode: "edit", label: "编辑", title: "切换到编辑模式", icon: PencilLine },
  { mode: "preview", label: "预览", title: "切换到 Markdown 预览", icon: Eye },
  { mode: "split", label: "分屏", title: "切换到编辑和预览分屏", icon: Columns2 },
];

/** 格式化当前笔记的阅读统计，帮助用户快速判断内容长度。 */
function getReadingStats(content: string) {
  const words = content.replace(/\s+/g, "").length;
  const minutes = Math.max(1, Math.ceil(words / 450));

  return { words, minutes };
}

/** 中间 Markdown 编辑器，展示当前笔记内容和 Agent 写入确认入口。 */
export function EditorPane({
  note,
  knowledgeBase,
  proposedChange,
  isBusy,
  isDirty,
  viewMode,
  onViewModeChange,
  onSaveNote,
  onContentChange,
  onRequestRewrite,
  onRenameNote,
  onDeleteNote,
  onAcceptChange,
  onRejectChange,
}: {
  note?: Note;
  knowledgeBase: KnowledgeBase;
  proposedChange?: ProposedChange;
  isBusy: boolean;
  isDirty: boolean;
  viewMode: MarkdownViewMode;
  onViewModeChange: (mode: MarkdownViewMode) => void;
  onSaveNote: () => void;
  onContentChange: (content: string) => void;
  onRequestRewrite: () => void;
  onRenameNote: () => void;
  onDeleteNote: () => void;
  onAcceptChange: () => void;
  onRejectChange: () => void;
}) {
  /** 分屏模式下同步源码和预览滚动；非分屏时 hook 会保持静默。 */
  const { editorRef, previewRef, handleEditorScroll, handlePreviewScroll } = useSyncedMarkdownScroll(viewMode === "split");

  if (!note) {
    return (
      <section className="editor-pane" aria-label="Markdown 编辑器">
        <header className="editor-header">
          <div>
            <p className="path-label">{knowledgeBase.name}</p>
            <h2>暂无 Markdown 笔记</h2>
          </div>
          <div className="editor-actions" />
        </header>
        <div className="editor-empty-state">
          {knowledgeBase.status === "error" ? (
            <>
              <strong>当前知识库目录暂不可访问。</strong>
              <span>{knowledgeBase.description}</span>
            </>
          ) : (
            <>
              <strong>当前知识库还没有可编辑的 Markdown 文件。</strong>
              <span>请在左侧目录树的根目录或任意文件夹中使用新建入口创建文档，或在本地目录中添加文件后重新扫描。</span>
            </>
          )}
        </div>
      </section>
    );
  }

  const stats = getReadingStats(note.content);
  const shouldShowEditor = viewMode === "edit" || viewMode === "split";
  const shouldShowPreview = viewMode === "preview" || viewMode === "split";

  return (
    <section className="editor-pane" aria-label="Markdown 编辑器">
      <header className="editor-header">
        <div>
          <p className="path-label">
            {knowledgeBase.name} / {note.path}
          </p>
          <h2>{note.title}</h2>
        </div>
        <div className="editor-actions">
          <div className="view-mode-toggle" aria-label="Markdown 视图模式">
            {MARKDOWN_VIEW_OPTIONS.map(({ mode, label, title, icon: Icon }) => (
              <button
                className={viewMode === mode ? "active" : ""}
                key={mode}
                type="button"
                title={title}
                onClick={() => onViewModeChange(mode)}
              >
                <Icon size={15} />
                <span>{label}</span>
              </button>
            ))}
          </div>
          <button className="text-button" type="button" onClick={onSaveNote} disabled={isBusy || !isDirty}>
            <Save size={16} />
            {isDirty ? "保存草稿" : "已保存"}
          </button>
          <button className="text-button" type="button" title="重命名当前笔记" onClick={onRenameNote} disabled={isBusy}>
            <FilePenLine size={18} />
            重命名
          </button>
          <button className="text-button danger" type="button" title="删除当前笔记" onClick={onDeleteNote} disabled={isBusy}>
            <Trash2 size={18} />
            删除
          </button>
          <button className="text-button" type="button" onClick={onRequestRewrite} disabled={isBusy}>
            <Wand2 size={16} />
            改写段落
          </button>
        </div>
      </header>

      <div className="meta-strip">
        <span>
          <Clock3 size={14} />
          {note.updatedAt}
        </span>
        <span>
          <PencilLine size={14} />
          {stats.words} 字，约 {stats.minutes} 分钟
        </span>
        <span>
          <Tags size={14} />
          {note.tags.length ? note.tags.join(" / ") : "无标签"}
        </span>
        <span className={isDirty ? "dirty-indicator" : ""}>
          <Save size={14} />
          {isDirty ? "未保存草稿" : "已保存到本地"}
        </span>
      </div>

      <div className={`editor-body mode-${viewMode}`}>
        {shouldShowEditor && (
          <textarea
            className="markdown-editor"
            ref={editorRef}
            value={note.content}
            onChange={(event) => onContentChange(event.target.value)}
            onKeyDown={(event) => {
              // 拦截系统保存快捷键，确保桌面端写入统一经过 Tauri hash 和路径校验。
              if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
                event.preventDefault();
                onSaveNote();
              }
            }}
            onScroll={handleEditorScroll}
            spellCheck={false}
            aria-label="当前 Markdown 笔记内容"
          />
        )}
        {shouldShowPreview && (
          <MarkdownPreview content={note.content} previewRef={previewRef} onScroll={handlePreviewScroll} />
        )}
      </div>

      {proposedChange?.status === "pending" && (
        <DiffPanel change={proposedChange} onAccept={onAcceptChange} onReject={onRejectChange} isBusy={isBusy} />
      )}
    </section>
  );
}

/** 安全的 GFM Markdown 预览，渲染内存草稿并通过 rehype-sanitize 禁用危险 HTML。 */
function MarkdownPreview({
  content,
  previewRef,
  onScroll,
}: {
  content: string;
  previewRef: RefObject<HTMLDivElement | null>;
  onScroll: UIEventHandler<HTMLDivElement>;
}) {
  return (
    <div className="markdown-preview" ref={previewRef} onScroll={onScroll} aria-label="Markdown 预览">
      {content.trim() ? (
        <ReactMarkdown remarkPlugins={[remarkGfm]} rehypePlugins={[rehypeSanitize]}>
          {content}
        </ReactMarkdown>
      ) : (
        <p className="markdown-preview-empty">空白笔记</p>
      )}
    </div>
  );
}
