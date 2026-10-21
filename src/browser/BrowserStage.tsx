import { MonitorPlay } from "lucide-react";
import { useRef } from "react";
import type { BrowserFrame, BrowserUserInput } from "../shared/types";

/** 把 object-fit: contain 的画面坐标还原为浏览器 CSS 坐标；留白不接受点击。 */
export function framePoint(rect: { left: number; top: number; width: number; height: number }, frame: BrowserFrame, clientX: number, clientY: number) {
  const scale = Math.min(rect.width / frame.width, rect.height / frame.height);
  if (!Number.isFinite(scale) || scale <= 0) return null;
  const x = (clientX - rect.left - (rect.width - frame.width * scale) / 2) / scale;
  const y = (clientY - rect.top - (rect.height - frame.height * scale) / 2) / scale;
  return x >= 0 && y >= 0 && x < frame.width && y < frame.height ? { x, y } : null;
}

/** 画面接管后的输入层；中文输入法与粘贴通过隐藏 textarea 的 input 事件提交。 */
export function BrowserStage({ frame, enabled, onInput }: {
  frame: BrowserFrame | null;
  enabled: boolean;
  onInput: (input: BrowserUserInput) => void;
}) {
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const compositionRef = useRef(false);
  const committedCompositionRef = useRef<string | null>(null);

  return (
    <div className="browser-stage">
      {enabled && frame ? (
        <img
          className="browser-stage-frame"
          src={`data:image/jpeg;base64,${frame.data}`}
          alt="浏览器实时画面，点击后可输入文字"
          draggable={false}
          onClick={(event) => {
            const point = framePoint(event.currentTarget.getBoundingClientRect(), frame, event.clientX, event.clientY);
            if (!point) return;
            inputRef.current?.focus({ preventScroll: true });
            onInput({ kind: "click", ...point });
          }}
          onWheel={(event) => {
            const point = framePoint(event.currentTarget.getBoundingClientRect(), frame, event.clientX, event.clientY);
            if (!point) return;
            const factor = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? frame.height : 1;
            onInput({ kind: "scroll", ...point, deltaX: event.deltaX * factor, deltaY: event.deltaY * factor });
          }}
        />
      ) : (
        <div className="browser-stage-placeholder">
          <MonitorPlay size={20} aria-hidden />
          <p>{enabled ? "等待页面画面…" : "实时画面已关闭"}</p>
        </div>
      )}
      <textarea
        ref={inputRef}
        className="browser-stage-input"
        aria-label="浏览器页面文字输入"
        disabled={!enabled || !frame}
        autoComplete="off"
        onCompositionStart={() => {
          compositionRef.current = true;
          committedCompositionRef.current = null;
        }}
        onCompositionEnd={(event) => {
          compositionRef.current = false;
          committedCompositionRef.current = event.currentTarget.value;
          if (event.currentTarget.value) onInput({ kind: "text", text: event.currentTarget.value });
          event.currentTarget.value = "";
        }}
        onInput={(event) => {
          if (compositionRef.current) return;
          const inputType = (event.nativeEvent as InputEvent).inputType;
          // 部分 WebView 在 compositionend 后补发提交事件，不能把同一段中文插入两次。
          const repeatedComposition = ["insertCompositionText", "insertFromComposition"].includes(inputType)
            && event.currentTarget.value === committedCompositionRef.current;
          committedCompositionRef.current = null;
          if (repeatedComposition) {
            event.currentTarget.value = "";
            return;
          }
          if (event.currentTarget.value) onInput({ kind: "text", text: event.currentTarget.value });
          event.currentTarget.value = "";
        }}
        onKeyDown={(event) => {
          if (event.nativeEvent.isComposing || compositionRef.current || event.key === "Process") return;
          // 普通字符和粘贴交给 input 事件，保留中文和多行文本；其余按键送给页面。
          if (event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey) return;
          if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "v") return;
          if (["Control", "Shift", "Alt", "Meta"].includes(event.key)) return;
          event.preventDefault();
          onInput({ kind: "key", key: event.key, modifiers: (event.altKey ? 1 : 0) | (event.ctrlKey ? 2 : 0) | (event.metaKey ? 4 : 0) | (event.shiftKey ? 8 : 0) });
        }}
      />
    </div>
  );
}
