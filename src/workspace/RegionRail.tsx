import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Maximize2 } from "lucide-react";
import { cn } from "../shared/cn";
import type { CSSProperties, ReactNode } from "react";
import type { RegionEdge, RegionItem, RegionItemId } from "./collapsibleRegions";

/** 鼠标移入图标后延迟打开浮层，避免扫过图标栏时反复弹出。 */
const PREVIEW_OPEN_DELAY = 220;
/** 指针离开图标栏和浮层后延迟关闭，给指针在两者之间移动留出时间。 */
const PREVIEW_CLOSE_DELAY = 180;
/** 浮层与图标栏之间的贴合间距，单位 px。 */
const PREVIEW_GAP = 4;

/** 关闭前的指针命中判定向外放宽的像素，避免指针压在边框上被误判为已离开。 */
const CLOSE_HIT_SLOP = 6;

/**
 * 判断一个视口坐标是否落在「图标栏 + 浮层」可交互区域内。
 *
 * 浮层里是可以操作的，指针在其内部移动、点击控件、聚焦输入框都会产生 mouseleave，
 * 关闭前用它复核指针真的离开了这块区域，否则浮层会「一碰就关」。
 */
function isPointWithinRegionSurface(rail: HTMLElement | null, preview: HTMLElement | null, point: { x: number; y: number }) {
  return [rail, preview].some((element) => {
    if (!element) {
      return false;
    }

    const rect = element.getBoundingClientRect();

    return (
      point.x >= rect.left - CLOSE_HIT_SLOP &&
      point.x <= rect.right + CLOSE_HIT_SLOP &&
      point.y >= rect.top - CLOSE_HIT_SLOP &&
      point.y <= rect.bottom + CLOSE_HIT_SLOP
    );
  });
}

export interface RegionRailProps {
  edge: RegionEdge;
  /** 取消悬浮时读取的条目；每个条目对应一个图标和一个浮层。 */
  items: RegionItem[];
  activeItemId?: RegionItemId;
  /** 区域无障碍名称，如「知识库栏」。 */
  label: string;
  onActivateItem: (itemId: RegionItemId) => void;
  /**
   * 浮层即将打开时通知调用方悬浮的是哪个条目。
   *
   * 左栏的会话/文件是一对共享的互斥分块，浮层内容由共享状态决定；
   * 悬浮「文件」时把这个选择同步给共享状态，悬浮看到的内容才和图标一致。
   */
  onHoverItem?: (itemId: RegionItemId) => void;
  /** 渲染某个条目展开后的界面；由调用方按区域分发。浮层里这份是可交互的。 */
  renderPreview: (itemId: RegionItemId) => ReactNode;
  /** 浮层宽度：取该区域展开态的实际宽度，让浮层里的换行和展开后一致。 */
  previewWidth?: number;
}

/**
 * 收起态的图标栏。
 *
 * 每个图标代表区域内的一块内容；鼠标悬浮时在侧边弹出浮层，
 * 浮层里渲染的是该内容**展开后的真实界面，并且可以直接操作**——
 * 它和展开态共用上层状态，所以在浮层里点会话、切分块、往输入框里打字都会就地生效。
 * 浮层只在「指针离开图标栏和浮层」「点击浮层外部」「按 Esc」时关闭。
 */
export function RegionRail({
  edge,
  items,
  activeItemId,
  label,
  onActivateItem,
  onHoverItem,
  renderPreview,
  previewWidth,
}: RegionRailProps) {
  const railRef = useRef<HTMLElement | null>(null);
  const previewRef = useRef<HTMLDivElement | null>(null);
  const itemRefs = useRef(new Map<RegionItemId, HTMLButtonElement>());
  /** 最新指针坐标，用于关闭前复核指针是否真的离开了图标栏和浮层。 */
  const pointerPositionRef = useRef<{ x: number; y: number } | null>(null);
  /** 当前悬浮的条目；为空表示没有浮层。 */
  const [hoveredItemId, setHoveredItemId] = useState<RegionItemId | null>(null);
  /** 浮层的 fixed 坐标，按图标位置实时计算。 */
  const [previewPosition, setPreviewPosition] = useState<{ left: number; top: number } | null>(null);
  /** 打开定时器；指针移入浮层前不清除关闭定时器，因此两者分开存。 */
  const openTimerRef = useRef<number | null>(null);
  const closeTimerRef = useRef<number | null>(null);

  const clearTimers = useCallback(() => {
    if (openTimerRef.current !== null) {
      window.clearTimeout(openTimerRef.current);
      openTimerRef.current = null;
    }

    if (closeTimerRef.current !== null) {
      window.clearTimeout(closeTimerRef.current);
      closeTimerRef.current = null;
    }
  }, []);

  useEffect(() => clearTimers, [clearTimers]);

  /** 按图标位置和浮层尺寸把浮层贴到图标栏外侧，并限制在视口内。 */
  const updatePreviewPosition = useCallback((itemId: RegionItemId) => {
    const button = itemRefs.current.get(itemId);

    if (!button) {
      return;
    }

    const rect = button.getBoundingClientRect();
    const margin = 8;
    // 左栏浮层挂在图标右侧、右栏挂在图标左侧；横向位置由各自的 translateX 收拢。
    const left = edge === "left" ? rect.right + PREVIEW_GAP : rect.left - PREVIEW_GAP;

    setPreviewPosition({
      left,
      // 这里只保证纵向不越出视口。
      top: Math.min(Math.max(margin, rect.top), Math.max(margin, window.innerHeight - margin)),
    });
  }, [edge]);

  const scheduleOpen = useCallback((itemId: RegionItemId) => {
    if (closeTimerRef.current !== null) {
      window.clearTimeout(closeTimerRef.current);
      closeTimerRef.current = null;
    }

    if (hoveredItemId === itemId) {
      return;
    }

    if (openTimerRef.current !== null) {
      window.clearTimeout(openTimerRef.current);
    }

    openTimerRef.current = window.setTimeout(() => {
      openTimerRef.current = null;
      // 先同步共享选择，再打开浮层，避免浮层先按旧分块渲染一帧再跳变。
      onHoverItem?.(itemId);
      setHoveredItemId(itemId);
    }, PREVIEW_OPEN_DELAY);
  }, [hoveredItemId, onHoverItem]);

  /**
   * 判断指针是否仍在「图标栏 + 浮层」这两块可交互区域内。
   *
   * 浮层里可以操作，指针在其内部移动、点控件、聚焦输入框都会产生 mouseleave，
   * 关闭前用它复核指针确实离开了这块区域，否则浮层会「一碰就关」。
   * 判定用坐标而不是 mouseenter/mouseleave 配对，因为浮层内的控件会捕获指针事件。
   */
  const isPointerWithinRegionSurface = useCallback(() => {
    return pointerPositionRef.current
      ? isPointWithinRegionSurface(railRef.current, previewRef.current, pointerPositionRef.current)
      : false;
  }, []);

  /** 指针离开图标栏或浮层后延迟关闭，关闭前复核指针真的离开了。 */
  const scheduleClose = useCallback(() => {
    if (openTimerRef.current !== null) {
      window.clearTimeout(openTimerRef.current);
      openTimerRef.current = null;
    }

    if (closeTimerRef.current !== null) {
      window.clearTimeout(closeTimerRef.current);
    }

    closeTimerRef.current = window.setTimeout(() => {
      closeTimerRef.current = null;

      if (isPointerWithinRegionSurface()) {
        return;
      }

      setHoveredItemId(null);
    }, PREVIEW_CLOSE_DELAY);
  }, [isPointerWithinRegionSurface]);

  /** 浮层里开始交互时立刻取消待执行的关闭，避免刚点进去就被收走。 */
  const handlePreviewInteraction = useCallback(() => {
    if (closeTimerRef.current !== null) {
      window.clearTimeout(closeTimerRef.current);
      closeTimerRef.current = null;
    }
  }, []);

  /** 记录最新指针坐标，供关闭前复核使用。 */
  useEffect(() => {
    function handlePointerMove(event: PointerEvent) {
      pointerPositionRef.current = { x: event.clientX, y: event.clientY };
    }

    window.addEventListener("pointermove", handlePointerMove, true);

    return () => window.removeEventListener("pointermove", handlePointerMove, true);
  }, []);

  /** 浮层打开时挂外部点击和 Esc 关闭；浮层内部的操作不触发关闭。 */
  useEffect(() => {
    if (!hoveredItemId) {
      return undefined;
    }

    function isInsideRegionSurface(target: Node) {
      const rail = railRef.current;
      const preview = previewRef.current;

      return Boolean(rail?.contains(target) || preview?.contains(target));
    }

    function handlePointerDown(event: MouseEvent) {
      if (!(event.target instanceof Node)) {
        return;
      }

      // 点到图标栏或浮层内部属于正常操作，不关闭浮层。
      if (isInsideRegionSurface(event.target)) {
        return;
      }

      setHoveredItemId(null);
    }

    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        setHoveredItemId(null);
      }
    }

    document.addEventListener("mousedown", handlePointerDown, true);
    document.addEventListener("keydown", handleKeyDown, true);

    return () => {
      document.removeEventListener("mousedown", handlePointerDown, true);
      document.removeEventListener("keydown", handleKeyDown, true);
    };
  }, [hoveredItemId]);

  /** 浮层打开期间跟随滚动和窗口尺寸变化，避免浮层与图标脱节。 */
  useEffect(() => {
    if (!hoveredItemId) {
      return undefined;
    }

    updatePreviewPosition(hoveredItemId);

    const handleReposition = () => updatePreviewPosition(hoveredItemId);

    window.addEventListener("resize", handleReposition);
    window.addEventListener("scroll", handleReposition, true);

    return () => {
      window.removeEventListener("resize", handleReposition);
      window.removeEventListener("scroll", handleReposition, true);
    };
  }, [hoveredItemId, updatePreviewPosition]);

  /**
   * 浮层实际展示的条目。
   *
   * 正常情况下就是悬浮的那个图标；但会话和文件共享同一个分块状态，
   * 用户可以在浮层内部切到另一个条目。这时要用共享的 activeItemId，
   * 否则标题、页脚和「展开」会停留在打开时悬浮的图标上，
   * 点「展开」还会把用户在浮层里的选择打回。
   */
  const hoveredItem = hoveredItemId ? items.find((item) => item.id === hoveredItemId) : undefined;

  /**
   * 浮层实际展示的条目。
   *
   * 正常情况下就是悬浮的那个图标；但会话和文件共享同一个分块状态，
   * 用户可以在浮层内部切到另一个条目。这时要用共享的 activeItemId，
   * 否则标题、页脚和「展开」会停留在打开时悬浮的图标上，
   * 点「展开」还会把用户在浮层里的选择打回。
   */
  const displayedItem =
    hoveredItem && activeItemId && items.some((item) => item.id === activeItemId)
      ? (items.find((item) => item.id === activeItemId) ?? hoveredItem)
      : hoveredItem;

  return (
    <>
      <aside
        ref={railRef}
        className={cn("region-rail", `region-rail-${edge}`)}
        aria-label={`${label}（已收起）`}
        aria-orientation="vertical"
      >
        <ul className="region-rail-list">
          {items.map((item) => {
            const ItemIcon = item.Icon;
            const isActive = item.id === activeItemId;

            return (
              <li key={item.id}>
                <button
                  type="button"
                  ref={(element) => {
                    if (element) {
                      itemRefs.current.set(item.id, element);
                    } else {
                      itemRefs.current.delete(item.id);
                    }
                  }}
                  className={cn("region-rail-item", isActive && "is-active")}
                  aria-label={
                    // 单一内容的区域（顶栏/编辑区/Agent）不重复读区域名。
                    items.length > 1 ? `展开${label}：${item.label}` : `展开${label}`
                  }
                  aria-current={isActive ? "true" : undefined}
                  title={items.length > 1 ? item.label : label}
                  onClick={() => onActivateItem(item.id)}
                  onMouseEnter={() => scheduleOpen(item.id)}
                  onMouseLeave={scheduleClose}
                  onFocus={() => scheduleOpen(item.id)}
                  onBlur={scheduleClose}
                >
                  <ItemIcon size={17} aria-hidden />
                </button>
              </li>
            );
          })}
        </ul>
      </aside>

      {/*
        浮层通过 portal 挂到 body：图标栏自身只有几十像素宽且 overflow 受限，
        浮层必须逃出这两个约束才能渲染完整的展开态界面。
      */}
      {displayedItem &&
        previewPosition &&
        createPortal(
          <div
            ref={previewRef}
            className={cn("region-preview", `region-preview-${edge}`)}
            style={{
              left: previewPosition.left,
              top: previewPosition.top,
              ...(previewWidth && (edge === "left" || edge === "right")
                ? ({ "--region-preview-width": `${previewWidth}px` } as CSSProperties)
                : null),
            }}
            data-region-preview="true"
            role="dialog"
            aria-label={`${displayedItem.label} 浮层`}
            onMouseEnter={() => handlePreviewInteraction()}
            onMouseMove={handlePreviewInteraction}
            onMouseDown={handlePreviewInteraction}
            onMouseLeave={scheduleClose}
            onFocus={handlePreviewInteraction}
          >
            {/*
              这里渲染的是展开态的真实界面，并且可以直接操作：
              它和展开态共用上层状态，所以在浮层里点会话、切分块、往输入框打字都会就地生效。
            */}
            <div className="region-preview-body">{renderPreview(displayedItem.id)}</div>
            <div className="region-preview-footer">
              <span className="region-preview-hint">{items.length > 1 ? displayedItem.label : label}</span>
              <button
                type="button"
                className="region-preview-expand"
                onClick={() => onActivateItem(displayedItem.id)}
              >
                <Maximize2 size={13} aria-hidden />
                展开
              </button>
            </div>
          </div>,
          document.body,
        )}
    </>
  );
}
