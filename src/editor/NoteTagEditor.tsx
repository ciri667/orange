import { Plus } from "lucide-react";
import { useRef, useState, type FormEvent, type KeyboardEvent } from "react";
import { Button } from "../shared/Button";
import { Chip } from "../shared/Chip";
import { cn } from "../shared/cn";
import { FilterChip } from "../shared/FilterChip";
import { logDebug } from "../shared/logger";
import { MAX_NOTE_TAGS, normalizeTagName } from "../shared/noteTags";
import { fieldControlClassName } from "../shared/ui";

/** 可视化标签编辑浮层，把增删结果交给调用方写回 Markdown 正文。 */
export function NoteTagEditor({
  tags,
  availableTags,
  disabled = false,
  onChange,
}: {
  tags: string[];
  availableTags: string[];
  disabled?: boolean;
  onChange: (tags: string[]) => void;
}) {
  /** 输入框草稿只存在于浮层内部，确认后才写回正文。 */
  const [draft, setDraft] = useState("");
  /** 非法输入的短提示，不进入工作台全局 notice。 */
  const [error, setError] = useState("");
  const composingRef = useRef(false);
  const suggestions = availableTags.filter((tag) => !tags.includes(tag)).slice(0, 8);

  /** 把输入框或建议标签加入当前列表，并保持数量上限。 */
  function addTag(raw: string) {
    const name = normalizeTagName(raw);

    if (!name) {
      setError("标签不能包含空格或 #,[]{}: 等符号。");
      return;
    }

    if (tags.includes(name)) {
      setDraft("");
      setError("");
      return;
    }

    if (tags.length >= MAX_NOTE_TAGS) {
      setError(`最多添加 ${MAX_NOTE_TAGS} 个标签。`);
      return;
    }

    logDebug("添加笔记标签。", {
      category: "frontend",
      event: "note_tag_add",
      status: "completed",
      metadata: { tagCount: tags.length + 1 },
    });
    onChange([...tags, name]);
    setDraft("");
    setError("");
  }

  /** 从当前列表移除标签，正文由调用方同步删除对应 `#标签` 或 frontmatter。 */
  function removeTag(tag: string) {
    logDebug("移除笔记标签。", {
      category: "frontend",
      event: "note_tag_remove",
      status: "completed",
      metadata: { tagCount: Math.max(0, tags.length - 1) },
    });
    onChange(tags.filter((item) => item !== tag));
    setError("");
  }

  function handleSubmit(event: FormEvent) {
    event.preventDefault();
    addTag(draft);
  }

  function handleKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key !== "Enter" || composingRef.current) {
      return;
    }

    event.preventDefault();
    addTag(draft);
  }

  return (
    <div
      className="absolute top-[calc(100%+8px)] left-0 z-menu grid w-[min(420px,calc(100vw-48px))] gap-2.5 rounded-control border border-border bg-surface p-3 shadow-app-soft"
      role="dialog"
      aria-label="编辑文档标签"
    >
      <div className="flex flex-wrap gap-1.5">
        {tags.length ? (
          tags.map((tag) => (
            <Chip key={tag} onRemove={disabled ? undefined : () => removeTag(tag)} removeLabel={`移除标签 ${tag}`}>
              #{tag}
            </Chip>
          ))
        ) : (
          <span className="text-xs text-ink-muted">还没有标签。可以在下面添加，或在文末单独一行写 #标签。</span>
        )}
      </div>
      <form className="flex min-w-0 items-center gap-2" onSubmit={handleSubmit}>
        <input
          className={cn(fieldControlClassName, "min-h-[34px]")}
          value={draft}
          disabled={disabled}
          placeholder="输入标签，回车添加"
          aria-label="新标签"
          onChange={(event) => {
            setDraft(event.target.value);
            if (error) {
              setError("");
            }
          }}
          onCompositionStart={() => {
            composingRef.current = true;
          }}
          onCompositionEnd={() => {
            composingRef.current = false;
          }}
          onKeyDown={handleKeyDown}
        />
        <Button variant="ghost" size="compact" type="submit" disabled={disabled || !draft.trim()}>
          <Plus size={14} />
          添加
        </Button>
      </form>
      {error ? <p className="m-0 text-xs text-danger">{error}</p> : null}
      {suggestions.length ? (
        <div className="grid gap-1.5">
          <p className="m-0 text-[11px] font-bold tracking-[0.02em] text-ink-soft uppercase">本库已有标签</p>
          <div className="flex flex-wrap gap-1.5">
            {suggestions.map((tag) => (
              <FilterChip key={tag} disabled={disabled} onClick={() => addTag(tag)}>
                #{tag}
              </FilterChip>
            ))}
          </div>
        </div>
      ) : null}
      <p className="m-0 text-[11px] leading-[1.5] text-ink-soft">在文末单独一行写 #标签 也会同步到这里；保存草稿后写入本地文件。</p>
    </div>
  );
}
