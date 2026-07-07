import type { AgentSession, LlmProviderConfig, LlmProviderModel, ModelConfig } from "./types";

/** 会话/本轮模型选择器统一使用的“跟随默认”占位值，不写入具体 providerId 或 modelId。 */
export const FOLLOW_DEFAULT_MODEL_SELECTION = "";

/** select value 中 providerId 和 modelId 的分隔符；两侧都先 encode，避免模型 ID 中的斜杠或冒号冲突。 */
const MODEL_SELECTION_SEPARATOR = "::";

/** 把 providerId 和 modelId 编码成 select 可用的稳定字符串。 */
export function encodeModelSelection(providerId: string, modelId: string) {
  return `${encodeURIComponent(providerId)}${MODEL_SELECTION_SEPARATOR}${encodeURIComponent(modelId)}`;
}

/** 从 select 值解析 providerId/modelId；空值表示跟随默认。 */
export function decodeModelSelection(selection: string) {
  if (!selection) {
    return { providerId: "", modelId: "" };
  }

  const [providerPart = "", modelPart = ""] = selection.split(MODEL_SELECTION_SEPARATOR);

  return {
    providerId: decodeURIComponent(providerPart),
    modelId: decodeURIComponent(modelPart),
  };
}

/** 返回 provider 当前可选择的启用模型；没有发现列表时使用旧的 provider.model 字段兜底。 */
export function getSelectableModels(provider: LlmProviderConfig): LlmProviderModel[] {
  const enabledModels = provider.models.filter((model) => model.enabled);

  if (enabledModels.length) {
    return enabledModels;
  }

  if (!provider.model.trim()) {
    return [];
  }

  return [
    {
      id: provider.model,
      name: provider.model,
      enabled: true,
      source: "manual",
      updatedAt: provider.updatedAt,
    },
  ];
}

/** 读取 provider 当前默认模型的展示名；缺失时退回 model ID。 */
export function getProviderModelLabel(provider: LlmProviderConfig, modelId = provider.model) {
  const model = provider.models.find((candidate) => candidate.id === modelId);

  return model?.name || modelId || "模型未配置";
}

/** 生成用户可读的 provider/model 标签，供摘要条和选择器选项共用。 */
export function getProviderModelSelectionLabel(provider: LlmProviderConfig, modelId = provider.model) {
  return `${provider.name} / ${getProviderModelLabel(provider, modelId)}`;
}

/** 解析会话默认模型的展示标签；无会话覆盖时回退全局默认 provider.model。 */
export function getSessionModelLabel(session: AgentSession, modelConfig: ModelConfig) {
  const sessionProvider = session.modelProviderId
    ? modelConfig.providers.find((provider) => provider.id === session.modelProviderId)
    : undefined;
  const defaultProvider = modelConfig.providers.find((provider) => provider.id === modelConfig.defaultProviderId);
  const provider = sessionProvider ?? defaultProvider;

  return provider ? getProviderModelSelectionLabel(provider, sessionProvider ? session.modelId || provider.model : provider.model) : "模型未配置";
}
