import { Button } from "../../../shared/Button";
import { cn } from "../../../shared/cn";

/** 模型设置中的紧凑开关，保留原生按钮的键盘操作和明确的开关语义。 */
export function ModelSettingSwitch({ checked, onChange, label, disabled, describedBy }: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  disabled?: boolean;
  describedBy?: string;
}) {
  return (
    <Button
      role="switch"
      aria-checked={checked}
      aria-label={label}
      aria-describedby={describedBy}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        "h-5 min-h-5 w-9 rounded-full border-0 p-0.5",
        checked ? "bg-accent hover:enabled:bg-accent-strong" : "bg-border-strong hover:enabled:bg-ink-soft",
      )}
    >
      <span aria-hidden="true" className={cn(
        "size-4 rounded-full bg-surface shadow-sm transition-transform duration-150 motion-reduce:transition-none",
        checked ? "translate-x-2" : "-translate-x-2",
      )} />
    </Button>
  );
}
