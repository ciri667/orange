import { listen } from "@tauri-apps/api/event";
import { invokeLogged, isTauriRuntime } from "./runtime";
import { browserMock, cloneUserSettings } from "../mock/browser";
import { mockSemanticAction, mockSemanticStatuses } from "../mock/semantic";
import type { KnowledgeBase, SemanticIndexAction, SemanticIndexStatus, SemanticSearchSettings, UserSettings } from "../types";

/** 仅判断用户选择的端点是否在本机；真正的地址与隐私校验在 Rust 中执行。 */
export function isRemoteSemanticEndpoint(settings: SemanticSearchSettings) {
  try {
    const host = new URL(settings.apiBase).hostname;
    return host !== "localhost" && host !== "[::1]" && !/^127\.\d+\.\d+\.\d+$/.test(host);
  } catch { return true; }
}

export async function saveSemanticSearchSettings(settings: SemanticSearchSettings, allowCloudUpload: boolean): Promise<UserSettings> {
  if (!isTauriRuntime()) {
    if (settings.enabled && isRemoteSemanticEndpoint(settings) && browserMock.userSettings.privacyPolicy === "local-only") throw new Error("仅本地隐私策略禁止云端语义索引。");
    browserMock.userSettings.semanticSearch = { ...settings, cloudConsentFingerprint: allowCloudUpload ? "browser-preview" : settings.cloudConsentFingerprint };
    return cloneUserSettings(browserMock.userSettings);
  }
  return invokeLogged("save_semantic_search_settings", { payload: { settings, allowCloudUpload } });
}

export async function loadSemanticIndexStatuses(knowledgeBases: KnowledgeBase[] = []): Promise<SemanticIndexStatus[]> {
  if (!isTauriRuntime()) return mockSemanticStatuses(knowledgeBases);
  return invokeLogged("load_semantic_index_statuses");
}

export async function controlSemanticIndex(knowledgeBaseId: string, action: SemanticIndexAction, allowCloudUpload = false): Promise<SemanticIndexStatus[]> {
  if (!isTauriRuntime()) return mockSemanticAction(knowledgeBaseId, action);
  return invokeLogged("control_semantic_index", { payload: { knowledgeBaseId, action, allowCloudUpload } });
}

export async function listenSemanticIndexProgress(onProgress: (states: SemanticIndexStatus[]) => void): Promise<() => void> {
  if (!isTauriRuntime()) return () => {};
  return listen<SemanticIndexStatus[]>("semantic-index-progress", (event) => onProgress(event.payload));
}

export async function saveSemanticApiKey(settings: SemanticSearchSettings, apiKey: string): Promise<{ configured: boolean }> {
  if (!isTauriRuntime()) throw new Error("浏览器预览不能保存密钥，请使用桌面端。");
  return invokeLogged("save_semantic_api_key", { payload: { settings, apiKey } });
}

export async function loadSemanticCredentialStatus(settings: SemanticSearchSettings): Promise<{ configured: boolean }> {
  if (!isTauriRuntime()) return { configured: false };
  return invokeLogged("load_semantic_credential_status", { settings });
}

export async function probeSemanticSearch(settings: SemanticSearchSettings): Promise<{ dimensions: number; message: string }> {
  if (!isTauriRuntime()) throw new Error("浏览器预览不连接向量服务，请在桌面端测试连接。");
  return invokeLogged("probe_semantic_search", { settings });
}
