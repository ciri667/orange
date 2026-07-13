import { spawnSync } from "node:child_process";

/** macOS 发布包所需的显式正式签名身份环境变量名称。 */
const RELEASE_SIGNING_IDENTITY_ENV = "ORANGE_RELEASE_SIGNING_IDENTITY";

/** 启动 Tauri 打包，并确保 macOS 绝不回退为 ad-hoc 签名。 */
function runDesktopBuild() {
  const signingIdentity = process.env[RELEASE_SIGNING_IDENTITY_ENV]?.trim();
  const args = ["build"];

  if (process.platform === "darwin") {
    if (!signingIdentity) {
      console.error(
        `[release-signing] level=error event=missing_identity environment=${RELEASE_SIGNING_IDENTITY_ENV}`,
      );
      console.error(`Set ${RELEASE_SIGNING_IDENTITY_ENV} to a valid macOS code-signing identity before running npm run desktop:build.`);
      process.exit(1);
    }

    // 仅把签名身份传给 Tauri 配置覆盖；日志不回显身份字符串。
    args.push("--config", JSON.stringify({ bundle: { macOS: { signingIdentity } } }));
    console.info("[release-signing] level=info event=identity_configured platform=darwin identityConfigured=true");
  } else {
    console.info(`[release-signing] level=info event=skipped platform=${process.platform}`);
  }

  args.push(...process.argv.slice(2));
  const result = spawnSync("tauri", args, { stdio: "inherit" });

  if (result.error) {
    throw result.error;
  }

  process.exit(result.status ?? 1);
}

try {
  runDesktopBuild();
} catch (error) {
  const message = error instanceof Error ? error.message : String(error);

  console.error(`[release-signing] level=error event=build_failed message=${message}`);
  process.exit(1);
}
