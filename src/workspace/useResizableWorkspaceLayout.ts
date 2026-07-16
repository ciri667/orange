import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent, PointerEvent as ReactPointerEvent } from "react";

/** 可调整的主工作台侧栏类型；Agent 已改为浮窗，仅保留左侧知识库侧栏。 */
type ResizablePane = "sidebar";

/** 主工作台可持久化的布局尺寸，只保存知识库侧栏宽度，编辑器使用剩余空间。 */
interface WorkspaceLayoutSizes {
  sidebarWidth: number;
}

/** 指针拖拽过程中的起始状态，用于把鼠标位移稳定换算成侧栏宽度。 */
interface ResizeDragState {
  pane: ResizablePane;
  startX: number;
  startSizes: WorkspaceLayoutSizes;
}

/** 本机 UI 偏好存储键，版本后缀用于未来布局模型变化时安全回退。 */
const WORKSPACE_LAYOUT_STORAGE_KEY = "orange.workspace-layout.v1";
/** 旧版本品牌命名遗留的存储键，用于一次性迁移到新键，迁移后旧的保留作回滚保险。 */
const LEGACY_WORKSPACE_LAYOUT_STORAGE_KEY = "cici-note.workspace-layout.v1";

/** 默认侧栏宽度，沿用历史默认值，避免首次进入时视觉比例突变。 */
const DEFAULT_WORKSPACE_LAYOUT: WorkspaceLayoutSizes = {
  sidebarWidth: 285,
};

/** 知识库侧栏宽度边界，保证目录树可读且不会挤压编辑器。 */
const SIDEBAR_WIDTH_LIMIT = {
  min: 220,
  max: 420,
};

/** 编辑器最小宽度，拖拽侧栏时始终给正文编辑区保留主要空间。 */
const EDITOR_MIN_WIDTH = 500;

/** 分隔条轨道宽度，参与动态最大宽度计算，避免编辑器被实际轨道挤窄。 */
const RESIZER_TRACK_WIDTH = 10;

/** 键盘调整的基础步长；按住 Shift 时使用三倍步长快速调整。 */
const KEYBOARD_RESIZE_STEP = 16;

/** 未挂载工作台前的估算宽度，与全局最小窗口宽度保持一致。 */
const DEFAULT_WORKSPACE_WIDTH = 1180;

/** 判断 localStorage 中解析出的值是否是可参与尺寸计算的数字。 */
function isFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

/** 将数值限制在给定区间；当动态上限低于下限时优先保留下限。 */
function clampNumber(value: number, min: number, max: number) {
  const safeMax = Math.max(min, max);

  return Math.min(Math.max(value, min), safeMax);
}

/** 读取本机保存的工作台布局；损坏、过期或不可访问时回退默认布局。 */
function readStoredWorkspaceLayout(): WorkspaceLayoutSizes {
  if (typeof window === "undefined") {
    return DEFAULT_WORKSPACE_LAYOUT;
  }

  try {
    const rawLayout = window.localStorage.getItem(WORKSPACE_LAYOUT_STORAGE_KEY);

    if (!rawLayout) {
      // 旧品牌（cici-note）遗留的布局偏好做一次性迁移：旧键存在则复制到新键，
      // 旧键保留作回滚保险。仅在新键缺失时执行，保证迁移幂等。
      const legacyLayout = window.localStorage.getItem(LEGACY_WORKSPACE_LAYOUT_STORAGE_KEY);
      if (legacyLayout) {
        window.localStorage.setItem(WORKSPACE_LAYOUT_STORAGE_KEY, legacyLayout);
        return parseStoredWorkspaceLayout(legacyLayout);
      }
      return DEFAULT_WORKSPACE_LAYOUT;
    }

    return parseStoredWorkspaceLayout(rawLayout);
  } catch {
    // 损坏数据不阻断启动，回归默认布局。
    return DEFAULT_WORKSPACE_LAYOUT;
  }
}

/** 解析已读取的布局 JSON 字符串，损坏或越界时回退默认布局。 */
function parseStoredWorkspaceLayout(rawLayout: string): WorkspaceLayoutSizes {
  try {
    // 旧版可能含 agentWidth；Agent 已改为浮窗，这里只读取 sidebarWidth 并忽略 agentWidth。
    const parsedLayout = JSON.parse(rawLayout) as Partial<WorkspaceLayoutSizes> & { agentWidth?: unknown };

    if (!isFiniteNumber(parsedLayout.sidebarWidth)) {
      return DEFAULT_WORKSPACE_LAYOUT;
    }

    return {
      sidebarWidth: clampNumber(parsedLayout.sidebarWidth, SIDEBAR_WIDTH_LIMIT.min, SIDEBAR_WIDTH_LIMIT.max),
    };
  } catch {
    return DEFAULT_WORKSPACE_LAYOUT;
  }
}

/** 保存本机工作台布局偏好；只写入 sidebarWidth，存储不可用时静默降级。 */
function saveWorkspaceLayout(sizes: WorkspaceLayoutSizes) {
  if (typeof window === "undefined") {
    return;
  }

  try {
    window.localStorage.setItem(
      WORKSPACE_LAYOUT_STORAGE_KEY,
      JSON.stringify({ sidebarWidth: sizes.sidebarWidth }),
    );
  } catch {
    // localStorage 可能被隐私模式或 WebView 策略禁用，布局拖拽仍应继续可用。
  }
}

/** 计算侧栏在当前容器中最多能占用的宽度（保留一个分隔条与编辑器最小宽度）。 */
function getAvailableSidePanelWidth(containerWidth: number) {
  return Math.max(0, containerWidth - RESIZER_TRACK_WIDTH - EDITOR_MIN_WIDTH);
}

/** 根据当前容器宽度把侧栏尺寸限制在安全范围内。 */
function clampWorkspaceLayoutSizes(
  nextSizes: WorkspaceLayoutSizes,
  containerWidth: number,
): WorkspaceLayoutSizes {
  const availableSidePanelWidth = getAvailableSidePanelWidth(containerWidth);
  const maxSidebarWidth = Math.min(SIDEBAR_WIDTH_LIMIT.max, availableSidePanelWidth);

  return {
    sidebarWidth: clampNumber(nextSizes.sidebarWidth, SIDEBAR_WIDTH_LIMIT.min, maxSidebarWidth),
  };
}

/** 计算侧栏分隔条当前可表达的 aria 最小值和最大值。 */
function getPaneBounds(containerWidth: number) {
  const availableSidePanelWidth = getAvailableSidePanelWidth(containerWidth);
  const dynamicMax = Math.min(SIDEBAR_WIDTH_LIMIT.max, availableSidePanelWidth);

  return {
    min: SIDEBAR_WIDTH_LIMIT.min,
    max: Math.max(SIDEBAR_WIDTH_LIMIT.min, dynamicMax),
  };
}

/** 管理工作台侧栏拖拽、键盘调整和本机持久化；Agent 以浮窗形式独立布局。 */
export function useResizableWorkspaceLayout() {
  const [workspaceElement, setWorkspaceElement] = useState<HTMLElement | null>(null);
  const [workspaceWidth, setWorkspaceWidth] = useState(DEFAULT_WORKSPACE_WIDTH);
  const [sizes, setSizes] = useState<WorkspaceLayoutSizes>(() => readStoredWorkspaceLayout());
  const [resizingPane, setResizingPane] = useState<ResizablePane | null>(null);
  /** 当前工作台 DOM 节点，供原生 pointer 事件读取最新容器宽度。 */
  const workspaceElementRef = useRef<HTMLElement | null>(null);
  /** 最新布局尺寸引用，避免 pointerdown 闭包拿到过期宽度。 */
  const sizesRef = useRef(sizes);
  /** 当前拖拽起点；为空表示没有处于拖拽会话。 */
  const dragStateRef = useRef<ResizeDragState | null>(null);

  /** 回调 ref 能在加载态切换到主工作台时重新挂载 ResizeObserver。 */
  const workspaceRef = useCallback((element: HTMLElement | null) => {
    workspaceElementRef.current = element;
    setWorkspaceElement(element);
  }, []);

  /** 获取当前工作台宽度，DOM 尚未挂载时使用默认宽度做保守计算。 */
  const getWorkspaceWidth = useCallback(() => {
    return workspaceElementRef.current?.getBoundingClientRect().width ?? DEFAULT_WORKSPACE_WIDTH;
  }, []);

  useEffect(() => {
    sizesRef.current = sizes;
  }, [sizes]);

  useEffect(() => {
    if (!workspaceElement) {
      return;
    }

    saveWorkspaceLayout(sizes);
  }, [sizes, workspaceElement]);

  useEffect(() => {
    if (!workspaceElement) {
      return undefined;
    }

    /** 同步容器宽度并顺手修正超出新窗口范围的已保存尺寸。 */
    function syncWorkspaceWidth(nextWidth: number) {
      setWorkspaceWidth(nextWidth);
      setSizes((currentSizes) => clampWorkspaceLayoutSizes(currentSizes, nextWidth));
    }

    syncWorkspaceWidth(workspaceElement.getBoundingClientRect().width);

    if (typeof ResizeObserver === "undefined") {
      const handleWindowResize = () => syncWorkspaceWidth(workspaceElement.getBoundingClientRect().width);

      window.addEventListener("resize", handleWindowResize);

      return () => window.removeEventListener("resize", handleWindowResize);
    }

    const resizeObserver = new ResizeObserver((entries) => {
      const entry = entries[0];

      if (entry) {
        syncWorkspaceWidth(entry.contentRect.width);
      }
    });

    resizeObserver.observe(workspaceElement);

    return () => resizeObserver.disconnect();
  }, [workspaceElement]);

  useEffect(() => {
    if (!resizingPane) {
      return undefined;
    }

    if (typeof document !== "undefined") {
      document.body.classList.add("workspace-resize-active");
    }

    /** 根据指针横向位移实时换算侧栏宽度。 */
    function handlePointerMove(event: PointerEvent) {
      const dragState = dragStateRef.current;

      if (!dragState) {
        return;
      }

      const pointerDelta = event.clientX - dragState.startX;
      const nextSizes = {
        sidebarWidth: dragState.startSizes.sidebarWidth + pointerDelta,
      };

      event.preventDefault();
      setSizes(clampWorkspaceLayoutSizes(nextSizes, getWorkspaceWidth()));
    }

    /** 结束拖拽并恢复全局选择、光标状态。 */
    function stopResize() {
      dragStateRef.current = null;
      setResizingPane(null);
    }

    window.addEventListener("pointermove", handlePointerMove);
    window.addEventListener("pointerup", stopResize);
    window.addEventListener("pointercancel", stopResize);

    return () => {
      window.removeEventListener("pointermove", handlePointerMove);
      window.removeEventListener("pointerup", stopResize);
      window.removeEventListener("pointercancel", stopResize);

      if (typeof document !== "undefined") {
        document.body.classList.remove("workspace-resize-active");
      }
    };
  }, [getWorkspaceWidth, resizingPane]);

  /** 指针按下时记录起点，后续移动由 window 级事件接管。 */
  const handleSeparatorPointerDown = useCallback(
    (pane: ResizablePane, event: ReactPointerEvent<HTMLDivElement>) => {
      if (event.pointerType === "mouse" && event.button !== 0) {
        return;
      }

      dragStateRef.current = {
        pane,
        startX: event.clientX,
        startSizes: sizesRef.current,
      };
      setResizingPane(pane);
      event.currentTarget.setPointerCapture(event.pointerId);
      event.preventDefault();
    },
    [],
  );

  /** 通过键盘调整分隔条，满足无鼠标场景下的基础可访问性。 */
  const handleSeparatorKeyDown = useCallback(
    (pane: ResizablePane, event: ReactKeyboardEvent<HTMLDivElement>) => {
      const paneBounds = getPaneBounds(getWorkspaceWidth());
      const currentValue = sizesRef.current.sidebarWidth;
      const step = event.shiftKey ? KEYBOARD_RESIZE_STEP * 3 : KEYBOARD_RESIZE_STEP;
      let nextValue: number | null = null;

      if (event.key === "Home") {
        nextValue = paneBounds.min;
      } else if (event.key === "End") {
        nextValue = paneBounds.max;
      } else if (event.key === "ArrowLeft") {
        nextValue = currentValue - step;
      } else if (event.key === "ArrowRight") {
        nextValue = currentValue + step;
      }

      if (nextValue === null) {
        return;
      }

      event.preventDefault();
      setSizes((currentSizes) =>
        clampWorkspaceLayoutSizes({ ...currentSizes, sidebarWidth: nextValue }, getWorkspaceWidth()),
      );
    },
    [getWorkspaceWidth],
  );

  /** 双击分隔条恢复默认侧栏宽度。 */
  const handleSeparatorDoubleClick = useCallback(() => {
    setSizes(clampWorkspaceLayoutSizes(DEFAULT_WORKSPACE_LAYOUT, getWorkspaceWidth()));
  }, [getWorkspaceWidth]);

  /** 主 grid 的列定义由 hook 统一生成：侧栏 + 分隔条 + 弹性编辑区。 */
  const gridTemplateColumns = useMemo(
    () =>
      `${sizes.sidebarWidth}px ${RESIZER_TRACK_WIDTH}px minmax(${EDITOR_MIN_WIDTH}px, 1fr)`,
    [sizes.sidebarWidth],
  );

  /** 生成侧栏分隔条需要的交互和 aria 属性。 */
  const getSeparatorProps = useCallback(
    (pane: ResizablePane) => {
      const paneBounds = getPaneBounds(workspaceWidth);

      return {
        role: "separator",
        tabIndex: 0,
        "aria-label": "调整知识库侧栏宽度",
        "aria-orientation": "vertical" as const,
        "aria-valuemin": paneBounds.min,
        "aria-valuemax": paneBounds.max,
        "aria-valuenow": Math.round(sizes.sidebarWidth),
        onDoubleClick: handleSeparatorDoubleClick,
        onKeyDown: (event: ReactKeyboardEvent<HTMLDivElement>) => handleSeparatorKeyDown(pane, event),
        onPointerDown: (event: ReactPointerEvent<HTMLDivElement>) => handleSeparatorPointerDown(pane, event),
      };
    },
    [
      handleSeparatorDoubleClick,
      handleSeparatorKeyDown,
      handleSeparatorPointerDown,
      sizes,
      workspaceWidth,
    ],
  );

  return {
    workspaceRef,
    gridTemplateColumns,
    resizingPane,
    getSeparatorProps,
  };
}
