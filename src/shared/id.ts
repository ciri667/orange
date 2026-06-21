/** 生成带前缀的本地唯一 ID，首版用于前端 mock 和 Tauri 快照协议。 */
export function createLocalId(prefix: string) {
  return `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

/** 生成简易内容 hash，浏览器 mock 用于模拟写入冲突校验。 */
export function createContentHash(content: string) {
  let hash = 0;

  // 这里不追求加密强度，只用于浏览器开发态识别内容是否变化。
  for (let index = 0; index < content.length; index += 1) {
    hash = (hash << 5) - hash + content.charCodeAt(index);
    hash |= 0;
  }

  return `mock-${Math.abs(hash)}`;
}
