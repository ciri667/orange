import { convertFileSrc } from "@tauri-apps/api/core";
import { ChevronDown, Clock3, Columns2, Eye, FileDown, FilePenLine, PencilLine, Save, Tags, Trash2 } from "lucide-react";
import { useState } from "react";
import type { ClipboardEventHandler, RefObject, UIEventHandler } from "react";
import ReactMarkdown, { defaultUrlTransform } from "react-markdown";
import type { UrlTransform } from "react-markdown";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";
import type { Options as RehypeSanitizeOptions } from "rehype-sanitize";
import remarkGfm from "remark-gfm";
import { DiffPanel } from "../diff/DiffPanel";
import type { ExportFormat, KnowledgeBase, MarkdownViewMode, Note, ProposedChange } from "../shared/types";
import { useSyncedMarkdownScroll } from "./useSyncedMarkdownScroll";

/** 编辑器视图切换按钮配置，集中维护标签、图标和 aria 文案。 */
const MARKDOWN_VIEW_OPTIONS: Array<{ mode: MarkdownViewMode; label: string; title: string; icon: typeof PencilLine }> = [
  { mode: "edit", label: "编辑", title: "切换到编辑模式", icon: PencilLine },
  { mode: "preview", label: "预览", title: "切换到 Markdown 预览", icon: Eye },
  { mode: "split", label: "分屏", title: "切换到编辑和预览分屏", icon: Columns2 },
];

/** Markdown 文件支持的导出菜单项；PDF 由后端生成阅读版。 */
const MARKDOWN_EXPORT_OPTIONS: Array<{ format: ExportFormat; label: string }> = [
  { format: "original", label: "原文件 .md" },
  { format: "pdf", label: "转为 .pdf" },
];

/** 允许图片预览读取本地资源协议；链接等其他 URL 仍保留 rehype-sanitize 默认规则。 */
const MARKDOWN_PREVIEW_SANITIZE_SCHEMA: RehypeSanitizeOptions = {
  ...defaultSchema,
  protocols: {
    ...defaultSchema.protocols,
    // 图片 src 的协议由 urlTransform 做最终过滤，这样 Windows 盘符路径不会在转换前被 sanitizer 删除。
    src: [],
  },
};

/** Windows 盘符绝对路径识别，用于兼容跨平台 Markdown 图片引用。 */
const WINDOWS_ABSOLUTE_PATH_PATTERN = /^[a-zA-Z]:[\\/]/;

/** 格式化当前笔记的阅读统计，帮助用户快速判断内容长度。 */
function getReadingStats(content: string) {
  const words = content.replace(/\s+/g, "").length;
  const minutes = Math.max(1, Math.ceil(words / 450));

  return { words, minutes };
}

/** 从剪贴板中提取图片文件；非图片内容保持默认粘贴行为。 */
function getImageFilesFromClipboard(clipboardData: DataTransfer) {
  return Array.from(clipboardData.items)
    .filter((item) => item.kind === "file" && item.type.toLowerCase().startsWith("image/"))
    .map((item) => item.getAsFile())
    .filter((file): file is File => Boolean(file));
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
  onPasteImages,
  onExportFile,
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
  onPasteImages: (files: File[], selectionStart: number, selectionEnd: number) => void;
  onExportFile: (format: ExportFormat) => void | Promise<void>;
  onRenameNote: () => void;
  onDeleteNote: () => void;
  onAcceptChange: () => void;
  onRejectChange: () => void;
}) {
  /** 分屏模式下同步源码和预览滚动；非分屏时 hook 会保持静默。 */
  const { editorRef, previewRef, handleEditorScroll, handlePreviewScroll } = useSyncedMarkdownScroll(viewMode === "split");
  /** 导出菜单只在当前编辑器头部短暂展开，不写入全局工作台状态。 */
  const [isExportMenuOpen, setIsExportMenuOpen] = useState(false);

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
  /** 图片粘贴需要先落盘附件，再把返回的 Markdown 片段插入当前草稿。 */
  const handlePaste: ClipboardEventHandler<HTMLTextAreaElement> = (event) => {
    const imageFiles = getImageFilesFromClipboard(event.clipboardData);

    if (!imageFiles.length) {
      return;
    }

    event.preventDefault();
    onPasteImages(imageFiles, event.currentTarget.selectionStart, event.currentTarget.selectionEnd);
  };

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
          <div className="export-menu-wrapper">
            <button
              className="text-button"
              type="button"
              title="导出当前 Markdown"
              aria-haspopup="menu"
              aria-expanded={isExportMenuOpen}
              onClick={() => setIsExportMenuOpen((isOpen) => !isOpen)}
              disabled={isBusy}
            >
              <FileDown size={16} />
              导出
              <ChevronDown size={14} />
            </button>
            {isExportMenuOpen && (
              <div className="export-action-menu" role="menu">
                {MARKDOWN_EXPORT_OPTIONS.map((option) => (
                  <button
                    key={option.format}
                    type="button"
                    role="menuitem"
                    onClick={() => {
                      setIsExportMenuOpen(false);
                      void onExportFile(option.format);
                    }}
                  >
                    <FileDown size={14} />
                    {option.label}
                  </button>
                ))}
              </div>
            )}
          </div>
          <button className="text-button" type="button" title="重命名当前笔记" onClick={onRenameNote} disabled={isBusy}>
            <FilePenLine size={18} />
            重命名
          </button>
          <button className="text-button danger" type="button" title="删除当前笔记" onClick={onDeleteNote} disabled={isBusy}>
            <Trash2 size={18} />
            删除
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
            onPaste={handlePaste}
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
          <MarkdownPreview
            content={note.content}
            knowledgeBase={knowledgeBase}
            note={note}
            previewRef={previewRef}
            onScroll={handlePreviewScroll}
          />
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
  knowledgeBase,
  note,
  previewRef,
  onScroll,
}: {
  content: string;
  knowledgeBase: KnowledgeBase;
  note: Note;
  previewRef: RefObject<HTMLDivElement | null>;
  onScroll: UIEventHandler<HTMLDivElement>;
}) {
  /** 当前笔记上下文决定相对图片从哪个本地目录解析。 */
  const imageUrlTransform = createMarkdownPreviewUrlTransform(knowledgeBase, note);

  return (
    <div className="markdown-preview" ref={previewRef} onScroll={onScroll} aria-label="Markdown 预览">
      {content.trim() ? (
        <ReactMarkdown
          remarkPlugins={[remarkGfm]}
          rehypePlugins={[[rehypeSanitize, MARKDOWN_PREVIEW_SANITIZE_SCHEMA]]}
          urlTransform={imageUrlTransform}
        >
          {content}
        </ReactMarkdown>
      ) : (
        <p className="markdown-preview-empty">空白笔记</p>
      )}
    </div>
  );
}

/** 创建 Markdown URL 转换器，只把图片 src 的本地路径改写为 Tauri asset URL。 */
function createMarkdownPreviewUrlTransform(knowledgeBase: KnowledgeBase, note: Note): UrlTransform {
  return (url, key, node) => {
    if (key !== "src" || node.tagName !== "img") {
      return defaultUrlTransform(url);
    }

    return transformMarkdownImageSource(url, knowledgeBase, note);
  };
}

/** 将 Markdown 图片路径解析为浏览器可加载的安全 URL。 */
function transformMarkdownImageSource(source: string, knowledgeBase: KnowledgeBase, note: Note) {
  const trimmedSource = source.trim();

  if (!trimmedSource) {
    return "";
  }

  const protocol = getUrlProtocol(trimmedSource);

  if (protocol === "http" || protocol === "https" || trimmedSource.startsWith("//")) {
    return defaultUrlTransform(trimmedSource);
  }

  if (protocol && protocol !== "file" && !WINDOWS_ABSOLUTE_PATH_PATTERN.test(trimmedSource)) {
    // 未知协议保持 react-markdown 默认安全过滤，避免 javascript: 等危险 src。
    return defaultUrlTransform(trimmedSource);
  }

  if (!isTauriAssetRuntime()) {
    return defaultUrlTransform(trimmedSource);
  }

  const localImagePath = resolveLocalMarkdownImagePath(trimmedSource, knowledgeBase, note);

  if (!localImagePath) {
    return defaultUrlTransform(trimmedSource);
  }

  return `${convertFileSrc(localImagePath.path)}${localImagePath.suffix}`;
}

/** 判断当前是否具备 Tauri asset 协议转换能力。 */
function isTauriAssetRuntime() {
  if (typeof window === "undefined") {
    return false;
  }

  const tauriInternals = window.__TAURI_INTERNALS__;

  return typeof tauriInternals === "object" && tauriInternals !== null && "convertFileSrc" in tauriInternals;
}

/** 提取 URL 协议；相对路径、绝对文件路径和查询片段中的冒号不视为协议。 */
function getUrlProtocol(source: string) {
  const colonIndex = source.indexOf(":");
  const questionIndex = source.indexOf("?");
  const hashIndex = source.indexOf("#");
  const slashIndex = source.indexOf("/");

  if (
    colonIndex < 0 ||
    (slashIndex >= 0 && colonIndex > slashIndex) ||
    (questionIndex >= 0 && colonIndex > questionIndex) ||
    (hashIndex >= 0 && colonIndex > hashIndex)
  ) {
    return "";
  }

  return source.slice(0, colonIndex).toLowerCase();
}

/** 把 Markdown 图片引用解析为本地文件路径，并保留查询和 hash 后缀。 */
function resolveLocalMarkdownImagePath(source: string, knowledgeBase: KnowledgeBase, note: Note) {
  if (source.toLowerCase().startsWith("file:")) {
    return parseFileUrl(source);
  }

  const { path, suffix } = splitLocalPathSuffix(source);
  const decodedPath = decodeLocalPath(path);

  if (!decodedPath) {
    return null;
  }

  if (isAbsoluteLocalPath(decodedPath)) {
    return { path: normalizeLocalFilePath(decodedPath), suffix };
  }

  // 相对图片以当前 Markdown 文件所在目录为基准，而不是以 Vite/Tauri 应用地址为基准。
  const noteDirectory = getDirectoryPath(note.path);
  const resolvedPath = joinLocalFilePath(knowledgeBase.path, noteDirectory, decodedPath);

  return { path: resolvedPath, suffix };
}

/** 从 file:// URL 提取系统路径，兼容带空格或中文的本地文件名。 */
function parseFileUrl(source: string) {
  try {
    const fileUrl = new URL(source);
    const suffix = `${fileUrl.search}${fileUrl.hash}`;
    const decodedPath = decodeURIComponent(fileUrl.pathname);
    const platformPath = decodedPath.match(/^\/[a-zA-Z]:\//) ? decodedPath.slice(1) : decodedPath;

    if (!platformPath) {
      return null;
    }

    return { path: normalizeLocalFilePath(platformPath), suffix };
  } catch {
    const withoutProtocol = source.replace(/^file:\/\//i, "");
    const { path, suffix } = splitLocalPathSuffix(withoutProtocol);

    return { path: normalizeLocalFilePath(decodeLocalPath(path)), suffix };
  }
}

/** 将本地路径主体与查询/hash 拆开，避免把 cache busting 参数当作文件名。 */
function splitLocalPathSuffix(source: string) {
  const suffixIndex = source.search(/[?#]/);

  if (suffixIndex < 0) {
    return { path: source, suffix: "" };
  }

  return {
    path: source.slice(0, suffixIndex),
    suffix: source.slice(suffixIndex),
  };
}

/** 解码 Markdown URL 中的转义字符；非法转义时保留原值，避免预览整体失败。 */
function decodeLocalPath(path: string) {
  try {
    return decodeURI(path);
  } catch {
    return path;
  }
}

/** 判断路径是否已经是系统绝对路径。 */
function isAbsoluteLocalPath(path: string) {
  return path.startsWith("/") || WINDOWS_ABSOLUTE_PATH_PATTERN.test(path);
}

/** 获取文件路径的父目录，用于 Markdown 相对图片定位。 */
function getDirectoryPath(filePath: string) {
  const normalizedPath = filePath.replace(/\\/g, "/");
  const separatorIndex = normalizedPath.lastIndexOf("/");

  return separatorIndex >= 0 ? normalizedPath.slice(0, separatorIndex) : "";
}

/** 拼接本地路径片段，并清理 . 与 ..，避免生成浏览器无法识别的 asset 路径。 */
function joinLocalFilePath(...segments: string[]) {
  return normalizeLocalFilePath(segments.filter(Boolean).join("/"));
}

/** 归一化本地路径分隔符和相对段，保留 POSIX 根路径与 Windows 盘符。 */
function normalizeLocalFilePath(path: string) {
  const normalizedPath = path.replace(/\\/g, "/");
  const driveMatch = normalizedPath.match(/^([a-zA-Z]:)(\/|$)/);
  const prefix = driveMatch ? `${driveMatch[1]}/` : normalizedPath.startsWith("/") ? "/" : "";
  const pathWithoutPrefix = driveMatch
    ? normalizedPath.slice(prefix.length)
    : prefix
      ? normalizedPath.slice(prefix.length)
      : normalizedPath;
  const normalizedSegments: string[] = [];

  for (const segment of pathWithoutPrefix.split("/")) {
    if (!segment || segment === ".") {
      continue;
    }

    if (segment === "..") {
      // 有根路径时不允许 .. 穿出根；相对路径则保留前导 ..，交给调用方决定是否允许。
      if (normalizedSegments.length && normalizedSegments[normalizedSegments.length - 1] !== "..") {
        normalizedSegments.pop();
      } else if (!prefix) {
        normalizedSegments.push(segment);
      }
      continue;
    }

    normalizedSegments.push(segment);
  }

  return `${prefix}${normalizedSegments.join("/")}`;
}
