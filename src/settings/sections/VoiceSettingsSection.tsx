import { Download, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "../../shared/Button";
import { ConfirmDialog } from "../../shared/ConfirmDialog";
import {
  deleteVoiceModel,
  downloadVoiceModel,
  loadVoiceModelStatus,
  type VoiceModelStatus,
} from "../../shared/api/voice";
import { settingsCardClassName, settingsSectionClassName } from "../../shared/ui";
import { SettingsSectionHeader } from "../SettingsChrome";

/** 本机中文语音模型的下载和删除。录音按钮在输入框里，不在这里。 */
export function VoiceSettingsSection() {
  const [status, setStatus] = useState<VoiceModelStatus | null>(null);
  const [error, setError] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void loadVoiceModelStatus()
      .then((next) => {
        if (!cancelled) {
          setStatus(next);
        }
      })
      .catch((loadError: unknown) => {
        if (!cancelled) {
          setError(loadError instanceof Error ? loadError.message : String(loadError));
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function handleDownload() {
    setBusy(true);
    setError("");
    try {
      const next = await downloadVoiceModel(setStatus);
      setStatus(next);
    } catch (downloadError: unknown) {
      setError(downloadError instanceof Error ? downloadError.message : String(downloadError));
    } finally {
      setBusy(false);
    }
  }

  async function handleDelete() {
    setBusy(true);
    setError("");
    try {
      setStatus(await deleteVoiceModel());
      setConfirmDelete(false);
    } catch (deleteError: unknown) {
      setError(deleteError instanceof Error ? deleteError.message : String(deleteError));
    } finally {
      setBusy(false);
    }
  }

  const progress = status?.downloading && status.totalBytes
    ? ` ${Math.min(100, Math.round((status.receivedBytes / status.totalBytes) * 100))}%`
    : "";

  return (
    <section className={settingsSectionClassName} aria-labelledby="voice-settings-title">
      <SettingsSectionHeader
        kicker="Configuration"
        title="语音输入"
        titleId="voice-settings-title"
        description="按住输入框旁的麦克风说话，松开后把中文写进光标位置。识别在本机完成，音频不会上传。"
      />
      <article className={settingsCardClassName}>
        <p className="m-0 text-[13px] leading-[1.55] text-ink-muted">{status?.message ?? "正在读取语音模型状态…"}{progress}</p>
        {error ? <p className="m-0 text-[13px] leading-[1.55] text-danger">{error}</p> : null}
        <div className="flex flex-wrap items-center gap-2">
          <Button variant="ghost" size="compact" onClick={() => void handleDownload()} disabled={busy || status?.downloading || status?.supported === false}>
            <Download size={13} />
            {status?.ready ? "重新下载模型" : "下载中文模型"}
          </Button>
          <Button variant="ghost" size="compact" tone="danger" onClick={() => setConfirmDelete(true)} disabled={busy || !status?.ready}>
            <Trash2 size={13} />
            删除模型
          </Button>
        </div>
      </article>
      {confirmDelete ? (
        <ConfirmDialog
          title="删除语音模型"
          message="删除后需要重新下载大约 230MB 的中文模型，才能继续按住说话。"
          confirmLabel="删除模型"
          tone="danger"
          isBusy={busy}
          onCancel={() => setConfirmDelete(false)}
          onConfirm={() => void handleDelete()}
        />
      ) : null}
    </section>
  );
}
