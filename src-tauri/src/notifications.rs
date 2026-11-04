use crate::domain::ScheduledJob;
use crate::logging::{self, AppEventBuilder, AppLogCategory, AppLogLevel};
use serde_json::json;
use tauri::{AppHandle, Manager, UserAttentionType};
use tauri_plugin_notification::NotificationExt;

/** 主窗口标识；判断「用户是否正看着应用」以它为准。 */
const MAIN_WINDOW_LABEL: &str = "main";

/** 系统通知正文的最大字符数，避免超长任务说明把通知撑成一段墙。 */
const NOTIFICATION_BODY_LIMIT: usize = 120;

/**
 * 用户是否正看着应用。
 *
 * 只有「窗口可见且聚焦」才算在看着：窗口隐藏到托盘、被最小化、
 * 或者焦点在别的应用上时都算没看着，这时才值得打扰系统通知中心。
 */
pub fn is_user_watching(app: &AppHandle) -> bool {
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return false;
    };
    let visible = window.is_visible().unwrap_or(false);
    let focused = window.is_focused().unwrap_or(false);
    visible && focused
}

/**
 * 提醒到点时的系统层通知。
 *
 * 窗口在前台时只发事件、由前端 Toast 呈现，这里直接返回，避免双重打扰；
 * 窗口不在前台时补一条系统通知，并请求任务栏注意（Windows 上表现为任务栏闪烁），
 * 让用户即使在看别的窗口也能注意到。通知失败只记日志，不影响任务状态。
 */
pub fn notify_reminder(app: &AppHandle, job: &ScheduledJob) {
    if is_user_watching(app) {
        return;
    }

    let title = format!("提醒 · {}", job.name);
    let body = notification_body(job);
    let result = app
        .notification()
        .builder()
        .title(&title)
        .body(&body)
        .show();

    match result {
        Ok(()) => {
            logging::write_app_event_best_effort(
                app,
                AppEventBuilder::new(
                    AppLogLevel::Info,
                    AppLogCategory::Agent,
                    "scheduled_job_notify",
                    "completed",
                    "已发送系统提醒通知。",
                )
                .metadata(json!({ "jobId": job.id })),
            );
        }
        Err(error) => {
            logging::write_app_event_best_effort(
                app,
                AppEventBuilder::new(
                    AppLogLevel::Warn,
                    AppLogCategory::Agent,
                    "scheduled_job_notify",
                    "failed",
                    format!("系统提醒通知发送失败：{error}"),
                )
                .metadata(json!({ "jobId": job.id })),
            );
        }
    }

    // 通知中心可能被勿扰模式静音，任务栏闪烁是另一条独立的可见线索。
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = window.request_user_attention(Some(UserAttentionType::Informational));
    }
}

/** 取任务说明首行作为通知正文；空说明退化为周期描述。 */
fn notification_body(job: &ScheduledJob) -> String {
    let first_line = job
        .prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let text = if first_line.is_empty() {
        job.schedule_display.clone()
    } else {
        first_line.to_owned()
    };
    truncate_chars(&text, NOTIFICATION_BODY_LIMIT)
}

/** 按字符（而非字节）截断，避免把中文截成半个字。 */
fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    let mut truncated: String = value.chars().take(limit).collect();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{JobSchedule, ScheduledJob};

    fn sample_job(prompt: &str) -> ScheduledJob {
        ScheduledJob {
            id: "sched-notify".to_owned(),
            name: "喝水".to_owned(),
            prompt: prompt.to_owned(),
            schedule: JobSchedule::Daily {
                time: "09:00".to_owned(),
            },
            schedule_display: "每天 09:00".to_owned(),
            scheduled_at: None,
            timezone: "Asia/Shanghai".to_owned(),
            knowledge_base_ids: Vec::new(),
            output_mode: "reminder".to_owned(),
            inbox_knowledge_base_id: None,
            inbox_folder: None,
            session_policy: "newEachRun".to_owned(),
            continued_session_id: None,
            model_provider_id: None,
            model_id: None,
            explicit_skill_ids: Vec::new(),
            enabled: true,
            next_run_at: None,
            last_run_at: None,
            last_status: None,
            last_error: None,
            last_session_id: None,
            failure_streak: 0,
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            updated_at: "2026-01-01T00:00:00Z".to_owned(),
        }
    }

    #[test]
    fn notification_body_uses_first_non_empty_line() {
        let job = sample_job("\n\n  起来活动一下  \n第二行不展示");
        assert_eq!(notification_body(&job), "起来活动一下");
    }

    #[test]
    fn notification_body_falls_back_to_schedule_display() {
        let job = sample_job("   \n  ");
        assert_eq!(notification_body(&job), "每天 09:00");
    }

    #[test]
    fn notification_body_truncates_by_chars() {
        let job = sample_job(&"提".repeat(NOTIFICATION_BODY_LIMIT + 10));
        let body = notification_body(&job);
        assert_eq!(body.chars().count(), NOTIFICATION_BODY_LIMIT + 1);
        assert!(body.ends_with('…'));
    }
}
