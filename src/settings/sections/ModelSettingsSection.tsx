import { Check, Eye, EyeOff, KeyRound, Pencil, Plus, RotateCw, Save, ShieldCheck, Star, Trash2, X } from "lucide-react";
import { useState } from "react";
import { Button } from "../../shared/Button";
import { cn } from "../../shared/cn";
import { OverflowTooltipText } from "../../shared/OverflowTooltipText";
import { SelectControl } from "../../shared/SelectControl";
import { ToggleRow } from "../../shared/ToggleRow";
import { Checkbox } from "../../shared/Checkbox";
import { parseContextLengthInput } from "../../shared/providerModels";
import {
  fieldControlClassName,
  fieldLabelClassName,
  sectionLabelClassName,
  settingsCardClassName,
  settingsSectionClassName,
} from "../../shared/ui";
import { SettingsPolicyRow, SettingsSectionHeader, SettingsSubblockHeader } from "../SettingsChrome";
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
  onSaveSettings,
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
  onSaveSettings: () => void | Promise<void>;
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

  return (
    <section className={settingsSectionClassName} aria-labelledby="model-settings-title">
      <SettingsSectionHeader
        kicker="Configuration"
        title="模型与隐私"
        titleId="model-settings-title"
        description="多服务商管理（兼容 OpenAI 协议），支持指定默认服务商。"
        actions={
          <Button variant="primary" size="compact" onClick={onSaveSettings} disabled={isBusy}>
            <Save size={14} />
            保存设置
          </Button>
        }
      />
      <div className="grid grid-cols-2 gap-x-3.5 gap-y-3 max-[820px]:grid-cols-1">
        <ToggleRow className="col-span-full" checked={settingsDraft.modelConfig.enabled} onChange={onModelEnabledChange}>
          启用云端模型（关闭后 Agent 只使用本地规则回复）
        </ToggleRow>
        <label className={fieldLabelClassName}>
          <span>隐私策略</span>
          <SelectControl value={settingsDraft.privacyPolicy} onChange={(event) => onPrivacyPolicyChange(event.target.value as UserSettings["privacyPolicy"])}>
            <option value="allow-selected-scope">允许已选 scope</option>
            <option value="local-only">仅本地规则 Agent</option>
          </SelectControl>
        </label>
      </div>

      <div className="grid grid-cols-[minmax(0,1fr)_auto] items-stretch gap-2 max-[820px]:grid-cols-1">
        <SelectControl value={selectedTemplateId} onChange={(event) => onSelectedTemplateIdChange(event.target.value)}>
          {providerTemplates.map((template) => (
            <option key={template.templateId} value={template.templateId}>
              {template.name}
            </option>
          ))}
        </SelectControl>
        <Button
          variant="ghost"
          className="min-h-[var(--control-height)]"
          onClick={() => onAddProviderFromTemplate(selectedTemplateId)}
          disabled={!selectedTemplateId}
        >
          <Plus size={14} />
          新增 Provider
        </Button>
      </div>

      <div className="mt-1 grid gap-2.5">
        {providers.length ? (
          providers.map((provider) => {
            const keyStatus = modelApiKeyStatuses.find((status) => status.providerId === provider.id) ?? null;
            const isDefault = provider.id === settingsDraft.modelConfig.defaultProviderId;
            const apiKeyDraft = apiKeyDraftByProvider[provider.id] ?? "";
            const isApiKeyVisible = apiKeyVisibleByProvider[provider.id] === true;
            const revealedApiKey = revealedApiKeyByProvider[provider.id];
            const trimmedApiKeyDraft = apiKeyDraft.trim();
            const canSaveApiKey = Boolean(trimmedApiKeyDraft) && trimmedApiKeyDraft !== revealedApiKey;
            const enabledModels = provider.models.filter((model) => model.enabled);
            const selectableDefaultModels = enabledModels.some((model) => model.id === provider.model)
              ? enabledModels
              : provider.model
                ? [
                    ...enabledModels,
                    {
                      id: provider.model,
                      name: provider.model,
                      enabled: true,
                      source: "manual" as const,
                      updatedAt: provider.updatedAt,
                    },
                  ]
                : enabledModels;

            return (
              <article className={settingsCardClassName} key={provider.id}>
                <div className="flex flex-wrap items-center gap-2.5 max-[760px]:items-start">
                  <input
                    className="min-w-0 flex-[1_1_160px] rounded-small border-0 bg-transparent px-0 py-1 text-sm font-bold text-ink focus-visible:outline-[3px] focus-visible:outline-[var(--control-ring)] focus-visible:outline-offset-2"
                    value={provider.name}
                    onChange={(event) => onProviderFieldChange(provider.id, "name", event.target.value)}
                    placeholder="Provider 名称"
                  />
                  <div className="flex min-w-0 flex-wrap items-center gap-2">
                    {isDefault ? (
                      <span className="inline-flex items-center gap-1 rounded-full border border-primary-border-strong bg-accent-soft px-[9px] py-1 text-xs font-bold text-accent-strong">
                        <Star size={12} />
                        默认
                      </span>
                    ) : (
                      <Button variant="ghost" size="compact" onClick={() => onSetDefaultProvider(provider.id)}>
                        设为默认
                      </Button>
                    )}
                    <ToggleRow
                      compact
                      checked={provider.enabled}
                      disabled={isDefault}
                      onChange={(enabled) => onProviderFieldChange(provider.id, "enabled", enabled)}
                    >
                      启用
                    </ToggleRow>
                    <Button
                      variant="icon"
                      tone="danger"
                      title={isDefault ? "默认 Provider 不能直接删除，请先设为默认后再移除" : "移除 Provider"}
                      onClick={() => onRequestRemoveProvider(provider.id)}
                      disabled={isDefault || providers.length <= 1}
                    >
                      <Trash2 size={14} />
                    </Button>
                    <Button
                      variant="ghost"
                      size="compact"
                      onClick={() => onRefreshProviderModels(provider.id)}
                      disabled={isBusy || !provider.apiBase.trim()}
                    >
                      <RotateCw size={13} />
                      获取模型
                    </Button>
                  </div>
                </div>
                <div className="grid grid-cols-2 gap-x-3.5 gap-y-2.5 max-[820px]:grid-cols-1">
                  <label className={fieldLabelClassName}>
                    <span>API base</span>
                    <input
                      className={fieldControlClassName}
                      value={provider.apiBase}
                      onChange={(event) => onProviderFieldChange(provider.id, "apiBase", event.target.value)}
                      placeholder="https://api.openai.com/v1"
                    />
                  </label>
                  <label className={fieldLabelClassName}>
                    <span>默认模型</span>
                    {provider.models.length ? (
                      <SelectControl
                        value={provider.model}
                        onChange={(event) => onProviderFieldChange(provider.id, "model", event.target.value)}
                      >
                        {selectableDefaultModels.map((model) => (
                          <option key={model.id} value={model.id}>
                            {model.name || model.id}
                          </option>
                        ))}
                      </SelectControl>
                    ) : (
                      <input
                        className={fieldControlClassName}
                        value=""
                        placeholder="请先在下方添加模型"
                        disabled
                        readOnly
                      />
                    )}
                  </label>
                  <ToggleRow compact checked={provider.supportsTools} onChange={(checked) => onProviderFieldChange(provider.id, "supportsTools", checked)}>
                    支持工具调用（Function Calling）
                  </ToggleRow>
                  <ToggleRow compact checked={provider.requiresApiKey} onChange={(checked) => onProviderFieldChange(provider.id, "requiresApiKey", checked)}>
                    需要 API key（本地免鉴权服务可关闭）
                  </ToggleRow>
                  {provider.requiresApiKey && (
                    <div className={cn(fieldLabelClassName, "col-span-full")}>
                      <span>API key</span>
                      <div className="grid grid-cols-[minmax(0,1fr)_auto] gap-2 max-[820px]:grid-cols-1">
                        <div className="relative min-w-0">
                          <input
                            className={cn(fieldControlClassName, "pr-9 tracking-[0.02em]")}
                            value={apiKeyDraft}
                            onChange={(event) => onApiKeyDraftChange(provider.id, event.target.value)}
                            placeholder={keyStatus?.configured ? "已保存，输入新值可替换" : "sk-..."}
                            type={isApiKeyVisible ? "text" : "password"}
                            autoComplete="off"
                            spellCheck={false}
                            aria-label="API key"
                          />
                          <Button
                            variant="icon"
                            size="compact"
                            className="absolute right-1.5 top-1/2 -translate-y-1/2 border-transparent bg-transparent hover:enabled:border-transparent hover:enabled:bg-surface-hover"
                            title={isApiKeyVisible ? "隐藏密钥" : "查看密钥"}
                            aria-label={isApiKeyVisible ? "隐藏密钥" : "查看密钥"}
                            aria-pressed={isApiKeyVisible}
                            onClick={() => void onToggleApiKeyVisibility(provider.id)}
                            disabled={isBusy}
                          >
                            {isApiKeyVisible ? <EyeOff size={13} /> : <Eye size={13} />}
                          </Button>
                        </div>
                        <Button variant="ghost" size="compact" onClick={() => onSaveApiKey(provider.id)} disabled={isBusy || !canSaveApiKey}>
                          <KeyRound size={13} />
                          保存密钥
                        </Button>
                      </div>
                      <div className={cn("mt-2 flex items-center gap-1.5 text-xs", keyStatus?.configured ? "text-success" : "text-warning")}>
                        <KeyRound size={13} />
                        <OverflowTooltipText text={keyStatus?.message ?? "尚未读取模型密钥状态。"} logArea="settings_model_key_status" />
                      </div>
                    </div>
                  )}
                  <ProviderModelsPanel
                    provider={provider}
                    isBusy={isBusy}
                    onModelEnabledChange={(modelId, enabled) => onProviderModelEnabledChange(provider.id, modelId, enabled)}
                    onAddModel={(modelId, name, contextLength) => onAddProviderModel(provider.id, modelId, name, contextLength)}
                    onUpdateModel={(originalId, nextId, nextName, contextLength) =>
                      onUpdateProviderModel(provider.id, originalId, nextId, nextName, contextLength)
                    }
                    onRemoveModel={(modelId) => onRemoveProviderModel(provider.id, modelId)}
                  />
                </div>
              </article>
            );
          })
        ) : (
          <p className="m-0 text-[13px] text-ink-muted">暂无 Provider，先从上方模板新增一个。</p>
        )}
      </div>

      <SettingsPolicyRow icon={<ShieldCheck size={16} />}>
        Agent 写入工具只能生成 diff；用户确认后才执行路径校验、hash 校验和原子写入。
      </SettingsPolicyRow>
    </section>
  );
}

/** 单个 provider 的可用模型列表：发现的模型可启停并补窗口，手动模型支持增删改。 */
function ProviderModelsPanel({
  provider,
  isBusy,
  onModelEnabledChange,
  onAddModel,
  onUpdateModel,
  onRemoveModel,
}: {
  provider: LlmProviderConfig;
  isBusy: boolean;
  onModelEnabledChange: (modelId: string, enabled: boolean) => void;
  onAddModel: (modelId: string, name: string, contextLength?: number) => string | null;
  onUpdateModel: (originalId: string, nextId: string, nextName: string, contextLength?: number) => string | null;
  onRemoveModel: (modelId: string) => string | null;
}) {
  const enabledCount = provider.models.filter((model) => model.enabled).length;
  const [modelSearch, setModelSearch] = useState("");
  const [addId, setAddId] = useState("");
  const [addName, setAddName] = useState("");
  const [addContextLength, setAddContextLength] = useState("");
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editId, setEditId] = useState("");
  const [editName, setEditName] = useState("");
  const [editContextLength, setEditContextLength] = useState("");
  const [notice, setNotice] = useState("");

  const filteredModels = provider.models.filter((model) => {
    const searchableText = [model.id, model.name, model.ownedBy ?? "", model.source].join(" ").toLowerCase();

    return searchableText.includes(modelSearch.trim().toLowerCase());
  });

  /** 开始编辑一条模型，把当前 ID/显示名/窗口填进草稿。 */
  function beginEdit(modelId: string, modelName: string, contextLength?: number) {
    setEditingId(modelId);
    setEditId(modelId);
    setEditName(modelName);
    setEditContextLength(contextLength ? String(contextLength) : "");
    setNotice("");
  }

  /** 提交新增手动模型；成功后清空输入，失败则展示原因。 */
  function submitAdd() {
    const parsedWindow = parseContextLengthInput(addContextLength);
    if (!parsedWindow.ok) {
      setNotice(parsedWindow.error);
      return;
    }

    const error = onAddModel(addId, addName, parsedWindow.value);

    if (error) {
      setNotice(error);
      return;
    }

    setAddId("");
    setAddName("");
    setAddContextLength("");
    setNotice("");
  }

  /** 提交模型的 ID/显示名/窗口修改；发现的模型只能改窗口。 */
  function submitEdit() {
    if (!editingId) {
      return;
    }

    const parsedWindow = parseContextLengthInput(editContextLength);
    if (!parsedWindow.ok) {
      setNotice(parsedWindow.error);
      return;
    }

    const error = onUpdateModel(editingId, editId, editName, parsedWindow.value);

    if (error) {
      setNotice(error);
      return;
    }

    setEditingId(null);
    setNotice("");
  }

  /** 删除手动模型；默认模型会由上层拒绝并返回错误文案。 */
  function submitRemove(modelId: string) {
    const error = onRemoveModel(modelId);

    if (error) {
      setNotice(error);
      return;
    }

    if (editingId === modelId) {
      setEditingId(null);
    }

    setNotice("");
  }

  return (
    <div className="col-span-full grid gap-2.5 rounded-lg border border-border bg-white p-2.5">
      <div className="grid grid-cols-[minmax(0,1fr)_minmax(160px,240px)] items-end gap-2.5 max-[820px]:grid-cols-1">
        <div className="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1">
          <span className="text-xs font-bold text-[#24323c]">可用模型</span>
          <strong className="text-xs font-semibold text-ink-muted">
            已启用 {enabledCount}/{provider.models.length}
          </strong>
          {provider.modelsFetchedAt && <small className="basis-full text-xs font-normal text-ink-muted">上次获取：{provider.modelsFetchedAt}</small>}
        </div>
        <input
          className={fieldControlClassName}
          value={modelSearch}
          onChange={(event) => setModelSearch(event.target.value)}
          placeholder="搜索模型"
        />
      </div>
      <div className="grid max-h-[260px] min-w-0 gap-1.5 overflow-auto pr-0.5">
        {filteredModels.length ? (
          filteredModels.map((model) => {
            const isDefault = model.id === provider.model;
            const isManual = model.source === "manual";
            const isEditing = editingId === model.id;

            return (
              <div
                className="relative flex min-w-0 flex-wrap items-center gap-x-2 gap-y-[7px] rounded-[7px] border border-border bg-[#fbfcfd] p-2 text-xs font-normal text-ink-muted"
                key={model.id}
              >
                <label className="inline-flex items-center">
                  <Checkbox
                    checked={model.enabled}
                    disabled={isDefault}
                    onChange={(event) => onModelEnabledChange(model.id, event.target.checked)}
                  />
                </label>
                {isEditing ? (
                  <div className="grid min-w-0 flex-[1_1_220px] grid-cols-3 gap-1.5 max-[820px]:grid-cols-1">
                    <input
                      className={fieldControlClassName}
                      value={editName}
                      onChange={(event) => setEditName(event.target.value)}
                      onKeyDown={(event) => {
                        if (event.key === "Enter") {
                          event.preventDefault();
                          submitEdit();
                        }
                        if (event.key === "Escape") {
                          setEditingId(null);
                        }
                      }}
                      placeholder="显示名"
                      aria-label="模型显示名"
                      disabled={!isManual}
                    />
                    <input
                      className={cn(fieldControlClassName, "font-mono")}
                      value={editId}
                      onChange={(event) => setEditId(event.target.value)}
                      onKeyDown={(event) => {
                        if (event.key === "Enter") {
                          event.preventDefault();
                          submitEdit();
                        }
                        if (event.key === "Escape") {
                          setEditingId(null);
                        }
                      }}
                      placeholder="模型 ID"
                      aria-label="模型 ID"
                      disabled={!isManual}
                    />
                    <input
                      className={cn(fieldControlClassName, "font-mono")}
                      value={editContextLength}
                      onChange={(event) => setEditContextLength(event.target.value)}
                      onKeyDown={(event) => {
                        if (event.key === "Enter") {
                          event.preventDefault();
                          submitEdit();
                        }
                        if (event.key === "Escape") {
                          setEditingId(null);
                        }
                      }}
                      placeholder="窗口 tokens"
                      aria-label="上下文窗口"
                      inputMode="numeric"
                    />
                  </div>
                ) : (
                  <span className="grid min-w-0 flex-[1_1_220px] gap-0.5">
                    <OverflowTooltipText as="strong" className="truncate text-[13px] text-ink-strong" text={model.name || model.id} logArea="settings_model_name" />
                    <OverflowTooltipText as="code" className="truncate font-mono text-[11px] text-ink-muted" text={model.id} logArea="settings_model_id" />
                  </span>
                )}
                <span
                  className={cn(
                    "inline-flex max-w-full items-center rounded-full px-[7px] py-[3px] text-[11px] font-bold",
                    model.source === "discovered" ? "bg-accent-soft text-accent" : "bg-[#f7f1e7] text-[#8a5b12]",
                  )}
                >
                  {isManual ? "手动" : "发现"}
                </span>
                {model.contextLength ? <span className="inline-flex max-w-full items-center rounded-full bg-[#f4f6f8] px-[7px] py-[3px] text-[11px] font-bold">{model.contextLength.toLocaleString()} ctx</span> : null}
                {model.ownedBy ? <span className="inline-flex max-w-full items-center rounded-full bg-[#f4f6f8] px-[7px] py-[3px] text-[11px] font-bold">{model.ownedBy}</span> : null}
                {isDefault && (
                  <span className="inline-flex items-center rounded-full border border-primary-border-strong bg-accent-soft px-[9px] py-1 text-xs font-bold text-accent-strong">
                    默认
                  </span>
                )}
                <div className="ml-auto flex items-center gap-1">
                  {isEditing ? (
                    <>
                      <Button variant="icon" size="compact" title="保存模型" onClick={submitEdit} disabled={isBusy}>
                        <Check size={13} />
                      </Button>
                      <Button variant="icon" size="compact" title="取消编辑" onClick={() => setEditingId(null)}>
                        <X size={13} />
                      </Button>
                    </>
                  ) : (
                    <>
                      <Button
                        variant="icon"
                        size="compact"
                        title={isManual ? "编辑模型" : "填写上下文窗口"}
                        onClick={() => beginEdit(model.id, model.name || model.id, model.contextLength)}
                        disabled={isBusy}
                      >
                        <Pencil size={13} />
                      </Button>
                      {isManual ? (
                        <Button
                          variant="icon"
                          size="compact"
                          tone="danger"
                          className={isDefault ? "opacity-40" : undefined}
                          title={isDefault ? "默认模型不能删除，请先更换默认模型" : "删除模型"}
                          onClick={() => submitRemove(model.id)}
                          disabled={isBusy || isDefault}
                        >
                          <Trash2 size={13} />
                        </Button>
                      ) : null}
                    </>
                  )}
                </div>
              </div>
            );
          })
        ) : (
          <p className="m-0 rounded-[7px] border border-dashed border-border bg-[#fbfcfd] p-2.5 text-[13px] text-ink-muted">
            {provider.models.length ? "没有匹配的模型。" : "还没有模型。手动添加模型 ID，或点击「获取模型」。"}
          </p>
        )}
      </div>
      <div className="grid grid-cols-[minmax(0,1.1fr)_minmax(0,1fr)_minmax(0,0.9fr)_auto] items-end gap-2 max-[820px]:grid-cols-1">
        <label className={fieldLabelClassName}>
          <span>模型 ID</span>
          <input
            className={cn(fieldControlClassName, "font-mono")}
            value={addId}
            onChange={(event) => setAddId(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                submitAdd();
              }
            }}
            placeholder="例如 glm-5.2"
            disabled={isBusy}
          />
        </label>
        <label className={fieldLabelClassName}>
          <span>显示名（可选）</span>
          <input
            className={fieldControlClassName}
            value={addName}
            onChange={(event) => setAddName(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                submitAdd();
              }
            }}
            placeholder="和 ID 相同可留空"
            disabled={isBusy}
          />
        </label>
        <label className={fieldLabelClassName}>
          <span>上下文窗口（可选）</span>
          <input
            className={cn(fieldControlClassName, "font-mono")}
            value={addContextLength}
            onChange={(event) => setAddContextLength(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                submitAdd();
              }
            }}
            placeholder="例如 131072"
            disabled={isBusy}
            inputMode="numeric"
          />
        </label>
        <Button variant="ghost" size="compact" onClick={submitAdd} disabled={isBusy || !addId.trim()}>
          <Plus size={13} />
          添加模型
        </Button>
      </div>
      {notice ? <p className="m-0 text-xs text-danger">{notice}</p> : null}
    </div>
  );
}
