import { buildMarkdownDiff } from "../diff/markdownDiff";
import { Button } from "../shared/Button";
import { Checkbox } from "../shared/Checkbox";
import type { AgentSession } from "../shared/types";

/** 独立操作可选择；目录以完整清单为一个确认单元，成功项不可重复执行。 */
export function FileChangeSetReview({ session, busy, onToggle, onApply, onReject }: {
  session: AgentSession; busy: boolean;
  onToggle: (id: string, selected: boolean) => void; onApply: () => void; onReject: () => void;
}) {
  const set = session.pendingChangeSet;
  if (!set || !["pending", "partial", "expired"].includes(set.status)) return null;
  const readonly = session.planMode || session.securityLevel === "workspace-readonly" || Boolean(session.scheduleIdentity);
  const expired = set.status === "expired";
  return <section className="mx-3 my-2 max-h-[45vh] shrink-0 overflow-auto rounded-control border border-border bg-surface p-3" aria-label="文件变更审阅">
    <strong>{set.summary} · {set.status === "partial" ? "部分完成" : expired ? "已过期" : "待审阅"}</strong>
    {set.warnings.map((warning, i) => <p key={i} className="text-xs text-warning">{warning}</p>)}
    {set.operations.map((op) => <div key={op.id} className="my-2 rounded-control border border-border p-2 text-xs">
      <label className="flex items-start gap-2">
        <Checkbox checked={op.selected} disabled={busy || expired || op.status === "completed"} onChange={(event) => onToggle(op.id, event.target.checked)} />
        <span className="min-w-0 break-all"><strong>{{ create: "新建文件", modify: "修改正文", create_folder: "新建目录", move: "移动", rename: "重命名", delete: "移至回收站" }[op.operation] ?? op.operation}</strong> · {op.status === "completed" ? "已完成" : op.status === "failed" ? "失败" : "未执行"}<br />
          {op.sourcePath && <>源：{op.sourcePath}<br /></>}目标：{op.targetPath}<br />影响文件：{op.affectedFiles ?? 1} · {op.byteSize} 字节
        </span>
      </label>
      {op.error && <p className="text-danger">{op.error}</p>}
      {!op.binary && (op.original !== undefined || op.next !== undefined) && <details>
        <summary className="cursor-pointer py-1">查看文本 diff</summary>
        <pre className="max-h-64 overflow-auto whitespace-pre-wrap break-all">{buildMarkdownDiff(op.original ?? "", op.next ?? "").hunks.flatMap((h) => h.lines).map((line) => <div key={line.id} className={line.kind === "added" ? "text-success" : line.kind === "removed" ? "text-danger" : "text-ink-muted"}>{line.kind === "added" ? "+ " : line.kind === "removed" ? "− " : "  "}{line.text}</div>)}</pre>
      </details>}
    </div>)}
    {readonly && <p className="text-xs text-ink-muted">只读或 Plan 下可以查看、拒绝，切换权限后才能应用。</p>}
    <footer className="flex justify-end gap-2">
      <Button variant="ghost" size="compact" onClick={onReject} disabled={busy || expired}>拒绝未执行项</Button>
      <Button variant="primary" size="compact" onClick={onApply} disabled={busy || readonly || expired || !set.operations.some((op) => op.selected && op.status !== "completed")}>应用已选变更</Button>
    </footer>
  </section>;
}
