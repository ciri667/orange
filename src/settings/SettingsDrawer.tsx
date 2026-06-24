import { Check, FolderOpen, KeyRound, Plus, RotateCw, Save, Sparkles, Trash2, X } from "lucide-react";
import { useEffect, useState } from "react";
import type {
  AgentSkill,
  AppEventLog,
  AppEventLogCategory,
  AppEventLogLevel,
  KnowledgeBase,
  ModelApiKeyStatus,
  RequestAuditLog,
  UserSettings,
} from "../shared/types";
import { SkillsModal } from "./SkillsModal";

/** 设置抽屉，展示多知识库管理、模型策略和写入权限。 */
export function SettingsDrawer({
  knowledgeBases,
  activeKnowledgeBaseId,
  settings,
  skills,
  modelApiKeyStatus,
  auditLogs,
  appEventLogs,
  isBusy,
  onSelectKnowledgeBase,
  onAddKnowledgeBase,
  onRescanKnowledgeBase,
  onRemoveKnowledgeBase,
  onSaveSettings,
  onSaveSkill,
  onToggleSkill,
  onDeleteSkill,
  onOpenUserSkillsFolder,
  onSaveApiKey,
  onRefreshAuditLogs,
  onRefreshAppEventLogs,
  onClearAppEventLogs,
  onOpenAppLogFolder,
  onClose,
}: {
  knowledgeBases: KnowledgeBase[];
  activeKnowledgeBaseId: string;
  settings: UserSettings;
  skills: AgentSkill[];
  modelApiKeyStatus: ModelApiKeyStatus | null;
  auditLogs: RequestAuditLog[];
  appEventLogs: AppEventLog[];
  isBusy: boolean;
  onSelectKnowledgeBase: (knowledgeBaseId: string) => void;
  onAddKnowledgeBase: () => void;
  onRescanKnowledgeBase: (knowledgeBaseId: string) => void;
  onRemoveKnowledgeBase: (knowledgeBaseId: string) => void;
  onSaveSettings: (settings: UserSettings) => Promise<void> | void;
  onSaveSkill: (skill: AgentSkill) => Promise<AgentSkill | void> | AgentSkill | void;
  onToggleSkill: (skillId: string, enabled: boolean, allowAutoInvoke?: boolean) => Promise<void> | void;
  onDeleteSkill: (skillId: string) => Promise<void> | void;
  onOpenUserSkillsFolder: () => Promise<void> | void;
  onSaveApiKey: (apiKey: string) => Promise<void> | void;
  onRefreshAuditLogs: () => Promise<void> | void;
  onRefreshAppEventLogs: (filters?: { level?: AppEventLogLevel | ""; category?: AppEventLogCategory | "" }) => Promise<void> | void;
  onClearAppEventLogs: (filters?: { level?: AppEventLogLevel | ""; category?: AppEventLogCategory | "" }) => Promise<void> | void;
  onOpenAppLogFolder: () => Promise<void> | void;
  onClose: () => void;
}) {
  /** 模型设置表单草稿，用户保存前不影响正在运行的 Agent Runtime。 */
  const [settingsDraft, setSettingsDraft] = useState<UserSettings>(settings);
  /** API key 草稿只保留在输入框中，保存后由外层写入系统安全存储。 */
  const [apiKeyDraft, setApiKeyDraft] = useState("");
  /** Skills 管理弹窗状态，避免设置抽屉内一次性铺开完整管理页。 */
  const [isSkillsModalOpen, setIsSkillsModalOpen] = useState(false);
  /** 应用事件日志级别筛选，空字符串表示不过滤。 */
  const [eventLogLevel, setEventLogLevel] = useState<AppEventLogLevel | "">("");
  /** 应用事件日志分类筛选，空字符串表示不过滤。 */
  const [eventLogCategory, setEventLogCategory] = useState<AppEventLogCategory | "">("");
  /** 已启用 skill 数量，用于设置摘要快速说明能力状态。 */
  const enabledSkillCount = skills.filter((skill) => skill.enabled).length;
  /** 允许自动触发的已启用 skill 数量，用于提示自动匹配覆盖范围。 */
  const autoSkillCount = skills.filter((skill) => skill.enabled && skill.allowAutoInvoke).length;
  /** 文件式 skill 数量用于确认用户目录扫描是否已生效。 */
  const fileSkillCount = skills.filter((skill) => skill.source === "file").length;

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

  /** 返回当前运行日志筛选条件，供刷新和清空后重载列表复用。 */
  function currentEventLogFilters() {
    return {
      level: eventLogLevel,
      category: eventLogCategory,
    };
  }

  /** 切换运行日志级别筛选，并立即向后端请求对应列表。 */
  function handleEventLogLevelChange(level: AppEventLogLevel | "") {
    setEventLogLevel(level);
    void onRefreshAppEventLogs({ level, category: eventLogCategory });
  }

  /** 切换运行日志分类筛选，并立即向后端请求对应列表。 */
  function handleEventLogCategoryChange(category: AppEventLogCategory | "") {
    setEventLogCategory(category);
    void onRefreshAppEventLogs({ level: eventLogLevel, category });
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
          <div className="settings-section-title">
            <h3>Skills 能力</h3>
            <button className="ghost-button" type="button" onClick={() => setIsSkillsModalOpen(true)}>
              <Sparkles size={14} />
              管理 Skills
            </button>
          </div>
          <div className="skills-summary">
            <div>
              <span>启用</span>
              <strong>
                {enabledSkillCount} / {skills.length}
              </strong>
            </div>
            <div>
              <span>自动触发</span>
              <strong>{settings.skillSettings.activationMode === "auto" ? `${autoSkillCount} 个` : "已关闭"}</strong>
            </div>
            <div>
              <span>文件 Skills</span>
              <strong>{fileSkillCount} 个</strong>
            </div>
          </div>
          <label className="toggle-row">
            <input
              checked={settingsDraft.skillSettings.activationMode === "auto"}
              onChange={(event) =>
                setSettingsDraft((currentSettings) => ({
                  ...currentSettings,
                  skillSettings: {
                    activationMode: event.target.checked ? "auto" : "manual",
                  },
                }))
              }
              type="checkbox"
            />
            <span>允许未显式选择时自动匹配 skill</span>
          </label>
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
            <h3>运行日志与诊断</h3>
            <div className="settings-title-actions">
              <button className="ghost-button" type="button" onClick={onOpenAppLogFolder} disabled={isBusy}>
                <FolderOpen size={14} />
                文件日志
              </button>
              <button className="ghost-button" type="button" onClick={() => onRefreshAppEventLogs(currentEventLogFilters())} disabled={isBusy}>
                <RotateCw size={14} />
                刷新
              </button>
              <button className="ghost-button danger-action" type="button" onClick={() => onClearAppEventLogs(currentEventLogFilters())} disabled={isBusy}>
                <Trash2 size={14} />
                清空
              </button>
            </div>
          </div>
          <div className="event-log-filters">
            <label>
              <span>级别</span>
              <select value={eventLogLevel} onChange={(event) => handleEventLogLevelChange(event.target.value as AppEventLogLevel | "")}>
                <option value="">全部</option>
                <option value="error">错误</option>
                <option value="warn">警告</option>
                <option value="info">信息</option>
                <option value="debug">调试</option>
              </select>
            </label>
            <label>
              <span>分类</span>
              <select
                value={eventLogCategory}
                onChange={(event) => handleEventLogCategoryChange(event.target.value as AppEventLogCategory | "")}
              >
                <option value="">全部</option>
                <option value="app">应用</option>
                <option value="storage">存储</option>
                <option value="knowledge_base">知识库</option>
                <option value="editor">编辑器</option>
                <option value="agent">Agent</option>
                <option value="model">模型</option>
                <option value="skill">Skill</option>
                <option value="settings">设置</option>
                <option value="security">安全</option>
                <option value="frontend">前端</option>
              </select>
            </label>
          </div>
          <div className="audit-list">
            {appEventLogs.length ? (
              appEventLogs.map((log) => <AppEventLogCard key={log.id} log={log} />)
            ) : (
              <p>暂无运行日志。</p>
            )}
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
        {isSkillsModalOpen && (
          <SkillsModal
            skills={skills}
            isBusy={isBusy}
            onSaveSkill={onSaveSkill}
            onToggleSkill={onToggleSkill}
            onDeleteSkill={onDeleteSkill}
            onOpenUserSkillsFolder={onOpenUserSkillsFolder}
            onClose={() => setIsSkillsModalOpen(false)}
          />
        )}
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

/** 单条应用事件日志卡片，展示运行级别、分类、状态和脱敏上下文。 */
function AppEventLogCard({ log }: { log: AppEventLog }) {
  return (
    <article className={`audit-card event-log-card ${log.level}`}>
      <div className="audit-card-header">
        <strong>
          {formatEventLogLevel(log.level)} · {formatEventLogCategory(log.category)}
        </strong>
        <span>{log.createdAt}</span>
      </div>
      <p>
        {formatEventStatus(log.status)} / {log.event}
      </p>
      <p>{log.message}</p>
      <code>{formatEventLogContext(log)}</code>
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

/** 把运行日志级别转成设置页中文标签。 */
function formatEventLogLevel(level: AppEventLogLevel) {
  const labels: Record<AppEventLogLevel, string> = {
    debug: "调试",
    info: "信息",
    warn: "警告",
    error: "错误",
  };

  return labels[level];
}

/** 把运行日志分类转成设置页中文标签。 */
function formatEventLogCategory(category: AppEventLogCategory) {
  const labels: Record<AppEventLogCategory, string> = {
    app: "应用",
    storage: "存储",
    knowledge_base: "知识库",
    editor: "编辑器",
    agent: "Agent",
    model: "模型",
    skill: "Skill",
    settings: "设置",
    security: "安全",
    frontend: "前端",
  };

  return labels[category];
}

/** 把后端事件状态转成简短中文标签，保留未知状态原文便于排查。 */
function formatEventStatus(status: string) {
  const labels: Record<string, string> = {
    started: "开始",
    completed: "完成",
    failed: "失败",
    blocked: "阻止",
  };

  return labels[status] ?? status;
}

/** 汇总事件日志的轻量上下文，避免卡片中散落过多字段。 */
function formatEventLogContext(log: AppEventLog) {
  const parts = [
    log.operationId ? `op=${log.operationId}` : "",
    log.sessionId ? `session=${log.sessionId}` : "",
    log.knowledgeBaseId ? `kb=${log.knowledgeBaseId}` : "",
    log.entityType && log.entityId ? `${log.entityType}=${log.entityId}` : "",
    log.relativePath ? `path=${log.relativePath}` : "",
    typeof log.durationMs === "number" ? `${log.durationMs}ms` : "",
  ].filter(Boolean);

  return parts.length ? parts.join(" · ") : "无额外上下文";
}
