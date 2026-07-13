import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { DEVELOPMENT_SIGNING_IDENTITY, hasDevelopmentSigningIdentity } from "./macos-dev-signing-init.mjs";

/** 开发态二进制的稳定 bundle identifier，和生产应用隔离。 */
const DEVELOPMENT_BUNDLE_IDENTIFIER = "app.orange.desktop.dev";

/** 输出结构化的开发签名日志，避免泄漏钥匙串内容或业务密钥。 */
function log(level, event, extra = "") {
  console[level](`[dev-signing] level=${level} event=${event}${extra ? ` ${extra}` : ""}`);
}

/** 执行命令并继承当前终端 I/O，让 Tauri 开发日志保持可见。 */
function run(command, args) {
  const result = spawnSync(command, args, { stdio: "inherit" });

  if (result.error) {
    throw result.error;
  }

  if (result.status !== 0) {
    throw new Error(`${command} exited with status ${result.status}`);
  }
}

/** 以固定证书重签名每个刚编译的 debug 二进制，再启动实际应用。 */
function runSignedBinary() {
  const [binaryPath, ...binaryArgs] = process.argv.slice(2);

  if (!binaryPath || !existsSync(binaryPath)) {
    throw new Error("Cargo runner did not receive a valid application binary path");
  }

  if (!hasDevelopmentSigningIdentity()) {
    throw new Error("development signing identity is missing; run npm run dev:signing:init");
  }

  log("info", "resign_started", "platform=darwin identityConfigured=true");
  // 每次 Rust 编译都会生成新 Mach-O；固定证书和 identifier 令钥匙串 ACL 可持续匹配。
  run("codesign", [
    "--force",
    "--sign",
    DEVELOPMENT_SIGNING_IDENTITY,
    "--identifier",
    DEVELOPMENT_BUNDLE_IDENTIFIER,
    "--timestamp=none",
    binaryPath,
  ]);
  run("codesign", ["--verify", "--strict", "--verbose=2", binaryPath]);
  log("info", "resign_completed", "platform=darwin identifier=app.orange.desktop.dev");

  const applicationResult = spawnSync(binaryPath, binaryArgs, { stdio: "inherit" });

  if (applicationResult.error) {
    throw applicationResult.error;
  }

  // 通过 exitCode 让刚写入的完成日志先刷新，避免快速退出时丢失可观测性事件。
  return applicationResult.status ?? 1;
}

try {
  if (process.platform !== "darwin") {
    throw new Error("macOS development runner was selected on a non-macOS platform");
  }

  process.exitCode = runSignedBinary();
} catch (error) {
  const message = error instanceof Error ? error.message : String(error);

  log("error", "resign_failed", `message=${message}`);
  console.error("Run npm run dev:signing:init, then restart npm run desktop:dev.");
  process.exit(1);
}
