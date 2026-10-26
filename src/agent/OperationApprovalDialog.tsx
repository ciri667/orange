import { useEffect, useRef, useState } from "react";
import { Button } from "../shared/Button";
import { ModalBackdrop, ModalForm } from "../shared/Modal";
import { decideOperation, subscribeOperationApprovals, type OperationApprovalRequest } from "../shared/api/operationApproval";

/** 所有外部能力共用审批队列；后端关闭事件移除收权、超时及取消的请求。 */
export function OperationApprovalDialog() {
  const [queue, setQueue] = useState<OperationApprovalRequest[]>([]);
  const [busy, setBusy] = useState(false);
  const queueRef = useRef(queue);
  queueRef.current = queue;
  useEffect(() => {
    let disposed = false;
    let dispose: (() => void) | undefined;
    void subscribeOperationApprovals(
      (request) => { if (!disposed) setQueue((old) => old.some((item) => item.id === request.id) ? old : [...old, request]); },
      (id) => { if (!disposed) setQueue((old) => old.filter((item) => item.id !== id)); },
    ).then((cleanup) => { if (disposed) cleanup(); else dispose = cleanup; });
    return () => {
      disposed = true; dispose?.();
      for (const request of queueRef.current) void decideOperation(request.id, false).catch(() => {});
    };
  }, []);
  const current = queue[0];
  if (!current) return null;
  async function decide(approved: boolean) {
    if (!current) return;
    setBusy(true);
    try { await decideOperation(current.id, approved); }
    finally { setQueue((old) => old.filter((item) => item.id !== current.id)); setBusy(false); }
  }
  return <ModalBackdrop onClose={() => void decide(false).catch(() => {})}>
    <ModalForm aria-label="Agent 操作审批" onSubmit={(event) => event.preventDefault()}>
      <h3 className="m-0 text-sm font-semibold text-ink">允许本次 {current.toolName} 操作</h3>
      <p className="text-xs text-ink-muted">会话 {current.sessionId} · 权限版本 {current.permissionVersion}</p>
      <pre className="max-h-80 overflow-auto rounded-control bg-surface-muted p-3 text-xs whitespace-pre-wrap break-all text-ink">{JSON.stringify(current.arguments, null, 2)}</pre>
      <p className="text-xs leading-relaxed text-ink-muted">{current.warning}</p>
      <p className="text-xs text-ink-muted">关闭、拒绝、停止或等待 300 秒超时均不执行。批准仅对本次请求有效。</p>
      <footer className="flex justify-end gap-2">
        <Button variant="ghost" disabled={busy} onClick={() => void decide(false).catch(() => {})}>拒绝</Button>
        <Button variant="primary" disabled={busy} onClick={() => void decide(true).catch(() => {})}>允许本次操作</Button>
      </footer>
    </ModalForm>
  </ModalBackdrop>;
}
