import {
  BookOpen,
  Brain,
  History,
  MessageCircle,
  ScrollText,
  Settings2,
  ShieldCheck,
  Sparkles,
  X,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { Button } from "../shared/Button";
import { ConfirmDialog } from "../shared/ConfirmDialog";
import { createLocalId, formatLocalDateTime } from "../shared/id";
import { ListRow } from "../shared/ListRow";
import { logDebug, logError, logInfo } from "../shared/logger";
import { ModalBackdrop, ModalHeader } from "../shared/Modal";
import { cn } from "../shared/cn";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import {
  addManualProviderModel,
  removeManualProviderModel,
  updateManualProviderModel,
  type ProviderModelMutationResult,
} from "../shared/providerModels";
import { sectionLabelClassName } from "../shared/ui";
import type {
  AgentSkill,
  AppEventLog,
  AppEventLogCategory,
  AppEventLogLevel,
  InstallAgentSkillPayload,
  InstallAgentSkillResult,
  FeishuIntegrationSettings,
  FeishuCredentialStatus,
  FeishuGatewayStatus,
  ImIntegrationSettings,
  KnowledgeBase,
  KnowledgeBaseMemory,
  Note,
  LlmProviderConfig,
  ModelApiKeyStatus,
  ProviderTemplate,
  RequestAuditLog,
  UserSettings,
} from "../shared/types";
import { SkillsModal } from "./SkillsModal";
import {
  AgentMemorySettingsSection,
  AuditLogsSettingsSection,
  EventLogsSettingsSection,
  ImSettingsSection,
  KnowledgeSettingsSection,
  ModelSettingsSection,
  AgentSecuritySettingsSection,
  SkillsSettingsSection,
} from "./SettingsSections";

/** 设置页左侧导航的可选分区，和右侧主内容一一对应。 */
type SettingsSectionId = "knowledge" | "model" | "agentSecurity" | "im" | "skills" | "agentMemory" | "eventLogs" | "auditLogs";

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

/** 构造飞书 provider 默认草稿；用于兼容旧 mock 或缺失 provider 的异常状态。 */
function createDefaultFeishuProvider(): FeishuIntegrationSettings {
  return {
    providerId: "feishu",
    enabled: false,
    defaultKnowledgeBaseIds: [],
    allowedUserOpenIds: [],
    allowedChatIds: [],
    discoveredUserOpenIds: [],
    discoveredChatIds: [],
    requireMention: true,
    updatedAt: "刚刚",
    config: {
      type: "feishu",
      domain: "feishu",
      appId: "",
      secretKeyReference: "orange-feishu-app-secret",
    },
  };
}

/** 从 IM 设置中读取飞书 provider；首版 UI 只渲染该 provider。 */
function getFeishuProvider(settings: ImIntegrationSettings): FeishuIntegrationSettings {
  const provider = settings.providers.find((candidate) => candidate.providerId === "feishu");

  return provider ? (provider as FeishuIntegrationSettings) : createDefaultFeishuProvider();
}

/** 更新飞书 provider，并保证 providers 数组中只替换对应项，不影响未来其它 IM provider。 */
function updateFeishuProvider(
  settings: ImIntegrationSettings,
  updater: (provider: FeishuIntegrationSettings) => FeishuIntegrationSettings,
): ImIntegrationSettings {
  const currentProvider = getFeishuProvider(settings);
  const nextProvider = updater(currentProvider);
  const hasFeishuProvider = settings.providers.some((provider) => provider.providerId === "feishu");

  return {
    ...settings,
    providers: hasFeishuProvider
      ? settings.providers.map((provider) => (provider.providerId === "feishu" ? nextProvider : provider))
      : [...settings.providers, nextProvider],
  };
}

/** 设置抽屉，展示多知识库、模型策略、Skills 和诊断信息。 */
export function SettingsDrawer({
  knowledgeBases,
  notes,
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
  knowledgeBaseMemories,
  isBusy,
  onSelectKnowledgeBase,
  onAddKnowledgeBase,
  onRescanKnowledgeBase,
  onRemoveKnowledgeBase,
  onCreateOrOpenProjectInstruction,
  onSaveSettings,
  onSaveImSettings,
  onSaveKnowledgeBaseMemory,
  onDeleteKnowledgeBaseMemory,
  onSaveSkill,
  onInstallSkill,
  onToggleSkill,
  onDeleteSkill,
  onOpenUserSkillsFolder,
  onRevealApiKey,
  onSaveApiKey,
  onRefreshProviderModels,
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
  notes: Note[];
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
  knowledgeBaseMemories: KnowledgeBaseMemory[];
  isBusy: boolean;
  onSelectKnowledgeBase: (knowledgeBaseId: string) => void;
  onAddKnowledgeBase: () => void;
  onRescanKnowledgeBase: (knowledgeBaseId: string) => void;
  onRemoveKnowledgeBase: (knowledgeBaseId: string) => void;
  onCreateOrOpenProjectInstruction: (knowledgeBaseId: string) => void;
  onSaveSettings: (settings: UserSettings) => Promise<void> | void;
  onSaveImSettings: (settings: ImIntegrationSettings) => Promise<void> | void;
  onSaveKnowledgeBaseMemory: (memory: KnowledgeBaseMemory) => Promise<KnowledgeBaseMemory> | KnowledgeBaseMemory;
  onDeleteKnowledgeBaseMemory: (knowledgeBaseId: string) => Promise<void> | void;
  onSaveSkill: (skill: AgentSkill) => Promise<AgentSkill | void> | AgentSkill | void;
  onInstallSkill: (payload: InstallAgentSkillPayload) => Promise<InstallAgentSkillResult> | InstallAgentSkillResult;
  onToggleSkill: (skillId: string, enabled: boolean) => Promise<void> | void;
  onDeleteSkill: (skillId: string) => Promise<void> | void;
  onOpenUserSkillsFolder: () => Promise<void> | void;
  onRevealApiKey: (providerId: string) => Promise<string>;
  onSaveApiKey: (providerId: string, apiKey: string) => Promise<void> | void;
  onRefreshProviderModels: (providerId: string) => Promise<UserSettings> | UserSettings;
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
  /** 当前哪些 provider 正在以明文显示密钥；关闭设置抽屉后随组件卸载丢弃。 */
  const [apiKeyVisibleByProvider, setApiKeyVisibleByProvider] = useState<Record<string, boolean>>({});
  /** 本次抽屉会话内已揭示或刚保存的密钥，用于再次查看和判断保存按钮是否有改动。 */
  const [revealedApiKeyByProvider, setRevealedApiKeyByProvider] = useState<Record<string, string>>({});
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
  /** 已启用跨会话记忆的知识库数量，用于设置页导航计数。 */
  const enabledMemoryCount = knowledgeBaseMemories.filter((memory) => memory.enabled).length;
  /** 当前可配置的飞书 provider 草稿；后续多 IM provider 可在此扩展为 tabs/list。 */
  const feishuProviderDraft = getFeishuProvider(imSettingsDraft);
  /** 设置工作台顶部摘要只展示计数和状态，避免路径、密钥和请求内容外露。 */
  const settingsSummary = {
    knowledgeBaseCount: knowledgeBases.length,
    providerCount: settingsDraft.modelConfig.providers.length,
    feishuStatus: feishuGatewayStatus?.running ? "运行中" : feishuProviderDraft.enabled ? "已配置" : "未启用",
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
        label: "模型设置",
        description: "模型配置和行为边界",
        meta: settingsDraft.modelConfig.enabled ? "已启用" : "未启用",
        icon: Settings2,
        tone: settingsDraft.modelConfig.enabled ? "success" : "neutral",
      },
      {
        id: "agentSecurity",
        group: "配置",
        label: "Agent 权限",
        description: "你愿意把多少执行权交给 Agent",
        meta: `${settingsDraft.agentSecurity.resourceLimits.timeoutSeconds}s`,
        icon: ShieldCheck,
        tone: "neutral",
      },
      {
        id: "im",
        group: "配置",
        label: "即时通讯",
        description: "飞书/Lark 长连接和白名单",
        meta: settingsSummary.feishuStatus,
        icon: MessageCircle,
        tone: feishuGatewayStatus?.running ? "success" : feishuProviderDraft.enabled ? "warning" : "neutral",
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
        id: "agentMemory",
        group: "配置",
        label: "Agent 记忆",
        description: "跨会话长期偏好和约定",
        meta: `${enabledMemoryCount}/${knowledgeBaseMemories.length}`,
        icon: Brain,
        tone: enabledMemoryCount > 0 ? "success" : "neutral",
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
      enabledMemoryCount,
      enabledSkillCount,
      feishuGatewayStatus?.running,
      feishuProviderDraft.enabled,
      knowledgeBaseMemories,
      settingsSummary.feishuStatus,
      knowledgeBases,
      settingsDraft.modelConfig.enabled,
      settingsDraft.agentSecurity.resourceLimits.timeoutSeconds,
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
      feishuProviderDraft.defaultKnowledgeBaseIds.length > 0
    ) {
      return;
    }

    hasSeededDefaultImKnowledgeBaseRef.current = true;

    // 默认范围只写入设置页草稿；仍由用户保存或启动时显式持久化，避免打开设置页就改配置。
    setImSettingsDraft((currentSettings) => ({
      ...updateFeishuProvider(currentSettings, (provider) => ({
        ...provider,
        defaultKnowledgeBaseIds: [fallbackKnowledgeBaseId],
      })),
    }));
  }, [activeKnowledgeBaseId, feishuProviderDraft.defaultKnowledgeBaseIds.length, knowledgeBases]);

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
  function buildNormalizedImSettings(settingsDraftForSave: ImIntegrationSettings = imSettingsDraft): ImIntegrationSettings {
    const feishu = getFeishuProvider(settingsDraftForSave);
    const allowedUserOpenIds = uniqueTrimmedList(feishu.allowedUserOpenIds);
    const allowedChatIds = uniqueTrimmedList(feishu.allowedChatIds);

    return updateFeishuProvider(settingsDraftForSave, (provider) => ({
      ...provider,
      config: {
        ...provider.config,
        appId: provider.config.appId.trim(),
      },
      defaultKnowledgeBaseIds: uniqueTrimmedList(provider.defaultKnowledgeBaseIds),
      allowedUserOpenIds,
      allowedChatIds,
      discoveredUserOpenIds: uniqueTrimmedList(provider.discoveredUserOpenIds).filter((openId) => !allowedUserOpenIds.includes(openId)),
      discoveredChatIds: uniqueTrimmedList(provider.discoveredChatIds).filter((chatId) => !allowedChatIds.includes(chatId)),
      updatedAt: formatLocalDateTime(),
    }));
  }

  /** 保存即时通讯设置；日志只记录计数和状态，不记录 open_id/chat_id 原文。 */
  async function handleSaveImSettings(options: { enableFeishuBeforeSave?: boolean } = {}) {
    const startedAt = performance.now();
    const draftForSave = options.enableFeishuBeforeSave
      ? updateFeishuProvider(imSettingsDraft, (provider) => ({
          ...provider,
          // 启动网关代表用户要让该 provider 接收消息；先落库 enabled，避免 sidecar 收到消息后被运行态拦截。
          enabled: true,
          updatedAt: formatLocalDateTime(),
        }))
      : imSettingsDraft;
    const nextSettings = buildNormalizedImSettings(draftForSave);
    const feishu = getFeishuProvider(nextSettings);

    logInfo("设置页保存即时通讯设置。", {
      category: "im",
      event: "im_settings_save",
      status: "started",
      metadata: {
        providerId: "feishu",
        feishuEnabled: feishu.enabled,
        domain: feishu.config.domain,
        knowledgeBaseCount: feishu.defaultKnowledgeBaseIds.length,
        allowedUserCount: feishu.allowedUserOpenIds.length,
        allowedChatCount: feishu.allowedChatIds.length,
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
      await handleSaveImSettings({ enableFeishuBeforeSave: true });
      await onStartFeishuGateway();
      logInfo("设置页启动飞书网关完成。", {
        category: "im",
        event: "im_gateway_start_from_settings",
        status: "completed",
        durationMs: performance.now() - startedAt,
        metadata: { providerId: "feishu" },
      });
    } catch (error) {
      logError("设置页启动飞书网关失败。", {
        category: "im",
        event: "im_gateway_start_from_settings",
        status: "failed",
        durationMs: performance.now() - startedAt,
        metadata: { providerId: "feishu" },
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
  function updateFeishuDraft<K extends keyof FeishuIntegrationSettings>(
    field: K,
    value: FeishuIntegrationSettings[K],
  ) {
    setImSettingsDraft((currentSettings) => ({
      ...updateFeishuProvider(currentSettings, (provider) => ({
        ...provider,
        [field]: value,
      })),
    }));
  }

  /** 更新飞书 provider config 草稿中的单个字段，避免平台字段混入通用 provider 顶层。 */
  function updateFeishuConfigDraft<K extends keyof FeishuIntegrationSettings["config"]>(
    field: K,
    value: FeishuIntegrationSettings["config"][K],
  ) {
    setImSettingsDraft((currentSettings) => ({
      ...updateFeishuProvider(currentSettings, (provider) => ({
        ...provider,
        config: {
          ...provider.config,
          [field]: value,
        },
      })),
    }));
  }

  /** 将 textarea 的多行 ID 转成去重数组；不记录原始 ID 内容。 */
  function parseMultilineIds(value: string) {
    return uniqueTrimmedList(value.split(/\r?\n|,/));
  }

  /** 将后端自动发现的飞书用户加入 allowlist，同时从候选列表移除。 */
  function allowDiscoveredFeishuUser(openId: string) {
    const allowedUserOpenIds = uniqueTrimmedList([...feishuProviderDraft.allowedUserOpenIds, openId]);

    setImSettingsDraft((currentSettings) => ({
      ...updateFeishuProvider(currentSettings, (provider) => ({
        ...provider,
        allowedUserOpenIds,
        discoveredUserOpenIds: provider.discoveredUserOpenIds.filter((candidate) => candidate !== openId),
      })),
    }));
  }

  /** 将后端自动发现的飞书群加入 allowlist，同时从候选列表移除。 */
  function allowDiscoveredFeishuChat(chatId: string) {
    const allowedChatIds = uniqueTrimmedList([...feishuProviderDraft.allowedChatIds, chatId]);

    setImSettingsDraft((currentSettings) => ({
      ...updateFeishuProvider(currentSettings, (provider) => ({
        ...provider,
        allowedChatIds,
        discoveredChatIds: provider.discoveredChatIds.filter((candidate) => candidate !== chatId),
      })),
    }));
  }

  /** 密钥已写入系统安全存储后：收起明文、清空输入，并缓存新值供本次抽屉内再次查看。 */
  function markApiKeyPersisted(providerId: string, apiKey: string) {
    setApiKeyDraftByProvider((current) => ({ ...current, [providerId]: "" }));
    setApiKeyVisibleByProvider((current) => ({ ...current, [providerId]: false }));
    setRevealedApiKeyByProvider((current) => ({ ...current, [providerId]: apiKey }));
  }

  /** 切换密钥显隐；草稿为空且已配置时才向系统安全存储按需读取明文。 */
  async function handleToggleApiKeyVisibility(providerId: string) {
    if (isBusy) {
      return;
    }

    const isVisible = apiKeyVisibleByProvider[providerId] === true;
    const apiKeyDraft = apiKeyDraftByProvider[providerId] ?? "";
    const revealedApiKey = revealedApiKeyByProvider[providerId];

    if (isVisible) {
      if (revealedApiKey !== undefined && apiKeyDraft === revealedApiKey) {
        setApiKeyDraftByProvider((current) => ({ ...current, [providerId]: "" }));
      }

      setApiKeyVisibleByProvider((current) => ({ ...current, [providerId]: false }));
      return;
    }

    if (apiKeyDraft.trim()) {
      setApiKeyVisibleByProvider((current) => ({ ...current, [providerId]: true }));
      return;
    }

    const keyStatus = modelApiKeyStatuses.find((status) => status.providerId === providerId);
    if (!keyStatus?.configured) {
      setApiKeyVisibleByProvider((current) => ({ ...current, [providerId]: true }));
      return;
    }

    if (revealedApiKey) {
      setApiKeyDraftByProvider((current) => ({ ...current, [providerId]: revealedApiKey }));
      setApiKeyVisibleByProvider((current) => ({ ...current, [providerId]: true }));
      return;
    }

    const startedAt = performance.now();

    logInfo("设置页查看模型密钥。", {
      category: "settings",
      event: "model_api_key_reveal",
      status: "started",
      metadata: { providerId },
    });

    try {
      const apiKey = await onRevealApiKey(providerId);

      setRevealedApiKeyByProvider((current) => ({ ...current, [providerId]: apiKey }));
      setApiKeyDraftByProvider((current) => ({ ...current, [providerId]: apiKey }));
      setApiKeyVisibleByProvider((current) => ({ ...current, [providerId]: true }));
      logInfo("设置页查看模型密钥完成。", {
        category: "settings",
        event: "model_api_key_reveal",
        status: "completed",
        durationMs: performance.now() - startedAt,
        metadata: { providerId },
      });
    } catch (error) {
      logError("设置页查看模型密钥失败。", {
        category: "settings",
        event: "model_api_key_reveal",
        status: "failed",
        durationMs: performance.now() - startedAt,
        metadata: { providerId },
        error,
      });
    }
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
      markApiKeyPersisted(providerId, apiKeyDraft.trim());
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

  /** 保存当前模型草稿和临时 API key 后刷新 provider 模型列表。 */
  async function handleRefreshProviderModels(providerId: string) {
    const startedAt = performance.now();
    const apiKeyDraft = apiKeyDraftByProvider[providerId] ?? "";

    logInfo("设置页刷新 Provider 模型列表。", {
      category: "settings",
      event: "provider_models_refresh",
      status: "started",
      metadata: {
        providerId,
        hasApiKeyDraft: Boolean(apiKeyDraft.trim()),
      },
    });

    try {
      if (apiKeyDraft.trim()) {
        await onSaveApiKey(providerId, apiKeyDraft);
        markApiKeyPersisted(providerId, apiKeyDraft.trim());
      }

      await handleSaveSettings();
      const nextSettings = await onRefreshProviderModels(providerId);

      setSettingsDraft(nextSettings);
      logInfo("设置页刷新 Provider 模型列表完成。", {
        category: "settings",
        event: "provider_models_refresh",
        status: "completed",
        durationMs: performance.now() - startedAt,
        metadata: { providerId },
      });
    } catch (error) {
      logError("设置页刷新 Provider 模型列表失败。", {
        category: "settings",
        event: "provider_models_refresh",
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
        // 开启云端模型时默认 Provider 必须同步启用；默认 Provider 在 UI 中不可关闭。
        providers:
          field === "enabled" && value
            ? currentSettings.modelConfig.providers.map((provider) =>
                provider.id === currentSettings.modelConfig.defaultProviderId
                  ? { ...provider, enabled: true, updatedAt: formatLocalDateTime() }
                  : provider,
              )
            : currentSettings.modelConfig.providers,
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
                models:
                  field === "model" && typeof value === "string"
                    ? provider.models.map((model) => (model.id === value ? { ...model, enabled: true } : model))
                    : provider.models,
                updatedAt: formatLocalDateTime(),
              }
            : provider,
        ),
      },
    }));
  }

  /** 启停 provider 下的单个模型；默认模型必须保持启用，避免运行态无模型可用。 */
  function updateProviderModelEnabled(providerId: string, modelId: string, enabled: boolean) {
    setSettingsDraft((currentSettings) => ({
      ...currentSettings,
      modelConfig: {
        ...currentSettings.modelConfig,
        providers: currentSettings.modelConfig.providers.map((provider) => {
          if (provider.id !== providerId || provider.model === modelId) {
            return provider;
          }

          return {
            ...provider,
            models: provider.models.map((model) => (model.id === modelId ? { ...model, enabled } : model)),
            updatedAt: formatLocalDateTime(),
          };
        }),
      },
    }));
  }

  /** 把手动模型增删改应用到草稿；失败时返回可展示文案，成功返回 null。 */
  function applyProviderModelMutation(
    providerId: string,
    mutate: (provider: LlmProviderConfig) => ProviderModelMutationResult,
  ) {
    let error: string | null = null;

    setSettingsDraft((currentSettings) => {
      const provider = currentSettings.modelConfig.providers.find((item) => item.id === providerId);

      if (!provider) {
        error = "找不到要更新的 Provider。";
        return currentSettings;
      }

      const result = mutate(provider);

      if (!result.ok) {
        error = result.error;
        return currentSettings;
      }

      return {
        ...currentSettings,
        modelConfig: {
          ...currentSettings.modelConfig,
          providers: currentSettings.modelConfig.providers.map((item) =>
            item.id === providerId ? result.provider : item,
          ),
        },
      };
    });

    return error;
  }

  /** 向指定 provider 追加一条手动模型，供自定义兼容服务补模型 ID。 */
  function addProviderModel(providerId: string, modelId: string, name: string, contextLength?: number) {
    const error = applyProviderModelMutation(providerId, (provider) =>
      addManualProviderModel(provider, modelId, name, formatLocalDateTime(), contextLength),
    );

    if (!error) {
      logInfo("设置页新增手动模型。", {
        category: "settings",
        event: "provider_model_add",
        status: "completed",
        metadata: { providerId, modelId: modelId.trim() },
      });
    }

    return error;
  }

  /** 修改手动模型的 ID 或显示名；若改的是当前默认模型，会同步 provider.model。 */
  function updateProviderModel(
    providerId: string,
    originalId: string,
    nextId: string,
    nextName: string,
    contextLength?: number,
  ) {
    const error = applyProviderModelMutation(providerId, (provider) =>
      updateManualProviderModel(provider, originalId, nextId, nextName, formatLocalDateTime(), contextLength),
    );

    if (!error) {
      logInfo("设置页更新手动模型。", {
        category: "settings",
        event: "provider_model_update",
        status: "completed",
        metadata: { providerId, originalId, modelId: nextId.trim() },
      });
    }

    return error;
  }

  /** 从草稿中删除手动模型；默认模型和发现的模型不能删。 */
  function removeProviderModel(providerId: string, modelId: string) {
    const error = applyProviderModelMutation(providerId, (provider) =>
      removeManualProviderModel(provider, modelId, formatLocalDateTime()),
    );

    if (!error) {
      logInfo("设置页删除手动模型。", {
        category: "settings",
        event: "provider_model_remove",
        status: "completed",
        metadata: { providerId, modelId },
      });
    }

    return error;
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
      keyReference: `orange-llm-provider-${providerId}-api-key`,
      enabled: true,
      supportsTools: true,
      requiresApiKey: template.requiresApiKey,
      models: template.model
        ? [
            {
              id: template.model,
              name: template.model,
              enabled: true,
              source: "manual",
              updatedAt: now,
            },
          ]
        : [],
      modelsFetchedAt: undefined,
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
      <div className="grid gap-2 max-[1100px]:min-w-0 max-[1100px]:shrink-0" key={group}>
        <p className={sectionLabelClassName}>{group}</p>
        <div className="grid gap-[7px] max-[1100px]:flex max-[1100px]:gap-2">
          {groupItems.map((item) => {
            const SectionIcon = item.icon;

            return (
              <ListRow
                className="grid grid-cols-[auto_minmax(0,1fr)_auto]"
                active={activeSection === item.id}
                key={item.id}
                aria-current={activeSection === item.id ? "page" : undefined}
                onClick={() => handleActiveSectionChange(item.id)}
              >
                <SectionIcon size={17} className={activeSection === item.id ? "text-accent-strong" : "text-ink-muted"} />
                <span className="min-w-0">
                  <OverflowTooltipText as="strong" className="block truncate text-ink-strong" text={item.label} logArea="settings_nav_label" />
                  <OverflowTooltipText as="small" className="block truncate text-xs text-ink-muted" text={item.description} logArea="settings_nav_description" />
                </span>
                <OverflowTooltipText
                  as="em"
                  className={cn(
                    "whitespace-nowrap rounded-full border border-[rgba(230,224,214,0.78)] bg-surface-muted px-[7px] py-[3px] text-[11px] not-italic text-ink-muted",
                    item.tone === "success" && "bg-success-soft text-success",
                    item.tone === "warning" && "border-[rgba(var(--warning-rgb),0.28)] bg-warning-soft text-warning",
                  )}
                  text={item.meta}
                  logArea="settings_nav_meta"
                />
              </ListRow>
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
        <KnowledgeSettingsSection
          knowledgeBases={knowledgeBases}
          notes={notes}
          activeKnowledgeBaseId={activeKnowledgeBaseId}
          isBusy={isBusy}
          onSelectKnowledgeBase={onSelectKnowledgeBase}
          onAddKnowledgeBase={onAddKnowledgeBase}
          onRescanKnowledgeBase={onRescanKnowledgeBase}
          onRemoveKnowledgeBase={onRemoveKnowledgeBase}
          onCreateOrOpenProjectInstruction={onCreateOrOpenProjectInstruction}
        />
      );
    }

    if (activeSection === "model") {
      return (
        <ModelSettingsSection
          settingsDraft={settingsDraft}
          providerTemplates={providerTemplates}
          selectedTemplateId={selectedTemplateId}
          modelApiKeyStatuses={modelApiKeyStatuses}
          apiKeyDraftByProvider={apiKeyDraftByProvider}
          apiKeyVisibleByProvider={apiKeyVisibleByProvider}
          revealedApiKeyByProvider={revealedApiKeyByProvider}
          isBusy={isBusy}
          onSaveSettings={handleSaveSettings}
          onModelEnabledChange={(enabled) => updateModelConfig("enabled", enabled)}
          onPrivacyPolicyChange={(privacyPolicy) =>
            setSettingsDraft((currentSettings) => ({
              ...currentSettings,
              privacyPolicy,
            }))
          }
          onSelectedTemplateIdChange={setSelectedTemplateId}
          onAddProviderFromTemplate={addProviderFromTemplate}
          onProviderFieldChange={updateProviderField}
          onSetDefaultProvider={setDefaultProvider}
          onRequestRemoveProvider={requestRemoveProvider}
          onApiKeyDraftChange={(providerId, apiKey) =>
            setApiKeyDraftByProvider((current) => ({ ...current, [providerId]: apiKey }))
          }
          onToggleApiKeyVisibility={handleToggleApiKeyVisibility}
          onSaveApiKey={handleSaveApiKey}
          onRefreshProviderModels={handleRefreshProviderModels}
          onProviderModelEnabledChange={updateProviderModelEnabled}
          onAddProviderModel={addProviderModel}
          onUpdateProviderModel={updateProviderModel}
          onRemoveProviderModel={removeProviderModel}
        />
      );
    }

    if (activeSection === "skills") {
      return (
        <SkillsSettingsSection
          skills={skills}
          enabledSkillCount={enabledSkillCount}
          customSkillCount={customSkillCount}
          isBusy={isBusy}
          onOpenSkillsModal={() => setIsSkillsModalOpen(true)}
          onSaveSettings={handleSaveSettings}
        />
      );
    }

    if (activeSection === "agentSecurity") {
      return (
        <AgentSecuritySettingsSection
          settingsDraft={settingsDraft}
          isBusy={isBusy}
          onSettingsChange={setSettingsDraft}
          onSaveSettings={handleSaveSettings}
        />
      );
    }

    if (activeSection === "agentMemory") {
      return (
        <AgentMemorySettingsSection
          knowledgeBases={knowledgeBases}
          knowledgeBaseMemories={knowledgeBaseMemories}
          isBusy={isBusy}
          onSaveKnowledgeBaseMemory={onSaveKnowledgeBaseMemory}
          onDeleteKnowledgeBaseMemory={onDeleteKnowledgeBaseMemory}
        />
      );
    }

    if (activeSection === "im") {
      return (
        <ImSettingsSection
          knowledgeBases={knowledgeBases}
          feishu={feishuProviderDraft}
          feishuCredentialStatus={feishuCredentialStatus}
          feishuGatewayStatus={feishuGatewayStatus}
          feishuSecretDraft={feishuSecretDraft}
          isBusy={isBusy}
          onFeishuDraftChange={updateFeishuDraft}
          onFeishuConfigDraftChange={updateFeishuConfigDraft}
          onFeishuSecretDraftChange={setFeishuSecretDraft}
          onParseMultilineIds={parseMultilineIds}
          onAllowDiscoveredFeishuUser={allowDiscoveredFeishuUser}
          onAllowDiscoveredFeishuChat={allowDiscoveredFeishuChat}
          onRefreshFeishuStatus={onRefreshFeishuStatus}
          onStopFeishuGateway={onStopFeishuGateway}
          onStartFeishuGateway={handleStartFeishuGateway}
          onSaveImSettings={() => handleSaveImSettings()}
          onSaveFeishuSecret={handleSaveFeishuSecret}
        />
      );
    }

    if (activeSection === "eventLogs") {
      return (
        <EventLogsSettingsSection
          appEventLogs={appEventLogs}
          eventLogLevel={eventLogLevel}
          eventLogCategory={eventLogCategory}
          isBusy={isBusy}
          onEventLogLevelChange={handleEventLogLevelChange}
          onEventLogCategoryChange={handleEventLogCategoryChange}
          onRefreshAppEventLogs={() => onRefreshAppEventLogs(currentEventLogFilters())}
          onClearAppEventLogs={() => onClearAppEventLogs(currentEventLogFilters())}
          onOpenAppLogFolder={onOpenAppLogFolder}
        />
      );
    }

    return <AuditLogsSettingsSection auditLogs={auditLogs} isBusy={isBusy} onRefreshAuditLogs={onRefreshAuditLogs} />;
  }

  return (
    <ModalBackdrop onClose={onClose} className="z-settings p-6 max-[760px]:p-2.5">
      <aside
        className="settings-drawer relative grid h-[min(820px,calc(100vh-40px))] w-[min(1120px,calc(100vw-40px))] min-h-0 isolate grid-rows-[auto_minmax(0,1fr)] overflow-hidden rounded-panel border border-border-translucent bg-surface-translucent-strong shadow-app max-[760px]:h-[calc(100vh-20px)] max-[760px]:w-[min(100%,calc(100vw-20px))]"
        aria-label="设置"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <ModalHeader className="px-5 py-[18px]">
          <div className="min-w-0">
            <p className={sectionLabelClassName}>Settings</p>
            <h2 className="mt-1 mb-0 text-xl leading-tight text-ink-strong">设置工作台</h2>
          </div>
          <Button variant="icon" title="关闭设置" onClick={onClose}>
            <X size={18} />
          </Button>
        </ModalHeader>

        <div className="grid min-h-0 min-w-0 grid-cols-[260px_minmax(0,1fr)]">
          <nav className="grid min-h-0 min-w-0 content-start gap-[18px] overflow-auto border-r border-border bg-warm-panel p-4" aria-label="设置项">
            <div className="grid gap-[7px] rounded-control border border-border-translucent bg-surface-translucent p-2.5 text-xs text-ink-muted" aria-label="设置摘要">
              <strong className="text-[13px] text-ink-strong">本地 Agent 环境</strong>
              <span>{settingsSummary.knowledgeBaseCount} 个资料库</span>
              <span>{settingsSummary.providerCount} 个模型 Provider</span>
              <span>{settingsSummary.enabledSkillCount} 个 Skill 启用</span>
              {settingsSummary.errorLogCount > 0 && <em className="not-italic text-danger">{settingsSummary.errorLogCount} 条错误日志</em>}
            </div>
            {SETTINGS_SECTION_GROUPS.map((group) => renderNavigationGroup(group))}
          </nav>
          <main className="min-h-0 min-w-0 overflow-auto bg-surface p-5 max-[1100px]:p-4 max-[760px]:p-4" aria-label="设置主要内容">
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
    </ModalBackdrop>
  );
}

/** 去重并清理用户在多行输入中的 ID；日志和状态只使用数量。 */
function uniqueTrimmedList(values: string[]) {
  return Array.from(new Set(values.map((value) => value.trim()).filter(Boolean)));
}
