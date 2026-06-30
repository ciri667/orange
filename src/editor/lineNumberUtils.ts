/** 按真实换行符拆分正文；末尾换行会自然产生最后一个空白逻辑行。 */
export function splitLogicalLines(content: string) {
  return content.split(/\r\n|\r|\n/);
}

/** 统计真实行数，空文档也按编辑器常见行为显示第 1 行。 */
export function countLogicalLines(content: string) {
  return splitLogicalLines(content).length;
}
