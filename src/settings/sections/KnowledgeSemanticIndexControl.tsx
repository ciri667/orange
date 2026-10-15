import { useState } from "react";
import { Button } from "../../shared/Button";
import { ConfirmDialog } from "../../shared/ConfirmDialog";
import { isRemoteSemanticEndpoint } from "../../shared/api/semantic";
import type { SemanticIndexAction, SemanticIndexStatus, SemanticSearchSettings } from "../../shared/types";

export const SEMANTIC_STATUS_LABEL: Record<SemanticIndexStatus["status"], string> = {
  disabled: "关键词", pending: "待建立", indexing: "建立中", ready: "混合索引可用", paused: "已暂停",
  partial_failure: "部分失败", error: "配置错误", stale: "配置过期",
};

/** 按库控制生成任务；确认与操作只影响语义索引，不改写文档或工作台快照。 */
export function KnowledgeSemanticIndexControl({ id, name, status, config, busy, act }: {
  id: string; name: string; status?: SemanticIndexStatus; config: SemanticSearchSettings; busy: boolean;
  act: (id: string, action: SemanticIndexAction, consent?: boolean) => Promise<void>;
}) {
  const [pending, setPending] = useState<SemanticIndexAction | null>(null);
  const remote = isRemoteSemanticEndpoint(config);
  const enabled = status?.enabled ?? false;
  function request(action: SemanticIndexAction) {
    if (action === "rebuild" || (action === "enable" && remote)) setPending(action);
    else void act(id, action);
  }
  return <div className="mt-3 grid gap-2">
    <p className="m-0 text-xs text-ink-muted">语义索引：{status ? SEMANTIC_STATUS_LABEL[status.status] : "读取中"}
      {enabled && status ? ` · ${status.indexedChunks}/${status.totalChunks} 个片段 · 待处理 ${status.pendingChunks} · 失败 ${status.failedChunks}` : ""}
      {status?.dimensions ? ` · ${status.dimensions} 维` : ""}
    </p>
    {enabled && status?.message && <p className="m-0 text-xs text-ink-muted">{status.message}</p>}
    <div className="flex flex-wrap gap-2">
      {!enabled ? <Button variant="ghost" size="compact" disabled={busy || !config.enabled || !status} onClick={() => request("enable")}>开启语义索引</Button> : <>
        <Button variant="ghost" size="compact" disabled={busy || !config.enabled} onClick={() => request(status?.paused || status?.status === "error" || status?.failedChunks ? "resume" : "pause")}>
          {status?.paused || status?.status === "error" || status?.failedChunks ? "继续／重试" : "暂停生成"}
        </Button>
        <Button variant="ghost" size="compact" disabled={busy || !config.enabled} onClick={() => request("rebuild")}>重建</Button>
        <Button variant="ghost" size="compact" disabled={busy} onClick={() => request("disable")}>关闭并清理向量</Button>
      </>}
    </div>
    {!config.enabled && <p className="m-0 text-xs text-ink-muted">请先在「语义检索」中配置并开启服务。</p>}
    {pending && <ConfirmDialog title={pending === "enable" ? "开启云端语义索引" : "重建语义索引"}
      message={pending === "enable" ? `将自动向所配置的云端服务发送「${name}」的文本片段、标题、相对路径和标签，后续保存也会更新。` : `将清理「${name}」已有向量并重新生成，期间继续提供关键词检索。${remote ? "这会重新向云端发送该库文本，并可能产生费用。" : ""}`}
      confirmLabel={pending === "enable" ? "允许并开启" : "重建"} onCancel={() => setPending(null)} onConfirm={() => { const action = pending; setPending(null); void act(id, action, remote); }} />}
  </div>;
}
