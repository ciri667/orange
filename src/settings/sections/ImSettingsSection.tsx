import { KeyRound, MessageCircle, RotateCw, Save } from "lucide-react";
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
  FeishuCredentialStatus,
  FeishuGatewayStatus,
  FeishuIntegrationSettings,
  KnowledgeBase,
} from "../../shared/types";

/** 即时通讯设置分区，首版仅渲染飞书/Lark provider。 */
export function ImSettingsSection({
  knowledgeBases,
  feishu,
  feishuCredentialStatus,
  feishuGatewayStatus,
  feishuSecretDraft,
  isBusy,
  onFeishuDraftChange,
  onFeishuConfigDraftChange,
  onFeishuSecretDraftChange,
  onParseMultilineIds,
  onAllowDiscoveredFeishuUser,
  onAllowDiscoveredFeishuChat,
  onRefreshFeishuStatus,
  onStopFeishuGateway,
  onStartFeishuGateway,
  onSaveImSettings,
  onSaveFeishuSecret,
}: {
  knowledgeBases: KnowledgeBase[];
  feishu: FeishuIntegrationSettings;
  feishuCredentialStatus: FeishuCredentialStatus | null;
  feishuGatewayStatus: FeishuGatewayStatus | null;
  feishuSecretDraft: string;
  isBusy: boolean;
  onFeishuDraftChange: <K extends keyof FeishuIntegrationSettings>(field: K, value: FeishuIntegrationSettings[K]) => void;
  onFeishuConfigDraftChange: <K extends keyof FeishuIntegrationSettings["config"]>(
    field: K,
    value: FeishuIntegrationSettings["config"][K],
  ) => void;
  onFeishuSecretDraftChange: (value: string) => void;
  onParseMultilineIds: (value: string) => string[];
  onAllowDiscoveredFeishuUser: (openId: string) => void;
  onAllowDiscoveredFeishuChat: (chatId: string) => void;
  onRefreshFeishuStatus: () => void | Promise<void>;
  onStopFeishuGateway: () => void | Promise<void>;
  onStartFeishuGateway: () => void | Promise<void>;
  onSaveImSettings: () => void | Promise<void>;
  onSaveFeishuSecret: () => void | Promise<void>;
}) {
  const selectedKnowledgeBaseIds = new Set(feishu.defaultKnowledgeBaseIds);

  return (
    <section className={settingsSectionClassName} aria-labelledby="im-settings-title">
      <SettingsSectionHeader
        kicker="Configuration"
        title="即时通讯"
        titleId="im-settings-title"
        description="连接已注册的即时通讯 provider，允许白名单用户通过文本消息调用 Agent。"
        actions={
          <>
            <Button variant="ghost" onClick={onRefreshFeishuStatus} disabled={isBusy}>
              <RotateCw size={14} />
              刷新
            </Button>
            {feishuGatewayStatus?.running ? (
              <Button variant="ghost" tone="danger" onClick={onStopFeishuGateway} disabled={isBusy}>
                停止
              </Button>
            ) : (
              <Button variant="primary" size="compact" onClick={onStartFeishuGateway} disabled={isBusy}>
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

      <div className="grid grid-cols-2 gap-x-3.5 gap-y-3 max-[820px]:grid-cols-1">
        <ToggleRow className="col-span-full" checked={feishu.enabled} onChange={(checked) => onFeishuDraftChange("enabled", checked)}>
          启用飞书/Lark 集成
        </ToggleRow>
        <label className={fieldLabelClassName}>
          <span>平台</span>
          <SelectControl value={feishu.config.domain} onChange={(event) => onFeishuConfigDraftChange("domain", event.target.value as "feishu" | "lark")}>
            <option value="feishu">飞书</option>
            <option value="lark">Lark</option>
          </SelectControl>
        </label>
        <label className={fieldLabelClassName}>
          <span>App ID</span>
          <input className={fieldControlClassName} value={feishu.config.appId} onChange={(event) => onFeishuConfigDraftChange("appId", event.target.value)} placeholder="cli_xxx" />
        </label>
        <label className={cn(fieldLabelClassName, "col-span-full")}>
          <span>App Secret</span>
          <div className="grid grid-cols-[minmax(0,1fr)_auto] gap-2">
            <input
              className={cn(fieldControlClassName, "tracking-[0.02em]")}
              type="password"
              value={feishuSecretDraft}
              onChange={(event) => onFeishuSecretDraftChange(event.target.value)}
              placeholder={feishuCredentialStatus?.configured ? "已保存，输入新值可替换" : "输入飞书 appSecret"}
            />
            <Button variant="ghost" onClick={onSaveFeishuSecret} disabled={isBusy || !feishuSecretDraft.trim()}>
              <KeyRound size={14} />
              保存密钥
            </Button>
          </div>
          <em className="font-normal not-italic">{feishuCredentialStatus?.message ?? "尚未读取凭证状态。"}</em>
        </label>
        <ToggleRow className="col-span-full" checked={feishu.requireMention} onChange={(checked) => onFeishuDraftChange("requireMention", checked)}>
          群聊必须直接 @ 机器人
        </ToggleRow>
      </div>

      <div className="grid gap-3">
        <SettingsSubblockHeader title="默认知识库范围" description="飞书消息只能检索这些知识库；写入类请求仍只生成待确认 diff。" />
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
                    const nextIds = new Set(feishu.defaultKnowledgeBaseIds);

                    // 多选范围允许用户手动增减；这里只更新草稿，保存/启动时再持久化。
                    if (nextIds.has(knowledgeBase.id)) {
                      nextIds.delete(knowledgeBase.id);
                    } else {
                      nextIds.add(knowledgeBase.id);
                    }

                    onFeishuDraftChange("defaultKnowledgeBaseIds", Array.from(nextIds));
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
        <SettingsSubblockHeader title="待授权飞书对象" description="收到未授权消息后会自动出现在这里；点击允许后保存设置即可生效。" />
        {feishu.discoveredUserOpenIds.length || feishu.discoveredChatIds.length ? (
          <div className="grid gap-2">
            {feishu.discoveredUserOpenIds.map((openId, index) => (
              <div className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-2.5 rounded-[7px] border border-border bg-white p-2.5" key={openId}>
                <span className="grid min-w-0 gap-[3px]">
                  <strong className="text-[13px] text-ink-strong">用户候选 {index + 1}</strong>
                  <OverflowTooltipText className="font-mono text-xs text-ink-muted" text={formatIdentifierPreview(openId)} logArea="settings_im_discovered_user" />
                </span>
                <Button variant="ghost" size="compact" onClick={() => onAllowDiscoveredFeishuUser(openId)}>
                  允许用户
                </Button>
              </div>
            ))}
            {feishu.discoveredChatIds.map((chatId, index) => (
              <div className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-2.5 rounded-[7px] border border-border bg-white p-2.5" key={chatId}>
                <span className="grid min-w-0 gap-[3px]">
                  <strong className="text-[13px] text-ink-strong">群候选 {index + 1}</strong>
                  <OverflowTooltipText className="font-mono text-xs text-ink-muted" text={formatIdentifierPreview(chatId)} logArea="settings_im_discovered_chat" />
                </span>
                <Button variant="ghost" size="compact" onClick={() => onAllowDiscoveredFeishuChat(chatId)}>
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
          <span>允许用户 open_id</span>
          <textarea
            className={fieldTextareaClassName}
            value={feishu.allowedUserOpenIds.join("\n")}
            onChange={(event) => onFeishuDraftChange("allowedUserOpenIds", onParseMultilineIds(event.target.value))}
            rows={4}
            placeholder="ou_xxx，每行一个"
          />
        </label>
        <label className={cn(fieldLabelClassName, "col-span-full")}>
          <span>允许群 chat_id</span>
          <textarea
            className={fieldTextareaClassName}
            value={feishu.allowedChatIds.join("\n")}
            onChange={(event) => onFeishuDraftChange("allowedChatIds", onParseMultilineIds(event.target.value))}
            rows={4}
            placeholder="oc_xxx，每行一个；私聊可留空"
          />
        </label>
      </div>

      <SettingsPolicyRow icon={<MessageCircle size={16} />}>
        网关：{feishuGatewayStatus?.running ? "运行中" : "未运行"} / 连接：
        {feishuGatewayStatus?.connected ? "已收到事件" : "未确认"} / 平台：{feishuGatewayStatus?.domain ?? feishu.config.domain}
      </SettingsPolicyRow>
      <p className="m-0 text-[13px] text-ink-muted">
        待确认改动使用飞书审批卡片。请在飞书开发者后台启用长连接，并订阅 <code>im.message.receive_v1</code> 和 <code>card.action.trigger</code>，完成消息相关权限授权后再启动网关。
      </p>
      {feishuGatewayStatus?.lastError ? <p className="m-0 text-[13px] text-ink-muted">{feishuGatewayStatus.lastError}</p> : null}
    </section>
  );
}

/** 设置页只展示飞书 ID 的短预览；完整 ID 保留在本地输入框和持久化配置中。 */
function formatIdentifierPreview(value: string) {
  const trimmed = value.trim();

  if (trimmed.length <= 12) {
    return trimmed || "未命名对象";
  }

  return `${trimmed.slice(0, 6)}...${trimmed.slice(-4)}`;
}
