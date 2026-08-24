import type { WorkspaceSnapshot } from "../shared/types";

/** 根据扫描报告生成状态提示，让空目录、失败文件和跳过目录都有可读反馈。 */
export function buildScanNotice(snapshot: WorkspaceSnapshot, knowledgeBaseId: string) {
  const knowledgeBase = snapshot.knowledgeBases.find((item) => item.id === knowledgeBaseId);
  const report = knowledgeBase?.scanReport;

  if (!knowledgeBase) {
    return "";
  }

  if (knowledgeBase.status === "error") {
    return knowledgeBase.description;
  }

  if (!report) {
    return `已扫描「${knowledgeBase.name}」，发现 ${knowledgeBase.noteCount} 篇 Markdown、${knowledgeBase.documentCount} 个普通文档。`;
  }

  const skippedText = report.skippedDirectories.length ? `，跳过 ${report.skippedDirectories.length} 个依赖或隐藏目录` : "";
  const errorText = report.failedFileCount ? `，${report.failedFileCount} 个文件读取失败` : "";

  if (report.scannedFileCount === 0 && !report.failedFileCount) {
    return `「${knowledgeBase.name}」暂未发现支持文档${skippedText}。`;
  }

  return `已扫描「${knowledgeBase.name}」：${report.scannedFileCount} 个支持文档${errorText}${skippedText}。`;
}
