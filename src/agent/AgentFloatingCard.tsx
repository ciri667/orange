import type { PointerEvent as ReactPointerEvent, ReactNode } from "react";
import type { AgentFloatLayout, AgentFloatResizeEdge } from "./agentFloatGeometry";

const RESIZE_EDGES: AgentFloatResizeEdge[] = ["n", "s", "e", "w", "ne", "nw", "se", "sw"];

/** 非模态 Agent 浮窗外壳：定位与八向缩放；拖动由内部 AgentPanel header 触发。 */
export function AgentFloatingCard({
  layout,
  isInteracting,
  onBeginResize,
  children,
}: {
  layout: AgentFloatLayout;
  isInteracting: boolean;
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
