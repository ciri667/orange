import { listen } from "@tauri-apps/api/event";
import { invokeLogged, isTauriRuntime } from "./runtime";

export interface OperationApprovalRequest {
  id: string;
  sessionId: string;
  permissionVersion: number;
  toolName: string;
  arguments: Record<string, unknown>;
  warning: string;
}
/** 只提交决定，不能替换后端待执行参数。 */
export async function decideOperation(id: string, approved: boolean) {
  if (isTauriRuntime()) await invokeLogged<void>("decide_operation_approval", { id, approved });
}
export async function subscribeOperationApprovals(onRequest: (request: OperationApprovalRequest) => void, onClosed: (id: string) => void) {
  if (!isTauriRuntime()) return () => {};
  const requests = await listen<OperationApprovalRequest>("operation-approval", (event) => onRequest(event.payload));
  try {
    const closed = await listen<string>("operation-approval-closed", (event) => onClosed(event.payload));
    return () => { requests(); closed(); };
  } catch (error) { requests(); throw error; }
}
