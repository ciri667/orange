import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

/** Vite 开发与构建配置，保持原型以 React 单页应用方式运行。 */
export default defineConfig({
  plugins: [react()],
});
