import { Check, Copy, Download, FoldVertical, Lightbulb, UnfoldVertical } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import rehypeSanitize from "rehype-sanitize";
import { Button } from "../shared/Button";
import { copyTextToClipboard } from "../shared/clipboard";
import { cn } from "../shared/cn";
import { logWarn } from "../shared/logger";
import {
  createMarkdownComponents,
  markdownMessageClassName,
  markdownRemarkPlugins,
  protectGfmTablePipesInInlineCode,
} from "../shared/markdown";
import { PLAN_CARD_COLLAPSED_MAX_PX, planDownloadFilename } from "../shared/planMode";

/** 复制成功后的短暂反馈时长，避免按钮文案一直停在「已复制」。 */
const PLAN_COPY_RESET_MS = 1600;

/**
 * 规划模式留下的执行计划卡片。
 * 标头固定写着「执行计划」，折叠后会话里仍能认出这一轮用过规划模式。
 */
export function PlanMessageCard({
  content,
  initiallyExpanded = false,
}: {
  content: string;
  /** 当前仍待确认的最新计划默认展开，历史计划默认收起。 */
  initiallyExpanded?: boolean;
}) {
  const [expanded, setExpanded] = useState(initiallyExpanded);
  const [canCollapse, setCanCollapse] = useState(false);
  const [copied, setCopied] = useState(false);
  const bodyRef = useRef<HTMLDivElement>(null);
  const resetTimerRef = useRef<number | null>(null);

  // 不再是待确认的最新计划时收回去，历史里只留卡片标头和预览。
  useEffect(() => {
    if (!initiallyExpanded) {
      setExpanded(false);
    }
  }, [initiallyExpanded]);

  useEffect(() => {
    const element = bodyRef.current;
    if (!element) {
      return;
    }

    // scrollHeight 含被裁掉的部分，和折叠高度比较才知道有没有必要展开。
    const measure = () => {
      setCanCollapse(element.scrollHeight > PLAN_CARD_COLLAPSED_MAX_PX + 1);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [content]);

  useEffect(() => {
    return () => {
      if (resetTimerRef.current != null) {
        window.clearTimeout(resetTimerRef.current);
      }
    };
  }, []);

  const collapsed = canCollapse && !expanded;

  return (
    <section
      className="mt-2 overflow-hidden rounded-panel border border-border bg-surface shadow-app-soft"
      aria-label="执行计划"
    >
      <header className="flex min-w-0 items-center gap-2 px-3 pt-2.5">
        <span className="inline-flex min-w-0 items-center gap-1.5 text-[12px] font-medium text-ink-muted">
          <Lightbulb size={14} className="shrink-0 text-accent" />
          执行计划
        </span>
        <span className="ml-auto inline-flex shrink-0 items-center">
          <Button
            variant="icon"
            size="compact"
            className="text-ink-soft"
            title="下载计划"
            aria-label="下载计划"
            onClick={() => downloadPlanMarkdown(content)}
          >
            <Download size={14} />
          </Button>
          <Button
            variant="icon"
            size="compact"
            className={cn("text-ink-soft", copied && "text-success")}
            title={copied ? "已复制" : "复制计划"}
            aria-label={copied ? "已复制" : "复制计划"}
            onClick={() => {
              void copyPlanContent(content, () => {
                setCopied(true);
                if (resetTimerRef.current != null) {
                  window.clearTimeout(resetTimerRef.current);
                }
                resetTimerRef.current = window.setTimeout(() => setCopied(false), PLAN_COPY_RESET_MS);
              });
            }}
          >
            {copied ? <Check size={14} /> : <Copy size={14} />}
          </Button>
          {canCollapse ? (
            <Button
              variant="icon"
              size="compact"
              className="text-ink-soft"
              title={expanded ? "收起计划" : "展开计划"}
              aria-label={expanded ? "收起计划" : "展开计划"}
              aria-expanded={expanded}
              onClick={() => setExpanded((open) => !open)}
            >
              {expanded ? <FoldVertical size={14} /> : <UnfoldVertical size={14} />}
            </Button>
          ) : null}
        </span>
      </header>
      <div className="relative">
        <div
          ref={bodyRef}
          className={cn("px-3.5 pb-3", collapsed && "overflow-hidden")}
          style={
            collapsed
              ? {
                  maxHeight: PLAN_CARD_COLLAPSED_MAX_PX,
                  maskImage: "linear-gradient(to bottom, #000 62%, transparent 100%)",
                  WebkitMaskImage: "linear-gradient(to bottom, #000 62%, transparent 100%)",
                }
              : undefined
          }
        >
          <div className={cn(markdownMessageClassName, "mt-1")}>
            <ReactMarkdown
              remarkPlugins={markdownRemarkPlugins}
              rehypePlugins={[rehypeSanitize]}
              components={createMarkdownComponents("agent_message", "message")}
            >
              {protectGfmTablePipesInInlineCode(content)}
            </ReactMarkdown>
          </div>
        </div>
        {collapsed ? (
          <button
            type="button"
            tabIndex={-1}
            aria-hidden
            className="absolute inset-x-0 bottom-0 h-16 cursor-pointer border-0 bg-transparent p-0"
            onClick={() => setExpanded(true)}
          />
        ) : null}
      </div>
    </section>
  );
}

/** 复制计划原文；失败只记长度，不把正文写入诊断。 */
async function copyPlanContent(content: string, onCopied: () => void) {
  try {
    await copyTextToClipboard(content);
    onCopied();
  } catch (error) {
    logWarn("复制计划失败", {
      category: "frontend",
      event: "copy_agent_plan",
      error,
      metadata: { contentLength: content.length },
    });
  }
}

/** 把计划 Markdown 下载到本地，方便离开会话后单独留存。 */
function downloadPlanMarkdown(content: string) {
  const blob = new Blob([content], { type: "text/markdown;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = planDownloadFilename(content);
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  URL.revokeObjectURL(url);
}
