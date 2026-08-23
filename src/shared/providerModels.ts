import type { LlmProviderConfig, LlmProviderModel } from "./types";

/** 手动模型增删改的结果：成功时返回更新后的 provider，失败时返回可展示的错误文案。 */
export type ProviderModelMutationResult =
  | { ok: true; provider: LlmProviderConfig }
  | { ok: false; error: string };

/** 构造一条用户手填的模型条目；未给显示名时用模型 ID。 */
export function createManualProviderModel(id: string, name: string, updatedAt: string): LlmProviderModel {
  return {
    id,
    name: name.trim() || id,
    enabled: true,
    source: "manual",
    updatedAt,
  };
}

/** 向 provider 追加一条手动模型；若当前默认模型还不在列表中，会一并补进去。 */
export function addManualProviderModel(
  provider: LlmProviderConfig,
  modelId: string,
  name: string,
  updatedAt: string,
): ProviderModelMutationResult {
  const id = modelId.trim();
  const displayName = name.trim() || id;

  if (!id) {
    return { ok: false, error: "模型 ID 不能为空。" };
  }

  if (provider.models.some((model) => model.id === id)) {
    return { ok: false, error: `模型「${id}」已存在。` };
  }

  const models = [...provider.models];
  const defaultId = provider.model.trim();

  // 创建时先手填了默认模型、但还没写入 models 时，补进列表以免被新条目挤掉。
  if (defaultId && defaultId !== id && !models.some((model) => model.id === defaultId)) {
    models.push(createManualProviderModel(defaultId, defaultId, updatedAt));
  }

  models.push(createManualProviderModel(id, displayName, updatedAt));

  return {
    ok: true,
    provider: {
      ...provider,
      model: defaultId || id,
      models,
      updatedAt,
    },
  };
}

/** 仅允许修改手动模型的 ID 和显示名；改到当前默认模型时同步 provider.model。 */
export function updateManualProviderModel(
  provider: LlmProviderConfig,
  originalId: string,
  nextId: string,
  nextName: string,
  updatedAt: string,
): ProviderModelMutationResult {
  const id = nextId.trim();
  const name = nextName.trim() || id;
  const target = provider.models.find((model) => model.id === originalId);

  if (!id) {
    return { ok: false, error: "模型 ID 不能为空。" };
  }

  if (!target) {
    return { ok: false, error: `未找到模型「${originalId}」。` };
  }

  if (target.source !== "manual") {
    return { ok: false, error: "发现的模型不能改 ID，请停用或重新获取列表。" };
  }

  if (id !== originalId && provider.models.some((model) => model.id === id)) {
    return { ok: false, error: `模型「${id}」已存在。` };
  }

  return {
    ok: true,
    provider: {
      ...provider,
      model: provider.model === originalId ? id : provider.model,
      models: provider.models.map((model) =>
        model.id === originalId ? { ...model, id, name, updatedAt } : model,
      ),
      updatedAt,
    },
  };
}

/** 删除手动模型；默认模型必须先换掉才能删，发现的模型不允许从本地列表移除。 */
export function removeManualProviderModel(
  provider: LlmProviderConfig,
  modelId: string,
  updatedAt: string,
): ProviderModelMutationResult {
  const target = provider.models.find((model) => model.id === modelId);

  if (!target) {
    return { ok: false, error: `未找到模型「${modelId}」。` };
  }

  if (provider.model === modelId) {
    return { ok: false, error: "默认模型不能删除，请先更换默认模型。" };
  }

  if (target.source !== "manual") {
    return { ok: false, error: "发现的模型不能删除，可停用或重新获取列表。" };
  }

  return {
    ok: true,
    provider: {
      ...provider,
      models: provider.models.filter((model) => model.id !== modelId),
      updatedAt,
    },
  };
}
