import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { spawnSync } from "node:child_process";
import { randomBytes } from "node:crypto";

/** macOS 开发态固定的本地代码签名证书名称。 */
export const DEVELOPMENT_SIGNING_IDENTITY = "Orange Local Development";

/** 运行系统命令并收集输出；命令参数中不传递业务凭据。 */
function run(command, args, options = {}) {
  return spawnSync(command, args, {
    encoding: "utf8",
    stdio: options.inherit ? "inherit" : "pipe",
  });
}

/** 判断登录钥匙串中是否已有可供 codesign 使用的开发证书。 */
export function hasDevelopmentSigningIdentity() {
  const result = run("security", ["find-identity", "-v", "-p", "codesigning"]);

  return result.status === 0 && result.stdout.includes(`\"${DEVELOPMENT_SIGNING_IDENTITY}\"`);
}

/** 创建本机自签名证书并导入登录钥匙串；临时私钥文件会在结束时删除。 */
function createDevelopmentSigningIdentity() {
  const tempDirectory = mkdtempSync(join(tmpdir(), "orange-dev-signing-"));
  const keyPath = join(tempDirectory, "orange-dev.key");
  const certificatePath = join(tempDirectory, "orange-dev.crt");
  const archivePath = join(tempDirectory, "orange-dev.p12");
  const opensslConfigPath = join(tempDirectory, "openssl.cnf");
  // P12 导入密码仅在当前进程内短暂存在，既不写入磁盘也不输出到日志。
  const archivePassword = randomBytes(24).toString("hex");

  try {
    console.info("[dev-signing] level=info event=certificate_create_started platform=darwin");
    // 使用配置文件声明 codeSigning 扩展，兼容 macOS 自带 OpenSSL/LibreSSL 的不同参数集。
    writeFileSync(
      opensslConfigPath,
      `[req]\ndistinguished_name = subject\nx509_extensions = extensions\nprompt = no\n\n[subject]\nCN = ${DEVELOPMENT_SIGNING_IDENTITY}\n\n[extensions]\nkeyUsage = critical, digitalSignature\nextendedKeyUsage = codeSigning\n`,
      { encoding: "utf8", mode: 0o600 },
    );
    const certificateResult = run("openssl", [
      "req",
      "-x509",
      "-newkey",
      "rsa:2048",
      "-sha256",
      "-nodes",
      "-keyout",
      keyPath,
      "-out",
      certificatePath,
      "-days",
      "3650",
      "-config",
      opensslConfigPath,
    ]);

    if (certificateResult.status !== 0) {
      throw new Error("openssl could not create the development certificate");
    }

    const archiveResult = run("openssl", [
      "pkcs12",
      "-export",
      "-out",
      archivePath,
      "-inkey",
      keyPath,
      "-in",
      certificatePath,
      "-passout",
      `pass:${archivePassword}`,
      // macOS Security.framework 无法导入 OpenSSL 3 默认的 PBES2/AES P12 封装。
      "-keypbe",
      "PBE-SHA1-3DES",
      "-certpbe",
      "PBE-SHA1-3DES",
      "-macalg",
      "sha1",
    ]);

    if (archiveResult.status !== 0) {
      throw new Error("openssl could not package the development certificate");
    }

    // -T 仅授权 codesign 使用导入私钥，避免向其他可执行程序开放访问。
    const importResult = run("security", [
      "import",
      archivePath,
      "-f",
      "pkcs12",
      "-k",
      "login.keychain-db",
      "-P",
      archivePassword,
      "-T",
      "/usr/bin/codesign",
    ]);

    if (importResult.status !== 0) {
      throw new Error("security could not import the development certificate into the login keychain");
    }

    // 自签名证书需要显式标记为本机 codeSign 信任，security 才会将其列为有效签名身份。
    const trustResult = run("security", [
      "add-trusted-cert",
      "-d",
      "-r",
      "trustRoot",
      "-k",
      "login.keychain-db",
      certificatePath,
    ]);

    if (trustResult.status !== 0) {
      throw new Error("security could not trust the development certificate for code signing");
    }
  } finally {
    // 临时目录包含仅用于证书导入的私钥，必须无论成功失败都立即清理。
    rmSync(tempDirectory, { recursive: true, force: true });
  }
}

/** 在 macOS 上确保固定开发签名身份存在；其他平台显式跳过。 */
export function ensureDevelopmentSigningIdentity() {
  if (process.platform !== "darwin") {
    console.info(`[dev-signing] level=info event=skipped platform=${process.platform}`);
    return;
  }

  if (!hasDevelopmentSigningIdentity()) {
    createDevelopmentSigningIdentity();
  }

  if (!hasDevelopmentSigningIdentity()) {
    throw new Error("development signing identity is unavailable after initialization");
  }

  console.info("[dev-signing] level=info event=identity_ready platform=darwin identityConfigured=true");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    ensureDevelopmentSigningIdentity();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);

    console.error(`[dev-signing] level=error event=initialization_failed message=${message}`);
    console.error("Install the macOS command line tools (security, codesign, openssl), then rerun: npm run dev:signing:init");
    process.exit(1);
  }
}
