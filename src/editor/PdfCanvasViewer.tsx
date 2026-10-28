import type { PDFDocumentProxy, RenderTask } from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import { ChevronLeft, ChevronRight, Search } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { Button } from "../shared/Button";
import type { DocumentPreviewBlock } from "../shared/types";

// pdf.js 只在真正打开 PDF 时加载，避免把 1 MB+ 的解析器放进首屏包体。
type PdfModule = typeof import("pdfjs-dist");
let pdfModulePromise: Promise<PdfModule> | null = null;

function loadPdfModule(): Promise<PdfModule> {
  if (!pdfModulePromise) {
    pdfModulePromise = import("pdfjs-dist").then((module) => {
      // worker 交给 Vite 打包成独立资源，避免运行时去 CDN 取 worker。
      module.GlobalWorkerOptions.workerSrc = workerUrl;
      return module;
    });
  }
  return pdfModulePromise;
}

/** 单页画布渲染的 PDF 阅读器：页码导航、页内文本搜索、引用跳页。 */
export function PdfCanvasViewer({
  src,
  page,
  pageCount,
  blocks,
}: {
  src: string;
  /** 外部（引用）要求跳转的页码，1-based。 */
  page?: number;
  /** 后端已知页数；pdf.js 加载完成前先用它显示总数。 */
  pageCount?: number;
  /** 后端文本层，用于搜索定位（不依赖 pdf.js 文本层）。 */
  blocks?: DocumentPreviewBlock[];
}) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const renderTaskRef = useRef<RenderTask | null>(null);
  const [document, setDocument] = useState<PDFDocumentProxy | null>(null);
  const [currentPage, setCurrentPage] = useState(page && page > 0 ? page : 1);
  const [totalPages, setTotalPages] = useState(pageCount ?? 0);
  const [status, setStatus] = useState<"loading" | "ready" | "error">("loading");
  const [errorMessage, setErrorMessage] = useState("");
  const [query, setQuery] = useState("");
  const [pageInput, setPageInput] = useState(String(page ?? 1));

  useEffect(() => {
    let cancelled = false;
    let destroy: (() => void) | null = null;

    void loadPdfModule()
      .then((pdfjs) => {
        if (cancelled) {
          return null;
        }
        const loadingTask = pdfjs.getDocument({ url: src, useSystemFonts: true });
        destroy = () => void loadingTask.destroy();
        return loadingTask.promise;
      })
      .then((loaded) => {
        if (!loaded) {
          return;
        }
        if (cancelled) {
          void loaded.destroy();
          return;
        }
        setDocument(loaded);
        setTotalPages(loaded.numPages);
        setStatus("ready");
      })
      .catch((error: unknown) => {
        if (cancelled) {
          return;
        }
        setStatus("error");
        setErrorMessage(error instanceof Error ? error.message : "PDF 加载失败。");
      });

    return () => {
      cancelled = true;
      destroy?.();
    };
  }, [src]);

  useEffect(() => {
    if (page && page > 0) {
      setCurrentPage(page);
      setPageInput(String(page));
    }
  }, [page]);

  useEffect(() => {
    if (status !== "ready" || !document) {
      return;
    }
    let cancelled = false;

    async function renderPage() {
      const canvas = canvasRef.current;
      if (!canvas) {
        return;
      }
      // 同一 canvas 不能并发渲染：先取消上一页的渲染任务。
      renderTaskRef.current?.cancel();
      renderTaskRef.current = null;

      const pdfPage = await document!.getPage(currentPage);
      if (cancelled) {
        return;
      }
      const containerWidth = canvas.parentElement?.clientWidth ?? 900;
      const baseViewport = pdfPage.getViewport({ scale: 1 });
      const scale = Math.min(2.5, Math.max(0.6, containerWidth / baseViewport.width));
      const viewport = pdfPage.getViewport({ scale });
      const ratio = window.devicePixelRatio || 1;
      canvas.width = Math.floor(viewport.width * ratio);
      canvas.height = Math.floor(viewport.height * ratio);
      canvas.style.width = `${Math.floor(viewport.width)}px`;
      canvas.style.height = `${Math.floor(viewport.height)}px`;

      const context = canvas.getContext("2d");
      if (!context) {
        return;
      }
      context.setTransform(ratio, 0, 0, ratio, 0, 0);
      const task = pdfPage.render({ canvasContext: context, viewport });
      renderTaskRef.current = task;
      try {
        await task.promise;
      } catch (error) {
        // cancel() 会以 RenderingCancelledException 结束，属于正常流程。
        if (!(error instanceof Error) || !error.message.includes("cancelled")) {
          setStatus("error");
          setErrorMessage(error instanceof Error ? error.message : "PDF 页面渲染失败。");
        }
      }
    }

    void renderPage();
    return () => {
      cancelled = true;
      renderTaskRef.current?.cancel();
    };
  }, [currentPage, document, status]);

  /** 搜索走后端文本层，命中后直接跳页，避免额外依赖 pdf.js 文本层。 */
  const searchHits = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) {
      return [] as number[];
    }
    return (blocks ?? [])
      .filter((block) => block.page && block.text.toLowerCase().includes(needle))
      .map((block) => block.page as number);
  }, [blocks, query]);

  function goToPage(nextPage: number) {
    const bounded = Math.min(Math.max(1, nextPage), Math.max(1, totalPages));
    setCurrentPage(bounded);
    setPageInput(String(bounded));
  }

  if (status === "error") {
    return (
      <div className="grid min-h-0 min-w-0 place-items-center rounded-panel border border-dashed border-border-strong bg-surface p-6 text-center text-[13px] leading-[1.55] text-ink-muted" role="alert">
        PDF 阅读器加载失败：{errorMessage}
      </div>
    );
  }

  return (
    <div className="grid min-h-0 min-w-0 grid-rows-[auto_minmax(0,1fr)] gap-2" aria-label="PDF 阅读器">
      <div className="flex flex-wrap items-center gap-2 px-1" role="toolbar" aria-label="PDF 页码与搜索">
        <Button
          size="compact"
          variant="icon"
          aria-label="上一页"
          title="上一页"
          disabled={currentPage <= 1}
          onClick={() => goToPage(currentPage - 1)}
        >
          <ChevronLeft size={16} />
        </Button>
        <input
          aria-label="页码"
          className="w-14 rounded-md border border-border bg-surface px-2 py-1 text-center text-[12px] text-ink"
          inputMode="numeric"
          value={pageInput}
          onChange={(event) => setPageInput(event.target.value.replace(/[^\d]/g, ""))}
          onBlur={() => {
            const next = Number(pageInput);
            if (Number.isFinite(next) && next > 0) {
              goToPage(next);
            } else {
              setPageInput(String(currentPage));
            }
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              const next = Number(pageInput);
              if (Number.isFinite(next) && next > 0) {
                goToPage(next);
              }
            }
          }}
        />
        <span className="text-[12px] text-ink-soft">/ {totalPages || "?"} 页</span>
        <Button
          size="compact"
          variant="icon"
          aria-label="下一页"
          title="下一页"
          disabled={totalPages > 0 && currentPage >= totalPages}
          onClick={() => goToPage(currentPage + 1)}
        >
          <ChevronRight size={16} />
        </Button>

        <label className="ml-2 flex items-center gap-1 rounded-md border border-border bg-surface px-2 py-1">
          <Search size={13} className="text-ink-soft" />
          <input
            aria-label="在 PDF 文本层中搜索"
            className="w-36 bg-transparent text-[12px] text-ink outline-none"
            placeholder="搜索文本并跳页"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
        {query.trim() ? (
          <span className="text-[11px] text-ink-soft">
            {searchHits.length > 0 ? `${searchHits.length} 处命中` : "没有命中"}
            {searchHits.length > 0 ? (
              <button
                className="ml-2 underline decoration-dotted"
                type="button"
                onClick={() => goToPage(searchHits[0])}
              >
                跳到第 {searchHits[0]} 页
              </button>
            ) : null}
          </span>
        ) : null}
      </div>

      <div className="min-h-0 min-w-0 overflow-auto rounded-none border-0 bg-surface-muted p-3">
        {status === "loading" ? <div className="text-[13px] text-ink-muted">正在加载 PDF...</div> : null}
        <canvas ref={canvasRef} className="mx-auto block shadow-sm" />
      </div>
    </div>
  );
}