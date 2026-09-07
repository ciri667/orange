import { invokeLogged, isTauriRuntime } from "./runtime";
import { formatLocalDateTime } from "../id";
import { browserMock, cloneImSettings, getImProvider } from "../mock/browser";
import {
  FeishuCredentialStatus,
  FeishuGatewayStatus,
  ImGatewayStatus,
  ImIntegrationSettings,
  ImLoginStatus,
  ImProviderCredentialStatus,
  ImProviderId,
} from "../types";

/** 读取即时通讯设置；浏览器开发态返回内存 mock。 */
export async function loadImSettings(): Promise<ImIntegrationSettings> {
  if (!isTauriRuntime()) {
    return cloneImSettings(browserMock.imSettings);
  }

  return invokeLogged<ImIntegrationSettings>("load_im_settings");
}

/** 保存即时通讯设置；敏感凭证由独立 keyring 命令处理。 */
export async function saveImSettings(settings: ImIntegrationSettings): Promise<ImIntegrationSettings> {
  if (!isTauriRuntime()) {
    browserMock.imSettings = cloneImSettings(settings);
    for (const provider of settings.providers) {
      const current = browserMock.imGatewayByProvider[provider.providerId] ?? browserMock.imGatewayByProvider.feishu;
      const identityConfigured =
        provider.config.type === "feishu"
          ? Boolean(provider.config.appId.trim())
          : provider.config.type === "qq"
            ? Boolean(provider.config.appId.trim())
            : Boolean(provider.config.accountId.trim());
      browserMock.imGatewayByProvider[provider.providerId] = {
        ...current,
        providerId: provider.providerId,
        domain: provider.config.type === "feishu" ? provider.config.domain : provider.providerId,
        appIdConfigured: identityConfigured,
      };
    }
    browserMock.feishuGatewayStatus = browserMock.imGatewayByProvider.feishu;

    return loadImSettings();
  }

  return invokeLogged<ImIntegrationSettings>("save_im_settings", { payload: { settings } });
}

/** 保存 IM provider secret；桌面端写入系统安全存储，浏览器态只返回不可用说明。 */
export async function saveImProviderSecret(providerId: ImProviderId, secret: string): Promise<ImProviderCredentialStatus> {
  if (!isTauriRuntime()) {
    throw new Error("浏览器开发态不能保存 IM provider secret，请在 Tauri 桌面端配置。");
  }

  return invokeLogged<ImProviderCredentialStatus>("save_im_provider_secret", { payload: { providerId, secret } });
}

/** 读取 IM provider secret 是否已配置；不会返回明文 secret。 */
export async function loadImProviderCredentialStatus(providerId: ImProviderId): Promise<ImProviderCredentialStatus> {
  if (!isTauriRuntime()) {
    const provider = getImProvider(browserMock.imSettings, providerId);

    return {
      providerId,
      keyReference: provider.config.secretKeyReference,
      configured: false,
      message: "浏览器开发态未连接系统安全存储。",
    };
  }

  return invokeLogged<ImProviderCredentialStatus>("load_im_provider_credential_status", { payload: { providerId } });
}

/** 启动 IM provider 长连接网关；浏览器态只返回不可用状态。 */
export async function startImGateway(providerId: ImProviderId): Promise<ImGatewayStatus> {
  if (!isTauriRuntime()) {
    browserMock.imGatewayByProvider[providerId] = {
      ...browserMock.imGatewayByProvider[providerId],
      providerId,
      running: false,
      connected: false,
      lastError: "浏览器开发态不能启动 IM 长连接网关。",
    };
    browserMock.feishuGatewayStatus = browserMock.imGatewayByProvider.feishu;

    return browserMock.imGatewayByProvider[providerId];
  }

  return invokeLogged<ImGatewayStatus>("start_im_gateway", { payload: { providerId } });
}

/** 停止 IM provider 长连接网关；不会清空设置和凭证。 */
export async function stopImGateway(providerId: ImProviderId): Promise<ImGatewayStatus> {
  if (!isTauriRuntime()) {
    browserMock.imGatewayByProvider[providerId] = {
      ...browserMock.imGatewayByProvider[providerId],
      providerId,
      running: false,
      connected: false,
      lastStoppedAt: formatLocalDateTime(),
    };
    browserMock.feishuGatewayStatus = browserMock.imGatewayByProvider.feishu;

    return browserMock.imGatewayByProvider[providerId];
  }

  return invokeLogged<ImGatewayStatus>("stop_im_gateway", { payload: { providerId } });
}

/** 读取 IM provider 长连接网关运行态。 */
export async function loadImGatewayStatus(providerId: ImProviderId): Promise<ImGatewayStatus> {
  if (!isTauriRuntime()) {
    return { ...browserMock.imGatewayByProvider[providerId], providerId };
  }

  return invokeLogged<ImGatewayStatus>("load_im_gateway_status", { payload: { providerId } });
}

/** 保存飞书 appSecret；兼容旧调用，内部走通用 IM provider secret 命令。 */
export async function saveFeishuAppSecret(appSecret: string): Promise<FeishuCredentialStatus> {
  return saveImProviderSecret("feishu", appSecret);
}

/** 读取飞书 appSecret 是否已配置；兼容旧调用。 */
export async function loadFeishuCredentialStatus(): Promise<FeishuCredentialStatus> {
  return loadImProviderCredentialStatus("feishu");
}

/** 启动飞书长连接网关；兼容旧调用。 */
export async function startFeishuGateway(): Promise<FeishuGatewayStatus> {
  return startImGateway("feishu");
}

/** 停止飞书长连接网关；兼容旧调用。 */
export async function stopFeishuGateway(): Promise<FeishuGatewayStatus> {
  return stopImGateway("feishu");
}

/** 读取飞书长连接网关运行态；兼容旧调用。 */
export async function loadFeishuGatewayStatus(): Promise<FeishuGatewayStatus> {
  return loadImGatewayStatus("feishu");
}

function emptyLoginStatus(providerId: ImProviderId, message: string): ImLoginStatus {
  return {
    providerId,
    status: "idle",
    message,
  };
}

/** 启动 IM 扫码登录；浏览器开发态不连接真实接口。 */
export async function startImLogin(providerId: ImProviderId): Promise<ImLoginStatus> {
  if (!isTauriRuntime()) {
    return emptyLoginStatus(providerId, "浏览器开发态不能扫码登录，请在 Tauri 桌面端操作。");
  }

  return invokeLogged<ImLoginStatus>("start_im_login", { payload: { providerId } });
}

/** 读取 IM 扫码登录状态。 */
export async function loadImLoginStatus(providerId: ImProviderId): Promise<ImLoginStatus> {
  if (!isTauriRuntime()) {
    return emptyLoginStatus(providerId, "浏览器开发态未连接扫码登录。");
  }

  return invokeLogged<ImLoginStatus>("load_im_login_status", { payload: { providerId } });
}

/** 取消进行中的 IM 扫码登录。 */
export async function cancelImLogin(providerId: ImProviderId): Promise<ImLoginStatus> {
  if (!isTauriRuntime()) {
    return emptyLoginStatus(providerId, "浏览器开发态没有进行中的扫码登录。");
  }

  return invokeLogged<ImLoginStatus>("cancel_im_login", { payload: { providerId } });
}
