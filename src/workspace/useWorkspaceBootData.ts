import { useEffect, useState } from "react";
import { logWarn } from "../shared/logger";
import {
  loadAgentSkills,
  loadAppEventLogs,
  loadImGatewayStatus,
  loadImProviderCredentialStatus,
  loadImSettings,
  loadKnowledgeBaseMemories,
  loadLlmProviderTemplates,
  loadModelApiKeyStatuses,
  loadRequestAuditLogs,
  loadUserSettings,
  loadWorkspaceState,
} from "../shared/tauriApi";
import type {
  AgentSkill,
  AppEventLog,
  FeishuCredentialStatus,
  FeishuGatewayStatus,
  ImIntegrationSettings,
  KnowledgeBaseMemory,
  ModelApiKeyStatus,
  ProviderTemplate,
  RequestAuditLog,
  UserSettings,
  WorkspaceEditorState,
  WorkspaceSnapshot,
} from "../shared/types";

/** 将未知异常统一转换为可展示文案，避免启动错误页渲染空对象。 */
function formatBootErrorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

/** 将已由后端校验的编辑器会话投射到领域快照；无有效焦点时显式保持编辑区空白。 */
function applyEditorStateToSnapshot(snapshot: WorkspaceSnapshot, editorState: WorkspaceEditorState): WorkspaceSnapshot {
  const activeNote = editorState.activeTab?.kind === "note"
    ? snapshot.notes.find((note) => note.id === editorState.activeTab?.id)
    : undefined;
  const activeDocument = editorState.activeTab?.kind === "document"
    ? snapshot.documents.find((document) => document.id === editorState.activeTab?.id)
    : undefined;
  const activeKnowledgeBaseId = activeNote?.knowledgeBaseId ?? activeDocument?.knowledgeBaseId ??
    (snapshot.knowledgeBases.some((knowledgeBase) => knowledgeBase.id === editorState.activeKnowledgeBaseId)
      ? editorState.activeKnowledgeBaseId
      : snapshot.activeKnowledgeBaseId);
  const activeSessionId = snapshot.sessions.find((session) => session.knowledgeBaseIds.includes(activeKnowledgeBaseId))?.id ?? "";

  return {
    ...snapshot,
    activeKnowledgeBaseId,
    activeNoteId: activeNote?.id ?? "",
    activeDocumentId: activeDocument?.id ?? "",
    activeSessionId,
  };
}

/** 启动数据 hook 的外部回调，只传递脱敏 notice 和快照初始化信号。 */
interface WorkspaceBootDataOptions {
  onSnapshotInitialized: (snapshot: WorkspaceSnapshot) => void;
  onEditorStateInitialized: (editorState: WorkspaceEditorState) => void;
  onNoticeChange: (notice: string) => void;
}

/** 集中加载工作台启动数据和诊断日志，保持根组件只负责业务编排和 UI 分发。 */
export function useWorkspaceBootData({ onSnapshotInitialized, onEditorStateInitialized, onNoticeChange }: WorkspaceBootDataOptions) {
  /** 工作台完整快照，是知识库、文件树、会话和 diff 状态的单一前端来源。 */
  const [snapshot, setSnapshot] = useState<WorkspaceSnapshot | null>(null);
  /** 用户模型和隐私设置；启动失败时保持 null 进入错误页。 */
  const [userSettings, setUserSettings] = useState<UserSettings | null>(null);
  /** 即时通讯设置由后端持久化；敏感凭证状态单独读取。 */
  const [imSettings, setImSettings] = useState<ImIntegrationSettings | null>(null);
  /** Agent skills 列表由后端合并内置与用户自建定义，前端只保存展示状态。 */
  const [agentSkills, setAgentSkills] = useState<AgentSkill[]>([]);
  /** 模型密钥状态按 providerId 隔离，只保存是否可读，不包含明文 API key。 */
  const [modelApiKeyStatuses, setModelApiKeyStatuses] = useState<ModelApiKeyStatus[]>([]);
  /** 飞书 appSecret 状态只说明是否可读取，不包含明文 secret。 */
  const [feishuCredentialStatus, setFeishuCredentialStatus] = useState<FeishuCredentialStatus | null>(null);
  /** 飞书长连接网关运行态，用于设置页手动启停和错误展示。 */
  const [feishuGatewayStatus, setFeishuGatewayStatus] = useState<FeishuGatewayStatus | null>(null);
  /** 内置 LLM Provider 模板，驱动设置页“新增 Provider”入口。 */
  const [providerTemplates, setProviderTemplates] = useState<ProviderTemplate[]>([]);
  /** 首屏初始化是否仍在进行，用于区分加载中和加载失败。 */
  const [isBooting, setIsBooting] = useState(true);
  /** 首屏初始化失败原因，失败后展示重试入口而不是停留在 loading。 */
  const [bootError, setBootError] = useState("");
  /** 最近请求审计日志，设置页按需展示模型请求和工具边界。 */
  const [auditLogs, setAuditLogs] = useState<RequestAuditLog[]>([]);
  /** 用户可读运行事件日志，只在设置页展示，不阻塞首屏工作台。 */
  const [appEventLogs, setAppEventLogs] = useState<AppEventLog[]>([]);
  /** 跨会话记忆集合，每知识库一份；默认关闭，用户在设置页手动开启后注入 Runtime。 */
  const [knowledgeBaseMemories, setKnowledgeBaseMemories] = useState<KnowledgeBaseMemory[]>([]);

  useEffect(() => {
    let isMounted = true;

    void loadInitialData(() => isMounted);

    return () => {
      isMounted = false;
    };
  }, []);

  /** 加载首屏必需数据；诊断日志失败不阻断进入工作台。 */
  async function loadInitialData(shouldCommit: () => boolean = () => true) {
    setIsBooting(true);
    setBootError("");
    onNoticeChange("");

    try {
      // 工作台快照和用户设置是首屏必需数据，必须同时成功后才能进入主界面。
      const [
        nextSnapshot,
        nextUserSettings,
        nextImSettings,
        nextAgentSkills,
        nextModelApiKeyStatuses,
        nextProviderTemplates,
        nextFeishuCredentialStatus,
        nextFeishuGatewayStatus,
        nextKnowledgeBaseMemories,
      ] = await Promise.all([
        loadWorkspaceState(),
        loadUserSettings(),
        loadImSettings(),
        loadAgentSkills(),
        loadModelApiKeyStatuses().catch((error) => {
          logWarn("读取模型密钥状态失败。", { category: "settings", event: "model_api_key_status_load", status: "failed", error });

          return [] as ModelApiKeyStatus[];
        }),
        loadLlmProviderTemplates().catch((error) => {
          logWarn("读取 Provider 模板失败。", { category: "settings", event: "provider_templates_load", status: "failed", error });

          return [] as ProviderTemplate[];
        }),
        loadImProviderCredentialStatus("feishu").catch((error) => {
          logWarn("读取 IM provider 凭证状态失败。", {
            category: "im",
            event: "im_provider_credential_status_load",
            status: "failed",
            metadata: { providerId: "feishu" },
            error,
          });

          return null;
        }),
        loadImGatewayStatus("feishu").catch((error) => {
          logWarn("读取 IM 网关状态失败。", {
            category: "im",
            event: "im_gateway_status_load",
            status: "failed",
            metadata: { providerId: "feishu" },
            error,
          });

          return null;
        }),
        loadKnowledgeBaseMemories().catch((error) => {
          logWarn("读取跨会话记忆失败。", { category: "settings", event: "kb_memory_load", status: "failed", error });

          return [] as KnowledgeBaseMemory[];
        }),
      ]);

      if (!shouldCommit()) {
        return;
      }

      const restoredSnapshot = applyEditorStateToSnapshot(nextSnapshot.snapshot, nextSnapshot.editorState);

      setSnapshot(restoredSnapshot);
      setUserSettings(nextUserSettings);
      setImSettings(nextImSettings);
      setAgentSkills(nextAgentSkills);
      setModelApiKeyStatuses(nextModelApiKeyStatuses);
      setProviderTemplates(nextProviderTemplates);
      setFeishuCredentialStatus(nextFeishuCredentialStatus);
      setFeishuGatewayStatus(nextFeishuGatewayStatus);
      setKnowledgeBaseMemories(nextKnowledgeBaseMemories);
      onSnapshotInitialized(restoredSnapshot);
      onEditorStateInitialized(nextSnapshot.editorState);
      setIsBooting(false);

      void loadInitialDiagnosticLogs(shouldCommit);
    } catch (error) {
      if (shouldCommit()) {
        setSnapshot(null);
        setUserSettings(null);
        setImSettings(null);
        setAgentSkills([]);
        setFeishuCredentialStatus(null);
        setFeishuGatewayStatus(null);
        setAuditLogs([]);
        setAppEventLogs([]);
        setBootError(formatBootErrorMessage(error));
      }
    } finally {
      if (shouldCommit()) {
        setIsBooting(false);
      }
    }
  }

  /** 后台加载非首屏必需的诊断日志，失败时降级为空列表并提示用户。 */
  async function loadInitialDiagnosticLogs(shouldCommit: () => boolean = () => true) {
    try {
      const [nextAuditLogs, nextAppEventLogs] = await Promise.all([loadRequestAuditLogs(), loadAppEventLogs()]);

      if (!shouldCommit()) {
        return;
      }

      setAuditLogs(nextAuditLogs);
      setAppEventLogs(nextAppEventLogs);
    } catch (error) {
      if (shouldCommit()) {
        setAuditLogs([]);
        setAppEventLogs([]);
        onNoticeChange(`诊断日志加载失败：${formatBootErrorMessage(error)}`);
      }
    }
  }

  return {
    snapshot,
    setSnapshot,
    userSettings,
    setUserSettings,
    imSettings,
    setImSettings,
    agentSkills,
    setAgentSkills,
    modelApiKeyStatuses,
    setModelApiKeyStatuses,
    feishuCredentialStatus,
    setFeishuCredentialStatus,
    feishuGatewayStatus,
    setFeishuGatewayStatus,
    providerTemplates,
    isBooting,
    bootError,
    knowledgeBaseMemories,
    setKnowledgeBaseMemories,
    auditLogs,
    setAuditLogs,
    appEventLogs,
    setAppEventLogs,
    loadInitialData,
    loadInitialDiagnosticLogs,
  };
}
