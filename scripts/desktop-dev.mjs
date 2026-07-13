import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";
import { ensureDevelopmentSigningIdentity } from "./macos-dev-signing-init.mjs";

/** 当前 Node 脚本所在目录，用于生成不受 Tauri 工作目录影响的 runner 绝对路径。 */
const scriptsDirectory = resolve(fileURLToPath(new URL(".", import.meta.url)));

/** 根据宿主 CPU 架构选择 Cargo 识别的 macOS target runner 环境变量。 */
function cargoRunnerEnvironmentName() {
  if (process.arch === "arm64") {
    return "CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER";
  }

  if (process.arch === "x64") {
    return "CARGO_TARGET_X86_64_APPLE_DARWIN_RUNNER";
  }

  throw new Error(`unsupported macOS development architecture: ${process.arch}`);
}

/** 启动 Tauri 开发模式；仅 macOS 注入一次性 Cargo runner，避免影响普通 Cargo 测试。 */
function runDesktopDevelopment() {
  const environment = { ...process.env };

  if (process.platform === "darwin") {
    ensureDevelopmentSigningIdentity();

    const runnerEnvironment = cargoRunnerEnvironmentName();
    const runnerPath = resolve(scriptsDirectory, "macos-dev-cargo-runner.sh");

    // 使用绝对路径防止 Tauri 切换到 src-tauri 工作目录后无法定位 runner。
    environment[runnerEnvironment] = runnerPath;
    console.info(`[dev-signing] level=info event=cargo_runner_configured platform=darwin environment=${runnerEnvironment}`);
  } else {
    console.info(`[dev-signing] level=info event=cargo_runner_skipped platform=${process.platform}`);
  }

  const result = spawnSync("tauri", ["dev", ...process.argv.slice(2)], {
    env: environment,
    stdio: "inherit",
  });

  if (result.error) {
    throw result.error;
  }

  process.exit(result.status ?? 1);
}

try {
  runDesktopDevelopment();
} catch (error) {
  const message = error instanceof Error ? error.message : String(error);

  console.error(`[dev-signing] level=error event=desktop_dev_failed message=${message}`);
  console.error("Run npm run dev:signing:init, then restart npm run desktop:dev.");
  process.exit(1);
}
