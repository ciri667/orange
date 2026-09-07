use crate::logging::{self, AppEventBuilder, AppLogCategory, AppLogLevel};
use serde::Serialize;
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use tauri::AppHandle;

/** 已启动的 sidecar 进程句柄；stdin 在写入配置后关闭。 */
pub(crate) struct SpawnedSidecar {
    pub child: Child,
    pub stdout: ChildStdout,
    pub stderr: ChildStderr,
}

/** 启动 IM sidecar：配置经 stdin JSON 注入，secret 不出现在命令行。 */
pub(crate) fn spawn_im_sidecar(
    app: &AppHandle,
    provider_id: &str,
    binary_name: &str,
    config: &impl Serialize,
) -> Result<SpawnedSidecar, String> {
    let sidecar_path = super::sidecar_binary_path(app, provider_id, binary_name)?;
    let mut child = Command::new(&sidecar_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            format!(
                "无法启动 IM sidecar {}：{error}",
                sidecar_path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or(binary_name)
            )
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        let config_line = serde_json::to_string(config)
            .map_err(|error| format!("无法序列化 IM sidecar 配置：{error}"))?;
        writeln!(stdin, "{config_line}")
            .map_err(|error| format!("无法写入 IM sidecar 配置：{error}"))?;
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "IM sidecar 未提供 stdout。".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "IM sidecar 未提供 stderr。".to_owned())?;

    Ok(SpawnedSidecar {
        child,
        stdout,
        stderr,
    })
}

/** 后台读取 sidecar stderr，只写脱敏运行日志。 */
pub(crate) fn spawn_stderr_reader(app: AppHandle, provider_id: String, stderr: ChildStderr) {
    tauri::async_runtime::spawn_blocking(move || {
        let reader = BufReader::new(stderr);

        for line in reader.lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }

            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    AppLogLevel::Warn,
                    AppLogCategory::Im,
                    "im_gateway_stderr",
                    "failed",
                    logging::sanitize_log_text(&line),
                )
                .metadata(json!({ "providerId": provider_id })),
            );
        }
    });
}

/** 记录 sidecar stdout 中的非事件内容。 */
pub(crate) fn record_stdout_noise(app: &AppHandle, provider_id: &str, line: &str) {
    logging::write_app_event_best_effort(
        app,
        AppEventBuilder::new(
            AppLogLevel::Warn,
            AppLogCategory::Im,
            "im_gateway_stdout_ignored",
            "skipped",
            logging::sanitize_log_text(line),
        )
        .metadata(json!({ "providerId": provider_id })),
    );
}
