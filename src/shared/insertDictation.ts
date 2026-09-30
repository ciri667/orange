/** 把识别文本插入到选区；没有选区时插在光标处，并返回新的光标位置。 */
export function insertDictationText(
  value: string,
  selectionStart: number,
  selectionEnd: number,
  transcript: string,
): { value: string; caret: number } {
  const text = transcript.trim();
  const length = value.length;
  const start = clamp(selectionStart, 0, length);
  const end = clamp(Math.max(selectionEnd, start), start, length);
  if (!text) {
    return { value, caret: start };
  }

  return {
    value: `${value.slice(0, start)}${text}${value.slice(end)}`,
    caret: start + text.length,
  };
}

function clamp(value: number, min: number, max: number) {
  if (!Number.isFinite(value)) {
    return min;
  }
  return Math.min(Math.max(value, min), max);
}
