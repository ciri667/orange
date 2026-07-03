use crate::domain::AppEventLog;
use crate::storage;
use chrono::{Duration as ChronoDuration, Local};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tauri::{AppHandle, Manager};

/** 文件诊断日志最长保留天数，避免发布版 app log 目录长期膨胀。 */
const FILE_LOG_RETENTION_DAYS: i64 = 14;

/** 单条事件消息最大长度，避免错误对象或第三方响应把日志撑得过大。 */
const MAX_EVENT_MESSAGE_CHARS: usize = 600;

/** 支持的应用事件日志级别，和前端筛选项保持一致。 */
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub enum AppLogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl AppLogLevel {
    /** 返回写入 SQLite 和前端展示使用的小写级别。 */
    fn as_str(self) -> &'static str {
        match self {
            AppLogLevel::Debug => "debug",
            AppLogLevel::Info => "info",
            AppLogLevel::Warn => "warn",
            AppLogLevel::Error => "error",
        }
    }
}

/** 支持的应用事件分类，约束日志入口，不让模块自由拼写分类。 */
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub enum AppLogCategory {
    App,
    Storage,
    KnowledgeBase,
    Editor,
    Agent,
    Im,
    Model,
    Skill,
    Settings,
    Security,
    Frontend,
}

impl AppLogCategory {
    /** 返回写入 SQLite 和前端展示使用的分类标识。 */
    fn as_str(self) -> &'static str {
        match self {
            AppLogCategory::App => "app",
            AppLogCategory::Storage => "storage",
            AppLogCategory::KnowledgeBase => "knowledge_base",
            AppLogCategory::Editor => "editor",
            AppLogCategory::Agent => "agent",
            AppLogCategory::Im => "im",
            AppLogCategory::Model => "model",
            AppLogCategory::Skill => "skill",
            AppLogCategory::Settings => "settings",
            AppLogCategory::Security => "security",
            AppLogCategory::Frontend => "frontend",
        }
    }
}

/** 应用事件日志构造器，避免命令层重复铺开完整 AppEventLog 结构。 */
#[derive(Clone, Debug)]
pub struct AppEventBuilder {
    level: AppLogLevel,
    category: AppLogCategory,
    event: String,
    message: String,
    status: String,
    operation_id: Option<String>,
    session_id: Option<String>,
    knowledge_base_id: Option<String>,
    entity_type: Option<String>,
    entity_id: Option<String>,
    relative_path: Option<String>,
    duration_ms: Option<i64>,
    metadata_json: Option<String>,
}

impl AppEventBuilder {
    /** 创建一条最小应用事件，后续通过链式方法补齐上下文。 */
    pub fn new(
        level: AppLogLevel,
        category: AppLogCategory,
        event: impl Into<String>,
        status: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            level,
            category,
            event: event.into(),
            message: sanitize_log_text(&message.into()),
            status: status.into(),
            operation_id: None,
            session_id: None,
            knowledge_base_id: None,
            entity_type: None,
            entity_id: None,
            relative_path: None,
            duration_ms: None,
            metadata_json: None,
        }
    }

    /** 记录同一次业务操作的关联 ID，便于 start/completed/failed 串起来。 */
    pub fn operation_id(mut self, operation_id: impl Into<String>) -> Self {
        self.operation_id = Some(operation_id.into());
        self
    }

    /** 记录 Agent 会话 ID，不记录消息正文。 */
    pub fn session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /** 记录知识库 ID，不记录知识库绝对路径。 */
    pub fn knowledge_base_id(mut self, knowledge_base_id: impl Into<String>) -> Self {
        self.knowledge_base_id = Some(knowledge_base_id.into());
        self
    }

    /** 记录业务实体类型和 ID，例如 note/document/session。 */
    pub fn entity(mut self, entity_type: impl Into<String>, entity_id: impl Into<String>) -> Self {
        self.entity_type = Some(entity_type.into());
        self.entity_id = Some(entity_id.into());
        self
    }

    /** 记录知识库内相对路径；调用方不得传入绝对路径。 */
    pub fn relative_path(mut self, relative_path: impl Into<String>) -> Self {
        self.relative_path = Some(sanitize_relative_path(&relative_path.into()));
        self
    }

    /** 记录操作耗时，统一转换为毫秒。 */
    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration_ms = Some(duration.as_millis().min(i64::MAX as u128) as i64);
        self
    }

    /** 记录结构化轻量元数据；调用方只能放计数、状态、模型名等脱敏字段。 */
    pub fn metadata(mut self, metadata: Value) -> Self {
        self.metadata_json = Some(sanitize_log_text(&metadata.to_string()));
        self
    }

    /** 转换为持久化模型，补齐 ID 和创建时间。 */
    fn build(self) -> AppEventLog {
        AppEventLog {
            id: storage::create_id("event"),
            level: self.level.as_str().to_owned(),
            category: self.category.as_str().to_owned(),
            event: self.event,
            message: self.message,
            status: self.status,
            operation_id: self.operation_id,
            session_id: self.session_id,
            knowledge_base_id: self.knowledge_base_id,
            entity_type: self.entity_type,
            entity_id: self.entity_id,
            relative_path: self.relative_path,
            duration_ms: self.duration_ms,
            metadata_json: self.metadata_json,
            created_at: storage::format_local_datetime(),
        }
    }
}

/** 写入一条应用事件日志；失败会向调用方返回，适合命令本身就是日志操作时使用。 */
pub fn write_app_event(app: &AppHandle, event: AppEventBuilder) -> Result<(), String> {
    let log = event.build();

    write_diagnostic_log(&log);
    storage::append_app_event_log(app, &log)
}

/** 尽力写入应用事件日志；失败只写诊断日志，不影响用户当前操作。 */
pub fn write_app_event_best_effort(app: &AppHandle, event: AppEventBuilder) {
    if let Err(error) = write_app_event(app, event) {
        log::warn!(target: "logging", "应用事件日志写入失败：{}", sanitize_log_text(&error));
    }
}

/** 返回 Tauri app log 目录路径，用于打开目录或清理旧日志。 */
pub fn app_log_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_log_dir()
        .map_err(|error| format!("无法定位应用日志目录：{error}"))
}

/** 清理 14 天前的 .log 文件，保留插件当前正在写入的新日志。 */
pub fn cleanup_old_file_logs(app: &AppHandle) -> Result<usize, String> {
    let log_dir = app_log_dir(app)?;

    if !log_dir.exists() {
        return Ok(0);
    }

    let retention_cutoff = Local::now() - ChronoDuration::days(FILE_LOG_RETENTION_DAYS);
    let mut removed_count = 0;

    for entry in fs::read_dir(&log_dir).map_err(|error| format!("无法读取应用日志目录：{error}"))?
    {
        let entry = entry.map_err(|error| format!("无法读取应用日志文件：{error}"))?;
        let path = entry.path();

        // 只清理插件生成的 .log 文件，避免误删同目录下其他诊断资料。
        if path.extension().and_then(|value| value.to_str()) != Some("log") {
            continue;
        }

        let metadata = entry
            .metadata()
            .map_err(|error| format!("无法读取应用日志文件元数据：{error}"))?;
        let Ok(modified_at) = metadata.modified() else {
            continue;
        };
        let modified_at: chrono::DateTime<Local> = modified_at.into();

        if modified_at >= retention_cutoff {
            continue;
        }

        fs::remove_file(&path).map_err(|error| {
            format!(
                "无法清理过期应用日志文件 {}：{error}",
                path.file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("<unknown>")
            )
        })?;
        removed_count += 1;
    }

    Ok(removed_count)
}

/** 将用户可读事件同步写入诊断日志文件，便于从系统日志目录排查。 */
fn write_diagnostic_log(log: &AppEventLog) {
    let message = format!(
        "category={} event={} status={} op={} kb={} entity={}:{} path={} duration_ms={} {}",
        log.category,
        log.event,
        log.status,
        log.operation_id.as_deref().unwrap_or("-"),
        log.knowledge_base_id.as_deref().unwrap_or("-"),
        log.entity_type.as_deref().unwrap_or("-"),
        log.entity_id.as_deref().unwrap_or("-"),
        log.relative_path.as_deref().unwrap_or("-"),
        log.duration_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_owned()),
        log.message
    );

    match log.level.as_str() {
        "debug" => log::debug!(target: "app_event", "{message}"),
        "info" => log::info!(target: "app_event", "{message}"),
        "warn" => log::warn!(target: "app_event", "{message}"),
        "error" => log::error!(target: "app_event", "{message}"),
        _ => log::info!(target: "app_event", "{message}"),
    }
}

/** 对日志文本做基础脱敏和长度限制，防止密钥或大段正文进入日志。 */
pub fn sanitize_log_text(text: &str) -> String {
    let redacted = redact_sensitive_tokens(text);
    let collapsed = redacted.split_whitespace().collect::<Vec<_>>().join(" ");

    if collapsed.chars().count() <= MAX_EVENT_MESSAGE_CHARS {
        return collapsed;
    }

    let truncated = collapsed
        .chars()
        .take(MAX_EVENT_MESSAGE_CHARS)
        .collect::<String>();

    format!("{truncated}...")
}

/** 相对路径只保留知识库内路径片段，并去掉可能暴露系统目录的路径穿越符号。 */
fn sanitize_relative_path(path: &str) -> String {
    path.replace('\\', "/")
        .split('/')
        .filter(|part| !part.is_empty() && *part != "." && *part != "..")
        .collect::<Vec<_>>()
        .join("/")
}

/** 粗略识别常见 API key 和绝对路径片段，日志中统一替换为占位符。 */
fn redact_sensitive_tokens(text: &str) -> String {
    text.split_whitespace()
        .map(|token| {
            let lower_token = token.to_lowercase();

            if token.starts_with("sk-")
                || token.starts_with("sess-")
                || lower_token.contains("api_key")
                || lower_token.contains("apikey")
                || lower_token.contains("authorization:")
                || lower_token.contains("bearer")
            {
                "[redacted]".to_owned()
            } else if looks_like_absolute_path(token) {
                "[path]".to_owned()
            } else {
                token.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/** 判断一个空白分隔 token 是否像本机绝对路径，避免知识库或系统目录进入日志。 */
fn looks_like_absolute_path(token: &str) -> bool {
    let trimmed = token.trim_matches(|character: char| {
        matches!(
            character,
            '"' | '\''
                | '`'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '<'
                | '>'
                | ','
                | '，'
                | '。'
                | ':'
                | '：'
                | ';'
                | '；'
        )
    });
    let bytes = trimmed.as_bytes();
    let is_windows_drive_path = bytes.len() >= 3
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
        && bytes[0].is_ascii_alphabetic();

    trimmed.starts_with('/') || trimmed.starts_with("~/") || is_windows_drive_path
}

#[cfg(test)]
mod tests {
    use super::{sanitize_log_text, sanitize_relative_path};

    /** 日志脱敏必须移除常见 API key 片段，避免明文密钥落盘。 */
    #[test]
    fn sanitize_log_text_redacts_api_keys() {
        let sanitized = sanitize_log_text("request failed with sk-test-secret api_key=abc");

        assert!(!sanitized.contains("sk-test-secret"));
        assert!(!sanitized.contains("api_key=abc"));
        assert!(sanitized.contains("[redacted]"));
    }

    /** 日志脱敏必须移除本机绝对路径，避免知识库目录落入诊断文件或事件表。 */
    #[test]
    fn sanitize_log_text_redacts_absolute_paths() {
        let sanitized = sanitize_log_text(
            "无法授权 Markdown 图片预览目录 /Users/vg/Documents/KnowledgeBase：denied",
        );

        assert!(!sanitized.contains("/Users/vg/Documents/KnowledgeBase"));
        assert!(sanitized.contains("[path]"));
    }

    /** 相对路径脱敏必须去掉路径穿越片段，避免日志展示越界路径。 */
    #[test]
    fn sanitize_relative_path_removes_traversal_parts() {
        assert_eq!(sanitize_relative_path("../a/./b.md"), "a/b.md");
    }
}
