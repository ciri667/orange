import { renderAsync } from "docx-preview";
import { useEffect, useRef, useState } from "react";

/** 内容变化后重新读取同一路径时加一个版本参数，避免命中旧的 asset 缓存；data/blob 不能附加查询串。 */
function withCacheKey(src: string, renderKey?: string) {
  if (!renderKey || /^(data:|blob:)/i.test(src)) {
    return src;
  }

  return `${src}${src.includes("?") ? "&" : "?"}v=${encodeURIComponent(renderKey)}`;
}

/**
 * 用 docx-preview 渲染 DOCX 的版式预览（表格、图片、页眉页脚、分页）。
 *
 * 只从本地 asset URL 读取一次字节，不加载任何外部资源；渲染失败时把错误
 * 交给父组件决定回退到结构块文本视图，而不是显示空白。
 *
 * docx-preview 的 renderAsync 会先清空容器再写入，所以「重新渲染」等于把整篇
 * 文档拆掉重建：画面闪一下，滚动位置也会被弹回顶部。为了让父组件的高频重渲染
 * （Agent 轮询、快照提交、进度事件）不再造成重建，这里做了两件事：
 * 1. onError 放进 ref，不参与渲染 effect 的依赖（父组件每次渲染都会传入新函数）；
 * 2. 记录已成功渲染的内容键，同一份内容只解析一次，直接复用现有 DOM。
 */
export function DocxLayoutPreview({
  src,
  renderKey,
  title,
  onError,
}: {
  src: string;
  /** 文件内容标识（文档 contentHash）；变化时才重新解析，同内容不会重建 DOM。 */
  renderKey?: string;
  title: string;
  onError: (message: string) => void;
}) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const onErrorRef = useRef(onError);
  /** 已成功写入 DOM 的内容键；用于挡掉重复解析。 */
  const renderedKeyRef = useRef("");
  const [isRendering, setIsRendering] = useState(true);

  // 只同步最新回调，不触发重新解析。
  useEffect(() => {
    onErrorRef.current = onError;
  }, [onError]);

  useEffect(() => {
    const container = containerRef.current;

    if (!src || !container) {
      return;
    }

    const key = renderKey ? `${src}::${renderKey}` : src;

    // 同一份内容已经渲染完成：不再解析，DOM 和滚动位置原样保留。
    if (renderedKeyRef.current === key && container.childElementCount > 0) {
      setIsRendering(false);
      return;
    }

    let cancelled = false;
    setIsRendering(true);

    // 只有真正换了内容才需要回到顶部（重建后旧位置在新文档里没有意义）。
    if (scrollRef.current) {
      scrollRef.current.scrollTop = 0;
    }

    async function render() {
      try {
        const response = await fetch(withCacheKey(src, renderKey));
        if (!response.ok) {
          throw new Error(`读取文件失败（${response.status}）`);
        }
        const buffer = await response.arrayBuffer();
        if (cancelled || !containerRef.current) {
          return;
        }
        await renderAsync(buffer, containerRef.current, undefined, {
          className: "docx-layout",
          inWrapper: true,
          breakPages: true,
          ignoreWidth: false,
          ignoreHeight: true,
          ignoreFonts: false,
          renderHeaders: true,
          renderFooters: true,
          renderFootnotes: true,
          renderEndnotes: true,
          // 图片内联成 data URL，避免生成外部 blob 引用或额外网络请求。
          useBase64URL: true,
        });
        if (cancelled) {
          return;
        }
        renderedKeyRef.current = key;
        setIsRendering(false);
      } catch (error) {
        if (!cancelled) {
          setIsRendering(false);
          onErrorRef.current(error instanceof Error ? error.message : "DOCX 版式渲染失败。");
        }
      }
    }

    void render();
    return () => {
      cancelled = true;
    };
  }, [renderKey, src]);

  return (
    <div className="min-h-0 min-w-0 overflow-auto rounded-none border-0 bg-surface-muted py-4" aria-label={`${title} 版式预览`} ref={scrollRef}>
      {isRendering ? (
        <div className="px-4 text-[13px] text-ink-muted">正在渲染版式...</div>
      ) : null}
      <div ref={containerRef} className="docx-layout-host mx-auto w-fit max-w-full" />
    </div>
  );
}
