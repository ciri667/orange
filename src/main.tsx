import { Component, StrictMode, type ErrorInfo, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { logError } from "./shared/logger";
import "./styles/app.css";

/** 根错误边界，捕获 React 渲染异常并写入前端诊断日志。 */
class RootErrorBoundary extends Component<{ children: ReactNode }, { hasError: boolean }> {
  /** 错误边界状态只用于替换崩溃 UI，避免继续渲染损坏组件树。 */
  state = { hasError: false };

  /** React 捕获渲染异常后记录脱敏错误和组件栈。 */
  componentDidCatch(error: Error, errorInfo: ErrorInfo) {
    this.setState({ hasError: true });
    logError("React 渲染异常。", {
      category: "frontend",
      event: "react_error_boundary",
      status: "failed",
      error,
      metadata: {
        componentStack: errorInfo.componentStack ?? undefined,
      },
    });
  }

  /** 渲染应用或兜底错误界面，避免异常后显示空白窗口。 */
  render() {
    if (this.state.hasError) {
      return (
        <main className="loading-shell boot-error-shell">
          <p>界面渲染失败</p>
          <p className="boot-error-message">请重启 Cici Note 后重试。</p>
        </main>
      );
    }

    return this.props.children;
  }
}

/** 捕获浏览器全局错误，覆盖 React 生命周期之外的异常。 */
window.addEventListener("error", (event) => {
  logError("前端全局错误。", {
    category: "frontend",
    event: "window_error",
    status: "failed",
    error: event.error ?? event.message,
  });
});

/** 捕获未处理 Promise rejection，避免异步错误只停留在 DevTools。 */
window.addEventListener("unhandledrejection", (event) => {
  logError("前端未处理异步错误。", {
    category: "frontend",
    event: "unhandled_rejection",
    status: "failed",
    error: event.reason,
  });
});

/** React 应用挂载节点，承载正式桌面端知识库 Agent 工作台。 */
const rootElement = document.getElementById("root");

// 如果 HTML 入口缺少挂载节点，直接抛错可以更早暴露构建或模板问题。
if (!rootElement) {
  throw new Error("Root element #root was not found.");
}

createRoot(rootElement).render(
  <StrictMode>
    <RootErrorBoundary>
      <App />
    </RootErrorBoundary>
  </StrictMode>,
);
