import { useState } from "react";
import { ConfirmDialog } from "../shared/ConfirmDialog";
import { isRemoteSemanticEndpoint, saveSemanticSearchSettings } from "../shared/api/semantic";
import { logError, logInfo } from "../shared/logger";
import type { KnowledgeBase, UserSettings } from "../shared/types";
import { useSemanticIndexActions } from "../workspace/useSemanticIndexActions";
import { useSettingsAutosave } from "./useSettingsAutosave";

/** 通用设置自动保存；云端语义索引仍只在用户明确确认后通过专用 IPC 授权。 */
export function useUserSettingsAutosave(settings: UserSettings, onSave: (next: UserSettings) => Promise<UserSettings>) {
  const [cloudConsent, setCloudConsent] = useState<((allowed: boolean) => void) | null>(null);
  const autosave = useSettingsAutosave(settings, async (draft) => {
    const startedAt = performance.now();
    let next = { ...draft, writeConfirmationRequired: true };
    try {
      if (JSON.stringify(next.semanticSearch) !== JSON.stringify(settings.semanticSearch)) {
        const config = next.semanticSearch;
        const needsConsent = config.enabled && isRemoteSemanticEndpoint(config) && !config.cloudConsentFingerprint;
        const allowed = needsConsent ? await new Promise<boolean>((resolve) => setCloudConsent(() => resolve)) : false;
        if (needsConsent && !allowed) {
          // 取消授权只撤回语义配置，保留同一批次中其它已完成的设置修改。
          next = { ...next, semanticSearch: settings.semanticSearch };
        } else {
          const saved = await saveSemanticSearchSettings(config, allowed);
          next = { ...next, semanticSearch: saved.semanticSearch };
        }
      }
      const saved = await onSave(next);
      logInfo("设置页用户设置自动保存完成。", {
        category: "settings", event: "settings_save", status: "completed", durationMs: performance.now() - startedAt,
      });
      return saved;
    } catch (error) {
      logError("设置页用户设置自动保存失败。", {
        category: "settings", event: "settings_save", status: "failed", durationMs: performance.now() - startedAt, error,
      });
      throw error;
    }
  });

  function resolveCloudConsent(allowed: boolean) {
    setCloudConsent(null);
    cloudConsent?.(allowed);
  }

  return { ...autosave, needsCloudConsent: cloudConsent !== null, resolveCloudConsent };
}

/** 授权时展示受影响的知识库；仅在弹窗打开时读取索引状态。 */
export function SemanticCloudConsentDialog({ knowledgeBases, onResolve }: {
  knowledgeBases: KnowledgeBase[];
  onResolve: (allowed: boolean) => void;
}) {
  const index = useSemanticIndexActions(knowledgeBases);
  const enabledNames = knowledgeBases.filter((kb) => index.statuses.some((status) => status.knowledgeBaseId === kb.id && status.enabled)).map((kb) => kb.name);
  return <ConfirmDialog title="允许云端语义索引"
    message={`开启后，橘记会向此服务发送已开启知识库的文本片段，并在检索时发送查询。${enabledNames.length ? `目前已开启：${enabledNames.join("、")}，确认后会自动为这些知识库更新云端索引。` : "确认后可到知识库管理中选择需要索引的知识库。"}保存和重新扫描会自动更新索引，服务可能按使用量收费。知识库管理中可随时暂停或关闭。`}
    confirmLabel="允许并启用" onCancel={() => onResolve(false)} onConfirm={() => onResolve(true)} />;
}
