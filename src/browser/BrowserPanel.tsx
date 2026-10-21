import { Globe, Hand, MonitorPlay, MonitorOff, RefreshCw, X, Plus, Trash2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Button } from "../shared/Button";
import { cn } from "../shared/cn";
import { ConfirmDialog } from "../shared/ConfirmDialog";
import { BrowserStage } from "./BrowserStage";
import type {
  BrowserActivity,
  BrowserApprovalRequest,
  BrowserFrame,
  BrowserReceipt,
  BrowserState,
  BrowserUserInput,
} from "../shared/types";
import {
  browserClearSiteData,
  browserStart,
  browserCreateTab,
  browserActivateTab,
  browserUserInput,
  browserCloseTab,
  browserSetScreencast,
  browserUserNavigate,
  browserUserTakeOver,
  decideBrowserOperation,
  loadBrowserStatus,
  subscribeBrowserActivity,
  subscribeBrowserApprovals,
  subscribeBrowserFrames,
  subscribeBrowserReceipts,
  subscribeBrowserState,
} from "../shared/tauriApi";

/** 活动提示的中文名映射；未知活动原样展示。 */
const ACTIVITY_LABELS: Record<string, string> = {
  "user-takeover": "你已接管浏览器",
  "user-navigate": "你在地址栏导航",
};

/** Agent 操作的活动提示保留时长。 */
const ACTIVITY_TTL_MS = 4000;

/**
 * 浏览器面板：与 Agent 共享同一个受控 Chromium 会话。
 * 展示标签页、地址栏、实时画面、Agent 活动状态和“我来接管”按钮。
 */
export function BrowserPanel({ className, onClose }: { className?: string; onClose?: () => void }) {
  const [state, setState] = useState<BrowserState | null>(null);
  const [frame, setFrame] = useState<BrowserFrame | null>(null);
  const [activity, setActivity] = useState<BrowserActivity | null>(null);
  const [receipt, setReceipt] = useState<BrowserReceipt | null>(null);
  const [approval, setApproval] = useState<BrowserApprovalRequest | null>(null);
  const [addressInput, setAddressInput] = useState("");
  const [isScreencastOn, setIsScreencastOn] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmClear, setConfirmClear] = useState(false);
  const inputQueueRef = useRef<Promise<void>>(Promise.resolve());
  const stateRef = useRef(state);
  stateRef.current = state;
  const activityTimerRef = useRef<number | null>(null);

  const activeTab = useMemo(
    () => state?.tabs.find((tab) => tab.tabId === state.activeTabId) ?? null,
    [state],
  );

  /** 操作失败直接展示在面板，避免按钮无响应却只在日志中留下原因。 */
  const reportError = useCallback((cause: unknown) => {
    setError(cause instanceof Error ? cause.message : String(cause));
  }, []);
  const runAction = useCallback(async (action: () => Promise<BrowserState | void>) => {
    setBusy(true);
    setError(null);
    try {
      const next = await action();
      if (next) setState(next);
    } catch (cause) {
      reportError(cause);
    } finally {
      setBusy(false);
    }
  }, [reportError]);

  /** 保证点击聚焦先于文字输入，同一面板输入顺序与用户操作顺序一致。 */
  const handleInput = useCallback((input: BrowserUserInput) => {
    const tabId = stateRef.current?.visibleTabId;
    if (!tabId) return;
    inputQueueRef.current = inputQueueRef.current
      .then(() => browserUserInput(tabId, input))
      .catch(reportError);
  }, [reportError]);

  /** 初始加载一次状态，然后靠事件流保持同步。 */
  useEffect(() => {
    let disposed = false;
    const unsubscribers: Array<() => void> = [];
    const track = (promise: Promise<() => void>) => {
      void promise.then((unlisten) => {
        if (disposed) {
          unlisten();
        } else {
          unsubscribers.push(unlisten);
        }
      }).catch((cause) => { if (!disposed) reportError(cause); });
    };

    void loadBrowserStatus().then((initial) => {
      if (!disposed) {
        setState(initial);
      }
    }).catch((cause) => { if (!disposed) reportError(cause); });
    track(subscribeBrowserState(setState));
    track(
      subscribeBrowserFrames((next) => {
        // 后端已节流；首帧和标签切换后的补帧必须保留，静态页面可能不再发送第二帧。
        setFrame(next);
      }),
    );
    track(
      subscribeBrowserActivity((next) => {
        setActivity(next);
        if (activityTimerRef.current) {
          window.clearTimeout(activityTimerRef.current);
        }
        activityTimerRef.current = window.setTimeout(() => setActivity(null), ACTIVITY_TTL_MS);
      }),
    );
    track(subscribeBrowserReceipts(setReceipt));
    track(subscribeBrowserApprovals(setApproval));

    return () => {
      disposed = true;
      unsubscribers.forEach((unlisten) => unlisten());
      if (activityTimerRef.current) {
        window.clearTimeout(activityTimerRef.current);
      }
    };
  }, [reportError]);

  /** 浏览器运行后默认开启画面，后端负责标签切换与重复开启去重。 */
  useEffect(() => {
    if (!state?.available) {
      return;
    }
    void browserSetScreencast(isScreencastOn).catch(reportError);
  }, [isScreencastOn, state?.available, state?.visibleTabId, reportError]);

  useEffect(() => {
    setAddressInput(activeTab?.url === "about:blank" ? "" : activeTab?.url ?? "");
    setFrame((previous) => previous?.tabId === activeTab?.tabId ? previous : null);
  }, [activeTab?.tabId, activeTab?.url]);

  /** 读取低频状态以收录网页弹出的标签页；画面仍由事件流驱动。 */
  useEffect(() => {
    let disposed = false;
    let timer: number;
    const refresh = async () => {
      try {
        const next = await loadBrowserStatus();
        if (!disposed) setState(next);
      } catch {
        // 按钮操作会展示具体错误；轮询失败等待下一次读取。
      } finally {
        // 等上一条查询完成才安排下一条，慢页面不会积累 IPC 请求。
        if (!disposed) timer = window.setTimeout(() => { void refresh(); }, 2000);
      }
    };
    timer = window.setTimeout(() => { void refresh(); }, 2000);
    return () => {
      disposed = true;
      window.clearTimeout(timer);
      void browserSetScreencast(false).catch(() => undefined);
    };
  }, []);

  const handleNavigate = useCallback(
    (event: React.FormEvent) => {
      event.preventDefault();
      const url = addressInput.trim();
      if (!url) {
        return;
      }
      void runAction(() => browserUserNavigate(url));
    },
    [addressInput, runAction],
  );

  const handleTakeOver = useCallback(() => {
    setApproval(null);
    void runAction(() => browserUserTakeOver(state?.control.sessionId ?? null));
  }, [state?.control.sessionId, runAction]);

  const agentControlling = Boolean(state?.control.running && state?.control.sessionId);

  return (
    <aside className={cn("browser-panel", className)} aria-label="内嵌浏览器">
      <header className="browser-panel-header">
        <div className="browser-panel-title">
          <Globe size={14} aria-hidden />
          <span>内嵌浏览器</span>
        </div>
        <div className="flex items-center gap-1">
          <Button
            variant="ghost"
            size="compact"
            aria-label={isScreencastOn ? "关闭实时画面" : "开启实时画面"}
            title={isScreencastOn ? "关闭实时画面" : "开启实时画面"}
            disabled={busy}
            onClick={() => {
              const next = !isScreencastOn;
              void runAction(async () => {
                await browserSetScreencast(next);
                setIsScreencastOn(next);
                return loadBrowserStatus();
              });
            }}
          >
            {isScreencastOn ? <MonitorOff size={14} /> : <MonitorPlay size={14} />}
          </Button>
          <Button
            variant="ghost"
            size="compact"
            aria-label="刷新浏览器状态"
            title="刷新浏览器状态"
            disabled={busy}
            onClick={() => { void runAction(() => loadBrowserStatus()); }}
          >
            <RefreshCw size={14} />
          </Button>
          <Button variant="ghost" size="compact" aria-label="清除站点数据" title="清除站点数据" disabled={busy} onClick={() => setConfirmClear(true)}>
            <Trash2 size={14} />
          </Button>
          {onClose && <Button variant="icon" size="compact" aria-label="收起浏览器面板" title="收起浏览器面板" onClick={onClose}><X size={14} /></Button>}
        </div>
      </header>

      {(error || state?.launchError) && <p className="m-0 border-b border-border px-3 py-2 text-xs text-danger" role="alert">{error || state?.launchError}</p>}
      {!state?.available && (
        <div className="flex items-center justify-between gap-2 border-b border-border px-3 py-2 text-xs text-ink-muted">
          <span>{state?.chromiumInstalled ? "浏览器尚未启动" : "请在设置中下载浏览器内核"}</span>
          <Button variant="primary" size="compact" disabled={busy || !state?.chromiumInstalled} onClick={() => { void runAction(() => browserStart()); }}>
            {busy ? "启动中…" : error || state?.launchError ? "重试启动" : "启动浏览器"}
          </Button>
        </div>
      )}

      {state && state.tabs.length > 0 && (
        <div className="browser-tab-strip" role="tablist" aria-label="浏览器标签页">
          {state.tabs.map((tab) => (
            <div
              key={tab.tabId}
              className={cn("browser-tab", tab.active && "is-active")}
            >
              <button type="button" role="tab" aria-selected={tab.active} disabled={busy} className="browser-tab-title border-0 bg-transparent p-0" title={tab.title || tab.url} onClick={() => { void runAction(() => browserActivateTab(tab.tabId)); }}>
                {tab.title || (tab.url === "about:blank" ? "空白页" : tab.url) || "空白页"}
              </button>
              <button
                type="button"
                aria-label={`关闭 ${tab.title || "标签页"}`}
                className="browser-tab-close"
                disabled={busy}
                onClick={(event) => {
                  event.stopPropagation();
                  void runAction(() => browserCloseTab(tab.tabId));
                }}
              >
                <X size={10} />
              </button>
            </div>
          ))}
          <Button variant="icon" size="compact" aria-label="新建浏览器标签页" disabled={busy} onClick={() => { void runAction(() => browserCreateTab()); }}><Plus size={14} /></Button>
        </div>
      )}

      <form className="browser-address-bar" onSubmit={handleNavigate}>
        <input
          value={addressInput}
          onChange={(event) => setAddressInput(event.target.value)}
          placeholder={activeTab?.url && activeTab.url !== "about:blank" ? activeTab.url : "输入网址，回车导航"}
          aria-label="地址栏"
          spellCheck={false}
          disabled={busy || !state?.chromiumInstalled}
        />
      </form>

      <BrowserStage frame={frame?.tabId === state?.visibleTabId ? frame : null} enabled={isScreencastOn && Boolean(state?.available)} onInput={handleInput} />
      {receipt && <div className="px-3 py-1 text-xs text-ink-soft">最近操作 {receipt.operation}</div>}

      <footer className="browser-panel-footer">
        <div className="browser-status" aria-live="polite">
          {activity ? (
            <span className="browser-status-activity">
              {ACTIVITY_LABELS[activity.activity] ?? `Agent：${activity.activity}`}
            </span>
          ) : agentControlling ? (
            <span className="browser-status-agent">Agent 正在操作浏览器</span>
          ) : (
            <span className="browser-status-idle">
              {state?.available ? "浏览器空闲" : state?.enabled ? "浏览器未启动" : "浏览器未启用"}
            </span>
          )}
        </div>
        <Button
          variant="primary"
          size="compact"
          onClick={handleTakeOver}
          disabled={busy || !state?.available}
          className={cn(agentControlling && "browser-takeover-active")}
        >
          <Hand size={14} aria-hidden />
          我来接管
        </Button>
      </footer>

      {confirmClear && <ConfirmDialog
        title="清除浏览器站点数据"
        message="将关闭内嵌浏览器并清除其中的登录状态、Cookie 和缓存。下次打开网页需要重新登录。"
        confirmLabel="清除数据" tone="danger" isBusy={busy}
        onCancel={() => { if (!busy) setConfirmClear(false); }}
        onConfirm={() => { void runAction(async () => {
          await browserClearSiteData();
          setFrame(null);
          setReceipt(null);
          setConfirmClear(false);
          return loadBrowserStatus();
        }); }}
      />}

      {approval && (
        <ConfirmDialog
          title="Agent 请求执行浏览器操作"
          message={`操作：${approval.operation}${approval.elementSummary ? `；目标元素：${approval.elementSummary}` : ""}`}
          confirmLabel="允许一次"
          cancelLabel="拒绝"
          isBusy={busy}
          onCancel={() => {
            if (busy) return;
            void runAction(async () => {
              await decideBrowserOperation({ approvalId: approval.approvalId, approved: false });
              setApproval(null);
            });
          }}
          onConfirm={() => {
            void runAction(async () => {
              await decideBrowserOperation({ approvalId: approval.approvalId, approved: true });
              setApproval(null);
            });
          }}
        />
      )}
    </aside>
  );
}
