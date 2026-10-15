import type { KnowledgeBase, SemanticIndexAction, SemanticIndexStatus, SemanticSearchSettings } from "../types";

export const defaultSemanticSearchSettings: SemanticSearchSettings = {
  enabled: false, provider: "ollama", apiBase: "http://127.0.0.1:11434", model: "",
  requiresApiKey: false, minSimilarity: 0.35, cloudConsentFingerprint: null,
};

/** 浏览器仅模拟进度状态，不发送网络请求，不生成虚假的可用向量。 */
const states = new Map<string, SemanticIndexStatus>();
let previewKnowledgeBases: KnowledgeBase[] = [];
export function mockSemanticStatuses(knowledgeBases: KnowledgeBase[]): SemanticIndexStatus[] {
  previewKnowledgeBases = knowledgeBases;
  return knowledgeBases.map((kb) => states.get(kb.id) ?? {
    knowledgeBaseId: kb.id, enabled: false, paused: false, status: "disabled", indexedChunks: 0,
    pendingChunks: 0, failedChunks: 0, totalChunks: 0, dimensions: null, message: "浏览器预览不生成向量。",
  });
}

export function mockSemanticAction(id: string, action: SemanticIndexAction) {
  const knowledgeBases = previewKnowledgeBases;
  const status = mockSemanticStatuses(knowledgeBases).find((s) => s.knowledgeBaseId === id);
  if (!status) throw new Error("知识库不存在。");
  const enabled = action === "enable" || action === "rebuild" || (action !== "disable" && status.enabled);
  states.set(id, { ...status, enabled, paused: action === "pause", status: enabled ? action === "pause" ? "paused" : "pending" : "disabled" });
  return mockSemanticStatuses(knowledgeBases);
}
