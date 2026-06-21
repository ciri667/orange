import { Check, X } from "lucide-react";
import type { ProposedChange } from "../shared/types";

/** Agent 变更确认面板，确保所有写入都经过用户确认。 */
export function DiffPanel({
  change,
  onAccept,
  onReject,
  isBusy,
}: {
  change: ProposedChange;
  onAccept: () => void;
  onReject: () => void;
  isBusy: boolean;
}) {
  return (
    <aside className="diff-panel" aria-label="Agent 变更预览">
      <div className="diff-header">
        <div>
          <p className="section-label">Agent 建议写入</p>
          <h3>{change.title}</h3>
          <span>{change.targetPath}</span>
        </div>
        <div className="diff-actions">
          <button className="ghost-button" type="button" onClick={onReject} disabled={isBusy}>
            <X size={16} />
            取消
          </button>
          <button className="primary-button compact" type="button" onClick={onAccept} disabled={isBusy}>
            <Check size={16} />
            确认写入
          </button>
        </div>
      </div>
      <div className="diff-grid">
        <div className="diff-column removed">
          <span>- 原文</span>
          <p>{change.original || "新建文件，暂无原文。"}</p>
        </div>
        <div className="diff-column added">
          <span>+ 建议</span>
          <p>{change.next}</p>
        </div>
      </div>
    </aside>
  );
}
