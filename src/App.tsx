import { WorkspaceShell } from "./workspace/WorkspaceShell";

/** 应用根组件，正式入口只挂载知识库 Agent 助手工作台。 */
export default function App() {
  return <WorkspaceShell />;
}
