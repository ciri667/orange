use serde::{Deserialize, Serialize};

/** 默认定时产出文件夹，位于所选知识库根目录下。 */
pub const DEFAULT_SCHEDULE_INBOX_FOLDER: &str = "定时产出";

/** 连续失败达到该次数后自动暂停，避免坏任务反复烧 token。 */
pub const SCHEDULE_FAILURE_PAUSE_STREAK: i64 = 3;

/** 漏跑补跑窗口下限。 */
pub const SCHEDULE_CATCH_UP_MIN_SECS: i64 = 120;

/** 漏跑补跑窗口上限，避免合盖一晚连跑多次每日任务。 */
pub const SCHEDULE_CATCH_UP_MAX_SECS: i64 = 2 * 60 * 60;

/** 定时任务产出方式；首版只跑会话，收件箱写入留给后续阶段。 */
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ScheduleOutputMode {
    Session,
    Inbox,
}

impl ScheduleOutputMode {
    pub fn parse(value: &str) -> Self {
        match value {
            "inbox" => Self::Inbox,
            _ => Self::Session,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Inbox => "inbox",
        }
    }
}

/** 定时任务会话策略：每次新建，或在同一会话上续跑。 */
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ScheduleSessionPolicy {
    NewEachRun,
    Continue,
}

impl ScheduleSessionPolicy {
    pub fn parse(value: &str) -> Self {
        match value {
            "continue" => Self::Continue,
            _ => Self::NewEachRun,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NewEachRun => "newEachRun",
            Self::Continue => "continue",
        }
    }
}

/** 与 Codex 对齐的星期枚举，序列化为 MO/TU/... */
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum JobWeekday {
    Mo,
    Tu,
    We,
    Th,
    Fr,
    Sa,
    Su,
}

impl JobWeekday {
    pub fn as_chrono(self) -> chrono::Weekday {
        match self {
            Self::Mo => chrono::Weekday::Mon,
            Self::Tu => chrono::Weekday::Tue,
            Self::We => chrono::Weekday::Wed,
            Self::Th => chrono::Weekday::Thu,
            Self::Fr => chrono::Weekday::Fri,
            Self::Sa => chrono::Weekday::Sat,
            Self::Su => chrono::Weekday::Sun,
        }
    }

    #[allow(dead_code)]
    pub fn from_chrono(weekday: chrono::Weekday) -> Self {
        match weekday {
            chrono::Weekday::Mon => Self::Mo,
            chrono::Weekday::Tue => Self::Tu,
            chrono::Weekday::Wed => Self::We,
            chrono::Weekday::Thu => Self::Th,
            chrono::Weekday::Fri => Self::Fr,
            chrono::Weekday::Sat => Self::Sa,
            chrono::Weekday::Sun => Self::Su,
        }
    }

    pub fn display_zh(self) -> &'static str {
        match self {
            Self::Mo => "星期一",
            Self::Tu => "星期二",
            Self::We => "星期三",
            Self::Th => "星期四",
            Self::Fr => "星期五",
            Self::Sa => "星期六",
            Self::Su => "星期日",
        }
    }
}

/** 结构化周期；不做 crontab 或自然语言解析。 */
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum JobSchedule {
    #[serde(rename_all = "camelCase")]
    Hourly {
        interval_hours: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        days: Option<Vec<JobWeekday>>,
    },
    #[serde(rename_all = "camelCase")]
    Daily { time: String },
    #[serde(rename_all = "camelCase")]
    Weekdays { time: String },
    #[serde(rename_all = "camelCase")]
    Weekly {
        days: Vec<JobWeekday>,
        time: String,
    },
    #[serde(rename_all = "camelCase")]
    Once { at: String },
}

/** 绑定到 Agent 会话的定时任务身份，与 IM 身份并列。 */
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleSessionIdentity {
    pub job_id: String,
    pub job_name: String,
}

/** 一条持久化定时任务。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledJob {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub schedule: JobSchedule,
    pub schedule_display: String,
    pub timezone: String,
    pub knowledge_base_ids: Vec<String>,
    pub output_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbox_knowledge_base_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbox_folder: Option<String>,
    pub session_policy: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continued_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub explicit_skill_ids: Vec<String>,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_session_id: Option<String>,
    pub failure_streak: i64,
    pub created_at: String,
    pub updated_at: String,
}

/** 一次调度拍的执行记录。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledJobRun {
    pub id: String,
    pub job_id: String,
    pub scheduled_instant: String,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/** 建议模板；用户点选后填少量槽位再变成真实任务。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleBlueprint {
    pub key: String,
    pub name: String,
    pub description: String,
    pub prompt: String,
    pub schedule: JobSchedule,
    pub schedule_display: String,
    pub session_policy: String,
    pub output_mode: String,
    pub needs_topic: bool,
}

/** 创建定时任务命令入参。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateScheduledJobPayload {
    pub name: String,
    pub prompt: String,
    pub schedule: JobSchedule,
    pub knowledge_base_ids: Vec<String>,
    #[serde(default)]
    pub output_mode: Option<String>,
    #[serde(default)]
    pub inbox_knowledge_base_id: Option<String>,
    #[serde(default)]
    pub inbox_folder: Option<String>,
    #[serde(default)]
    pub session_policy: Option<String>,
    #[serde(default)]
    pub model_provider_id: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub explicit_skill_ids: Vec<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

/** 更新定时任务命令入参；未出现的字段保持原值。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateScheduledJobPayload {
    pub job_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub schedule: Option<JobSchedule>,
    #[serde(default)]
    pub knowledge_base_ids: Option<Vec<String>>,
    #[serde(default)]
    pub output_mode: Option<String>,
    #[serde(default)]
    pub inbox_knowledge_base_id: Option<String>,
    #[serde(default)]
    pub inbox_folder: Option<String>,
    #[serde(default)]
    pub session_policy: Option<String>,
    #[serde(default)]
    pub model_provider_id: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub explicit_skill_ids: Option<Vec<String>>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

/** 只带任务 ID 的命令入参。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledJobIdPayload {
    pub job_id: String,
}

/** 读取运行历史的命令入参。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadScheduledJobRunsPayload {
    pub job_id: String,
    #[serde(default)]
    pub limit: Option<u32>,
}

/** 调度器派发完成后推给前端的轻量事件。 */
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledJobUpdatedEvent {
    pub job: ScheduledJob,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/** 把结构化周期格式化成中文短句，供列表展示。 */
pub fn schedule_display(schedule: &JobSchedule) -> String {
    match schedule {
        JobSchedule::Daily { time } => format!("每天 {}", format_clock(time)),
        JobSchedule::Weekdays { time } => format!("工作日 {}", format_clock(time)),
        JobSchedule::Weekly { days, time } => {
            let names = days
                .iter()
                .map(|day| day.display_zh())
                .collect::<Vec<_>>()
                .join("、");
            if names.is_empty() {
                format!("每周 {}", format_clock(time))
            } else {
                format!("{names} {}", format_clock(time))
            }
        }
        JobSchedule::Hourly {
            interval_hours,
            days,
        } => {
            let interval = if *interval_hours <= 1 {
                "每小时".to_owned()
            } else {
                format!("每 {interval_hours} 小时")
            };
            match days {
                Some(days) if !days.is_empty() => {
                    let names = days
                        .iter()
                        .map(|day| day.display_zh())
                        .collect::<Vec<_>>()
                        .join("、");
                    format!("{interval}（{names}）")
                }
                _ => interval,
            }
        }
        JobSchedule::Once { at } => format!("一次 {at}"),
    }
}

fn format_clock(time: &str) -> String {
    time.trim().chars().take(5).collect()
}

/** 内置建议模板，对应 Codex 空态的每日简报 / 每周回顾 / 跟进。 */
pub fn schedule_blueprints() -> Vec<ScheduleBlueprint> {
    vec![
        ScheduleBlueprint {
            key: "daily-briefing".to_owned(),
            name: "每日简报".to_owned(),
            description: "每个工作日检索知识库近期改动，整理成可扫读的简报。".to_owned(),
            prompt: "请检索当前知识库范围内最近 24 小时有更新的笔记，整理一份简洁的每日简报：今日重点、未完成事项、值得跟进的问题。用中文短段落，不要编造不存在的笔记内容。若没有任何更新，明确说明并给出 1-2 条基于现有笔记的建议。".to_owned(),
            schedule: JobSchedule::Weekdays {
                time: "08:00".to_owned(),
            },
            schedule_display: "工作日 8:00".to_owned(),
            session_policy: ScheduleSessionPolicy::NewEachRun.as_str().to_owned(),
            output_mode: ScheduleOutputMode::Session.as_str().to_owned(),
            needs_topic: false,
        },
        ScheduleBlueprint {
            key: "weekly-review".to_owned(),
            name: "每周回顾".to_owned(),
            description: "每周五把最近的笔记整理成简明的状态更新。".to_owned(),
            prompt: "请检索当前知识库范围内最近 7 天的笔记，写一份每周回顾：本周完成了什么、仍未完成的事项、建议下周优先关注的 3 件事。引用具体笔记标题，不要编造。".to_owned(),
            schedule: JobSchedule::Weekly {
                days: vec![JobWeekday::Fr],
                time: "16:00".to_owned(),
            },
            schedule_display: "星期五 16:00".to_owned(),
            session_policy: ScheduleSessionPolicy::NewEachRun.as_str().to_owned(),
            output_mode: ScheduleOutputMode::Session.as_str().to_owned(),
            needs_topic: false,
        },
        ScheduleBlueprint {
            key: "topic-watch".to_owned(),
            name: "主题跟进".to_owned(),
            description: "每个工作日查看指定主题在知识库中的新增，并标出需要关注的事项。".to_owned(),
            prompt: "请围绕主题「{topic}」检索当前知识库。只报告相对上次（若有会话历史）的新增或变化；没有新增时回复「暂无需要关注的更新」，不要重复旧内容。标出需要用户关注的事项。".to_owned(),
            schedule: JobSchedule::Weekdays {
                time: "09:00".to_owned(),
            },
            schedule_display: "工作日 9:00".to_owned(),
            session_policy: ScheduleSessionPolicy::Continue.as_str().to_owned(),
            output_mode: ScheduleOutputMode::Session.as_str().to_owned(),
            needs_topic: true,
        },
    ]
}
