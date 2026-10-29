import { useLayoutEffect, useRef } from "react";
import { browserFocusSurface, browserSetSurfaceBounds, browserSetSurfaceVisible } from "../shared/tauriApi";

const MIN_SURFACE_SIZE = 24;
const OCCLUDER = "[data-native-surface-occluder]";
type Bounds = Parameters<typeof browserSetSurfaceBounds>[0];
interface SurfaceView { visible: boolean; bounds?: Bounds }

/** 原生 HWND 无法被 DOM 的 z-index 遮住；模态层和菜单出现时主动让出整个 stage。 */
function hasOccluder() {
  return Array.from(document.querySelectorAll<HTMLElement>(OCCLUDER))
    .some((element) => element.getClientRects().length > 0);
}

/** 只同步几何与显隐，浏览器输入由 Chromium 原生接收。 */
export function BrowserSurfaceHost({ active, available }: { active: boolean; available: boolean }) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const desired = useRef<SurfaceView>({ visible: false });
  const syncing = useRef(false);

  useLayoutEffect(() => {
    let frame: number | null = null;
    let disposed = false;

    // 串行且合并中间布局；弹层打开后，旧 bounds 的异步完成不能再次开启窗口。
    const requestView = (view: SurfaceView) => {
      desired.current = view;
      if (syncing.current) return;
      syncing.current = true;
      void (async () => {
        let applied: SurfaceView;
        do {
          applied = desired.current;
          if (applied.visible && applied.bounds) {
            await browserSetSurfaceBounds(applied.bounds);
            if (applied !== desired.current) continue;
          }
          await browserSetSurfaceVisible(applied.visible);
        } while (applied !== desired.current);
      })().catch(() => undefined).finally(() => { syncing.current = false; });
    };

    const sync = () => {
      const rect = hostRef.current?.getBoundingClientRect();
      const visible = !disposed && active && available && document.visibilityState === "visible"
        && !hasOccluder() && rect && rect.width >= MIN_SURFACE_SIZE && rect.height >= MIN_SURFACE_SIZE;
      requestView(visible ? {
        visible: true,
        bounds: { x: rect.left, y: rect.top, width: rect.width, height: rect.height, scale: window.devicePixelRatio || 1 },
      } : { visible: false });
    };
    const schedule = () => {
      if (frame === null) frame = window.requestAnimationFrame(() => { frame = null; sync(); });
    };

    // 首次渲染也发送 false；后端会在 Chromium 尚未启动时保存这个意图。
    sync();
    const resize = new ResizeObserver(schedule);
    if (hostRef.current) resize.observe(hostRef.current);
    const overlays = new MutationObserver((records) => {
      // 过滤文本流、页签标题等普通 DOM 更新，避免 Agent 输出触发浏览器布局 IPC。
      const relevant = records.some((record) => record.type === "attributes"
        || [...record.addedNodes, ...record.removedNodes].some((node) => node instanceof Element
          && (node.matches(OCCLUDER) || node.querySelector(OCCLUDER))));
      if (relevant) sync();
    });
    overlays.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ["hidden", "aria-hidden"] });
    window.addEventListener("resize", schedule);
    window.addEventListener("scroll", schedule, true);
    document.addEventListener("visibilitychange", sync);
    return () => {
      disposed = true;
      resize.disconnect();
      overlays.disconnect();
      window.removeEventListener("resize", schedule);
      window.removeEventListener("scroll", schedule, true);
      document.removeEventListener("visibilitychange", sync);
      if (frame !== null) window.cancelAnimationFrame(frame);
      requestView({ visible: false });
    };
  }, [active, available]);

  return <div ref={hostRef} className="browser-stage browser-surface-host"
    onMouseDown={() => void browserFocusSurface().catch(() => undefined)} />;
}
