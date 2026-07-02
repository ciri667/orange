import { AlertTriangle, ShieldCheck } from "lucide-react";

/** 确认弹窗语义类型，danger 用于删除、移除授权等不可直接撤销的操作。 */
export type ConfirmDialogTone = "default" | "danger";

/** 应用内确认弹窗的展示配置，不依赖浏览器 window.confirm 或 Tauri dialog 权限。 */
export interface ConfirmDialogConfig {
  title: string;
  message: string;
  confirmLabel: string;
  cancelLabel?: string;
  tone?: ConfirmDialogTone;
}

/** 应用内确认弹窗属性，调用方负责执行确认后的真实业务动作。 */
interface ConfirmDialogProps extends ConfirmDialogConfig {
  isBusy?: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}

/** 统一的应用内确认弹窗，避免 WebView 调用系统 confirm 触发 Tauri 权限错误。 */
export function ConfirmDialog({
  title,
  message,
  confirmLabel,
  cancelLabel = "取消",
  tone = "default",
  isBusy = false,
  onCancel,
  onConfirm,
}: ConfirmDialogProps) {
  /** 默认确认使用信任图标，只有危险态才显示警告三角。 */
  const ToneIcon = tone === "danger" ? AlertTriangle : ShieldCheck;
  const toneLabel = tone === "danger" ? "需要确认" : "确认操作";

  return (
    <div
      className="modal-backdrop confirm-backdrop"
      role="presentation"
      onMouseDown={(event) => {
        event.stopPropagation();
        onCancel();
      }}
    >
      <section
        className={`confirm-dialog ${tone}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby="confirm-dialog-title"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="confirm-dialog-heading">
          <span className="confirm-dialog-icon" aria-hidden="true">
            <ToneIcon size={18} />
          </span>
          <div>
            <p className="section-label">{toneLabel}</p>
            <h2 id="confirm-dialog-title">{title}</h2>
          </div>
        </div>
        <p className="confirm-dialog-message">{message}</p>
        <div className="modal-actions">
          <button className="ghost-button" type="button" onClick={onCancel} disabled={isBusy}>
            {cancelLabel}
          </button>
          <button className={`primary-button compact confirm-dialog-confirm ${tone}`} type="button" onClick={onConfirm} disabled={isBusy}>
            {confirmLabel}
          </button>
        </div>
      </section>
    </div>
  );
}
