import { useEffect, useState } from "react";
import { ChevronDown, ChevronRight, Info, Loader2, Pencil, Plus, RotateCw, ShieldCheck, Trash2, X } from "lucide-react";
import { Button } from "../../shared/Button";
import { Checkbox } from "../../shared/Checkbox";
import { cn } from "../../shared/cn";
import { ConfirmDialog } from "../../shared/ConfirmDialog";
import { ModalBackdrop, ModalPanel } from "../../shared/Modal";
import { SegmentedControl, SegmentedControlItem } from "../../shared/SegmentedControl";
import { ToggleRow } from "../../shared/ToggleRow";
import {
  loadMcpOverview,
  reloadMcpServers,
  removeMcpServer,
  saveMcpServer,
  setMcpServerEnabled,
  testMcpServer,
} from "../../shared/api/mcp";
import {
  fieldControlClassName,
  fieldLabelClassName,
  fieldTextareaClassName,
  settingsCardClassName,
  settingsSectionClassName,
} from "../../shared/ui";
import type { McpOverview, McpProbeResult, McpServerDraft, McpServerView, McpTransportKind } from "../../shared/types";
import { SettingsPolicyRow, SettingsSectionHeader } from "../SettingsChrome";

/** 连接状态徽标的文案与配色，和 Rust Slot.state 的四种取值一一对应。 */
const STATE_BADGE: Record<string, { label: string; className: string }> = {
  ready: { label: "已连接", className: "bg-success-soft text-success" },
  connecting: { label: "连接中", className: "bg-warning-soft text-warning" },
  disabled: { label: "已停用", className: "bg-surface-muted text-ink-muted" },
  failed: { label: "连接失败", className: "bg-danger-soft text-danger" },
};

/** 状态徽标；未知状态按中性色兜底，避免后端新增状态时界面出错。 */
function StateBadge({ state }: { state: string }) {
  const badge = STATE_BADGE[state] ?? { label: state, className: "bg-surface-muted text-ink-muted" };

  return <span className={cn("inline-flex items-center rounded-full px-2 py-0.5 text-[11px] font-medium", badge.className)}>{badge.label}</span>;
}

/** 传输类型的小标签。 */
function TransportBadge({ transport }: { transport: string }) {
  return (
    <span className="inline-flex items-center rounded-full border border-border px-2 py-0.5 text-[11px] font-medium text-ink-muted">
      {transport === "http" ? "HTTP" : "stdio"}
    </span>
  );
}

/** 空草稿；新增入口和切换传输类型时复用。 */
function emptyDraft(): McpServerDraft {
  return {
    name: "",
    enabled: true,
    transport: "stdio",
    command: "",
    argsText: "",
    url: "",
    cwd: "",
    timeoutSeconds: 60,
    envText: "",
    headersText: "",
    disabledTools: [],
  };
}

/** 从现有服务器视图回填编辑草稿；env/headers 不回显（后端出于安全不返回），保存会覆盖。 */
function draftFromServer(server: McpServerView): McpServerDraft {
  return {
    name: server.name,
    enabled: server.enabled,
    transport: server.transport === "http" ? "http" : "stdio",
    command: server.command,
    argsText: server.args.join("\n"),
    url: server.url,
    cwd: server.cwd,
    timeoutSeconds: server.timeoutSeconds,
    envText: "",
    headersText: "",
    disabledTools: server.tools.filter((tool) => !tool.exposed).map((tool) => tool.rawName),
  };
}

/** 服务器摘要行：stdio 显示命令行，HTTP 显示地址。 */
function serverTarget(server: McpServerView): string {
  return server.transport === "http" ? server.url : [server.command, ...server.args].join(" ");
}

/** 编辑弹窗的表单状态。 */
interface EditSession {
  draft: McpServerDraft;
  /** 编辑既有服务器时锁定名称，且令牌允许留空保持原值。 */
  isExisting: boolean;
  hasAuthorization: boolean;
  tokenInput: string;
  clearToken: boolean;
}

/** MCP 服务设置分区：服务器列表、编辑表单、连接测试和确认状态说明。 */
export function McpSettingsSection({ isBusy }: { isBusy: boolean }) {
  const [overview, setOverview] = useState<McpOverview | null>(null);
  const [loadError, setLoadError] = useState("");
  const [busy, setBusy] = useState(false);
  /** 正在单独切换启用状态或测试连接的服务器名。 */
  const [rowBusyName, setRowBusyName] = useState("");
  const [probeResults, setProbeResults] = useState<Record<string, McpProbeResult>>({});
  const [expandedNames, setExpandedNames] = useState<Record<string, boolean>>({});
  const [pendingDelete, setPendingDelete] = useState<McpServerView | null>(null);
  const [editSession, setEditSession] = useState<EditSession | null>(null);
  const [formError, setFormError] = useState("");

  useEffect(() => {
    let cancelled = false;
    void loadMcpOverview()
      .then((next) => {
        if (!cancelled) {
          setOverview(next);
        }
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          setLoadError(error instanceof Error ? error.message : String(error));
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const locked = isBusy || busy;

  /** 概览刷新统一入口；失败只提示，不清空已有列表。 */
  async function runRefresh(action: () => Promise<McpOverview>) {
    setBusy(true);
    setLoadError("");
    try {
      setOverview(await action());
    } catch (error: unknown) {
      setLoadError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  }

  async function handleToggleEnabled(server: McpServerView, enabled: boolean) {
    setRowBusyName(server.name);
    setLoadError("");
    try {
      setOverview(await setMcpServerEnabled(server.name, enabled));
    } catch (error: unknown) {
      setLoadError(error instanceof Error ? error.message : String(error));
    } finally {
      setRowBusyName("");
    }
  }

  async function handleTest(server: McpServerView) {
    setRowBusyName(server.name);
    setLoadError("");
    try {
      const result = await testMcpServer(server.name);
      setProbeResults((previous) => ({ ...previous, [server.name]: result }));
    } catch (error: unknown) {
      setProbeResults((previous) => ({
        ...previous,
        [server.name]: {
          serverName: server.name,
          ok: false,
          message: error instanceof Error ? error.message : String(error),
          toolNames: [],
        },
      }));
    } finally {
      setRowBusyName("");
    }
  }

  async function handleDelete(server: McpServerView) {
    setPendingDelete(null);
    await runRefresh(() => removeMcpServer(server.name));
  }

  async function handleSaveDraft() {
    if (!editSession) {
      return;
    }
    const draft = editSession.draft;
    // 令牌三态：清除复选框→空串；输入了新值→覆盖；两者都没有→undefined 保持原样。
    const authorizationToken = editSession.clearToken
      ? ""
      : editSession.tokenInput.trim().length > 0
        ? editSession.tokenInput
        : undefined;
    const nextDraft: McpServerDraft = {
      ...draft,
      name: draft.name.trim(),
      authorizationToken,
    };
    setBusy(true);
    setFormError("");
    try {
      setOverview(await saveMcpServer(nextDraft));
      setEditSession(null);
    } catch (error: unknown) {
      setFormError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className={settingsSectionClassName} aria-labelledby="mcp-settings-title">
      <SettingsSectionHeader
        kicker="MCP"
        title="MCP 服务"
        titleId="mcp-settings-title"
        description="接入 MCP 工具服务器，扩展 Agent 能力。工具只在进阶级或完全级的本地会话中可用。"
        actions={
          <>
            <Button variant="ghost" size="compact" type="button" disabled={locked} onClick={() => void runRefresh(reloadMcpServers)}>
              <RotateCw size={14} />
              重新加载
            </Button>
            <Button
              variant="primary"
              size="compact"
              type="button"
              disabled={locked}
              onClick={() => {
                setFormError("");
                setEditSession({ draft: emptyDraft(), isExisting: false, hasAuthorization: false, tokenInput: "", clearToken: false });
              }}
            >
              <Plus size={14} />
              添加服务器
            </Button>
          </>
        }
      />

      <SettingsPolicyRow icon={<Info size={14} />}>
        配置保存在 <code className="font-mono">{overview?.configPath ?? "~/.orange/mcp.json"}</code>
        ，本页面会自动读写。stdio 服务器由本机启动进程通信；HTTP 服务器通过 streamable HTTP 连接，环境变量值支持 <code className="font-mono">{"${VAR}"}</code> 插值。
      </SettingsPolicyRow>

      {loadError ? <p className="m-0 rounded-xl border border-danger/30 bg-danger-soft p-3 text-[13px] text-danger">{loadError}</p> : null}

      {overview && overview.issues.length > 0 ? (
        <div className="grid gap-2">
          {overview.issues.map((issue) => (
            <p key={issue.serverName} className="m-0 rounded-xl border border-warning/35 bg-warning-soft p-3 text-[13px] leading-[1.55] text-warning">
              <strong>{issue.serverName}</strong>：{issue.message}
            </p>
          ))}
        </div>
      ) : null}

      {overview && overview.servers.length === 0 ? (
        <p className="m-0 rounded-xl border border-dashed border-border p-6 text-center text-[13px] text-ink-muted">
          还没有 MCP 服务器。点「添加服务器」接入第一个工具服务。
        </p>
      ) : null}

      {overview?.servers.map((server) => {
        const probe = probeResults[server.name];
        const expanded = expandedNames[server.name] ?? false;
        const rowLocked = locked || rowBusyName.length > 0;
        return (
          <article key={server.name} className={cn(settingsCardClassName, "content-start")}>
            <div className="flex min-w-0 flex-wrap items-center gap-2">
              <h4 className="m-0 min-w-0 text-sm font-bold text-ink-strong [overflow-wrap:anywhere]">{server.name}</h4>
              <TransportBadge transport={server.transport} />
              <StateBadge state={server.state} />
              {server.hasAuthorization ? (
                <span className="inline-flex items-center gap-1 rounded-full bg-surface-muted px-2 py-0.5 text-[11px] font-medium text-ink-muted">
                  <ShieldCheck size={11} />
                  已存令牌
                </span>
              ) : null}
              <span className="ml-auto text-[11px] text-ink-soft">超时 {server.timeoutSeconds}s</span>
              <ToggleRow
                compact
                checked={server.enabled}
                disabled={rowLocked || rowBusyName === server.name}
                onChange={(checked) => void handleToggleEnabled(server, checked)}
                className="shrink-0"
              >
                启用
              </ToggleRow>
            </div>

            <code className="block min-w-0 truncate font-mono text-xs text-ink-muted" title={serverTarget(server)}>
              {serverTarget(server)}
            </code>

            {server.error ? <p className="m-0 rounded-lg bg-danger-soft p-2.5 text-xs leading-[1.55] text-danger [overflow-wrap:anywhere]">{server.error}</p> : null}
            {server.state === "failed" && server.stderrTail ? (
              <pre className="m-0 max-h-32 overflow-auto rounded-lg bg-surface-muted p-2.5 font-mono text-[11px] leading-[1.5] whitespace-pre-wrap text-ink-muted [overflow-wrap:anywhere]">
                {server.stderrTail}
              </pre>
            ) : null}

            <div className="flex min-w-0 flex-wrap items-center gap-1.5">
              <Button variant="ghost" size="compact" type="button" disabled={rowLocked} onClick={() => void handleTest(server)}>
                {rowBusyName === server.name ? <Loader2 size={13} className="animate-spin" /> : null}
                测试连接
              </Button>
              <Button
                variant="ghost"
                size="compact"
                type="button"
                disabled={rowLocked}
                onClick={() => {
                  setFormError("");
                  setEditSession({
                    draft: draftFromServer(server),
                    isExisting: true,
                    hasAuthorization: server.hasAuthorization,
                    tokenInput: "",
                    clearToken: false,
                  });
                }}
              >
                <Pencil size={13} />
                编辑
              </Button>
              <Button variant="ghost" size="compact" tone="danger" type="button" disabled={rowLocked} onClick={() => setPendingDelete(server)}>
                <Trash2 size={13} />
                删除
              </Button>
              {server.tools.length > 0 ? (
                <Button
                  variant="text"
                  size="compact"
                  type="button"
                  className="ml-auto"
                  onClick={() => setExpandedNames((previous) => ({ ...previous, [server.name]: !(previous[server.name] ?? false) }))}
                >
                  {expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                  工具 {server.exposedToolCount}/{server.tools.length}
                </Button>
              ) : (
                <span className="ml-auto text-[11px] text-ink-soft">{server.enabled ? "未发现工具" : "启用后显示工具列表"}</span>
              )}
            </div>

            {probe ? (
              <p className={cn("m-0 text-xs leading-[1.55]", probe.ok ? "text-success" : "text-danger")}>
                {probe.ok ? `✓ ${probe.message}` : `✗ ${probe.message}`}
                {probe.ok && probe.toolNames.length > 0 ? `（${probe.toolNames.join("、")}）` : ""}
              </p>
            ) : null}

            {expanded && server.tools.length > 0 ? (
              <ul className="m-0 grid min-w-0 list-none gap-1.5 border-t border-border pt-2.5">
                {server.tools.map((tool) => (
                  <li key={tool.publicName} className="grid gap-0.5">
                    <div className="flex min-w-0 flex-wrap items-center gap-1.5">
                      <code className={cn("min-w-0 font-mono text-xs text-ink [overflow-wrap:anywhere]", !tool.exposed && "text-ink-soft line-through")}>
                        {tool.rawName}
                      </code>
                      {tool.readOnly ? (
                        <span className="rounded-full bg-success-soft px-1.5 py-0.5 text-[10px] font-medium text-success">只读</span>
                      ) : (
                        <span className="rounded-full bg-warning-soft px-1.5 py-0.5 text-[10px] font-medium text-warning">需确认</span>
                      )}
                      {!tool.exposed ? <span className="rounded-full bg-surface-muted px-1.5 py-0.5 text-[10px] font-medium text-ink-soft">已屏蔽</span> : null}
                    </div>
                    {tool.description ? <p className="m-0 text-[11px] leading-[1.5] text-ink-muted">{tool.description}</p> : null}
                  </li>
                ))}
              </ul>
            ) : null}
          </article>
        );
      }) ?? null}

      {editSession ? (
        <ModalBackdrop onClose={() => setEditSession(null)}>
          <ModalPanel
            aria-label={editSession.isExisting ? `编辑 MCP 服务器 ${editSession.draft.name}` : "添加 MCP 服务器"}
            className="w-[min(560px,calc(100vw-40px))] max-h-[calc(100vh-48px)] gap-4 overflow-y-auto p-4 max-[760px]:p-3"
          >
            <header className="flex items-start justify-between gap-3 border-b border-border pb-3">
              <div className="min-w-0">
                <h3 className="m-0 text-base font-semibold text-ink-strong">{editSession.isExisting ? "编辑服务器" : "添加 MCP 服务器"}</h3>
                <p className="mt-1 mb-0 text-xs text-ink-muted">保存后立即写入配置并重连已启用的服务器。</p>
              </div>
              <Button variant="icon" type="button" aria-label="关闭" onClick={() => setEditSession(null)}>
                <X size={16} />
              </Button>
            </header>

            <label className={fieldLabelClassName}>
              服务器名
              <input
                className={fieldControlClassName}
                value={editSession.draft.name}
                disabled={editSession.isExisting || locked}
                placeholder="例如 filesystem"
                onChange={(event) => setEditSession({ ...editSession, draft: { ...editSession.draft, name: event.target.value } })}
              />
              <span className="text-[11px] font-normal text-ink-soft">只能包含字母、数字、_ 和 -，保存后不能改名。</span>
            </label>

            <div className="grid gap-2">
              <span className="text-xs font-bold text-ink-muted">连接方式</span>
              <SegmentedControl aria-label="MCP 连接方式" role="radiogroup">
                {(["stdio", "http"] as McpTransportKind[]).map((transport) => (
                  <SegmentedControlItem
                    key={transport}
                    active={editSession.draft.transport === transport}
                    aria-pressed={editSession.draft.transport === transport}
                    disabled={locked}
                    onClick={() => setEditSession({ ...editSession, draft: { ...editSession.draft, transport } })}
                  >
                    {transport === "stdio" ? "本机命令 (stdio)" : "远程地址 (HTTP)"}
                  </SegmentedControlItem>
                ))}
              </SegmentedControl>
            </div>

            {editSession.draft.transport === "stdio" ? (
              <>
                <label className={fieldLabelClassName}>
                  命令
                  <input
                    className={fieldControlClassName}
                    value={editSession.draft.command}
                    disabled={locked}
                    placeholder="例如 npx、node 或可执行文件绝对路径"
                    onChange={(event) => setEditSession({ ...editSession, draft: { ...editSession.draft, command: event.target.value } })}
                  />
                </label>
                <label className={fieldLabelClassName}>
                  参数（每行一个）
                  <textarea
                    className={fieldTextareaClassName}
                    value={editSession.draft.argsText}
                    disabled={locked}
                    placeholder={"-y\n@modelcontextprotocol/server-filesystem\nD:\\data"}
                    onChange={(event) => setEditSession({ ...editSession, draft: { ...editSession.draft, argsText: event.target.value } })}
                  />
                </label>
                <label className={fieldLabelClassName}>
                  工作目录（可选）
                  <input
                    className={fieldControlClassName}
                    value={editSession.draft.cwd}
                    disabled={locked}
                    placeholder="服务器进程的启动目录"
                    onChange={(event) => setEditSession({ ...editSession, draft: { ...editSession.draft, cwd: event.target.value } })}
                  />
                </label>
                <label className={fieldLabelClassName}>
                  环境变量（每行 KEY=VALUE）
                  <textarea
                    className={fieldTextareaClassName}
                    value={editSession.draft.envText}
                    disabled={locked}
                    placeholder={"API_TOKEN=${MY_API_TOKEN}"}
                    onChange={(event) => setEditSession({ ...editSession, draft: { ...editSession.draft, envText: event.target.value } })}
                  />
                  <span className="text-[11px] font-normal text-ink-soft">值支持 {"${ENV}"} 插值；密钥建议用插值，不要明文写入。</span>
                </label>
              </>
            ) : (
              <>
                <label className={fieldLabelClassName}>
                  服务器地址
                  <input
                    className={fieldControlClassName}
                    value={editSession.draft.url}
                    disabled={locked}
                    placeholder="https://example.com/mcp"
                    onChange={(event) => setEditSession({ ...editSession, draft: { ...editSession.draft, url: event.target.value } })}
                  />
                </label>
                <label className={fieldLabelClassName}>
                  请求头（每行 KEY=VALUE）
                  <textarea
                    className={fieldTextareaClassName}
                    value={editSession.draft.headersText}
                    disabled={locked}
                    placeholder={"X-API-KEY=${MY_TOKEN}"}
                    onChange={(event) => setEditSession({ ...editSession, draft: { ...editSession.draft, headersText: event.target.value } })}
                  />
                </label>
                <label className={fieldLabelClassName}>
                  Bearer 令牌（可选）
                  <input
                    className={fieldControlClassName}
                    type="password"
                    autoComplete="off"
                    value={editSession.tokenInput}
                    disabled={locked}
                    placeholder={editSession.hasAuthorization ? "已保存，留空保持不变" : "存入系统钥匙串，不写入配置文件"}
                    onChange={(event) => setEditSession({ ...editSession, tokenInput: event.target.value })}
                  />
                </label>
                {editSession.hasAuthorization ? (
                  <label className="flex items-center gap-2 text-xs text-ink">
                    <Checkbox
                      checked={editSession.clearToken}
                      disabled={locked}
                      onChange={(event) => setEditSession({ ...editSession, clearToken: event.target.checked })}
                    />
                    清除已保存的令牌
                  </label>
                ) : null}
              </>
            )}

            <label className={fieldLabelClassName}>
              单次调用超时（秒）
              <input
                className={fieldControlClassName}
                type="number"
                min={1}
                max={300}
                value={Number.isFinite(editSession.draft.timeoutSeconds) ? editSession.draft.timeoutSeconds : ""}
                disabled={locked}
                onChange={(event) =>
                  setEditSession({ ...editSession, draft: { ...editSession.draft, timeoutSeconds: Number.parseInt(event.target.value, 10) || 0 } })
                }
              />
            </label>

            <ToggleRow checked={editSession.draft.enabled} disabled={locked} onChange={(checked) => setEditSession({ ...editSession, draft: { ...editSession.draft, enabled: checked } })}>
              保存后立即启用
            </ToggleRow>

            {editSession.isExisting ? (
              <p className="m-0 rounded-lg bg-warning-soft p-2.5 text-[11px] leading-[1.55] text-warning">
                保存会覆盖这台服务器的配置。出于安全，已保存的环境变量和请求头不会显示在这里；如需保留请重新填写。
              </p>
            ) : null}

            {formError ? <p className="m-0 text-xs text-danger">{formError}</p> : null}

            <footer className="flex items-center justify-end gap-2 border-t border-border pt-3">
              <Button variant="ghost" type="button" onClick={() => setEditSession(null)}>
                取消
              </Button>
              <Button variant="primary" type="button" disabled={locked} onClick={() => void handleSaveDraft()}>
                {busy ? <Loader2 size={14} className="animate-spin" /> : null}
                保存
              </Button>
            </footer>
          </ModalPanel>
        </ModalBackdrop>
      ) : null}

      {pendingDelete ? (
        <ConfirmDialog
          title={`删除 MCP 服务器 ${pendingDelete.name}？`}
          message="配置条目和已保存的访问令牌会一并移除，此操作不可撤销。"
          confirmLabel="删除"
          tone="danger"
          isBusy={busy}
          onCancel={() => setPendingDelete(null)}
          onConfirm={() => void handleDelete(pendingDelete)}
        />
      ) : null}
    </section>
  );
}
