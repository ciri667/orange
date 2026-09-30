import { Mic } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { Button } from "./Button";
import { cn } from "./cn";
import { insertDictationText } from "./insertDictation";
import {
  downloadVoiceModel,
  loadVoiceModelStatus,
  startVoiceCapture,
  stopVoiceCapture,
  type VoiceModelStatus,
} from "./api/voice";

/** 按住说话按钮。识别结果写回调用方传入的 textarea。 */
export function VoiceDictationButton({
  getTextarea,
  onValueChange,
  onNotice,
  disabled = false,
}: {
  getTextarea: () => HTMLTextAreaElement | null;
  onValueChange: (value: string) => void;
  onNotice?: (message: string) => void;
  disabled?: boolean;
}) {
  const [modelStatus, setModelStatus] = useState<VoiceModelStatus | null>(null);
  const [phase, setPhase] = useState<"idle" | "recording" | "recognizing" | "downloading">("idle");
  const [hint, setHint] = useState("");
  const phaseRef = useRef(phase);
  const holdingRef = useRef(false);
  const selectionRef = useRef({ start: 0, end: 0 });
  const getTextareaRef = useRef(getTextarea);
  const onValueChangeRef = useRef(onValueChange);
  const onNoticeRef = useRef(onNotice);

  useEffect(() => {
    getTextareaRef.current = getTextarea;
    onValueChangeRef.current = onValueChange;
    onNoticeRef.current = onNotice;
  }, [getTextarea, onNotice, onValueChange]);

  useEffect(() => {
    let cancelled = false;
    void loadVoiceModelStatus()
      .then((status) => {
        if (!cancelled) {
          setModelStatus(status);
        }
      })
      .catch(() => {
        // 状态读取失败时仍允许再次按按钮，错误会在按下时提示。
      });
    return () => {
      cancelled = true;
    };
  }, []);

  function report(message: string) {
    setHint(message);
    onNoticeRef.current?.(message);
  }

  function setPhaseBoth(next: typeof phase) {
    phaseRef.current = next;
    setPhase(next);
  }

  async function beginRecording() {
    if (disabled || phaseRef.current !== "idle") {
      return;
    }

    const textarea = getTextareaRef.current();
    selectionRef.current = {
      start: textarea?.selectionStart ?? 0,
      end: textarea?.selectionEnd ?? textarea?.selectionStart ?? 0,
    };

    let status = modelStatus;
    try {
      status = await loadVoiceModelStatus();
      setModelStatus(status);
    } catch (error) {
      report(errorText(error));
      return;
    }

    if (!status.supported) {
      report(status.message);
      return;
    }
    if (!status.ready) {
      setPhaseBoth("downloading");
      report("正在下载中文语音模型，大约 230MB。完成后请再按住说话。");
      try {
        const downloaded = await downloadVoiceModel((progress) => {
          setModelStatus(progress);
          if (progress.totalBytes) {
            const percent = Math.min(100, Math.round((progress.receivedBytes / progress.totalBytes) * 100));
            setHint(`正在下载中文语音模型 ${percent}%`);
          }
        });
        setModelStatus(downloaded);
        report(downloaded.ready ? "中文语音模型已就绪，请按住麦克风说话。" : downloaded.message);
      } catch (error) {
        report(errorText(error));
      } finally {
        setPhaseBoth("idle");
      }
      return;
    }

    if (!holdingRef.current) {
      return;
    }

    try {
      await startVoiceCapture();
      setPhaseBoth("recording");
      setHint("正在录音，松开后识别");
      if (!holdingRef.current) {
        await finishRecording();
      }
    } catch (error) {
      setPhaseBoth("idle");
      report(errorText(error));
    }
  }

  async function finishRecording() {
    if (phaseRef.current !== "recording") {
      return;
    }

    setPhaseBoth("recognizing");
    setHint("正在识别…");
    try {
      const transcript = await stopVoiceCapture();
      const textarea = getTextareaRef.current();
      const value = textarea?.value ?? "";
      const start = textarea ? textarea.selectionStart : selectionRef.current.start;
      const end = textarea ? textarea.selectionEnd : selectionRef.current.end;
      const inserted = insertDictationText(value, start, end, transcript.text);
      if (!transcript.text.trim()) {
        report("没有识别到语音。");
        return;
      }
      onValueChangeRef.current(inserted.value);
      requestAnimationFrame(() => {
        const node = getTextareaRef.current();
        if (!node) {
          return;
        }
        node.focus();
        node.setSelectionRange(inserted.caret, inserted.caret);
      });
      setHint(transcript.truncated ? "已写入识别结果，并只保留了前 60 秒。" : "");
      if (transcript.truncated) {
        onNoticeRef.current?.("录音超过 60 秒，只识别了前面一段。");
      }
    } catch (error) {
      report(errorText(error));
    } finally {
      setPhaseBoth("idle");
    }
  }

  const title = hint || (phase === "recording" ? "松手结束并识别" : modelStatus?.ready ? "按住说话" : modelStatus?.message || "按住说话");
  const busy = phase === "recognizing" || phase === "downloading";

  return (
    <>
      <Button
        variant="icon"
        size="compact"
        className={cn(phase === "recording" && "bg-danger-soft text-danger")}
        title={title}
        aria-label={phase === "recording" ? "松手结束录音" : "按住说话"}
        aria-pressed={phase === "recording"}
        disabled={disabled || busy}
        onPointerDown={(event) => {
          if (event.button !== 0) {
            return;
          }
          event.preventDefault();
          event.currentTarget.setPointerCapture(event.pointerId);
          holdingRef.current = true;
          void beginRecording();
        }}
        onPointerUp={() => {
          holdingRef.current = false;
          void finishRecording();
        }}
        onPointerCancel={() => {
          holdingRef.current = false;
          void finishRecording();
        }}
        onKeyDown={(event) => {
          if (event.repeat || (event.key !== " " && event.key !== "Enter")) {
            return;
          }
          event.preventDefault();
          holdingRef.current = true;
          void beginRecording();
        }}
        onKeyUp={(event) => {
          if (event.key !== " " && event.key !== "Enter") {
            return;
          }
          event.preventDefault();
          holdingRef.current = false;
          void finishRecording();
        }}
        onContextMenu={(event) => event.preventDefault()}
      >
        <Mic size={16} />
      </Button>
      <span className="sr-only" role="status">
        {hint}
      </span>
    </>
  );
}

function errorText(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}
