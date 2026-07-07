import {
  clearAppEventLogs,
  deleteAgentSkill,
  installAgentSkill,
  loadAgentSkills,
  loadAppEventLogs,
  loadImGatewayStatus,
  loadImProviderCredentialStatus,
  loadImSettings,
  loadRequestAuditLogs,
  openAppLogFolder,
  openUserSkillsFolder,
  refreshLlmProviderModels,
  saveAgentSkill,
  saveImProviderSecret,
  saveImSettings,
  saveModelApiKey,
  saveUserSettings,
  startImGateway,
  stopImGateway,
  toggleAgentSkill,
} from "../shared/tauriApi";
import type {
  AgentSkill,
  AppEventLog,
  AppEventLogCategory,
  AppEventLogLevel,
  FeishuCredentialStatus,
  FeishuGatewayStatus,
  ImIntegrationSettings,
  InstallAgentSkillPayload,
  InstallAgentSkillResult,
  ModelApiKeyStatus,
  RequestAuditLog,
  UserSettings,
} from "../shared/types";

/** 将未知异常统一转换为可展示文案，避免设置操作错误展示空对象。 */
function formatSettingsErrorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

/** 生成 skill 安装完成提示，保留 warning 数量但限制文本长度避免挤占全局 notice。 */
function buildSkillInstallNotice(result: InstallAgentSkillResult, enabledAfterInstall: boolean) {
  const statusText = enabledAfterInstall ? "已启用。" : "默认停用，审阅后可手动启用。";
  const warningText = result.warnings.length ? ` 警告：${result.warnings.slice(0, 2).join("；")}` : "";
  const extraWarningText = result.warnings.length > 2 ? ` 等 ${result.warnings.length} 条。` : "";
  const notice = `${result.summary} ${statusText}${warningText}${extraWarningText}`;

  return notice.length > 360 ? `${notice.slice(0, 360)}...` : notice;
}

/** 设置动作 hook 依赖的外部状态写入器，保持根组件拥有全局状态归属。 */
interface WorkspaceSettingsActionsOptions {
  beginBusy: (label: string) => void;
  endBusy: () => void;
  setNotice: (notice: string) => void;
  imSettings: ImIntegrationSettings | null;
  feishuCredentialStatus: FeishuCredentialStatus | null;
  feishuGatewayStatus: FeishuGatewayStatus | null;
  setUserSettings: (settings: UserSettings) => void;
  setImSettings: (settings: ImIntegrationSettings) => void;
  setAgentSkills: (skills: AgentSkill[]) => void;
  setModelApiKeyStatuses: (updater: (current: ModelApiKeyStatus[]) => ModelApiKeyStatus[]) => void;
  setFeishuCredentialStatus: (status: FeishuCredentialStatus | null) => void;
  setFeishuGatewayStatus: (status: FeishuGatewayStatus | null) => void;
  setAuditLogs: (logs: RequestAuditLog[]) => void;
  setAppEventLogs: (logs: AppEventLog[]) => void;
}

/** 封装设置页保存、刷新和诊断动作，根组件只负责把 handler 传给抽屉。 */
export function useWorkspaceSettingsActions({
  beginBusy,
  endBusy,
  setNotice,
  imSettings,
  feishuCredentialStatus,
  feishuGatewayStatus,
  setUserSettings,
  setImSettings,
  setAgentSkills,
  setModelApiKeyStatuses,
  setFeishuCredentialStatus,
  setFeishuGatewayStatus,
  setAuditLogs,
  setAppEventLogs,
}: WorkspaceSettingsActionsOptions) {
  /** 保存模型、隐私和写入设置，密钥由单独入口写入系统安全存储。 */
  async function handleSaveSettings(nextSettings: UserSettings) {
    beginBusy("正在保存 Agent 设置...");

    try {
      setUserSettings(await saveUserSettings(nextSettings));
      setNotice("已保存 Agent 设置。");
    } catch (error) {
      setNotice(formatSettingsErrorMessage(error));
    } finally {
      endBusy();
    }
  }

  /** 保存即时通讯设置；敏感凭证由单独入口写入系统安全存储。 */
  async function handleSaveImSettings(nextSettings: ImIntegrationSettings) {
    beginBusy("正在保存即时通讯设置...");

    try {
      setImSettings(await saveImSettings(nextSettings));
      setFeishuGatewayStatus(await loadImGatewayStatus("feishu").catch(() => feishuGatewayStatus));
      setNotice("已保存即时通讯设置。");
    } catch (error) {
      setNotice(formatSettingsErrorMessage(error));
      throw error;
    } finally {
      endBusy();
    }
  }

  /** 保存飞书 appSecret；明文只在本次调用中传给后端 keyring 命令。 */
  async function handleSaveFeishuSecret(appSecret: string) {
    beginBusy("正在保存飞书 appSecret...");

    try {
      const status = await saveImProviderSecret("feishu", appSecret);

      setFeishuCredentialStatus(status);
      setFeishuGatewayStatus(await loadImGatewayStatus("feishu").catch(() => feishuGatewayStatus));
      setNotice("已保存飞书 appSecret。");
    } catch (error) {
      setNotice(formatSettingsErrorMessage(error));
      throw error;
    } finally {
      endBusy();
    }
  }

  /** 手动启动飞书长连接网关；启动失败时保留现有配置供用户修正。 */
  async function handleStartFeishuGateway() {
    beginBusy("正在启动飞书长连接...");

    try {
      setFeishuGatewayStatus(await startImGateway("feishu"));
      setNotice("已启动飞书长连接网关。");
    } catch (error) {
      setFeishuGatewayStatus(await loadImGatewayStatus("feishu").catch(() => feishuGatewayStatus));
      setNotice(formatSettingsErrorMessage(error));
      throw error;
    } finally {
      endBusy();
    }
  }

  /** 手动停止飞书长连接网关；不会清空凭证或白名单。 */
  async function handleStopFeishuGateway() {
    beginBusy("正在停止飞书长连接...");

    try {
      setFeishuGatewayStatus(await stopImGateway("feishu"));
      setNotice("已停止飞书长连接网关。");
    } catch (error) {
      setNotice(formatSettingsErrorMessage(error));
    } finally {
      endBusy();
    }
  }

  /** 刷新飞书网关、凭证和 IM 设置；未授权消息发现的候选对象也通过这里进入设置页。 */
  async function handleRefreshFeishuStatus() {
    beginBusy("正在刷新飞书状态...");

    try {
      const [credentialStatus, gatewayStatus, nextImSettings] = await Promise.all([
        loadImProviderCredentialStatus("feishu").catch(() => feishuCredentialStatus),
        loadImGatewayStatus("feishu").catch(() => feishuGatewayStatus),
        loadImSettings().catch(() => imSettings),
      ]);

      setFeishuCredentialStatus(credentialStatus);
      setFeishuGatewayStatus(gatewayStatus);
      if (nextImSettings) {
        setImSettings(nextImSettings);
      }
    } catch (error) {
      setNotice(formatSettingsErrorMessage(error));
    } finally {
      endBusy();
    }
  }

  /** 保存用户自建 skill 后刷新列表，保证后端归一化后的 ID、name 和时间进入 UI。 */
  async function handleSaveSkill(skill: AgentSkill) {
    beginBusy("正在保存 Skill...");

    try {
      const savedSkill = await saveAgentSkill(skill);
      const nextSkills = await loadAgentSkills();

      setAgentSkills(nextSkills);
      setNotice("已保存 Skill。");

      return savedSkill;
    } catch (error) {
      setNotice(formatSettingsErrorMessage(error));
      throw error;
    } finally {
      endBusy();
    }
  }

  /** 安装第三方 skill 包并刷新列表；日志由 API 层和后端记录脱敏摘要。 */
  async function handleInstallSkill(payload: InstallAgentSkillPayload): Promise<InstallAgentSkillResult> {
    beginBusy("正在安装 Skill...");

    try {
      const result = await installAgentSkill(payload);

      setAgentSkills(result.skills);
      setNotice(buildSkillInstallNotice(result, payload.enableAfterInstall));

      return result;
    } catch (error) {
      const message = formatSettingsErrorMessage(error);

      setNotice(message);
      throw new Error(message);
    } finally {
      endBusy();
    }
  }

  /** 启停 skill 后刷新列表；启用的 skill 会以名称和描述进入 Agent system prompt。 */
  async function handleToggleSkill(skillId: string, enabled: boolean) {
    beginBusy("正在更新 Skill...");

    try {
      await toggleAgentSkill(skillId, enabled);
      const nextSkills = await loadAgentSkills();

      setAgentSkills(nextSkills);
      setNotice("已更新 Skill。");
    } catch (error) {
      setNotice(formatSettingsErrorMessage(error));
      throw error;
    } finally {
      endBusy();
    }
  }

  /** 删除用户自建 skill；内置 skill 由后端拒绝删除并保留为可禁用项。 */
  async function handleDeleteSkill(skillId: string) {
    beginBusy("正在删除 Skill...");

    try {
      const nextSkills = await deleteAgentSkill(skillId);

      setAgentSkills(nextSkills);
      setNotice("已删除 Skill。");
    } catch (error) {
      setNotice(formatSettingsErrorMessage(error));
      throw error;
    } finally {
      endBusy();
    }
  }

  /** 打开橘记 用户 Skills 文件夹；浏览器开发态只展示 mock 路径。 */
  async function handleOpenUserSkillsFolder() {
    beginBusy("正在打开用户 Skills 文件夹...");

    try {
      const skillsFolderPath = await openUserSkillsFolder();

      setNotice(`用户 Skills 文件夹：${skillsFolderPath}`);
    } catch (error) {
      setNotice(formatSettingsErrorMessage(error));
      throw error;
    } finally {
      endBusy();
    }
  }

  /** 保存指定 provider 的 BYOK API key；桌面端写入系统 keyring，避免明文进入 SQLite。 */
  async function handleSaveApiKey(providerId: string, apiKey: string) {
    const trimmedApiKey = apiKey.trim();

    if (!trimmedApiKey) {
      setNotice("API key 不能为空。");
      return;
    }

    beginBusy("正在保存模型密钥...");

    try {
      const nextStatus = await saveModelApiKey(providerId, trimmedApiKey);

      setModelApiKeyStatuses((current) => {
        const withoutProvider = current.filter((status) => status.providerId !== providerId);

        return [...withoutProvider, nextStatus];
      });
      setNotice(nextStatus.message);
    } catch (error) {
      const message = formatSettingsErrorMessage(error);

      setNotice(message);
      throw new Error(message);
    } finally {
      endBusy();
    }
  }

  /** 刷新指定 provider 的模型列表；后端会读取已保存设置和 keyring，不接收明文密钥。 */
  async function handleRefreshProviderModels(providerId: string): Promise<UserSettings> {
    beginBusy("正在获取模型列表...");

    try {
      const result = await refreshLlmProviderModels(providerId);

      setUserSettings(result.settings);
      setNotice(result.message);

      return result.settings;
    } catch (error) {
      const message = formatSettingsErrorMessage(error);

      setNotice(message);
      throw new Error(message);
    } finally {
      endBusy();
    }
  }

  /** 重新读取最近审计日志，便于设置页查看最新模型和工具调用边界。 */
  async function handleRefreshAuditLogs() {
    beginBusy("正在刷新审计日志...");

    try {
      setAuditLogs(await loadRequestAuditLogs());
    } catch (error) {
      setNotice(formatSettingsErrorMessage(error));
    } finally {
      endBusy();
    }
  }

  /** 重新读取最近应用事件日志，支持设置页级别和分类筛选。 */
  async function handleRefreshAppEventLogs(filters?: { level?: AppEventLogLevel | ""; category?: AppEventLogCategory | "" }) {
    beginBusy("正在刷新运行日志...");

    try {
      setAppEventLogs(await loadAppEventLogs({ limit: 100, ...filters }));
    } catch (error) {
      setNotice(formatSettingsErrorMessage(error));
    } finally {
      endBusy();
    }
  }

  /** 清空用户可读事件日志后立即重载列表，保留桌面端文件诊断日志。 */
  async function handleClearAppEventLogs(filters?: { level?: AppEventLogLevel | ""; category?: AppEventLogCategory | "" }) {
    beginBusy("正在清空运行日志...");

    try {
      await clearAppEventLogs();
      setAppEventLogs(await loadAppEventLogs({ limit: 100, ...filters }));
      setNotice("已清空应用事件日志。");
    } catch (error) {
      setNotice(formatSettingsErrorMessage(error));
    } finally {
      endBusy();
    }
  }

  /** 打开系统 app log 目录，便于用户附带文件诊断日志排查问题。 */
  async function handleOpenAppLogFolder() {
    beginBusy("正在打开应用日志目录...");

    try {
      const logFolderPath = await openAppLogFolder();

      setNotice(`应用日志目录：${logFolderPath}`);
    } catch (error) {
      setNotice(formatSettingsErrorMessage(error));
    } finally {
      endBusy();
    }
  }

  return {
    handleSaveSettings,
    handleSaveImSettings,
    handleSaveFeishuSecret,
    handleStartFeishuGateway,
    handleStopFeishuGateway,
    handleRefreshFeishuStatus,
    handleSaveSkill,
    handleInstallSkill,
    handleToggleSkill,
    handleDeleteSkill,
    handleOpenUserSkillsFolder,
    handleSaveApiKey,
    handleRefreshProviderModels,
    handleRefreshAuditLogs,
    handleRefreshAppEventLogs,
    handleClearAppEventLogs,
    handleOpenAppLogFolder,
  };
}
