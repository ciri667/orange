use super::*;
use crate::domain::{
    schedule_display, CreateScheduledJobPayload, JobSchedule, JobWeekday, ScheduleOutputMode,
    ScheduleSessionPolicy, ScheduledJob, ScheduledJobRun, UpdateScheduledJobPayload,
    DEFAULT_SCHEDULE_INBOX_FOLDER, SCHEDULE_DISPATCH_TOLERANCE_SECS, SCHEDULE_FAILURE_PAUSE_STREAK,
};
use chrono::{
    DateTime, Datelike, Duration as ChronoDuration, Local, NaiveTime, TimeZone, Timelike, Utc,
};
use rusqlite::{params, Connection, OptionalExtension};

pub(crate) const SCHEDULED_JOBS_SCHEMA: &str = r#"
            CREATE TABLE IF NOT EXISTS scheduled_jobs (
              id TEXT PRIMARY KEY,
              name TEXT NOT NULL,
              prompt TEXT NOT NULL,
              schedule_json TEXT NOT NULL,
              scheduled_at TEXT,
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

/** 一次 tick 的认领结果；错过的任务也必须交回调度器广播状态。 */
#[derive(Debug, Default)]
pub struct DueScheduledJobs {
    pub claimed: Vec<ClaimedScheduledRun>,
    pub skipped: Vec<ScheduledJob>,
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

/** 调度器只派发准点附近的拍；错过统一标记，不自动补跑。 */
pub fn dispatch_tolerance_secs() -> i64 {
    SCHEDULE_DISPATCH_TOLERANCE_SECS
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
    get_scheduled_job_on(&connection, job_id)?.ok_or_else(|| "找不到该定时任务。".to_owned())
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

    let next_run_at = next_run.map(format_utc);
    let scheduled_at = matches!(&payload.schedule, JobSchedule::Once { .. })
        .then(|| next_run_at.clone())
        .flatten();
    let output_mode =
        ScheduleOutputMode::parse(payload.output_mode.as_deref().unwrap_or("session"));
    let session_policy =
        ScheduleSessionPolicy::parse(payload.session_policy.as_deref().unwrap_or("newEachRun"));
    let now = format_utc(Utc::now());
    let job = ScheduledJob {
        id: create_id("sched"),
        name: name.to_owned(),
        prompt: prompt.to_owned(),
        schedule_display: schedule_display(&payload.schedule),
        scheduled_at,
        schedule: payload.schedule,
        timezone: "local".to_owned(),
        knowledge_base_ids: payload.knowledge_base_ids,
        output_mode: output_mode.as_str().to_owned(),
        inbox_knowledge_base_id: payload
            .inbox_knowledge_base_id
            .filter(|value| !value.is_empty()),
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
        next_run_at,
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
    update_scheduled_job_on(&connection, payload, Local::now())
}

/** 更新任务与重算调度共用同一时刻，便于验证过期日程的编辑行为。 */
fn update_scheduled_job_on(
    connection: &Connection,
    payload: UpdateScheduledJobPayload,
    now: DateTime<Local>,
) -> Result<ScheduledJob, String> {
    let mut job = get_scheduled_job_on(connection, &payload.job_id)?
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
    let was_once = matches!(&job.schedule, JobSchedule::Once { .. });
    let mut schedule_changed = false;
    if let Some(schedule) = payload.schedule {
        validate_schedule(&schedule)?;
        // 一次性时间允许本地/RFC3339 两种写法；只改显示格式不算改期。
        schedule_changed = match (&job.schedule, &schedule) {
            (JobSchedule::Once { at: previous }, JobSchedule::Once { at: next }) => {
                parse_once_local(previous)? != parse_once_local(next)?
            }
            _ => job.schedule != schedule,
        };
        // 原定时间独立于未来的 next_run_at，编辑过期日程也不能将它清空。
        job.scheduled_at = match &schedule {
            JobSchedule::Once { at } => Some(format_utc(parse_once_local(at)?.with_timezone(&Utc))),
            _ => None,
        };
        job.schedule_display = schedule_display(&schedule);
        job.schedule = schedule;
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
    if schedule_changed {
        job.next_run_at = next_run_after(&job.schedule, now).map(format_utc);
        // 已错过的日程改期后应重新进入待开始状态；一次性改成周期任务时也应恢复为已安排。
        if payload.enabled.is_none()
            && job.next_run_at.is_some()
            && (was_once || matches!(&job.schedule, JobSchedule::Once { .. }))
        {
            job.enabled = true;
        }
    }
    if payload.enabled == Some(true) {
        job.next_run_at = next_run_after(&job.schedule, now).map(format_utc);
    }
    job.updated_at = format_utc(now.with_timezone(&Utc));
    persist_scheduled_job_on(connection, &job)?;
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
 * 超出容忍窗口的拍记 skipped 并返回，调用方负责广播状态。
 */
pub fn claim_due_jobs(
    connection: &Connection,
    now: DateTime<Utc>,
    limit: usize,
) -> Result<DueScheduledJobs, String> {
    if limit == 0 {
        return Ok(DueScheduledJobs::default());
    }
    let now_text = format_utc(now);
    let jobs = list_due_jobs_on(connection, &now_text)?;
    let mut result = DueScheduledJobs::default();
    let now_local = now.with_timezone(&Local);

    for mut job in jobs {
        if result.claimed.len() >= limit {
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
        let window = ChronoDuration::seconds(dispatch_tolerance_secs());
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
            result.skipped.push(job);
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
        result.claimed.push(ClaimedScheduledRun {
            job,
            run_id,
            scheduled_instant: slot,
        });
    }

    Ok(result)
}

/** 立即运行：额外记一拍，不消耗下次调度时间。 */
pub fn claim_manual_run(
    connection: &Connection,
    job_id: &str,
    now: DateTime<Utc>,
) -> Result<ClaimedScheduledRun, String> {
    let job =
        get_scheduled_job_on(connection, job_id)?.ok_or_else(|| "找不到该定时任务。".to_owned())?;
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

/**
 * 应用启动时结算所有已过期的拍。
 *
 * 任务不做自动补跑：启动后发现 `next_run_at` 已过期，就把这一拍记为
 * `skipped`（UI 显示「已错过」），一次性任务停用，周期任务推进到下一次。
 */
pub fn settle_missed_jobs_on_start(
    connection: &Connection,
    now: DateTime<Utc>,
) -> Result<Vec<ScheduledJob>, String> {
    let now_text = format_utc(now);
    let now_local = now.with_timezone(&Local);
    let jobs = list_due_jobs_on(connection, &now_text)?;
    let mut settled = Vec::new();

    for mut job in jobs {
        let Some(slot) = job.next_run_at.clone() else {
            continue;
        };

        // 崩溃恢复时可能还留着运行中的拍，不要把它覆盖成已错过。
        let existing_status: Option<String> = connection
            .query_row(
                "SELECT status FROM scheduled_job_runs WHERE job_id = ?1 AND scheduled_instant = ?2",
                params![job.id, slot],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("无法检查运行中的任务拍：{error}"))?;
        if existing_status.as_deref() == Some("running") {
            continue;
        }

        let error = "应用启动时已错过预定时间，等待手动补做。";
        let _ = insert_run_slot(connection, &job.id, &slot, "skipped", Some(error))?;
        job.last_status = Some("skipped".to_owned());
        job.last_error = Some(error.to_owned());
        if matches!(&job.schedule, JobSchedule::Once { .. }) {
            job.enabled = false;
            job.next_run_at = None;
        } else {
            job.next_run_at = next_run_after(&job.schedule, now_local).map(format_utc);
        }
        job.updated_at = now_text.clone();
        persist_scheduled_job_on(connection, &job)?;
        settled.push(job);
    }

    Ok(settled)
}

pub fn settle_missed_jobs_for_app(
    app: &AppHandle,
    now: DateTime<Utc>,
) -> Result<Vec<ScheduledJob>, String> {
    let connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    settle_missed_jobs_on_start(&connection, now)
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

    let mut job =
        get_scheduled_job_on(connection, job_id)?.ok_or_else(|| "找不到该定时任务。".to_owned())?;
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
) -> Result<DueScheduledJobs, String> {
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
            "SELECT id, name, prompt, schedule_json, scheduled_at, timezone, knowledge_base_ids_json, output_mode,
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
            "SELECT id, name, prompt, schedule_json, scheduled_at, timezone, knowledge_base_ids_json, output_mode,
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
            "SELECT id, name, prompt, schedule_json, scheduled_at, timezone, knowledge_base_ids_json, output_mode,
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
    let schedule_json =
        serde_json::to_string(&job.schedule).map_err(|error| format!("无法序列化周期：{error}"))?;
    let knowledge_base_ids_json = serde_json::to_string(&job.knowledge_base_ids)
        .map_err(|error| format!("无法序列化知识库范围：{error}"))?;
    let explicit_skill_ids_json = serde_json::to_string(&job.explicit_skill_ids)
        .map_err(|error| format!("无法序列化 Skill：{error}"))?;
    connection
        .execute(
            "INSERT INTO scheduled_jobs (
                id, name, prompt, schedule_json, scheduled_at, timezone, knowledge_base_ids_json, output_mode,
                inbox_knowledge_base_id, inbox_folder, session_policy, continued_session_id,
                model_provider_id, model_id, explicit_skill_ids_json, enabled, next_run_at,
                last_run_at, last_status, last_error, last_session_id, failure_streak,
                created_at, updated_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24
             )",
            params![
                job.id,
                job.name,
                job.prompt,
                schedule_json,
                job.scheduled_at,
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
    let schedule_json =
        serde_json::to_string(&job.schedule).map_err(|error| format!("无法序列化周期：{error}"))?;
    let knowledge_base_ids_json = serde_json::to_string(&job.knowledge_base_ids)
        .map_err(|error| format!("无法序列化知识库范围：{error}"))?;
    let explicit_skill_ids_json = serde_json::to_string(&job.explicit_skill_ids)
        .map_err(|error| format!("无法序列化 Skill：{error}"))?;
    connection
        .execute(
            "UPDATE scheduled_jobs SET
                name = ?2, prompt = ?3, schedule_json = ?4, scheduled_at = ?5, timezone = ?6,
                knowledge_base_ids_json = ?7, output_mode = ?8, inbox_knowledge_base_id = ?9,
                inbox_folder = ?10, session_policy = ?11, continued_session_id = ?12,
                model_provider_id = ?13, model_id = ?14, explicit_skill_ids_json = ?15,
                enabled = ?16, next_run_at = ?17, last_run_at = ?18, last_status = ?19,
                last_error = ?20, last_session_id = ?21, failure_streak = ?22, updated_at = ?23
             WHERE id = ?1",
            params![
                job.id,
                job.name,
                job.prompt,
                schedule_json,
                job.scheduled_at,
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
    let scheduled_at: Option<String> = row.get(4)?;
    let knowledge_base_ids_json: String = row.get(6)?;
    let explicit_skill_ids_json: String = row.get(14)?;
    let schedule: JobSchedule =
        serde_json::from_str(&schedule_json).unwrap_or(JobSchedule::Daily {
            time: "08:00".to_owned(),
        });
    let knowledge_base_ids: Vec<String> =
        serde_json::from_str(&knowledge_base_ids_json).unwrap_or_default();
    let explicit_skill_ids: Vec<String> =
        serde_json::from_str(&explicit_skill_ids_json).unwrap_or_default();
    let enabled: i64 = row.get(15)?;
    Ok(ScheduledJob {
        id: row.get(0)?,
        name: row.get(1)?,
        prompt: row.get(2)?,
        schedule_display: schedule_display(&schedule),
        schedule,
        scheduled_at,
        timezone: row.get(5)?,
        knowledge_base_ids,
        output_mode: row.get(7)?,
        inbox_knowledge_base_id: row.get(8)?,
        inbox_folder: row.get(9)?,
        session_policy: row.get(10)?,
        continued_session_id: row.get(11)?,
        model_provider_id: row.get(12)?,
        model_id: row.get(13)?,
        explicit_skill_ids,
        enabled: enabled != 0,
        next_run_at: row.get(16)?,
        last_run_at: row.get(17)?,
        last_status: row.get(18)?,
        last_error: row.get(19)?,
        last_session_id: row.get(20)?,
        failure_streak: row.get(21)?,
        created_at: row.get(22)?,
        updated_at: row.get(23)?,
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
            scheduled_at: None,
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
    fn dispatch_tolerance_is_shared_by_all_schedule_kinds() {
        assert_eq!(dispatch_tolerance_secs(), SCHEDULE_DISPATCH_TOLERANCE_SECS);
    }

    #[test]
    fn scheduled_at_round_trips_for_agenda_job() {
        let connection = memory_db();
        let slot = format_utc(local(2026, 9, 18, 8, 0).with_timezone(&Utc));
        let mut job = sample_job(JobSchedule::Once { at: slot.clone() }, &slot);
        job.scheduled_at = Some(slot.clone());
        insert_scheduled_job_on(&connection, &job).unwrap();

        let stored = get_scheduled_job_on(&connection, "sched-test")
            .unwrap()
            .unwrap();
        assert_eq!(stored.scheduled_at.as_deref(), Some(slot.as_str()));
    }

    /** 编辑已结束日程时，表单仍会提交日期，不能重置原定时间或重新启用。 */
    #[test]
    fn editing_finished_once_job_preserves_calendar_date_and_disabled_state() {
        for status in ["skipped", "ok"] {
            let connection = memory_db();
            let slot_local = local(2026, 9, 18, 8, 0);
            let slot = format_utc(slot_local.with_timezone(&Utc));
            let mut job = sample_job(JobSchedule::Once { at: slot.clone() }, &slot);
            job.scheduled_at = Some(slot.clone());
            job.next_run_at = None;
            job.enabled = false;
            job.last_status = Some(status.to_owned());
            insert_scheduled_job_on(&connection, &job).unwrap();

            // 编辑器会把 UTC 转成本地 datetime-local，语义相同的时间不算改期。
            let payload = serde_json::from_value(serde_json::json!({
                "jobId": job.id,
                "name": "修改后的任务名称",
                "schedule": { "type": "once", "at": slot_local.format("%Y-%m-%dT%H:%M").to_string() }
            }))
            .unwrap();
            let saved = update_scheduled_job_on(
                &connection,
                payload,
                slot_local + ChronoDuration::hours(2),
            )
            .unwrap();

            assert_eq!(saved.scheduled_at.as_deref(), Some(slot.as_str()));
            assert!(saved.next_run_at.is_none());
            assert!(!saved.enabled);
            assert_eq!(saved.last_status.as_deref(), Some(status));
            let stored = get_scheduled_job_on(&connection, &job.id).unwrap().unwrap();
            assert_eq!(stored.scheduled_at, saved.scheduled_at);
        }
    }

    /** 更改一次性日程的日期时，日历保存原定时间，即使该日期仍在过去。 */
    #[test]
    fn changing_once_date_keeps_calendar_date_independent_of_next_run() {
        for future in [false, true] {
            let connection = memory_db();
            let slot_local = local(2026, 9, 18, 8, 0);
            let slot = format_utc(slot_local.with_timezone(&Utc));
            let mut job = sample_job(JobSchedule::Once { at: slot.clone() }, &slot);
            job.scheduled_at = Some(slot);
            job.next_run_at = None;
            job.enabled = false;
            job.last_status = Some("skipped".to_owned());
            insert_scheduled_job_on(&connection, &job).unwrap();

            let now = slot_local + ChronoDuration::hours(2);
            let new_date = now + ChronoDuration::hours(if future { 1 } else { -1 });
            let new_slot = format_utc(new_date.with_timezone(&Utc));
            let payload = serde_json::from_value(serde_json::json!({
                "jobId": job.id,
                "schedule": { "type": "once", "at": new_slot }
            }))
            .unwrap();
            let saved = update_scheduled_job_on(&connection, payload, now).unwrap();

            assert_eq!(saved.scheduled_at.as_deref(), Some(new_slot.as_str()));
            assert_eq!(
                saved.next_run_at.as_deref(),
                future.then_some(new_slot.as_str())
            );
            assert_eq!(saved.enabled, future);
        }
    }

    #[test]
    fn startup_settlement_marks_overdue_once_job_missed() {
        let connection = memory_db();
        let slot_local = local(2026, 9, 18, 8, 0);
        let slot = format_utc(slot_local.with_timezone(&Utc));
        let job = sample_job(JobSchedule::Once { at: slot.clone() }, &slot);
        insert_scheduled_job_on(&connection, &job).unwrap();

        let now = (slot_local + ChronoDuration::hours(2)).with_timezone(&Utc);
        let settled = settle_missed_jobs_on_start(&connection, now).unwrap();
        assert_eq!(settled.len(), 1);

        let stored = get_scheduled_job_on(&connection, "sched-test")
            .unwrap()
            .unwrap();
        assert_eq!(stored.last_status.as_deref(), Some("skipped"));
        assert!(!stored.enabled);
        assert!(stored.next_run_at.is_none());

        let status: String = connection
            .query_row(
                "SELECT status FROM scheduled_job_runs WHERE job_id = 'sched-test'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "skipped");
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

        let now = (slot_local + ChronoDuration::seconds(30)).with_timezone(&Utc);
        let first = claim_due_jobs(&connection, now, 1).unwrap();
        let second = claim_due_jobs(&connection, now, 1).unwrap();

        assert_eq!(first.claimed.len(), 1);
        assert_eq!(first.claimed[0].scheduled_instant, slot);
        assert!(second.claimed.is_empty());
        let stored = get_scheduled_job_on(&connection, "sched-test")
            .unwrap()
            .unwrap();
        let next = parse_utc(stored.next_run_at.as_deref().unwrap()).unwrap();
        assert!(next > now);
    }

    /** 同时到期和执行期间新到期的任务都先认领，等待超过窗口也不能变成 skipped。 */
    #[test]
    fn due_batches_are_claimed_before_prior_runs_finish() {
        let connection = memory_db();
        let start = local(2026, 9, 18, 8, 0).with_timezone(&Utc);
        for (id, delay_minutes) in [("first", 0), ("second", 0), ("later", 1)] {
            let slot = format_utc(start + ChronoDuration::minutes(delay_minutes));
            let mut job = sample_job(JobSchedule::Once { at: slot.clone() }, &slot);
            job.id = id.to_owned();
            insert_scheduled_job_on(&connection, &job).unwrap();
        }

        let first =
            claim_due_jobs(&connection, start + ChronoDuration::seconds(30), usize::MAX).unwrap();
        assert_eq!(first.claimed.len(), 2);
        // 前一批还未完成时，新到期任务仍然能够被 tick 认领。
        let later =
            claim_due_jobs(&connection, start + ChronoDuration::seconds(90), usize::MAX).unwrap();
        assert_eq!(later.claimed.len(), 1);
        let after_wait =
            claim_due_jobs(&connection, start + ChronoDuration::minutes(5), usize::MAX).unwrap();
        assert!(after_wait.claimed.is_empty());
        assert!(after_wait.skipped.is_empty());

        for run in first.claimed.into_iter().chain(later.claimed) {
            let stored = finish_scheduled_run(
                &connection,
                &run.run_id,
                &run.job.id,
                "ok",
                None,
                None,
                None,
            )
            .unwrap();
            assert_eq!(stored.last_status.as_deref(), Some("ok"));
        }
    }

    /** 混合批次既返回准点任务，也返回需要前端同步的已错过任务。 */
    #[test]
    fn due_batch_returns_skipped_jobs_alongside_claimed_jobs() {
        let connection = memory_db();
        let start = local(2026, 9, 18, 8, 0).with_timezone(&Utc);
        for (id, delay_minutes) in [("missed", 0), ("on-time", 2)] {
            let slot = format_utc(start + ChronoDuration::minutes(delay_minutes));
            let mut job = sample_job(JobSchedule::Once { at: slot.clone() }, &slot);
            job.id = id.to_owned();
            insert_scheduled_job_on(&connection, &job).unwrap();
        }

        let now = start + ChronoDuration::seconds(150);
        let result = claim_due_jobs(&connection, now, usize::MAX).unwrap();
        assert_eq!(result.claimed.len(), 1);
        assert_eq!(result.claimed[0].job.id, "on-time");
        assert_eq!(result.skipped.len(), 1);
        assert_eq!(result.skipped[0].id, "missed");
        assert_eq!(result.skipped[0].last_status.as_deref(), Some("skipped"));
        assert!(!result.skipped[0].enabled);
        assert!(result.skipped[0].next_run_at.is_none());
        let second = claim_due_jobs(&connection, now, usize::MAX).unwrap();
        assert!(second.skipped.is_empty());
    }

    /** 超过调度容忍窗口的每日任务只跳拍，不派发，并返回更新后的任务。 */
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
        assert!(claimed.claimed.is_empty());
        assert_eq!(claimed.skipped.len(), 1);
        assert_eq!(claimed.skipped[0].last_status.as_deref(), Some("skipped"));

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
