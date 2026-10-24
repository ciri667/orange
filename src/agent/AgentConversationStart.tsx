import { ArrowUpRight, FileText, ListTree, Network, PenLine } from "lucide-react";
import { Button } from "../shared/Button";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import type { Note } from "../shared/types";
import { OrangeMark } from "../shared/OrangeMark";

/** 空会话从真实资料开始；只填入可修改的草稿，不替用户发送请求。 */
export function AgentConversationStart({
  note,
  knowledgeBaseName,
  onChoosePrompt,
}: {
  /** 宿主只传入当前会话已授权范围内的笔记。 */
  note?: Note;
  knowledgeBaseName: string;
  onChoosePrompt: (prompt: string, noteId?: string) => void;
}) {
  const starters = note
    ? [
        { label: "梳理要点", detail: "让思路更清楚", icon: ListTree, prompt: "请梳理这篇笔记的核心观点，指出值得继续展开的部分。" },
        { label: "发现关联", detail: "把零散的知识连起来", icon: Network, prompt: "请在当前授权知识库中寻找与这篇笔记相关的内容，说明联系并附上来源。" },
        { label: "一起润色", detail: "保留你的想法，打磨表达", icon: PenLine, prompt: "请帮我改进这篇笔记的表达，保留原意。先说明修改建议，再给出可审阅的改动。" },
      ]
    : [
        { label: "找个答案", detail: "从你的资料里寻找线索", icon: ListTree, prompt: "请帮我从当前授权知识库中查找相关资料，并附上可核对的来源：" },
        { label: "整理想法", detail: "把灵感变成一篇笔记", icon: PenLine, prompt: "请帮我把下面的想法整理成一篇笔记，先讨论结构：" },
        { label: "接着探索", detail: "沿着一个问题往前走", icon: Network, prompt: "我想和你一起探索这个问题，请结合当前授权知识库中的资料：" },
      ];

  return (
    <section className="agent-conversation-start" aria-label="开始与橘记协作">
      <OrangeMark className="size-12" />
      <div className="grid gap-2">
        <h3 className="m-0 text-xl font-semibold leading-snug tracking-tight text-ink-strong">想法，从这里展开</h3>
        <p className="m-0 text-sm leading-relaxed text-ink-muted">读到的、想到的，都可以接着聊。</p>
      </div>
      <div className="conversation-source">
        <FileText size={16} className="shrink-0 text-ink-muted" />
        <div className="grid min-w-0 gap-1">
          <span className="text-xs text-ink-muted">{note ? "从这篇笔记开始" : "本次资料范围"}</span>
          <OverflowTooltipText className="truncate text-sm font-medium text-ink" text={note?.title ?? knowledgeBaseName} logArea="conversation_start_source" />
        </div>
      </div>
      <div className="conversation-starters" aria-label="问题建议">
        {starters.map((starter) => (
          <Button key={starter.label} className="conversation-starter" onClick={() => onChoosePrompt(starter.prompt, note?.id)}>
            <starter.icon size={18} className="shrink-0 text-ink-muted" />
            <span className="grid flex-1 gap-1 text-left">
              <span className="text-sm font-medium text-ink">{starter.label}</span>
              <span className="text-xs font-normal text-ink-muted">{starter.detail}</span>
            </span>
            <ArrowUpRight size={15} className="conversation-starter-arrow shrink-0 text-ink-muted" />
          </Button>
        ))}
      </div>
      <p className="m-0 text-xs leading-relaxed text-ink-muted">先聊聊思路，改动由你确认。</p>
    </section>
  );
}
