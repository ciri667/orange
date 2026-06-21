import { Check, Plus, RotateCw, Trash2, X } from "lucide-react";
import type { KnowledgeBase } from "../shared/types";

/** 设置抽屉，展示多知识库管理、模型策略和写入权限。 */
export function SettingsDrawer({
  knowledgeBases,
  activeKnowledgeBaseId,
  isBusy,
  onSelectKnowledgeBase,
  onAddKnowledgeBase,
  onRescanKnowledgeBase,
  onRemoveKnowledgeBase,
  onClose,
}: {
  knowledgeBases: KnowledgeBase[];
  activeKnowledgeBaseId: string;
  isBusy: boolean;
  onSelectKnowledgeBase: (knowledgeBaseId: string) => void;
  onAddKnowledgeBase: () => void;
  onRescanKnowledgeBase: (knowledgeBaseId: string) => void;
  onRemoveKnowledgeBase: (knowledgeBaseId: string) => void;
  onClose: () => void;
}) {
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
          <p>// todo: 正式版本应支持语义索引开关和更细粒度的跳过目录配置。</p>
        </div>

        <div className="settings-section">
          <h3>模型与工具</h3>
          <label>
            <span>首版策略</span>
            <input value="云端模型 BYOK；本地文件访问只能通过受控工具执行" readOnly />
          </label>
          <p>// todo: 接入模型配置、密钥安全存储、请求审计和 embedding 范围确认。</p>
        </div>

        <div className="settings-section">
          <h3>写入策略</h3>
          <div className="policy-row">
            <Check size={16} />
            <span>Agent 写入工具只能生成 diff；用户确认后才执行路径校验、hash 校验和原子写入。</span>
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
