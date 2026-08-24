import { invokeLogged, isTauriRuntime } from "./runtime";
import { formatLocalDateTime } from "../id";
import { browserMock, cloneImSettings, getFeishuProvider } from "../mock/browser";
import {
  FeishuCredentialStatus,
  FeishuGatewayStatus,
  ImGatewayStatus,
  ImIntegrationSettings,
  ImProviderCredentialStatus,
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
    const feishu = getFeishuProvider(settings);

    browserMock.imSettings = cloneImSettings(settings);
    browserMock.feishuGatewayStatus = {
      ...browserMock.feishuGatewayStatus,
      domain: feishu.config.domain,
      appIdConfigured: Boolean(feishu.config.appId.trim()),
    };

    return loadImSettings();
  }

  return invokeLogged<ImIntegrationSettings>("save_im_settings", { payload: { settings } });
}

/** 保存 IM provider secret；桌面端写入系统安全存储，浏览器态只返回不可用说明。 */
export async function saveImProviderSecret(providerId: "feishu", secret: string): Promise<ImProviderCredentialStatus> {
  if (!isTauriRuntime()) {
    throw new Error("浏览器开发态不能保存 IM provider secret，请在 Tauri 桌面端配置。");
  }

  return invokeLogged<ImProviderCredentialStatus>("save_im_provider_secret", { payload: { providerId, secret } });
}

/** 读取 IM provider secret 是否已配置；不会返回明文 secret。 */
export async function loadImProviderCredentialStatus(providerId: "feishu"): Promise<ImProviderCredentialStatus> {
  if (!isTauriRuntime()) {
    const feishu = getFeishuProvider(browserMock.imSettings);

    return {
      providerId,
      keyReference: feishu.config.secretKeyReference,
      configured: false,
      message: "浏览器开发态未连接系统安全存储。",
    };
  }

  return invokeLogged<ImProviderCredentialStatus>("load_im_provider_credential_status", { payload: { providerId } });
}

/** 启动 IM provider 长连接网关；浏览器态只返回不可用状态。 */
export async function startImGateway(providerId: "feishu"): Promise<ImGatewayStatus> {
  if (!isTauriRuntime()) {
    browserMock.feishuGatewayStatus = {
      ...browserMock.feishuGatewayStatus,
      providerId,
      running: false,
      connected: false,
      lastError: "浏览器开发态不能启动 IM 长连接网关。",
    };

    return browserMock.feishuGatewayStatus;
  }

  return invokeLogged<ImGatewayStatus>("start_im_gateway", { payload: { providerId } });
}

/** 停止 IM provider 长连接网关；不会清空设置和凭证。 */
export async function stopImGateway(providerId: "feishu"): Promise<ImGatewayStatus> {
  if (!isTauriRuntime()) {
    browserMock.feishuGatewayStatus = {
      ...browserMock.feishuGatewayStatus,
      providerId,
      running: false,
      connected: false,
      lastStoppedAt: formatLocalDateTime(),
    };

    return browserMock.feishuGatewayStatus;
  }

  return invokeLogged<ImGatewayStatus>("stop_im_gateway", { payload: { providerId } });
}

/** 读取 IM provider 长连接网关运行态。 */
export async function loadImGatewayStatus(providerId: "feishu"): Promise<ImGatewayStatus> {
  if (!isTauriRuntime()) {
    return { ...browserMock.feishuGatewayStatus, providerId };
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
