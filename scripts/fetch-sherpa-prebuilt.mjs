import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, renameSync, statSync, unlinkSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/** 与 Cargo.toml 里的 sherpa-onnx 版本保持一致，文件名必须能被其构建脚本认出来。 */
const SHERPA_VERSION = "1.13.8";

/** Windows 使用动态库，避免静态 MT 运行库和 Rust 默认 MD 运行库冲突。 */
const ARCHIVE_NAME = `sherpa-onnx-v${SHERPA_VERSION}-win-x64-shared-MT-Release-lib.tar.bz2`;

/** 官方发布页在部分网络上 TLS 失败，镜像放在前面。 */
const ARCHIVE_URLS = [
  `https://ghfast.top/https://github.com/k2-fsa/sherpa-onnx/releases/download/v${SHERPA_VERSION}/${ARCHIVE_NAME}`,
  `https://github.com/k2-fsa/sherpa-onnx/releases/download/v${SHERPA_VERSION}/${ARCHIVE_NAME}`,
];

/** 预编译压缩包应明显大于一个错误页。 */
const MIN_ARCHIVE_BYTES = 5_000_000;

const repositoryRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));
const archiveDirectory = resolve(repositoryRoot, "src-tauri", "sherpa-prebuilt");
const archivePath = resolve(archiveDirectory, ARCHIVE_NAME);

/** 让后续 Cargo 调用不必依赖工作目录如何发现 .cargo/config.toml。 */
export function sherpaArchiveEnvironment(environment = process.env) {
  return {
    ...environment,
    SHERPA_ONNX_ARCHIVE_DIR: archiveDirectory,
  };
}

/** Windows 上确保 sherpa-onnx 预编译库已下载。其他平台直接跳过。 */
export async function ensureSherpaPrebuilt() {
  if (process.platform !== "win32") {
    return archiveDirectory;
  }

  if (existsSync(archivePath) && statSync(archivePath).size >= MIN_ARCHIVE_BYTES) {
    return archiveDirectory;
  }

  mkdirSync(archiveDirectory, { recursive: true });
  const partialPath = `${archivePath}.partial`;
  let lastError = null;

  for (const url of ARCHIVE_URLS) {
    console.info(`[sherpa] level=info event=prebuilt_download_started url=${url}`);
    try {
      await downloadToFile(url, partialPath);
      const size = statSync(partialPath).size;
      if (size < MIN_ARCHIVE_BYTES) {
        throw new Error(`downloaded archive is too small (${size} bytes)`);
      }
      if (existsSync(archivePath)) {
        unlinkSync(archivePath);
      }
      renameSync(partialPath, archivePath);
      console.info(`[sherpa] level=info event=prebuilt_download_completed bytes=${size}`);
      return archiveDirectory;
    } catch (error) {
      lastError = error;
      if (existsSync(partialPath)) {
        unlinkSync(partialPath);
      }
      const message = error instanceof Error ? error.message : String(error);
      console.error(`[sherpa] level=error event=prebuilt_download_failed message=${message}`);
    }
  }

  throw lastError instanceof Error ? lastError : new Error("failed to download sherpa-onnx prebuilt libraries");
}

/** 用 curl 下载，沿用本机已经能访问的镜像，并跟随跳转。 */
function downloadToFile(url, destination) {
  mkdirSync(dirname(destination), { recursive: true });

  return new Promise((resolveDownload, rejectDownload) => {
    const curl = spawn(
      "curl.exe",
      ["-L", "--fail", "--retry", "2", "--retry-all-errors", "-o", destination, url],
      { stdio: ["ignore", "inherit", "inherit"] },
    );

    curl.on("error", rejectDownload);
    curl.on("exit", (status) => {
      if (status === 0) {
        resolveDownload();
        return;
      }
      rejectDownload(new Error(`curl exited with status ${status ?? "unknown"}`));
    });
  });
}

/** 直接执行时下载预编译库；若参数里有 `--`，再用带好环境变量的方式运行后面的命令。 */
async function main() {
  await ensureSherpaPrebuilt();
  const separator = process.argv.indexOf("--");
  if (separator === -1) {
    return;
  }

  const command = process.argv.slice(separator + 1);
  if (!command.length) {
    throw new Error("missing command after --");
  }

  const result = spawnSync(command[0], command.slice(1), {
    env: sherpaArchiveEnvironment(),
    stdio: "inherit",
    shell: false,
  });

  if (result.error) {
    throw result.error;
  }
  process.exit(result.status ?? 1);
}

const invokedDirectly = process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (invokedDirectly) {
  main().catch((error) => {
    const message = error instanceof Error ? error.message : String(error);
    console.error(`[sherpa] level=error event=prebuilt_fetch_failed message=${message}`);
    process.exit(1);
  });
}
