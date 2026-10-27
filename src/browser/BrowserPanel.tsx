import { Hand, MonitorPlay, MonitorOff, RefreshCw, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { Button } from "../shared/Button";
import { ConfirmDialog } from "../shared/ConfirmDialog";
import { browserClearSiteData, browserUserInput, browserUserNavigate, browserUserTakeOver, isTauriRuntime, loadBrowserStatus } from "../shared/tauriApi";
import type { BrowserUserInput } from "../shared/types";
import { BrowserStage } from "./BrowserStage";
import type { BrowserWorkspace } from "./useBrowserWorkspace";

/** 单网页内容；常驻控制器拥有事件订阅和审批，面板只负责当前网页交互。 */
export function BrowserPanel({ browser, tabId, active, onRetry }: { browser: BrowserWorkspace; tabId: string | null; active: boolean; onRetry: () => void }) {
  const { state, frame, activity, error, busy } = browser;
  const tab = state?.tabs.find((item) => item.tabId === tabId);
  const [address, setAddress] = useState("");
  const [confirmClear, setConfirmClear] = useState(false);
  const inputQueue = useRef(Promise.resolve());
  const currentView = useRef({ tabId, active });
  currentView.current = { tabId, active };
  useEffect(() => setAddress(tab?.url === "about:blank" ? "" : tab?.url ?? ""), [tabId, tab?.url]);
  const action = (work: () => Promise<import("../shared/types").BrowserState | void>) => void browser.runAction(work).catch(() => undefined);
  const handleInput = (input: BrowserUserInput) => {
    if (!tabId || !active) return;
    const selectedId = tabId;
    inputQueue.current = inputQueue.current.then(async () => {
      // 已排队输入不能在用户切换到另一页后到达错误的网页。
      if (currentView.current.active && currentView.current.tabId === selectedId) await browserUserInput(selectedId, input);
    }).catch((cause) => { action(async () => { throw cause; }); });
  };
  const controlling = Boolean(state?.control.running);
  return <section className="browser-panel" hidden={!active} aria-label="网页内容">
    <header className="browser-panel-header">
      <span className="text-xs text-ink-muted">{controlling ? "Agent 正在操作浏览器" : "内置浏览器"}</span>
      <div className="flex items-center gap-1">
        <Button variant="icon" size="compact" title={browser.screencast ? "关闭实时画面" : "开启实时画面"} aria-label={browser.screencast ? "关闭实时画面" : "开启实时画面"} onClick={() => browser.setScreencast(!browser.screencast)}>{browser.screencast ? <MonitorOff size={14} /> : <MonitorPlay size={14} />}</Button>
        <Button variant="icon" size="compact" title="刷新浏览器状态" aria-label="刷新浏览器状态" disabled={busy} onClick={() => action(loadBrowserStatus)}><RefreshCw size={14} /></Button>
        <Button variant="icon" size="compact" title="清除站点数据" aria-label="清除站点数据" disabled={busy} onClick={() => setConfirmClear(true)}><Trash2 size={14} /></Button>
      </div>
    </header>
    {(error || state?.launchError) && <p className="m-0 px-3 py-2 text-xs text-danger" role="alert">{error || state?.launchError}</p>}
    {!tab && <div className="flex items-center justify-between gap-2 px-3 py-2 text-xs text-ink-muted"><span>网页尚未打开{state?.chromiumInstalled ? "" : "，请先在设置中下载浏览器内核"}</span><Button size="compact" disabled={busy} onClick={onRetry}>重试打开</Button></div>}
    <form className="browser-address-bar" onSubmit={(event) => { event.preventDefault(); if (address.trim() && tab) action(() => browserUserNavigate(address.trim(), tab.tabId)); }}>
      <input value={address} onChange={(event) => setAddress(event.target.value)} placeholder="输入网址，回车导航" aria-label="地址栏" spellCheck={false} disabled={busy || !tab} />
    </form>
    {!isTauriRuntime() && <p className="m-0 px-3 py-2 text-xs text-ink-muted">网页页签预览；实时浏览器仅在桌面端可用。</p>}
    <BrowserStage frame={frame?.tabId === tabId ? frame : null} enabled={active && browser.screencast && Boolean(state?.available)} onInput={handleInput} />
    {browser.receipt?.tabId === tabId && <div className="px-3 py-1 text-xs text-ink-soft">最近操作 {browser.receipt.operation}</div>}
    <footer className="browser-panel-footer">
      <span className="browser-status" aria-live="polite">{activity ? `活动：${activity.activity}` : controlling ? "Agent 正在操作浏览器" : "浏览器空闲"}</span>
      <Button variant="primary" size="compact" disabled={busy || !state?.available} onClick={() => action(() => browserUserTakeOver(state?.control.sessionId))}><Hand size={14} />我来接管</Button>
    </footer>
    {confirmClear && <ConfirmDialog title="清除浏览器站点数据" message="将关闭所有浏览器网页并清除登录状态、Cookie 和缓存。" confirmLabel="清除数据" tone="danger" isBusy={busy} onCancel={() => setConfirmClear(false)} onConfirm={() => action(async () => { await browserClearSiteData(); setConfirmClear(false); return loadBrowserStatus(); })} />}
  </section>;
}
