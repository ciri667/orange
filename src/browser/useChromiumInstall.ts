import { useEffect, useRef, useState } from "react";
import {
  downloadBrowserChromium,
  loadBrowserChromiumStatus,
  subscribeBrowserDownloadProgress,
} from "../shared/tauriApi";
import type { ChromiumInstallStatus } from "../shared/types";

/** 后端拥有下载任务；此 hook 只订阅快照，切换设置或关闭抽屉后重新挂载即可恢复。 */
export function useChromiumInstall() {
  const [status, setStatus] = useState<ChromiumInstallStatus | null>(null);
  const [error, setError] = useState("");
  const [requesting, setRequesting] = useState(false);
  const mounted = useRef(false);

  useEffect(() => {
    let disposed = false;
    let revision = 0;
    let unlisten: (() => void) | undefined;
    mounted.current = true;
    void (async () => {
      try {
        // 先订阅再查询；查询期间若收到更新事件，不用旧查询结果覆盖最新进度。
        const stop = await subscribeBrowserDownloadProgress((next) => {
          revision += 1;
          if (!disposed) {
            setStatus(next);
          }
        });
        if (disposed) {
          stop();
          return;
        }
        unlisten = stop;
        const snapshotRevision = revision;
        const next = await loadBrowserChromiumStatus();
        if (!disposed && revision === snapshotRevision) {
          setStatus(next);
        }
      } catch (loadError: unknown) {
        if (!disposed) {
          setError(loadError instanceof Error ? loadError.message : String(loadError));
        }
      }
    })();
    return () => {
      disposed = true;
      mounted.current = false;
      unlisten?.();
    };
  }, []);

  /** 调用不随组件卸载取消；完成和失败事件由后端广播给当前仍挂载的页面。 */
  async function download() {
    setRequesting(true);
    setError("");
    try {
      const next = await downloadBrowserChromium();
      if (mounted.current) {
        setStatus(next);
      }
    } catch (downloadError: unknown) {
      if (mounted.current) {
        setError(downloadError instanceof Error ? downloadError.message : String(downloadError));
      }
    } finally {
      if (mounted.current) {
        setRequesting(false);
      }
    }
  }

  return { status, error: error || status?.error || "", requesting, download };
}
