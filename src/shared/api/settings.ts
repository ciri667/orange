import { invokeLogged, isTauriRuntime } from "./runtime";
import { formatLocalDateTime } from "../id";
import {
  browserMock,
  browserProviderTemplates,
  cloneUserSettings,
  createBrowserDiscoveredModels,
  mergeBrowserProviderModels,
} from "../mock/browser";
import {
  LlmProviderConfig,
  LlmProviderModel,
  LlmProviderModelRefreshResult,
  ModelApiKeyStatus,
  ProviderTemplate,
  UserSettings,
} from "../types";

/** 读取用户模型、隐私和写入设置；浏览器开发态返回内存默认值。 */
export async function loadUserSettings(): Promise<UserSettings> {
  if (!isTauriRuntime()) {
    return cloneUserSettings(browserMock.userSettings);
  }

  return invokeLogged<UserSettings>("load_user_settings");
}

/** 保存用户模型、隐私和写入设置；API key 由单独入口处理。 */
export async function saveUserSettings(settings: UserSettings): Promise<UserSettings> {
  if (!isTauriRuntime()) {
    browserMock.userSettings = cloneUserSettings(settings);

    return loadUserSettings();
  }

  return invokeLogged<UserSettings>("save_user_settings", { payload: { settings } });
}

/** 保存 BYOK 模型密钥；桌面端按 providerId 写入系统安全存储并返回读回校验状态。 */
export async function saveModelApiKey(providerId: string, apiKey: string): Promise<ModelApiKeyStatus> {
  if (!isTauriRuntime()) {
    throw new Error("浏览器开发态不能保存模型密钥，请在 Tauri 桌面端配置。");
  }

  return invokeLogged<ModelApiKeyStatus>("save_model_api_key", { payload: { providerId, apiKey } });
}

/** 批量读取每个 provider 的 BYOK 模型密钥状态；不返回明文密钥。 */
export async function loadModelApiKeyStatuses(): Promise<ModelApiKeyStatus[]> {
  if (!isTauriRuntime()) {
    return browserMock.userSettings.modelConfig.providers.map((provider) => ({
      providerId: provider.id,
      keyReference: provider.keyReference,
      configured: false,
      message: "浏览器开发态未连接系统安全存储。",
    }));
  }

  return invokeLogged<ModelApiKeyStatus[]>("load_model_api_key_statuses");
}

/** 读取内置 LLM Provider 模板，供设置页“新增 Provider”入口预填参数。 */
export async function loadLlmProviderTemplates(): Promise<ProviderTemplate[]> {
  if (!isTauriRuntime()) {
    return browserProviderTemplates;
  }

  return invokeLogged<ProviderTemplate[]>("load_llm_provider_templates");
}

/** 刷新单个 LLM provider 的模型列表；桌面端从 keyring 读取密钥，浏览器态返回 mock 列表。 */
export async function refreshLlmProviderModels(providerId: string): Promise<LlmProviderModelRefreshResult> {
  if (!isTauriRuntime()) {
    const fetchedAt = formatLocalDateTime();
    const provider = browserMock.userSettings.modelConfig.providers.find((item) => item.id === providerId);

    if (!provider) {
      throw new Error("找不到要获取模型列表的 Provider。");
    }

    const nextModels = mergeBrowserProviderModels(provider, createBrowserDiscoveredModels(provider), fetchedAt);
    const nextProvider: LlmProviderConfig = {
      ...provider,
      models: nextModels,
      modelsFetchedAt: fetchedAt,
      model: nextModels.find((model) => model.enabled)?.id ?? provider.model,
      updatedAt: fetchedAt,
    };

    browserMock.userSettings = {
      ...browserMock.userSettings,
      modelConfig: {
        ...browserMock.userSettings.modelConfig,
        providers: browserMock.userSettings.modelConfig.providers.map((item) => (item.id === providerId ? nextProvider : item)),
      },
    };

    return {
      settings: cloneUserSettings(browserMock.userSettings),
      providerId,
      fetchedAt,
      fetchedCount: nextModels.length,
      modelCount: nextModels.length,
      enabledCount: nextModels.filter((model) => model.enabled).length,
      message: `浏览器开发态已模拟获取 ${nextModels.length} 个模型。`,
    };
  }

  return invokeLogged<LlmProviderModelRefreshResult>("refresh_llm_provider_models", { payload: { providerId } });
}
