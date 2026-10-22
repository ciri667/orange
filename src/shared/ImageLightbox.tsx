import { Minus, Plus, X } from "lucide-react";
import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { createPortal } from "react-dom";
import { Button } from "./Button";
import { cn } from "./cn";

/** 缩放下限：再小就只剩一个色块，没有查看价值。 */
const MIN_SCALE = 0.5;
/** 缩放上限：5 倍足够看清截图里的文字，继续放大会糊成马赛克。 */
const MAX_SCALE = 5;
/** 滚轮缩放力度：按 deltaY 做指数缩放，触控板的小步长也不会跳变。 */
const WHEEL_ZOOM_RATE = 0.0018;
/** 双击放大到的倍数。 */
const DOUBLE_CLICK_SCALE = 2;
/** 缩放按钮的步进倍数。 */
const BUTTON_ZOOM_STEP = 1.25;
/** 图片与视窗之间保留的留白，和图片 max-w / max-h 的 48px 约束对齐。 */
const VIEWPORT_PADDING = 24;

/** 图片的缩放与平移状态；translate 在 scale 之前应用，缩放原点始终是图片中心。 */
interface ImageView {
  scale: number;
  x: number;
  y: number;
}

/** 初始视图：原始大小、居中。 */
const IDENTITY_VIEW: ImageView = { scale: 1, x: 0, y: 0 };

function clamp(value: number, min: number, max: number) {
  return Math.min(Math.max(value, min), max);
}

/**
 * 把缩放收进上下限，并把平移限制在图片边缘之内：
 * 放大后可以拖到任意角落，但不会把图片拖出视窗。
 */
function clampView(view: ImageView, image: HTMLImageElement | null, container: HTMLElement | null): ImageView {
  const scale = clamp(view.scale, MIN_SCALE, MAX_SCALE);
  if (!image || !container) {
    return { scale, x: 0, y: 0 };
  }
  // offsetWidth / offsetHeight 是 transform 之前的布局尺寸，不受当前缩放影响。
  const availableWidth = Math.max(0, container.clientWidth - VIEWPORT_PADDING * 2);
  const availableHeight = Math.max(0, container.clientHeight - VIEWPORT_PADDING * 2);
  const maxX = Math.max(0, (image.offsetWidth * scale - availableWidth) / 2);
  const maxY = Math.max(0, (image.offsetHeight * scale - availableHeight) / 2);
  return { scale, x: clamp(view.x, -maxX, maxX), y: clamp(view.y, -maxY, maxY) };
}

/**
 * 全屏查看一张图片。
 * 滚轮（或触控板双指）以光标为锚点缩放，范围 50%–500%；放大后可拖动平移，
 * 双击或点击百分比还原。Esc 关闭，+ / - / 0 也能缩放和还原。
 */
export function ImageLightbox({
  src,
  alt,
  onClose,
}: {
  src: string;
  alt: string;
  onClose: () => void;
}) {
  const titleId = useId();
  const hintId = useId();
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const overlayRef = useRef<HTMLDivElement>(null);
  const imageRef = useRef<HTMLImageElement>(null);
  const [view, setView] = useState<ImageView>(IDENTITY_VIEW);
  const [isDragging, setIsDragging] = useState(false);
  const label = alt.trim() || "图片预览";

  /** 原生 wheel / pointer / touch 监听器读 ref，避免闭包拿到旧的缩放值。 */
  const viewRef = useRef(view);
  /** 图片未缩放时的布局中心（视口坐标）；transform 不改变布局，所以它是个常量。 */
  const layoutCenterRef = useRef<{ x: number; y: number } | null>(null);
  /** 当前按下的指针集合：双指缩放时不能同时被单指拖动逻辑接管。 */
  const activePointersRef = useRef(new Set<number>());
  const dragRef = useRef<{
    pointerId: number;
    startX: number;
    startY: number;
    originX: number;
    originY: number;
  } | null>(null);

  /** 统一提交视图：先同步 ref 再 setState，连续滚轮事件不会基于旧值累加。 */
  const commitView = useCallback((next: ImageView) => {
    const clamped = clampView(next, imageRef.current, overlayRef.current);
    const previous = viewRef.current;
    if (clamped.scale === previous.scale && clamped.x === previous.x && clamped.y === previous.y) {
      return;
    }
    viewRef.current = clamped;
    setView(clamped);
  }, []);

  /** 记录图片未缩放时的中心点，缩放时用它换算光标在图片坐标系里的位置。 */
  const measureLayoutCenter = useCallback(() => {
    const image = imageRef.current;
    if (!image) {
      return;
    }
    const rect = image.getBoundingClientRect();
    const current = viewRef.current;
    layoutCenterRef.current = {
      x: rect.left + rect.width / 2 - current.x,
      y: rect.top + rect.height / 2 - current.y,
    };
  }, []);

  /**
   * 缩放到 nextScale；传了 anchor（视口坐标）就保持光标下的像素不动，
   * 否则以图片中心缩放。
   */
  const zoomTo = useCallback(
    (nextScale: number, anchor?: { x: number; y: number }) => {
      const previous = viewRef.current;
      const scale = clamp(nextScale, MIN_SCALE, MAX_SCALE);
      if (Math.abs(scale - previous.scale) < 0.0001) {
        return;
      }

      let x = previous.x;
      let y = previous.y;
      const center = layoutCenterRef.current;
      if (anchor && center) {
        // 保持锚点不动：t' = (s'/s)·t + (1 - s'/s)·(锚点 - 图片中心)。
        const ratio = scale / previous.scale;
        x = ratio * previous.x + (1 - ratio) * (anchor.x - center.x);
        y = ratio * previous.y + (1 - ratio) * (anchor.y - center.y);
      }
      commitView({ scale, x, y });
    },
    [commitView],
  );

  /** 打开后把焦点交给关闭按钮，Esc 随时可退出。 */
  useEffect(() => {
    closeButtonRef.current?.focus();
  }, []);

  /** 换图时回到原始大小，避免上一张的缩放和位移带过来。 */
  useEffect(() => {
    commitView(IDENTITY_VIEW);
  }, [src, commitView]);

  /**
   * 布局完成后测量一次。
   * 命中缓存的图片可能早于 React 挂载就完成加载，onLoad 不会再来，所以这里补一次。
   */
  useLayoutEffect(() => {
    const image = imageRef.current;
    if (!image) {
      return;
    }
    if (image.complete) {
      measureLayoutCenter();
      return;
    }
    // 未加载完时布局尺寸还是 0，等 onLoad 再测；这里先挂一次性监听兜底。
    const handleLoad = () => measureLayoutCenter();
    image.addEventListener("load", handleLoad, { once: true });
    return () => image.removeEventListener("load", handleLoad);
  }, [measureLayoutCenter, src]);

  /** 窗口尺寸变化后图片的布局尺寸会变，重新测量并收回越界的平移。 */
  useEffect(() => {
    function handleResize() {
      measureLayoutCenter();
      commitView(viewRef.current);
    }

    window.addEventListener("resize", handleResize);
    return () => window.removeEventListener("resize", handleResize);
  }, [commitView, measureLayoutCenter]);

  /** 键盘快捷键：Esc 关闭，+ / - 缩放，0 还原。 */
  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onClose();
        return;
      }
      // 带修饰键的组合留给系统快捷键（如 Ctrl + / - 缩放整个界面）。
      if (event.altKey || event.ctrlKey || event.metaKey) {
        return;
      }
      if (event.key === "+" || event.key === "=") {
        event.preventDefault();
        zoomTo(viewRef.current.scale * BUTTON_ZOOM_STEP);
      } else if (event.key === "-" || event.key === "_") {
        event.preventDefault();
        zoomTo(viewRef.current.scale / BUTTON_ZOOM_STEP);
      } else if (event.key === "0") {
        event.preventDefault();
        commitView(IDENTITY_VIEW);
      }
    }

    window.addEventListener("keydown", handleKeyDown, true);
    return () => window.removeEventListener("keydown", handleKeyDown, true);
  }, [commitView, onClose, zoomTo]);

  /** 滚轮缩放：整个浮层都接管滚轮，背后的消息列表不会跟着滚。 */
  useEffect(() => {
    const overlay = overlayRef.current;
    if (!overlay) {
      return;
    }

    function handleWheel(event: WheelEvent) {
      event.preventDefault();
      // deltaMode 不是像素时先换算成像素，行模式按 16px 估算。
      const unit =
        event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? (overlayRef.current?.clientHeight ?? 0) : 1;
      const factor = Math.exp(-event.deltaY * unit * WHEEL_ZOOM_RATE);
      zoomTo(viewRef.current.scale * factor, { x: event.clientX, y: event.clientY });
    }

    overlay.addEventListener("wheel", handleWheel, { passive: false });
    return () => overlay.removeEventListener("wheel", handleWheel);
  }, [zoomTo]);

  /** 双指缩放：以两指中点为锚点，同时跟随中点移动做平移。 */
  useEffect(() => {
    const overlay = overlayRef.current;
    if (!overlay) {
      return;
    }

    let pinch: { distance: number; scale: number; midX: number; midY: number } | null = null;

    function touchDistance(a: Touch, b: Touch) {
      return Math.hypot(b.clientX - a.clientX, b.clientY - a.clientY);
    }

    function handleTouchStart(event: TouchEvent) {
      if (event.touches.length !== 2) {
        return;
      }
      const [a, b] = [event.touches[0], event.touches[1]];
      pinch = {
        distance: touchDistance(a, b),
        scale: viewRef.current.scale,
        midX: (a.clientX + b.clientX) / 2,
        midY: (a.clientY + b.clientY) / 2,
      };
    }

    function handleTouchMove(event: TouchEvent) {
      if (!pinch || event.touches.length !== 2) {
        return;
      }
      event.preventDefault();
      const [a, b] = [event.touches[0], event.touches[1]];
      const midX = (a.clientX + b.clientX) / 2;
      const midY = (a.clientY + b.clientY) / 2;
      const nextScale = clamp(
        pinch.scale * (touchDistance(a, b) / Math.max(pinch.distance, 1)),
        MIN_SCALE,
        MAX_SCALE,
      );

      const previous = viewRef.current;
      const center = layoutCenterRef.current;
      let x = previous.x;
      let y = previous.y;
      if (center) {
        // 上一刻中点下的像素要跟到新中点：t' = (s'/s)·t + d新 - (s'/s)·d旧。
        const ratio = nextScale / previous.scale;
        x = ratio * previous.x + (midX - center.x) - ratio * (pinch.midX - center.x);
        y = ratio * previous.y + (midY - center.y) - ratio * (pinch.midY - center.y);
      }
      pinch.midX = midX;
      pinch.midY = midY;
      commitView({ scale: nextScale, x, y });
    }

    function handleTouchEnd(event: TouchEvent) {
      if (event.touches.length < 2) {
        pinch = null;
      }
    }

    overlay.addEventListener("touchstart", handleTouchStart, { passive: true });
    overlay.addEventListener("touchmove", handleTouchMove, { passive: false });
    overlay.addEventListener("touchend", handleTouchEnd, { passive: true });
    overlay.addEventListener("touchcancel", handleTouchEnd, { passive: true });

    return () => {
      overlay.removeEventListener("touchstart", handleTouchStart);
      overlay.removeEventListener("touchmove", handleTouchMove);
      overlay.removeEventListener("touchend", handleTouchEnd);
      overlay.removeEventListener("touchcancel", handleTouchEnd);
    };
  }, [commitView]);

  /**
   * 结束一次拖动。
   * 指针可能在图片外面抬起（没有 pointer capture 时），所以 window 上也监听一份，
   * 否则 activePointersRef 会残留，之后的拖动会被误判成双指而失效。
   */
  useEffect(() => {
    function release(event: PointerEvent) {
      activePointersRef.current.delete(event.pointerId);
      if (dragRef.current?.pointerId !== event.pointerId) {
        return;
      }
      dragRef.current = null;
      setIsDragging(false);
    }

    window.addEventListener("pointerup", release);
    window.addEventListener("pointercancel", release);
    return () => {
      window.removeEventListener("pointerup", release);
      window.removeEventListener("pointercancel", release);
    };
  }, []);

  /** 放大后按住图片拖动平移；未放大时没有可拖动的余量。 */
  function handlePointerDown(event: ReactPointerEvent<HTMLImageElement>) {
    activePointersRef.current.add(event.pointerId);
    if (event.button !== 0 || viewRef.current.scale <= 1 || activePointersRef.current.size > 1) {
      return;
    }
    event.currentTarget.setPointerCapture(event.pointerId);
    dragRef.current = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      originX: viewRef.current.x,
      originY: viewRef.current.y,
    };
    setIsDragging(true);
  }

  function handlePointerMove(event: ReactPointerEvent<HTMLImageElement>) {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId || activePointersRef.current.size !== 1) {
      return;
    }
    commitView({
      scale: viewRef.current.scale,
      x: drag.originX + event.clientX - drag.startX,
      y: drag.originY + event.clientY - drag.startY,
    });
  }

  /** 双击在原始大小和 2 倍之间切换，双击点同样作为缩放锚点。 */
  function handleDoubleClick(event: ReactMouseEvent<HTMLImageElement>) {
    if (viewRef.current.scale > 1.01) {
      commitView(IDENTITY_VIEW);
      return;
    }
    zoomTo(DOUBLE_CLICK_SCALE, { x: event.clientX, y: event.clientY });
  }

  const isZoomedIn = view.scale > 1.01;
  const canZoomOut = view.scale > MIN_SCALE + 0.001;
  const canZoomIn = view.scale < MAX_SCALE - 0.001;

  return createPortal(
    <div
      ref={overlayRef}
      className="fixed inset-0 z-confirm isolate grid select-none place-items-center overflow-hidden bg-[rgba(17,24,39,0.78)] p-4"
      role="dialog"
      aria-modal="true"
      aria-labelledby={titleId}
      aria-describedby={hintId}
      onMouseDown={(event) => {
        if (event.target !== event.currentTarget) {
          return;
        }
        event.stopPropagation();
        onClose();
      }}
    >
      <div
        className="relative grid max-h-full max-w-full place-items-center"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <h2 id={titleId} className="sr-only">
          {label}
        </h2>
        <img
          ref={imageRef}
          src={src}
          alt={label}
          draggable={false}
          onLoad={measureLayoutCenter}
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onDoubleClick={handleDoubleClick}
          style={{ transform: `translate3d(${view.x}px, ${view.y}px, 0) scale(${view.scale})` }}
          className={cn(
            "max-h-[calc(100vh-48px)] max-w-[min(1100px,calc(100vw-48px))] touch-none rounded-md object-contain shadow-app will-change-transform",
            isZoomedIn ? (isDragging ? "cursor-grabbing" : "cursor-grab") : "cursor-zoom-in",
          )}
        />
      </div>

      <p
        id={hintId}
        className="pointer-events-none absolute top-4 left-4 hidden rounded-full bg-[rgba(17,24,39,0.6)] px-3 py-1.5 text-[11px] text-white/80 sm:block"
      >
        滚轮缩放 · 拖动平移 · 双击还原
      </p>

      <div
        className="absolute bottom-4 left-1/2 flex -translate-x-1/2 items-center gap-0.5 rounded-full border border-border bg-surface/95 p-1 shadow-app backdrop-blur-[6px]"
        role="group"
        aria-label="图片缩放"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <Button
          variant="icon"
          size="compact"
          aria-label="缩小图片"
          title="缩小（-）"
          disabled={!canZoomOut}
          onClick={() => zoomTo(viewRef.current.scale / BUTTON_ZOOM_STEP)}
        >
          <Minus size={15} />
        </Button>
        <Button
          variant="text"
          size="compact"
          className="min-w-[52px] tabular-nums"
          aria-label={`当前缩放 ${Math.round(view.scale * 100)}%，点击还原`}
          title="还原到 100%"
          onClick={() => commitView(IDENTITY_VIEW)}
        >
          {Math.round(view.scale * 100)}%
        </Button>
        <Button
          variant="icon"
          size="compact"
          aria-label="放大图片"
          title="放大（+）"
          disabled={!canZoomIn}
          onClick={() => zoomTo(viewRef.current.scale * BUTTON_ZOOM_STEP)}
        >
          <Plus size={15} />
        </Button>
      </div>

      <Button
        ref={closeButtonRef}
        variant="icon"
        className="absolute top-4 right-4 border-transparent bg-white/92 text-ink hover:enabled:bg-white"
        aria-label="关闭图片预览"
        onMouseDown={(event) => {
          event.stopPropagation();
          onClose();
        }}
      >
        <X size={18} />
      </Button>
    </div>,
    document.body,
  );
}
