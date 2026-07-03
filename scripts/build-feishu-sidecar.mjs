import { chmodSync, mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

/** 项目根目录，脚本从 package.json 所在目录运行。 */
const projectRoot = resolve(new URL("..", import.meta.url).pathname);
/** 飞书 sidecar Go 模块目录，依赖官方 SDK 接收长连接事件。 */
const sidecarSourceDir = join(projectRoot, "src-tauri", "sidecars", "feishu-gateway");
/** Rust 开发态查找的 sidecar 二进制输出目录。 */
const sidecarOutputDir = join(projectRoot, "src-tauri", "sidecars", "bin");
/** 当前平台的 sidecar 文件名，Windows 需要 .exe 后缀。 */
const sidecarFileName = process.platform === "win32" ? "feishu-gateway.exe" : "feishu-gateway";
/** 最终输出路径；不要放在源码目录同名位置，避免目录和二进制混淆。 */
const sidecarOutputPath = join(sidecarOutputDir, sidecarFileName);

/** 运行外部命令，并把 stdout/stderr 直接透传给调用者。 */
function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: sidecarSourceDir,
    stdio: "inherit",
    ...options,
  });

  if (result.error) {
    throw result.error;
  }

  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(" ")} exited with ${result.status}`);
  }
}

try {
  mkdirSync(sidecarOutputDir, { recursive: true });
  // 先显式检查 Go，错误信息比 spawn ENOENT 更容易理解。
  run("go", ["version"]);
  // 整理模块依赖并生成 go.sum；Go 会拒绝缺少校验条目的 SDK 依赖。
  run("go", ["mod", "tidy"]);
  run("go", ["build", "-trimpath", "-o", sidecarOutputPath, "."]);

  if (process.platform !== "win32") {
    chmodSync(sidecarOutputPath, 0o755);
  }

  console.log(`Feishu sidecar built: ${sidecarOutputPath}`);
} catch (error) {
  const message = error instanceof Error ? error.message : String(error);

  console.error(`Failed to build Feishu sidecar: ${message}`);
  console.error("Please make sure Go 1.22+ is installed and network access to Go modules is available, then rerun: npm run sidecar:feishu:build");
  process.exit(1);
}
