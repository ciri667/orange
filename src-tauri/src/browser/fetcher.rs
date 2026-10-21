//! 独立 Chromium 的下载与安装。
//!
//! 固定使用 Chrome for Testing 的一个版本，首次启用时由用户显式触发下载，
//! 绝不静默下载。zip 自带 CRC 完整性校验；解压后用标记文件记录版本，
//! 避免半成品被当成可用浏览器。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;

use crate::logging::{self, AppEventBuilder, AppLogCategory, AppLogLevel};

/** 事件携带完整状态；重新进入设置页时也可通过状态命令恢复同一份快照。 */
pub const BROWSER_DOWNLOAD_PROGRESS_EVENT: &str = "browser-download-progress";

/** 固定的 Chrome for Testing 版本；升级时同步更新下载 URL 与目录标记。 */
pub const CHROMIUM_VERSION: &str = "155.0.8059.84";

/** 下载解压目录里的版本标记文件名。 */
const VERSION_MARKER: &str = "orange-chromium-version.txt";

#[derive(Debug, Deserialize)]
struct ChannelRelease {
    version: String,
}

#[derive(Debug, Deserialize)]
struct DownloadItem {
    platform: String,
    url: String,
}

#[derive(Debug, Default, Deserialize)]
struct ChannelDownloads {
    #[serde(default)]
    chrome: Vec<DownloadItem>,
}

#[derive(Debug, Deserialize)]
struct LkgPayload {
    channels: HashMap<String, ChannelWithDownloads>,
}

#[derive(Debug, Deserialize)]
struct ChannelWithDownloads {
    version: String,
    #[serde(default)]
    downloads: ChannelDownloads,
}

use std::collections::HashMap;

/** 当前平台对应的 CfT platform 标识与解压后的可执行文件相对路径。 */
#[cfg(target_os = "windows")]
const PLATFORM_ID: &str = "win64";
#[cfg(target_os = "macos")]
const PLATFORM_ID: &str = "mac-arm64";
#[cfg(all(unix, not(target_os = "macos")))]
const PLATFORM_ID: &str = "linux64";

#[cfg(target_os = "windows")]
const EXECUTABLE_SUFFIX: &str = "chrome-win64/chrome.exe";
#[cfg(target_os = "macos")]
const EXECUTABLE_SUFFIX: &str =
    "chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing";
#[cfg(all(unix, not(target_os = "macos")))]
const EXECUTABLE_SUFFIX: &str = "chrome-linux64/chrome";

fn chromium_root(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法读取应用数据目录：{error}"))?
        .join("browser")
        .join("chromium"))
}

pub fn chromium_executable(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(chromium_root(app)?.join(EXECUTABLE_SUFFIX))
}

/// 读取设置里的浏览器开关；设置缺失或损坏时按默认（关闭）处理。
pub fn browser_settings_enabled(app: &AppHandle) -> bool {
    crate::storage::load_user_settings(app)
        .map(|settings| settings.browser.enabled)
        .unwrap_or(false)
}

/** 下载地址固定指向 CfT 公开存储；版本不可变，URL 由版本号推导。 */
fn download_url(version: &str) -> String {
    format!(
        "https://storage.googleapis.com/chrome-for-testing-public/{version}/{PLATFORM_ID}/chrome-{PLATFORM_ID}.zip"
    )
}

/** 下载并安装固定版本 Chromium；后端持有任务状态，页面卸载不会中断下载。 */
pub async fn ensure_chromium_downloaded(app: &AppHandle) -> Result<String, String> {
    let root = chromium_root(app)?;
    {
        let mut progress = lock_download();
        if progress.downloading {
            return Err("浏览器内核正在下载或安装，请等待当前任务完成。".to_owned());
        }
        if let Some(version) = installed_version(&root) {
            return Ok(version);
        }
        // 检查与占用在同一把锁内完成，避免重复点击或多个窗口写同一个安装目录。
        progress.start()?;
    }

    let operation_id = crate::storage::create_id("browser-download");
    let started_at = Instant::now();
    publish_download_status(app);
    log_download(
        app,
        &operation_id,
        started_at,
        AppLogLevel::Info,
        "started",
        "开始下载内嵌浏览器内核。",
        None,
    );

    let archive_path = root.join(format!("chrome-{PLATFORM_ID}.zip"));
    let result = async {
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(|error| format!("无法创建浏览器目录：{error}"))?;
        // 安装完成前不得保留旧标记，否则半成品可执行文件可能被当成可用浏览器。
        let marker = root.join(VERSION_MARKER);
        if marker.exists() {
            tokio::fs::remove_file(marker)
                .await
                .map_err(|error| format!("无法清理浏览器版本标记：{error}"))?;
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .read_timeout(Duration::from_secs(60))
            .timeout(Duration::from_secs(30 * 60))
            .user_agent("Orange-Juji/0.1 (browser installer)")
            .build()
            .map_err(|error| format!("无法创建浏览器下载客户端：{error}"))?;
        let url = download_url(CHROMIUM_VERSION);
        for attempt in 1..=3 {
            let mut last_log = Instant::now();
            let mut connected = false;
            let download = download_archive(&client, &url, &archive_path, |downloaded, total| {
                {
                    let mut progress = lock_download();
                    progress.phase = ChromiumInstallPhase::Downloading;
                    progress.downloaded = downloaded;
                    progress.total = total;
                }
                publish_download_status(app);
                // 界面每 250ms 更新；日志每 10 秒记录一次，避免大文件下载刷屏。
                if !connected || last_log.elapsed() >= Duration::from_secs(10) {
                    let message = if !connected {
                        "已连接下载服务器，正在接收浏览器内核。".to_owned()
                    } else {
                        format!(
                            "浏览器内核已下载 {:.1} MB。",
                            downloaded as f64 / 1_048_576.0
                        )
                    };
                    log_download(
                        app,
                        &operation_id,
                        started_at,
                        AppLogLevel::Info,
                        "downloading",
                        &message,
                        Some(attempt),
                    );
                    connected = true;
                    last_log = Instant::now();
                }
            })
            .await;
            match download {
                Ok(()) => break,
                Err(error) if attempt < 3 => {
                    {
                        let mut progress = lock_download();
                        progress.phase = ChromiumInstallPhase::Connecting;
                        progress.downloaded = 0;
                        progress.total = None;
                    }
                    publish_download_status(app);
                    log_download(
                        app,
                        &operation_id,
                        started_at,
                        AppLogLevel::Warn,
                        "retrying",
                        &format!("{error}，正在重试下载。"),
                        Some(attempt + 1),
                    );
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Err(error) => return Err(error),
            }
        }

        lock_download().phase = ChromiumInstallPhase::Extracting;
        publish_download_status(app);
        log_download(
            app,
            &operation_id,
            started_at,
            AppLogLevel::Info,
            "extracting",
            "浏览器内核下载完成，正在校验并解压安装。",
            None,
        );

        // 大文件解压放到阻塞线程，避免阻塞状态查询和前端进度事件。
        let archive = archive_path.clone();
        let target = root.clone();
        tokio::task::spawn_blocking(move || unpack_chromium(&archive, &target))
            .await
            .map_err(|error| format!("浏览器解压任务失败：{error}"))??;
        tokio::fs::write(root.join(VERSION_MARKER), CHROMIUM_VERSION)
            .await
            .map_err(|error| format!("无法写入版本标记：{error}"))?;
        Ok::<_, String>(CHROMIUM_VERSION.to_owned())
    }
    .await;

    let _ = tokio::fs::remove_file(&archive_path).await;
    lock_download().finish(&result);
    publish_download_status(app);
    match &result {
        Ok(_) => log_download(
            app,
            &operation_id,
            started_at,
            AppLogLevel::Info,
            "completed",
            "内嵌浏览器内核安装完成，可以使用浏览器功能。",
            None,
        ),
        Err(error) => log_download(
            app,
            &operation_id,
            started_at,
            AppLogLevel::Error,
            "failed",
            &format!("浏览器内核下载或安装失败：{error}"),
            None,
        ),
    }
    result
}

/** 流式下载到临时文件，实时报告实际字节数；失败时清理半成品以便重试。 */
async fn download_archive(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    mut on_progress: impl FnMut(u64, Option<u64>) + Send,
) -> Result<(), String> {
    let partial = dest.with_extension("zip.partial");
    let result = async {
        let response = client
            .get(url)
            .send()
            .await
            .map_err(|error| format!("连接浏览器下载服务器失败：{error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "浏览器内核下载失败，服务器返回 {}。",
                response.status()
            ));
        }
        let total = response.content_length().filter(|total| *total > 0);
        on_progress(0, total);
        let mut file = tokio::fs::File::create(&partial)
            .await
            .map_err(|error| format!("无法创建浏览器下载文件：{error}"))?;
        let mut stream = response.bytes_stream();
        let mut downloaded = 0u64;
        let mut last_emit = Instant::now();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| format!("浏览器内核下载中断：{error}"))?;
            file.write_all(&chunk)
                .await
                .map_err(|error| format!("无法写入浏览器下载文件：{error}"))?;
            downloaded = downloaded.saturating_add(chunk.len() as u64);
            if last_emit.elapsed() >= Duration::from_millis(250) {
                on_progress(downloaded, total);
                last_emit = Instant::now();
            }
        }
        if downloaded == 0 || total.is_some_and(|total| total != downloaded) {
            return Err("浏览器内核下载不完整，请重试。".to_owned());
        }
        file.flush()
            .await
            .map_err(|error| format!("无法保存浏览器下载文件：{error}"))?;
        drop(file);
        on_progress(downloaded, total);
        tokio::fs::rename(&partial, dest)
            .await
            .map_err(|error| format!("无法保存浏览器下载文件：{error}"))?;
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&partial).await;
    }
    result
}

/** 完成 CRC 校验和解压后再检查可执行文件；只有成功后调用方才能写入安装标记。 */
fn unpack_chromium(archive: &Path, target: &Path) -> Result<(), String> {
    unzip_archive(archive, target)?;
    if !executable_is_ready(target) {
        return Err("浏览器安装包缺少可执行文件，请重新下载。".to_owned());
    }
    Ok(())
}

/** 解压 zip 到目标目录；任何损坏都会报错，避免半成品浏览器。 */
fn unzip_archive(archive: &Path, target: &Path) -> Result<(), String> {
    let file =
        std::fs::File::open(archive).map_err(|error| format!("无法打开下载文件：{error}"))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|error| format!("下载文件损坏（zip 校验失败）：{error}"))?;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| format!("下载文件损坏：{error}"))?;
        let Some(relative) = entry.enclosed_name().map(|path| path.to_path_buf()) else {
            continue; // 跳过带 .. 的恶意路径。
        };
        let out_path = target.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path).map_err(|error| format!("无法创建目录：{error}"))?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("无法创建目录：{error}"))?;
        }
        let mut out =
            std::fs::File::create(&out_path).map_err(|error| format!("无法写入文件：{error}"))?;
        std::io::copy(&mut entry, &mut out).map_err(|error| format!("解压失败：{error}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(mode) = entry.unix_mode() {
                let _ = std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(mode));
            }
        }
    }
    Ok(())
}

fn read_version_marker(root: &Path) -> Option<String> {
    let mut content = String::new();
    std::fs::File::open(root.join(VERSION_MARKER))
        .ok()?
        .read_to_string(&mut content)
        .ok()?;
    let trimmed = content.trim().to_owned();
    (!trimmed.is_empty()).then_some(trimmed)
}

/** 供设置页展示的安装状态。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChromiumInstallStatus {
    pub installed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub downloading: bool,
    pub phase: ChromiumInstallPhase,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
}

/** 下载、解压与最终结果共用一套阶段，前端不再自行猜测是否仍在下载。 */
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChromiumInstallPhase {
    #[default]
    Idle,
    Connecting,
    Downloading,
    Extracting,
    Installed,
    Failed,
}

/** 只保存运行中任务的状态；安装完成与否仍以磁盘上的完整安装标记为准。 */
#[derive(Default)]
struct ChromiumDownloadProgress {
    downloading: bool,
    phase: ChromiumInstallPhase,
    downloaded: u64,
    total: Option<u64>,
    error: Option<String>,
}

impl ChromiumDownloadProgress {
    fn start(&mut self) -> Result<(), String> {
        if self.downloading {
            return Err("浏览器内核正在下载或安装，请等待当前任务完成。".to_owned());
        }
        *self = Self {
            downloading: true,
            phase: ChromiumInstallPhase::Connecting,
            ..Self::default()
        };
        Ok(())
    }

    fn finish(&mut self, result: &Result<String, String>) {
        self.downloading = false;
        match result {
            Ok(_) => {
                self.phase = ChromiumInstallPhase::Installed;
                self.error = None;
            }
            Err(error) => {
                self.phase = ChromiumInstallPhase::Failed;
                self.error = Some(error.clone());
            }
        }
    }
}

fn lock_download() -> MutexGuard<'static, ChromiumDownloadProgress> {
    static DOWNLOAD: OnceLock<Mutex<ChromiumDownloadProgress>> = OnceLock::new();
    DOWNLOAD
        .get_or_init(|| Mutex::new(ChromiumDownloadProgress::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn executable_is_ready(root: &Path) -> bool {
    std::fs::metadata(root.join(EXECUTABLE_SUFFIX))
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
}

/** 标记与可执行文件缺一不可，避免解压到一半时提前显示「已安装」。 */
fn installed_version(root: &Path) -> Option<String> {
    executable_is_ready(root)
        .then(|| read_version_marker(root))
        .flatten()
}

pub fn chromium_is_ready(app: &AppHandle) -> bool {
    chromium_root(app)
        .ok()
        .and_then(|root| installed_version(&root))
        .is_some()
}

/** 查询和进度事件复用同一快照，任务结束和失败也会推送给重新打开的设置页。 */
fn download_status(app: &AppHandle) -> ChromiumInstallStatus {
    let progress = lock_download();
    let version = chromium_root(app)
        .ok()
        .and_then(|root| installed_version(&root));
    let installed = version.is_some() && !progress.downloading;
    ChromiumInstallStatus {
        installed,
        version,
        downloading: progress.downloading,
        phase: if installed {
            ChromiumInstallPhase::Installed
        } else {
            progress.phase
        },
        downloaded: progress.downloaded,
        total: progress.total,
        error: progress.error.clone(),
    }
}

pub async fn chromium_install_status(app: &AppHandle) -> ChromiumInstallStatus {
    download_status(app)
}

fn publish_download_status(app: &AppHandle) {
    let _ = app.emit(BROWSER_DOWNLOAD_PROGRESS_EVENT, download_status(app));
}

/** 同时写入用户可读事件日志和诊断日志，所有阶段共享操作 ID 与耗时。 */
fn log_download(
    app: &AppHandle,
    operation_id: &str,
    started_at: Instant,
    level: AppLogLevel,
    status: &str,
    message: &str,
    attempt: Option<u32>,
) {
    let progress = lock_download();
    let metadata = serde_json::json!({
        "version": CHROMIUM_VERSION,
        "platform": PLATFORM_ID,
        "downloaded": progress.downloaded,
        "total": progress.total,
        "phase": progress.phase,
        "attempt": attempt,
    });
    drop(progress);
    logging::write_app_event_best_effort(
        app,
        AppEventBuilder::new(
            level,
            AppLogCategory::App,
            "browser_chromium_download",
            status,
            message,
        )
        .operation_id(operation_id)
        .duration(started_at.elapsed())
        .metadata(metadata),
    );
}

/// 保留对 LKG 端点的类型定义，供未来“查询最新固定版本”的升级流程复用。
#[allow(dead_code)]
fn parse_lkg(payload: &str) -> Option<String> {
    let data: LkgPayload = serde_json::from_str(payload).ok()?;
    let stable = data.channels.get("Stable")?;
    let url = stable
        .downloads
        .chrome
        .iter()
        .find(|item| item.platform == PLATFORM_ID)
        .map(|item| item.url.clone())?;
    (!url.is_empty()).then_some(stable.version.clone())
}

/// ChannelRelease 保留给单版本元数据端点复用。
#[allow(dead_code)]
fn parse_version(payload: &str) -> Option<String> {
    serde_json::from_str::<ChannelRelease>(payload)
        .ok()
        .map(|release| release.version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    /** 本地 HTTP 服务分块发送数据，验证收到中途进度而非仅完成回调。 */
    fn serve_download(
        headers: &str,
        chunks: Vec<Vec<u8>>,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let headers = headers.to_owned();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = [0; 4096];
            socket.read(&mut request).unwrap();
            socket.write_all(headers.as_bytes()).unwrap();
            for chunk in chunks {
                std::thread::sleep(Duration::from_millis(300));
                socket.write_all(&chunk).unwrap();
            }
        });
        (format!("http://{address}/chrome.zip"), server)
    }

    #[tokio::test]
    async fn download_reports_progress_before_completion_and_keeps_actual_total() {
        let directory = tempfile::tempdir().unwrap();
        let dest = directory.path().join("chrome.zip");
        let (url, server) = serve_download(
            "HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\n",
            vec![b"abcd".to_vec(), b"efgh".to_vec()],
        );
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let mut updates = Vec::new();
        download_archive(&client, &url, &dest, |received, total| {
            updates.push((received, total))
        })
        .await
        .unwrap();
        server.join().unwrap();
        assert!(updates
            .iter()
            .any(|(received, total)| *received > 0 && *received < 8 && *total == Some(8)));
        assert_eq!(updates.last(), Some(&(8, Some(8))));
        assert_eq!(std::fs::read(dest).unwrap(), b"abcdefgh");
    }

    #[tokio::test]
    async fn download_without_content_length_does_not_invent_a_total() {
        let directory = tempfile::tempdir().unwrap();
        let dest = directory.path().join("chrome.zip");
        let (url, server) = serve_download(
            "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n",
            vec![b"abcd".to_vec()],
        );
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let mut updates = Vec::new();
        download_archive(&client, &url, &dest, |received, total| {
            updates.push((received, total))
        })
        .await
        .unwrap();
        server.join().unwrap();
        assert_eq!(updates.last(), Some(&(4, None)));
        assert!(updates.iter().all(|(_, total)| total.is_none()));
    }

    #[tokio::test]
    async fn interrupted_download_removes_partial_file_and_can_be_retried() {
        let directory = tempfile::tempdir().unwrap();
        let dest = directory.path().join("chrome.zip");
        let (url, server) = serve_download(
            "HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\n",
            vec![b"abcd".to_vec()],
        );
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        assert!(download_archive(&client, &url, &dest, |_, _| {})
            .await
            .is_err());
        server.join().unwrap();
        assert!(!dest.exists());
        assert!(!dest.with_extension("zip.partial").exists());

        let (url, server) = serve_download(
            "HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\n",
            vec![b"done".to_vec()],
        );
        download_archive(&client, &url, &dest, |_, _| {})
            .await
            .unwrap();
        server.join().unwrap();
        assert_eq!(std::fs::read(dest).unwrap(), b"done");
    }

    #[test]
    fn active_install_rejects_duplicates_and_failure_allows_a_fresh_retry() {
        let mut progress = ChromiumDownloadProgress::default();
        progress.start().unwrap();
        progress.downloaded = 128;
        progress.total = Some(256);
        // 解压也占有下载任务，不能在下载字节达到 100% 后开启第二个写入者。
        progress.phase = ChromiumInstallPhase::Extracting;
        assert!(progress.start().is_err());
        assert!(progress.downloading);
        assert_eq!(progress.downloaded, 128);

        progress.finish(&Err("zip 损坏".to_owned()));
        assert!(!progress.downloading);
        assert_eq!(progress.phase, ChromiumInstallPhase::Failed);
        assert_eq!(progress.error.as_deref(), Some("zip 损坏"));
        progress.start().unwrap();
        assert!(progress.downloading);
        assert_eq!(progress.phase, ChromiumInstallPhase::Connecting);
        assert_eq!(progress.downloaded, 0);
        assert_eq!(progress.total, None);
        assert_eq!(progress.error, None);
        progress.finish(&Ok(CHROMIUM_VERSION.to_owned()));
        assert!(!progress.downloading);
        assert_eq!(progress.phase, ChromiumInstallPhase::Installed);
    }

    #[test]
    fn executable_without_completion_marker_is_not_installed() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join(EXECUTABLE_SUFFIX);
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, b"partial executable").unwrap();
        assert_eq!(installed_version(directory.path()), None);
        std::fs::write(directory.path().join(VERSION_MARKER), CHROMIUM_VERSION).unwrap();
        assert_eq!(
            installed_version(directory.path()).as_deref(),
            Some(CHROMIUM_VERSION)
        );
        std::fs::write(&executable, b"").unwrap();
        assert_eq!(installed_version(directory.path()), None);
    }

    #[test]
    fn corrupt_or_incomplete_archive_never_creates_an_install_marker() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("chrome.zip");
        std::fs::write(&archive, b"not a zip").unwrap();
        assert!(unpack_chromium(&archive, directory.path()).is_err());

        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        zip.start_file("readme.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"missing executable").unwrap();
        zip.finish().unwrap();
        assert!(unpack_chromium(&archive, directory.path())
            .unwrap_err()
            .contains("缺少可执行文件"));
        assert_eq!(installed_version(directory.path()), None);
        assert!(!directory.path().join(VERSION_MARKER).exists());
    }

    #[test]
    fn download_url_uses_fixed_version_and_platform() {
        let url = download_url("155.0.8059.84");
        assert!(url.contains("155.0.8059.84"));
        assert!(url.ends_with(&format!("chrome-{PLATFORM_ID}.zip")));
    }

    #[test]
    fn parse_lkg_reads_stable_platform_url() {
        let payload = r#"{
            "timestamp": "t",
            "channels": {
                "Stable": {
                    "channel": "Stable",
                    "version": "155.0.8059.84",
                    "revision": "1",
                    "downloads": {
                        "chrome": [
                            { "platform": "win64", "url": "https://example.com/win64.zip" },
                            { "platform": "linux64", "url": "https://example.com/linux64.zip" }
                        ]
                    }
                }
            }
        }"#;
        assert_eq!(parse_lkg(payload).as_deref(), Some("155.0.8059.84"));
    }

    #[test]
    fn parse_version_reads_single_release() {
        assert_eq!(
            parse_version(r#"{ "version": "1.2.3.4" }"#).as_deref(),
            Some("1.2.3.4")
        );
        assert_eq!(parse_version("not json"), None);
    }
}
