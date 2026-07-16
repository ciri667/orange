import { useCallback, useEffect, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import {
  applyMoveDelta,
  applyResizeDelta,
  type AgentFloatLayout,
  type AgentFloatResizeEdge,
  clampAgentFloatLayout,
  readStoredAgentFloatLayout,
  saveAgentFloatLayout,
} from "./agentFloatGeometry";

type DragKind =
  | { type: "move"; startX: number; startY: number; startLayout: AgentFloatLayout }
  | {
      type: "resize";
      edge: AgentFloatResizeEdge;
      startX: number;
      startY: number;
      startLayout: AgentFloatLayout;
    };

function getViewportSize() {
  if (typeof window === "undefined") {
    return { width: 1280, height: 800 };
  }
  return { width: window.innerWidth, height: window.innerHeight };
}

/** Agent 浮窗开关、几何、拖拽移动/缩放与本机持久化。 */
export function useAgentFloatLayout() {
  const [layout, setLayout] = useState<AgentFloatLayout>(() => {
    const { width, height } = getViewportSize();
    return readStoredAgentFloatLayout(width, height);
  });
  const layoutRef = useRef(layout);
  const dragRef = useRef<DragKind | null>(null);
  const [isInteracting, setIsInteracting] = useState(false);

  useEffect(() => {
    layoutRef.current = layout;
  }, [layout]);

  useEffect(() => {
    saveAgentFloatLayout(layout);
  }, [layout]);

  /** 视口变化时把卡片夹回安全区。 */
  useEffect(() => {
    function handleResize() {
      const { width, height } = getViewportSize();
      setLayout((current) => clampAgentFloatLayout(current, width, height));
    }
    window.addEventListener("resize", handleResize);
    return () => window.removeEventListener("resize", handleResize);
  }, []);

  useEffect(() => {
    if (!isInteracting) {
      return undefined;
    }

    function handlePointerMove(event: PointerEvent) {
      const drag = dragRef.current;
      if (!drag) {
        return;
      }
      const { width, height } = getViewportSize();
      const dx = event.clientX - drag.startX;
      const dy = event.clientY - drag.startY;
      event.preventDefault();
      if (drag.type === "move") {
        setLayout(applyMoveDelta(drag.startLayout, dx, dy, width, height));
        return;
      }
      setLayout(
        applyResizeDelta(drag.startLayout, drag.edge, dx, dy, width, height),
      );
    }

    function stopDrag() {
      dragRef.current = null;
      setIsInteracting(false);
      if (typeof document !== "undefined") {
        document.body.classList.remove("agent-float-interacting");
      }
    }

    window.addEventListener("pointermove", handlePointerMove);
    window.addEventListener("pointerup", stopDrag);
    window.addEventListener("pointercancel", stopDrag);
    return () => {
      window.removeEventListener("pointermove", handlePointerMove);
      window.removeEventListener("pointerup", stopDrag);
      window.removeEventListener("pointercancel", stopDrag);
      if (typeof document !== "undefined") {
        document.body.classList.remove("agent-float-interacting");
      }
    };
  }, [isInteracting]);

  const setOpen = useCallback((open: boolean) => {
    setLayout((current) => ({ ...current, open }));
  }, []);

  const toggleOpen = useCallback(() => {
    setLayout((current) => ({ ...current, open: !current.open }));
  }, []);

  const beginMove = useCallback((event: ReactPointerEvent) => {
    if (event.pointerType === "mouse" && event.button !== 0) {
      return;
    }
    // 仅主按钮；由外壳在 header 空白区调用。
    dragRef.current = {
      type: "move",
      startX: event.clientX,
      startY: event.clientY,
      startLayout: layoutRef.current,
    };
    setIsInteracting(true);
    if (typeof document !== "undefined") {
      document.body.classList.add("agent-float-interacting");
    }
    event.preventDefault();
  }, []);

  const beginResize = useCallback((edge: AgentFloatResizeEdge, event: ReactPointerEvent) => {
    if (event.pointerType === "mouse" && event.button !== 0) {
      return;
    }
    dragRef.current = {
      type: "resize",
      edge,
      startX: event.clientX,
      startY: event.clientY,
      startLayout: layoutRef.current,
    };
    setIsInteracting(true);
    if (typeof document !== "undefined") {
      document.body.classList.add("agent-float-interacting");
    }
    event.preventDefault();
    event.stopPropagation();
  }, []);

  return {
    layout,
    isInteracting,
    setOpen,
    toggleOpen,
    beginMove,
    beginResize,
  };
}
