import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import "./styles/app.css";

/** React 应用挂载节点，承载正式桌面端知识库 Agent 工作台。 */
const rootElement = document.getElementById("root");

// 如果 HTML 入口缺少挂载节点，直接抛错可以更早暴露构建或模板问题。
if (!rootElement) {
  throw new Error("Root element #root was not found.");
}

createRoot(rootElement).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
