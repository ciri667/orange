use super::common::*;
use crate::domain::{
    schedule_blueprints, AgentTurnRequest, CreateScheduledJobPayload, LoadScheduledJobRunsPayload,
    ScheduleBlueprint, ScheduleSessionPolicy, ScheduledJob, ScheduledJobIdPayload, ScheduledJobRun,
    UpdateScheduledJobPayload, UserSettings,
};
use crate::model_provider;
use crate::scheduler;

/** 定时任务一轮执行结果；skipped/blocked 不视为调度器崩溃。 */
pub struct ScheduledTurnOutcome {
    pub status: String,
    pub session_id: Option<String>,
    pub error: Option<String>,
}

#[tauri::command]
pub async fn list_scheduled_jobs(app: AppHandle) -> Result<Vec<ScheduledJob>, String> {
    run_blocking("读取定时任务", move || storage::list_scheduled_jobs(&app)).await
}

#[tauri::command]
pub async fn list_schedule_blueprints() -> Result<Vec<ScheduleBlueprint>, String> {
    Ok(schedule_blueprints())
}

#[tauri::command]
pub async fn create_scheduled_job(
    app: AppHandle,
    payload: CreateScheduledJobPayload,
) -> Result<ScheduledJob, String> {
    let job = run_blocking("创建定时任务", move || {
        storage::create_scheduled_job(&app, payload)
    })
    .await?;
    Ok(job)
}

#[tauri::command]
pub async fn update_scheduled_job(
    app: AppHandle,
    payload: UpdateScheduledJobPayload,
) -> Result<ScheduledJob, String> {
    let app_for_event = app.clone();
    let job = run_blocking("更新定时任务", move || {
        storage::update_scheduled_job(&app, payload)
    })
    .await?;
    scheduler::emit_job_updated(&app_for_event, job.clone(), None);
    Ok(job)
}

#[tauri::command]
pub async fn delete_scheduled_job(
    app: AppHandle,
    payload: ScheduledJobIdPayload,
) -> Result<(), String> {
    run_blocking("删除定时任务", move || {
        storage::delete_scheduled_job(&app, &payload.job_id)
    })
    .await
}

#[tauri::command]
pub async fn list_scheduled_job_runs(
    app: AppHandle,
    payload: LoadScheduledJobRunsPayload,
) -> Result<Vec<ScheduledJobRun>, String> {
    let limit = payload.limit.unwrap_or(20);
    run_blocking("读取定时任务运行历史", move || {
        storage::list_scheduled_job_runs(&app, &payload.job_id, limit)
    })
    .await
}

#[tauri::command]
pub async fn trigger_scheduled_job(
    app: AppHandle,
    payload: ScheduledJobIdPayload,
) -> Result<ScheduledJob, String> {
    scheduler::trigger_now(app, payload.job_id).await
}

/** 调度器认领之后真正跑 Agent；失败文案进入 run ledger，不抛给 tick。 */
pub(crate) async fn run_scheduled_job_turn(
    app: AppHandle,
    job: ScheduledJob,
) -> Result<ScheduledTurnOutcome, String> {
    let snapshot_app = app.clone();
    let mut snapshot = run_blocking("加载定时任务工作台状态", move || {
        storage::load_workspace_snapshot(&snapshot_app)
    })
    .await?;

    let valid_scope_ids = snapshot
        .knowledge_bases
        .iter()
        .filter(|knowledge_base| {
            job.knowledge_base_ids
                .iter()
                .any(|id| id == &knowledge_base.id)
        })
        .map(|knowledge_base| knowledge_base.id.clone())
        .collect::<Vec<_>>();
    if valid_scope_ids.is_empty() {
        return Ok(ScheduledTurnOutcome {
            status: "blocked".to_owned(),
            session_id: None,
            error: Some("定时任务绑定的知识库已失效。".to_owned()),
        });
    }

    let settings_app = app.clone();
    let settings = run_blocking("读取模型设置", move || {
        storage::load_user_settings(&settings_app)
    })
    .await?;
    if let Err(error) = preflight_schedule_model(&settings, &job) {
        return Ok(ScheduledTurnOutcome {
            status: "blocked".to_owned(),
            session_id: None,
            error: Some(error),
        });
    }

    let session_id = resolve_schedule_session(&mut snapshot, &job, valid_scope_ids.clone())?;
    if let Some(session) = snapshot
        .sessions
        .iter()
        .find(|session| session.id == session_id)
    {
        if let Some(reason) = scheduler::session_blocks_schedule(session) {
            return Ok(ScheduledTurnOutcome {
                status: "skipped".to_owned(),
                session_id: Some(session_id),
                error: Some(reason.to_owned()),
            });
        }
    }

    let prompt = scheduler::build_schedule_turn_prompt(&job);
    let user_message = scheduler::build_schedule_user_message(&prompt);
    let user_message_id = user_message.id.clone();
    if let Some(session) = snapshot
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id)
    {
        session.knowledge_base_ids = valid_scope_ids.clone();
        session.messages.push(user_message);
        session.updated_at = storage::format_local_datetime();
        if let Some(identity) = &mut session.schedule_identity {
            identity.job_name = job.name.clone();
        }
    }

    let active_knowledge_base_id = valid_scope_ids.first().cloned().unwrap_or_default();
    snapshot.active_session_id = session_id.clone();
    snapshot.active_knowledge_base_id = active_knowledge_base_id.clone();
    snapshot.active_note_id.clear();
    snapshot.active_document_id.clear();
    storage::save_snapshot_session(&app, &snapshot, &session_id)?;

    let cancel = runtime::register_agent_cancel(&session_id)?;
    let _cancel_guard = runtime::AgentCancelGuard::new(session_id.clone(), cancel.clone());
    let skills_app = app.clone();
    let available_skills = run_blocking("读取 Agent Skills", move || {
        let connection = storage::open_database(&skills_app)?;
        skills::load_agent_skills(&skills_app, &connection)
    })
    .await?;

    let request = AgentTurnRequest {
        prompt,
        action: "ask".to_owned(),
        session_id: session_id.clone(),
        active_knowledge_base_id: active_knowledge_base_id.clone(),
        active_note_id: String::new(),
        client_message_id: Some(user_message_id),
        model_provider_id: job.model_provider_id.clone(),
        model_id: job.model_id.clone(),
        explicit_skill_ids: job.explicit_skill_ids.clone(),
        mentioned_file_ids: Vec::new(),
        image_ids: Vec::new(),
    };

    let runtime_result =
        runtime::run_agent_turn(&app, snapshot, request, settings, available_skills, cancel).await;
    let audit_app = app.clone();
    let audit_log = runtime_result.audit_log.clone();
    run_blocking("写入定时任务审计日志", move || {
        storage::append_request_audit_log(&audit_app, &audit_log)
    })
    .await?;
    upsert_snapshot_index_in_background(app.clone(), &runtime_result.turn_result.snapshot).await?;
    persist_turn_session(&app, &runtime_result.turn_result.snapshot, &session_id).await?;

    let failed = runtime_result.audit_log.kind.contains("error")
        || runtime_result
            .turn_result
            .snapshot
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .and_then(|session| session.messages.last())
            .is_some_and(|message| message.interrupted);

    Ok(ScheduledTurnOutcome {
        status: if failed {
            "failed".to_owned()
        } else {
            "ok".to_owned()
        },
        session_id: Some(session_id),
        error: None,
    })
}

fn preflight_schedule_model(settings: &UserSettings, job: &ScheduledJob) -> Result<(), String> {
    let selection = model_provider::resolve_model_selection(
        &settings.model_config,
        job.model_provider_id.as_deref(),
        job.model_id.as_deref(),
        None,
        None,
    )
    .map_err(|error| error.to_string())?;
    if selection.provider.requires_api_key {
        match storage::load_model_api_key(&selection.provider.key_reference) {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err(format!(
                "Provider「{}」未找到模型密钥。",
                selection.provider.name
            )),
            Err(error) => Err(error),
        }
    } else {
        Ok(())
    }
}

fn resolve_schedule_session(
    snapshot: &mut crate::domain::WorkspaceSnapshot,
    job: &ScheduledJob,
    knowledge_base_ids: Vec<String>,
) -> Result<String, String> {
    let policy = scheduler::schedule_session_policy(job);
    if matches!(policy, ScheduleSessionPolicy::Continue) {
        if let Some(existing_id) = job.continued_session_id.as_deref() {
            if let Some(session) = snapshot
                .sessions
                .iter_mut()
                .find(|session| session.id == existing_id)
            {
                let matches_job = session
                    .schedule_identity
                    .as_ref()
                    .is_some_and(|identity| identity.job_id == job.id);
                if matches_job && session.deleted_at.is_none() {
                    session.knowledge_base_ids = knowledge_base_ids;
                    return Ok(session.id.clone());
                }
            }
        }
    }

    let mut session = scheduler::build_schedule_agent_session(job);
    session.knowledge_base_ids = knowledge_base_ids;
    let session_id = session.id.clone();
    snapshot.sessions.insert(0, session);
    Ok(session_id)
}
