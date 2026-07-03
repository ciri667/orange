import {
  BookOpen,
  Check,
  FolderOpen,
  History,
  KeyRound,
  MessageCircle,
  Plus,
  RotateCw,
  Save,
  ScrollText,
  Settings2,
  ShieldCheck,
  Sparkles,
  Star,
  Trash2,
  X,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { ConfirmDialog } from "../shared/ConfirmDialog";
import { createLocalId, formatLocalDateTime } from "../shared/id";
import { logDebug, logError, logInfo } from "../shared/logger";
import type {
  AgentSkill,
  AppEventLog,
  AppEventLogCategory,
  AppEventLogLevel,
  InstallAgentSkillPayload,
  InstallAgentSkillResult,
  FeishuCredentialStatus,
  FeishuGatewayStatus,
  ImIntegrationSettings,
  KnowledgeBase,
  LlmProviderConfig,
  ModelApiKeyStatus,
  ProviderTemplate,
  RequestAuditLog,
  UserSettings,
} from "../shared/types";
import { SkillsModal } from "./SkillsModal";

/** 设置页左侧导航的可选分区，和右侧主内容一一对应。 */
type SettingsSectionId = "knowledge" | "model" | "im" | "skills" | "eventLogs" | "auditLogs";

/** 设置页导航分组，帮助用户区分可配置项和只读诊断项。 */
type SettingsSectionGroup = "配置" | "诊断";

/** 设置页导航项展示模型，meta 只放脱敏状态或轻量计数。 */
interface SettingsSectionNavItem {
  id: SettingsSectionId;
  group: SettingsSectionGroup;
  label: string;
  description: string;
  meta: string;
  icon: LucideIcon;
  tone?: "neutral" | "success" | "warning";
}

/** 左侧导航分组顺序，保证配置项始终排在诊断项之前。 */
const SETTINGS_SECTION_GROUPS: SettingsSectionGroup[] = ["配置", "诊断"];

/** 设置抽屉，展示多知识库、模型策略、Skills 和诊断信息。 */
export function SettingsDrawer({
  knowledgeBases,
  activeKnowledgeBaseId,
  settings,
  imSettings,
  skills,
  modelApiKeyStatuses,
  feishuCredentialStatus,
  feishuGatewayStatus,
  providerTemplates,
  auditLogs,
  appEventLogs,
  isBusy,
  onSelectKnowledgeBase,
  onAddKnowledgeBase,
  onRescanKnowledgeBase,
  onRemoveKnowledgeBase,
  onSaveSettings,
  onSaveImSettings,
  onSaveSkill,
  onInstallSkill,
  onToggleSkill,
  onDeleteSkill,
  onOpenUserSkillsFolder,
  onSaveApiKey,
  onSaveFeishuSecret,
  onStartFeishuGateway,
  onStopFeishuGateway,
  onRefreshFeishuStatus,
  onRefreshAuditLogs,
  onRefreshAppEventLogs,
  onClearAppEventLogs,
  onOpenAppLogFolder,
  onClose,
}: {
  knowledgeBases: KnowledgeBase[];
  activeKnowledgeBaseId: string;
  settings: UserSettings;
  imSettings: ImIntegrationSettings;
  skills: AgentSkill[];
  modelApiKeyStatuses: ModelApiKeyStatus[];
  feishuCredentialStatus: FeishuCredentialStatus | null;
  feishuGatewayStatus: FeishuGatewayStatus | null;
  providerTemplates: ProviderTemplate[];
  auditLogs: RequestAuditLog[];
  appEventLogs: AppEventLog[];
  isBusy: boolean;
  onSelectKnowledgeBase: (knowledgeBaseId: string) => void;
  onAddKnowledgeBase: () => void;
  onRescanKnowledgeBase: (knowledgeBaseId: string) => void;
  onRemoveKnowledgeBase: (knowledgeBaseId: string) => void;
  onSaveSettings: (settings: UserSettings) => Promise<void> | void;
  onSaveImSettings: (settings: ImIntegrationSettings) => Promise<void> | void;
  onSaveSkill: (skill: AgentSkill) => Promise<AgentSkill | void> | AgentSkill | void;
  onInstallSkill: (payload: InstallAgentSkillPayload) => Promise<InstallAgentSkillResult> | InstallAgentSkillResult;
  onToggleSkill: (skillId: string, enabled: boolean) => Promise<void> | void;
  onDeleteSkill: (skillId: string) => Promise<void> | void;
  onOpenUserSkillsFolder: () => Promise<void> | void;
  onSaveApiKey: (providerId: string, apiKey: string) => Promise<void> | void;
  onSaveFeishuSecret: (appSecret: string) => Promise<void> | void;
  onStartFeishuGateway: () => Promise<void> | void;
  onStopFeishuGateway: () => Promise<void> | void;
  onRefreshFeishuStatus: () => Promise<void> | void;
  onRefreshAuditLogs: () => Promise<void> | void;
  onRefreshAppEventLogs: (filters?: { level?: AppEventLogLevel | ""; category?: AppEventLogCategory | "" }) => Promise<void> | void;
  onClearAppEventLogs: (filters?: { level?: AppEventLogLevel | ""; category?: AppEventLogCategory | "" }) => Promise<void> | void;
  onOpenAppLogFolder: () => Promise<void> | void;
  onClose: () => void;
}) {
  /** 模型设置表单草稿，用户保存前不影响正在运行的 Agent Runtime。 */
  const [settingsDraft, setSettingsDraft] = useState<UserSettings>(settings);
  /** 即时通讯设置草稿，保存前不影响正在运行的飞书网关。 */
  const [imSettingsDraft, setImSettingsDraft] = useState<ImIntegrationSettings>(imSettings);
  /** 飞书 appSecret 草稿只存在输入框中，保存后立即清空。 */
  const [feishuSecretDraft, setFeishuSecretDraft] = useState("");
  /** 每个 provider 的 API key 草稿只保留在输入框中，保存后由外层写入系统安全存储。 */
  const [apiKeyDraftByProvider, setApiKeyDraftByProvider] = useState<Record<string, string>>({});
  /** “新增 Provider”入口当前选中的内置模板 ID。 */
  const [selectedTemplateId, setSelectedTemplateId] = useState(providerTemplates[0]?.templateId ?? "");
  /** 待确认移除的 provider ID；非默认 provider 删除前需要用户二次确认。 */
  const [providerPendingRemoval, setProviderPendingRemoval] = useState<string | null>(null);
  /** Skills 管理弹窗状态，避免设置抽屉内一次性铺开完整管理页。 */
  const [isSkillsModalOpen, setIsSkillsModalOpen] = useState(false);
  /** 当前设置分区，驱动左侧导航高亮和右侧单页内容渲染。 */
  const [activeSection, setActiveSection] = useState<SettingsSectionId>("knowledge");
  /** 应用事件日志级别筛选，空字符串表示不过滤。 */
  const [eventLogLevel, setEventLogLevel] = useState<AppEventLogLevel | "">("");
  /** 应用事件日志分类筛选，空字符串表示不过滤。 */
  const [eventLogCategory, setEventLogCategory] = useState<AppEventLogCategory | "">("");
  /** 默认知识库自动预选只执行一次，避免用户手动取消后又被 effect 选回。 */
  const hasSeededDefaultImKnowledgeBaseRef = useRef(false);
  /** 已启用 skill 数量，用于设置摘要快速说明能力状态。 */
  const enabledSkillCount = skills.filter((skill) => skill.enabled).length;
  /** 自定义 skill 数量用于确认用户目录扫描是否已生效。 */
  const customSkillCount = skills.filter((skill) => skill.source === "custom").length;
  /** 设置工作台顶部摘要只展示计数和状态，避免路径、密钥和请求内容外露。 */
  const settingsSummary = {
    knowledgeBaseCount: knowledgeBases.length,
    providerCount: settingsDraft.modelConfig.providers.length,
    feishuStatus: feishuGatewayStatus?.running ? "运行中" : imSettingsDraft.feishu.enabled ? "已配置" : "未启用",
    enabledSkillCount,
    errorLogCount: appEventLogs.filter((log) => log.level === "error").length,
  };
  /** 左侧导航项只汇总脱敏状态和轻量计数，避免路径、密钥或请求内容进入 UI 状态元数据。 */
  const settingsNavItems = useMemo<SettingsSectionNavItem[]>(
    () => [
      {
        id: "knowledge",
        group: "配置",
        label: "知识库管理",
        description: "目录授权、激活和重新扫描",
        meta: `${knowledgeBases.length} 个`,
        icon: BookOpen,
        tone: knowledgeBases.some((knowledgeBase) => knowledgeBase.status === "error") ? "warning" : "neutral",
      },
      {
        id: "model",
        group: "配置",
        label: "模型与隐私",
        description: "BYOK、模型端点和发送边界",
        meta: settingsDraft.modelConfig.enabled ? "已启用" : "未启用",
        icon: Settings2,
        tone: settingsDraft.modelConfig.enabled ? "success" : "neutral",
      },
      {
        id: "im",
        group: "配置",
        label: "即时通讯",
        description: "飞书/Lark 长连接和白名单",
        meta: settingsSummary.feishuStatus,
        icon: MessageCircle,
        tone: feishuGatewayStatus?.running ? "success" : imSettingsDraft.feishu.enabled ? "warning" : "neutral",
      },
      {
        id: "skills",
        group: "配置",
        label: "Skills 能力",
        description: "启用状态和能力管理",
        meta: `${enabledSkillCount}/${skills.length}`,
        icon: Sparkles,
        tone: enabledSkillCount > 0 ? "success" : "neutral",
      },
      {
        id: "eventLogs",
        group: "诊断",
        label: "运行日志",
        description: "应用事件、级别和分类筛选",
        meta: `${appEventLogs.length} 条`,
        icon: History,
        tone: appEventLogs.some((log) => log.level === "error") ? "warning" : "neutral",
      },
      {
        id: "auditLogs",
        group: "诊断",
        label: "请求审计",
        description: "模型请求和工具边界",
        meta: `${auditLogs.length} 条`,
        icon: ScrollText,
        tone: "neutral",
      },
    ],
    [
      appEventLogs,
      auditLogs.length,
      enabledSkillCount,
      feishuGatewayStatus?.running,
      imSettingsDraft.feishu.enabled,
      settingsSummary.feishuStatus,
      knowledgeBases,
      settingsDraft.modelConfig.enabled,
      skills.length,
    ],
  );

  useEffect(() => {
    setSettingsDraft(settings);
  }, [settings]);

  useEffect(() => {
    setImSettingsDraft(imSettings);
  }, [imSettings]);

  useEffect(() => {
    const fallbackKnowledgeBaseId = activeKnowledgeBaseId || knowledgeBases[0]?.id;

    if (
      hasSeededDefaultImKnowledgeBaseRef.current ||
      !fallbackKnowledgeBaseId ||
      imSettingsDraft.feishu.defaultKnowledgeBaseIds.length > 0
    ) {
      return;
    }

    hasSeededDefaultImKnowledgeBaseRef.current = true;

    // 默认范围只写入设置页草稿；仍由用户保存或启动时显式持久化，避免打开设置页就改配置。
    setImSettingsDraft((currentSettings) => ({
      ...currentSettings,
      feishu: {
        ...currentSettings.feishu,
        defaultKnowledgeBaseIds: [fallbackKnowledgeBaseId],
      },
    }));
  }, [activeKnowledgeBaseId, imSettingsDraft.feishu.defaultKnowledgeBaseIds.length, knowledgeBases]);

  useEffect(() => {
    if (providerTemplates.length && !providerTemplates.some((template) => template.templateId === selectedTemplateId)) {
      setSelectedTemplateId(providerTemplates[0].templateId);
    }
  }, [providerTemplates, selectedTemplateId]);

  /** 保存模型、隐私和 skill 设置；日志只记录状态和耗时，不记录端点、模型名或密钥。 */
  async function handleSaveSettings() {
    const startedAt = performance.now();
    const nextSettings = {
      ...settingsDraft,
      writeConfirmationRequired: true,
    };

    logInfo("设置页保存用户设置。", {
      category: "settings",
      event: "settings_save",
      status: "started",
      metadata: {
        modelEnabled: nextSettings.modelConfig.enabled,
        providerCount: nextSettings.modelConfig.providers.length,
        defaultProviderId: nextSettings.modelConfig.defaultProviderId,
        privacyPolicy: nextSettings.privacyPolicy,
        enabledSkillCount,
      },
    });

    try {
      await onSaveSettings(nextSettings);
      logInfo("设置页用户设置保存完成。", {
        category: "settings",
        event: "settings_save",
        status: "completed",
        durationMs: performance.now() - startedAt,
      });
    } catch (error) {
      logError("设置页用户设置保存失败。", {
        category: "settings",
        event: "settings_save",
        status: "failed",
        durationMs: performance.now() - startedAt,
        error,
      });
      throw error;
    }
  }

  /** 生成可持久化的即时通讯设置，统一裁剪空白和去重，避免保存/启动路径出现不一致。 */
  function buildNormalizedImSettings(): ImIntegrationSettings {
    const allowedUserOpenIds = uniqueTrimmedList(imSettingsDraft.feishu.allowedUserOpenIds);
    const allowedChatIds = uniqueTrimmedList(imSettingsDraft.feishu.allowedChatIds);

    return {
      ...imSettingsDraft,
      feishu: {
        ...imSettingsDraft.feishu,
        appId: imSettingsDraft.feishu.appId.trim(),
        defaultKnowledgeBaseIds: uniqueTrimmedList(imSettingsDraft.feishu.defaultKnowledgeBaseIds),
        allowedUserOpenIds,
        allowedChatIds,
        discoveredUserOpenIds: uniqueTrimmedList(imSettingsDraft.feishu.discoveredUserOpenIds).filter(
          (openId) => !allowedUserOpenIds.includes(openId),
        ),
        discoveredChatIds: uniqueTrimmedList(imSettingsDraft.feishu.discoveredChatIds).filter(
          (chatId) => !allowedChatIds.includes(chatId),
        ),
        updatedAt: formatLocalDateTime(),
      },
    };
  }

  /** 保存即时通讯设置；日志只记录计数和状态，不记录 open_id/chat_id 原文。 */
  async function handleSaveImSettings() {
    const startedAt = performance.now();
    const nextSettings = buildNormalizedImSettings();

    logInfo("设置页保存即时通讯设置。", {
      category: "im",
      event: "im_settings_save",
      status: "started",
      metadata: {
        feishuEnabled: nextSettings.feishu.enabled,
        domain: nextSettings.feishu.domain,
        knowledgeBaseCount: nextSettings.feishu.defaultKnowledgeBaseIds.length,
        allowedUserCount: nextSettings.feishu.allowedUserOpenIds.length,
        allowedChatCount: nextSettings.feishu.allowedChatIds.length,
      },
    });

    try {
      await onSaveImSettings(nextSettings);
      setImSettingsDraft(nextSettings);
      logInfo("设置页即时通讯设置保存完成。", {
        category: "im",
        event: "im_settings_save",
        status: "completed",
        durationMs: performance.now() - startedAt,
      });
    } catch (error) {
      logError("设置页即时通讯设置保存失败。", {
        category: "im",
        event: "im_settings_save",
        status: "failed",
        durationMs: performance.now() - startedAt,
        error,
      });
      throw error;
    }
  }

  /** 启动飞书网关前先保存当前草稿，确保后端读取到最新知识库范围和白名单。 */
  async function handleStartFeishuGateway() {
    const startedAt = performance.now();

    try {
      await handleSaveImSettings();
      await onStartFeishuGateway();
      logInfo("设置页启动飞书网关完成。", {
        category: "im",
        event: "feishu_gateway_start_from_settings",
        status: "completed",
        durationMs: performance.now() - startedAt,
      });
    } catch (error) {
      logError("设置页启动飞书网关失败。", {
        category: "im",
        event: "feishu_gateway_start_from_settings",
        status: "failed",
        durationMs: performance.now() - startedAt,
        error,
      });
    }
  }

  /** 保存飞书 appSecret 后立即清空输入框，避免敏感信息留在 React state。 */
  async function handleSaveFeishuSecret() {
    const appSecret = feishuSecretDraft.trim();

    if (!appSecret) {
      return;
    }

    try {
      await onSaveFeishuSecret(appSecret);
      setFeishuSecretDraft("");
    } catch {
      // 外层 notice 和前端日志已经说明失败；保留输入方便用户修正重试。
    }
  }

  /** 更新飞书设置草稿中的单个字段。 */
  function updateFeishuDraft<K extends keyof ImIntegrationSettings["feishu"]>(
    field: K,
    value: ImIntegrationSettings["feishu"][K],
  ) {
    setImSettingsDraft((currentSettings) => ({
      ...currentSettings,
      feishu: {
        ...currentSettings.feishu,
        [field]: value,
      },
    }));
  }

  /** 将 textarea 的多行 ID 转成去重数组；不记录原始 ID 内容。 */
  function parseMultilineIds(value: string) {
    return uniqueTrimmedList(value.split(/\r?\n|,/));
  }

  /** 将后端自动发现的飞书用户加入 allowlist，同时从候选列表移除。 */
  function allowDiscoveredFeishuUser(openId: string) {
    const allowedUserOpenIds = uniqueTrimmedList([...imSettingsDraft.feishu.allowedUserOpenIds, openId]);

    setImSettingsDraft((currentSettings) => ({
      ...currentSettings,
      feishu: {
        ...currentSettings.feishu,
        allowedUserOpenIds,
        discoveredUserOpenIds: currentSettings.feishu.discoveredUserOpenIds.filter((candidate) => candidate !== openId),
      },
    }));
  }

  /** 将后端自动发现的飞书群加入 allowlist，同时从候选列表移除。 */
  function allowDiscoveredFeishuChat(chatId: string) {
    const allowedChatIds = uniqueTrimmedList([...imSettingsDraft.feishu.allowedChatIds, chatId]);

    setImSettingsDraft((currentSettings) => ({
      ...currentSettings,
      feishu: {
        ...currentSettings.feishu,
        allowedChatIds,
        discoveredChatIds: currentSettings.feishu.discoveredChatIds.filter((candidate) => candidate !== chatId),
      },
    }));
  }

  /** 保存指定 provider 的 BYOK key 后清空对应输入框，日志只记录是否提交了输入，不记录密钥内容。 */
  async function handleSaveApiKey(providerId: string) {
    const startedAt = performance.now();
    const apiKeyDraft = apiKeyDraftByProvider[providerId] ?? "";

    logInfo("设置页保存模型密钥。", {
      category: "settings",
      event: "model_api_key_save",
      status: "started",
      metadata: {
        providerId,
        hasInput: Boolean(apiKeyDraft.trim()),
      },
    });

    try {
      await onSaveApiKey(providerId, apiKeyDraft);
      setApiKeyDraftByProvider((current) => ({ ...current, [providerId]: "" }));
      logInfo("设置页模型密钥保存完成。", {
        category: "settings",
        event: "model_api_key_save",
        status: "completed",
        durationMs: performance.now() - startedAt,
        metadata: { providerId },
      });
    } catch (error) {
      // 外层已经把失败原因写入全局 notice；这里保留输入，方便用户修正后重试。
      logError("设置页模型密钥保存失败。", {
        category: "settings",
        event: "model_api_key_save",
        status: "failed",
        durationMs: performance.now() - startedAt,
        metadata: { providerId },
        error,
      });
    }
  }

  /** 更新模型配置草稿中的全局字段（启用开关等），保持 provider 列表不变。 */
  function updateModelConfig(field: "enabled", value: boolean) {
    setSettingsDraft((currentSettings) => ({
      ...currentSettings,
      modelConfig: {
        ...currentSettings.modelConfig,
        [field]: value,
      },
    }));
  }

  /** 更新草稿中单个 provider 的字段，其余 provider 保持不变。 */
  function updateProviderField(providerId: string, field: keyof LlmProviderConfig, value: string | boolean) {
    setSettingsDraft((currentSettings) => ({
      ...currentSettings,
      modelConfig: {
        ...currentSettings.modelConfig,
        providers: currentSettings.modelConfig.providers.map((provider) =>
          provider.id === providerId
            ? {
                ...provider,
                [field]: value,
                // 默认 Provider 必须保持启用，否则全局默认解析会固定落到停用项并阻断 Agent。
                enabled:
                  field === "enabled" && provider.id === currentSettings.modelConfig.defaultProviderId
                    ? true
                    : field === "enabled"
                      ? Boolean(value)
                      : provider.enabled,
                updatedAt: formatLocalDateTime(),
              }
            : provider,
        ),
      },
    }));
  }

  /** 将草稿中某个 provider 设为默认 provider，未显式选择模型的请求会回退到它。 */
  function setDefaultProvider(providerId: string) {
    setSettingsDraft((currentSettings) => ({
      ...currentSettings,
      modelConfig: {
        ...currentSettings.modelConfig,
        defaultProviderId: providerId,
        providers: currentSettings.modelConfig.providers.map((provider) =>
          provider.id === providerId
            ? { ...provider, enabled: true, updatedAt: formatLocalDateTime() }
            : provider,
        ),
      },
    }));
  }

  /** 依据内置模板在草稿中新增一个 provider 实例，新增后立即可编辑名称、端点和模型。 */
  function addProviderFromTemplate(templateId: string) {
    const template = providerTemplates.find((item) => item.templateId === templateId);

    if (!template) {
      return;
    }

    const now = formatLocalDateTime();
    const providerId = createLocalId("provider");
    const newProvider: LlmProviderConfig = {
      id: providerId,
      name: template.name,
      provider: template.provider,
      apiBase: template.apiBase,
      model: template.model,
      // 后端保存设置时会强制按 providerId 重新计算 key_reference（见 model_provider::normalize_model_config_key_references），
      // 这里保持同样的派生格式只是为了让草稿状态在保存前也保持一致，不作为最终依据。
      keyReference: `cici-note-llm-provider-${providerId}-api-key`,
      enabled: true,
      supportsTools: true,
      requiresApiKey: template.requiresApiKey,
      createdAt: now,
      updatedAt: now,
    };

    setSettingsDraft((currentSettings) => {
      const providers = [...currentSettings.modelConfig.providers, newProvider];
      const hasDefault = providers.some((provider) => provider.id === currentSettings.modelConfig.defaultProviderId);

      return {
        ...currentSettings,
        modelConfig: {
          ...currentSettings.modelConfig,
          providers,
          defaultProviderId: hasDefault ? currentSettings.modelConfig.defaultProviderId : newProvider.id,
        },
      };
    });

    logInfo("设置页新增 Provider。", {
      category: "settings",
      event: "provider_add",
      status: "completed",
      metadata: { templateId },
    });
  }

  /** 默认 provider 不允许直接删除，需要先把其他 provider 设为默认。 */
  function requestRemoveProvider(providerId: string) {
    if (providerId === settingsDraft.modelConfig.defaultProviderId) {
      setProviderPendingRemoval(null);
      return;
    }

    setProviderPendingRemoval(providerId);
  }

  /** 用户在确认弹窗中确认后才真正从草稿中移除该 provider。 */
  function confirmRemoveProvider() {
    const providerId = providerPendingRemoval;

    if (!providerId) {
      return;
    }

    setSettingsDraft((currentSettings) => ({
      ...currentSettings,
      modelConfig: {
        ...currentSettings.modelConfig,
        providers: currentSettings.modelConfig.providers.filter((provider) => provider.id !== providerId),
      },
    }));

    logInfo("设置页移除 Provider。", {
      category: "settings",
      event: "provider_remove",
      status: "completed",
      metadata: { providerId },
    });
    setProviderPendingRemoval(null);
  }

  /** 切换设置分区并写入低频调试日志，便于诊断导航状态但不影响渲染性能。 */
  function handleActiveSectionChange(sectionId: SettingsSectionId) {
    if (sectionId === activeSection) {
      return;
    }

    logDebug("切换设置页分区。", {
      category: "settings",
      event: "settings_section_change",
      status: "completed",
      metadata: {
        from: activeSection,
        to: sectionId,
      },
    });
    setActiveSection(sectionId);
  }

  /** 返回当前运行日志筛选条件，供刷新和清空后重载列表复用。 */
  function currentEventLogFilters() {
    return {
      level: eventLogLevel,
      category: eventLogCategory,
    };
  }

  /** 切换运行日志级别筛选，并立即向后端请求对应列表。 */
  function handleEventLogLevelChange(level: AppEventLogLevel | "") {
    setEventLogLevel(level);
    void onRefreshAppEventLogs({ level, category: eventLogCategory });
  }

  /** 切换运行日志分类筛选，并立即向后端请求对应列表。 */
  function handleEventLogCategoryChange(category: AppEventLogCategory | "") {
    setEventLogCategory(category);
    void onRefreshAppEventLogs({ level: eventLogLevel, category });
  }

  /** 渲染设置导航分组；按配置/诊断拆开，避免所有入口混在同一列表里。 */
  function renderNavigationGroup(group: SettingsSectionGroup) {
    const groupItems = settingsNavItems.filter((item) => item.group === group);

    return (
      <div className="settings-nav-group" key={group}>
        <p className="settings-nav-group-label">{group}</p>
        <div className="settings-nav-items">
          {groupItems.map((item) => {
            const SectionIcon = item.icon;

            return (
              <button
                className={`settings-nav-item ${activeSection === item.id ? "active" : ""}`}
                key={item.id}
                type="button"
                aria-current={activeSection === item.id ? "page" : undefined}
                onClick={() => handleActiveSectionChange(item.id)}
              >
                <SectionIcon size={17} />
                <span className="settings-nav-text">
                  <strong>{item.label}</strong>
                  <small>{item.description}</small>
                </span>
                <em className={`settings-nav-meta ${item.tone ?? "neutral"}`}>{item.meta}</em>
              </button>
            );
          })}
        </div>
      </div>
    );
  }

  /** 根据左侧选中项只渲染一个主配置区，避免设置面板出现长串卡片和双重滚动。 */
  function renderActiveSection() {
    if (activeSection === "knowledge") {
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
                      <strong>{knowledgeBase.name}</strong>
                      <span>{knowledgeBase.status === "error" ? "目录失效" : knowledgeBase.semanticIndexEnabled ? "本地向量" : "FTS5"}</span>
                      {knowledgeBase.id === activeKnowledgeBaseId && <span>当前激活</span>}
                    </div>
                    <p>{knowledgeBase.description}</p>
                    <code>{knowledgeBase.path}</code>
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

    if (activeSection === "model") {
      const providers = settingsDraft.modelConfig.providers;

      return (
        <section className="settings-section" aria-labelledby="model-settings-title">
          <div className="settings-section-title settings-content-title">
            <div>
              <p className="section-label">Configuration</p>
              <h3 id="model-settings-title">模型与隐私</h3>
              <p>管理多个 OpenAI-compatible Provider，选择默认 Provider 和发送边界。</p>
            </div>
            <button className="primary-button compact" type="button" onClick={handleSaveSettings} disabled={isBusy}>
              <Save size={14} />
              保存设置
            </button>
          </div>
          <div className="settings-grid">
            <label className="toggle-row">
              <input
                className="control-checkbox-input"
                checked={settingsDraft.modelConfig.enabled}
                onChange={(event) => updateModelConfig("enabled", event.target.checked)}
                type="checkbox"
              />
              <span className="control-checkbox" aria-hidden="true" />
              <span>启用云端模型（关闭后 Agent 只使用本地规则回复）</span>
            </label>
            <label>
              <span>隐私策略</span>
              <span className="select-control">
                <select
                  value={settingsDraft.privacyPolicy}
                  onChange={(event) =>
                    setSettingsDraft((currentSettings) => ({
                      ...currentSettings,
                      privacyPolicy: event.target.value as UserSettings["privacyPolicy"],
                    }))
                  }
                >
                  <option value="allow-selected-scope">允许已选 scope</option>
                  <option value="local-only">仅本地规则 Agent</option>
                </select>
              </span>
            </label>
          </div>

          <div className="provider-add-row">
            <span className="select-control">
              <select value={selectedTemplateId} onChange={(event) => setSelectedTemplateId(event.target.value)}>
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
              onClick={() => addProviderFromTemplate(selectedTemplateId)}
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

                return (
                  <article className="provider-card" key={provider.id}>
                    <div className="provider-card-header">
                      <input
                        className="provider-name-input"
                        value={provider.name}
                        onChange={(event) => updateProviderField(provider.id, "name", event.target.value)}
                        placeholder="Provider 名称"
                      />
                      <div className="provider-card-badges">
                        {isDefault ? (
                          <span className="provider-badge default">
                            <Star size={12} />
                            默认
                          </span>
                        ) : (
                          <button className="ghost-button compact" type="button" onClick={() => setDefaultProvider(provider.id)}>
                            设为默认
                          </button>
                        )}
                        <label className="toggle-row compact">
                          <input
                            className="control-checkbox-input"
                            checked={provider.enabled}
                            onChange={(event) => updateProviderField(provider.id, "enabled", event.target.checked)}
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
                          onClick={() => requestRemoveProvider(provider.id)}
                          disabled={isDefault || providers.length <= 1}
                        >
                          <Trash2 size={14} />
                        </button>
                      </div>
                    </div>
                    <div className="provider-card-grid">
                      <label>
                        <span>API base</span>
                        <input
                          value={provider.apiBase}
                          onChange={(event) => updateProviderField(provider.id, "apiBase", event.target.value)}
                          placeholder="https://api.openai.com/v1"
                        />
                      </label>
                      <label>
                        <span>模型</span>
                        <input
                          value={provider.model}
                          onChange={(event) => updateProviderField(provider.id, "model", event.target.value)}
                          placeholder="gpt-4o-mini"
                        />
                      </label>
                      <label className="toggle-row compact">
                        <input
                          className="control-checkbox-input"
                          checked={provider.supportsTools}
                          onChange={(event) => updateProviderField(provider.id, "supportsTools", event.target.checked)}
                          type="checkbox"
                        />
                        <span className="control-checkbox" aria-hidden="true" />
                        <span>支持工具调用（Function Calling）</span>
                      </label>
                      <label className="toggle-row compact">
                        <input
                          className="control-checkbox-input"
                          checked={provider.requiresApiKey}
                          onChange={(event) => updateProviderField(provider.id, "requiresApiKey", event.target.checked)}
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
                              onChange={(event) =>
                                setApiKeyDraftByProvider((current) => ({ ...current, [provider.id]: event.target.value }))
                              }
                              placeholder="sk-..."
                              type="password"
                            />
                            <button
                              type="button"
                              onClick={() => handleSaveApiKey(provider.id)}
                              disabled={isBusy || !apiKeyDraft.trim()}
                            >
                              <KeyRound size={13} />
                              保存密钥
                            </button>
                          </div>
                          <div className={`key-status ${keyStatus?.configured ? "verified" : "missing"}`}>
                            <KeyRound size={13} />
                            <span>{keyStatus?.message ?? "尚未读取模型密钥状态。"}</span>
                          </div>
                        </label>
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

    if (activeSection === "skills") {
      return (
        <section className="settings-section" aria-labelledby="skills-settings-title">
          <div className="settings-section-title settings-content-title">
            <div>
              <p className="section-label">Configuration</p>
              <h3 id="skills-settings-title">Skills 能力</h3>
              <p>管理 Agent 可用能力和未显式选择时的匹配方式。</p>
            </div>
            <div className="settings-title-actions">
              <button className="ghost-button" type="button" onClick={() => setIsSkillsModalOpen(true)}>
                <Sparkles size={14} />
                管理 Skills
              </button>
              <button className="primary-button compact" type="button" onClick={handleSaveSettings} disabled={isBusy}>
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

    if (activeSection === "im") {
      const feishu = imSettingsDraft.feishu;
      const selectedKnowledgeBaseIds = new Set(feishu.defaultKnowledgeBaseIds);

      return (
        <section className="settings-section" aria-labelledby="im-settings-title">
          <div className="settings-section-title settings-content-title">
            <div>
              <p className="section-label">Configuration</p>
              <h3 id="im-settings-title">即时通讯</h3>
              <p>连接飞书/Lark 自建应用，允许白名单用户通过文本消息调用 Agent。</p>
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
                <button className="primary-button compact" type="button" onClick={handleStartFeishuGateway} disabled={isBusy}>
                  启动
                </button>
              )}
              <button className="primary-button compact" type="button" onClick={handleSaveImSettings} disabled={isBusy}>
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
                onChange={(event) => updateFeishuDraft("enabled", event.target.checked)}
                type="checkbox"
              />
              <span className="control-checkbox" aria-hidden="true" />
              <span>启用飞书/Lark 集成</span>
            </label>
            <label>
              <span>平台</span>
              <span className="select-control">
                <select value={feishu.domain} onChange={(event) => updateFeishuDraft("domain", event.target.value as "feishu" | "lark")}>
                  <option value="feishu">飞书</option>
                  <option value="lark">Lark</option>
                </select>
              </span>
            </label>
            <label>
              <span>App ID</span>
              <input value={feishu.appId} onChange={(event) => updateFeishuDraft("appId", event.target.value)} placeholder="cli_xxx" />
            </label>
            <label className="settings-full-row">
              <span>App Secret</span>
              <div className="inline-secret-row">
                <input
                  type="password"
                  value={feishuSecretDraft}
                  onChange={(event) => setFeishuSecretDraft(event.target.value)}
                  placeholder={feishuCredentialStatus?.configured ? "已保存，输入新值可替换" : "输入飞书 appSecret"}
                />
                <button className="ghost-button" type="button" onClick={handleSaveFeishuSecret} disabled={isBusy || !feishuSecretDraft.trim()}>
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
                onChange={(event) => updateFeishuDraft("requireMention", event.target.checked)}
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

                        updateFeishuDraft("defaultKnowledgeBaseIds", Array.from(nextIds));
                      }}
                    />
                    <span className="scope-check" aria-hidden="true">
                      {isSelected && <Check size={12} />}
                    </span>
                    <span className="scope-option-copy">
                      <strong>{knowledgeBase.name}</strong>
                      <span>{knowledgeBase.status === "error" ? "目录失效" : `${knowledgeBase.noteCount} 篇笔记`}</span>
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
                      <span>{formatIdentifierPreview(openId)}</span>
                    </span>
                    <button className="ghost-button compact" type="button" onClick={() => allowDiscoveredFeishuUser(openId)}>
                      允许用户
                    </button>
                  </div>
                ))}
                {feishu.discoveredChatIds.map((chatId, index) => (
                  <div className="discovered-peer-row" key={chatId}>
                    <span>
                      <strong>群候选 {index + 1}</strong>
                      <span>{formatIdentifierPreview(chatId)}</span>
                    </span>
                    <button className="ghost-button compact" type="button" onClick={() => allowDiscoveredFeishuChat(chatId)}>
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
                onChange={(event) => updateFeishuDraft("allowedUserOpenIds", parseMultilineIds(event.target.value))}
                rows={4}
                placeholder="ou_xxx，每行一个"
              />
            </label>
            <label className="settings-full-row">
              <span>允许群 chat_id</span>
              <textarea
                value={feishu.allowedChatIds.join("\n")}
                onChange={(event) => updateFeishuDraft("allowedChatIds", parseMultilineIds(event.target.value))}
                rows={4}
                placeholder="oc_xxx，每行一个；私聊可留空"
              />
            </label>
          </div>

          <div className="policy-row">
            <MessageCircle size={16} />
            <span>
              网关：{feishuGatewayStatus?.running ? "运行中" : "未运行"} / 连接：
              {feishuGatewayStatus?.connected ? "已收到事件" : "未确认"} / 平台：{feishuGatewayStatus?.domain ?? feishu.domain}
            </span>
          </div>
          {feishuGatewayStatus?.lastError ? <p className="settings-empty">{feishuGatewayStatus.lastError}</p> : null}
        </section>
      );
    }

    if (activeSection === "eventLogs") {
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
              <button className="ghost-button" type="button" onClick={() => onRefreshAppEventLogs(currentEventLogFilters())} disabled={isBusy}>
                <RotateCw size={14} />
                刷新
              </button>
              <button
                className="ghost-button danger-action"
                type="button"
                onClick={() => onClearAppEventLogs(currentEventLogFilters())}
                disabled={isBusy}
              >
                <Trash2 size={14} />
                清空
              </button>
            </div>
          </div>
          <div className="event-log-filters">
            <label>
              <span>级别</span>
              <span className="select-control">
                <select value={eventLogLevel} onChange={(event) => handleEventLogLevelChange(event.target.value as AppEventLogLevel | "")}>
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
                <select
                  value={eventLogCategory}
                  onChange={(event) => handleEventLogCategoryChange(event.target.value as AppEventLogCategory | "")}
                >
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
            {appEventLogs.length ? (
              appEventLogs.map((log) => <AppEventLogCard key={log.id} log={log} />)
            ) : (
              <p className="settings-empty">暂无运行日志。</p>
            )}
          </div>
        </section>
      );
    }

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

  return (
    <div className="settings-backdrop" role="presentation" onMouseDown={onClose}>
      <aside className="settings-drawer" aria-label="设置" onMouseDown={(event) => event.stopPropagation()}>
        <header className="settings-header">
          <div>
            <p className="section-label">Settings</p>
            <h2>设置工作台</h2>
          </div>
          <button className="icon-button" type="button" title="关闭设置" onClick={onClose}>
            <X size={18} />
          </button>
        </header>

        <div className="settings-workbench">
          <nav className="settings-sidebar" aria-label="设置项">
            <div className="settings-overview" aria-label="设置摘要">
              <strong>本地 Agent 环境</strong>
              <span>{settingsSummary.knowledgeBaseCount} 个资料库</span>
              <span>{settingsSummary.providerCount} 个模型 Provider</span>
              <span>{settingsSummary.enabledSkillCount} 个 Skill 启用</span>
              {settingsSummary.errorLogCount > 0 && <em>{settingsSummary.errorLogCount} 条错误日志</em>}
            </div>
            {SETTINGS_SECTION_GROUPS.map((group) => renderNavigationGroup(group))}
          </nav>
          <main className="settings-content" aria-label="设置主要内容">
            {renderActiveSection()}
          </main>
        </div>
        {isSkillsModalOpen && (
          <SkillsModal
            skills={skills}
            isBusy={isBusy}
            onSaveSkill={onSaveSkill}
            onInstallSkill={onInstallSkill}
            onToggleSkill={onToggleSkill}
            onDeleteSkill={onDeleteSkill}
            onOpenUserSkillsFolder={onOpenUserSkillsFolder}
            onClose={() => setIsSkillsModalOpen(false)}
          />
        )}
        {providerPendingRemoval && (
          <ConfirmDialog
            title="移除 Provider"
            message={`确认移除「${
              settingsDraft.modelConfig.providers.find((provider) => provider.id === providerPendingRemoval)?.name ?? "该 Provider"
            }」？移除后需要重新新增才能恢复配置，已保存的 API key 不会被读取。`}
            confirmLabel="移除"
            tone="danger"
            onCancel={() => setProviderPendingRemoval(null)}
            onConfirm={confirmRemoveProvider}
          />
        )}
      </aside>
    </div>
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
        <strong>{formatAuditKind(log.kind)}</strong>
        <span>{log.createdAt}</span>
      </div>
      <p>{log.scopeSummary}</p>
      <p>{log.contentSummary}</p>
      <code>{log.toolSummary}</code>
    </article>
  );
}

/** 单条应用事件日志卡片，展示运行级别、分类、状态和脱敏上下文。 */
function AppEventLogCard({ log }: { log: AppEventLog }) {
  return (
    <article className={`audit-card event-log-card ${log.level}`}>
      <div className="audit-card-header">
        <strong>
          {formatEventLogLevel(log.level)} · {formatEventLogCategory(log.category)}
        </strong>
        <span>{log.createdAt}</span>
      </div>
      <p>
        {formatEventStatus(log.status)} / {log.event}
      </p>
      <p>{log.message}</p>
      <code>{formatEventLogContext(log)}</code>
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

/** 去重并清理用户在多行输入中的 ID；日志和状态只使用数量。 */
function uniqueTrimmedList(values: string[]) {
  return Array.from(new Set(values.map((value) => value.trim()).filter(Boolean)));
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
