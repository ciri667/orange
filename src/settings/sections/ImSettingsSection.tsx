import { KeyRound, MessageCircle, QrCode, RotateCw, Save } from "lucide-react";
import { useState } from "react";
import { Button } from "../../shared/Button";
import { Checkbox } from "../../shared/Checkbox";
import { cn } from "../../shared/cn";
import { listRowClassName } from "../../shared/ListRow";
import { OverflowTooltipText } from "../../shared/OverflowTooltipText";
import { SelectControl } from "../../shared/SelectControl";
import { ToggleRow } from "../../shared/ToggleRow";
import {
  fieldControlClassName,
  fieldLabelClassName,
  fieldTextareaClassName,
  settingsSectionClassName,
} from "../../shared/ui";
import { SettingsPolicyRow, SettingsSectionHeader, SettingsSubblockHeader } from "../SettingsChrome";
import type {
  FeishuProviderConfig,
  ImGatewayStatus,
  ImLoginStatus,
  ImProviderCredentialStatus,
  ImProviderId,
  ImProviderSettings,
  KnowledgeBase,
  QqProviderConfig,
  WeixinProviderConfig,
} from "../../shared/types";

const PROVIDER_TABS: Array<{ id: ImProviderId; label: string }> = [
  { id: "feishu", label: "飞书" },
  { id: "qq", label: "QQ" },
  { id: "weixin", label: "微信" },
];

/** 即时通讯设置分区，按 provider 切换飞书、QQ 官方机器人和个人微信。 */
export function ImSettingsSection({
  knowledgeBases,
  providers,
  credentialByProvider,
  gatewayByProvider,
  secretDraftByProvider,
  weixinLoginStatus,
  isBusy,
  onProviderDraftChange,
  onSecretDraftChange,
  onParseMultilineIds,
  onAllowDiscoveredUser,
  onAllowDiscoveredChat,
  onRefreshStatus,
  onStopGateway,
  onStartGateway,
  onSaveImSettings,
  onSaveSecret,
  onStartWeixinLogin,
  onCancelWeixinLogin,
}: {
  knowledgeBases: KnowledgeBase[];
  providers: ImProviderSettings[];
  credentialByProvider: Partial<Record<ImProviderId, ImProviderCredentialStatus | null>>;
  gatewayByProvider: Partial<Record<ImProviderId, ImGatewayStatus | null>>;
  secretDraftByProvider: Partial<Record<ImProviderId, string>>;
  weixinLoginStatus: ImLoginStatus | null;
  isBusy: boolean;
  onProviderDraftChange: (providerId: ImProviderId, next: ImProviderSettings) => void;
  onSecretDraftChange: (providerId: ImProviderId, value: string) => void;
  onParseMultilineIds: (value: string) => string[];
  onAllowDiscoveredUser: (providerId: ImProviderId, openId: string) => void;
  onAllowDiscoveredChat: (providerId: ImProviderId, chatId: string) => void;
  onRefreshStatus: (providerId: ImProviderId) => void | Promise<void>;
  onStopGateway: (providerId: ImProviderId) => void | Promise<void>;
  onStartGateway: (providerId: ImProviderId) => void | Promise<void>;
  onSaveImSettings: () => void | Promise<void>;
  onSaveSecret: (providerId: ImProviderId) => void | Promise<void>;
  onStartWeixinLogin: () => void | Promise<void>;
  onCancelWeixinLogin: () => void | Promise<void>;
}) {
  const [activeProviderId, setActiveProviderId] = useState<ImProviderId>("feishu");
  const provider = providers.find((candidate) => candidate.providerId === activeProviderId) ?? providers[0];
  const gatewayStatus = gatewayByProvider[provider.providerId] ?? null;
  const credentialStatus = credentialByProvider[provider.providerId] ?? null;
  const secretDraft = secretDraftByProvider[provider.providerId] ?? "";
  const selectedKnowledgeBaseIds = new Set(provider.defaultKnowledgeBaseIds);

  return (
    <section className={settingsSectionClassName} aria-labelledby="im-settings-title">
      <SettingsSectionHeader
        kicker="Configuration"
        title="即时通讯"
        titleId="im-settings-title"
        description="连接已注册的即时通讯 provider，允许白名单用户通过文本消息调用 Agent。"
        actions={
          <>
            <Button variant="ghost" onClick={() => onRefreshStatus(provider.providerId)} disabled={isBusy}>
              <RotateCw size={14} />
              刷新
            </Button>
            {gatewayStatus?.running ? (
              <Button variant="ghost" tone="danger" onClick={() => onStopGateway(provider.providerId)} disabled={isBusy}>
                停止
              </Button>
            ) : (
              <Button variant="primary" size="compact" onClick={() => onStartGateway(provider.providerId)} disabled={isBusy}>
                启动
              </Button>
            )}
            <Button variant="primary" size="compact" onClick={onSaveImSettings} disabled={isBusy}>
              <Save size={14} />
              保存设置
            </Button>
          </>
        }
      />

      <div className="flex flex-wrap gap-1.5">
        {PROVIDER_TABS.map((tab) => {
          const tabProvider = providers.find((candidate) => candidate.providerId === tab.id);
          const tabGateway = gatewayByProvider[tab.id];
          return (
            <button
              key={tab.id}
              type="button"
              className={cn(
                "rounded-md border px-3 py-1.5 text-[13px]",
                activeProviderId === tab.id
                  ? "border-primary-border bg-primary-wash text-ink-strong"
                  : "border-border bg-surface text-ink-muted",
              )}
              onClick={() => setActiveProviderId(tab.id)}
            >
              {tab.label}
              {tabGateway?.running ? " · 运行中" : tabProvider?.enabled ? " · 已配置" : ""}
            </button>
          );
        })}
      </div>

      <div className="grid grid-cols-2 gap-x-3.5 gap-y-3 max-[820px]:grid-cols-1">
        <ToggleRow
          className="col-span-full"
          checked={provider.enabled}
          onChange={(checked) => onProviderDraftChange(provider.providerId, { ...provider, enabled: checked })}
        >
          启用{providerLabel(provider.providerId)}集成
        </ToggleRow>
        {provider.config.type === "feishu" ? (
          <FeishuConfigFields
            config={provider.config}
            onChange={(config) => onProviderDraftChange(provider.providerId, { ...provider, config })}
          />
        ) : null}
        {provider.config.type === "qq" ? (
          <QqConfigFields
            config={provider.config}
            onChange={(config) => onProviderDraftChange(provider.providerId, { ...provider, config })}
          />
        ) : null}
        {provider.config.type === "weixin" ? (
          <WeixinConfigFields config={provider.config} loginStatus={weixinLoginStatus} isBusy={isBusy} onStartLogin={onStartWeixinLogin} onCancelLogin={onCancelWeixinLogin} />
        ) : null}
        {provider.config.type !== "weixin" ? (
          <label className={cn(fieldLabelClassName, "col-span-full")}>
            <span>{provider.config.type === "qq" ? "App Secret" : "App Secret"}</span>
            <div className="grid grid-cols-[minmax(0,1fr)_auto] gap-2">
              <input
                className={cn(fieldControlClassName, "tracking-[0.02em]")}
                type="password"
                value={secretDraft}
                onChange={(event) => onSecretDraftChange(provider.providerId, event.target.value)}
                placeholder={credentialStatus?.configured ? "已保存，输入新值可替换" : `输入${providerLabel(provider.providerId)}密钥`}
              />
              <Button variant="ghost" onClick={() => onSaveSecret(provider.providerId)} disabled={isBusy || !secretDraft.trim()}>
                <KeyRound size={14} />
                保存密钥
              </Button>
            </div>
            <em className="font-normal not-italic">{credentialStatus?.message ?? "尚未读取凭证状态。"}</em>
          </label>
        ) : (
          <p className="col-span-full m-0 text-[13px] text-ink-muted">{credentialStatus?.message ?? "扫码成功后会自动保存登录 token。"}</p>
        )}
        <ToggleRow
          className="col-span-full"
          checked={provider.requireMention}
          onChange={(checked) => onProviderDraftChange(provider.providerId, { ...provider, requireMention: checked })}
        >
          群聊必须直接 @ 机器人
        </ToggleRow>
      </div>

      <div className="grid gap-3">
        <SettingsSubblockHeader title="默认知识库范围" description={`${providerLabel(provider.providerId)}消息只能检索这些知识库；写入类请求仍只生成待确认 diff。`} />
        <div className="grid grid-cols-2 gap-2 max-[820px]:grid-cols-1">
          {knowledgeBases.map((knowledgeBase) => {
            const isSelected = selectedKnowledgeBaseIds.has(knowledgeBase.id);

            return (
              <label
                className={listRowClassName({
                  active: isSelected,
                  className: "relative border-border-translucent bg-surface-translucent",
                })}
                key={knowledgeBase.id}
              >
                <Checkbox
                  checked={isSelected}
                  onChange={() => {
                    const nextIds = new Set(provider.defaultKnowledgeBaseIds);
                    if (nextIds.has(knowledgeBase.id)) {
                      nextIds.delete(knowledgeBase.id);
                    } else {
                      nextIds.add(knowledgeBase.id);
                    }
                    onProviderDraftChange(provider.providerId, {
                      ...provider,
                      defaultKnowledgeBaseIds: Array.from(nextIds),
                    });
                  }}
                />
                <span className="min-w-0">
                  <OverflowTooltipText as="strong" className="block truncate text-ink-strong" text={knowledgeBase.name} logArea="settings_im_scope_name" />
                  <OverflowTooltipText
                    className="mt-[3px] block truncate text-xs text-ink-muted"
                    text={knowledgeBase.status === "error" ? "目录失效" : `${knowledgeBase.noteCount} 篇笔记`}
                    logArea="settings_im_scope_detail"
                  />
                </span>
              </label>
            );
          })}
        </div>
      </div>

      <div className="grid gap-3">
        <SettingsSubblockHeader title="待授权对象" description="收到未授权消息后会自动出现在这里；点击允许后保存设置即可生效。" />
        {provider.discoveredUserOpenIds.length || provider.discoveredChatIds.length ? (
          <div className="grid gap-2">
            {provider.discoveredUserOpenIds.map((openId, index) => (
              <div className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-2.5 rounded-[7px] border border-border bg-white p-2.5" key={openId}>
                <span className="grid min-w-0 gap-[3px]">
                  <strong className="text-[13px] text-ink-strong">用户候选 {index + 1}</strong>
                  <OverflowTooltipText className="font-mono text-xs text-ink-muted" text={formatIdentifierPreview(openId)} logArea="settings_im_discovered_user" />
                </span>
                <Button variant="ghost" size="compact" onClick={() => onAllowDiscoveredUser(provider.providerId, openId)}>
                  允许用户
                </Button>
              </div>
            ))}
            {provider.discoveredChatIds.map((chatId, index) => (
              <div className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-2.5 rounded-[7px] border border-border bg-white p-2.5" key={chatId}>
                <span className="grid min-w-0 gap-[3px]">
                  <strong className="text-[13px] text-ink-strong">群候选 {index + 1}</strong>
                  <OverflowTooltipText className="font-mono text-xs text-ink-muted" text={formatIdentifierPreview(chatId)} logArea="settings_im_discovered_chat" />
                </span>
                <Button variant="ghost" size="compact" onClick={() => onAllowDiscoveredChat(provider.providerId, chatId)}>
                  允许群聊
                </Button>
              </div>
            ))}
          </div>
        ) : (
          <p className="m-0 text-[13px] text-ink-muted">暂无待授权对象。让用户或群先给机器人发送一条消息后刷新状态。</p>
        )}
      </div>

      <div className="grid grid-cols-2 gap-x-3.5 gap-y-3 max-[820px]:grid-cols-1">
        <label className={cn(fieldLabelClassName, "col-span-full")}>
          <span>允许用户 ID</span>
          <textarea
            className={fieldTextareaClassName}
            value={provider.allowedUserOpenIds.join("\n")}
            onChange={(event) => onProviderDraftChange(provider.providerId, { ...provider, allowedUserOpenIds: onParseMultilineIds(event.target.value) })}
            rows={4}
            placeholder="每行一个用户 ID；私聊必须授权发送人"
          />
        </label>
        <label className={cn(fieldLabelClassName, "col-span-full")}>
          <span>允许群 ID</span>
          <textarea
            className={fieldTextareaClassName}
            value={provider.allowedChatIds.join("\n")}
            onChange={(event) => onProviderDraftChange(provider.providerId, { ...provider, allowedChatIds: onParseMultilineIds(event.target.value) })}
            rows={4}
            placeholder="每行一个群 ID；私聊可留空"
          />
        </label>
      </div>

      <SettingsPolicyRow icon={<MessageCircle size={16} />}>
        网关：{gatewayStatus?.running ? "运行中" : "未运行"} / 连接：
        {gatewayStatus?.connected ? "已收到事件" : "未确认"} / 平台：{gatewayStatus?.domain ?? provider.providerId}
      </SettingsPolicyRow>
      <p className="m-0 text-[13px] text-ink-muted">{providerHelpText(provider.providerId)}</p>
      {gatewayStatus?.lastError ? <p className="m-0 text-[13px] text-ink-muted">{gatewayStatus.lastError}</p> : null}
    </section>
  );
}

function FeishuConfigFields({
  config,
  onChange,
}: {
  config: FeishuProviderConfig;
  onChange: (config: FeishuProviderConfig) => void;
}) {
  return (
    <>
      <label className={fieldLabelClassName}>
        <span>平台</span>
        <SelectControl value={config.domain} onChange={(event) => onChange({ ...config, domain: event.target.value as "feishu" | "lark" })}>
          <option value="feishu">飞书</option>
          <option value="lark">Lark</option>
        </SelectControl>
      </label>
      <label className={fieldLabelClassName}>
        <span>App ID</span>
        <input className={fieldControlClassName} value={config.appId} onChange={(event) => onChange({ ...config, appId: event.target.value })} placeholder="cli_xxx" />
      </label>
    </>
  );
}

function QqConfigFields({
  config,
  onChange,
}: {
  config: QqProviderConfig;
  onChange: (config: QqProviderConfig) => void;
}) {
  return (
    <label className={cn(fieldLabelClassName, "col-span-full")}>
      <span>App ID</span>
      <input className={fieldControlClassName} value={config.appId} onChange={(event) => onChange({ ...config, appId: event.target.value })} placeholder="QQ 开放平台 AppID" />
    </label>
  );
}

function WeixinConfigFields({
  config,
  loginStatus,
  isBusy,
  onStartLogin,
  onCancelLogin,
}: {
  config: WeixinProviderConfig;
  loginStatus: ImLoginStatus | null;
  isBusy: boolean;
  onStartLogin: () => void | Promise<void>;
  onCancelLogin: () => void | Promise<void>;
}) {
  const qrImage = loginStatus?.qrImageBase64;
  const loggedIn = Boolean(config.accountId.trim()) && loginStatus?.status !== "expired";

  return (
    <div className="col-span-full grid gap-2 rounded-lg border border-border bg-surface p-3">
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="primary" size="compact" onClick={onStartLogin} disabled={isBusy}>
          <QrCode size={14} />
          {loggedIn ? "重新扫码" : "扫码登录"}
        </Button>
        {loginStatus?.status === "wait" ? (
          <Button variant="ghost" size="compact" onClick={onCancelLogin} disabled={isBusy}>
            取消
          </Button>
        ) : null}
        <span className="text-[13px] text-ink-muted">{loginStatus?.message ?? (loggedIn ? `已登录 ${config.accountId}` : "尚未登录")}</span>
      </div>
      {qrImage ? (
        <img src={qrImage} alt="微信登录二维码" className="h-48 w-48 rounded-md border border-border bg-white p-2" />
      ) : null}
    </div>
  );
}

function providerLabel(providerId: ImProviderId) {
  if (providerId === "qq") {
    return "QQ";
  }
  if (providerId === "weixin") {
    return "微信";
  }
  return "飞书/Lark";
}

function providerHelpText(providerId: ImProviderId) {
  if (providerId === "qq") {
    return "使用 QQ 官方机器人 WebSocket。请在 q.qq.com 创建机器人，填写 AppID/Secret，并按需配置出站 IP 白名单与沙箱群。待确认改动使用“详情 / 确认 / 取消 <编号>”文字指令。";
  }
  if (providerId === "weixin") {
    return "使用腾讯官方个人微信助手接口（iLink）。需要较新的手机微信，且客户端包含 ClawBot 插件。扫码登录后无需公网回调。待确认改动使用文字指令审批。";
  }
  return "待确认改动使用飞书审批卡片。请在飞书开发者后台启用长连接，并订阅 im.message.receive_v1 和 card.action.trigger。";
}

/** 设置页只展示 ID 的短预览；完整 ID 保留在本地输入框和持久化配置中。 */
function formatIdentifierPreview(value: string) {
  const trimmed = value.trim();

  if (trimmed.length <= 12) {
    return trimmed || "未命名对象";
  }

  return `${trimmed.slice(0, 6)}...${trimmed.slice(-4)}`;
}
