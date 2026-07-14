import { useEffect, useId, useRef } from "react";
import { AlertTriangle, ShieldCheck } from "lucide-react";

/** 确认弹窗语义类型，danger 用于删除、移除授权等不可直接撤销的操作。 */
export type ConfirmDialogTone = "default" | "danger";

/**
 * 确认弹窗中的可选第三操作。
 *
 * 适用于“保存并关闭 / 放弃更改 / 取消”这类需要保留取消语义、
 * 同时提供另一条明确处理路径的场景。
 */
export interface ConfirmDialogThirdAction {
  /** 操作按钮展示文案。 */
  label: string;
  /** 操作的视觉语义；放弃更改等不可逆操作应使用 danger。 */
  tone?: ConfirmDialogTone;
  /** 可选的辅助技术专用名称，未提供时使用 label。 */
  ariaLabel?: string;
}

/** 应用内确认弹窗的展示配置，不依赖浏览器 window.confirm 或 Tauri dialog 权限。 */
export interface ConfirmDialogConfig {
  title: string;
  message: string;
  confirmLabel: string;
  cancelLabel?: string;
  tone?: ConfirmDialogTone;
  /** 可选的第三操作配置；未配置时保持现有二按钮展示。 */
  thirdAction?: ConfirmDialogThirdAction;
}

/** 应用内确认弹窗属性，调用方负责执行确认后的真实业务动作。 */
interface ConfirmDialogProps extends ConfirmDialogConfig {
  isBusy?: boolean;
  onCancel: () => void;
  onConfirm: () => void;
  /** 第三操作的业务回调，仅在配置 thirdAction 后使用。 */
  onThirdAction?: () => void;
}

/** 统一的应用内确认弹窗，避免 WebView 调用系统 confirm 触发 Tauri 权限错误。 */
export function ConfirmDialog({
  title,
  message,
  confirmLabel,
  cancelLabel = "取消",
  tone = "default",
  thirdAction,
  isBusy = false,
  onCancel,
  onConfirm,
  onThirdAction,
}: ConfirmDialogProps) {
  /** 默认确认使用信任图标，只有危险态才显示警告三角。 */
  const ToneIcon = tone === "danger" ? AlertTriangle : ShieldCheck;
  const toneLabel = tone === "danger" ? "需要确认" : "确认操作";
  const titleId = useId();
  const cancelButtonRef = useRef<HTMLButtonElement>(null);

  /** 弹窗打开后优先将焦点置于取消按钮，防止回车误触发危险操作。 */
  useEffect(() => {
    cancelButtonRef.current?.focus();
  }, []);

  /** Escape 始终视为取消；忙碌时禁止关闭以避免中断正在提交的操作。 */
  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape" && !isBusy) {
        event.preventDefault();
        onCancel();
      }
    }

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [isBusy, onCancel]);

  /** 仅在配置和回调同时存在时展示第三操作，避免渲染无效按钮。 */
  const canUseThirdAction = Boolean(thirdAction && onThirdAction);

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
        aria-labelledby={titleId}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="confirm-dialog-heading">
          <span className="confirm-dialog-icon" aria-hidden="true">
            <ToneIcon size={18} />
          </span>
          <div>
            <p className="section-label">{toneLabel}</p>
            <h2 id={titleId}>{title}</h2>
          </div>
        </div>
        <p className="confirm-dialog-message">{message}</p>
        <div className="modal-actions">
          <button ref={cancelButtonRef} className="ghost-button" type="button" onClick={onCancel} disabled={isBusy}>
            {cancelLabel}
          </button>
          {canUseThirdAction && thirdAction ? (
            <button
              className={`primary-button compact confirm-dialog-confirm ${thirdAction.tone ?? "default"}`}
              type="button"
              onClick={onThirdAction}
              disabled={isBusy}
              aria-label={thirdAction.ariaLabel ?? thirdAction.label}
            >
              {thirdAction.label}
            </button>
          ) : null}
          <button className={`primary-button compact confirm-dialog-confirm ${tone}`} type="button" onClick={onConfirm} disabled={isBusy}>
            {confirmLabel}
          </button>
        </div>
      </section>
    </div>
  );
}
