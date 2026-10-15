import { useEffect, useState } from "react";
import { Save } from "lucide-react";
import { Button } from "../../shared/Button";
import { Checkbox } from "../../shared/Checkbox";
import { ConfirmDialog } from "../../shared/ConfirmDialog";
import { isRemoteSemanticEndpoint, loadSemanticCredentialStatus, probeSemanticSearch, saveSemanticApiKey, saveSemanticSearchSettings } from "../../shared/api/semantic";
import { fieldControlClassName, fieldLabelClassName, settingsSectionClassName } from "../../shared/ui";
import type { KnowledgeBase, SemanticSearchSettings, UserSettings } from "../../shared/types";
import { useSemanticIndexActions } from "../../workspace/useSemanticIndexActions";
import { SettingsSectionHeader } from "../SettingsChrome";

/** 独立 embedding 配置；连接测试只发送固定文本，密钥不进入设置草稿。 */
export function SemanticSearchSettingsSection({ knowledgeBases, settingsDraft, isBusy, onSettingsChange, onSaved }: {
  knowledgeBases: KnowledgeBase[];
  settingsDraft: UserSettings;
  isBusy: boolean;
  onSettingsChange: (settings: UserSettings) => void;
  onSaved: (settings: UserSettings) => Promise<void> | void;
}) {
  const config = settingsDraft.semanticSearch;
  const index = useSemanticIndexActions(knowledgeBases);
  const enabledNames = knowledgeBases.filter((kb) => index.statuses.some((status) => status.knowledgeBaseId === kb.id && status.enabled)).map((kb) => kb.name);
  const [keyDraft, setKeyDraft] = useState("");
  const [configured, setConfigured] = useState(false);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const [confirmCloud, setConfirmCloud] = useState(false);
  const remote = isRemoteSemanticEndpoint(config);
  const locked = isBusy || busy;
  const change = (next: Partial<SemanticSearchSettings>) => {
    const identityChanged = next.provider !== undefined || next.apiBase !== undefined || next.model !== undefined;
    onSettingsChange({ ...settingsDraft, semanticSearch: { ...config, ...(identityChanged ? { cloudConsentFingerprint: null } : {}), ...next } });
    setMessage("");
  };

  useEffect(() => {
    let disposed = false;
    setConfigured(false);
    void loadSemanticCredentialStatus(config).then((status) => { if (!disposed) setConfigured(status.configured); }).catch(() => {});
    return () => { disposed = true; };
  }, [config.provider, config.apiBase, config.model]);

  async function save(allowCloudUpload = false) {
    setBusy(true); setError(""); setConfirmCloud(false);
    try {
      const saved = await saveSemanticSearchSettings(config, allowCloudUpload);
      onSettingsChange(saved);
      await onSaved(saved);
      setMessage("语义检索设置已保存。请在知识库管理中选择需要建立索引的知识库。");
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  }

  async function testConnection() {
    setBusy(true); setError(""); setMessage("");
    try { const result = await probeSemanticSearch(config); setMessage(`${result.message} · ${result.dimensions} 维`); }
    catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  }

  async function saveKey() {
    setBusy(true); setError("");
    try { const result = await saveSemanticApiKey(config, keyDraft); setConfigured(result.configured); setKeyDraft(""); setMessage("密钥已保存到系统安全存储。"); }
    catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  }

  return <section className={settingsSectionClassName} aria-labelledby="semantic-search-title">
    <SettingsSectionHeader kicker="Retrieval" title="语义检索" titleId="semantic-search-title"
      description="用独立的 embedding 模型改善同义词和概念问题召回。当前支持 Markdown，关键词检索始终可用。"
      actions={<Button variant="primary" size="compact" disabled={locked} onClick={() => {
        if (config.enabled && remote && !config.cloudConsentFingerprint) setConfirmCloud(true); else void save();
      }}><Save size={14} />保存设置</Button>} />
    <label className="flex items-center gap-2 text-sm"><Checkbox checked={config.enabled} disabled={locked} onChange={(e) => change({ enabled: e.target.checked })} />开启语义检索</label>
    <div className="grid gap-4 sm:grid-cols-2">
      <label className={fieldLabelClassName}>服务类型
        <select className={fieldControlClassName} value={config.provider} disabled={locked} onChange={(e) => {
          const provider = e.target.value as SemanticSearchSettings["provider"];
          change({ provider, apiBase: provider === "ollama" ? "http://127.0.0.1:11434" : provider === "lm-studio" ? "http://127.0.0.1:1234/v1" : "https://", model: "", requiresApiKey: provider === "openai-compatible" });
        }}><option value="ollama">Ollama（本机）</option><option value="lm-studio">LM Studio（本机）</option><option value="openai-compatible">OpenAI-compatible（云端／自定义）</option></select>
      </label>
      <label className={fieldLabelClassName}>模型名称<input className={fieldControlClassName} value={config.model} disabled={locked} placeholder="填写可生成 embedding 的模型名称" onChange={(e) => change({ model: e.target.value })} /></label>
      <label className={`${fieldLabelClassName} sm:col-span-2`}>服务地址<input className={fieldControlClassName} value={config.apiBase} disabled={locked} onChange={(e) => change({ apiBase: e.target.value })} /></label>
    </div>
    <p className="m-0 text-xs text-ink-muted">{remote ? "云端服务会收到已开启知识库的标题、相对路径、标签、文本片段和检索查询。生成的向量保存在本机。" : "本地服务仅访问本机回环地址，不自动下载模型，也不会失败后切换云端。"}</p>
    {remote && settingsDraft.privacyPolicy === "local-only" && <p className="text-sm text-danger">当前为仅本地隐私策略，云端 embedding 请求已禁用。</p>}
    <label className="flex items-center gap-2 text-sm"><Checkbox checked={config.requiresApiKey} disabled={locked} onChange={(e) => change({ requiresApiKey: e.target.checked })} />服务需要 API key</label>
    {config.requiresApiKey && <div className="grid gap-2">
      <label className={fieldLabelClassName}>API key（{configured ? "已配置" : "未配置"}）<input className={fieldControlClassName} type="password" autoComplete="off" value={keyDraft} disabled={locked} placeholder="密钥仅保存到系统安全存储" onChange={(e) => setKeyDraft(e.target.value)} /></label>
      <div><Button variant="ghost" size="compact" disabled={locked || !keyDraft.trim() || !config.model.trim()} onClick={() => void saveKey()}>保存密钥</Button></div>
    </div>}
    <details><summary className="cursor-pointer text-sm text-ink-muted">高级设置</summary>
      <label className={`${fieldLabelClassName} mt-3`}>最低语义相似度<input className={fieldControlClassName} type="number" min={-1} max={1} step={0.05} value={config.minSimilarity} disabled={locked} onChange={(e) => change({ minSimilarity: Number(e.target.value) })} /></label>
      <p className="text-xs text-ink-muted">默认 0.35。提高可减少弱相关命中；数值含义受模型影响。</p>
    </details>
    <div><Button variant="ghost" size="compact" disabled={locked || !config.model.trim() || (remote && settingsDraft.privacyPolicy === "local-only")} onClick={() => void testConnection()}>测试连接（固定测试文本）</Button></div>
    <p className="m-0 text-xs text-ink-muted">开启知识库后后台自动增量更新。切换模型或地址将重新建立索引，期间使用关键词检索。</p>
    {message && <p role="status" className="text-sm text-ink-muted">{message}</p>}
    {error && <p role="alert" className="text-sm text-danger">{error}</p>}
    {confirmCloud && <ConfirmDialog title="允许云端语义索引" message={`开启后，橘记会向此服务发送已开启知识库的文本片段，并在检索时发送查询。${enabledNames.length ? `目前已开启：${enabledNames.join("、")}，保存后会自动为这些知识库更新云端索引。` : "目前没有已开启的知识库，保存后请到知识库管理中选择。"}保存和重新扫描会自动更新索引，服务可能按使用量收费。知识库管理中可随时暂停或关闭。`} confirmLabel="允许并保存" onCancel={() => setConfirmCloud(false)} onConfirm={() => void save(true)} />}
  </section>;
}
