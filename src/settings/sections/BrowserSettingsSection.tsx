import { Download } from "lucide-react";
import { useChromiumInstall } from "../../browser/useChromiumInstall";
import { Button } from "../../shared/Button";
import { Checkbox } from "../../shared/Checkbox";
import { cn } from "../../shared/cn";
import { settingsSectionClassName } from "../../shared/ui";
import type { UserSettings } from "../../shared/types";
import { SettingsSectionHeader } from "../SettingsChrome";

/**
 * 浏览器功能设置：默认关闭；启用后需显式确认下载固定版本 Chromium。
 * 后端维护下载状态并流式推送进度，zip 自带 CRC 完整性校验。
 */
export function BrowserSettingsSection({
  settingsDraft,
  isBusy,
  onSettingsChange,
}: {
  settingsDraft: UserSettings;
  isBusy: boolean;
  onSettingsChange: (settings: UserSettings) => void;
}) {
  const { status: installStatus, error, requesting, download } = useChromiumInstall();
  const browser = settingsDraft.browser;
  const downloading = requesting || Boolean(installStatus?.downloading);
  const locked = isBusy || downloading;
  const phase = requesting && !installStatus?.downloading ? "connecting" : installStatus?.phase;
  const downloadedMb = ((installStatus?.downloaded ?? 0) / 1024 / 1024).toFixed(1);
  const totalMb = installStatus?.total ? (installStatus.total / 1024 / 1024).toFixed(1) : null;
  // 没有 Content-Length 时展示活动进度条和实际下载量，不伪造百分比。
  const percent = installStatus?.total
    ? Math.min(100, Math.floor(installStatus.downloaded / installStatus.total * 100))
    : null;
  const statusText = downloading
    ? phase === "extracting" ? "下载完成，正在校验并解压安装…"
      : phase === "downloading" ? "正在下载浏览器内核…" : "正在连接下载服务器…"
    : installStatus?.installed ? `已安装${installStatus.version ? `（${installStatus.version}）` : ""}`
      : installStatus?.phase === "failed" ? "下载或安装失败，可以重试"
        : installStatus ? "未安装，启用后需先下载" : error ? "无法读取浏览器内核状态" : "正在读取安装状态…";

  return (
    <section className={settingsSectionClassName} aria-labelledby="browser-settings-title">
      <SettingsSectionHeader
        kicker="Agent"
        title="内嵌浏览器"
        titleId="browser-settings-title"
        description="让 Agent 在受控 Chromium 里打开网页、读取元素并执行点击/填表。只有本机交互会话的两种完全权限档可用；截图不会发给模型，你可以随时在面板里接管。"
      />
      <label className="mt-4 flex items-center gap-2 text-sm text-ink">
        <Checkbox
          checked={browser.enabled}
          disabled={locked}
          onChange={(event) =>
            onSettingsChange({
              ...settingsDraft,
              browser: { ...browser, enabled: event.target.checked },
            })
          }
        />
        启用内嵌浏览器
      </label>
      <div className="mt-4 rounded-lg border border-border bg-surface-muted p-3 text-xs text-ink-soft">
        <p>
          浏览器内核：Chrome for Testing（固定版本，独立 profile，不读取你日常浏览器的任何数据）。
        </p>
        <p className="mt-1" role="status" aria-live="polite">
          状态：{statusText}
        </p>
        {downloading ? (
          <div className="mt-3 space-y-2">
            <div
              className="h-2 overflow-hidden rounded-full bg-accent-soft"
              role="progressbar"
              aria-label="浏览器内核下载进度"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={percent ?? undefined}
              aria-valuetext={phase === "extracting" ? "下载完成，正在校验并解压安装" : percent === null ? `已下载 ${downloadedMb} MB，总大小未知` : `${percent}%`}
            >
              <div
                className={cn("h-full rounded-full bg-ink transition-[width]", percent === null && "w-1/3 animate-pulse motion-reduce:animate-none")}
                style={percent === null ? undefined : { width: `${percent}%` }}
              />
            </div>
            <p className="m-0 flex justify-between gap-3 text-ink">
              <span>已下载 {downloadedMb}{totalMb ? ` / ${totalMb}` : ""} MB</span>
              <span>{phase === "extracting" ? "下载完成" : percent === null ? "总大小未知" : `${percent}%`}</span>
            </p>
            <p className="m-0 text-ink-soft">可以切换设置或关闭设置面板，下载会在后台继续。</p>
          </div>
        ) : null}
        {!installStatus?.installed && (
          <Button
            variant="ghost"
            size="compact"
            type="button"
            className="mt-2"
            disabled={locked || !installStatus}
            onClick={() => void download()}
          >
            <Download size={14} />
            {downloading ? phase === "extracting" ? "安装中…" : "下载中…" : installStatus?.phase === "failed" ? "重试下载" : "下载浏览器内核"}
          </Button>
        )}
      </div>
      {error ? <p className="mt-2 text-xs text-danger" role="alert">{error}</p> : null}
      <p className="mt-3 text-xs text-ink-soft">
        完全权限，需审批档的所有操作（包括读取）逐次确认；无需审批档自动执行，Plan 禁用。清除站点数据会关闭浏览器并删除隔离 profile。
      </p>
    </section>
  );
}
