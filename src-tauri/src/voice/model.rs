use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use futures_util::StreamExt;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;

use crate::domain::VoiceModelStatus;

/** 前端监听的下载进度事件。payload 是 VoiceModelStatus。 */
pub const VOICE_MODEL_PROGRESS_EVENT: &str = "voice-model-progress";

const MODEL_FILE: &str = "model.int8.onnx";
const TOKENS_FILE: &str = "tokens.txt";
/** 公开的 int8 包大约 228MB。明显更小的文件多半是下载到一半或错误页。 */
const MIN_MODEL_BYTES: u64 = 200_000_000;
const MIN_TOKENS_BYTES: u64 = 32;
const MODEL_REPO: &str = "csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17";

/** 下载进度只保留字节数和说明，不记录 URL 查询参数或文件内容。 */
struct DownloadProgress {
    downloading: bool,
    received_bytes: u64,
    total_bytes: Option<u64>,
    message: String,
}

static DOWNLOAD: Mutex<DownloadProgress> = Mutex::new(DownloadProgress {
    downloading: false,
    received_bytes: 0,
    total_bytes: None,
    message: String::new(),
});

/** 模型在应用数据目录中的位置，不打进安装包。 */
pub fn model_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法获取应用数据目录：{error}"))?;
    Ok(app_data_dir.join("voice").join("sense-voice"))
}

/** 给界面用的模型状态。macOS 当前不支持录音，supported 为 false。 */
pub fn model_status(app: &AppHandle) -> VoiceModelStatus {
    let progress = lock_download();
    let ready = model_dir(app).ok().is_some_and(|dir| files_are_ready(&dir));
    let message = if !platform_supported() {
        "语音输入目前只在 Windows 桌面端可用。".to_owned()
    } else if progress.downloading {
        progress.message.clone()
    } else if ready {
        "中文语音模型已就绪。在输入框按住麦克风说话。".to_owned()
    } else {
        "尚未下载中文语音模型。".to_owned()
    };

    VoiceModelStatus {
        supported: platform_supported(),
        ready: platform_supported() && ready,
        downloading: progress.downloading,
        received_bytes: progress.received_bytes,
        total_bytes: progress.total_bytes,
        message,
    }
}

pub fn is_downloading() -> bool {
    lock_download().downloading
}

/** 模型文件大小和内容都像一份可用的 SenseVoice int8 包。 */
pub fn is_ready(app: &AppHandle) -> bool {
    model_dir(app).ok().is_some_and(|dir| files_are_ready(&dir))
}

/** 下载 SenseVoice int8 和词表。已有其他下载进行时直接拒绝。 */
pub async fn download_model(app: &AppHandle) -> Result<VoiceModelStatus, String> {
    if !platform_supported() {
        return Err("语音输入目前只在 Windows 桌面端可用。".to_owned());
    }
    if is_capture_active() {
        return Err("请先松开麦克风，再下载语音模型。".to_owned());
    }

    {
        let mut progress = lock_download();
        if progress.downloading {
            return Err("语音模型正在下载。".to_owned());
        }
        progress.downloading = true;
        progress.received_bytes = 0;
        progress.total_bytes = None;
        progress.message = "正在下载中文语音模型…".to_owned();
    }

    let directory = match model_dir(app) {
        Ok(directory) => directory,
        Err(error) => {
            finish_download(false, error.clone());
            return Err(error);
        }
    };
    if let Err(error) = fs::create_dir_all(&directory) {
        let message = format!("无法创建语音模型目录：{error}");
        finish_download(false, message.clone());
        return Err(message);
    }

    let result = async {
        download_file(
            app,
            &directory,
            MODEL_FILE,
            &model_urls(MODEL_FILE),
            MIN_MODEL_BYTES,
        )
        .await?;
        download_file(
            app,
            &directory,
            TOKENS_FILE,
            &model_urls(TOKENS_FILE),
            MIN_TOKENS_BYTES,
        )
        .await?;
        Ok::<(), String>(())
    }
    .await;

    match result {
        Ok(()) => {
            // 模型文件换过之后，已加载的识别器还指着旧映射，必须丢掉。
            release_loaded_recognizer();
            finish_download(true, "中文语音模型已就绪。".to_owned());
            let _ = app.emit(VOICE_MODEL_PROGRESS_EVENT, model_status(app));
            Ok(model_status(app))
        }
        Err(error) => {
            finish_download(false, error.clone());
            Err(error)
        }
    }
}

/** 删除已下载的模型文件。录音或识别进行中时不删除。 */
pub fn delete_model(app: &AppHandle) -> Result<VoiceModelStatus, String> {
    if !platform_supported() {
        return Err("语音输入目前只在 Windows 桌面端可用。".to_owned());
    }
    if is_downloading() {
        return Err("语音模型正在下载，暂时不能删除。".to_owned());
    }
    if is_capture_active() {
        return Err("请先结束当前录音，再删除语音模型。".to_owned());
    }

    release_loaded_recognizer();
    let directory = model_dir(app)?;
    if directory.exists() {
        fs::remove_dir_all(&directory).map_err(|error| format!("无法删除语音模型：{error}"))?;
    }
    Ok(model_status(app))
}

pub fn model_file(dir: &Path) -> PathBuf {
    dir.join(MODEL_FILE)
}

pub fn tokens_file(dir: &Path) -> PathBuf {
    dir.join(TOKENS_FILE)
}

fn platform_supported() -> bool {
    cfg!(windows)
}

fn is_capture_active() -> bool {
    #[cfg(windows)]
    {
        super::engine::is_busy()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn release_loaded_recognizer() {
    #[cfg(windows)]
    super::engine::release_recognizer();
}

fn files_are_ready(dir: &Path) -> bool {
    file_is_ready(&model_file(dir), MIN_MODEL_BYTES)
        && file_is_ready(&tokens_file(dir), MIN_TOKENS_BYTES)
}

fn file_is_ready(path: &Path, min_bytes: u64) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    metadata.is_file() && metadata.len() >= min_bytes && !looks_like_html(path)
}

fn looks_like_html(path: &Path) -> bool {
    let mut header = [0_u8; 32];
    let Ok(mut file) = File::open(path) else {
        return true;
    };
    let Ok(read) = file.read(&mut header) else {
        return true;
    };
    let prefix = String::from_utf8_lossy(&header[..read]).to_ascii_lowercase();
    prefix.contains("<!doctype") || prefix.contains("<html")
}

fn model_urls(file_name: &str) -> [String; 2] {
    [
        format!("https://hf-mirror.com/{MODEL_REPO}/resolve/main/{file_name}"),
        format!("https://huggingface.co/{MODEL_REPO}/resolve/main/{file_name}"),
    ]
}

async fn download_file(
    app: &AppHandle,
    directory: &Path,
    file_name: &str,
    urls: &[String],
    min_bytes: u64,
) -> Result<(), String> {
    let destination = directory.join(file_name);
    if file_is_ready(&destination, min_bytes) {
        return Ok(());
    }

    let partial = directory.join(format!("{file_name}.partial"));
    let mut last_error = format!("无法下载{file_name}。");
    for url in urls {
        match download_one(app, url, &partial, file_name).await {
            Ok(()) if file_is_ready(&partial, min_bytes) => {
                if destination.exists() {
                    fs::remove_file(&destination)
                        .map_err(|error| format!("无法替换{file_name}：{error}"))?;
                }
                fs::rename(&partial, &destination)
                    .map_err(|error| format!("无法保存{file_name}：{error}"))?;
                return Ok(());
            }
            Ok(()) => {
                let _ = fs::remove_file(&partial);
                last_error = format!("{file_name} 下载不完整，请重试。");
            }
            Err(error) => {
                let _ = fs::remove_file(&partial);
                last_error = error;
            }
        }
    }
    Err(last_error)
}

async fn download_one(
    app: &AppHandle,
    url: &str,
    partial: &Path,
    file_name: &str,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(30 * 60))
        .user_agent("Orange-Juji/0.1 (local voice)")
        .build()
        .map_err(|error| format!("无法创建下载客户端：{error}"))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("下载{file_name}失败：{error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "下载{file_name}失败，服务返回 {}。",
            response.status()
        ));
    }

    let total_bytes = response.content_length();
    {
        let mut progress = lock_download();
        progress.received_bytes = 0;
        progress.total_bytes = total_bytes;
        progress.message = format!("正在下载{file_name}…");
    }
    let _ = app.emit(VOICE_MODEL_PROGRESS_EVENT, model_status(app));

    let mut file = tokio::fs::File::create(partial)
        .await
        .map_err(|error| format!("无法写入{file_name}：{error}"))?;
    let mut stream = response.bytes_stream();
    let mut received = 0_u64;
    let mut next_emit = 1_048_576_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("下载{file_name}中断：{error}"))?;
        file.write_all(&chunk)
            .await
            .map_err(|error| format!("写入{file_name}失败：{error}"))?;
        received = received.saturating_add(chunk.len() as u64);
        if received >= next_emit {
            next_emit = received.saturating_add(1_048_576);
            let mut progress = lock_download();
            progress.received_bytes = received;
            drop(progress);
            let _ = app.emit(VOICE_MODEL_PROGRESS_EVENT, model_status(app));
        }
    }
    file.flush()
        .await
        .map_err(|error| format!("保存{file_name}失败：{error}"))?;
    {
        let mut progress = lock_download();
        progress.received_bytes = received;
    }
    Ok(())
}

fn finish_download(success: bool, message: String) {
    let mut progress = lock_download();
    progress.downloading = false;
    if !success {
        progress.received_bytes = 0;
        progress.total_bytes = None;
    }
    progress.message = message;
}

fn lock_download() -> std::sync::MutexGuard<'static, DownloadProgress> {
    DOWNLOAD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_or_html_files_are_not_ready() {
        let directory =
            std::env::temp_dir().join(format!("orange-voice-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("create temp voice dir");
        fs::write(model_file(&directory), b"<html>not a model</html>").expect("write fake model");
        fs::write(tokens_file(&directory), b"ok").expect("write tiny tokens");
        assert!(!files_are_ready(&directory));
        let _ = fs::remove_dir_all(&directory);
    }
}
