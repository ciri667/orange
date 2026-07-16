import type { PointerEvent as ReactPointerEvent, ReactNode } from "react";
import type { AgentFloatLayout, AgentFloatResizeEdge } from "./agentFloatGeometry";

const RESIZE_EDGES: AgentFloatResizeEdge[] = ["n", "s", "e", "w", "ne", "nw", "se", "sw"];

/** 非模态 Agent 浮窗外壳：定位、header 拖动、八向缩放；业务内容由 children 提供。 */
export function AgentFloatingCard({
  layout,
  isInteracting,
  onBeginMove,
  onBeginResize,
  children,
}: {
  layout: AgentFloatLayout;
  isInteracting: boolean;
  onBeginMove: (event: ReactPointerEvent) => void;
  onBeginResize: (edge: AgentFloatResizeEdge, event: ReactPointerEvent) => void;
  children: ReactNode;
}) {
  if (!layout.open) {
    return null;
  }

  return (
    <div
      className={`agent-float-card ${isInteracting ? "is-interacting" : ""}`}
      style={{
        left: layout.x,
        top: layout.y,
        width: layout.width,
        height: layout.height,
      }}
      role="complementary"
      aria-label="Agent 协作浮窗"
    >
      {/* 透明拖动手柄叠在 header 区域：仅空白处拖动由 AgentPanel 配合，
          这里提供整卡顶部条作为默认拖区；按钮仍可点是因为 header actions z-index 更高。 */}
      <div
        className="agent-float-drag-region"
        onPointerDown={onBeginMove}
        aria-hidden="true"
      />
      <div className="agent-float-card-body">{children}</div>
      {RESIZE_EDGES.map((edge) => (
        <div
          key={edge}
          className={`agent-float-resize-handle agent-float-resize-${edge}`}
          onPointerDown={(event) => onBeginResize(edge, event)}
        />
      ))}
    </div>
  );
}
