import { useEffect, useState } from "react";
import { Save } from "lucide-react";
import { Button } from "../../shared/Button";
import { Checkbox } from "../../shared/Checkbox";
import {
  loadWebSearchCredentialStatus,
  probeWebSearch,
  saveWebSearchApiKey,
} from "../../shared/api/settings";
import { fieldControlClassName, fieldLabelClassName, settingsSectionClassName } from "../../shared/ui";
import type { UserSettings, WebSearchCredentialStatus } from "../../shared/types";
import { SettingsSectionHeader } from "../SettingsChrome";

const STATUS_LABEL: Record<WebSearchCredentialStatus["credentialStatus"], string> = {
  untested: "未测试",
  valid: "可用",
  invalid_credentials: "密钥无效",
  rate_limited: "限流",
  network_error: "网络错误",
  not_configured: "未配置密钥",
};

/** 联网搜索开关和 Tavily 密钥。密钥不进入设置草稿。 */
export function WebSearchSettingsSection({
  settingsDraft,
  isBusy,
  onSettingsChange,
  onSaveSettings,
}: {
  settingsDraft: UserSettings;
  isBusy: boolean;
  onSettingsChange: (settings: UserSettings) => void;
  onSaveSettings: () => void | Promise<void>;
}) {
  const [credential, setCredential] = useState<WebSearchCredentialStatus | null>(null);
  const [apiKeyDraft, setApiKeyDraft] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const webSearch = settingsDraft.webSearch;

  useEffect(() => {
    let cancelled = false;
    void loadWebSearchCredentialStatus()
      .then((status) => {
        if (!cancelled) {
          setCredential(status);
        }
      })
      .catch((loadError: unknown) => {
        if (!cancelled) {
          setError(loadError instanceof Error ? loadError.message : String(loadError));
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function handleSaveKey() {
    setBusy(true);
    setError("");
    try {
      const status = await saveWebSearchApiKey(apiKeyDraft);
      setCredential(status);
      setApiKeyDraft("");
      onSettingsChange({
        ...settingsDraft,
        webSearch: { ...webSearch, credentialStatus: status.credentialStatus },
      });
    } catch (saveError: unknown) {
      setError(saveError instanceof Error ? saveError.message : String(saveError));
    } finally {
      setBusy(false);
    }
  }

  async function handleProbe() {
    setBusy(true);
    setError("");
    try {
      const status = await probeWebSearch();
      setCredential(status);
      onSettingsChange({
        ...settingsDraft,
        webSearch: { ...webSearch, credentialStatus: status.credentialStatus },
      });
    } catch (probeError: unknown) {
      setError(probeError instanceof Error ? probeError.message : String(probeError));
      const status = await loadWebSearchCredentialStatus().catch(() => null);
      if (status) {
        setCredential(status);
      }
    } finally {
      setBusy(false);
    }
  }

  const status = credential?.credentialStatus ?? webSearch.credentialStatus;
  const locked = isBusy || busy;

  return (
    <section className={settingsSectionClassName} aria-labelledby="web-search-settings-title">
      <SettingsSectionHeader
        kicker="Web"
        title="联网搜索"
        titleId="web-search-settings-title"
        description="用 Tavily 查公开网页。默认关闭。打开后，基础、进阶和完全级别都可以用。"
        actions={
          <Button variant="primary" size="compact" type="button" disabled={locked} onClick={() => void onSaveSettings()}>
            <Save size={14} />
            保存设置
          </Button>
        }
      />
      <label className="mt-4 flex items-center gap-2 text-sm text-ink">
        <Checkbox
          checked={webSearch.enabled}
          disabled={locked}
          onChange={(event) =>
            onSettingsChange({
              ...settingsDraft,
              webSearch: { ...webSearch, enabled: event.target.checked },
            })
          }
        />
        允许 Agent 联网搜索
      </label>
      <p className="mt-3 text-xs text-ink-soft">
        后端固定为 Tavily。状态：{STATUS_LABEL[status]}
        {credential?.configured ? " · 密钥已配置" : " · 密钥未配置"}
      </p>
      <label className={`${fieldLabelClassName} mt-4`}>
        Tavily API key
        <input
          className={fieldControlClassName}
          type="password"
          autoComplete="off"
          value={apiKeyDraft}
          disabled={locked}
          placeholder={credential?.configured ? "已配置，留空则不改" : "粘贴 API key"}
          onChange={(event) => setApiKeyDraft(event.target.value)}
        />
      </label>
      <div className="mt-3 flex gap-2">
        <Button variant="ghost" size="compact" type="button" disabled={locked || apiKeyDraft.trim().length === 0} onClick={() => void handleSaveKey()}>
          保存密钥
        </Button>
        <Button variant="ghost" size="compact" type="button" disabled={locked || !credential?.configured} onClick={() => void handleProbe()}>
          探测
        </Button>
      </div>
      {credential?.message ? <p className="mt-3 text-xs text-ink-muted">{credential.message}</p> : null}
      {error ? <p className="mt-2 text-xs text-danger">{error}</p> : null}
    </section>
  );
}
