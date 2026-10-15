import { useCallback, useEffect, useRef, useState } from "react";
import { controlSemanticIndex, listenSemanticIndexProgress, loadSemanticIndexStatuses } from "../shared/api/semantic";
import type { KnowledgeBase, SemanticIndexAction, SemanticIndexStatus } from "../shared/types";

/** 语义索引状态单独加载，进度只更新当前设置界面，不持有或覆盖工作台快照。 */
export function useSemanticIndexActions(knowledgeBases: KnowledgeBase[]) {
  const [statuses, setStatuses] = useState<SemanticIndexStatus[]>([]);
  const [busyId, setBusyId] = useState("");
  const [error, setError] = useState("");
  const revision = useRef(0);
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    // 先订阅再读取，若事件先到，避免较旧的查询响应覆盖它。
    void listenSemanticIndexProgress((next) => {
      revision.current += 1;
      if (!disposed) setStatuses(next);
    }).then(async (stop) => {
      if (disposed) { stop(); return; }
      unlisten = stop;
      const before = revision.current;
      const next = await loadSemanticIndexStatuses(knowledgeBases);
      if (!disposed && revision.current === before) setStatuses(next);
    }).catch((cause: unknown) => { if (!disposed) setError(String(cause)); });
    return () => { disposed = true; unlisten?.(); };
  }, [knowledgeBases.map((kb) => kb.id).join("\n")]);

  const act = useCallback(async (id: string, action: SemanticIndexAction, cloudConsent = false) => {
    setBusyId(id); setError("");
    const before = revision.current;
    try { const next = await controlSemanticIndex(id, action, cloudConsent); if (before === revision.current) setStatuses(next); }
    catch (cause) { setError(String(cause)); }
    finally { setBusyId(""); }
  }, []);
  return { statuses, busyId, error, act };
}
