import { Plus, Save, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "../../shared/Button";
import { cn } from "../../shared/cn";
import { ConfirmDialog } from "../../shared/ConfirmDialog";
import { OverflowTooltipText } from "../../shared/OverflowTooltipText";
import { SelectControl } from "../../shared/SelectControl";
import { ToggleRow } from "../../shared/ToggleRow";
import {
  fieldControlClassName,
  fieldLabelClassName,
  fieldTextareaClassName,
  sectionLabelClassName,
  settingsCardClassName,
  settingsSectionClassName,
} from "../../shared/ui";
import { SettingsSectionHeader } from "../SettingsChrome";
import type { AgentMemoryEntry, KnowledgeBase, KnowledgeBaseMemory } from "../../shared/types";

/** 跨会话记忆分类标签与值的映射，保持与后端 category 常量一致。 */
const MEMORY_CATEGORY_OPTIONS: Array<{ value: string; label: string }> = [
  { value: "noteStructure", label: "笔记结构" },
  { value: "tagConvention", label: "标签规范" },
  { value: "organization", label: "整理习惯" },
  { value: "convention", label: "知识库约定" },
  { value: "other", label: "其他偏好" },
];

/** 记忆分类值转中文标签，未识别值透传原值，保持 prompt 与 UI 一致。 */
function memoryCategoryLabel(category: string): string {
  return MEMORY_CATEGORY_OPTIONS.find((option) => option.value === category)?.label ?? category;
}

/** 新建一条跨会话记忆占位结构，id 在前端生成后随保存写回。 */
function buildBlankMemoryEntry(): AgentMemoryEntry {
  return {
    id: `mem-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    category: "other",
    content: "",
    source: "user",
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
  };
}

/** 为指定知识库构造默认记忆集合；新知识库默认关闭，需用户手动开启。 */
function buildBlankKnowledgeBaseMemory(knowledgeBaseId: string): KnowledgeBaseMemory {
  return {
    knowledgeBaseId,
    enabled: false,
    entries: [],
    updatedAt: new Date().toISOString(),
  };
}

/** 跨会话记忆设置分区，按知识库管理长期偏好；保存前由后端做敏感信息脱敏。 */
export function AgentMemorySettingsSection({
  knowledgeBases,
  knowledgeBaseMemories,
  isBusy,
  onSaveKnowledgeBaseMemory,
  onDeleteKnowledgeBaseMemory,
}: {
  knowledgeBases: KnowledgeBase[];
  knowledgeBaseMemories: KnowledgeBaseMemory[];
  isBusy: boolean;
  onSaveKnowledgeBaseMemory: (memory: KnowledgeBaseMemory) => Promise<KnowledgeBaseMemory> | KnowledgeBaseMemory;
  onDeleteKnowledgeBaseMemory: (knowledgeBaseId: string) => Promise<void> | void;
}) {
  /** 当前选中的知识库 ID，默认第一个已授权知识库。 */
  const [activeKnowledgeBaseId, setActiveKnowledgeBaseId] = useState<string>(
    knowledgeBases[0]?.id ?? "",
  );
  /** 当前选中知识库的记忆草稿，保存前可自由编辑。 */
  const initialDraft =
    knowledgeBaseMemories.find((memory) => memory.knowledgeBaseId === activeKnowledgeBaseId) ??
    buildBlankKnowledgeBaseMemory(activeKnowledgeBaseId);
  const [memoryDraft, setMemoryDraft] = useState<KnowledgeBaseMemory>(initialDraft);
  /** 待确认删除的知识库 ID；非空时展示危险确认弹窗，避免误删长期记忆。 */
  const [pendingDeleteKnowledgeBaseId, setPendingDeleteKnowledgeBaseId] = useState<string | null>(null);
  /** 当前选中的知识库对象，用于编辑区标题和状态展示。 */
  const activeKnowledgeBase = knowledgeBases.find((knowledgeBase) => knowledgeBase.id === activeKnowledgeBaseId);
  /** 待删除知识库名称，用于确认弹窗中明确展示影响范围。 */
  const pendingDeleteKnowledgeBaseName =
    knowledgeBases.find((knowledgeBase) => knowledgeBase.id === pendingDeleteKnowledgeBaseId)?.name ?? "该知识库";
  /** 当前知识库是否已有持久化记忆，用于决定删除按钮状态。 */
  const hasPersistedMemory = knowledgeBaseMemories.some((memory) => memory.knowledgeBaseId === activeKnowledgeBaseId);
  /** 当前草稿状态文案，保持列表卡片和编辑面板的信息密度一致。 */
  const activeMemorySummary = memoryDraft.enabled
    ? `${memoryDraft.entries.length} 条 · 已启用`
    : memoryDraft.entries.length
      ? `${memoryDraft.entries.length} 条 · 未启用`
      : "未配置";

  useEffect(() => {
    // 知识库列表可能在设置页打开后变化；当前选择失效时回到第一个可编辑知识库。
    if (!knowledgeBases.some((knowledgeBase) => knowledgeBase.id === activeKnowledgeBaseId)) {
      setActiveKnowledgeBaseId(knowledgeBases[0]?.id ?? "");
    }
  }, [activeKnowledgeBaseId, knowledgeBases]);

  useEffect(() => {
    // 父层保存或重新加载后会拿到后端归一化结果，当前草稿必须同步，避免继续展示敏感原文。
    setMemoryDraft(
      knowledgeBaseMemories.find((memory) => memory.knowledgeBaseId === activeKnowledgeBaseId) ??
        buildBlankKnowledgeBaseMemory(activeKnowledgeBaseId),
    );
  }, [activeKnowledgeBaseId, knowledgeBaseMemories]);

  useEffect(() => {
    // 删除目标若被外部刷新移除，关闭确认框，避免对已不存在的记忆重复发起删除。
    if (
      pendingDeleteKnowledgeBaseId &&
      !knowledgeBaseMemories.some((memory) => memory.knowledgeBaseId === pendingDeleteKnowledgeBaseId)
    ) {
      setPendingDeleteKnowledgeBaseId(null);
    }
  }, [knowledgeBaseMemories, pendingDeleteKnowledgeBaseId]);

  /** 切换知识库时重置草稿为该知识库的持久化值或空集合。 */
  function handleSelectKnowledgeBase(knowledgeBaseId: string) {
    if (knowledgeBaseId === activeKnowledgeBaseId) {
      return;
    }
    setActiveKnowledgeBaseId(knowledgeBaseId);
  }

  /** 切换当前知识库记忆总开关。 */
  function handleEnabledChange(enabled: boolean) {
    setMemoryDraft((draft) => ({ ...draft, enabled }));
  }

  /** 修改单条记忆条目字段。 */
  function handleEntryChange(entryId: string, field: keyof AgentMemoryEntry, value: string) {
    setMemoryDraft((draft) => ({
      ...draft,
      entries: draft.entries.map((entry) =>
        entry.id === entryId ? { ...entry, [field]: value, updatedAt: new Date().toISOString() } : entry,
      ),
    }));
  }

  /** 新增一条空白记忆条目。 */
  function handleAddEntry() {
    setMemoryDraft((draft) => ({
      ...draft,
      entries: [...draft.entries, buildBlankMemoryEntry()],
    }));
  }

  /** 删除指定记忆条目。 */
  function handleRemoveEntry(entryId: string) {
    setMemoryDraft((draft) => ({
      ...draft,
      entries: draft.entries.filter((entry) => entry.id !== entryId),
    }));
  }

  async function handleSave() {
    const saved = await onSaveKnowledgeBaseMemory({
      ...memoryDraft,
      knowledgeBaseId: activeKnowledgeBaseId,
    });
    // 后端会返回脱敏、截断和时间归一化后的结果，保存后立即回填避免 UI 继续展示敏感原文。
    setMemoryDraft(saved);
  }

  /** 请求删除当前知识库记忆；真实删除动作必须等确认弹窗通过后再执行。 */
  function handleRequestDelete() {
    if (!activeKnowledgeBaseId || !hasPersistedMemory) {
      return;
    }

    setPendingDeleteKnowledgeBaseId(activeKnowledgeBaseId);
  }

  /** 确认删除指定知识库的长期记忆，删除后该知识库不会再向 Agent 注入跨会话偏好。 */
  async function handleConfirmDelete() {
    const knowledgeBaseId = pendingDeleteKnowledgeBaseId;

    if (!knowledgeBaseId) {
      return;
    }

    await onDeleteKnowledgeBaseMemory(knowledgeBaseId);
    // 用户可能在确认框打开后切换知识库，只重置当前仍在编辑的草稿。
    if (knowledgeBaseId === activeKnowledgeBaseId) {
      setMemoryDraft(buildBlankKnowledgeBaseMemory(knowledgeBaseId));
    }
    setPendingDeleteKnowledgeBaseId(null);
  }

  return (
    <section className={settingsSectionClassName} aria-labelledby="agent-memory-settings-title">
      <SettingsSectionHeader
        kicker="Configuration"
        title="Agent 记忆"
        titleId="agent-memory-settings-title"
        description="管理每个知识库的跨会话长期偏好，默认关闭，开启后注入 Agent 上下文。"
        actions={
          <>
            <Button
              variant="primary"
              size="compact"
              onClick={handleSave}
              disabled={isBusy || !activeKnowledgeBaseId}
            >
              <Save size={14} />
              保存记忆
            </Button>
            <Button
              variant="text"
              tone="danger"
              className="min-h-[34px] px-2.5"
              onClick={handleRequestDelete}
              disabled={isBusy || !activeKnowledgeBaseId || !hasPersistedMemory}
            >
              <Trash2 size={14} />
              删除记忆
            </Button>
          </>
        }
      />
      <p className="m-0 max-w-[760px] text-[13px] leading-[1.55] text-ink-muted">
        适合保存：笔记结构、标签规范、整理习惯、已确认的知识库约定。请勿填写 API key、手机号、身份证、密码或私密正文片段——保存时会自动做敏感信息脱敏。
      </p>
      {knowledgeBases.length === 0 ? (
        <p className="m-0 text-[13px] text-ink-muted">暂无已授权知识库，请先在“知识库管理”中添加。</p>
      ) : (
        <div className="grid grid-cols-[minmax(180px,240px)_minmax(0,1fr)] items-start gap-3 max-[820px]:grid-cols-1">
          <div className="grid gap-2" aria-label="知识库记忆列表">
            {knowledgeBases.map((knowledgeBase) => {
              const memory = knowledgeBaseMemories.find((item) => item.knowledgeBaseId === knowledgeBase.id);
              const entryCount = memory?.entries.length ?? 0;
              const summary = memory?.enabled ? `${entryCount} 条 · 已启用` : entryCount ? `${entryCount} 条 · 未启用` : "未配置";
              const isActive = knowledgeBase.id === activeKnowledgeBaseId;

              return (
                <button
                  className={cn(
                    "grid w-full min-w-0 gap-1.5 rounded-control border border-border bg-[#fbfaf7] p-2.5 text-left text-ink",
                    "hover:enabled:border-primary-border hover:enabled:bg-surface-hover",
                    isActive && "border-primary-border bg-surface-hover shadow-[inset_3px_0_0_var(--accent)]",
                  )}
                  key={knowledgeBase.id}
                  type="button"
                  onClick={() => handleSelectKnowledgeBase(knowledgeBase.id)}
                  disabled={isBusy}
                  aria-pressed={isActive}
                >
                  <div className="flex min-w-0 items-center gap-2">
                    <OverflowTooltipText as="strong" className="min-w-0 truncate text-[13px] font-bold text-ink-strong" text={knowledgeBase.name} logArea="settings_memory_kb_name" />
                    <span
                      className={cn(
                        "inline-flex shrink-0 rounded-full border border-border bg-white/60 px-[7px] py-0.5 text-[11px] font-bold text-ink-muted",
                        memory?.enabled && "border-[rgba(47,111,104,0.28)] bg-accent-soft text-accent-strong",
                      )}
                    >
                      {memory?.enabled ? "启用" : "关闭"}
                    </span>
                  </div>
                  <span className="text-xs text-ink-muted">{summary}</span>
                </button>
              );
            })}
          </div>
          {activeKnowledgeBaseId && (
            <article className="grid min-w-0 gap-3 rounded-control border border-border bg-warm-panel p-3">
              <header className="flex min-w-0 items-center justify-between gap-3 max-[820px]:flex-wrap max-[820px]:items-start">
                <div className="flex min-w-0 items-center gap-2">
                  <strong className="min-w-0 truncate text-[13px] font-bold text-ink-strong">{activeKnowledgeBase?.name ?? "知识库"}</strong>
                  <span
                    className={cn(
                      "inline-flex shrink-0 rounded-full border border-border bg-white/60 px-[7px] py-0.5 text-[11px] font-bold text-ink-muted",
                      memoryDraft.enabled && "border-[rgba(47,111,104,0.28)] bg-accent-soft text-accent-strong",
                    )}
                  >
                    {activeMemorySummary}
                  </span>
                </div>
                <ToggleRow
                  compact
                  className="shrink-0"
                  checked={memoryDraft.enabled}
                  disabled={isBusy}
                  onChange={handleEnabledChange}
                >
                  注入 Agent 上下文
                </ToggleRow>
              </header>
              <div className="grid gap-2">
                {memoryDraft.entries.length === 0 ? (
                  <p className="m-0 rounded-[7px] border border-dashed border-border bg-[#fbfcfd] p-2.5 text-[13px] text-ink-muted">尚未添加记忆条目。</p>
                ) : (
                  memoryDraft.entries.map((entry) => (
                    <article className="grid min-w-0 gap-2 rounded-control border border-border-translucent bg-surface p-2.5" key={entry.id}>
                      <div className="flex min-w-0 items-end gap-2 max-[820px]:flex-wrap max-[820px]:items-start">
                        <label className={cn(fieldLabelClassName, "min-w-0 flex-[1_1_180px] gap-[5px]")}>
                          <span className={sectionLabelClassName}>分类</span>
                          <SelectControl
                            className="min-h-8 text-xs leading-8"
                            value={entry.category}
                            onChange={(event) => handleEntryChange(entry.id, "category", event.target.value)}
                            disabled={isBusy}
                          >
                            {MEMORY_CATEGORY_OPTIONS.map((option) => (
                              <option key={option.value} value={option.value}>
                                {option.label}
                              </option>
                            ))}
                          </SelectControl>
                        </label>
                        <span className="inline-flex min-h-8 items-center whitespace-nowrap rounded-control border border-border bg-white/70 px-[9px] text-xs font-bold text-ink-muted">
                          {entry.source === "auto" ? "自动生成" : "用户录入"}
                        </span>
                        <Button
                          variant="icon"
                          tone="danger"
                          className="size-8 min-h-8"
                          onClick={() => handleRemoveEntry(entry.id)}
                          disabled={isBusy}
                          title="删除条目"
                          aria-label="删除记忆条目"
                        >
                          <Trash2 size={14} />
                        </Button>
                      </div>
                      <textarea
                        className={cn(fieldTextareaClassName, "min-h-[76px] text-[13px] leading-normal")}
                        value={entry.content}
                        onChange={(event) => handleEntryChange(entry.id, "content", event.target.value)}
                        placeholder={`例如：${memoryCategoryLabel(entry.category)}的约定`}
                        rows={3}
                        disabled={isBusy}
                      />
                    </article>
                  ))
                )}
                <div className="flex justify-start">
                  <Button variant="ghost" size="compact" onClick={handleAddEntry} disabled={isBusy}>
                    <Plus size={14} />
                    新增条目
                  </Button>
                </div>
              </div>
            </article>
          )}
        </div>
      )}
      {pendingDeleteKnowledgeBaseId && (
        <ConfirmDialog
          title="删除 Agent 记忆"
          message={`删除「${pendingDeleteKnowledgeBaseName}」的跨会话记忆？删除后该知识库不会再向 Agent 注入这些长期偏好和约定。`}
          confirmLabel="删除记忆"
          tone="danger"
          isBusy={isBusy}
          onCancel={() => setPendingDeleteKnowledgeBaseId(null)}
          onConfirm={() => void handleConfirmDelete()}
        />
      )}
    </section>
  );
}
