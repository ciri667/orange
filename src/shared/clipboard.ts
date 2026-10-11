/** 把文本写入系统剪贴板；优先 Clipboard API，失败时回退到隐藏 textarea。 */
export async function copyTextToClipboard(text: string) {
  if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return;
    } catch {
      // 无权限或非安全上下文时走 execCommand 回退。
    }
  }

  copyTextWithExecCommand(text);
}

/** Clipboard API 不可用时的同步回退，依赖用户点击手势。 */
function copyTextWithExecCommand(text: string) {
  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.setAttribute("readonly", "");
  textarea.style.position = "fixed";
  textarea.style.top = "0";
  textarea.style.left = "0";
  textarea.style.opacity = "0";
  textarea.style.pointerEvents = "none";
  document.body.appendChild(textarea);
  textarea.focus();
  textarea.select();
  textarea.setSelectionRange(0, textarea.value.length);
  const copied = document.execCommand("copy");
  textarea.remove();

  if (!copied) {
    throw new Error("clipboard_exec_command_failed");
  }
}
