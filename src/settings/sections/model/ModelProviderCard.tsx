import { ChevronDown, Eye, EyeOff, KeyRound, Pencil, Server, Star, Trash2 } from "lucide-react";
import { useEffect, useId, useState, type ComponentProps } from "react";
import { Button } from "../../../shared/Button";
import { cn } from "../../../shared/cn";
import { OverflowTooltipText } from "../../../shared/OverflowTooltipText";
import { SelectControl } from "../../../shared/SelectControl";
import { ToggleRow } from "../../../shared/ToggleRow";
import { fieldControlClassName, fieldLabelClassName } from "../../../shared/ui";
import type { LlmProviderConfig, ModelApiKeyStatus } from "../../../shared/types";
import { ModelSettingSwitch } from "./ModelSettingSwitch";
import { ProviderModelsPanel } from "./ProviderModelsPanel";

/** 服务商卡片的交互继续委托设置草稿；本组件仅持有折叠状态。 */
type ModelProviderCardProps = ComponentProps<typeof ProviderModelsPanel> & {
  isDefault: boolean;
  canRemove: boolean;
  keyStatus?: ModelApiKeyStatus;
  apiKeyDraft: string;
  isApiKeyVisible: boolean;
  revealedApiKey?: string;
  onFieldChange: (field: keyof LlmProviderConfig, value: string | boolean) => void;
  onSetDefault: () => void;
  onRemove: () => void;
  onApiKeyDraftChange: (value: string) => void;
  onToggleApiKeyVisibility: () => void | Promise<void>;
  onSaveApiKey: () => void | Promise<void>;
};

/** 将连接信息与模型列表分区，折叠时仍保留服务商状态和默认模型摘要。 */
export function ModelProviderCard({
  provider, isDefault, canRemove, keyStatus, apiKeyDraft, isApiKeyVisible, revealedApiKey,
  isBusy, onFieldChange, onSetDefault, onRemove, onApiKeyDraftChange, onToggleApiKeyVisibility,
  onSaveApiKey, onRefreshModels, onModelEnabledChange, onAddModel, onUpdateModel, onRemoveModel,
}: ModelProviderCardProps) {
  // 首次聚焦默认服务商；新添加的服务商尚无密钥状态，也直接展开供配置。
  const [expanded, setExpanded] = useState(isDefault || !keyStatus);
  // 切换默认服务商后展示它的配置，仍允许用户随后手动折叠。
  useEffect(() => { if (isDefault) setExpanded(true); }, [isDefault]);
  const contentId = useId();
  const enabledModels = provider.models.filter((model) => model.enabled);
  const canSaveApiKey = Boolean(apiKeyDraft.trim()) && apiKeyDraft.trim() !== revealedApiKey;
  // 兼容尚未被模型列表收录的旧默认值，防止下拉框看起来已选择其他模型。
  const selectableDefaultModels = enabledModels.some((model) => model.id === provider.model) || !provider.model
    ? enabledModels
    : [...enabledModels, { id: provider.model, name: provider.model }];

  return (
    <article className="@container min-w-0 rounded-xl border border-border bg-surface" aria-label={`${provider.name}服务商配置`}>
      <header className={cn("flex flex-wrap items-center gap-3 px-4 py-3.5", expanded && "border-b border-border")}>
        <Button
          className="min-w-0 flex-1 justify-start gap-3 rounded-small px-0 py-0 text-left hover:enabled:bg-transparent"
          aria-expanded={expanded}
          aria-controls={contentId}
          aria-label={`${expanded ? "收起" : "展开"}${provider.name}配置`}
          onClick={() => setExpanded(!expanded)}
        >
          <span className={cn("grid size-10 shrink-0 place-items-center rounded-control", isDefault ? "bg-accent-soft text-accent" : "bg-surface-muted text-ink-muted")}><Server size={19} /></span>
          <span className="grid min-w-0 gap-1">
            <span className="flex min-w-0 items-center gap-2">
              <span className="truncate text-sm font-semibold text-ink">{provider.name || "未命名服务商"}</span>
              {isDefault && <span className="inline-flex shrink-0 items-center gap-1 rounded-small bg-accent-soft px-1.5 py-0.5 text-[10px] font-medium text-accent"><Star size={10} />默认</span>}
            </span>
            <span className="truncate text-[11px] font-normal text-ink-muted">{enabledModels.length} 个模型已启用{provider.model ? ` · ${provider.model}` : " · 尚未设置默认模型"}</span>
          </span>
          <ChevronDown size={14} className={cn("shrink-0 text-ink-soft transition-transform motion-reduce:transition-none", expanded && "rotate-180")} />
        </Button>
        <div className="ml-auto flex items-center gap-3">
          {!isDefault && <Button size="compact" onClick={onSetDefault} disabled={isBusy} className="text-ink-muted">设为默认</Button>}
          <span className="text-xs text-ink-muted">{provider.enabled ? "已启用" : "已停用"}</span>
          <ModelSettingSwitch checked={provider.enabled} disabled={isDefault || isBusy} label={`启用${provider.name}`} onChange={(value) => onFieldChange("enabled", value)} />
          <Button
            variant="icon" size="compact" className="text-ink-soft hover:enabled:text-danger"
            title={isDefault ? "请先将其他服务商设为默认，再移除此服务商" : "移除服务商"}
            aria-label={`移除${provider.name}`} onClick={onRemove} disabled={isDefault || !canRemove || isBusy}
          ><Trash2 size={14} /></Button>
        </div>
      </header>

      {/* 保持内容挂载，折叠再展开时保留模型搜索和未完成的编辑。 */}
      <div id={contentId} hidden={!expanded}>
        <div className="grid gap-3 p-4">
          <div className="flex flex-wrap items-center gap-3">
            <h5 className="m-0 text-xs font-semibold text-ink">连接设置</h5>
            <label className="ml-auto flex min-w-0 max-w-full items-center gap-1.5 text-ink-muted">
              <Pencil size={11} className="shrink-0" />
              <input
                className="w-40 min-w-0 rounded-small border border-transparent bg-transparent px-1.5 py-1 text-xs text-ink-muted hover:border-border focus:border-border"
                value={provider.name} onChange={(event) => onFieldChange("name", event.target.value)}
                aria-label="服务商名称" placeholder="服务商名称"
              />
            </label>
          </div>
          <div className="grid min-w-0 gap-4 @min-[640px]:grid-cols-2">
            <label className={cn(fieldLabelClassName, "gap-1.5 font-medium")}>
              <span>服务地址 <span className="ml-1 font-normal text-ink-soft">API Base</span></span>
              <input className={cn(fieldControlClassName, "text-[13px]")} value={provider.apiBase} onChange={(event) => onFieldChange("apiBase", event.target.value)} placeholder="https://api.openai.com/v1" spellCheck={false} />
            </label>
            <label className={cn(fieldLabelClassName, "gap-1.5 font-medium")}>
              <span>默认模型</span>
              {provider.models.length ? (
                <SelectControl value={provider.model} onChange={(event) => onFieldChange("model", event.target.value)}>
                  {!provider.model && <option value="" disabled>选择默认模型</option>}
                  {selectableDefaultModels.map((model) => <option key={model.id} value={model.id}>{model.name || model.id}</option>)}
                </SelectControl>
              ) : <input className={cn(fieldControlClassName, "text-[13px]")} value="" placeholder="先获取或添加模型" disabled readOnly />}
            </label>
          </div>
          {provider.requiresApiKey && (
            <div className="grid min-w-0 gap-1.5">
              <div className="flex flex-wrap items-center justify-between gap-1.5">
                <label htmlFor={`${contentId}-key`} className="text-xs font-medium text-ink-muted">API 密钥</label>
                <span className={cn("flex items-center gap-1 text-[11px]", keyStatus?.configured ? "text-success" : "text-ink-muted")} title={keyStatus?.message}>
                  <KeyRound size={11} />{keyStatus?.configured ? "已安全保存" : keyStatus ? "尚未配置" : "状态未读取"}
                </span>
              </div>
              <div className="flex min-w-0 flex-wrap items-center gap-2">
                <div className="relative min-w-0 flex-1 basis-52">
                  <input
                    id={`${contentId}-key`} className={cn(fieldControlClassName, "pr-10 text-[13px]")}
                    value={apiKeyDraft} onChange={(event) => onApiKeyDraftChange(event.target.value)}
                    placeholder={keyStatus?.configured ? "已保存，输入新密钥可替换" : "输入 API 密钥"}
                    type={isApiKeyVisible ? "text" : "password"} autoComplete="off" spellCheck={false}
                  />
                  <Button
                    variant="icon" size="compact" className="absolute right-1 top-1/2 -translate-y-1/2"
                    title={isApiKeyVisible ? "隐藏密钥" : "查看密钥"} aria-label={isApiKeyVisible ? "隐藏密钥" : "查看密钥"}
                    aria-pressed={isApiKeyVisible} onClick={() => void onToggleApiKeyVisibility()} disabled={isBusy}
                  >{isApiKeyVisible ? <EyeOff size={14} /> : <Eye size={14} />}</Button>
                </div>
                <Button size="compact" className="min-h-9 border-border px-3" onClick={onSaveApiKey} disabled={isBusy || !canSaveApiKey}><KeyRound size={13} />保存密钥</Button>
              </div>
              {keyStatus && !keyStatus.configured && <OverflowTooltipText className="text-xs text-warning" text={keyStatus.message} logArea="settings_model_key_status" />}
            </div>
          )}
          <div className="flex flex-wrap items-center gap-x-6 gap-y-3 pt-1">
            <ToggleRow compact className="text-ink-muted" checked={provider.supportsTools} onChange={(value) => onFieldChange("supportsTools", value)}>支持工具调用</ToggleRow>
            <ToggleRow compact className="text-ink-muted" checked={provider.requiresApiKey} onChange={(value) => onFieldChange("requiresApiKey", value)}>使用 API 密钥</ToggleRow>
            {!provider.requiresApiKey && <span className="text-[11px] text-ink-soft">适用于本地免鉴权服务</span>}
          </div>
        </div>
        <ProviderModelsPanel
          provider={provider} isBusy={isBusy} onRefreshModels={onRefreshModels}
          onModelEnabledChange={onModelEnabledChange} onAddModel={onAddModel} onUpdateModel={onUpdateModel} onRemoveModel={onRemoveModel}
        />
      </div>
    </article>
  );
}
