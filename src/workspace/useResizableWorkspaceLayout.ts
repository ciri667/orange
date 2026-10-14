import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent, PointerEvent as ReactPointerEvent } from "react";
import { COLLAPSIBLE_REGIONS, REGION_ORDER, REGION_RAIL_SIZE } from "./collapsibleRegions";
import type { RegionId, RegionItemId } from "./collapsibleRegions";

/** 可调整的主工作台侧栏类型，用于区分左侧知识库和右侧 Agent 面板。 */
type ResizablePane = "sidebar" | "agent";

/** 每个区域收起态的显隐；中间编辑区始终占剩余空间，不参与收起。 */
export type RegionOpenState = Record<RegionId, boolean>;

/** 各区域在收起态图标栏上当前激活的条目，用于决定浮层预览哪一块内容。 */
export type RegionActiveItems = Partial<Record<RegionId, RegionItemId>>;

/** 主工作台可持久化的布局：两侧栏宽度、两个区域的显隐、各区域激活条目。 */
interface WorkspaceLayoutSizes {
  sidebarWidth: number;
  agentWidth: number;
  agentOpen: boolean;
  regions: RegionOpenState;
  activeItems: RegionActiveItems;
}

/** 指针拖拽过程中的起始状态，用于把鼠标位移稳定换算成目标栏宽。 */
interface ResizeDragState {
  pane: ResizablePane;
  startX: number;
  startSizes: WorkspaceLayoutSizes;
}

/** 本机 UI 偏好存储键，版本后缀用于未来布局模型变化时安全回退。 */
const WORKSPACE_LAYOUT_STORAGE_KEY = "orange.workspace-layout.v2";
/** 上一版布局存储键，读取一次迁移到 v2；迁移后旧的保留作回滚保险。 */
const LEGACY_WORKSPACE_LAYOUT_STORAGE_KEY = "orange.workspace-layout.v1";
/** 旧版本品牌命名遗留的存储键，用于一次性迁移到新键。 */
const LEGACY_BRAND_LAYOUT_STORAGE_KEY = "cici-note.workspace-layout.v1";
/** 浮窗时期的几何存储，只读取宽度和开关，用于迁回停靠工作台。 */
const AGENT_FLOAT_STORAGE_KEY = "orange.agent-float.v1";

/** 两个区域的默认显隐；默认全部展开，保持与升级前一致的观感。 */
const DEFAULT_REGION_OPEN_STATE: RegionOpenState = {
  sidebar: true,
  agent: true,
};

/** 各区域默认激活的条目，决定收起态图标栏初始高亮和浮层内容。 */
const DEFAULT_REGION_ACTIVE_ITEMS: RegionActiveItems = {
  sidebar: "sessions",
  agent: "main",
};

/** 默认布局沿用三栏宽度；Agent 默认展开，让协作区成为一等停靠栏。 */
const DEFAULT_WORKSPACE_LAYOUT: WorkspaceLayoutSizes = {
  sidebarWidth: 285,
  agentWidth: 380,
  agentOpen: true,
  regions: DEFAULT_REGION_OPEN_STATE,
  activeItems: DEFAULT_REGION_ACTIVE_ITEMS,
};

/** 知识库侧栏宽度边界，保证目录树可读且不会挤压编辑器。 */
const SIDEBAR_WIDTH_LIMIT = {
  min: 220,
  max: 420,
};

/** Agent 侧栏宽度边界，保证输入框、轨迹和变更卡可读。 */
const AGENT_WIDTH_LIMIT = {
  min: 300,
  max: 560,
};

/** 编辑器理想最小宽度；窄窗口时会动态下调，避免三栏把正文挤出容器。 */
const EDITOR_MIN_WIDTH = 500;

/** 分隔条轨道宽度，参与动态最大宽度计算，避免编辑器被实际轨道挤窄。 */
const RESIZER_TRACK_WIDTH = 10;

/** 键盘调整的基础步长；按住 Shift 时使用三倍步长快速调整。 */
const KEYBOARD_RESIZE_STEP = 16;

/** 未挂载工作台前的估算宽度，与全局最小窗口宽度保持一致。 */
const DEFAULT_WORKSPACE_WIDTH = 1180;

/** 读取工作台内容区宽度；grid 列宽按内容盒计算，不能把 padding 算进去。 */
function readWorkspaceContentWidth(element: HTMLElement) {
  const styles = window.getComputedStyle(element);
  const horizontalPadding =
    Number.parseFloat(styles.paddingLeft || "0") + Number.parseFloat(styles.paddingRight || "0");

  return Math.max(0, element.clientWidth - (Number.isFinite(horizontalPadding) ? horizontalPadding : 0));
}

/** 判断 localStorage 中解析出的值是否是可参与尺寸计算的数字。 */
function isFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

/** 将数值限制在给定区间；当动态上限低于下限时优先保留下限。 */
function clampNumber(value: number, min: number, max: number) {
  const safeMax = Math.max(min, max);

  return Math.min(Math.max(value, min), safeMax);
}

/**
 * 计算当前参与布局的轨道占位。
 *
 * 左栏收起时只占图标栏厚度，不再保留 resizer；编辑区收起时完全退出中间列。
 * 结果同时用于编辑器最小宽度、可用侧栏宽度和 grid 列定义，保证三者坐标系一致。
 */
function resolveTrackMetrics(regions: RegionOpenState) {
  const sidebarReserved = regions.sidebar
    ? { width: SIDEBAR_WIDTH_LIMIT.min, resizer: RESIZER_TRACK_WIDTH }
    : { width: REGION_RAIL_SIZE, resizer: 0 };
  const agentReserved = regions.agent
    ? { width: AGENT_WIDTH_LIMIT.min, resizer: RESIZER_TRACK_WIDTH }
    : { width: REGION_RAIL_SIZE, resizer: 0 };

  return {
    sidebarReserved,
    agentReserved,
    resizerCount: (sidebarReserved.resizer ? 1 : 0) + (agentReserved.resizer ? 1 : 0),
  };
}

/** 按当前容器和各区域显隐计算编辑器最小宽度，优先保住目录树和 Agent 下限。 */
function getEditorMinWidth(containerWidth: number, regions: RegionOpenState) {
  const { sidebarReserved, agentReserved, resizerCount } = resolveTrackMetrics(regions);
  const reserved = sidebarReserved.width + agentReserved.width + RESIZER_TRACK_WIDTH * resizerCount;
  const leftover = containerWidth - reserved;

  return clampNumber(leftover, 280, EDITOR_MIN_WIDTH);
}

/** 计算固定栏在当前容器中最多能占用的总宽度。 */
function getAvailableSidePanelWidth(containerWidth: number, regions: RegionOpenState) {
  const editorMin = getEditorMinWidth(containerWidth, regions);
  const { resizerCount } = resolveTrackMetrics(regions);

  return Math.max(0, containerWidth - RESIZER_TRACK_WIDTH * resizerCount - editorMin);
}

/** 从浮窗存储读取可迁移的宽度和开关；损坏时忽略，不影响停靠布局。 */
function readFloatMigration(): { agentWidth?: number; agentOpen?: boolean } {
  if (typeof window === "undefined") {
    return {};
  }

  try {
    const rawLayout = window.localStorage.getItem(AGENT_FLOAT_STORAGE_KEY);

    if (!rawLayout) {
      return {};
    }

    const parsedLayout = JSON.parse(rawLayout) as { width?: unknown; open?: unknown };
    const migrated: { agentWidth?: number; agentOpen?: boolean } = {};

    if (isFiniteNumber(parsedLayout.width)) {
      migrated.agentWidth = clampNumber(parsedLayout.width, AGENT_WIDTH_LIMIT.min, AGENT_WIDTH_LIMIT.max);
    }

    if (typeof parsedLayout.open === "boolean") {
      migrated.agentOpen = parsedLayout.open;
    }

    return migrated;
  } catch {
    return {};
  }
}

/** 读取本机保存的工作台布局；损坏、过期或不可访问时回退默认布局。 */
function readStoredWorkspaceLayout(): WorkspaceLayoutSizes {
  if (typeof window === "undefined") {
    return DEFAULT_WORKSPACE_LAYOUT;
  }

  try {
    const rawLayout = window.localStorage.getItem(WORKSPACE_LAYOUT_STORAGE_KEY);

    if (rawLayout) {
      return parseStoredWorkspaceLayout(rawLayout);
    }

    // v2 缺失时按 v1 → 品牌遗留键的顺序读一次旧数据，旧键保留不删，便于回滚。
    const legacyLayout =
      window.localStorage.getItem(LEGACY_WORKSPACE_LAYOUT_STORAGE_KEY) ??
      window.localStorage.getItem(LEGACY_BRAND_LAYOUT_STORAGE_KEY);

    if (legacyLayout) {
      window.localStorage.setItem(WORKSPACE_LAYOUT_STORAGE_KEY, legacyLayout);
      return parseStoredWorkspaceLayout(legacyLayout);
    }

    return parseStoredWorkspaceLayout("{}");
  } catch {
    return DEFAULT_WORKSPACE_LAYOUT;
  }
}

/** 解析区域显隐；缺字段回退默认，Agent 的旧布尔值由调用方单独兼容。 */
function parseRegionOpenState(parsed: Partial<WorkspaceLayoutSizes>, legacyAgentOpen?: boolean) {
  const nextState: RegionOpenState = { ...DEFAULT_REGION_OPEN_STATE };
  const rawRegions = parsed.regions;

  if (rawRegions && typeof rawRegions === "object") {
    for (const regionId of REGION_ORDER) {
      const value = (rawRegions as Partial<RegionOpenState>)[regionId];

      if (typeof value === "boolean") {
        nextState[regionId] = value;
      }
    }
  }

  // v1 只有 agentOpen 一个显隐字段，迁移时把它接到 agent 区域上。
  if (typeof legacyAgentOpen === "boolean") {
    nextState.agent = legacyAgentOpen;
  }

  return nextState;
}

/** 解析各区域激活条目；只接受该区域声明里存在的 id，避免脏数据点亮不存在的图标。 */
function parseActiveItems(parsed: Partial<WorkspaceLayoutSizes>): RegionActiveItems {
  const nextItems: RegionActiveItems = { ...DEFAULT_REGION_ACTIVE_ITEMS };
  const rawItems = parsed.activeItems;

  if (!rawItems || typeof rawItems !== "object") {
    return nextItems;
  }

  for (const regionId of REGION_ORDER) {
    const itemId = (rawItems as RegionActiveItems)[regionId];
    const region = COLLAPSIBLE_REGIONS[regionId];

    if (typeof itemId === "string" && region.items.some((item) => item.id === itemId)) {
      nextItems[regionId] = itemId;
    }
  }

  return nextItems;
}

/** 解析已读取的布局 JSON；缺字段时从浮窗存储补齐，再回退默认值。 */
function parseStoredWorkspaceLayout(rawLayout: string): WorkspaceLayoutSizes {
  const floatMigration = readFloatMigration();

  try {
    const parsedLayout = JSON.parse(rawLayout) as Partial<WorkspaceLayoutSizes>;
    const sidebarWidth = isFiniteNumber(parsedLayout.sidebarWidth)
      ? clampNumber(parsedLayout.sidebarWidth, SIDEBAR_WIDTH_LIMIT.min, SIDEBAR_WIDTH_LIMIT.max)
      : DEFAULT_WORKSPACE_LAYOUT.sidebarWidth;
    const agentWidth = isFiniteNumber(parsedLayout.agentWidth)
      ? clampNumber(parsedLayout.agentWidth, AGENT_WIDTH_LIMIT.min, AGENT_WIDTH_LIMIT.max)
      : (floatMigration.agentWidth ?? DEFAULT_WORKSPACE_LAYOUT.agentWidth);
    const legacyAgentOpen =
      typeof parsedLayout.agentOpen === "boolean" ? parsedLayout.agentOpen : floatMigration.agentOpen;
    const regions = parseRegionOpenState(parsedLayout, legacyAgentOpen);

    return {
      sidebarWidth,
      agentWidth,
      // agentOpen 保留为派生字段，供既有调用点继续读取。
      agentOpen: regions.agent,
      regions,
      activeItems: parseActiveItems(parsedLayout),
    };
  } catch {
    const regions = parseRegionOpenState({}, floatMigration.agentOpen);

    return {
      ...DEFAULT_WORKSPACE_LAYOUT,
      agentWidth: floatMigration.agentWidth ?? DEFAULT_WORKSPACE_LAYOUT.agentWidth,
      agentOpen: regions.agent,
      regions,
    };
  }
}

/** 保存本机工作台布局偏好；存储不可用时静默降级，不影响编辑器使用。 */
function saveWorkspaceLayout(sizes: WorkspaceLayoutSizes) {
  if (typeof window === "undefined") {
    return;
  }

  try {
    window.localStorage.setItem(WORKSPACE_LAYOUT_STORAGE_KEY, JSON.stringify(sizes));
  } catch {
    // localStorage 可能被隐私模式或 WebView 策略禁用，布局拖拽仍应继续可用。
  }
}

/** 根据当前容器宽度和正在调整的面板，把布局尺寸限制在安全范围内。 */
function clampWorkspaceLayoutSizes(
  nextSizes: WorkspaceLayoutSizes,
  containerWidth: number,
  activePane?: ResizablePane,
): WorkspaceLayoutSizes {
  const availableSidePanelWidth = getAvailableSidePanelWidth(containerWidth, nextSizes.regions);
  let sidebarWidth = clampNumber(nextSizes.sidebarWidth, SIDEBAR_WIDTH_LIMIT.min, SIDEBAR_WIDTH_LIMIT.max);
  let agentWidth = clampNumber(nextSizes.agentWidth, AGENT_WIDTH_LIMIT.min, AGENT_WIDTH_LIMIT.max);
  const regions = nextSizes.regions;
  // 宽度只在对应区域展开时才有意义；收起时保持原值，便于再次展开时沿用。
  const sidebarWidthInUse = regions.sidebar;
  const agentWidthInUse = regions.agent;

  if (!sidebarWidthInUse && !agentWidthInUse) {
    return { ...nextSizes, sidebarWidth, agentWidth };
  }

  if (!sidebarWidthInUse) {
    const maxAgentWidth = Math.min(AGENT_WIDTH_LIMIT.max, availableSidePanelWidth);

    return { ...nextSizes, sidebarWidth, agentWidth: clampNumber(agentWidth, AGENT_WIDTH_LIMIT.min, maxAgentWidth) };
  }

  if (!agentWidthInUse) {
    const maxSidebarWidth = Math.min(SIDEBAR_WIDTH_LIMIT.max, availableSidePanelWidth);

    return { ...nextSizes, sidebarWidth: clampNumber(sidebarWidth, SIDEBAR_WIDTH_LIMIT.min, maxSidebarWidth), agentWidth };
  }

  if (activePane === "sidebar") {
    const maxSidebarWidth = Math.min(SIDEBAR_WIDTH_LIMIT.max, availableSidePanelWidth - agentWidth);

    return { ...nextSizes, sidebarWidth: clampNumber(sidebarWidth, SIDEBAR_WIDTH_LIMIT.min, maxSidebarWidth), agentWidth };
  }

  if (activePane === "agent") {
    const maxAgentWidth = Math.min(AGENT_WIDTH_LIMIT.max, availableSidePanelWidth - sidebarWidth);

    return { ...nextSizes, sidebarWidth, agentWidth: clampNumber(agentWidth, AGENT_WIDTH_LIMIT.min, maxAgentWidth) };
  }

  if (sidebarWidth + agentWidth > availableSidePanelWidth) {
    // 容器变窄时优先压缩右侧 Agent，保留左侧目录树的可读宽度。
    agentWidth = clampNumber(availableSidePanelWidth - sidebarWidth, AGENT_WIDTH_LIMIT.min, AGENT_WIDTH_LIMIT.max);
  }

  if (sidebarWidth + agentWidth > availableSidePanelWidth) {
    sidebarWidth = clampNumber(availableSidePanelWidth - agentWidth, SIDEBAR_WIDTH_LIMIT.min, SIDEBAR_WIDTH_LIMIT.max);
  }

  return { ...nextSizes, sidebarWidth, agentWidth };
}

/** 切换单个区域显隐并重新夹取尺寸；agent 区域同步派生字段，兼容既有调用点。 */
function setRegionOpenState(
  currentSizes: WorkspaceLayoutSizes,
  regionId: RegionId,
  open: boolean,
  containerWidth: number,
): WorkspaceLayoutSizes {
  const regions = { ...currentSizes.regions, [regionId]: open };
  const nextSizes = { ...currentSizes, regions, agentOpen: regions.agent };

  return clampWorkspaceLayoutSizes(
    nextSizes,
    containerWidth,
    regionId === "sidebar" || regionId === "agent" ? regionId : undefined,
  );
}

/** 计算某个分隔条当前可表达的 aria 最小值和最大值；对应区域收起时返回 null，表示没有可用把手。 */
function getPaneBounds(pane: ResizablePane, sizes: WorkspaceLayoutSizes, containerWidth: number) {
  const availableSidePanelWidth = getAvailableSidePanelWidth(containerWidth, sizes.regions);

  if (pane === "sidebar") {
    if (!sizes.regions.sidebar) {
      return null;
    }

    const reserved = sizes.regions.agent ? sizes.agentWidth : 0;
    const dynamicMax = Math.min(SIDEBAR_WIDTH_LIMIT.max, availableSidePanelWidth - reserved);

    return {
      min: SIDEBAR_WIDTH_LIMIT.min,
      max: Math.max(SIDEBAR_WIDTH_LIMIT.min, dynamicMax),
    };
  }

  if (!sizes.regions.agent) {
    return null;
  }

  const dynamicMax = Math.min(AGENT_WIDTH_LIMIT.max, availableSidePanelWidth - sizes.sidebarWidth);

  return {
    min: AGENT_WIDTH_LIMIT.min,
    max: Math.max(AGENT_WIDTH_LIMIT.min, dynamicMax),
  };
}

/** 管理工作台三栏布局拖拽、键盘调整、Agent 停靠显隐和本机持久化。 */
export function useResizableWorkspaceLayout() {
  const [workspaceElement, setWorkspaceElement] = useState<HTMLElement | null>(null);
  const [workspaceWidth, setWorkspaceWidth] = useState(DEFAULT_WORKSPACE_WIDTH);
  const [sizes, setSizes] = useState<WorkspaceLayoutSizes>(() => readStoredWorkspaceLayout());
  const [resizingPane, setResizingPane] = useState<ResizablePane | null>(null);
  /** 当前工作台 DOM 节点，供原生 pointer 事件读取最新容器宽度。 */
  const workspaceElementRef = useRef<HTMLElement | null>(null);
  /** 内容区宽度，不含 padding，与 grid-template-columns 使用同一坐标系。 */
  const workspaceWidthRef = useRef(DEFAULT_WORKSPACE_WIDTH);
  /** 最新布局尺寸引用，避免 pointerdown 闭包拿到过期宽度。 */
  const sizesRef = useRef(sizes);
  /** 当前拖拽起点；为空表示没有处于拖拽会话。 */
  const dragStateRef = useRef<ResizeDragState | null>(null);

  /** 回调 ref 能在加载态切换到主工作台时重新挂载 ResizeObserver。 */
  const workspaceRef = useCallback((element: HTMLElement | null) => {
    workspaceElementRef.current = element;
    setWorkspaceElement(element);
  }, []);

  /** 获取当前工作台内容区宽度；观察器尚未回调时使用默认宽度。 */
  const getWorkspaceWidth = useCallback(() => {
    return workspaceWidthRef.current;
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

    /** 同步内容区宽度并顺手修正超出新窗口范围的已保存尺寸。 */
    function syncWorkspaceWidth(nextWidth: number) {
      workspaceWidthRef.current = nextWidth;
      setWorkspaceWidth(nextWidth);
      setSizes((currentSizes) => clampWorkspaceLayoutSizes(currentSizes, nextWidth));
    }

    syncWorkspaceWidth(readWorkspaceContentWidth(workspaceElement));

    if (typeof ResizeObserver === "undefined") {
      const handleWindowResize = () => syncWorkspaceWidth(readWorkspaceContentWidth(workspaceElement));

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

    /** 根据指针横向位移实时换算相邻面板宽度。 */
    function handlePointerMove(event: PointerEvent) {
      const dragState = dragStateRef.current;

      if (!dragState) {
        return;
      }

      const pointerDelta = event.clientX - dragState.startX;
      const nextSizes =
        dragState.pane === "sidebar"
          ? { ...dragState.startSizes, sidebarWidth: dragState.startSizes.sidebarWidth + pointerDelta }
          : { ...dragState.startSizes, agentWidth: dragState.startSizes.agentWidth - pointerDelta };

      event.preventDefault();
      setSizes(clampWorkspaceLayoutSizes(nextSizes, getWorkspaceWidth(), dragState.pane));
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

  /** 键盘调整分隔条前先确认该区域展开，收起态没有可拖拽的把手。 */
  const handleSeparatorKeyDown = useCallback(
    (pane: ResizablePane, event: ReactKeyboardEvent<HTMLDivElement>) => {
      const paneBounds = getPaneBounds(pane, sizesRef.current, getWorkspaceWidth());

      if (!paneBounds) {
        return;
      }
      const currentValue = pane === "sidebar" ? sizesRef.current.sidebarWidth : sizesRef.current.agentWidth;
      const step = event.shiftKey ? KEYBOARD_RESIZE_STEP * 3 : KEYBOARD_RESIZE_STEP;
      let nextValue: number | null = null;

      if (event.key === "Home") {
        nextValue = paneBounds.min;
      } else if (event.key === "End") {
        nextValue = paneBounds.max;
      } else if (event.key === "ArrowLeft") {
        nextValue = pane === "sidebar" ? currentValue - step : currentValue + step;
      } else if (event.key === "ArrowRight") {
        nextValue = pane === "sidebar" ? currentValue + step : currentValue - step;
      }

      if (nextValue === null) {
        return;
      }

      event.preventDefault();
      setSizes((currentSizes) => {
        const nextSizes =
          pane === "sidebar"
            ? { ...currentSizes, sidebarWidth: nextValue }
            : { ...currentSizes, agentWidth: nextValue };

        return clampWorkspaceLayoutSizes(nextSizes, getWorkspaceWidth(), pane);
      });
    },
    [getWorkspaceWidth],
  );

  /** 双击分隔条只恢复对应栏的默认宽度，不改变 Agent 显隐。 */
  const handleSeparatorDoubleClick = useCallback((pane: ResizablePane) => {
    setSizes((currentSizes) => {
      const nextSizes =
        pane === "sidebar"
          ? { ...currentSizes, sidebarWidth: DEFAULT_WORKSPACE_LAYOUT.sidebarWidth }
          : { ...currentSizes, agentWidth: DEFAULT_WORKSPACE_LAYOUT.agentWidth };

      return clampWorkspaceLayoutSizes(nextSizes, getWorkspaceWidth(), pane);
    });
  }, [getWorkspaceWidth]);

  /** 切换某个可收起区域的显隐；再次展开时沿用上次宽度或默认宽度。 */
  const setRegionOpen = useCallback((regionId: RegionId, open: boolean) => {
    setSizes((currentSizes) => setRegionOpenState(currentSizes, regionId, open, getWorkspaceWidth()));
  }, [getWorkspaceWidth]);

  /** 既有调用点仍按布尔值开关 Agent 区域，这里保持旧签名。 */
  const setAgentOpen = useCallback((open: boolean) => {
    setRegionOpen("agent", open);
  }, [setRegionOpen]);

  /** 在收起按钮、顶栏图标和区域内的收起入口之间复用同一个开关。 */
  const toggleRegion = useCallback((regionId: RegionId) => {
    setSizes((currentSizes) => setRegionOpenState(currentSizes, regionId, !currentSizes.regions[regionId], getWorkspaceWidth()));
  }, [getWorkspaceWidth]);

  /** 记录某个区域图标栏上当前激活的条目，决定悬浮浮层预览哪一块内容。 */
  const setRegionActiveItem = useCallback((regionId: RegionId, itemId: RegionItemId) => {
    setSizes((currentSizes) => {
      if (currentSizes.activeItems[regionId] === itemId) {
        return currentSizes;
      }

      return { ...currentSizes, activeItems: { ...currentSizes.activeItems, [regionId]: itemId } };
    });
  }, []);

  /** 主 grid 的列定义由 hook 统一生成；收起区域退化为图标栏，中间编辑区始终占剩余空间。 */
  const gridTemplateColumns = useMemo(() => {
    const editorMin = getEditorMinWidth(workspaceWidth, sizes.regions);
    const tracks: string[] = [];

    if (sizes.regions.sidebar) {
      tracks.push(`${sizes.sidebarWidth}px`, `${RESIZER_TRACK_WIDTH}px`);
    } else {
      tracks.push(`${REGION_RAIL_SIZE}px`);
    }

    tracks.push(`minmax(${editorMin}px, 1fr)`);

    if (sizes.regions.agent) {
      tracks.push(`${RESIZER_TRACK_WIDTH}px`, `${sizes.agentWidth}px`);
    } else {
      tracks.push(`${REGION_RAIL_SIZE}px`);
    }

    return tracks.join(" ");
  }, [sizes.agentWidth, sizes.regions, sizes.sidebarWidth, workspaceWidth]);

  /** 生成分隔条需要的交互和 aria 属性，保持两个把手行为一致。 */
  const getSeparatorProps = useCallback(
    (pane: ResizablePane) => {
      const paneBounds = getPaneBounds(pane, sizes, workspaceWidth);
      const currentValue = pane === "sidebar" ? sizes.sidebarWidth : sizes.agentWidth;

      return {
        role: "separator",
        tabIndex: 0,
        "aria-label": pane === "sidebar" ? "调整知识库侧栏宽度" : "调整 Agent 侧栏宽度",
        "aria-orientation": "vertical" as const,
        "aria-valuemin": paneBounds?.min ?? 0,
        "aria-valuemax": paneBounds?.max ?? 0,
        "aria-valuenow": Math.round(currentValue),
        onDoubleClick: () => handleSeparatorDoubleClick(pane),
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
    regionOpen: sizes.regions,
    activeItems: sizes.activeItems,
    /** 各区域展开态宽度，供收起态浮层按真实宽度渲染预览。 */
    regionPreviewWidth: {
      sidebar: sizes.sidebarWidth,
      agent: sizes.agentWidth,
    },
    setRegionOpen,
    toggleRegion,
    setRegionActiveItem,
    agentOpen: sizes.regions.agent,
    setAgentOpen,
  };
}
