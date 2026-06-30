import {
  BookOpen,
  FolderOpen,
  History,
  KeyRound,
  Plus,
  RotateCw,
  Save,
  ScrollText,
  Settings2,
  ShieldCheck,
  Sparkles,
  Trash2,
  X,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { logDebug, logError, logInfo } from "../shared/logger";
import type {
  AgentSkill,
  AppEventLog,
  AppEventLogCategory,
  AppEventLogLevel,
  InstallAgentSkillPayload,
  InstallAgentSkillResult,
  KnowledgeBase,
  ModelApiKeyStatus,
  RequestAuditLog,
  UserSettings,
} from "../shared/types";
import { SkillsModal } from "./SkillsModal";

/** 设置页左侧导航的可选分区，和右侧主内容一一对应。 */
type SettingsSectionId = "knowledge" | "model" | "skills" | "eventLogs" | "auditLogs";

/** 设置页导航分组，帮助用户区分可配置项和只读诊断项。 */
type SettingsSectionGroup = "配置" | "诊断";

/** 设置页导航项展示模型，meta 只放脱敏状态或轻量计数。 */
interface SettingsSectionNavItem {
  id: SettingsSectionId;
  group: SettingsSectionGroup;
  label: string;
  description: string;
  meta: string;
  icon: LucideIcon;
  tone?: "neutral" | "success" | "warning";
}

/** 左侧导航分组顺序，保证配置项始终排在诊断项之前。 */
const SETTINGS_SECTION_GROUPS: SettingsSectionGroup[] = ["配置", "诊断"];

/** 设置抽屉，展示多知识库、模型策略、Skills 和诊断信息。 */
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
  onInstallSkill,
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
  onInstallSkill: (payload: InstallAgentSkillPayload) => Promise<InstallAgentSkillResult> | InstallAgentSkillResult;
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
  /** 当前设置分区，驱动左侧导航高亮和右侧单页内容渲染。 */
  const [activeSection, setActiveSection] = useState<SettingsSectionId>("knowledge");
  /** 应用事件日志级别筛选，空字符串表示不过滤。 */
  const [eventLogLevel, setEventLogLevel] = useState<AppEventLogLevel | "">("");
  /** 应用事件日志分类筛选，空字符串表示不过滤。 */
  const [eventLogCategory, setEventLogCategory] = useState<AppEventLogCategory | "">("");
  /** 已启用 skill 数量，用于设置摘要快速说明能力状态。 */
  const enabledSkillCount = skills.filter((skill) => skill.enabled).length;
  /** 允许模型语义参考的已启用 skill 数量，用于提示能力目录覆盖范围。 */
  const autoSkillCount = skills.filter((skill) => skill.enabled && skill.allowAutoInvoke).length;
  /** 文件式 skill 数量用于确认用户目录扫描是否已生效。 */
  const fileSkillCount = skills.filter((skill) => skill.source === "file").length;
  /** 左侧导航项只汇总脱敏状态和轻量计数，避免路径、密钥或请求内容进入 UI 状态元数据。 */
  const settingsNavItems = useMemo<SettingsSectionNavItem[]>(
    () => [
      {
        id: "knowledge",
        group: "配置",
        label: "知识库管理",
        description: "目录授权、激活和重新扫描",
        meta: `${knowledgeBases.length} 个`,
        icon: BookOpen,
        tone: knowledgeBases.some((knowledgeBase) => knowledgeBase.status === "error") ? "warning" : "neutral",
      },
      {
        id: "model",
        group: "配置",
        label: "模型与隐私",
        description: "BYOK、模型端点和发送边界",
        meta: settingsDraft.modelConfig.enabled ? "已启用" : "未启用",
        icon: Settings2,
        tone: settingsDraft.modelConfig.enabled ? "success" : "neutral",
      },
      {
        id: "skills",
        group: "配置",
        label: "Skills 能力",
        description: "模型语义参考和能力管理",
        meta: `${enabledSkillCount}/${skills.length}`,
        icon: Sparkles,
        tone: settingsDraft.skillSettings.activationMode === "auto" ? "success" : "neutral",
      },
      {
        id: "eventLogs",
        group: "诊断",
        label: "运行日志",
        description: "应用事件、级别和分类筛选",
        meta: `${appEventLogs.length} 条`,
        icon: History,
        tone: appEventLogs.some((log) => log.level === "error") ? "warning" : "neutral",
      },
      {
        id: "auditLogs",
        group: "诊断",
        label: "请求审计",
        description: "模型请求和工具边界",
        meta: `${auditLogs.length} 条`,
        icon: ScrollText,
        tone: "neutral",
      },
    ],
    [
      appEventLogs,
      auditLogs.length,
      enabledSkillCount,
      knowledgeBases,
      settingsDraft.modelConfig.enabled,
      settingsDraft.skillSettings.activationMode,
      skills.length,
    ],
  );

  useEffect(() => {
    setSettingsDraft(settings);
  }, [settings]);

  /** 保存模型、隐私和 skill 设置；日志只记录状态和耗时，不记录端点、模型名或密钥。 */
  async function handleSaveSettings() {
    const startedAt = performance.now();
    const nextSettings = {
      ...settingsDraft,
      writeConfirmationRequired: true,
    };

    logInfo("设置页保存用户设置。", {
      category: "settings",
      event: "settings_save",
      status: "started",
      metadata: {
        modelEnabled: nextSettings.modelConfig.enabled,
        privacyPolicy: nextSettings.privacyPolicy,
        skillActivationMode: nextSettings.skillSettings.activationMode,
      },
    });

    try {
      await onSaveSettings(nextSettings);
      logInfo("设置页用户设置保存完成。", {
        category: "settings",
        event: "settings_save",
        status: "completed",
        durationMs: performance.now() - startedAt,
      });
    } catch (error) {
      logError("设置页用户设置保存失败。", {
        category: "settings",
        event: "settings_save",
        status: "failed",
        durationMs: performance.now() - startedAt,
        error,
      });
      throw error;
    }
  }

  /** 保存 BYOK key 后清空输入框，日志只记录是否提交了输入，不记录密钥内容。 */
  async function handleSaveApiKey() {
    const startedAt = performance.now();

    logInfo("设置页保存模型密钥。", {
      category: "settings",
      event: "model_api_key_save",
      status: "started",
      metadata: {
        hasInput: Boolean(apiKeyDraft.trim()),
      },
    });

    try {
      await onSaveApiKey(apiKeyDraft);
      setApiKeyDraft("");
      logInfo("设置页模型密钥保存完成。", {
        category: "settings",
        event: "model_api_key_save",
        status: "completed",
        durationMs: performance.now() - startedAt,
      });
    } catch (error) {
      // 外层已经把失败原因写入全局 notice；这里保留输入，方便用户修正后重试。
      logError("设置页模型密钥保存失败。", {
        category: "settings",
        event: "model_api_key_save",
        status: "failed",
        durationMs: performance.now() - startedAt,
        error,
      });
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

  /** 切换设置分区并写入低频调试日志，便于诊断导航状态但不影响渲染性能。 */
  function handleActiveSectionChange(sectionId: SettingsSectionId) {
    if (sectionId === activeSection) {
      return;
    }

    logDebug("切换设置页分区。", {
      category: "settings",
      event: "settings_section_change",
      status: "completed",
      metadata: {
        from: activeSection,
        to: sectionId,
      },
    });
    setActiveSection(sectionId);
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

  /** 渲染设置导航分组；按配置/诊断拆开，避免所有入口混在同一列表里。 */
  function renderNavigationGroup(group: SettingsSectionGroup) {
    const groupItems = settingsNavItems.filter((item) => item.group === group);

    return (
      <div className="settings-nav-group" key={group}>
        <p className="settings-nav-group-label">{group}</p>
        <div className="settings-nav-items">
          {groupItems.map((item) => {
            const SectionIcon = item.icon;

            return (
              <button
                className={`settings-nav-item ${activeSection === item.id ? "active" : ""}`}
                key={item.id}
                type="button"
                aria-current={activeSection === item.id ? "page" : undefined}
                onClick={() => handleActiveSectionChange(item.id)}
              >
                <SectionIcon size={17} />
                <span className="settings-nav-text">
                  <strong>{item.label}</strong>
                  <small>{item.description}</small>
                </span>
                <em className={`settings-nav-meta ${item.tone ?? "neutral"}`}>{item.meta}</em>
              </button>
            );
          })}
        </div>
      </div>
    );
  }

  /** 根据左侧选中项只渲染一个主配置区，避免设置面板出现长串卡片和双重滚动。 */
  function renderActiveSection() {
    if (activeSection === "knowledge") {
      return (
        <section className="settings-section" aria-labelledby="knowledge-settings-title">
          <div className="settings-section-title settings-content-title">
            <div>
              <p className="section-label">Configuration</p>
              <h3 id="knowledge-settings-title">知识库管理</h3>
              <p>管理已授权目录、激活知识库和本地索引刷新。</p>
            </div>
            <button className="ghost-button" type="button" onClick={onAddKnowledgeBase}>
              <Plus size={15} />
              添加知识库
            </button>
          </div>
          <div className="settings-kb-list">
            {knowledgeBases.length ? (
              knowledgeBases.map((knowledgeBase) => (
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
              ))
            ) : (
              <p className="settings-empty">暂无已授权知识库。</p>
            )}
          </div>
        </section>
      );
    }

    if (activeSection === "model") {
      return (
        <section className="settings-section" aria-labelledby="model-settings-title">
          <div className="settings-section-title settings-content-title">
            <div>
              <p className="section-label">Configuration</p>
              <h3 id="model-settings-title">模型与隐私</h3>
              <p>配置 OpenAI-compatible BYOK、发送边界和写入确认策略。</p>
            </div>
            <button className="primary-button compact" type="button" onClick={handleSaveSettings} disabled={isBusy}>
              <Save size={14} />
              保存设置
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
            <label className="settings-full-row">
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
          <div className="policy-row">
            <ShieldCheck size={16} />
            <span>Agent 写入工具只能生成 diff；用户确认后才执行路径校验、hash 校验和原子写入。</span>
          </div>
        </section>
      );
    }

    if (activeSection === "skills") {
      return (
        <section className="settings-section" aria-labelledby="skills-settings-title">
          <div className="settings-section-title settings-content-title">
            <div>
              <p className="section-label">Configuration</p>
              <h3 id="skills-settings-title">Skills 能力</h3>
              <p>管理 Agent 可用能力和未显式选择时的匹配方式。</p>
            </div>
            <div className="settings-title-actions">
              <button className="ghost-button" type="button" onClick={() => setIsSkillsModalOpen(true)}>
                <Sparkles size={14} />
                管理 Skills
              </button>
              <button className="primary-button compact" type="button" onClick={handleSaveSettings} disabled={isBusy}>
                <Save size={14} />
                保存设置
              </button>
            </div>
          </div>
          <div className="skills-summary">
            <div>
              <span>启用</span>
              <strong>
                {enabledSkillCount} / {skills.length}
              </strong>
            </div>
            <div>
              <span>模型参考</span>
              <strong>{settingsDraft.skillSettings.activationMode === "auto" ? `${autoSkillCount} 个` : "已关闭"}</strong>
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
            <span>允许未显式选择时让模型参考 Skill 目录</span>
          </label>
        </section>
      );
    }

    if (activeSection === "eventLogs") {
      return (
        <section className="settings-section" aria-labelledby="event-log-settings-title">
          <div className="settings-section-title settings-content-title">
            <div>
              <p className="section-label">Diagnostics</p>
              <h3 id="event-log-settings-title">运行日志</h3>
              <p>查看应用事件日志，按级别和分类筛选。</p>
            </div>
            <div className="settings-title-actions">
              <button className="ghost-button" type="button" onClick={onOpenAppLogFolder} disabled={isBusy}>
                <FolderOpen size={14} />
                文件日志
              </button>
              <button className="ghost-button" type="button" onClick={() => onRefreshAppEventLogs(currentEventLogFilters())} disabled={isBusy}>
                <RotateCw size={14} />
                刷新
              </button>
              <button
                className="ghost-button danger-action"
                type="button"
                onClick={() => onClearAppEventLogs(currentEventLogFilters())}
                disabled={isBusy}
              >
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
              <p className="settings-empty">暂无运行日志。</p>
            )}
          </div>
        </section>
      );
    }

    return (
      <section className="settings-section" aria-labelledby="audit-settings-title">
        <div className="settings-section-title settings-content-title">
          <div>
            <p className="section-label">Diagnostics</p>
            <h3 id="audit-settings-title">请求审计</h3>
            <p>查看最近模型请求、本地规则回退和工具边界摘要。</p>
          </div>
          <button className="ghost-button" type="button" onClick={onRefreshAuditLogs} disabled={isBusy}>
            <RotateCw size={14} />
            刷新
          </button>
        </div>
        <div className="audit-list">
          {auditLogs.length ? auditLogs.map((log) => <AuditLogCard key={log.id} log={log} />) : <p className="settings-empty">暂无审计记录。</p>}
        </div>
      </section>
    );
  }

  return (
    <div className="settings-backdrop" role="presentation" onMouseDown={onClose}>
      <aside className="settings-drawer" aria-label="设置" onMouseDown={(event) => event.stopPropagation()}>
        <header className="settings-header">
          <div>
            <p className="section-label">Settings</p>
            <h2>知识库与 Agent 设置</h2>
          </div>
          <button className="icon-button" type="button" title="关闭设置" onClick={onClose}>
            <X size={18} />
          </button>
        </header>

        <div className="settings-workbench">
          <nav className="settings-sidebar" aria-label="设置项">
            {SETTINGS_SECTION_GROUPS.map((group) => renderNavigationGroup(group))}
          </nav>
          <main className="settings-content" aria-label="设置主要内容">
            {renderActiveSection()}
          </main>
        </div>
        {isSkillsModalOpen && (
          <SkillsModal
            skills={skills}
            isBusy={isBusy}
            onSaveSkill={onSaveSkill}
            onInstallSkill={onInstallSkill}
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
