/** 前端单张图片预检查上限，和 Rust 存储层限制保持一致。 */
export const MAX_PASTE_IMAGE_BYTES = 20 * 1024 * 1024;

/** 前端单次粘贴总大小预检查上限，减少无意义 base64 读取和 IPC 成本。 */
export const MAX_PASTE_IMAGE_BATCH_BYTES = 50 * 1024 * 1024;

/** 将图片文件读成 base64 主体；调用方负责限制大小和记录脱敏日志。 */
export function readImageFileAsBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();

    reader.onerror = () => reject(new Error("无法读取剪贴板图片。"));
    reader.onload = () => {
      const result = typeof reader.result === "string" ? reader.result : "";
      const dataSeparatorIndex = result.indexOf(",");

      resolve(dataSeparatorIndex >= 0 ? result.slice(dataSeparatorIndex + 1) : result);
    };
    reader.readAsDataURL(file);
  });
}

/** 把 textarea selection 下标收敛到正文长度范围内，防止异步粘贴期间选区失效。 */
export function clampTextIndex(index: number, length: number) {
  if (!Number.isFinite(index)) {
    return length;
  }

  return Math.max(0, Math.min(index, length));
}

/** 将 Markdown 图片片段插入用户粘贴时的选区，保持 textarea 原有编辑语义。 */
export function insertMarkdownAtSelection(content: string, insertion: string, selectionStart: number, selectionEnd: number) {
  const start = clampTextIndex(selectionStart, content.length);
  const end = clampTextIndex(selectionEnd, content.length);
  const normalizedStart = Math.min(start, end);
  const normalizedEnd = Math.max(start, end);

  return `${content.slice(0, normalizedStart)}${insertion}${content.slice(normalizedEnd)}`;
}

/** 生成前端日志中的图片类型摘要，不记录文件名、路径或二进制内容。 */
export function summarizeImageMimeTypes(files: File[]) {
  return Array.from(new Set(files.map((file) => file.type || "unknown")))
    .sort()
    .join(",");
}
