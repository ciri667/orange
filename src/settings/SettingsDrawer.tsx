import { Check, KeyRound, Plus, RotateCw, Save, Trash2, X } from "lucide-react";
import { useEffect, useState } from "react";
import type { KnowledgeBase, ModelApiKeyStatus, RequestAuditLog, UserSettings } from "../shared/types";

/** 设置抽屉，展示多知识库管理、模型策略和写入权限。 */
export function SettingsDrawer({
  knowledgeBases,
  activeKnowledgeBaseId,
  settings,
  modelApiKeyStatus,
  auditLogs,
  isBusy,
  onSelectKnowledgeBase,
  onAddKnowledgeBase,
  onRescanKnowledgeBase,
  onRemoveKnowledgeBase,
  onSaveSettings,
  onSaveApiKey,
  onRefreshAuditLogs,
  onClose,
}: {
  knowledgeBases: KnowledgeBase[];
  activeKnowledgeBaseId: string;
  settings: UserSettings;
  modelApiKeyStatus: ModelApiKeyStatus | null;
  auditLogs: RequestAuditLog[];
  isBusy: boolean;
  onSelectKnowledgeBase: (knowledgeBaseId: string) => void;
  onAddKnowledgeBase: () => void;
  onRescanKnowledgeBase: (knowledgeBaseId: string) => void;
  onRemoveKnowledgeBase: (knowledgeBaseId: string) => void;
  onSaveSettings: (settings: UserSettings) => Promise<void> | void;
  onSaveApiKey: (apiKey: string) => Promise<void> | void;
  onRefreshAuditLogs: () => Promise<void> | void;
  onClose: () => void;
}) {
  /** 模型设置表单草稿，用户保存前不影响正在运行的 Agent Runtime。 */
  const [settingsDraft, setSettingsDraft] = useState<UserSettings>(settings);
  /** API key 草稿只保留在输入框中，保存后由外层写入系统安全存储。 */
  const [apiKeyDraft, setApiKeyDraft] = useState("");

  useEffect(() => {
    setSettingsDraft(settings);
  }, [settings]);

  /** 保存模型与隐私设置；写入确认策略在首版始终保持开启。 */
  async function handleSaveSettings() {
    await onSaveSettings({
      ...settingsDraft,
      writeConfirmationRequired: true,
    });
  }

  /** 保存 BYOK key 后清空输入框，避免密钥继续留在可见表单状态里。 */
  async function handleSaveApiKey() {
    try {
      await onSaveApiKey(apiKeyDraft);
      setApiKeyDraft("");
    } catch {
      // 外层已经把失败原因写入全局 notice；这里保留输入，方便用户修正后重试。
    }
  }

  /** 更新模型配置草稿中的单个字段，保持其他设置不变。 */
  function updateModelConfig(field: keyof UserSettings["modelConfig"], value: string | boolean) {
    setSettingsDraft((currentSettings) => ({
      ...currentSettings,
      modelConfig: {
        ...currentSettings.modelConfig,
        [field]: value,
      },
    }));
  }

  return (
    <div className="settings-backdrop" role="presentation">
      <aside className="settings-drawer" aria-label="设置">
        <header className="settings-header">
          <div>
            <p className="section-label">Settings</p>
            <h2>知识库与 Agent 设置</h2>
          </div>
          <button className="icon-button" type="button" title="关闭设置" onClick={onClose}>
            <X size={18} />
          </button>
        </header>

        <div className="settings-section">
          <div className="settings-section-title">
            <h3>知识库管理</h3>
            <button className="ghost-button" type="button" onClick={onAddKnowledgeBase}>
              <Plus size={15} />
              添加知识库
            </button>
          </div>
          <div className="settings-kb-list">
            {knowledgeBases.map((knowledgeBase) => (
              <article className="settings-kb-card" key={knowledgeBase.id}>
                <div>
                  <div className="kb-card-title">
                    <strong>{knowledgeBase.name}</strong>
                    <span>{knowledgeBase.status === "error" ? "目录失效" : knowledgeBase.semanticIndexEnabled ? "本地向量" : "FTS5"}</span>
                    {knowledgeBase.id === activeKnowledgeBaseId && <span>当前激活</span>}
                  </div>
                  <p>{knowledgeBase.description}</p>
                  <code>{knowledgeBase.path}</code>
                  <ScanReportDetails knowledgeBase={knowledgeBase} />
                </div>
                <div className="setting-actions">
                  <button type="button" onClick={() => onSelectKnowledgeBase(knowledgeBase.id)} disabled={isBusy}>
                    激活
                  </button>
                  <button type="button" onClick={() => onRescanKnowledgeBase(knowledgeBase.id)} disabled={isBusy}>
                    <RotateCw size={13} />
                    重新扫描
                  </button>
                  <button
                    className="danger-action"
                    type="button"
                    onClick={() => onRemoveKnowledgeBase(knowledgeBase.id)}
                    disabled={isBusy}
                  >
                    <Trash2 size={13} />
                    移除授权
                  </button>
                </div>
              </article>
            ))}
          </div>
        </div>

        <div className="settings-section">
          <div className="settings-section-title">
            <h3>模型与工具</h3>
            <button className="primary-button compact" type="button" onClick={handleSaveSettings} disabled={isBusy}>
              <Save size={14} />
              保存
            </button>
          </div>
          <div className="settings-grid">
            <label className="toggle-row">
              <input
                checked={settingsDraft.modelConfig.enabled}
                onChange={(event) => updateModelConfig("enabled", event.target.checked)}
                type="checkbox"
              />
              <span>启用云端模型</span>
            </label>
            <label>
              <span>Provider</span>
              <select value={settingsDraft.modelConfig.provider} disabled>
                <option value="openai-compatible">OpenAI-compatible</option>
              </select>
            </label>
            <label>
              <span>隐私策略</span>
              <select
                value={settingsDraft.privacyPolicy}
                onChange={(event) =>
                  setSettingsDraft((currentSettings) => ({
                    ...currentSettings,
                    privacyPolicy: event.target.value as UserSettings["privacyPolicy"],
                  }))
                }
              >
                <option value="allow-selected-scope">允许已选 scope</option>
                <option value="local-only">仅本地规则 Agent</option>
              </select>
            </label>
            <label>
              <span>API base</span>
              <input
                value={settingsDraft.modelConfig.apiBase}
                onChange={(event) => updateModelConfig("apiBase", event.target.value)}
                placeholder="https://api.openai.com/v1"
              />
            </label>
            <label>
              <span>模型</span>
              <input
                value={settingsDraft.modelConfig.model}
                onChange={(event) => updateModelConfig("model", event.target.value)}
                placeholder="gpt-4o-mini"
              />
            </label>
            <label>
              <span>Key reference</span>
              <input value={settingsDraft.modelConfig.keyReference} readOnly />
            </label>
            <label>
              <span>API key</span>
              <div className="key-save-row">
                <input
                  value={apiKeyDraft}
                  onChange={(event) => setApiKeyDraft(event.target.value)}
                  placeholder="sk-..."
                  type="password"
                />
                <button type="button" onClick={handleSaveApiKey} disabled={isBusy || !apiKeyDraft.trim()}>
                  <KeyRound size={13} />
                  保存密钥
                </button>
              </div>
              <div className={`key-status ${modelApiKeyStatus?.configured ? "verified" : "missing"}`}>
                <KeyRound size={13} />
                <span>{modelApiKeyStatus?.message ?? "尚未读取模型密钥状态。"}</span>
              </div>
            </label>
          </div>
        </div>

        <div className="settings-section">
          <h3>写入策略</h3>
          <div className="policy-row">
            <Check size={16} />
            <span>Agent 写入工具只能生成 diff；用户确认后才执行路径校验、hash 校验和原子写入。</span>
          </div>
        </div>

        <div className="settings-section">
          <div className="settings-section-title">
            <h3>请求审计</h3>
            <button className="ghost-button" type="button" onClick={onRefreshAuditLogs} disabled={isBusy}>
              <RotateCw size={14} />
              刷新
            </button>
          </div>
          <div className="audit-list">
            {auditLogs.length ? (
              auditLogs.map((log) => <AuditLogCard key={log.id} log={log} />)
            ) : (
              <p>暂无审计记录。</p>
            )}
          </div>
        </div>
      </aside>
    </div>
  );
}

/** 展示知识库最近扫描报告，便于定位空目录、坏文件和被跳过的大目录。 */
function ScanReportDetails({ knowledgeBase }: { knowledgeBase: KnowledgeBase }) {
  const report = knowledgeBase.scanReport;

  if (!report) {
    return null;
  }

  return (
    <div className="scan-report">
      <span>
        扫描 {report.scannedFileCount} 篇，失败 {report.failedFileCount} 个
      </span>
      {report.skippedDirectories.length > 0 && <span>跳过：{report.skippedDirectories.slice(0, 4).join(" / ")}</span>}
      {report.errors.length > 0 && <span className="scan-report-error">{report.errors[0]}</span>}
    </div>
  );
}

/** 单条审计日志卡片，展示请求类型、scope 摘要和工具调用摘要。 */
function AuditLogCard({ log }: { log: RequestAuditLog }) {
  return (
    <article className="audit-card">
      <div className="audit-card-header">
        <strong>{formatAuditKind(log.kind)}</strong>
        <span>{log.createdAt}</span>
      </div>
      <p>{log.scopeSummary}</p>
      <p>{log.contentSummary}</p>
      <code>{log.toolSummary}</code>
    </article>
  );
}

/** 把后端审计类型转成简短中文标签。 */
function formatAuditKind(kind: string) {
  const labels: Record<string, string> = {
    model_turn: "模型请求",
    model_error_turn: "模型失败",
    local_rule_turn: "本地规则",
    browser_mock_turn: "浏览器模拟",
  };

  return labels[kind] ?? kind;
}
