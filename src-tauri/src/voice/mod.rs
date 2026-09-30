mod audio;
#[cfg(windows)]
mod engine;
mod model;
mod text;

pub use model::VOICE_MODEL_PROGRESS_EVENT;

use tauri::AppHandle;

use crate::domain::{VoiceModelStatus, VoiceTranscript};

/** 读取本机语音模型状态。不访问麦克风。 */
pub fn model_status(app: &AppHandle) -> VoiceModelStatus {
    model::model_status(app)
}

/** 下载 SenseVoice 中文模型到应用数据目录。 */
pub async fn download_model(app: &AppHandle) -> Result<VoiceModelStatus, String> {
    model::download_model(app).await
}

/** 删除已下载的语音模型。 */
pub fn delete_model(app: &AppHandle) -> Result<VoiceModelStatus, String> {
    model::delete_model(app)
}

/** 开始一次按住说话。模型还没准备好时拒绝录音。 */
pub fn start_capture(app: &AppHandle) -> Result<(), String> {
    #[cfg(not(windows))]
    {
        let _ = app;
        return Err("语音输入目前只在 Windows 桌面端可用。".to_owned());
    }
    #[cfg(windows)]
    {
        if model::is_downloading() {
            return Err("语音模型还在下载，请稍候。".to_owned());
        }
        if !model::is_ready(app) {
            return Err("请先下载中文语音模型。".to_owned());
        }
        engine::start_capture()
    }
}

/** 结束录音并返回识别文本。 */
pub fn stop_and_transcribe(app: &AppHandle) -> Result<VoiceTranscript, String> {
    #[cfg(not(windows))]
    {
        let _ = app;
        Err("语音输入目前只在 Windows 桌面端可用。".to_owned())
    }
    #[cfg(windows)]
    {
        let directory = model::model_dir(app)?;
        engine::stop_and_transcribe(&directory)
    }
}
