use crate::logging::{self, AppEventBuilder, AppLogCategory, AppLogLevel};
use serde::Serialize;
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use tauri::AppHandle;

/** 已启动的 sidecar 进程句柄；配置写入后 stdin 交给调用方，不需要回复通道的 provider 丢弃即可关闭。 */
pub(crate) struct SpawnedSidecar {
    pub child: Child,
    pub stdout: ChildStdout,
    pub stderr: ChildStderr,
    pub stdin: Option<ChildStdin>,
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

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "IM sidecar 未提供 stdin。".to_owned())?;
    let config_line = serde_json::to_string(config)
        .map_err(|error| format!("无法序列化 IM sidecar 配置：{error}"))?;
    writeln!(stdin, "{config_line}")
        .map_err(|error| format!("无法写入 IM sidecar 配置：{error}"))?;

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
        stdin: Some(stdin),
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

/**
 * 检查 sidecar 是否已经退出，退出时顺手回收句柄。
 *
 * sidecar 是独立进程，崩溃、被系统回收或 Go 运行时致命错误都会让它悄无声息地消失；
 * 而网关状态此前只在显式 start/stop 时变化，于是界面会长期显示「运行中」却收不到任何
 * 消息。这里用非阻塞的 `try_wait` 把「进程还在不在」变成状态的唯一可信来源。
 *
 * 返回 `Some(描述)` 表示刚刚发现它已退出（`child` 已被取走，不会重复报告）；
 * 返回 `None` 表示仍在运行，或本来就没有启动过。
 */
pub(crate) fn take_exited_sidecar(child: &mut Option<Child>) -> Option<String> {
    let handle = child.as_mut()?;
    match handle.try_wait() {
        Ok(Some(status)) => {
            // 已经回收，必须清空句柄，避免下一次 poll 继续报告同一进程。
            *child = None;
            Some(format!("sidecar 已退出（{}）。", describe_exit_status(&status)))
        }
        // 仍在运行。
        Ok(None) => None,
        // 查询失败不等于已退出：保留句柄与 running，下一次状态轮询会再试一次。
        // 把一次瞬时查询错误当成进程死亡，会让好好的网关在界面上被误停。
        Err(error) => {
            log::warn!(
                target: "im",
                "读取 IM sidecar 状态失败，本次不改动网关状态：{error}"
            );
            None
        }
    }
}

/** 把退出状态转成不含路径的短描述，供状态栏和事件日志复用。 */
fn describe_exit_status(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("退出码 {code}"),
        None => "被信号终止".to_owned(),
    }
}

/**
 * 网关状态探测的共用流程：先判定进程是否已退出，再更新 running/connected/last_error。
 *
 * 四个 provider 的状态结构各自独立，但「退出后必须把 running 置假并记录原因」这条规则
 * 完全一致。把字段借用集中到一个结构体，是为了让这条规则只有一份实现——此前正是
 * 「一处加了存活检测、另一处忘了」的典型来源。
 *
 * `probe()` 返回 `Some(退出描述)` 表示本次调用刚刚发现进程退出，调用方应在
 * **释放状态锁之后**再写事件日志，避免文件 IO 拉长临界区。
 */
pub(crate) struct GatewayLiveness<'a> {
    pub child: &'a mut Option<Child>,
    pub running: &'a mut bool,
    pub connected: &'a mut bool,
    pub last_error: &'a mut Option<String>,
}

impl GatewayLiveness<'_> {
    /** 检测并应用退出结果；返回本次首次发现的退出描述。 */
    pub(crate) fn probe(&mut self) -> Option<String> {
        let description = take_exited_sidecar(self.child)?;
        *self.running = false;
        *self.connected = false;
        *self.last_error = Some(description.clone());
        Some(description)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /** 启动一个立刻退出的子进程，返回已经 wait 不到它的句柄。 */
    fn exited_child() -> Child {
        #[cfg(windows)]
        let child = Command::new("cmd")
            .args(["/C", "exit 7"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn cmd");
        #[cfg(unix)]
        let child = Command::new("sh")
            .args(["-c", "exit 7"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sh");
        let mut child = child;
        // 先阻塞等到它真的结束，避免 try_wait 抢在退出前拿到 None 造成偶发失败。
        let _ = child.wait();
        child
    }

    /** 启动一个会活到测试结束的子进程。 */
    fn running_child() -> Child {
        #[cfg(windows)]
        let child = Command::new("cmd")
            .args(["/C", "ping -n 30 127.0.0.1 > NUL"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn cmd");
        #[cfg(unix)]
        let child = Command::new("sh")
            .args(["-c", "sleep 30"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sh");
        child
    }

    /** 已退出的 sidecar 必须被发现，并且句柄被清空以防重复报告。 */
    #[test]
    fn detects_exited_sidecar_and_clears_the_handle() {
        let mut child = Some(exited_child());

        let first = take_exited_sidecar(&mut child);

        let description = first.expect("应检测到 sidecar 已退出");
        assert!(description.contains("已退出"), "{description}");
        assert!(description.contains('7'), "应带上退出码：{description}");
        assert!(child.is_none(), "退出后必须清空句柄");
        // 第二次不再报告，避免每次状态轮询都重复写一条断连日志。
        assert!(take_exited_sidecar(&mut child).is_none());
    }

    /** 仍在运行的 sidecar 不能被误判为退出。 */
    #[test]
    fn running_sidecar_is_not_reported_as_exited() {
        let mut child = Some(running_child());

        assert!(take_exited_sidecar(&mut child).is_none());
        assert!(child.is_some(), "运行中的句柄必须保留");

        if let Some(mut handle) = child.take() {
            let _ = handle.kill();
            let _ = handle.wait();
        }
    }

    /** 没有启动过 sidecar 时是空操作，不能凭空造出错误。 */
    #[test]
    fn missing_sidecar_is_a_no_op() {
        let mut child: Option<Child> = None;

        assert!(take_exited_sidecar(&mut child).is_none());
        assert!(child.is_none());
    }

    /** 探测到退出后，网关状态必须联动：running/connected 置假并记录原因。 */
    #[test]
    fn probe_marks_gateway_stopped_and_records_reason() {
        let mut child = Some(exited_child());
        let mut running = true;
        let mut connected = true;
        let mut last_error = None;

        let detected = GatewayLiveness {
            child: &mut child,
            running: &mut running,
            connected: &mut connected,
            last_error: &mut last_error,
        }
        .probe();

        assert!(detected.is_some());
        assert!(!running, "running 必须变成 false");
        assert!(!connected, "connected 必须变成 false");
        assert!(last_error.is_some(), "必须记录退出原因");
    }

    /** 进程仍活着时探测不得篡改任何状态字段。 */
    #[test]
    fn probe_leaves_live_gateway_untouched() {
        let mut child = Some(running_child());
        let mut running = true;
        let mut connected = true;
        let mut last_error: Option<String> = None;

        let detected = GatewayLiveness {
            child: &mut child,
            running: &mut running,
            connected: &mut connected,
            last_error: &mut last_error,
        }
        .probe();

        assert!(detected.is_none());
        assert!(running);
        assert!(connected);
        assert!(last_error.is_none());

        if let Some(mut handle) = child.take() {
            let _ = handle.kill();
            let _ = handle.wait();
        }
    }
}
