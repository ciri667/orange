import { Cloud, Plus, ShieldCheck, X } from "lucide-react";
import { useId, useState } from "react";
import { Button } from "../../shared/Button";
import { cn } from "../../shared/cn";
import { SelectControl } from "../../shared/SelectControl";
import { fieldLabelClassName, settingsSectionClassName } from "../../shared/ui";
import { SettingsSectionHeader } from "../SettingsChrome";
import { ModelProviderCard } from "./model/ModelProviderCard";
import { ModelSettingSwitch } from "./model/ModelSettingSwitch";
import type { LlmProviderConfig, ModelApiKeyStatus, ProviderTemplate, UserSettings } from "../../shared/types";

/** 模型 Provider 和隐私分区，所有更改先写入父级草稿。 */
export function ModelSettingsSection({
  settingsDraft,
  providerTemplates,
  selectedTemplateId,
  modelApiKeyStatuses,
  apiKeyDraftByProvider,
  apiKeyVisibleByProvider,
  revealedApiKeyByProvider,
  isBusy,
  onModelEnabledChange,
  onPrivacyPolicyChange,
  onSelectedTemplateIdChange,
  onAddProviderFromTemplate,
  onProviderFieldChange,
  onSetDefaultProvider,
  onRequestRemoveProvider,
  onApiKeyDraftChange,
  onToggleApiKeyVisibility,
  onSaveApiKey,
  onRefreshProviderModels,
  onProviderModelEnabledChange,
  onAddProviderModel,
  onUpdateProviderModel,
  onRemoveProviderModel,
}: {
  settingsDraft: UserSettings;
  providerTemplates: ProviderTemplate[];
  selectedTemplateId: string;
  modelApiKeyStatuses: ModelApiKeyStatus[];
  apiKeyDraftByProvider: Record<string, string>;
  apiKeyVisibleByProvider: Record<string, boolean>;
  revealedApiKeyByProvider: Record<string, string>;
  isBusy: boolean;
  onModelEnabledChange: (enabled: boolean) => void;
  onPrivacyPolicyChange: (policy: UserSettings["privacyPolicy"]) => void;
  onSelectedTemplateIdChange: (templateId: string) => void;
  onAddProviderFromTemplate: (templateId: string) => void;
  onProviderFieldChange: (providerId: string, field: keyof LlmProviderConfig, value: string | boolean) => void;
  onSetDefaultProvider: (providerId: string) => void;
  onRequestRemoveProvider: (providerId: string) => void;
  onApiKeyDraftChange: (providerId: string, apiKey: string) => void;
  onToggleApiKeyVisibility: (providerId: string) => void | Promise<void>;
  onSaveApiKey: (providerId: string) => void | Promise<void>;
  onRefreshProviderModels: (providerId: string) => void | Promise<void>;
  onProviderModelEnabledChange: (providerId: string, modelId: string, enabled: boolean) => void;
  onAddProviderModel: (providerId: string, modelId: string, name: string, contextLength?: number) => string | null;
  onUpdateProviderModel: (
    providerId: string,
    originalId: string,
    nextId: string,
    nextName: string,
    contextLength?: number,
  ) => string | null;
  onRemoveProviderModel: (providerId: string, modelId: string) => string | null;
}) {
  const providers = settingsDraft.modelConfig.providers;
  // 仅调整展示顺序，让常用配置先出现，不改变保存时的服务商顺序。
  const displayProviders = [...providers].sort((left, right) =>
    Number(right.id === settingsDraft.modelConfig.defaultProviderId) - Number(left.id === settingsDraft.modelConfig.defaultProviderId),
  );
  const [isAddingProvider, setIsAddingProvider] = useState(false);
  const templateFormId = useId();
  const cloudDescriptionId = useId();

  return (
    <section className={cn(settingsSectionClassName, "@container gap-5")} aria-labelledby="model-settings-title">
      <SettingsSectionHeader
        kicker="Configuration"
        title="模型与隐私"
        titleId="model-settings-title"
        description="连接你的模型服务，选择适合 Agent 的默认模型。"
      />

      <div className="grid min-w-0 gap-4 rounded-xl border border-border bg-surface-warm p-4 @min-[680px]:grid-cols-[minmax(0,1fr)_minmax(220px,0.8fr)] @min-[680px]:gap-5">
        <div className="flex min-w-0 items-center gap-3">
          <span className="grid size-9 shrink-0 place-items-center rounded-control border border-border bg-surface text-ink-muted">
            <Cloud size={18} />
          </span>
          <div className="min-w-0 flex-1">
            <div className="text-[13px] font-semibold text-ink">启用云端模型</div>
            <p id={cloudDescriptionId} className="mb-0 mt-1 text-xs leading-relaxed text-ink-muted">
              {settingsDraft.modelConfig.enabled ? "让 Agent 使用已配置的模型服务" : "已关闭，Agent 仅使用本地规则回复"}
            </p>
          </div>
          <ModelSettingSwitch
            label="启用云端模型"
            describedBy={cloudDescriptionId}
            checked={settingsDraft.modelConfig.enabled}
            onChange={onModelEnabledChange}
          />
        </div>
        <label className="grid min-w-0 items-center gap-2 border-t border-border pt-4 @min-[680px]:grid-cols-[auto_minmax(0,1fr)] @min-[680px]:gap-3 @min-[680px]:border-l @min-[680px]:border-t-0 @min-[680px]:pl-5 @min-[680px]:pt-0">
          <span className="flex items-center gap-1.5 text-xs font-medium text-ink-muted"><ShieldCheck size={14} />隐私策略</span>
          <SelectControl value={settingsDraft.privacyPolicy} onChange={(event) => onPrivacyPolicyChange(event.target.value as UserSettings["privacyPolicy"])}>
            <option value="allow-selected-scope">仅使用已选范围</option>
            <option value="local-only">仅本地规则 Agent</option>
          </SelectControl>
        </label>
      </div>

      <div className="grid min-w-0 gap-3">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="flex items-center gap-2">
            <h4 className="m-0 text-sm font-semibold text-ink">模型服务商</h4>
            <span className="rounded-small bg-surface-muted px-1.5 py-0.5 text-[11px] tabular-nums text-ink-muted">{providers.length}</span>
            <span className="ml-1 text-xs text-ink-muted">兼容 OpenAI 协议</span>
          </div>
          <Button
            size="compact"
            className="border-border bg-surface"
            aria-expanded={isAddingProvider}
            aria-controls={templateFormId}
            onClick={() => setIsAddingProvider(!isAddingProvider)}
            disabled={isBusy}
          >
            <Plus size={14} />添加服务商
          </Button>
        </div>

        {isAddingProvider && (
          <div id={templateFormId} className="flex flex-wrap items-end gap-2 rounded-xl border border-border bg-surface-warm p-4">
            <label className={cn(fieldLabelClassName, "flex-1 basis-52 font-medium")}>
              <span>选择服务商模板</span>
              <SelectControl autoFocus value={selectedTemplateId} onChange={(event) => onSelectedTemplateIdChange(event.target.value)}>
                {providerTemplates.map((template) => <option key={template.templateId} value={template.templateId}>{template.name}</option>)}
              </SelectControl>
            </label>
            <Button variant="primary" size="compact" className="min-h-9" disabled={isBusy || !selectedTemplateId} onClick={() => {
              onAddProviderFromTemplate(selectedTemplateId);
              setIsAddingProvider(false);
            }}>
              <Plus size={14} />添加
            </Button>
            <Button variant="icon" className="min-h-9" aria-label="取消添加服务商" onClick={() => setIsAddingProvider(false)}><X size={15} /></Button>
          </div>
        )}

        {displayProviders.map((provider) => (
          <ModelProviderCard
            key={provider.id}
            provider={provider}
            isDefault={provider.id === settingsDraft.modelConfig.defaultProviderId}
            canRemove={providers.length > 1}
            keyStatus={modelApiKeyStatuses.find((status) => status.providerId === provider.id)}
            apiKeyDraft={apiKeyDraftByProvider[provider.id] ?? ""}
            isApiKeyVisible={apiKeyVisibleByProvider[provider.id] === true}
            revealedApiKey={revealedApiKeyByProvider[provider.id]}
            isBusy={isBusy}
            onFieldChange={(field, value) => onProviderFieldChange(provider.id, field, value)}
            onSetDefault={() => onSetDefaultProvider(provider.id)}
            onRemove={() => onRequestRemoveProvider(provider.id)}
            onApiKeyDraftChange={(value) => onApiKeyDraftChange(provider.id, value)}
            onToggleApiKeyVisibility={() => onToggleApiKeyVisibility(provider.id)}
            onSaveApiKey={() => onSaveApiKey(provider.id)}
            onRefreshModels={() => onRefreshProviderModels(provider.id)}
            onModelEnabledChange={(modelId, enabled) => onProviderModelEnabledChange(provider.id, modelId, enabled)}
            onAddModel={(modelId, name, contextLength) => onAddProviderModel(provider.id, modelId, name, contextLength)}
            onUpdateModel={(originalId, nextId, nextName, contextLength) => onUpdateProviderModel(provider.id, originalId, nextId, nextName, contextLength)}
            onRemoveModel={(modelId) => onRemoveProviderModel(provider.id, modelId)}
          />
        ))}
        {!providers.length && <p className="m-0 rounded-xl border border-dashed border-border p-8 text-center text-[13px] text-ink-muted">还没有模型服务商，点击「添加服务商」开始配置。</p>}
      </div>

      <p className="m-0 flex items-center gap-2 px-1 text-xs leading-relaxed text-ink-muted">
        <ShieldCheck size={14} className="shrink-0" />选择或确认后自动保存，文字输入在失焦或按回车后保存；API 密钥单独保存至系统安全存储。
      </p>
    </section>
  );
}
