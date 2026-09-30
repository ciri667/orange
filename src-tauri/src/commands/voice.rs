use super::common::*;
use crate::domain::{VoiceModelStatus, VoiceTranscript};
use crate::voice::{self, VOICE_MODEL_PROGRESS_EVENT};

/** 读取本机中文语音模型是否已下载。不返回模型路径。 */
#[tauri::command]
pub async fn load_voice_model_status(app: AppHandle) -> Result<VoiceModelStatus, String> {
    Ok(voice::model_status(&app))
}

/** 下载 SenseVoice int8。进度通过 voice-model-progress 事件推送。 */
#[tauri::command]
pub async fn download_voice_model(app: AppHandle) -> Result<VoiceModelStatus, String> {
    let started = Instant::now();
    let result = voice::download_model(&app).await;
    match &result {
        Ok(status) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::App,
                "voice_model_download",
                "completed",
                "中文语音模型已下载。",
            )
            .duration(started.elapsed())
            .metadata(json!({
                "ready": status.ready,
                "eventName": VOICE_MODEL_PROGRESS_EVENT,
            })),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Warn,
                AppLogCategory::App,
                "voice_model_download",
                "failed",
                error.clone(),
            )
            .duration(started.elapsed()),
        ),
    }
    result
}

/** 删除本机语音模型。不删除笔记或会话。 */
#[tauri::command]
pub async fn delete_voice_model(app: AppHandle) -> Result<VoiceModelStatus, String> {
    let result = voice::delete_model(&app);
    match &result {
        Ok(_) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::App,
                "voice_model_delete",
                "completed",
                "已删除本机中文语音模型。",
            ),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Warn,
                AppLogCategory::App,
                "voice_model_delete",
                "failed",
                error.clone(),
            ),
        ),
    }
    result
}

/** 按住麦克风时开始采集。音频只留在进程内存里。 */
#[tauri::command]
pub async fn start_voice_capture(app: AppHandle) -> Result<(), String> {
    let capture_app = app.clone();
    let result = run_blocking("开始语音采集", move || {
        voice::start_capture(&capture_app)
    })
    .await;
    match &result {
        Ok(()) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::App,
                "voice_capture",
                "started",
                "开始采集麦克风。",
            ),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Warn,
                AppLogCategory::App,
                "voice_capture",
                "failed",
                error.clone(),
            ),
        ),
    }
    result
}

/** 松开麦克风后识别，并只把文本返回给输入框。日志不记录正文。 */
#[tauri::command]
pub async fn stop_voice_capture(app: AppHandle) -> Result<VoiceTranscript, String> {
    let started = Instant::now();
    let capture_app = app.clone();
    let result = run_blocking("识别语音", move || {
        voice::stop_and_transcribe(&capture_app)
    })
    .await;
    match &result {
        Ok(transcript) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::App,
                "voice_capture",
                "completed",
                "语音识别完成。",
            )
            .duration(started.elapsed())
            .metadata(json!({
                "audioDurationMs": transcript.duration_ms,
                "transcriptChars": transcript.text.chars().count(),
                "truncated": transcript.truncated,
            })),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Warn,
                AppLogCategory::App,
                "voice_capture",
                "failed",
                error.clone(),
            )
            .duration(started.elapsed()),
        ),
    }
    result
}
