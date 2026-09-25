use super::*;
use crate::domain::{
    schedule_display, CreateScheduledJobPayload, JobSchedule, JobWeekday, ScheduledJob,
    ScheduledJobRun, ScheduleOutputMode, ScheduleSessionPolicy, UpdateScheduledJobPayload,
    DEFAULT_SCHEDULE_INBOX_FOLDER, SCHEDULE_CATCH_UP_MAX_SECS, SCHEDULE_CATCH_UP_MIN_SECS,
    SCHEDULE_FAILURE_PAUSE_STREAK,
};
use chrono::{DateTime, Datelike, Duration as ChronoDuration, Local, NaiveTime, TimeZone, Timelike, Utc};
use rusqlite::{params, Connection, OptionalExtension};

pub(crate) const SCHEDULED_JOBS_SCHEMA: &str = r#"
            CREATE TABLE IF NOT EXISTS scheduled_jobs (
              id TEXT PRIMARY KEY,
              name TEXT NOT NULL,
              prompt TEXT NOT NULL,
              schedule_json TEXT NOT NULL,
              timezone TEXT NOT NULL,
              knowledge_base_ids_json TEXT NOT NULL,
              output_mode TEXT NOT NULL,
              inbox_knowledge_base_id TEXT,
              inbox_folder TEXT,
              session_policy TEXT NOT NULL,
              continued_session_id TEXT,
              model_provider_id TEXT,
              model_id TEXT,
              explicit_skill_ids_json TEXT NOT NULL DEFAULT '[]',
              enabled INTEGER NOT NULL,
              next_run_at TEXT,
              last_run_at TEXT,
              last_status TEXT,
              last_error TEXT,
              last_session_id TEXT,
              failure_streak INTEGER NOT NULL DEFAULT 0,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_scheduled_jobs_next_run
              ON scheduled_jobs(enabled, next_run_at);

            CREATE TABLE IF NOT EXISTS scheduled_job_runs (
              id TEXT PRIMARY KEY,
              job_id TEXT NOT NULL,
              scheduled_instant TEXT NOT NULL,
              started_at TEXT NOT NULL,
              finished_at TEXT,
              status TEXT NOT NULL,
              session_id TEXT,
              error TEXT
            );

            CREATE UNIQUE INDEX IF NOT EXISTS idx_scheduled_job_runs_slot
              ON scheduled_job_runs(job_id, scheduled_instant);
            "#;

/** 已被调度器认领、等待 Agent 执行的一拍。 */
#[derive(Clone, Debug)]
pub struct ClaimedScheduledRun {
    pub job: ScheduledJob,
    pub run_id: String,
    pub scheduled_instant: String,
}

/** 解析 HH:MM 时钟。 */
pub fn parse_hhmm(time: &str) -> Result<NaiveTime, String> {
    let trimmed = time.trim();
    NaiveTime::parse_from_str(trimmed, "%H:%M")
        .or_else(|_| NaiveTime::parse_from_str(trimmed, "%H:%M:%S"))
        .map_err(|_| format!("时间格式无效：{trimmed}，请使用 HH:MM。"))
}

/** 校验周期字段，创建和更新共用。 */
pub fn validate_schedule(schedule: &JobSchedule) -> Result<(), String> {
    match schedule {
        JobSchedule::Hourly {
            interval_hours,
            days,
        } => {
            if !(1..=24).contains(interval_hours) {
                return Err("小时间隔必须在 1 到 24 之间。".to_owned());
            }
            if matches!(days.as_ref(), Some(days) if days.is_empty()) {
                return Err("按小时重复时，若指定星期则不能为空。".to_owned());
            }
        }
        JobSchedule::Daily { time } | JobSchedule::Weekdays { time } => {
            parse_hhmm(time)?;
        }
        JobSchedule::Weekly { days, time } => {
            if days.is_empty() {
                return Err("每周任务至少选择一天。".to_owned());
            }
            parse_hhmm(time)?;
        }
        JobSchedule::Once { at } => {
            parse_once_local(at)?;
        }
    }
    Ok(())
}

/** 漏跑补跑窗口：周期一半，夹在 2 分钟到 2 小时。一次性任务固定 2 分钟宽限。 */
pub fn catch_up_window_secs(schedule: &JobSchedule) -> i64 {
    let period_secs = match schedule {
        JobSchedule::Hourly { interval_hours, .. } => i64::from(*interval_hours.max(&1)) * 3600,
        JobSchedule::Daily { .. } | JobSchedule::Weekdays { .. } => 24 * 3600,
        JobSchedule::Weekly { .. } => 7 * 24 * 3600,
        JobSchedule::Once { .. } => SCHEDULE_CATCH_UP_MIN_SECS,
    };
    (period_secs / 2).clamp(SCHEDULE_CATCH_UP_MIN_SECS, SCHEDULE_CATCH_UP_MAX_SECS)
}

/** 计算 `after` 之后的下一次触发（不含 after 本身）。 */
pub fn next_run_after(schedule: &JobSchedule, after: DateTime<Local>) -> Option<DateTime<Utc>> {
    match schedule {
        JobSchedule::Daily { time } => {
            let clock = parse_hhmm(time).ok()?;
            next_matching_local(after, clock, |_| true)
        }
        JobSchedule::Weekdays { time } => {
            let clock = parse_hhmm(time).ok()?;
            next_matching_local(after, clock, |weekday| weekday.number_from_monday() <= 5)
        }
        JobSchedule::Weekly { days, time } => {
            let clock = parse_hhmm(time).ok()?;
            let allowed = days
                .iter()
                .map(|day| day.as_chrono())
                .collect::<HashSet<_>>();
            next_matching_local(after, clock, |weekday| allowed.contains(&weekday))
        }
        JobSchedule::Hourly {
            interval_hours,
            days,
        } => next_hourly(after, *interval_hours, days.as_deref()),
        JobSchedule::Once { at } => {
            let instant = parse_once_local(at).ok()?;
            (instant > after).then(|| instant.with_timezone(&Utc))
        }
    }
}

/** UTC RFC3339，秒精度，Z 后缀。 */
pub fn format_utc(datetime: DateTime<Utc>) -> String {
    datetime.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn parse_utc(value: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value.trim())
        .map(|datetime| datetime.with_timezone(&Utc))
        .map_err(|_| format!("无法解析调度时间：{value}"))
}

/** 列出全部定时任务，按下次运行时间升序，已暂停的排后面。 */
pub fn list_scheduled_jobs(app: &AppHandle) -> Result<Vec<ScheduledJob>, String> {
    let connection = open_database(app)?;
    list_scheduled_jobs_on(&connection)
}

#[allow(dead_code)]
pub fn get_scheduled_job(app: &AppHandle, job_id: &str) -> Result<ScheduledJob, String> {
    let connection = open_database(app)?;
    get_scheduled_job_on(&connection, job_id)?
        .ok_or_else(|| "找不到该定时任务。".to_owned())
}

/** 创建任务并计算首次 next_run_at。过去的一次性任务会被拒绝。 */
pub fn create_scheduled_job(
    app: &AppHandle,
    payload: CreateScheduledJobPayload,
) -> Result<ScheduledJob, String> {
    validate_schedule(&payload.schedule)?;
    let name = payload.name.trim();
    let prompt = payload.prompt.trim();
    if name.is_empty() {
        return Err("请填写任务名称。".to_owned());
    }
    if prompt.is_empty() {
        return Err("请填写任务说明。".to_owned());
    }
    if payload.knowledge_base_ids.is_empty() {
        return Err("请至少选择一个知识库。".to_owned());
    }

    let now_local = Local::now();
    let next_run = next_run_after(&payload.schedule, now_local);
    if matches!(payload.schedule, JobSchedule::Once { .. }) && next_run.is_none() {
        return Err("一次性任务的时间必须晚于现在。".to_owned());
    }

    let output_mode = ScheduleOutputMode::parse(payload.output_mode.as_deref().unwrap_or("session"));
    let session_policy =
        ScheduleSessionPolicy::parse(payload.session_policy.as_deref().unwrap_or("newEachRun"));
    let now = format_utc(Utc::now());
    let job = ScheduledJob {
        id: create_id("sched"),
        name: name.to_owned(),
        prompt: prompt.to_owned(),
        schedule_display: schedule_display(&payload.schedule),
        schedule: payload.schedule,
        timezone: "local".to_owned(),
        knowledge_base_ids: payload.knowledge_base_ids,
        output_mode: output_mode.as_str().to_owned(),
        inbox_knowledge_base_id: payload.inbox_knowledge_base_id.filter(|value| !value.is_empty()),
        inbox_folder: Some(
            payload
                .inbox_folder
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(DEFAULT_SCHEDULE_INBOX_FOLDER)
                .to_owned(),
        ),
        session_policy: session_policy.as_str().to_owned(),
        continued_session_id: None,
        model_provider_id: payload.model_provider_id.filter(|value| !value.is_empty()),
        model_id: payload.model_id.filter(|value| !value.is_empty()),
        explicit_skill_ids: payload
            .explicit_skill_ids
            .into_iter()
            .filter(|value| !value.trim().is_empty())
            .collect(),
        enabled: payload.enabled.unwrap_or(true),
        next_run_at: next_run.map(format_utc),
        last_run_at: None,
        last_status: None,
        last_error: None,
        last_session_id: None,
        failure_streak: 0,
        created_at: now.clone(),
        updated_at: now,
    };

    let connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    insert_scheduled_job_on(&connection, &job)?;
    Ok(job)
}

pub fn update_scheduled_job(
    app: &AppHandle,
    payload: UpdateScheduledJobPayload,
) -> Result<ScheduledJob, String> {
    let connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    let mut job = get_scheduled_job_on(&connection, &payload.job_id)?
        .ok_or_else(|| "找不到该定时任务。".to_owned())?;

    if let Some(name) = payload.name {
        let name = name.trim();
        if name.is_empty() {
            return Err("请填写任务名称。".to_owned());
        }
        job.name = name.to_owned();
    }
    if let Some(prompt) = payload.prompt {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err("请填写任务说明。".to_owned());
        }
        job.prompt = prompt.to_owned();
    }
    let mut schedule_changed = false;
    if let Some(schedule) = payload.schedule {
        validate_schedule(&schedule)?;
        job.schedule_display = schedule_display(&schedule);
        job.schedule = schedule;
        schedule_changed = true;
    }
    if let Some(knowledge_base_ids) = payload.knowledge_base_ids {
        if knowledge_base_ids.is_empty() {
            return Err("请至少选择一个知识库。".to_owned());
        }
        job.knowledge_base_ids = knowledge_base_ids;
    }
    if let Some(output_mode) = payload.output_mode {
        job.output_mode = ScheduleOutputMode::parse(&output_mode).as_str().to_owned();
    }
    if payload.inbox_knowledge_base_id.is_some() {
        job.inbox_knowledge_base_id = payload
            .inbox_knowledge_base_id
            .filter(|value| !value.is_empty());
    }
    if let Some(inbox_folder) = payload.inbox_folder {
        job.inbox_folder = Some(
            inbox_folder
                .trim()
                .is_empty()
                .then(|| DEFAULT_SCHEDULE_INBOX_FOLDER.to_owned())
                .unwrap_or_else(|| inbox_folder.trim().to_owned()),
        );
    }
    if let Some(session_policy) = payload.session_policy {
        job.session_policy = ScheduleSessionPolicy::parse(&session_policy)
            .as_str()
            .to_owned();
    }
    if payload.model_provider_id.is_some() {
        job.model_provider_id = payload.model_provider_id.filter(|value| !value.is_empty());
    }
    if payload.model_id.is_some() {
        job.model_id = payload.model_id.filter(|value| !value.is_empty());
    }
    if let Some(explicit_skill_ids) = payload.explicit_skill_ids {
        job.explicit_skill_ids = explicit_skill_ids
            .into_iter()
            .filter(|value| !value.trim().is_empty())
            .collect();
    }
    if let Some(enabled) = payload.enabled {
        job.enabled = enabled;
    }
    if schedule_changed || payload.enabled == Some(true) {
        job.next_run_at = next_run_after(&job.schedule, Local::now()).map(format_utc);
    }
    if !job.enabled {
        // 暂停时保留 next_run_at 便于展示，恢复时再重算。
    }
    job.updated_at = format_utc(Utc::now());
    persist_scheduled_job_on(&connection, &job)?;
    Ok(job)
}

pub fn delete_scheduled_job(app: &AppHandle, job_id: &str) -> Result<(), String> {
    let connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    connection
        .execute(
            "DELETE FROM scheduled_job_runs WHERE job_id = ?1",
            params![job_id],
        )
        .map_err(|error| format!("无法删除定时任务运行记录：{error}"))?;
    let deleted = connection
        .execute("DELETE FROM scheduled_jobs WHERE id = ?1", params![job_id])
        .map_err(|error| format!("无法删除定时任务：{error}"))?;
    if deleted == 0 {
        return Err("找不到该定时任务。".to_owned());
    }
    Ok(())
}

pub fn list_scheduled_job_runs(
    app: &AppHandle,
    job_id: &str,
    limit: u32,
) -> Result<Vec<ScheduledJobRun>, String> {
    let connection = open_database(app)?;
    let limit = limit.clamp(1, 100) as i64;
    let mut statement = connection
        .prepare(
            "SELECT id, job_id, scheduled_instant, started_at, finished_at, status, session_id, error
             FROM scheduled_job_runs
             WHERE job_id = ?1
             ORDER BY started_at DESC
             LIMIT ?2",
        )
        .map_err(|error| format!("无法准备运行历史查询：{error}"))?;
    let rows = statement
        .query_map(params![job_id, limit], map_run_row)
        .map_err(|error| format!("无法查询运行历史：{error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法读取运行历史：{error}"))
}

/**
 * 认领到期任务：先写入 run ledger 并推进 next_run_at，再交给调用方执行。
 * 超出补跑窗口的拍记 skipped，不派发。
 */
pub fn claim_due_jobs(
    connection: &Connection,
    now: DateTime<Utc>,
    limit: usize,
) -> Result<Vec<ClaimedScheduledRun>, String> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let now_text = format_utc(now);
    let jobs = list_due_jobs_on(connection, &now_text)?;
    let mut claimed = Vec::new();
    let now_local = now.with_timezone(&Local);

    for mut job in jobs {
        if claimed.len() >= limit {
            break;
        }
        let Some(slot) = job.next_run_at.clone() else {
            continue;
        };
        let slot_instant = match parse_utc(&slot) {
            Ok(instant) => instant,
            Err(_) => continue,
        };
        let lag = now.signed_duration_since(slot_instant);
        let window = ChronoDuration::seconds(catch_up_window_secs(&job.schedule));
        let following = next_run_after(&job.schedule, now_local).map(format_utc);
        let is_once = matches!(job.schedule, JobSchedule::Once { .. });

        if lag > window {
            insert_run_slot(
                connection,
                &job.id,
                &slot,
                "skipped",
                Some("超出补跑窗口，已跳到下一拍。"),
            )?;
            job.next_run_at = following;
            if is_once {
                job.enabled = false;
                job.next_run_at = None;
            }
            job.last_status = Some("skipped".to_owned());
            job.last_error = Some("超出补跑窗口，已跳到下一拍。".to_owned());
            job.updated_at = now_text.clone();
            persist_scheduled_job_on(connection, &job)?;
            continue;
        }

        let run_id = match insert_run_slot(connection, &job.id, &slot, "running", None)? {
            Some(run_id) => run_id,
            None => continue,
        };
        job.next_run_at = following;
        if is_once {
            job.enabled = false;
            job.next_run_at = None;
        }
        job.updated_at = now_text.clone();
        persist_scheduled_job_on(connection, &job)?;
        claimed.push(ClaimedScheduledRun {
            job,
            run_id,
            scheduled_instant: slot,
        });
    }

    Ok(claimed)
}

/** 立即运行：额外记一拍，不消耗下次调度时间。 */
pub fn claim_manual_run(
    connection: &Connection,
    job_id: &str,
    now: DateTime<Utc>,
) -> Result<ClaimedScheduledRun, String> {
    let job = get_scheduled_job_on(connection, job_id)?
        .ok_or_else(|| "找不到该定时任务。".to_owned())?;
    let slot = format!("manual:{}", create_id("slot"));
    let run_id = insert_run_slot(connection, job_id, &slot, "running", None)?
        .ok_or_else(|| "无法登记立即运行。".to_owned())?;
    let _ = now;
    Ok(ClaimedScheduledRun {
        job,
        run_id,
        scheduled_instant: slot,
    })
}

pub fn finish_scheduled_run(
    connection: &Connection,
    run_id: &str,
    job_id: &str,
    status: &str,
    session_id: Option<&str>,
    error: Option<&str>,
    continued_session_id: Option<&str>,
) -> Result<ScheduledJob, String> {
    let now = format_utc(Utc::now());
    connection
        .execute(
            "UPDATE scheduled_job_runs
             SET finished_at = ?1, status = ?2, session_id = ?3, error = ?4
             WHERE id = ?5",
            params![now, status, session_id, error, run_id],
        )
        .map_err(|error| format!("无法更新运行记录：{error}"))?;

    let mut job = get_scheduled_job_on(connection, job_id)?
        .ok_or_else(|| "找不到该定时任务。".to_owned())?;
    job.last_run_at = Some(now.clone());
    job.last_status = Some(status.to_owned());
    job.last_error = error
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_owned());
    job.last_session_id = session_id.map(|value| value.to_owned());
    if let Some(session_id) = continued_session_id {
        job.continued_session_id = Some(session_id.to_owned());
    }
    if status == "ok" {
        job.failure_streak = 0;
    } else if matches!(status, "failed" | "blocked") {
        job.failure_streak += 1;
        if job.failure_streak >= SCHEDULE_FAILURE_PAUSE_STREAK {
            job.enabled = false;
        }
    }
    job.updated_at = now;
    persist_scheduled_job_on(connection, &job)?;
    Ok(job)
}

pub fn claim_due_jobs_for_app(
    app: &AppHandle,
    now: DateTime<Utc>,
    limit: usize,
) -> Result<Vec<ClaimedScheduledRun>, String> {
    let connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    claim_due_jobs(&connection, now, limit)
}

pub fn claim_manual_run_for_app(
    app: &AppHandle,
    job_id: &str,
) -> Result<ClaimedScheduledRun, String> {
    let connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    claim_manual_run(&connection, job_id, Utc::now())
}

pub fn finish_scheduled_run_for_app(
    app: &AppHandle,
    run_id: &str,
    job_id: &str,
    status: &str,
    session_id: Option<&str>,
    error: Option<&str>,
    continued_session_id: Option<&str>,
) -> Result<ScheduledJob, String> {
    let connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    finish_scheduled_run(
        &connection,
        run_id,
        job_id,
        status,
        session_id,
        error,
        continued_session_id,
    )
}

fn parse_once_local(at: &str) -> Result<DateTime<Local>, String> {
    let trimmed = at.trim();
    if let Ok(parsed) = DateTime::parse_from_rfc3339(trimmed) {
        return Ok(parsed.with_timezone(&Local));
    }
    let naive = chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S"))
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M"))
        .map_err(|_| format!("一次性时间格式无效：{trimmed}"))?;
    Local
        .from_local_datetime(&naive)
        .single()
        .ok_or_else(|| format!("一次性时间无法映射到本地时区：{trimmed}"))
}

fn next_matching_local(
    after: DateTime<Local>,
    clock: NaiveTime,
    mut allow_weekday: impl FnMut(chrono::Weekday) -> bool,
) -> Option<DateTime<Utc>> {
    let mut day = after.date_naive();
    for _ in 0..14 {
        if allow_weekday(day.weekday()) {
            let naive = day.and_time(clock);
            if let Some(candidate) = Local.from_local_datetime(&naive).single() {
                if candidate > after {
                    return Some(candidate.with_timezone(&Utc));
                }
            }
        }
        day = day.succ_opt()?;
    }
    None
}

fn next_hourly(
    after: DateTime<Local>,
    interval_hours: u32,
    days: Option<&[JobWeekday]>,
) -> Option<DateTime<Utc>> {
    let interval = interval_hours.max(1);
    let allowed = days.map(|days| {
        days.iter()
            .map(|day| day.as_chrono())
            .collect::<HashSet<_>>()
    });
    let hour_start = after
        .date_naive()
        .and_hms_opt(after.hour(), 0, 0)
        .and_then(|naive| Local.from_local_datetime(&naive).single())?;
    let mut candidate = if hour_start > after {
        hour_start
    } else {
        hour_start + ChronoDuration::hours(1)
    };
    for _ in 0..(24 * 14) {
        let day_ok = allowed
            .as_ref()
            .map(|days| days.contains(&candidate.weekday()))
            .unwrap_or(true);
        if day_ok && candidate.hour() % interval == 0 && candidate > after {
            return Some(candidate.with_timezone(&Utc));
        }
        candidate += ChronoDuration::hours(1);
    }
    None
}

fn list_scheduled_jobs_on(connection: &Connection) -> Result<Vec<ScheduledJob>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, name, prompt, schedule_json, timezone, knowledge_base_ids_json, output_mode,
                    inbox_knowledge_base_id, inbox_folder, session_policy, continued_session_id,
                    model_provider_id, model_id, explicit_skill_ids_json, enabled, next_run_at,
                    last_run_at, last_status, last_error, last_session_id, failure_streak,
                    created_at, updated_at
             FROM scheduled_jobs
             ORDER BY CASE WHEN enabled = 1 THEN 0 ELSE 1 END, next_run_at IS NULL, next_run_at, name",
        )
        .map_err(|error| format!("无法准备定时任务查询：{error}"))?;
    let rows = statement
        .query_map([], map_job_row)
        .map_err(|error| format!("无法查询定时任务：{error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法读取定时任务：{error}"))
}

fn list_due_jobs_on(connection: &Connection, now_text: &str) -> Result<Vec<ScheduledJob>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, name, prompt, schedule_json, timezone, knowledge_base_ids_json, output_mode,
                    inbox_knowledge_base_id, inbox_folder, session_policy, continued_session_id,
                    model_provider_id, model_id, explicit_skill_ids_json, enabled, next_run_at,
                    last_run_at, last_status, last_error, last_session_id, failure_streak,
                    created_at, updated_at
             FROM scheduled_jobs
             WHERE enabled = 1 AND next_run_at IS NOT NULL AND next_run_at <= ?1
             ORDER BY next_run_at",
        )
        .map_err(|error| format!("无法准备到期任务查询：{error}"))?;
    let rows = statement
        .query_map(params![now_text], map_job_row)
        .map_err(|error| format!("无法查询到期任务：{error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法读取到期任务：{error}"))
}

fn get_scheduled_job_on(
    connection: &Connection,
    job_id: &str,
) -> Result<Option<ScheduledJob>, String> {
    connection
        .query_row(
            "SELECT id, name, prompt, schedule_json, timezone, knowledge_base_ids_json, output_mode,
                    inbox_knowledge_base_id, inbox_folder, session_policy, continued_session_id,
                    model_provider_id, model_id, explicit_skill_ids_json, enabled, next_run_at,
                    last_run_at, last_status, last_error, last_session_id, failure_streak,
                    created_at, updated_at
             FROM scheduled_jobs WHERE id = ?1",
            params![job_id],
            map_job_row,
        )
        .optional()
        .map_err(|error| format!("无法读取定时任务：{error}"))
}

fn insert_scheduled_job_on(connection: &Connection, job: &ScheduledJob) -> Result<(), String> {
    let schedule_json = serde_json::to_string(&job.schedule)
        .map_err(|error| format!("无法序列化周期：{error}"))?;
    let knowledge_base_ids_json = serde_json::to_string(&job.knowledge_base_ids)
        .map_err(|error| format!("无法序列化知识库范围：{error}"))?;
    let explicit_skill_ids_json = serde_json::to_string(&job.explicit_skill_ids)
        .map_err(|error| format!("无法序列化 Skill：{error}"))?;
    connection
        .execute(
            "INSERT INTO scheduled_jobs (
                id, name, prompt, schedule_json, timezone, knowledge_base_ids_json, output_mode,
                inbox_knowledge_base_id, inbox_folder, session_policy, continued_session_id,
                model_provider_id, model_id, explicit_skill_ids_json, enabled, next_run_at,
                last_run_at, last_status, last_error, last_session_id, failure_streak,
                created_at, updated_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                ?17, ?18, ?19, ?20, ?21, ?22, ?23
             )",
            params![
                job.id,
                job.name,
                job.prompt,
                schedule_json,
                job.timezone,
                knowledge_base_ids_json,
                job.output_mode,
                job.inbox_knowledge_base_id,
                job.inbox_folder,
                job.session_policy,
                job.continued_session_id,
                job.model_provider_id,
                job.model_id,
                explicit_skill_ids_json,
                job.enabled as i64,
                job.next_run_at,
                job.last_run_at,
                job.last_status,
                job.last_error,
                job.last_session_id,
                job.failure_streak,
                job.created_at,
                job.updated_at,
            ],
        )
        .map_err(|error| format!("无法创建定时任务：{error}"))?;
    Ok(())
}

fn persist_scheduled_job_on(connection: &Connection, job: &ScheduledJob) -> Result<(), String> {
    let schedule_json = serde_json::to_string(&job.schedule)
        .map_err(|error| format!("无法序列化周期：{error}"))?;
    let knowledge_base_ids_json = serde_json::to_string(&job.knowledge_base_ids)
        .map_err(|error| format!("无法序列化知识库范围：{error}"))?;
    let explicit_skill_ids_json = serde_json::to_string(&job.explicit_skill_ids)
        .map_err(|error| format!("无法序列化 Skill：{error}"))?;
    connection
        .execute(
            "UPDATE scheduled_jobs SET
                name = ?2, prompt = ?3, schedule_json = ?4, timezone = ?5,
                knowledge_base_ids_json = ?6, output_mode = ?7, inbox_knowledge_base_id = ?8,
                inbox_folder = ?9, session_policy = ?10, continued_session_id = ?11,
                model_provider_id = ?12, model_id = ?13, explicit_skill_ids_json = ?14,
                enabled = ?15, next_run_at = ?16, last_run_at = ?17, last_status = ?18,
                last_error = ?19, last_session_id = ?20, failure_streak = ?21, updated_at = ?22
             WHERE id = ?1",
            params![
                job.id,
                job.name,
                job.prompt,
                schedule_json,
                job.timezone,
                knowledge_base_ids_json,
                job.output_mode,
                job.inbox_knowledge_base_id,
                job.inbox_folder,
                job.session_policy,
                job.continued_session_id,
                job.model_provider_id,
                job.model_id,
                explicit_skill_ids_json,
                job.enabled as i64,
                job.next_run_at,
                job.last_run_at,
                job.last_status,
                job.last_error,
                job.last_session_id,
                job.failure_streak,
                job.updated_at,
            ],
        )
        .map_err(|error| format!("无法更新定时任务：{error}"))?;
    Ok(())
}

fn insert_run_slot(
    connection: &Connection,
    job_id: &str,
    scheduled_instant: &str,
    status: &str,
    error: Option<&str>,
) -> Result<Option<String>, String> {
    let run_id = create_id("srun");
    let started_at = format_utc(Utc::now());
    let finished_at = if status == "running" {
        None
    } else {
        Some(started_at.clone())
    };
    let inserted = connection
        .execute(
            "INSERT OR IGNORE INTO scheduled_job_runs (
                id, job_id, scheduled_instant, started_at, finished_at, status, session_id, error
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)",
            params![
                run_id,
                job_id,
                scheduled_instant,
                started_at,
                finished_at,
                status,
                error,
            ],
        )
        .map_err(|error| format!("无法登记运行拍：{error}"))?;
    if inserted == 0 {
        Ok(None)
    } else {
        Ok(Some(run_id))
    }
}

fn map_job_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduledJob> {
    let schedule_json: String = row.get(3)?;
    let knowledge_base_ids_json: String = row.get(5)?;
    let explicit_skill_ids_json: String = row.get(13)?;
    let schedule: JobSchedule =
        serde_json::from_str(&schedule_json).unwrap_or(JobSchedule::Daily {
            time: "08:00".to_owned(),
        });
    let knowledge_base_ids: Vec<String> =
        serde_json::from_str(&knowledge_base_ids_json).unwrap_or_default();
    let explicit_skill_ids: Vec<String> =
        serde_json::from_str(&explicit_skill_ids_json).unwrap_or_default();
    let enabled: i64 = row.get(14)?;
    Ok(ScheduledJob {
        id: row.get(0)?,
        name: row.get(1)?,
        prompt: row.get(2)?,
        schedule_display: schedule_display(&schedule),
        schedule,
        timezone: row.get(4)?,
        knowledge_base_ids,
        output_mode: row.get(6)?,
        inbox_knowledge_base_id: row.get(7)?,
        inbox_folder: row.get(8)?,
        session_policy: row.get(9)?,
        continued_session_id: row.get(10)?,
        model_provider_id: row.get(11)?,
        model_id: row.get(12)?,
        explicit_skill_ids,
        enabled: enabled != 0,
        next_run_at: row.get(15)?,
        last_run_at: row.get(16)?,
        last_status: row.get(17)?,
        last_error: row.get(18)?,
        last_session_id: row.get(19)?,
        failure_streak: row.get(20)?,
        created_at: row.get(21)?,
        updated_at: row.get(22)?,
    })
}

fn map_run_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduledJobRun> {
    Ok(ScheduledJobRun {
        id: row.get(0)?,
        job_id: row.get(1)?,
        scheduled_instant: row.get(2)?,
        started_at: row.get(3)?,
        finished_at: row.get(4)?,
        status: row.get(5)?,
        session_id: row.get(6)?,
        error: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::JobWeekday;
    use chrono::TimeZone;

    fn local(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("local datetime")
    }

    fn memory_db() -> Connection {
        let connection = Connection::open_in_memory().expect("memory sqlite");
        connection
            .execute_batch(SCHEDULED_JOBS_SCHEMA)
            .expect("schema");
        connection
    }

    fn sample_job(schedule: JobSchedule, next_run_at: &str) -> ScheduledJob {
        let now = format_utc(Utc::now());
        ScheduledJob {
            id: "sched-test".to_owned(),
            name: "测试任务".to_owned(),
            prompt: "整理笔记".to_owned(),
            schedule_display: schedule_display(&schedule),
            schedule,
            timezone: "local".to_owned(),
            knowledge_base_ids: vec!["kb-a".to_owned()],
            output_mode: "session".to_owned(),
            inbox_knowledge_base_id: None,
            inbox_folder: Some("定时产出".to_owned()),
            session_policy: "newEachRun".to_owned(),
            continued_session_id: None,
            model_provider_id: None,
            model_id: None,
            explicit_skill_ids: Vec::new(),
            enabled: true,
            next_run_at: Some(next_run_at.to_owned()),
            last_run_at: None,
            last_status: None,
            last_error: None,
            last_session_id: None,
            failure_streak: 0,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    /** 2026-09-18 是星期五：7:00 之后的每天 8:00 应落在当天。 */
    #[test]
    fn daily_next_run_is_later_today_before_clock() {
        let after = local(2026, 9, 18, 7, 0);
        let next = next_run_after(
            &JobSchedule::Daily {
                time: "08:00".to_owned(),
            },
            after,
        )
        .expect("next");
        let local_next = next.with_timezone(&Local);
        assert_eq!(local_next.hour(), 8);
        assert_eq!(local_next.minute(), 0);
        assert_eq!(local_next.day(), 18);
    }

    /** 已经过了当天 8:00，下一拍是明天。 */
    #[test]
    fn daily_next_run_rolls_to_tomorrow_after_clock() {
        let after = local(2026, 9, 18, 8, 1);
        let next = next_run_after(
            &JobSchedule::Daily {
                time: "08:00".to_owned(),
            },
            after,
        )
        .expect("next");
        let local_next = next.with_timezone(&Local);
        assert_eq!(local_next.day(), 19);
        assert_eq!(local_next.hour(), 8);
    }

    /** 工作日任务在周五 9:00 之后跳到下周一。 */
    #[test]
    fn weekdays_skip_weekend() {
        let after = local(2026, 9, 18, 9, 0);
        let next = next_run_after(
            &JobSchedule::Weekdays {
                time: "08:00".to_owned(),
            },
            after,
        )
        .expect("next");
        let local_next = next.with_timezone(&Local);
        assert_eq!(local_next.weekday(), chrono::Weekday::Mon);
        assert_eq!(local_next.day(), 21);
        assert_eq!(local_next.hour(), 8);
    }

    /** 每周五 16:00：周五 15:00 仍落在当天。 */
    #[test]
    fn weekly_friday_stays_today_before_clock() {
        let after = local(2026, 9, 18, 15, 0);
        let next = next_run_after(
            &JobSchedule::Weekly {
                days: vec![JobWeekday::Fr],
                time: "16:00".to_owned(),
            },
            after,
        )
        .expect("next");
        let local_next = next.with_timezone(&Local);
        assert_eq!(local_next.weekday(), chrono::Weekday::Fri);
        assert_eq!(local_next.hour(), 16);
        assert_eq!(local_next.day(), 18);
    }

    /** 每 2 小时对齐整点。 */
    #[test]
    fn hourly_aligns_to_even_hours() {
        let after = local(2026, 9, 18, 9, 10);
        let next = next_run_after(
            &JobSchedule::Hourly {
                interval_hours: 2,
                days: None,
            },
            after,
        )
        .expect("next");
        let local_next = next.with_timezone(&Local);
        assert_eq!(local_next.hour(), 10);
        assert_eq!(local_next.minute(), 0);
    }

    #[test]
    fn catch_up_window_clamps_daily_to_two_hours() {
        let window = catch_up_window_secs(&JobSchedule::Daily {
            time: "08:00".to_owned(),
        });
        assert_eq!(window, SCHEDULE_CATCH_UP_MAX_SECS);
    }

    #[test]
    fn catch_up_window_for_15_minute_like_hourly_uses_floor() {
        let window = catch_up_window_secs(&JobSchedule::Hourly {
            interval_hours: 1,
            days: None,
        });
        assert_eq!(window, 1800);
    }

    /** 同一拍第二次认领必须得到空列表。 */
    #[test]
    fn claim_is_at_most_once_for_the_same_slot() {
        let connection = memory_db();
        let slot_local = local(2026, 9, 18, 8, 0);
        let slot = format_utc(slot_local.with_timezone(&Utc));
        let job = sample_job(
            JobSchedule::Daily {
                time: "08:00".to_owned(),
            },
            &slot,
        );
        insert_scheduled_job_on(&connection, &job).unwrap();

        let now = (slot_local + ChronoDuration::minutes(5)).with_timezone(&Utc);
        let first = claim_due_jobs(&connection, now, 1).unwrap();
        let second = claim_due_jobs(&connection, now, 1).unwrap();

        assert_eq!(first.len(), 1);
        assert_eq!(first[0].scheduled_instant, slot);
        assert!(second.is_empty());
        let stored = get_scheduled_job_on(&connection, "sched-test")
            .unwrap()
            .unwrap();
        let next = parse_utc(stored.next_run_at.as_deref().unwrap()).unwrap();
        assert!(next > now);
    }

    /** 超过 2 小时补跑窗口的每日任务只跳拍，不派发。 */
    #[test]
    fn missed_beyond_catch_up_is_skipped_not_fired() {
        let connection = memory_db();
        let slot_local = local(2026, 9, 18, 8, 0);
        let slot = format_utc(slot_local.with_timezone(&Utc));
        let job = sample_job(
            JobSchedule::Daily {
                time: "08:00".to_owned(),
            },
            &slot,
        );
        insert_scheduled_job_on(&connection, &job).unwrap();

        let now = (slot_local + ChronoDuration::hours(3)).with_timezone(&Utc);
        let claimed = claim_due_jobs(&connection, now, 1).unwrap();
        assert!(claimed.is_empty());

        let stored = get_scheduled_job_on(&connection, "sched-test")
            .unwrap()
            .unwrap();
        assert_eq!(stored.last_status.as_deref(), Some("skipped"));
        let next = parse_utc(stored.next_run_at.as_deref().unwrap()).unwrap();
        assert!(next > now);

        let mut statement = connection
            .prepare("SELECT status FROM scheduled_job_runs WHERE job_id = 'sched-test'")
            .unwrap();
        let status: String = statement.query_row([], |row| row.get(0)).unwrap();
        assert_eq!(status, "skipped");
    }

    /** 连续失败三次后自动暂停。 */
    #[test]
    fn failure_streak_pauses_job() {
        let connection = memory_db();
        let slot = format_utc(Utc::now());
        let job = sample_job(
            JobSchedule::Daily {
                time: "08:00".to_owned(),
            },
            &slot,
        );
        insert_scheduled_job_on(&connection, &job).unwrap();
        let run_id = insert_run_slot(&connection, "sched-test", "manual:a", "running", None)
            .unwrap()
            .unwrap();
        let updated = finish_scheduled_run(
            &connection,
            &run_id,
            "sched-test",
            "failed",
            None,
            Some("模型密钥缺失"),
            None,
        )
        .unwrap();
        assert_eq!(updated.failure_streak, 1);
        assert!(updated.enabled);

        for index in 0..2 {
            let run_id = insert_run_slot(
                &connection,
                "sched-test",
                &format!("manual:{index}"),
                "running",
                None,
            )
            .unwrap()
            .unwrap();
            let updated = finish_scheduled_run(
                &connection,
                &run_id,
                "sched-test",
                "failed",
                None,
                Some("模型密钥缺失"),
                None,
            )
            .unwrap();
            if index == 1 {
                assert!(!updated.enabled);
                assert_eq!(updated.failure_streak, 3);
            }
        }
    }
}
