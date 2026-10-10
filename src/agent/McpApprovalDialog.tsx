import { useEffect, useRef, useState } from "react";
import { Loader2, ShieldAlert } from "lucide-react";
import { Button } from "../shared/Button";
import { ModalBackdrop, ModalForm } from "../shared/Modal";
import { decideMcpToolCall, subscribeMcpApprovals } from "../shared/api/mcp";
import type { McpApprovalRequest } from "../shared/types";

/**
 * MCP 工具调用确认弹窗。
 *
 * 进阶级的 Agent 回合调用非只读 MCP 工具时，后端发出 mcp-tool-approval 事件并等待选择；
 * 这里把允许或拒绝送回 decide_mcp_tool_call。多条请求排队逐个展示，关闭视为拒绝。
 */
export function McpApprovalDialog() {
  /** 等待用户选择的确认请求，按事件到达顺序排队。 */
  const [queue, setQueue] = useState<McpApprovalRequest[]>([]);
  /** 正在发送决定的确认请求 id，防止重复点击。 */
  const [decidingId, setDecidingId] = useState("");
  /** 异步回来时按 id 出队，避免用过期的队列快照覆盖新到达的请求。 */
  const queueRef = useRef<McpApprovalRequest[]>([]);
  queueRef.current = queue;

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;

    void subscribeMcpApprovals((request) => {
      if (!disposed) {
        setQueue((previous) => [...previous, request]);
      }
    }).then((dispose) => {
      if (disposed) {
        dispose();
      } else {
        unlisten = dispose;
      }
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  /** 发送决定并从队列移除该请求；后端说“确认已结束”时同样静默出队。 */
  async function handleDecide(request: McpApprovalRequest, approved: boolean) {
    setDecidingId(request.approvalId);
    try {
      await decideMcpToolCall({ approvalId: request.approvalId, approved });
    } catch {
      // 回合已停止或超时时后端会报错，用户意图已达成，直接出队即可。
    } finally {
      setDecidingId("");
      setQueue((previous) => previous.filter((item) => item.approvalId !== request.approvalId));
    }
  }

  const current = queue[0];
  if (!current) {
    return null;
  }
  const busy = decidingId === current.approvalId;
  const pendingCount = queue.length - 1;

  return (
    <ModalBackdrop onClose={() => void handleDecide(current, false)}>
      <ModalForm aria-label="MCP 工具调用确认" onSubmit={(event) => event.preventDefault()}>
        <header className="flex items-start gap-2.5">
          <span className="mt-0.5 shrink-0 text-warning">
            <ShieldAlert size={18} />
          </span>
          <div className="min-w-0">
            <h3 className="m-0 text-sm font-semibold text-ink-strong">Agent 请求调用 MCP 工具</h3>
            <p className="mt-1 mb-0 text-xs leading-[1.55] text-ink-muted">
              服务器 <strong className="text-ink">{current.serverName}</strong> 的工具{" "}
              <code className="font-mono text-ink">{current.toolName}</code> 不是只读工具，需要你的允许才会执行。
            </p>
          </div>
        </header>

        <div className="grid gap-1.5 rounded-xl border border-border bg-surface-muted p-3">
          <p className="m-0 text-[11px] font-medium text-ink-soft">调用参数预览</p>
          <pre className="m-0 max-h-48 overflow-auto font-mono text-xs leading-[1.5] whitespace-pre-wrap text-ink [overflow-wrap:anywhere]">
            {current.argumentsPreview || "（无参数）"}
          </pre>
          {pendingCount > 0 ? <p className="m-0 text-[11px] text-ink-soft">后面还有 {pendingCount} 个调用等待确认。</p> : null}
          <p className="m-0 text-[11px] text-ink-soft">拒绝或等待超时都会跳过这次调用；关闭本窗口按拒绝处理。</p>
        </div>

        <footer className="flex items-center justify-end gap-2">
          <Button variant="ghost" tone="danger" type="button" disabled={busy} onClick={() => void handleDecide(current, false)}>
            拒绝
          </Button>
          <Button variant="primary" type="button" disabled={busy} onClick={() => void handleDecide(current, true)}>
            {busy ? <Loader2 size={14} className="animate-spin" /> : null}
            允许执行
          </Button>
        </footer>
      </ModalForm>
    </ModalBackdrop>
  );
}
