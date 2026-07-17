import { spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";

/** macOS 发布包所需的显式正式签名身份环境变量名称。 */
const RELEASE_SIGNING_IDENTITY_ENV = "ORANGE_RELEASE_SIGNING_IDENTITY";

/** 当前项目根目录；用于定位 Tauri 打包后的应用包和本地 DMG 输出目录。 */
const PROJECT_DIRECTORY = process.cwd();

/** 桌面包构建模式：本机验收使用无证书的 ad-hoc 签名，正式发布必须使用 Apple 发布证书。 */
const BUILD_MODE = {
  local: "local",
  localDmg: "local-dmg",
  release: "release",
};

/** 解析脚本专用构建模式，并保留其余参数透传给 Tauri CLI。 */
function parseBuildArguments() {
  const [modeArgument, ...tauriArguments] = process.argv.slice(2);
  const modeByArgument = {
    "--local": BUILD_MODE.local,
    "--local-dmg": BUILD_MODE.localDmg,
    "--release": BUILD_MODE.release,
  };
  const buildMode = modeByArgument[modeArgument];

  if (!buildMode) {
    throw new Error("missing build mode; use npm run desktop:build, npm run desktop:build:dmg, or npm run desktop:build:release");
  }

  return { buildMode, tauriArguments };
}

/** 根据构建模式注入 macOS 签名配置，且日志中绝不输出证书身份字符串。 */
function configureMacOSSigning(args, buildMode) {
  if (process.platform !== "darwin") {
    console.info(`[desktop-build] level=info event=signing_skipped platform=${process.platform} mode=${buildMode}`);
    return;
  }

  if (buildMode === BUILD_MODE.local || buildMode === BUILD_MODE.localDmg) {
    // 不传 signingIdentity 时，Tauri 使用不依赖证书的 ad-hoc 签名，避免创建或读取钥匙串证书。
    args.push("--bundles", "app");
    const artifact = buildMode === BUILD_MODE.localDmg ? "dmg" : "app";

    // 本地 DMG 由 hdiutil 在应用包完成后生成，避免 Tauri 的 Finder 自动化步骤影响无界面环境。
    console.info(`[desktop-build] level=info event=local_adhoc_signing platform=darwin certificateRequired=false bundleTarget=app artifact=${artifact}`);
    return;
  }

  const signingIdentity = process.env[RELEASE_SIGNING_IDENTITY_ENV]?.trim();

  if (!signingIdentity) {
    console.error(
      `[desktop-build] level=error event=missing_release_identity environment=${RELEASE_SIGNING_IDENTITY_ENV}`,
    );
    console.error(`Set ${RELEASE_SIGNING_IDENTITY_ENV} to a valid macOS code-signing identity before running npm run desktop:build:release.`);
    process.exit(1);
  }

  // 仅把正式签名身份传给 Tauri 配置覆盖；日志不回显身份字符串或其他钥匙串信息。
  args.push("--config", JSON.stringify({ bundle: { macOS: { signingIdentity } } }));
  console.info("[desktop-build] level=info event=release_identity_configured platform=darwin identityConfigured=true");
}

/** 读取产品配置并构造当前 CPU 架构对应的本地 DMG 输出路径。 */
function localDmgPaths() {
  const tauriConfigPath = resolve(PROJECT_DIRECTORY, "src-tauri", "tauri.conf.json");
  const tauriConfig = JSON.parse(readFileSync(tauriConfigPath, "utf8"));
  const productName = tauriConfig.productName;
  const version = tauriConfig.version;

  if (typeof productName !== "string" || typeof version !== "string") {
    throw new Error("invalid productName or version in src-tauri/tauri.conf.json");
  }

  const architecture = process.arch === "arm64" ? "aarch64" : process.arch;
  const bundleDirectory = resolve(PROJECT_DIRECTORY, "src-tauri", "target", "release", "bundle");
  const appPath = join(bundleDirectory, "macos", `${productName}.app`);
  const dmgPath = join(bundleDirectory, "dmg", `${productName}_${version}_${architecture}.dmg`);

  return { appPath, dmgPath, productName };
}

/** 使用系统 hdiutil 从已完成的 .app 生成简洁的本地测试 DMG，不依赖 Finder 或签名证书。 */
function createLocalDmg() {
  if (process.platform !== "darwin") {
    throw new Error("local DMG creation is only supported on macOS");
  }

  const { appPath, dmgPath, productName } = localDmgPaths();

  if (!existsSync(appPath)) {
    throw new Error("local app bundle is missing after Tauri build");
  }

  // Tauri 的 app-only 打包不会创建 dmg 目录；按需创建目标目录以支持全新工作区构建。
  mkdirSync(dirname(dmgPath), { recursive: true });
  const stagingDirectory = mkdtempSync(join(tmpdir(), "orange-dmg-"));
  const stagedAppPath = join(stagingDirectory, `${productName}.app`);

  try {
    // 将应用副本和 Applications 目录快捷方式放进镜像根目录，提供标准 macOS 拖拽安装流程。
    cpSync(appPath, stagedAppPath, { recursive: true, verbatimSymlinks: true });
    symlinkSync("/Applications", join(stagingDirectory, "Applications"), "dir");
    console.info("[desktop-build] level=info event=local_dmg_create_started platform=darwin certificateRequired=false installerLayout=applications_shortcut");
    // -ov 仅覆盖当前构建的同名 DMG，-srcfolder 使 DMG 根目录包含应用和 Applications 快捷方式。
    const result = spawnSync("hdiutil", ["create", "-volname", productName, "-srcfolder", stagingDirectory, "-ov", "-format", "UDZO", dmgPath], {
      stdio: "inherit",
    });

    if (result.error) {
      throw result.error;
    }

    if (result.status !== 0) {
      throw new Error(`hdiutil exited with status ${result.status}`);
    }
  } finally {
    // 暂存目录包含复制后的 .app，必须在成功和失败路径中均清理，避免占用本地磁盘空间。
    rmSync(stagingDirectory, { recursive: true, force: true });
  }

  console.info("[desktop-build] level=info event=local_dmg_create_completed platform=darwin artifact=dmg installerLayout=applications_shortcut");
}

/** 启动 Tauri 打包；本机包使用 ad-hoc 签名，正式发布包必须使用显式的发布签名身份。 */
function runDesktopBuild() {
  const { buildMode, tauriArguments } = parseBuildArguments();
  const args = ["build"];

  configureMacOSSigning(args, buildMode);
  console.info(`[desktop-build] level=info event=build_started platform=${process.platform} mode=${buildMode}`);

  args.push(...tauriArguments);
  const result = spawnSync("tauri", args, { stdio: "inherit" });

  if (result.error) {
    throw result.error;
  }

  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }

  if (buildMode === BUILD_MODE.localDmg) {
    createLocalDmg();
  }
}

try {
  runDesktopBuild();
} catch (error) {
  const message = error instanceof Error ? error.message : String(error);

  console.error(`[desktop-build] level=error event=build_failed message=${message}`);
  process.exit(1);
}
