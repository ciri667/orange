import { Button } from "../shared/Button";
import type { SettingsSaveStatus as SaveStatus } from "./useSettingsAutosave";

/** 保存反馈留在设置面板内，错误时保留输入并提供重试。 */
export function SettingsSaveStatus({ saves, onRetry }: {
  saves: Array<{ status: SaveStatus; error: string }>;
  onRetry: () => void;
}) {
  // 错误和正在保存优先，避免另一个分区的成功提示掩盖未保存的修改。
  const priority: SaveStatus[] = ["error", "saving", "pending", "saved", "idle"];
  const status = priority.find((candidate) => saves.some((save) => save.status === candidate)) ?? "idle";
  const error = saves.find((save) => save.status === "error")?.error ?? "";
  return (
    <div className="flex min-w-0 items-center gap-2 text-xs text-ink-muted">
      {status === "error" ? (
        <>
          <span role="alert" className="text-danger">保存失败：{error}</span>
          <Button size="compact" variant="ghost" onClick={onRetry}>重试</Button>
        </>
      ) : (
        <span role="status" aria-live="polite">
          {status === "saving" ? "正在保存…" : status === "saved" ? "已自动保存" : status === "pending" ? "完成输入后自动保存" : "修改后自动保存"}
        </span>
      )}
    </div>
  );
}
