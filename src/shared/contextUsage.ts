import type { AgentSession, LlmProviderModel, ModelConfig } from "./types";

/** 会话当前模型窗口与最近一次有效占用。 */
export interface AgentContextMeter {
  windowTokens?: number;
  usedTokens?: number;
  percent?: number;
  modelId?: string;
  usageModelId?: string;
  recordedAt?: string;
  matchesCurrentModel: boolean;
  windowKnown: boolean;
  usageKnown: boolean;
}

/** 把 token 数收成短标签，供摘要条和浮层共用。 */
export function formatTokenCount(tokens: number) {
  if (tokens >= 1_000_000) {
    const millions = tokens / 1_000_000;
    return `${millions >= 10 ? Math.round(millions) : trimTokenDecimal(millions)}M`;
  }

  if (tokens >= 1_000) {
    const thousands = tokens / 1_000;
    return `${tokens >= 10_000 ? Math.round(thousands) : trimTokenDecimal(thousands)}k`;
  }

  return tokens.toLocaleString();
}

function trimTokenDecimal(value: number) {
  return value.toFixed(1).replace(/\.0$/, "");
}

/** 解析会话当前生效的模型；未指定时跟随全局默认 provider。 */
export function resolveSessionModel(session: AgentSession, modelConfig: ModelConfig): {
  modelId?: string;
  model?: LlmProviderModel;
} {
  const provider = session.modelProviderId
    ? modelConfig.providers.find((item) => item.id === session.modelProviderId)
    : modelConfig.providers.find((item) => item.id === modelConfig.defaultProviderId);
  const modelId = session.modelId || provider?.model;
  const model = modelId ? provider?.models.find((item) => item.id === modelId) : undefined;

  return { modelId, model };
}

/** 组合最近一次 usage 与当前模型目录中的窗口大小。 */
export function resolveContextMeter(session: AgentSession, modelConfig: ModelConfig): AgentContextMeter {
  const { modelId, model } = resolveSessionModel(session, modelConfig);
  const usage = session.contextUsage;
  const usedTokens = usage && (usage.promptTokens > 0 || usage.totalTokens > 0)
    ? usage.promptTokens || usage.totalTokens
    : undefined;
  const catalogWindow = model?.contextLength && model.contextLength >= 1024 ? model.contextLength : undefined;
  const usageWindow = usage?.contextLength && usage.contextLength >= 1024 ? usage.contextLength : undefined;
  const windowTokens = catalogWindow ?? usageWindow;
  const percent = usedTokens != null && windowTokens ? Math.round((usedTokens / windowTokens) * 100) : undefined;

  return {
    windowTokens,
    usedTokens,
    percent,
    modelId,
    usageModelId: usage?.modelId,
    recordedAt: usage?.recordedAt,
    matchesCurrentModel: !usage || !modelId || usage.modelId === modelId,
    windowKnown: Boolean(windowTokens),
    usageKnown: usedTokens != null,
  };
}

/** 摘要条短标签；窗口和占用都未知时不展示。 */
export function formatContextMeterChip(meter: AgentContextMeter) {
  if (meter.usageKnown && meter.windowKnown) {
    return `${formatTokenCount(meter.usedTokens ?? 0)} / ${formatTokenCount(meter.windowTokens ?? 0)}`;
  }

  if (meter.usageKnown) {
    return `${formatTokenCount(meter.usedTokens ?? 0)} tokens`;
  }

  if (meter.windowKnown) {
    return `窗口 ${formatTokenCount(meter.windowTokens ?? 0)}`;
  }

  return null;
}

/** 浮层里的窗口文案。 */
export function formatContextWindowLabel(meter: AgentContextMeter) {
  if (!meter.windowKnown) {
    return "未知";
  }

  return `${(meter.windowTokens ?? 0).toLocaleString()} tokens`;
}

/** 浮层里的占用文案。 */
export function formatContextUsageLabel(meter: AgentContextMeter) {
  if (!meter.usageKnown) {
    return "尚未计量";
  }

  const used = (meter.usedTokens ?? 0).toLocaleString();

  if (meter.windowKnown) {
    const percentLabel = meter.percent == null
      ? ""
      : meter.percent === 0 && (meter.usedTokens ?? 0) > 0
        ? " · <1%"
        : ` · ${meter.percent}%`;
    return `${used} / ${(meter.windowTokens ?? 0).toLocaleString()}${percentLabel}`;
  }

  return `${used} tokens`;
}
