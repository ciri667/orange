import {
  Check,
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
import { ConfirmDialog } from "../shared/ConfirmDialog";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
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
    <section className="settings-section" aria-labelledby="knowledge-settings-title">
      <div className="settings-section-title settings-content-title">
        <div>
          <p className="section-label">Configuration</p>
          <h3 id="knowledge-settings-title">知识库管理</h3>
          <p>管理已授权目录、激活知识库和本地索引刷新。</p>
        </div>
        <button className="ghost-button" type="button" onClick={onAddKnowledgeBase}>
          <Plus size={15} />
          添加知识库
        </button>
      </div>
      <div className="settings-kb-list">
        {knowledgeBases.length ? (
          knowledgeBases.map((knowledgeBase) => (
            <article className="settings-kb-card" key={knowledgeBase.id}>
              <div>
                <div className="kb-card-title">
                  <OverflowTooltipText as="strong" text={knowledgeBase.name} logArea="settings_kb_name" />
                  <span>{knowledgeBase.status === "error" ? "目录失效" : knowledgeBase.semanticIndexEnabled ? "本地向量" : "FTS5"}</span>
                  {knowledgeBase.id === activeKnowledgeBaseId && <span>当前激活</span>}
                </div>
                <p>{knowledgeBase.description}</p>
                <OverflowTooltipText as="code" text={knowledgeBase.path} logArea="settings_kb_path" />
                <ScanReportDetails knowledgeBase={knowledgeBase} />
              </div>
              <div className="setting-actions">
                <button type="button" onClick={() => onSelectKnowledgeBase(knowledgeBase.id)} disabled={isBusy}>
                  激活
                </button>
                <button type="button" onClick={() => onRescanKnowledgeBase(knowledgeBase.id)} disabled={isBusy}>
                  <RotateCw size={13} />
                  重新扫描
                </button>
                <button
                  className="danger-action"
                  type="button"
                  onClick={() => onRemoveKnowledgeBase(knowledgeBase.id)}
                  disabled={isBusy}
                >
                  <Trash2 size={13} />
                  移除授权
                </button>
              </div>
            </article>
          ))
        ) : (
          <p className="settings-empty">暂无已授权知识库。</p>
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
    <section className="settings-section" aria-labelledby="model-settings-title">
      <div className="settings-section-title settings-content-title">
        <div>
          <p className="section-label">Configuration</p>
          <h3 id="model-settings-title">模型与隐私</h3>
          <p>管理多个 OpenAI-compatible Provider，选择默认 Provider 和发送边界。</p>
        </div>
        <button className="primary-button compact" type="button" onClick={onSaveSettings} disabled={isBusy}>
          <Save size={14} />
          保存设置
        </button>
      </div>
      <div className="settings-grid">
        <label className="toggle-row">
          <input
            className="control-checkbox-input"
            checked={settingsDraft.modelConfig.enabled}
            onChange={(event) => onModelEnabledChange(event.target.checked)}
            type="checkbox"
          />
          <span className="control-checkbox" aria-hidden="true" />
          <span>启用云端模型（关闭后 Agent 只使用本地规则回复）</span>
        </label>
        <label>
          <span>隐私策略</span>
          <span className="select-control">
            <select value={settingsDraft.privacyPolicy} onChange={(event) => onPrivacyPolicyChange(event.target.value as UserSettings["privacyPolicy"])}>
              <option value="allow-selected-scope">允许已选 scope</option>
              <option value="local-only">仅本地规则 Agent</option>
            </select>
          </span>
        </label>
      </div>

      <div className="provider-add-row">
        <span className="select-control">
          <select value={selectedTemplateId} onChange={(event) => onSelectedTemplateIdChange(event.target.value)}>
            {providerTemplates.map((template) => (
              <option key={template.templateId} value={template.templateId}>
                {template.name}
              </option>
            ))}
          </select>
        </span>
        <button
          className="ghost-button"
          type="button"
          onClick={() => onAddProviderFromTemplate(selectedTemplateId)}
          disabled={!selectedTemplateId}
        >
          <Plus size={14} />
          新增 Provider
        </button>
      </div>

      <div className="provider-list">
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
              <article className="provider-card" key={provider.id}>
                <div className="provider-card-header">
                  <input
                    className="provider-name-input"
                    value={provider.name}
                    onChange={(event) => onProviderFieldChange(provider.id, "name", event.target.value)}
                    placeholder="Provider 名称"
                  />
                  <div className="provider-card-badges">
                    {isDefault ? (
                      <span className="provider-badge default">
                        <Star size={12} />
                        默认
                      </span>
                    ) : (
                      <button className="ghost-button compact" type="button" onClick={() => onSetDefaultProvider(provider.id)}>
                        设为默认
                      </button>
                    )}
                    <label className="toggle-row compact">
                      <input
                        className="control-checkbox-input"
                        checked={provider.enabled}
                        onChange={(event) => onProviderFieldChange(provider.id, "enabled", event.target.checked)}
                        disabled={isDefault}
                        type="checkbox"
                      />
                      <span className="control-checkbox" aria-hidden="true" />
                      <span>启用</span>
                    </label>
                    <button
                      className="icon-button danger"
                      type="button"
                      title={isDefault ? "默认 Provider 不能直接删除，请先设为默认后再移除" : "移除 Provider"}
                      onClick={() => onRequestRemoveProvider(provider.id)}
                      disabled={isDefault || providers.length <= 1}
                    >
                      <Trash2 size={14} />
                    </button>
                    <button
                      className="ghost-button compact"
                      type="button"
                      onClick={() => onRefreshProviderModels(provider.id)}
                      disabled={isBusy || !provider.apiBase.trim()}
                    >
                      <RotateCw size={13} />
                      获取模型
                    </button>
                  </div>
                </div>
                <div className="provider-card-grid">
                  <label>
                    <span>API base</span>
                    <input
                      value={provider.apiBase}
                      onChange={(event) => onProviderFieldChange(provider.id, "apiBase", event.target.value)}
                      placeholder="https://api.openai.com/v1"
                    />
                  </label>
                  <label>
                    <span>默认模型</span>
                    {provider.models.length ? (
                      <span className="select-control">
                        <select
                          value={provider.model}
                          onChange={(event) => onProviderFieldChange(provider.id, "model", event.target.value)}
                        >
                          {selectableDefaultModels.map((model) => (
                            <option key={model.id} value={model.id}>
                              {model.name || model.id}
                            </option>
                          ))}
                        </select>
                      </span>
                    ) : (
                      <input
                        value={provider.model}
                        onChange={(event) => onProviderFieldChange(provider.id, "model", event.target.value)}
                        placeholder="gpt-4o-mini"
                      />
                    )}
                  </label>
                  <label className="toggle-row compact">
                    <input
                      className="control-checkbox-input"
                      checked={provider.supportsTools}
                      onChange={(event) => onProviderFieldChange(provider.id, "supportsTools", event.target.checked)}
                      type="checkbox"
                    />
                    <span className="control-checkbox" aria-hidden="true" />
                    <span>支持工具调用（Function Calling）</span>
                  </label>
                  <label className="toggle-row compact">
                    <input
                      className="control-checkbox-input"
                      checked={provider.requiresApiKey}
                      onChange={(event) => onProviderFieldChange(provider.id, "requiresApiKey", event.target.checked)}
                      type="checkbox"
                    />
                    <span className="control-checkbox" aria-hidden="true" />
                    <span>需要 API key（本地免鉴权服务可关闭）</span>
                  </label>
                  {provider.requiresApiKey && (
                    <label className="settings-full-row">
                      <span>API key</span>
                      <div className="key-save-row">
                        <input
                          value={apiKeyDraft}
                          onChange={(event) => onApiKeyDraftChange(provider.id, event.target.value)}
                          placeholder="sk-..."
                          type="password"
                        />
                        <button type="button" onClick={() => onSaveApiKey(provider.id)} disabled={isBusy || !apiKeyDraft.trim()}>
                          <KeyRound size={13} />
                          保存密钥
                        </button>
                      </div>
                      <div className={`key-status ${keyStatus?.configured ? "verified" : "missing"}`}>
                        <KeyRound size={13} />
                        <OverflowTooltipText text={keyStatus?.message ?? "尚未读取模型密钥状态。"} logArea="settings_model_key_status" />
                      </div>
                    </label>
                  )}
                  {provider.models.length > 0 && (
                    <div className="settings-full-row provider-models-panel">
                      <div className="provider-models-header">
                        <div>
                          <span>可用模型</span>
                          <strong>
                            已启用 {enabledModels.length}/{provider.models.length}
                          </strong>
                          {provider.modelsFetchedAt && <small>上次获取：{provider.modelsFetchedAt}</small>}
                        </div>
                        <input
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
                      <div className="provider-models-list">
                        {filteredModels.length ? (
                          filteredModels.map((model) => (
                            <label className="provider-model-row" key={model.id}>
                              <input
                                className="control-checkbox-input"
                                checked={model.enabled}
                                disabled={model.id === provider.model}
                                onChange={(event) => onProviderModelEnabledChange(provider.id, model.id, event.target.checked)}
                                type="checkbox"
                              />
                              <span className="control-checkbox" aria-hidden="true" />
                              <span className="provider-model-row-main">
                                <OverflowTooltipText as="strong" text={model.name || model.id} logArea="settings_model_name" />
                                <OverflowTooltipText as="code" text={model.id} logArea="settings_model_id" />
                              </span>
                              <span className={`provider-model-source ${model.source}`}>{model.source === "manual" ? "手动" : "发现"}</span>
                              {model.contextLength ? <span>{model.contextLength.toLocaleString()} ctx</span> : null}
                              {model.ownedBy ? <span>{model.ownedBy}</span> : null}
                              {model.id === provider.model && <span className="provider-badge default">默认</span>}
                            </label>
                          ))
                        ) : (
                          <p className="settings-empty compact">没有匹配的模型。</p>
                        )}
                      </div>
                    </div>
                  )}
                </div>
              </article>
            );
          })
        ) : (
          <p className="settings-empty">暂无 Provider，先从上方模板新增一个。</p>
        )}
      </div>

      <div className="policy-row">
        <ShieldCheck size={16} />
        <span>Agent 写入工具只能生成 diff；用户确认后才执行路径校验、hash 校验和原子写入。</span>
      </div>
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
    <section className="settings-section" aria-labelledby="skills-settings-title">
      <div className="settings-section-title settings-content-title">
        <div>
          <p className="section-label">Configuration</p>
          <h3 id="skills-settings-title">Skills 能力</h3>
          <p>管理 Agent 可用能力和未显式选择时的匹配方式。</p>
        </div>
        <div className="settings-title-actions">
          <button className="ghost-button" type="button" onClick={onOpenSkillsModal}>
            <Sparkles size={14} />
            管理 Skills
          </button>
          <button className="primary-button compact" type="button" onClick={onSaveSettings} disabled={isBusy}>
            <Save size={14} />
            保存设置
          </button>
        </div>
      </div>
      <div className="skills-summary">
        <div>
          <span>启用</span>
          <strong>
            {enabledSkillCount} / {skills.length}
          </strong>
        </div>
        <div>
          <span>Prompt 注入</span>
          <strong>{enabledSkillCount} 个</strong>
        </div>
        <div>
          <span>自定义 Skills</span>
          <strong>{customSkillCount} 个</strong>
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
    <section className="settings-section" aria-labelledby="im-settings-title">
      <div className="settings-section-title settings-content-title">
        <div>
          <p className="section-label">Configuration</p>
          <h3 id="im-settings-title">即时通讯</h3>
          <p>连接已注册的即时通讯 provider，允许白名单用户通过文本消息调用 Agent。</p>
        </div>
        <div className="settings-title-actions">
          <button className="ghost-button" type="button" onClick={onRefreshFeishuStatus} disabled={isBusy}>
            <RotateCw size={14} />
            刷新
          </button>
          {feishuGatewayStatus?.running ? (
            <button className="ghost-button danger-action" type="button" onClick={onStopFeishuGateway} disabled={isBusy}>
              停止
            </button>
          ) : (
            <button className="primary-button compact" type="button" onClick={onStartFeishuGateway} disabled={isBusy}>
              启动
            </button>
          )}
          <button className="primary-button compact" type="button" onClick={onSaveImSettings} disabled={isBusy}>
            <Save size={14} />
            保存设置
          </button>
        </div>
      </div>

      <div className="settings-grid">
        <label className="toggle-row settings-full-row">
          <input
            className="control-checkbox-input"
            checked={feishu.enabled}
            onChange={(event) => onFeishuDraftChange("enabled", event.target.checked)}
            type="checkbox"
          />
          <span className="control-checkbox" aria-hidden="true" />
          <span>启用飞书/Lark 集成</span>
        </label>
        <label>
          <span>平台</span>
          <span className="select-control">
            <select value={feishu.config.domain} onChange={(event) => onFeishuConfigDraftChange("domain", event.target.value as "feishu" | "lark")}>
              <option value="feishu">飞书</option>
              <option value="lark">Lark</option>
            </select>
          </span>
        </label>
        <label>
          <span>App ID</span>
          <input value={feishu.config.appId} onChange={(event) => onFeishuConfigDraftChange("appId", event.target.value)} placeholder="cli_xxx" />
        </label>
        <label className="settings-full-row">
          <span>App Secret</span>
          <div className="inline-secret-row">
            <input
              type="password"
              value={feishuSecretDraft}
              onChange={(event) => onFeishuSecretDraftChange(event.target.value)}
              placeholder={feishuCredentialStatus?.configured ? "已保存，输入新值可替换" : "输入飞书 appSecret"}
            />
            <button className="ghost-button" type="button" onClick={onSaveFeishuSecret} disabled={isBusy || !feishuSecretDraft.trim()}>
              <KeyRound size={14} />
              保存密钥
            </button>
          </div>
          <em>{feishuCredentialStatus?.message ?? "尚未读取凭证状态。"}</em>
        </label>
        <label className="toggle-row settings-full-row">
          <input
            className="control-checkbox-input"
            checked={feishu.requireMention}
            onChange={(event) => onFeishuDraftChange("requireMention", event.target.checked)}
            type="checkbox"
          />
          <span className="control-checkbox" aria-hidden="true" />
          <span>群聊必须直接 @ 机器人</span>
        </label>
      </div>

      <div className="settings-section-subblock">
        <div className="settings-section-title">
          <div>
            <h4>默认知识库范围</h4>
            <p>飞书消息只能检索这些知识库；写入类请求仍只生成待确认 diff。</p>
          </div>
        </div>
        <div className="settings-kb-list compact">
          {knowledgeBases.map((knowledgeBase) => {
            const isSelected = selectedKnowledgeBaseIds.has(knowledgeBase.id);

            return (
              <label className={isSelected ? "scope-option selected" : "scope-option"} key={knowledgeBase.id}>
                <input
                  type="checkbox"
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
                <span className="scope-check" aria-hidden="true">
                  {isSelected && <Check size={12} />}
                </span>
                <span className="scope-option-copy">
                  <OverflowTooltipText as="strong" text={knowledgeBase.name} logArea="settings_im_scope_name" />
                  <OverflowTooltipText
                    text={knowledgeBase.status === "error" ? "目录失效" : `${knowledgeBase.noteCount} 篇笔记`}
                    logArea="settings_im_scope_detail"
                  />
                </span>
              </label>
            );
          })}
        </div>
      </div>

      <div className="settings-section-subblock">
        <div className="settings-section-title">
          <div>
            <h4>待授权飞书对象</h4>
            <p>收到未授权消息后会自动出现在这里；点击允许后保存设置即可生效。</p>
          </div>
        </div>
        {feishu.discoveredUserOpenIds.length || feishu.discoveredChatIds.length ? (
          <div className="discovered-peer-list">
            {feishu.discoveredUserOpenIds.map((openId, index) => (
              <div className="discovered-peer-row" key={openId}>
                <span>
                  <strong>用户候选 {index + 1}</strong>
                  <OverflowTooltipText text={formatIdentifierPreview(openId)} logArea="settings_im_discovered_user" />
                </span>
                <button className="ghost-button compact" type="button" onClick={() => onAllowDiscoveredFeishuUser(openId)}>
                  允许用户
                </button>
              </div>
            ))}
            {feishu.discoveredChatIds.map((chatId, index) => (
              <div className="discovered-peer-row" key={chatId}>
                <span>
                  <strong>群候选 {index + 1}</strong>
                  <OverflowTooltipText text={formatIdentifierPreview(chatId)} logArea="settings_im_discovered_chat" />
                </span>
                <button className="ghost-button compact" type="button" onClick={() => onAllowDiscoveredFeishuChat(chatId)}>
                  允许群聊
                </button>
              </div>
            ))}
          </div>
        ) : (
          <p className="settings-empty">暂无待授权对象。让用户或群先给机器人发送一条消息后刷新状态。</p>
        )}
      </div>

      <div className="settings-grid">
        <label className="settings-full-row">
          <span>允许用户 open_id</span>
          <textarea
            value={feishu.allowedUserOpenIds.join("\n")}
            onChange={(event) => onFeishuDraftChange("allowedUserOpenIds", onParseMultilineIds(event.target.value))}
            rows={4}
            placeholder="ou_xxx，每行一个"
          />
        </label>
        <label className="settings-full-row">
          <span>允许群 chat_id</span>
          <textarea
            value={feishu.allowedChatIds.join("\n")}
            onChange={(event) => onFeishuDraftChange("allowedChatIds", onParseMultilineIds(event.target.value))}
            rows={4}
            placeholder="oc_xxx，每行一个；私聊可留空"
          />
        </label>
      </div>

      <div className="policy-row">
        <MessageCircle size={16} />
        <span>
          网关：{feishuGatewayStatus?.running ? "运行中" : "未运行"} / 连接：
          {feishuGatewayStatus?.connected ? "已收到事件" : "未确认"} / 平台：{feishuGatewayStatus?.domain ?? feishu.config.domain}
        </span>
      </div>
      {feishuGatewayStatus?.lastError ? <p className="settings-empty">{feishuGatewayStatus.lastError}</p> : null}
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
    <section className="settings-section" aria-labelledby="event-log-settings-title">
      <div className="settings-section-title settings-content-title">
        <div>
          <p className="section-label">Diagnostics</p>
          <h3 id="event-log-settings-title">运行日志</h3>
          <p>查看应用事件日志，按级别和分类筛选。</p>
        </div>
        <div className="settings-title-actions">
          <button className="ghost-button" type="button" onClick={onOpenAppLogFolder} disabled={isBusy}>
            <FolderOpen size={14} />
            文件日志
          </button>
          <button className="ghost-button" type="button" onClick={onRefreshAppEventLogs} disabled={isBusy}>
            <RotateCw size={14} />
            刷新
          </button>
          <button className="ghost-button danger-action" type="button" onClick={onClearAppEventLogs} disabled={isBusy}>
            <Trash2 size={14} />
            清空
          </button>
        </div>
      </div>
      <div className="event-log-filters">
        <label>
          <span>级别</span>
          <span className="select-control">
            <select value={eventLogLevel} onChange={(event) => onEventLogLevelChange(event.target.value as AppEventLogLevel | "")}>
              <option value="">全部</option>
              <option value="error">错误</option>
              <option value="warn">警告</option>
              <option value="info">信息</option>
              <option value="debug">调试</option>
            </select>
          </span>
        </label>
        <label>
          <span>分类</span>
          <span className="select-control">
            <select value={eventLogCategory} onChange={(event) => onEventLogCategoryChange(event.target.value as AppEventLogCategory | "")}>
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
            </select>
          </span>
        </label>
      </div>
      <div className="audit-list">
        {appEventLogs.length ? appEventLogs.map((log) => <AppEventLogCard key={log.id} log={log} />) : <p className="settings-empty">暂无运行日志。</p>}
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
    <section className="settings-section" aria-labelledby="audit-settings-title">
      <div className="settings-section-title settings-content-title">
        <div>
          <p className="section-label">Diagnostics</p>
          <h3 id="audit-settings-title">请求审计</h3>
          <p>查看最近模型请求、本地规则回退和工具边界摘要。</p>
        </div>
        <button className="ghost-button" type="button" onClick={onRefreshAuditLogs} disabled={isBusy}>
          <RotateCw size={14} />
          刷新
        </button>
      </div>
      <div className="audit-list">
        {auditLogs.length ? auditLogs.map((log) => <AuditLogCard key={log.id} log={log} />) : <p className="settings-empty">暂无审计记录。</p>}
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
    <div className="scan-report">
      <span>
        扫描 {report.scannedFileCount} 篇，失败 {report.failedFileCount} 个
      </span>
      {report.skippedDirectories.length > 0 && <span>跳过：{report.skippedDirectories.slice(0, 4).join(" / ")}</span>}
      {report.errors.length > 0 && <span className="scan-report-error">{report.errors[0]}</span>}
    </div>
  );
}

/** 单条审计日志卡片，展示请求类型、scope 摘要和工具调用摘要。 */
function AuditLogCard({ log }: { log: RequestAuditLog }) {
  return (
    <article className="audit-card">
      <div className="audit-card-header">
        <OverflowTooltipText as="strong" text={formatAuditKind(log.kind)} logArea="settings_audit_kind" />
        <OverflowTooltipText text={log.createdAt} logArea="settings_audit_created_at" />
      </div>
      <p>{log.scopeSummary}</p>
      <p>{log.contentSummary}</p>
      <OverflowTooltipText as="code" text={log.toolSummary} logArea="settings_audit_tool_summary" />
    </article>
  );
}

/** 单条应用事件日志卡片，展示运行级别、分类、状态和脱敏上下文。 */
function AppEventLogCard({ log }: { log: AppEventLog }) {
  return (
    <article className={`audit-card event-log-card ${log.level}`}>
      <div className="audit-card-header">
        <OverflowTooltipText
          as="strong"
          text={`${formatEventLogLevel(log.level)} · ${formatEventLogCategory(log.category)}`}
          logArea="settings_event_log_kind"
        />
        <OverflowTooltipText text={log.createdAt} logArea="settings_event_log_created_at" />
      </div>
      <OverflowTooltipText as="p" text={`${formatEventStatus(log.status)} / ${log.event}`} logArea="settings_event_log_status" />
      <p>{log.message}</p>
      <OverflowTooltipText as="code" text={formatEventLogContext(log)} logArea="settings_event_log_context" />
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
    <section className="settings-section" aria-labelledby="agent-memory-settings-title">
      <div className="settings-section-title settings-content-title">
        <div>
          <p className="section-label">Configuration</p>
          <h3 id="agent-memory-settings-title">Agent 记忆</h3>
          <p>管理每个知识库的跨会话长期偏好，默认关闭，开启后注入 Agent 上下文。</p>
        </div>
        <div className="settings-title-actions agent-memory-title-actions">
          <button
            className="primary-button compact"
            type="button"
            onClick={handleSave}
            disabled={isBusy || !activeKnowledgeBaseId}
          >
            <Save size={14} />
            保存记忆
          </button>
          <button
            className="text-button danger agent-memory-delete-button"
            type="button"
            onClick={handleRequestDelete}
            disabled={isBusy || !activeKnowledgeBaseId || !hasPersistedMemory}
          >
            <Trash2 size={14} />
            删除记忆
          </button>
        </div>
      </div>
      <p className="agent-memory-hint">
        适合保存：笔记结构、标签规范、整理习惯、已确认的知识库约定。请勿填写 API key、手机号、身份证、密码或私密正文片段——保存时会自动做敏感信息脱敏。
      </p>
      {knowledgeBases.length === 0 ? (
        <p className="settings-empty">暂无已授权知识库，请先在“知识库管理”中添加。</p>
      ) : (
        <div className="agent-memory-layout">
          <div className="agent-memory-kb-list" aria-label="知识库记忆列表">
            {knowledgeBases.map((knowledgeBase) => {
              const memory = knowledgeBaseMemories.find((item) => item.knowledgeBaseId === knowledgeBase.id);
              const entryCount = memory?.entries.length ?? 0;
              const summary = memory?.enabled ? `${entryCount} 条 · 已启用` : entryCount ? `${entryCount} 条 · 未启用` : "未配置";
              const isActive = knowledgeBase.id === activeKnowledgeBaseId;

              return (
                <button
                  className={`agent-memory-kb-card${isActive ? " active" : ""}`}
                  key={knowledgeBase.id}
                  type="button"
                  onClick={() => handleSelectKnowledgeBase(knowledgeBase.id)}
                  disabled={isBusy}
                  aria-pressed={isActive}
                >
                  <div className="agent-memory-kb-card-header">
                    <OverflowTooltipText as="strong" text={knowledgeBase.name} logArea="settings_memory_kb_name" />
                    <span className={memory?.enabled ? "agent-memory-status enabled" : "agent-memory-status"}>
                      {memory?.enabled ? "启用" : "关闭"}
                    </span>
                  </div>
                  <span>{summary}</span>
                </button>
              );
            })}
          </div>
          {activeKnowledgeBaseId && (
            <article className="agent-memory-editor">
              <header className="agent-memory-editor-header">
                <div className="agent-memory-editor-title">
                  <strong>{activeKnowledgeBase?.name ?? "知识库"}</strong>
                  <span className={memoryDraft.enabled ? "agent-memory-status enabled" : "agent-memory-status"}>
                    {activeMemorySummary}
                  </span>
                </div>
                <label className="toggle-row compact agent-memory-toggle">
                  <input
                    className="control-checkbox-input"
                    checked={memoryDraft.enabled}
                    onChange={(event) => handleEnabledChange(event.target.checked)}
                    type="checkbox"
                    disabled={isBusy}
                  />
                  <span className="control-checkbox" aria-hidden="true" />
                  <span>注入 Agent 上下文</span>
                </label>
              </header>
              <div className="agent-memory-entry-list">
                {memoryDraft.entries.length === 0 ? (
                  <p className="settings-empty compact">尚未添加记忆条目。</p>
                ) : (
                  memoryDraft.entries.map((entry) => (
                    <article className="agent-memory-entry" key={entry.id}>
                      <div className="agent-memory-entry-meta">
                        <label>
                          <span className="section-label">分类</span>
                          <span className="select-control">
                            <select
                              value={entry.category}
                              onChange={(event) => handleEntryChange(entry.id, "category", event.target.value)}
                              disabled={isBusy}
                            >
                              {MEMORY_CATEGORY_OPTIONS.map((option) => (
                                <option key={option.value} value={option.value}>
                                  {option.label}
                                </option>
                              ))}
                            </select>
                          </span>
                        </label>
                        <span className="agent-memory-source">{entry.source === "auto" ? "自动生成" : "用户录入"}</span>
                        <button
                          className="icon-button danger agent-memory-entry-delete"
                          type="button"
                          onClick={() => handleRemoveEntry(entry.id)}
                          disabled={isBusy}
                          title="删除条目"
                          aria-label="删除记忆条目"
                        >
                          <Trash2 size={14} />
                        </button>
                      </div>
                      <textarea
                        className="agent-memory-textarea"
                        value={entry.content}
                        onChange={(event) => handleEntryChange(entry.id, "content", event.target.value)}
                        placeholder={`例如：${memoryCategoryLabel(entry.category)}的约定`}
                        rows={3}
                        disabled={isBusy}
                      />
                    </article>
                  ))
                )}
                <div className="agent-memory-entry-actions">
                  <button className="ghost-button compact" type="button" onClick={handleAddEntry} disabled={isBusy}>
                    <Plus size={14} />
                    新增条目
                  </button>
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
