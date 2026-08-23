import {
  FolderOpen,
  KeyRound,
  MessageCircle,
  Plus,
  RotateCw,
  Save,
  ShieldCheck,
  Sparkles,
  Star,
  Trash2,
} from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "../shared/Button";
import { Checkbox } from "../shared/Checkbox";
import { cn } from "../shared/cn";
import { ConfirmDialog } from "../shared/ConfirmDialog";
import { listRowClassName } from "../shared/ListRow";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import { SelectControl } from "../shared/SelectControl";
import { ToggleRow } from "../shared/ToggleRow";
import {
  fieldControlClassName,
  fieldLabelClassName,
  fieldTextareaClassName,
  sectionLabelClassName,
  settingsCardClassName,
  settingsSectionClassName,
} from "../shared/ui";
import { SettingsPolicyRow, SettingsSectionHeader, SettingsSubblockHeader } from "./SettingsChrome";
import type {
  AgentMemoryEntry,
  AgentSkill,
  AppEventLog,
  AppEventLogCategory,
  AppEventLogLevel,
  FeishuCredentialStatus,
  FeishuGatewayStatus,
  FeishuIntegrationSettings,
  KnowledgeBase,
  KnowledgeBaseMemory,
  LlmProviderConfig,
  ModelApiKeyStatus,
  ProviderTemplate,
  RequestAuditLog,
  UserSettings,
} from "../shared/types";

/** 知识库设置分区，管理目录授权、激活和重新扫描。 */
export function KnowledgeSettingsSection({
  knowledgeBases,
  activeKnowledgeBaseId,
  isBusy,
  onSelectKnowledgeBase,
  onAddKnowledgeBase,
  onRescanKnowledgeBase,
  onRemoveKnowledgeBase,
}: {
  knowledgeBases: KnowledgeBase[];
  activeKnowledgeBaseId: string;
  isBusy: boolean;
  onSelectKnowledgeBase: (knowledgeBaseId: string) => void;
  onAddKnowledgeBase: () => void;
  onRescanKnowledgeBase: (knowledgeBaseId: string) => void;
  onRemoveKnowledgeBase: (knowledgeBaseId: string) => void;
}) {
  return (
    <section className={settingsSectionClassName} aria-labelledby="knowledge-settings-title">
      <SettingsSectionHeader
        kicker="Configuration"
        title="知识库管理"
        titleId="knowledge-settings-title"
        description="管理已授权目录、激活知识库和本地索引刷新。"
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
                <ScanReportDetails knowledgeBase={knowledgeBase} />
              </div>
              <div className="flex items-center gap-2">
                <Button variant="ghost" size="compact" onClick={() => onSelectKnowledgeBase(knowledgeBase.id)} disabled={isBusy}>
                  激活
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

/** 模型 Provider 和隐私分区，所有更改先写入父级草稿。 */
export function ModelSettingsSection({
  settingsDraft,
  providerTemplates,
  selectedTemplateId,
  modelApiKeyStatuses,
  apiKeyDraftByProvider,
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
  onSaveApiKey,
  onRefreshProviderModels,
  onProviderModelEnabledChange,
}: {
  settingsDraft: UserSettings;
  providerTemplates: ProviderTemplate[];
  selectedTemplateId: string;
  modelApiKeyStatuses: ModelApiKeyStatus[];
  apiKeyDraftByProvider: Record<string, string>;
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
  onSaveApiKey: (providerId: string) => void | Promise<void>;
  onRefreshProviderModels: (providerId: string) => void | Promise<void>;
  onProviderModelEnabledChange: (providerId: string, modelId: string, enabled: boolean) => void;
}) {
  const providers = settingsDraft.modelConfig.providers;
  /** 每个 provider 模型列表的本地搜索词，只影响当前设置页渲染，不进入持久化配置。 */
  const [modelSearchByProvider, setModelSearchByProvider] = useState<Record<string, string>>({});

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
            const enabledModels = provider.models.filter((model) => model.enabled);
            const modelSearch = modelSearchByProvider[provider.id] ?? "";
            const filteredModels = provider.models.filter((model) => {
              const searchableText = [model.id, model.name, model.ownedBy ?? "", model.source].join(" ").toLowerCase();

              return searchableText.includes(modelSearch.trim().toLowerCase());
            });
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
                        value={provider.model}
                        onChange={(event) => onProviderFieldChange(provider.id, "model", event.target.value)}
                        placeholder="gpt-4o-mini"
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
                    <label className={cn(fieldLabelClassName, "col-span-full")}>
                      <span>API key</span>
                      <div className="grid grid-cols-[minmax(0,1fr)_auto] gap-2 max-[820px]:grid-cols-1">
                        <input
                          className={cn(fieldControlClassName, "tracking-[0.02em]")}
                          value={apiKeyDraft}
                          onChange={(event) => onApiKeyDraftChange(provider.id, event.target.value)}
                          placeholder="sk-..."
                          type="password"
                        />
                        <Button variant="ghost" size="compact" onClick={() => onSaveApiKey(provider.id)} disabled={isBusy || !apiKeyDraft.trim()}>
                          <KeyRound size={13} />
                          保存密钥
                        </Button>
                      </div>
                      <div className={cn("mt-2 flex items-center gap-1.5 text-xs", keyStatus?.configured ? "text-success" : "text-warning")}>
                        <KeyRound size={13} />
                        <OverflowTooltipText text={keyStatus?.message ?? "尚未读取模型密钥状态。"} logArea="settings_model_key_status" />
                      </div>
                    </label>
                  )}
                  {provider.models.length > 0 && (
                    <div className="col-span-full grid gap-2.5 rounded-lg border border-border bg-white p-2.5">
                      <div className="grid grid-cols-[minmax(0,1fr)_minmax(160px,240px)] items-end gap-2.5 max-[820px]:grid-cols-1">
                        <div className="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1">
                          <span className="text-xs font-bold text-[#24323c]">可用模型</span>
                          <strong className="text-xs font-semibold text-ink-muted">
                            已启用 {enabledModels.length}/{provider.models.length}
                          </strong>
                          {provider.modelsFetchedAt && <small className="basis-full text-xs font-normal text-ink-muted">上次获取：{provider.modelsFetchedAt}</small>}
                        </div>
                        <input
                          className={fieldControlClassName}
                          value={modelSearch}
                          onChange={(event) =>
                            setModelSearchByProvider((current) => ({
                              ...current,
                              [provider.id]: event.target.value,
                            }))
                          }
                          placeholder="搜索模型"
                        />
                      </div>
                      <div className="grid max-h-[260px] min-w-0 gap-1.5 overflow-auto pr-0.5">
                        {filteredModels.length ? (
                          filteredModels.map((model) => (
                            <label
                              className="relative flex min-w-0 flex-wrap items-center gap-x-2 gap-y-[7px] rounded-[7px] border border-border bg-[#fbfcfd] p-2 text-xs font-normal text-ink-muted"
                              key={model.id}
                            >
                              <Checkbox
                                checked={model.enabled}
                                disabled={model.id === provider.model}
                                onChange={(event) => onProviderModelEnabledChange(provider.id, model.id, event.target.checked)}
                              />
                              <span className="grid min-w-0 flex-[1_1_220px] gap-0.5">
                                <OverflowTooltipText as="strong" className="truncate text-[13px] text-ink-strong" text={model.name || model.id} logArea="settings_model_name" />
                                <OverflowTooltipText as="code" className="truncate font-mono text-[11px] text-ink-muted" text={model.id} logArea="settings_model_id" />
                              </span>
                              <span
                                className={cn(
                                  "inline-flex max-w-full items-center rounded-full px-[7px] py-[3px] text-[11px] font-bold",
                                  model.source === "discovered" ? "bg-accent-soft text-accent" : "bg-[#f7f1e7] text-[#8a5b12]",
                                )}
                              >
                                {model.source === "manual" ? "手动" : "发现"}
                              </span>
                              {model.contextLength ? <span className="inline-flex max-w-full items-center rounded-full bg-[#f4f6f8] px-[7px] py-[3px] text-[11px] font-bold">{model.contextLength.toLocaleString()} ctx</span> : null}
                              {model.ownedBy ? <span className="inline-flex max-w-full items-center rounded-full bg-[#f4f6f8] px-[7px] py-[3px] text-[11px] font-bold">{model.ownedBy}</span> : null}
                              {model.id === provider.model && (
                                <span className="inline-flex items-center rounded-full border border-primary-border-strong bg-accent-soft px-[9px] py-1 text-xs font-bold text-accent-strong">
                                  默认
                                </span>
                              )}
                            </label>
                          ))
                        ) : (
                          <p className="m-0 rounded-[7px] border border-dashed border-border bg-[#fbfcfd] p-2.5 text-[13px] text-ink-muted">没有匹配的模型。</p>
                        )}
                      </div>
                    </div>
                  )}
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

/** Skills 设置摘要分区，完整 CRUD 仍由 SkillsModal 承载。 */
export function SkillsSettingsSection({
  skills,
  enabledSkillCount,
  customSkillCount,
  isBusy,
  onOpenSkillsModal,
  onSaveSettings,
}: {
  skills: AgentSkill[];
  enabledSkillCount: number;
  customSkillCount: number;
  isBusy: boolean;
  onOpenSkillsModal: () => void;
  onSaveSettings: () => void | Promise<void>;
}) {
  return (
    <section className={settingsSectionClassName} aria-labelledby="skills-settings-title">
      <SettingsSectionHeader
        kicker="Configuration"
        title="Skills 能力"
        titleId="skills-settings-title"
        description="管理 Agent 可用能力和未显式选择时的匹配方式。"
        actions={
          <>
            <Button variant="ghost" onClick={onOpenSkillsModal}>
              <Sparkles size={14} />
              管理 Skills
            </Button>
            <Button variant="primary" size="compact" onClick={onSaveSettings} disabled={isBusy}>
              <Save size={14} />
              保存设置
            </Button>
          </>
        }
      />
      <div className="grid grid-cols-3 gap-2 max-[820px]:grid-cols-1">
        <div className="rounded-control border border-border-translucent bg-warm-panel p-2.5">
          <span className="block text-xs text-ink-muted">启用</span>
          <strong className="mt-1 block text-base text-ink-strong">
            {enabledSkillCount} / {skills.length}
          </strong>
        </div>
        <div className="rounded-control border border-border-translucent bg-warm-panel p-2.5">
          <span className="block text-xs text-ink-muted">Prompt 注入</span>
          <strong className="mt-1 block text-base text-ink-strong">{enabledSkillCount} 个</strong>
        </div>
        <div className="rounded-control border border-border-translucent bg-warm-panel p-2.5">
          <span className="block text-xs text-ink-muted">自定义 Skills</span>
          <strong className="mt-1 block text-base text-ink-strong">{customSkillCount} 个</strong>
        </div>
      </div>
    </section>
  );
}

/** Agent 权限分区解释三级放手策略，并保存隔离任务资源上限。 */
export function AgentSecuritySettingsSection({
  settingsDraft,
  isBusy,
  onSettingsChange,
  onSaveSettings,
}: {
  settingsDraft: UserSettings;
  isBusy: boolean;
  onSettingsChange: (settings: UserSettings) => void;
  onSaveSettings: () => void | Promise<void>;
}) {
  const security = settingsDraft.agentSecurity;

  /** 数字输入在 UI 层限制到合理范围，后端保存时仍会二次归一化。 */
  function updateLimit(
    key: keyof UserSettings["agentSecurity"]["resourceLimits"],
    value: number,
  ) {
    onSettingsChange({
      ...settingsDraft,
      agentSecurity: {
        ...security,
        resourceLimits: {
          ...security.resourceLimits,
          [key]: Number.isFinite(value) ? value : 0,
        },
      },
    });
  }

  return (
    <section className={settingsSectionClassName} aria-labelledby="agent-security-settings-title">
      <SettingsSectionHeader
        kicker="Security"
        title="Agent 权限"
        titleId="agent-security-settings-title"
        description="三级权限决定你愿意把多少执行权交给 Agent，好让模型把能力用出来。Skill 只是更高权限下可发挥的能力之一。权限在 Agent 协作区按会话切换。"
        actions={
          <Button variant="primary" size="compact" onClick={onSaveSettings} disabled={isBusy}>
            <Save size={14} />
            保存设置
          </Button>
        }
      />

      <div className="grid gap-2" aria-label="三级权限说明">
        <article className="grid gap-1 rounded-lg border border-border bg-surface-warm px-3 py-2.5">
          <strong className="text-[13px] text-ink-strong">基础</strong>
          <p className="m-0 text-xs leading-normal text-ink-muted">你盯紧每一步。Agent 只在知识库文档里工作，写入必须你确认。</p>
        </article>
        <article className="grid gap-1 rounded-lg border border-border bg-surface-warm px-3 py-2.5">
          <strong className="text-[13px] text-ink-strong">进阶</strong>
          <p className="m-0 text-xs leading-normal text-ink-muted">开始放手。Agent 可以做更多事（整理目录、运行 Skill 等），落盘前仍要你看一眼。</p>
        </article>
        <article className="grid gap-1 rounded-lg border border-border bg-surface-warm px-3 py-2.5">
          <strong className="text-[13px] text-ink-strong">完全</strong>
          <p className="m-0 text-xs leading-normal text-ink-muted">真正放手。校验通过后连续执行并自动落盘，让模型一次把任务做完。系统保护边界仍然有效。</p>
        </article>
      </div>

      <div className="grid gap-3">
        <SettingsSubblockHeader title="隔离任务上限" description="当 Agent 被允许运行隔离进程时，超过任一上限就终止该任务。" />
        <div className="grid grid-cols-2 gap-x-3.5 gap-y-3 max-[820px]:grid-cols-1">
          <label className={fieldLabelClassName}><span>超时（秒）</span><input className={fieldControlClassName} type="number" min={5} max={1800} value={security.resourceLimits.timeoutSeconds} onChange={(event) => updateLimit("timeoutSeconds", Number(event.target.value))} /></label>
          <label className={fieldLabelClassName}><span>内存（MB）</span><input className={fieldControlClassName} type="number" min={64} max={4096} value={security.resourceLimits.maxMemoryMb} onChange={(event) => updateLimit("maxMemoryMb", Number(event.target.value))} /></label>
          <label className={fieldLabelClassName}><span>进程数</span><input className={fieldControlClassName} type="number" min={1} max={64} value={security.resourceLimits.maxProcesses} onChange={(event) => updateLimit("maxProcesses", Number(event.target.value))} /></label>
          <label className={fieldLabelClassName}><span>产物（MB）</span><input className={fieldControlClassName} type="number" min={1} max={1024} value={security.resourceLimits.maxArtifactMb} onChange={(event) => updateLimit("maxArtifactMb", Number(event.target.value))} /></label>
        </div>
      </div>
    </section>
  );
}

/** 即时通讯设置分区，首版仅渲染飞书/Lark provider。 */
export function ImSettingsSection({
  knowledgeBases,
  feishu,
  feishuCredentialStatus,
  feishuGatewayStatus,
  feishuSecretDraft,
  isBusy,
  onFeishuDraftChange,
  onFeishuConfigDraftChange,
  onFeishuSecretDraftChange,
  onParseMultilineIds,
  onAllowDiscoveredFeishuUser,
  onAllowDiscoveredFeishuChat,
  onRefreshFeishuStatus,
  onStopFeishuGateway,
  onStartFeishuGateway,
  onSaveImSettings,
  onSaveFeishuSecret,
}: {
  knowledgeBases: KnowledgeBase[];
  feishu: FeishuIntegrationSettings;
  feishuCredentialStatus: FeishuCredentialStatus | null;
  feishuGatewayStatus: FeishuGatewayStatus | null;
  feishuSecretDraft: string;
  isBusy: boolean;
  onFeishuDraftChange: <K extends keyof FeishuIntegrationSettings>(field: K, value: FeishuIntegrationSettings[K]) => void;
  onFeishuConfigDraftChange: <K extends keyof FeishuIntegrationSettings["config"]>(
    field: K,
    value: FeishuIntegrationSettings["config"][K],
  ) => void;
  onFeishuSecretDraftChange: (value: string) => void;
  onParseMultilineIds: (value: string) => string[];
  onAllowDiscoveredFeishuUser: (openId: string) => void;
  onAllowDiscoveredFeishuChat: (chatId: string) => void;
  onRefreshFeishuStatus: () => void | Promise<void>;
  onStopFeishuGateway: () => void | Promise<void>;
  onStartFeishuGateway: () => void | Promise<void>;
  onSaveImSettings: () => void | Promise<void>;
  onSaveFeishuSecret: () => void | Promise<void>;
}) {
  const selectedKnowledgeBaseIds = new Set(feishu.defaultKnowledgeBaseIds);

  return (
    <section className={settingsSectionClassName} aria-labelledby="im-settings-title">
      <SettingsSectionHeader
        kicker="Configuration"
        title="即时通讯"
        titleId="im-settings-title"
        description="连接已注册的即时通讯 provider，允许白名单用户通过文本消息调用 Agent。"
        actions={
          <>
            <Button variant="ghost" onClick={onRefreshFeishuStatus} disabled={isBusy}>
              <RotateCw size={14} />
              刷新
            </Button>
            {feishuGatewayStatus?.running ? (
              <Button variant="ghost" tone="danger" onClick={onStopFeishuGateway} disabled={isBusy}>
                停止
              </Button>
            ) : (
              <Button variant="primary" size="compact" onClick={onStartFeishuGateway} disabled={isBusy}>
                启动
              </Button>
            )}
            <Button variant="primary" size="compact" onClick={onSaveImSettings} disabled={isBusy}>
              <Save size={14} />
              保存设置
            </Button>
          </>
        }
      />

      <div className="grid grid-cols-2 gap-x-3.5 gap-y-3 max-[820px]:grid-cols-1">
        <ToggleRow className="col-span-full" checked={feishu.enabled} onChange={(checked) => onFeishuDraftChange("enabled", checked)}>
          启用飞书/Lark 集成
        </ToggleRow>
        <label className={fieldLabelClassName}>
          <span>平台</span>
          <SelectControl value={feishu.config.domain} onChange={(event) => onFeishuConfigDraftChange("domain", event.target.value as "feishu" | "lark")}>
            <option value="feishu">飞书</option>
            <option value="lark">Lark</option>
          </SelectControl>
        </label>
        <label className={fieldLabelClassName}>
          <span>App ID</span>
          <input className={fieldControlClassName} value={feishu.config.appId} onChange={(event) => onFeishuConfigDraftChange("appId", event.target.value)} placeholder="cli_xxx" />
        </label>
        <label className={cn(fieldLabelClassName, "col-span-full")}>
          <span>App Secret</span>
          <div className="grid grid-cols-[minmax(0,1fr)_auto] gap-2">
            <input
              className={cn(fieldControlClassName, "tracking-[0.02em]")}
              type="password"
              value={feishuSecretDraft}
              onChange={(event) => onFeishuSecretDraftChange(event.target.value)}
              placeholder={feishuCredentialStatus?.configured ? "已保存，输入新值可替换" : "输入飞书 appSecret"}
            />
            <Button variant="ghost" onClick={onSaveFeishuSecret} disabled={isBusy || !feishuSecretDraft.trim()}>
              <KeyRound size={14} />
              保存密钥
            </Button>
          </div>
          <em className="font-normal not-italic">{feishuCredentialStatus?.message ?? "尚未读取凭证状态。"}</em>
        </label>
        <ToggleRow className="col-span-full" checked={feishu.requireMention} onChange={(checked) => onFeishuDraftChange("requireMention", checked)}>
          群聊必须直接 @ 机器人
        </ToggleRow>
      </div>

      <div className="grid gap-3">
        <SettingsSubblockHeader title="默认知识库范围" description="飞书消息只能检索这些知识库；写入类请求仍只生成待确认 diff。" />
        <div className="grid grid-cols-2 gap-2 max-[820px]:grid-cols-1">
          {knowledgeBases.map((knowledgeBase) => {
            const isSelected = selectedKnowledgeBaseIds.has(knowledgeBase.id);

            return (
              <label
                className={listRowClassName({
                  active: isSelected,
                  className: "relative border-border-translucent bg-surface-translucent",
                })}
                key={knowledgeBase.id}
              >
                <Checkbox
                  checked={isSelected}
                  onChange={() => {
                    const nextIds = new Set(feishu.defaultKnowledgeBaseIds);

                    // 多选范围允许用户手动增减；这里只更新草稿，保存/启动时再持久化。
                    if (nextIds.has(knowledgeBase.id)) {
                      nextIds.delete(knowledgeBase.id);
                    } else {
                      nextIds.add(knowledgeBase.id);
                    }

                    onFeishuDraftChange("defaultKnowledgeBaseIds", Array.from(nextIds));
                  }}
                />
                <span className="min-w-0">
                  <OverflowTooltipText as="strong" className="block truncate text-ink-strong" text={knowledgeBase.name} logArea="settings_im_scope_name" />
                  <OverflowTooltipText
                    className="mt-[3px] block truncate text-xs text-ink-muted"
                    text={knowledgeBase.status === "error" ? "目录失效" : `${knowledgeBase.noteCount} 篇笔记`}
                    logArea="settings_im_scope_detail"
                  />
                </span>
              </label>
            );
          })}
        </div>
      </div>

      <div className="grid gap-3">
        <SettingsSubblockHeader title="待授权飞书对象" description="收到未授权消息后会自动出现在这里；点击允许后保存设置即可生效。" />
        {feishu.discoveredUserOpenIds.length || feishu.discoveredChatIds.length ? (
          <div className="grid gap-2">
            {feishu.discoveredUserOpenIds.map((openId, index) => (
              <div className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-2.5 rounded-[7px] border border-border bg-white p-2.5" key={openId}>
                <span className="grid min-w-0 gap-[3px]">
                  <strong className="text-[13px] text-ink-strong">用户候选 {index + 1}</strong>
                  <OverflowTooltipText className="font-mono text-xs text-ink-muted" text={formatIdentifierPreview(openId)} logArea="settings_im_discovered_user" />
                </span>
                <Button variant="ghost" size="compact" onClick={() => onAllowDiscoveredFeishuUser(openId)}>
                  允许用户
                </Button>
              </div>
            ))}
            {feishu.discoveredChatIds.map((chatId, index) => (
              <div className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-2.5 rounded-[7px] border border-border bg-white p-2.5" key={chatId}>
                <span className="grid min-w-0 gap-[3px]">
                  <strong className="text-[13px] text-ink-strong">群候选 {index + 1}</strong>
                  <OverflowTooltipText className="font-mono text-xs text-ink-muted" text={formatIdentifierPreview(chatId)} logArea="settings_im_discovered_chat" />
                </span>
                <Button variant="ghost" size="compact" onClick={() => onAllowDiscoveredFeishuChat(chatId)}>
                  允许群聊
                </Button>
              </div>
            ))}
          </div>
        ) : (
          <p className="m-0 text-[13px] text-ink-muted">暂无待授权对象。让用户或群先给机器人发送一条消息后刷新状态。</p>
        )}
      </div>

      <div className="grid grid-cols-2 gap-x-3.5 gap-y-3 max-[820px]:grid-cols-1">
        <label className={cn(fieldLabelClassName, "col-span-full")}>
          <span>允许用户 open_id</span>
          <textarea
            className={fieldTextareaClassName}
            value={feishu.allowedUserOpenIds.join("\n")}
            onChange={(event) => onFeishuDraftChange("allowedUserOpenIds", onParseMultilineIds(event.target.value))}
            rows={4}
            placeholder="ou_xxx，每行一个"
          />
        </label>
        <label className={cn(fieldLabelClassName, "col-span-full")}>
          <span>允许群 chat_id</span>
          <textarea
            className={fieldTextareaClassName}
            value={feishu.allowedChatIds.join("\n")}
            onChange={(event) => onFeishuDraftChange("allowedChatIds", onParseMultilineIds(event.target.value))}
            rows={4}
            placeholder="oc_xxx，每行一个；私聊可留空"
          />
        </label>
      </div>

      <SettingsPolicyRow icon={<MessageCircle size={16} />}>
        网关：{feishuGatewayStatus?.running ? "运行中" : "未运行"} / 连接：
        {feishuGatewayStatus?.connected ? "已收到事件" : "未确认"} / 平台：{feishuGatewayStatus?.domain ?? feishu.config.domain}
      </SettingsPolicyRow>
      <p className="m-0 text-[13px] text-ink-muted">
        待确认改动使用飞书审批卡片。请在飞书开发者后台启用长连接，并订阅 <code>im.message.receive_v1</code> 和 <code>card.action.trigger</code>，完成消息相关权限授权后再启动网关。
      </p>
      {feishuGatewayStatus?.lastError ? <p className="m-0 text-[13px] text-ink-muted">{feishuGatewayStatus.lastError}</p> : null}
    </section>
  );
}

/** 运行日志分区，支持级别/分类筛选、刷新和清空。 */
export function EventLogsSettingsSection({
  appEventLogs,
  eventLogLevel,
  eventLogCategory,
  isBusy,
  onEventLogLevelChange,
  onEventLogCategoryChange,
  onRefreshAppEventLogs,
  onClearAppEventLogs,
  onOpenAppLogFolder,
}: {
  appEventLogs: AppEventLog[];
  eventLogLevel: AppEventLogLevel | "";
  eventLogCategory: AppEventLogCategory | "";
  isBusy: boolean;
  onEventLogLevelChange: (level: AppEventLogLevel | "") => void;
  onEventLogCategoryChange: (category: AppEventLogCategory | "") => void;
  onRefreshAppEventLogs: () => void | Promise<void>;
  onClearAppEventLogs: () => void | Promise<void>;
  onOpenAppLogFolder: () => void | Promise<void>;
}) {
  return (
    <section className={settingsSectionClassName} aria-labelledby="event-log-settings-title">
      <SettingsSectionHeader
        kicker="Diagnostics"
        title="运行日志"
        titleId="event-log-settings-title"
        description="查看应用事件日志，按级别和分类筛选。"
        actions={
          <>
            <Button variant="ghost" onClick={onOpenAppLogFolder} disabled={isBusy}>
              <FolderOpen size={14} />
              文件日志
            </Button>
            <Button variant="ghost" onClick={onRefreshAppEventLogs} disabled={isBusy}>
              <RotateCw size={14} />
              刷新
            </Button>
            <Button variant="ghost" tone="danger" onClick={onClearAppEventLogs} disabled={isBusy}>
              <Trash2 size={14} />
              清空
            </Button>
          </>
        }
      />
      <div className="grid grid-cols-2 gap-2.5 max-[820px]:grid-cols-1">
        <label className={fieldLabelClassName}>
          <span>级别</span>
          <SelectControl value={eventLogLevel} onChange={(event) => onEventLogLevelChange(event.target.value as AppEventLogLevel | "")}>
            <option value="">全部</option>
            <option value="error">错误</option>
            <option value="warn">警告</option>
            <option value="info">信息</option>
            <option value="debug">调试</option>
          </SelectControl>
        </label>
        <label className={fieldLabelClassName}>
          <span>分类</span>
          <SelectControl value={eventLogCategory} onChange={(event) => onEventLogCategoryChange(event.target.value as AppEventLogCategory | "")}>
            <option value="">全部</option>
            <option value="app">应用</option>
            <option value="storage">存储</option>
            <option value="knowledge_base">知识库</option>
            <option value="editor">编辑器</option>
            <option value="agent">Agent</option>
            <option value="im">即时通讯</option>
            <option value="model">模型</option>
            <option value="skill">Skill</option>
            <option value="settings">设置</option>
            <option value="security">安全</option>
            <option value="frontend">前端</option>
          </SelectControl>
        </label>
      </div>
      <div className="grid gap-2.5">
        {appEventLogs.length ? appEventLogs.map((log) => <AppEventLogCard key={log.id} log={log} />) : <p className="m-0 text-[13px] text-ink-muted">暂无运行日志。</p>}
      </div>
    </section>
  );
}

/** 请求审计分区，展示模型请求和工具边界摘要。 */
export function AuditLogsSettingsSection({
  auditLogs,
  isBusy,
  onRefreshAuditLogs,
}: {
  auditLogs: RequestAuditLog[];
  isBusy: boolean;
  onRefreshAuditLogs: () => void | Promise<void>;
}) {
  return (
    <section className={settingsSectionClassName} aria-labelledby="audit-settings-title">
      <SettingsSectionHeader
        kicker="Diagnostics"
        title="请求审计"
        titleId="audit-settings-title"
        description="查看最近模型请求、本地规则回退和工具边界摘要。"
        actions={
          <Button variant="ghost" onClick={onRefreshAuditLogs} disabled={isBusy}>
            <RotateCw size={14} />
            刷新
          </Button>
        }
      />
      <div className="grid gap-2.5">
        {auditLogs.length ? auditLogs.map((log) => <AuditLogCard key={log.id} log={log} />) : <p className="m-0 text-[13px] text-ink-muted">暂无审计记录。</p>}
      </div>
    </section>
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

/** 单条审计日志卡片，展示请求类型、scope 摘要和工具调用摘要。 */
function AuditLogCard({ log }: { log: RequestAuditLog }) {
  return (
    <article className={cn(settingsCardClassName, "gap-1.5 p-2.5")}>
      <div className="flex min-w-0 items-center justify-between gap-2 text-[13px]">
        <OverflowTooltipText as="strong" className="min-w-0 truncate" text={formatAuditKind(log.kind)} logArea="settings_audit_kind" />
        <OverflowTooltipText className="min-w-0 truncate text-ink-muted" text={log.createdAt} logArea="settings_audit_created_at" />
      </div>
      <p className="m-0 text-ink">{log.scopeSummary}</p>
      <p className="m-0 text-ink">{log.contentSummary}</p>
      <OverflowTooltipText as="code" className="min-w-0 [overflow-wrap:anywhere] font-mono text-ink-muted [word-break:break-word]" text={log.toolSummary} logArea="settings_audit_tool_summary" />
    </article>
  );
}

/** 单条应用事件日志卡片，展示运行级别、分类、状态和脱敏上下文。 */
function AppEventLogCard({ log }: { log: AppEventLog }) {
  return (
    <article
      className={cn(
        settingsCardClassName,
        "gap-1.5 p-2.5",
        log.level === "error" && "border-[rgba(var(--danger-rgb),0.28)] bg-danger-soft",
        log.level === "warn" && "border-[rgba(var(--warning-rgb),0.28)] bg-warning-soft",
        log.level === "debug" && "bg-surface-muted",
      )}
    >
      <div className="flex min-w-0 items-center justify-between gap-2 text-[13px]">
        <OverflowTooltipText
          as="strong"
          className="min-w-0 truncate"
          text={`${formatEventLogLevel(log.level)} · ${formatEventLogCategory(log.category)}`}
          logArea="settings_event_log_kind"
        />
        <OverflowTooltipText className="min-w-0 truncate text-ink-muted" text={log.createdAt} logArea="settings_event_log_created_at" />
      </div>
      <OverflowTooltipText as="p" className="m-0 text-ink" text={`${formatEventStatus(log.status)} / ${log.event}`} logArea="settings_event_log_status" />
      <p className="m-0 text-ink">{log.message}</p>
      <OverflowTooltipText as="code" className="min-w-0 [overflow-wrap:anywhere] font-mono text-ink-muted [word-break:break-word]" text={formatEventLogContext(log)} logArea="settings_event_log_context" />
    </article>
  );
}

/** 把后端审计类型转成简短中文标签。 */
function formatAuditKind(kind: string) {
  const labels: Record<string, string> = {
    model_turn: "模型请求",
    model_error_turn: "模型失败",
    local_rule_turn: "本地规则",
    browser_mock_turn: "浏览器模拟",
  };

  return labels[kind] ?? kind;
}

/** 把运行日志级别转成设置页中文标签。 */
function formatEventLogLevel(level: AppEventLogLevel) {
  const labels: Record<AppEventLogLevel, string> = {
    debug: "调试",
    info: "信息",
    warn: "警告",
    error: "错误",
  };

  return labels[level];
}

/** 把运行日志分类转成设置页中文标签。 */
function formatEventLogCategory(category: AppEventLogCategory) {
  const labels: Record<AppEventLogCategory, string> = {
    app: "应用",
    storage: "存储",
    knowledge_base: "知识库",
    editor: "编辑器",
    agent: "Agent",
    im: "即时通讯",
    model: "模型",
    skill: "Skill",
    settings: "设置",
    security: "安全",
    frontend: "前端",
  };

  return labels[category];
}

/** 把后端事件状态转成简短中文标签，保留未知状态原文便于排查。 */
function formatEventStatus(status: string) {
  const labels: Record<string, string> = {
    started: "开始",
    completed: "完成",
    failed: "失败",
    blocked: "阻止",
  };

  return labels[status] ?? status;
}

/** 设置页只展示飞书 ID 的短预览；完整 ID 保留在本地输入框和持久化配置中。 */
function formatIdentifierPreview(value: string) {
  const trimmed = value.trim();

  if (trimmed.length <= 12) {
    return trimmed || "未命名对象";
  }

  return `${trimmed.slice(0, 6)}...${trimmed.slice(-4)}`;
}

/** 汇总事件日志的轻量上下文，避免卡片中散落过多字段。 */
function formatEventLogContext(log: AppEventLog) {
  const parts = [
    log.operationId ? `op=${log.operationId}` : "",
    log.sessionId ? `session=${log.sessionId}` : "",
    log.knowledgeBaseId ? `kb=${log.knowledgeBaseId}` : "",
    log.entityType && log.entityId ? `${log.entityType}=${log.entityId}` : "",
    log.relativePath ? `path=${log.relativePath}` : "",
    typeof log.durationMs === "number" ? `${log.durationMs}ms` : "",
  ].filter(Boolean);

  return parts.length ? parts.join(" · ") : "无额外上下文";
}

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
