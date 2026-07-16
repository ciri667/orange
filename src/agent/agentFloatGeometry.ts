/** Agent 浮窗几何与本机持久化（纯函数，无 React）。 */

export interface AgentFloatLayout {
  open: boolean;
  x: number;
  y: number;
  width: number;
  height: number;
}

export type AgentFloatResizeEdge =
  | "n"
  | "s"
  | "e"
  | "w"
  | "ne"
  | "nw"
  | "se"
  | "sw";

export const AGENT_FLOAT_STORAGE_KEY = "orange.agent-float.v1";
/** 从旧三栏布局读取默认宽度的键（只读迁移，不写回 agent 列）。 */
export const WORKSPACE_LAYOUT_STORAGE_KEY = "orange.workspace-layout.v1";

export const AGENT_FLOAT_MARGIN = 16;
export const AGENT_FLOAT_MIN_WIDTH = 300;
export const AGENT_FLOAT_MIN_HEIGHT = 360;
export const AGENT_FLOAT_DEFAULT_WIDTH = 380;

/** 视口安全矩形：四周留白，卡片不得超出。 */
export interface ViewportSafeRect {
  left: number;
  top: number;
  right: number;
  bottom: number;
  width: number;
  height: number;
}

export function isFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

export function clampNumber(value: number, min: number, max: number): number {
  const safeMax = Math.max(min, max);
  return Math.min(Math.max(value, min), safeMax);
}

/** 以 window 为坐标系的安全矩形（TopBar 下沿用 margin 近似）。 */
export function getViewportSafeRect(
  viewportWidth: number,
  viewportHeight: number,
  margin = AGENT_FLOAT_MARGIN,
): ViewportSafeRect {
  const left = margin;
  const top = margin;
  const right = Math.max(left, viewportWidth - margin);
  const bottom = Math.max(top, viewportHeight - margin);
  return {
    left,
    top,
    right,
    bottom,
    width: Math.max(0, right - left),
    height: Math.max(0, bottom - top),
  };
}

/** 从旧 workspace-layout 迁移 agentWidth，失败则默认 380。 */
export function readMigratedDefaultWidth(): number {
  if (typeof window === "undefined") {
    return AGENT_FLOAT_DEFAULT_WIDTH;
  }
  try {
    const raw = window.localStorage.getItem(WORKSPACE_LAYOUT_STORAGE_KEY);
    if (!raw) {
      return AGENT_FLOAT_DEFAULT_WIDTH;
    }
    const parsed = JSON.parse(raw) as { agentWidth?: unknown };
    if (!isFiniteNumber(parsed.agentWidth)) {
      return AGENT_FLOAT_DEFAULT_WIDTH;
    }
    return clampNumber(parsed.agentWidth, AGENT_FLOAT_MIN_WIDTH, 560);
  } catch {
    return AGENT_FLOAT_DEFAULT_WIDTH;
  }
}

/** 默认：收起；靠右；宽度 380（或迁移）；高度接近视口全高。 */
export function createDefaultAgentFloatLayout(
  viewportWidth: number,
  viewportHeight: number,
  preferredWidth = readMigratedDefaultWidth(),
): AgentFloatLayout {
  const safe = getViewportSafeRect(viewportWidth, viewportHeight);
  const width = clampNumber(preferredWidth, AGENT_FLOAT_MIN_WIDTH, safe.width);
  const height = clampNumber(safe.height, AGENT_FLOAT_MIN_HEIGHT, safe.height);
  const x = safe.right - width;
  const y = safe.top;
  return { open: false, x, y, width, height };
}

/** 将布局夹紧到视口内，保持宽高不低于最小值。 */
export function clampAgentFloatLayout(
  layout: AgentFloatLayout,
  viewportWidth: number,
  viewportHeight: number,
): AgentFloatLayout {
  const safe = getViewportSafeRect(viewportWidth, viewportHeight);
  const width = clampNumber(layout.width, AGENT_FLOAT_MIN_WIDTH, safe.width);
  const height = clampNumber(layout.height, AGENT_FLOAT_MIN_HEIGHT, safe.height);
  const maxX = safe.right - width;
  const maxY = safe.bottom - height;
  const x = clampNumber(layout.x, safe.left, Math.max(safe.left, maxX));
  const y = clampNumber(layout.y, safe.top, Math.max(safe.top, maxY));
  return {
    open: layout.open,
    x,
    y,
    width,
    height,
  };
}

/**
 * 根据拖动中的边/角与指针位移，从 start 布局算出新布局。
 * dx/dy 为 pointer 相对 pointerdown 的位移（client 坐标）。
 */
export function applyResizeDelta(
  start: AgentFloatLayout,
  edge: AgentFloatResizeEdge,
  dx: number,
  dy: number,
  viewportWidth: number,
  viewportHeight: number,
): AgentFloatLayout {
  let { x, y, width, height } = start;
  const touchesW = edge.includes("w");
  const touchesE = edge.includes("e");
  const touchesN = edge.includes("n");
  const touchesS = edge.includes("s");

  if (touchesE) {
    width = start.width + dx;
  }
  if (touchesS) {
    height = start.height + dy;
  }
  if (touchesW) {
    width = start.width - dx;
    x = start.x + dx;
  }
  if (touchesN) {
    height = start.height - dy;
    y = start.y + dy;
  }

  // 先按最小值修正，并回推 x/y，避免西/北边缩放时锚点漂移出预期。
  if (width < AGENT_FLOAT_MIN_WIDTH) {
    if (touchesW) {
      x = start.x + start.width - AGENT_FLOAT_MIN_WIDTH;
    }
    width = AGENT_FLOAT_MIN_WIDTH;
  }
  if (height < AGENT_FLOAT_MIN_HEIGHT) {
    if (touchesN) {
      y = start.y + start.height - AGENT_FLOAT_MIN_HEIGHT;
    }
    height = AGENT_FLOAT_MIN_HEIGHT;
  }

  return clampAgentFloatLayout(
    { open: start.open, x, y, width, height },
    viewportWidth,
    viewportHeight,
  );
}

/** 移动：仅改 x/y，尺寸不变。 */
export function applyMoveDelta(
  start: AgentFloatLayout,
  dx: number,
  dy: number,
  viewportWidth: number,
  viewportHeight: number,
): AgentFloatLayout {
  return clampAgentFloatLayout(
    {
      ...start,
      x: start.x + dx,
      y: start.y + dy,
    },
    viewportWidth,
    viewportHeight,
  );
}

export function parseStoredAgentFloatLayout(
  raw: string,
  viewportWidth: number,
  viewportHeight: number,
): AgentFloatLayout | null {
  try {
    const parsed = JSON.parse(raw) as Partial<AgentFloatLayout>;
    if (
      typeof parsed.open !== "boolean" ||
      !isFiniteNumber(parsed.x) ||
      !isFiniteNumber(parsed.y) ||
      !isFiniteNumber(parsed.width) ||
      !isFiniteNumber(parsed.height)
    ) {
      return null;
    }
    return clampAgentFloatLayout(
      {
        open: parsed.open,
        x: parsed.x,
        y: parsed.y,
        width: parsed.width,
        height: parsed.height,
      },
      viewportWidth,
      viewportHeight,
    );
  } catch {
    return null;
  }
}

export function readStoredAgentFloatLayout(
  viewportWidth: number,
  viewportHeight: number,
): AgentFloatLayout {
  const fallback = createDefaultAgentFloatLayout(viewportWidth, viewportHeight);
  if (typeof window === "undefined") {
    return fallback;
  }
  try {
    const raw = window.localStorage.getItem(AGENT_FLOAT_STORAGE_KEY);
    if (!raw) {
      return fallback;
    }
    return parseStoredAgentFloatLayout(raw, viewportWidth, viewportHeight) ?? fallback;
  } catch {
    return fallback;
  }
}

export function saveAgentFloatLayout(layout: AgentFloatLayout): void {
  if (typeof window === "undefined") {
    return;
  }
  try {
    window.localStorage.setItem(AGENT_FLOAT_STORAGE_KEY, JSON.stringify(layout));
  } catch {
    // 隐私模式等：忽略，拖拽仍可用。
  }
}
