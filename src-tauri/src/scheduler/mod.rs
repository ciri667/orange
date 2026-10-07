use crate::domain::{
    AgentMessage, AgentSession, ScheduleSessionIdentity, ScheduledJob, ScheduledJobUpdatedEvent,
    ScheduleSessionPolicy,
};
use crate::logging::{self, AppEventBuilder, AppLogCategory, AppLogLevel};
use crate::runtime;
use crate::storage::{self, ClaimedScheduledRun};
use chrono::Utc;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/** 前端监听的任务状态事件。 */
pub const SCHEDULED_JOB_UPDATED_EVENT: &str = "scheduled-job-updated";

/** 全局最多同时跑一个定时任务，避免和交互/IM 抢模型。 */
static SCHEDULE_RUN_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/** 应用启动后开始 30 秒 tick；睡眠醒来后按墙钟认领到期任务。 */
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if let Err(error) = tick_once(app.clone()).await {
                logging::write_app_event_best_effort(
                    &app,
                    AppEventBuilder::new(
                        AppLogLevel::Warn,
                        AppLogCategory::Agent,
                        "scheduled_job_tick",
                        "failed",
                        error,
                    ),
                );
            }
        }
    });
}

/** 认领最多一个到期任务并在后台执行。已有任务在跑时本拍让路，不推进 next_run。 */
pub async fn tick_once(app: AppHandle) -> Result<(), String> {
    if SCHEDULE_RUN_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return Ok(());
    }

    let claim_app = app.clone();
    let claimed = match tauri::async_runtime::spawn_blocking(move || {
        storage::claim_due_jobs_for_app(&claim_app, Utc::now(), 1)
    })
    .await
    {
        Ok(Ok(claimed)) => claimed,
        Ok(Err(error)) => {
            SCHEDULE_RUN_IN_FLIGHT.store(false, Ordering::SeqCst);
            return Err(error);
        }
        Err(error) => {
            SCHEDULE_RUN_IN_FLIGHT.store(false, Ordering::SeqCst);
            return Err(format!("认领到期定时任务失败：{error}"));
        }
    };

    let Some(claimed) = claimed.into_iter().next() else {
        SCHEDULE_RUN_IN_FLIGHT.store(false, Ordering::SeqCst);
        return Ok(());
    };

    tauri::async_runtime::spawn(async move {
        execute_claimed_job(app, claimed).await;
        SCHEDULE_RUN_IN_FLIGHT.store(false, Ordering::SeqCst);
    });

    Ok(())
}

/** 用户点击立即运行：不消耗下次调度拍。 */
pub async fn trigger_now(app: AppHandle, job_id: String) -> Result<ScheduledJob, String> {
    if SCHEDULE_RUN_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return Err("已有定时任务正在运行，请稍后再试。".to_owned());
    }

    let claim_app = app.clone();
    let job_id_for_claim = job_id.clone();
    let claimed = match tauri::async_runtime::spawn_blocking(move || {
        storage::claim_manual_run_for_app(&claim_app, &job_id_for_claim)
    })
    .await
    {
        Ok(Ok(claimed)) => claimed,
        Ok(Err(error)) => {
            SCHEDULE_RUN_IN_FLIGHT.store(false, Ordering::SeqCst);
            return Err(error);
        }
        Err(error) => {
            SCHEDULE_RUN_IN_FLIGHT.store(false, Ordering::SeqCst);
            return Err(format!("登记立即运行失败：{error}"));
        }
    };

    let job = claimed.job.clone();
    tauri::async_runtime::spawn(async move {
        execute_claimed_job(app, claimed).await;
        SCHEDULE_RUN_IN_FLIGHT.store(false, Ordering::SeqCst);
    });
    Ok(job)
}

async fn execute_claimed_job(app: AppHandle, claimed: ClaimedScheduledRun) {
    let job_id = claimed.job.id.clone();
    let run_id = claimed.run_id.clone();
    let job_name = claimed.job.name.clone();

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Agent,
            "scheduled_job_run",
            "started",
            "定时任务开始执行。",
        )
        .metadata(json!({
            "jobId": job_id,
            "runId": run_id,
            "scheduledInstant": claimed.scheduled_instant,
        })),
    );

    let result = crate::commands::run_scheduled_job_turn(app.clone(), claimed.job.clone()).await;
    let (status, session_id, error, continued_session_id) = match &result {
        Ok(outcome) => (
            outcome.status.as_str(),
            outcome.session_id.as_deref(),
            outcome.error.as_deref(),
            outcome.session_id.clone(),
        ),
        Err(error) => ("failed", None, Some(error.as_str()), None),
    };

    let finish_app = app.clone();
    let finish_run_id = run_id.clone();
    let finish_job_id = job_id.clone();
    let finish_session = session_id.map(|value| value.to_owned());
    let finish_error = error.map(|value| value.to_owned());
    let finish_continued = continued_session_id;
    let finish_status = status.to_owned();
    let finished_job = tauri::async_runtime::spawn_blocking(move || {
        storage::finish_scheduled_run_for_app(
            &finish_app,
            &finish_run_id,
            &finish_job_id,
            &finish_status,
            finish_session.as_deref(),
            finish_error.as_deref(),
            finish_continued.as_deref(),
        )
    })
    .await;

    match finished_job {
        Ok(Ok(job)) => {
            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    if status == "ok" {
                        AppLogLevel::Info
                    } else {
                        AppLogLevel::Warn
                    },
                    AppLogCategory::Agent,
                    "scheduled_job_run",
                    status,
                    "定时任务已结束。",
                )
                .session_id(session_id.unwrap_or(""))
                .metadata(json!({
                    "jobId": job_id,
                    "runId": run_id,
                    "enabled": job.enabled,
                    "failureStreak": job.failure_streak,
                })),
            );
            emit_job_updated(&app, job, session_id.map(|value| value.to_owned()));
        }
        Ok(Err(error)) => {
            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    AppLogLevel::Error,
                    AppLogCategory::Agent,
                    "scheduled_job_run",
                    "failed",
                    format!("定时任务「{job_name}」结束状态写入失败：{error}"),
                )
                .metadata(json!({ "jobId": job_id, "runId": run_id })),
            );
        }
        Err(error) => {
            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    AppLogLevel::Error,
                    AppLogCategory::Agent,
                    "scheduled_job_run",
                    "failed",
                    format!("定时任务「{job_name}」结束状态写入失败：{error}"),
                )
                .metadata(json!({ "jobId": job_id, "runId": run_id })),
            );
        }
    }
}

pub fn emit_job_updated(app: &AppHandle, job: ScheduledJob, session_id: Option<String>) {
    let _ = app.emit(
        SCHEDULED_JOB_UPDATED_EVENT,
        ScheduledJobUpdatedEvent { job, session_id },
    );
}

/** 构造定时任务会话；续跑策略复用已有会话。 */
pub fn build_schedule_agent_session(job: &ScheduledJob) -> AgentSession {
    let now = storage::format_local_datetime();
    AgentSession {
        id: storage::create_id("session-sched"),
        title: format!("定时任务 · {}", job.name),
        im_identity: None,
        schedule_identity: Some(ScheduleSessionIdentity {
            job_id: job.id.clone(),
            job_name: job.name.clone(),
        }),
        r#type: "knowledge-base".to_owned(),
        knowledge_base_ids: job.knowledge_base_ids.clone(),
        active_note_id: None,
        pinned_note_ids: Vec::new(),
        messages: Vec::new(),
        pending_change: None,
        pending_change_set: None,
        pending_execution: None,
        security_level: "basic".to_owned(),
        context_summary: None,
        created_at: now.clone(),
        updated_at: now,
        deleted_at: None,
        model_provider_id: job.model_provider_id.clone(),
        model_id: job.model_id.clone(),
        context_usage: None,
        title_customized: false,
        pinned_at: None,
        archived_at: None,
    }
}

pub fn schedule_session_policy(job: &ScheduledJob) -> ScheduleSessionPolicy {
    ScheduleSessionPolicy::parse(&job.session_policy)
}

pub fn build_schedule_user_message(prompt: &str) -> AgentMessage {
    AgentMessage {
        id: storage::create_id("user-sched"),
        role: "user".to_owned(),
        content: prompt.to_owned(),
        action: Some("ask".to_owned()),
        citations: None,
        tool_calls: None,
        mentioned_file_ids: Vec::new(),
        images: Vec::new(),
        trace: Vec::new(),
        turn_duration_ms: None,
        interrupted: false,
    }
}

pub fn build_schedule_turn_prompt(job: &ScheduledJob) -> String {
    let now = storage::format_local_datetime();
    format!(
        "【定时任务】当前时间：{now}。这是无人值守触发，请直接完成下列任务并给出可阅读的结果，不要询问用户是否继续。\n\n{}",
        job.prompt.trim()
    )
}

pub fn session_blocks_schedule(session: &AgentSession) -> Option<&'static str> {
    if runtime::is_session_turn_active(&session.id) {
        return Some("目标会话正在处理其它请求。");
    }
    if session
        .pending_change
        .as_ref()
        .is_some_and(|change| change.status == "pending")
    {
        return Some("目标会话有待确认变更，已跳过本拍。");
    }
    None
}
