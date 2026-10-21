import { Check, Pencil, Plus, RotateCw, Search, Trash2, X } from "lucide-react";
import { useId, useRef, useState, type FormEvent } from "react";
import { Button } from "../../../shared/Button";
import { Checkbox } from "../../../shared/Checkbox";
import { cn } from "../../../shared/cn";
import { OverflowTooltipText } from "../../../shared/OverflowTooltipText";
import { parseContextLengthInput } from "../../../shared/providerModels";
import { fieldControlClassName, fieldLabelClassName } from "../../../shared/ui";
import type { LlmProviderConfig, LlmProviderModel } from "../../../shared/types";

/** 使用紧凑窗口数字减少列表噪声；完整 token 数仍通过提示保留。 */
const contextFormatter = new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 1 });

/** 单个服务商的模型列表；新增和编辑按需展开，模型变更仍交给父级草稿。 */
export function ProviderModelsPanel({
  provider, isBusy, onRefreshModels, onModelEnabledChange, onAddModel, onUpdateModel, onRemoveModel,
}: {
  provider: LlmProviderConfig;
  isBusy: boolean;
  onRefreshModels: () => void | Promise<void>;
  onModelEnabledChange: (modelId: string, enabled: boolean) => void;
  onAddModel: (modelId: string, name: string, contextLength?: number) => string | null;
  onUpdateModel: (originalId: string, nextId: string, nextName: string, contextLength?: number) => string | null;
  onRemoveModel: (modelId: string) => string | null;
}) {
  const enabledCount = provider.models.filter((model) => model.enabled).length;
  const [modelSearch, setModelSearch] = useState("");
  const [isAdding, setIsAdding] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [notice, setNotice] = useState("");
  const addButtonRef = useRef<HTMLButtonElement>(null);
  const addFormId = useId();
  const filteredModels = provider.models.filter((model) =>
    [model.id, model.name, model.ownedBy ?? "", model.source].join(" ").toLowerCase().includes(modelSearch.trim().toLowerCase()),
  );

  /** 删除错误就地显示，默认模型与发现模型的删除限制仍由原有领域逻辑处理。 */
  function removeModel(modelId: string) {
    const error = onRemoveModel(modelId);
    setNotice(error ?? "");
    if (!error && editingId === modelId) setEditingId(null);
  }

  return (
    <div className="grid min-w-0 gap-3 border-t border-border p-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <h5 className="m-0 text-xs font-semibold text-ink">可用模型</h5>
          <span className="text-[11px] tabular-nums text-ink-muted">{enabledCount} / {provider.models.length} 已启用</span>
        </div>
        <div className="flex items-center gap-1">
          <Button size="compact" className="text-ink-muted" title={provider.modelsFetchedAt ? `上次获取：${provider.modelsFetchedAt}` : undefined} onClick={onRefreshModels} disabled={isBusy || !provider.apiBase.trim()}>
            <RotateCw size={13} />获取模型
          </Button>
          <Button ref={addButtonRef} size="compact" className="text-accent hover:enabled:bg-accent-soft" aria-expanded={isAdding} aria-controls={addFormId} onClick={() => {
            setIsAdding(!isAdding);
            setEditingId(null);
            setNotice("");
          }} disabled={isBusy}>
            <Plus size={14} />手动添加
          </Button>
        </div>
      </div>
      <div className="relative min-w-0">
        <Search size={14} aria-hidden="true" className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-ink-soft" />
        <input className={cn(fieldControlClassName, "bg-surface-warm pl-9 pr-9 text-xs")} value={modelSearch} onChange={(event) => setModelSearch(event.target.value)} placeholder="搜索模型名称或 ID…" aria-label={`搜索${provider.name}的模型`} />
        {modelSearch && <Button variant="icon" size="compact" className="absolute right-1 top-1/2 -translate-y-1/2" aria-label="清空模型搜索" onClick={() => setModelSearch("")}><X size={13} /></Button>}
      </div>

      {isAdding && (
        <div id={addFormId} className="rounded-control border border-border bg-surface-warm p-3">
          <ModelForm
            key="add" isBusy={isBusy} onCancel={() => { setIsAdding(false); addButtonRef.current?.focus(); }}
            onSubmit={(id, name, contextLength) => {
              const error = onAddModel(id, name, contextLength);
              if (!error) {
                // 新增成功后清空筛选，避免新模型被旧搜索词隐藏。
                setModelSearch("");
                setIsAdding(false);
                addButtonRef.current?.focus();
              }
              return error;
            }}
          />
        </div>
      )}

      <div className="max-h-[320px] min-w-0 overflow-y-auto overscroll-contain">
        {filteredModels.length ? (
          <ul className="m-0 grid list-none p-0">
            {filteredModels.map((model) => {
              const isDefault = model.id === provider.model;
              const isManual = model.source === "manual";
              const isEditing = editingId === model.id;
              return (
                <li key={model.id} className="min-w-0 border-b border-border last:border-b-0">
                  <div className="flex min-w-0 items-center gap-3 rounded-small px-2 py-3 transition-colors hover:bg-surface-warm">
                    <label className="relative flex shrink-0 items-center" title={isDefault ? "默认模型必须保持启用" : undefined}>
                      <Checkbox checked={model.enabled} disabled={isBusy || isDefault} boxClassName="peer-disabled:border-border-strong peer-disabled:bg-surface-muted peer-disabled:text-ink-muted" aria-label={`启用模型 ${model.name || model.id}`} onChange={(event) => onModelEnabledChange(model.id, event.target.checked)} />
                    </label>
                    <div className="grid min-w-0 flex-1 gap-1">
                      <div className="flex min-w-0 items-center gap-2">
                        <OverflowTooltipText className={cn("truncate text-[13px] font-medium", model.enabled ? "text-ink" : "text-ink-muted")} text={model.name || model.id} logArea="settings_model_name" />
                        {isDefault && <span className="shrink-0 rounded-small bg-accent-soft px-1.5 py-0.5 text-[10px] text-accent">默认</span>}
                      </div>
                      {/* 同名 ID 不再重复占一行；不同的调用 ID 仍保留便于核对。 */}
                      {model.name && model.name !== model.id && <OverflowTooltipText as="code" className="truncate font-mono text-[11px] text-ink-muted" text={model.id} logArea="settings_model_id" />}
                      {model.ownedBy && <OverflowTooltipText className="truncate text-[11px] text-ink-soft" text={model.ownedBy} logArea="settings_model_owner" />}
                    </div>
                    <span className="hidden shrink-0 text-[11px] text-ink-soft @min-[520px]:inline">{isManual ? "手动" : "自动获取"}</span>
                    <span className="w-16 shrink-0 text-right text-[11px] tabular-nums text-ink-muted" title={model.contextLength ? `上下文窗口：${model.contextLength.toLocaleString()} tokens` : "尚未设置上下文窗口"}>
                      {model.contextLength ? `${contextFormatter.format(model.contextLength)} 上下文` : "窗口未设置"}
                    </span>
                    <div className="flex w-14 shrink-0 items-center justify-end">
                      <Button variant="icon" size="compact" title={isManual ? "编辑模型" : "填写上下文窗口"} aria-label={`编辑模型 ${model.name || model.id}`} aria-expanded={isEditing} onClick={() => {
                        setEditingId(isEditing ? null : model.id);
                        setIsAdding(false);
                        setNotice("");
                      }} disabled={isBusy}><Pencil size={13} /></Button>
                      {isManual && <Button variant="icon" size="compact" className="text-ink-soft hover:enabled:text-danger" title={isDefault ? "默认模型不能删除，请先更换默认模型" : "删除模型"} aria-label={`删除模型 ${model.name || model.id}`} onClick={() => removeModel(model.id)} disabled={isBusy || isDefault}><Trash2 size={13} /></Button>}
                    </div>
                  </div>
                  {isEditing && (
                    <div className="mb-3 rounded-control border border-border bg-surface-warm p-3">
                      <ModelForm model={model} isBusy={isBusy} onCancel={() => setEditingId(null)} onSubmit={(id, name, contextLength) => {
                        const error = onUpdateModel(model.id, id, name, contextLength);
                        if (!error) setEditingId(null);
                        return error;
                      }} />
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        ) : (
          <div className="grid justify-items-center gap-2 rounded-control bg-surface-warm px-4 py-6 text-center">
            <Search size={20} className="text-ink-soft" />
            <p className="m-0 text-xs text-ink-muted">{provider.models.length ? "没有找到匹配的模型，试试其他名称或 ID。" : "还没有可用模型，获取模型列表或手动添加一个。"}</p>
            {modelSearch && <Button size="compact" onClick={() => setModelSearch("")}>清空搜索</Button>}
          </div>
        )}
      </div>
      {notice && <p role="alert" className="m-0 text-xs text-danger">{notice}</p>}
    </div>
  );
}

/** 新增与编辑共用字段和窗口校验；自动发现模型只能修改上下文窗口。 */
function ModelForm({ model, isBusy, onSubmit, onCancel }: {
  model?: LlmProviderModel;
  isBusy: boolean;
  onSubmit: (id: string, name: string, contextLength?: number) => string | null;
  onCancel: () => void;
}) {
  const [modelId, setModelId] = useState(model?.id ?? "");
  const [name, setName] = useState(model?.name ?? "");
  const [contextLength, setContextLength] = useState(model?.contextLength ? String(model.contextLength) : "");
  const [error, setError] = useState("");
  const isDiscovered = model?.source === "discovered";

  /** 先解析可选上下文窗口，再调用现有模型增改校验；错误保留在当前表单。 */
  function handleSubmit(event: FormEvent) {
    event.preventDefault();
    if (isBusy) return;
    const parsedWindow = parseContextLengthInput(contextLength);
    if (!parsedWindow.ok) {
      setError(parsedWindow.error);
      return;
    }
    setError(onSubmit(modelId, name, parsedWindow.value) ?? "");
  }

  return (
    <form className="grid gap-3" onSubmit={handleSubmit} onKeyDown={(event) => {
      if (event.key === "Escape") { event.stopPropagation(); onCancel(); }
    }}>
      <div className="flex items-center justify-between gap-2">
        <span className="text-xs font-medium text-ink">{model ? "编辑模型" : "手动添加模型"}</span>
        <Button variant="icon" size="compact" aria-label="关闭模型表单" onClick={onCancel}><X size={13} /></Button>
      </div>
      <div className="grid gap-3 @min-[580px]:grid-cols-3">
        <label className={cn(fieldLabelClassName, "gap-1.5 font-medium")}>
          <span>模型 ID</span>
          <input className={cn(fieldControlClassName, "font-mono text-xs")} value={modelId} onChange={(event) => setModelId(event.target.value)} placeholder="例如 my-model" disabled={isBusy || isDiscovered} autoFocus={!isDiscovered} />
        </label>
        <label className={cn(fieldLabelClassName, "gap-1.5 font-medium")}>
          <span>显示名 <span className="font-normal text-ink-soft">可选</span></span>
          <input className={cn(fieldControlClassName, "text-xs")} value={name} onChange={(event) => setName(event.target.value)} placeholder="默认与 ID 相同" disabled={isBusy || isDiscovered} />
        </label>
        <label className={cn(fieldLabelClassName, "gap-1.5 font-medium")}>
          <span>上下文窗口 <span className="font-normal text-ink-soft">可选</span></span>
          <input className={cn(fieldControlClassName, "font-mono text-xs")} value={contextLength} onChange={(event) => setContextLength(event.target.value)} placeholder="例如 131072" inputMode="numeric" disabled={isBusy} autoFocus={isDiscovered} />
        </label>
      </div>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="text-[11px] text-ink-muted">{isDiscovered ? "自动获取的模型仅支持修改上下文窗口。" : "模型 ID 需与服务商提供的名称一致。"}</span>
        <div className="ml-auto flex items-center gap-1">
          <Button size="compact" onClick={onCancel}>取消</Button>
          <Button type="submit" variant="primary" size="compact" disabled={isBusy || !modelId.trim()}><Check size={13} />{model ? "完成编辑" : "添加模型"}</Button>
        </div>
      </div>
      {error && <p role="alert" className="m-0 text-xs text-danger">{error}</p>}
    </form>
  );
}
