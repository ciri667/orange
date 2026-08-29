import { FilePenLine, Plus, RotateCw, Trash2 } from "lucide-react";
import { Button } from "../../shared/Button";
import { cn } from "../../shared/cn";
import { OverflowTooltipText } from "../../shared/OverflowTooltipText";
import { findProjectInstructionNote } from "../../shared/projectInstructions";
import { settingsCardClassName, settingsSectionClassName } from "../../shared/ui";
import { SettingsSectionHeader } from "../SettingsChrome";
import type { KnowledgeBase, Note } from "../../shared/types";

/** 知识库设置分区，管理目录授权、激活和重新扫描。 */
export function KnowledgeSettingsSection({
  knowledgeBases,
  notes,
  activeKnowledgeBaseId,
  isBusy,
  onSelectKnowledgeBase,
  onAddKnowledgeBase,
  onRescanKnowledgeBase,
  onRemoveKnowledgeBase,
  onCreateOrOpenProjectInstruction,
}: {
  knowledgeBases: KnowledgeBase[];
  notes: Note[];
  activeKnowledgeBaseId: string;
  isBusy: boolean;
  onSelectKnowledgeBase: (knowledgeBaseId: string) => void;
  onAddKnowledgeBase: () => void;
  onRescanKnowledgeBase: (knowledgeBaseId: string) => void;
  onRemoveKnowledgeBase: (knowledgeBaseId: string) => void;
  onCreateOrOpenProjectInstruction: (knowledgeBaseId: string) => void;
}) {
  return (
    <section className={settingsSectionClassName} aria-labelledby="knowledge-settings-title">
      <SettingsSectionHeader
        kicker="Configuration"
        title="知识库管理"
        titleId="knowledge-settings-title"
        description="管理已授权目录、激活知识库、本地索引刷新，以及给 Agent 看的 AGENTS.md 说明书。"
        actions={
          <Button variant="ghost" onClick={onAddKnowledgeBase}>
            <Plus size={15} />
            添加知识库
          </Button>
        }
      />
      <div className="grid gap-2.5">
        {knowledgeBases.length ? (
          knowledgeBases.map((knowledgeBase) => (
            <article className={cn(settingsCardClassName, "grid-cols-[minmax(0,1fr)_auto] items-start gap-3 max-[820px]:grid-cols-1")} key={knowledgeBase.id}>
              <div>
                <div className="flex flex-wrap items-center gap-1.5">
                  <OverflowTooltipText as="strong" text={knowledgeBase.name} logArea="settings_kb_name" />
                  <span className="rounded-full border border-[rgba(230,224,214,0.78)] bg-white/60 px-2 py-1 text-xs text-ink-muted">
                    {knowledgeBase.status === "error" ? "目录失效" : knowledgeBase.semanticIndexEnabled ? "本地向量" : "FTS5"}
                  </span>
                  {knowledgeBase.id === activeKnowledgeBaseId && (
                    <span className="rounded-full border border-[rgba(230,224,214,0.78)] bg-white/60 px-2 py-1 text-xs text-ink-muted">当前激活</span>
                  )}
                </div>
                <p className="m-0 text-[13px] leading-[1.55] text-ink-muted">{knowledgeBase.description}</p>
                <OverflowTooltipText as="code" className="mt-2 block min-w-0 [overflow-wrap:anywhere] text-ink-muted [word-break:break-word]" text={knowledgeBase.path} logArea="settings_kb_path" />
                <ProjectInstructionStatus knowledgeBaseId={knowledgeBase.id} notes={notes} />
                <ScanReportDetails knowledgeBase={knowledgeBase} />
              </div>
              <div className="flex flex-wrap items-center gap-2">
                <Button variant="ghost" size="compact" onClick={() => onSelectKnowledgeBase(knowledgeBase.id)} disabled={isBusy}>
                  激活
                </Button>
                <Button variant="ghost" size="compact" onClick={() => onCreateOrOpenProjectInstruction(knowledgeBase.id)} disabled={isBusy}>
                  <FilePenLine size={13} />
                  {findProjectInstructionNote(notes, knowledgeBase.id) ? "打开说明书" : "创建说明书"}
                </Button>
                <Button variant="ghost" size="compact" onClick={() => onRescanKnowledgeBase(knowledgeBase.id)} disabled={isBusy}>
                  <RotateCw size={13} />
                  重新扫描
                </Button>
                <Button
                  variant="ghost"
                  size="compact"
                  tone="danger"
                  onClick={() => onRemoveKnowledgeBase(knowledgeBase.id)}
                  disabled={isBusy}
                >
                  <Trash2 size={13} />
                  移除授权
                </Button>
              </div>
            </article>
          ))
        ) : (
          <p className="m-0 text-[13px] text-ink-muted">暂无已授权知识库。</p>
        )}
      </div>
    </section>
  );
}

/** 展示该知识库根目录是否已有 AGENTS.md / ORANGE_AGENT.md。 */
function ProjectInstructionStatus({ knowledgeBaseId, notes }: { knowledgeBaseId: string; notes: Note[] }) {
  const instruction = findProjectInstructionNote(notes, knowledgeBaseId);

  return (
    <p className="mt-2 mb-0 text-xs leading-[1.45] text-ink-muted">
      {instruction
        ? `Agent 说明书：已启用（${instruction.path}）。不要把密钥写进这份文件。`
        : "Agent 说明书：未配置。创建 AGENTS.md 后，橘记会在每次对话时自动读取。"}
    </p>
  );
}

/** 展示知识库最近扫描报告，便于定位空目录、坏文件和被跳过的大目录。 */
function ScanReportDetails({ knowledgeBase }: { knowledgeBase: KnowledgeBase }) {
  const report = knowledgeBase.scanReport;

  if (!report) {
    return null;
  }

  return (
    <div className="mt-[9px] grid gap-1 text-xs leading-[1.45] text-ink-muted">
      <span>
        扫描 {report.scannedFileCount} 篇，失败 {report.failedFileCount} 个
      </span>
      {report.skippedDirectories.length > 0 && <span>跳过：{report.skippedDirectories.slice(0, 4).join(" / ")}</span>}
      {report.errors.length > 0 && <span className="text-danger">{report.errors[0]}</span>}
    </div>
  );
}
