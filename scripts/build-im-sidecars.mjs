import { chmodSync, mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

/** 项目根目录，脚本从 package.json 所在目录运行，避免 cwd 变化影响路径解析。 */
const projectRoot = resolve(new URL("..", import.meta.url).pathname);

/** Rust 开发态和 Tauri 打包态共享的 sidecar 二进制输出目录。 */
const sidecarOutputDir = join(projectRoot, "src-tauri", "sidecars", "bin");

/** 已注册 IM sidecar provider；后续新增 IM 只需要在这里补充构建定义。 */
const sidecarProviders = [
  {
    providerId: "feishu",
    displayName: "Feishu/Lark",
    sourceDir: join(projectRoot, "src-tauri", "sidecars", "feishu-gateway"),
    binaryName: "feishu-gateway",
    buildTool: "go",
    buildArgs(outputPath) {
      return [
        ["go", ["version"]],
        ["go", ["mod", "tidy"]],
        ["go", ["build", "-trimpath", "-o", outputPath, "."]],
      ];
    },
  },
];

/** 解析命令行参数；只支持 provider 过滤，避免把未知参数静默吞掉。 */
function parseArgs(argv) {
  const options = { providerId: "" };

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];

    if (arg === "--provider") {
      options.providerId = argv[index + 1] ?? "";
      index += 1;
      continue;
    }

    if (arg.startsWith("--provider=")) {
      options.providerId = arg.slice("--provider=".length);
      continue;
    }

    throw new Error(`Unknown argument: ${arg}`);
  }

  return options;
}

/** 运行外部构建命令；stdout/stderr 透传给调用者，但日志不包含任何 IM 凭证。 */
function run(command, args, cwd) {
  const result = spawnSync(command, args, {
    cwd,
    stdio: "inherit",
  });

  if (result.error) {
    throw result.error;
  }

  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(" ")} exited with ${result.status}`);
  }
}

/** 生成当前平台的二进制文件名，Windows 需要 .exe 后缀。 */
function platformBinaryName(binaryName) {
  return process.platform === "win32" ? `${binaryName}.exe` : binaryName;
}

/** 构建单个 IM provider sidecar，并输出可观测但不含敏感信息的摘要日志。 */
function buildProvider(provider) {
  const startedAt = performance.now();
  const outputPath = join(sidecarOutputDir, platformBinaryName(provider.binaryName));

  console.info(`[im-sidecar] provider=${provider.providerId} status=started tool=${provider.buildTool}`);
  mkdirSync(sidecarOutputDir, { recursive: true });

  for (const [command, args] of provider.buildArgs(outputPath)) {
    // 外部命令的 cwd 固定在 provider 源码目录，避免跨 provider 构建互相污染。
    run(command, args, provider.sourceDir);
  }

  if (process.platform !== "win32") {
    chmodSync(outputPath, 0o755);
  }

  const elapsedMs = Math.round(performance.now() - startedAt);

  console.info(
    `[im-sidecar] provider=${provider.providerId} status=completed durationMs=${elapsedMs} output=${outputPath}`,
  );
}

try {
  const options = parseArgs(process.argv.slice(2));
  const providers = options.providerId
    ? sidecarProviders.filter((provider) => provider.providerId === options.providerId)
    : sidecarProviders;

  if (providers.length === 0) {
    throw new Error(`Unknown IM sidecar provider: ${options.providerId}`);
  }

  for (const provider of providers) {
    buildProvider(provider);
  }
} catch (error) {
  const message = error instanceof Error ? error.message : String(error);

  console.error(`[im-sidecar] status=failed message=${message}`);
  console.error(
    "Please make sure each provider build tool is installed, then rerun: npm run sidecar:im:build -- --provider feishu",
  );
  process.exit(1);
}
