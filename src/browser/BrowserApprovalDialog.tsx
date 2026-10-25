import { ConfirmDialog } from "../shared/ConfirmDialog";
import { decideBrowserOperation } from "../shared/tauriApi";
import type { BrowserWorkspace } from "./useBrowserWorkspace";

/** 审批不属于某个网页的可见 DOM，隐藏浏览器时也必须能回应。 */
export function BrowserApprovalDialog({ browser }: { browser: BrowserWorkspace }) {
  const request = browser.approval;
  if (!request) return null;
  const decide = (approved: boolean) => void browser.runAction(async () => {
    await decideBrowserOperation({ approvalId: request.approvalId, approved });
    browser.setApproval(null);
  }).catch(() => undefined);
  return <ConfirmDialog title="Agent 请求执行浏览器操作" message={`操作：${request.operation}${request.elementSummary ? `；目标：${request.elementSummary}` : ""}${browser.error ? `；${browser.error}` : ""}`} confirmLabel="允许一次" cancelLabel="拒绝" isBusy={browser.busy} onConfirm={() => decide(true)} onCancel={() => decide(false)} />;
}
