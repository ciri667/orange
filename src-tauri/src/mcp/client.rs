use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation, Tool,
};
use rmcp::service::{NotificationContext, RoleClient};
use rmcp::{ClientHandler, ServiceExt};
use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::ChildStderr;
use tokio::sync::Notify;

use super::config::{ResolvedMcpServer, McpTransport, STARTUP_TIMEOUT_SECONDS};
use super::policy::ContentPiece;

const STDERR_TAIL_BYTES: usize = 8 * 1024;

/** 已完成握手的一条连接。调用结束前必须 close，避免 stdio 子进程留下来。 */
pub struct LiveSession {
    service: rmcp::service::RunningService<RoleClient, OrangeHandler>,
    progress: Arc<Notify>,
    tools_changed: Arc<Notify>,
    closed: Arc<Notify>,
    pid: Option<u32>,
    stderr_tail: Arc<Mutex<String>>,
}

/** 从 tools/list 得到的、还没分配公开名的工具。 */
#[derive(Clone, Debug)]
pub struct RawTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub read_only: bool,
}

impl LiveSession {
    pub async fn list_tools(&self) -> Result<Vec<RawTool>, String> {
        let tools = tokio::time::timeout(
            Duration::from_secs(STARTUP_TIMEOUT_SECONDS),
            self.service.list_all_tools(),
        )
        .await
        .map_err(|_| "读取 MCP 工具列表超时。".to_owned())?
        .map_err(|error| format!("读取 MCP 工具列表失败：{error}"))?;
        Ok(tools.into_iter().map(raw_tool_from_model).collect())
    }

    /** 调用原始工具名。进度通知会把超时窗口重新拨回整段 timeout。 */
    pub async fn call_tool(
        &self,
        raw_name: &str,
        arguments: Option<Map<String, Value>>,
        timeout: Duration,
    ) -> Result<(bool, Vec<ContentPiece>), String> {
        let mut params = CallToolRequestParams::new(raw_name.to_owned());
        params.arguments = arguments;
        let call = self.service.call_tool(params);
        tokio::pin!(call);
        let mut deadline = tokio::time::Instant::now() + timeout;
        let result = loop {
            tokio::select! {
                result = &mut call => break result.map_err(|error| format!("MCP 工具调用失败：{error}"))?,
                _ = self.progress.notified() => {
                    deadline = tokio::time::Instant::now() + timeout;
                }
                _ = tokio::time::sleep_until(deadline) => {
                    return Err("MCP 工具调用超时。".to_owned());
                }
            }
        };
        Ok((result.is_error.unwrap_or(false), project_blocks(&result.content)))
    }

    pub fn stderr_tail(&self) -> String {
        self.stderr_tail
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn tools_changed(&self) -> &Notify {
        &self.tools_changed
    }

    pub fn closed(&self) -> &Notify {
        &self.closed
    }

    pub async fn close(mut self) {
        self.stop_child();
        let _ = tokio::time::timeout(Duration::from_secs(3), self.service.close()).await;
    }

    fn stop_child(&mut self) {
        if let Some(pid) = self.pid.take() {
            kill_process_tree(pid);
            forget_pid(pid);
        }
    }
}

impl Drop for LiveSession {
    fn drop(&mut self) {
        self.stop_child();
    }
}

/** 连接一台服务器并完成 tools/list。失败时不会留下子进程。 */
pub async fn connect(server: &ResolvedMcpServer, bearer_token: Option<String>) -> Result<LiveSession, String> {
    let progress = Arc::new(Notify::new());
    let tools_changed = Arc::new(Notify::new());
    let closed = Arc::new(Notify::new());
    let stderr_tail = Arc::new(Mutex::new(String::new()));
    let handler = OrangeHandler {
        info: client_config(),
        progress: progress.clone(),
        tools_changed: tools_changed.clone(),
    };

    let (pid, session) = match &server.transport {
        McpTransport::Stdio { command, args, env, cwd } => {
            let (transport, stderr, pid) = spawn_stdio(command, args, env, cwd.as_deref())?;
            if let Some(stderr) = stderr {
                let tail = stderr_tail.clone();
                let closed = closed.clone();
                tokio::spawn(drain_stderr(stderr, tail, closed));
            }
            if let Some(pid) = pid {
                remember_pid(pid);
            }
            let service = match handshake(handler, transport).await {
                Ok(service) => service,
                Err(error) => {
                    if let Some(pid) = pid {
                        kill_process_tree(pid);
                        forget_pid(pid);
                    }
                    return Err(with_stderr(&error, &stderr_tail));
                }
            };
            (pid, service)
        }
        McpTransport::Http { url, headers } => {
            let transport = http_transport(url, headers, bearer_token)?;
            let service = handshake(handler, transport).await?;
            (None, service)
        }
    };

    let session = LiveSession {
        service: session,
        progress,
        tools_changed,
        closed,
        pid,
        stderr_tail,
    };
    if let Err(error) = session.list_tools().await {
        let tail = session.stderr_tail();
        session.close().await;
        return Err(if tail.is_empty() { error } else { format!("{error}\n{tail}") });
    }
    Ok(session)
}

fn with_stderr(error: &str, tail: &Mutex<String>) -> String {
    let tail = tail.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    if tail.trim().is_empty() {
        error.to_owned()
    } else {
        format!("{error}\n{tail}")
    }
}

fn client_config() -> ClientConfig {
    ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::new("orange", env!("CARGO_PKG_VERSION")),
    )
}

async fn handshake<T, E, A>(
    handler: OrangeHandler,
    transport: T,
) -> Result<rmcp::service::RunningService<RoleClient, OrangeHandler>, String>
where
    T: rmcp::transport::IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    tokio::time::timeout(Duration::from_secs(STARTUP_TIMEOUT_SECONDS), handler.serve(transport))
        .await
        .map_err(|_| "MCP 服务器启动超时。".to_owned())?
        .map_err(|error| format!("MCP 初始化失败：{error}"))
}

fn spawn_stdio(
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    cwd: Option<&str>,
) -> Result<(rmcp::transport::TokioChildProcess, Option<ChildStderr>, Option<u32>), String> {
    let mut process = if command.contains('\\') || command.contains('/') {
        tokio::process::Command::new(command)
    } else {
        rmcp::transport::which_command(command).map_err(|error| format!("找不到命令 {command}：{error}"))?
    };
    process.args(args);
    if let Some(cwd) = cwd {
        process.current_dir(cwd);
    }
    process.env_clear();
    for (key, value) in super::config::scrub_parent_env(&super::config::process_env()) {
        process.env(key, value);
    }
    for (key, value) in env {
        process.env(key, value);
    }
    let (child, stderr) = rmcp::transport::TokioChildProcess::builder(process)
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("无法启动 MCP 进程：{error}"))?;
    let pid = child.id();
    Ok((child, stderr, pid))
}

fn http_transport(
    url: &str,
    headers: &BTreeMap<String, String>,
    bearer_token: Option<String>,
) -> Result<rmcp::transport::StreamableHttpClientTransport<reqwest::Client>, String> {
    let mut config = rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(url);
    if let Some(token) = bearer_token.filter(|token| !token.is_empty()) {
        if !headers.keys().any(|key| key.eq_ignore_ascii_case("authorization")) {
            config = config.auth_header(token);
        }
    }
    let mut custom = std::collections::HashMap::new();
    for (key, value) in headers {
        let name = http::HeaderName::try_from(key.as_str()).map_err(|_| format!("请求头名称无效：{key}"))?;
        let header_value = http::HeaderValue::try_from(value.as_str())
            .map_err(|_| format!("请求头 {key} 的值无效。"))?;
        custom.insert(name, header_value);
    }
    if !custom.is_empty() {
        config = config.custom_headers(custom);
    }
    Ok(rmcp::transport::StreamableHttpClientTransport::from_config(config))
}

fn raw_tool_from_model(tool: Tool) -> RawTool {
    let schema = serde_json::to_value(tool.input_schema.as_ref()).unwrap_or_else(|_| serde_json::json!({}));
    RawTool {
        name: tool.name.to_string(),
        description: tool
            .description
            .map(|text| text.to_string())
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| format!("MCP tool {}", tool.name)),
        input_schema: super::policy::normalize_input_schema(schema),
        read_only: tool
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.read_only_hint)
            .unwrap_or(false),
    }
}

fn project_blocks(blocks: &[rmcp::model::ContentBlock]) -> Vec<ContentPiece> {
    blocks
        .iter()
        .map(|block| match block {
            rmcp::model::ContentBlock::Text(text) => ContentPiece::Text(text.text.clone()),
            rmcp::model::ContentBlock::Image(image) => ContentPiece::Image {
                mime: image.mime_type.clone(),
            },
            rmcp::model::ContentBlock::Audio(audio) => ContentPiece::Audio {
                mime: audio.mime_type.clone(),
            },
            rmcp::model::ContentBlock::Resource(_) | rmcp::model::ContentBlock::ResourceLink(_) => {
                ContentPiece::Resource
            }
            _ => ContentPiece::Other,
        })
        .collect()
}

struct OrangeHandler {
    info: ClientConfig,
    progress: Arc<Notify>,
    tools_changed: Arc<Notify>,
}

impl ClientHandler for OrangeHandler {
    fn get_info(&self) -> ClientConfig {
        self.info.clone()
    }

    async fn on_progress(
        &self,
        _params: rmcp::model::ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        self.progress.notify_waiters();
    }

    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        self.tools_changed.notify_waiters();
    }
}

async fn drain_stderr(stderr: ChildStderr, tail: Arc<Mutex<String>>, closed: Arc<Notify>) {
    let mut reader = BufReader::new(stderr);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => {
                closed.notify_waiters();
                break;
            }
            Ok(_) => append_tail(&tail, &line),
        }
    }
}

fn append_tail(tail: &Mutex<String>, line: &str) {
    let mut guard = tail.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.push_str(line);
    if guard.len() > STDERR_TAIL_BYTES {
        let start = guard.len() - STDERR_TAIL_BYTES;
        *guard = guard[start..].to_owned();
    }
}

fn remember_pid(pid: u32) {
    pids().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(pid);
}

fn forget_pid(pid: u32) {
    pids().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&pid);
}

fn pids() -> &'static Mutex<std::collections::HashSet<u32>> {
    use std::sync::OnceLock;
    static PIDS: OnceLock<Mutex<std::collections::HashSet<u32>>> = OnceLock::new();
    PIDS.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

/** 进程退出时同步拆掉还活着的 stdio 子进程树。 */
pub fn kill_tracked_processes() {
    let tracked: Vec<u32> = pids()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .drain()
        .collect();
    for pid in tracked {
        kill_process_tree(pid);
    }
}

fn kill_process_tree(pid: u32) {
    if pid == 0 || pid == std::process::id() {
        return;
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::config::parse_mcp_config;
    use std::collections::HashMap;

    /** CARGO_BIN_EXE_* 只在集成测试里注入；lib 单测从自身可执行文件位置定位同目录层的 fixture。 */
    fn fixture_exe_path() -> String {
        // 单测可执行文件位于 target/<profile>/deps/，bin 产物在上一级 profile 目录。
        let mut path = std::env::current_exe().expect("读取测试可执行文件路径失败");
        path.pop();
        path.pop();
        path.push("orange-mcp-fixture");
        if cfg!(windows) {
            path.set_extension("exe");
        }
        path.to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn stdio_fixture_lists_and_calls_then_exits() {
        let exe = fixture_exe_path();
        let text = format!(
            r#"{{"mcpServers":{{"fixture":{{"command":{command},"enabled":true,"timeout":10}}}}}}"#,
            command = serde_json::to_string(&exe).expect("command")
        );
        let loaded = parse_mcp_config(&text, &HashMap::new()).expect("config");
        let server = loaded.servers.into_iter().next().expect("server");
        let session = connect(&server, None).await.expect("connect");
        let pid = session.pid.expect("pid");
        let tools = session.list_tools().await.expect("tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
        assert!(tools[0].read_only);
        let mut arguments = Map::new();
        arguments.insert("message".to_owned(), Value::String("橘".to_owned()));
        let (is_error, pieces) = session
            .call_tool("echo", Some(arguments), Duration::from_secs(10))
            .await
            .expect("call");
        assert!(!is_error);
        match &pieces[0] {
            ContentPiece::Text(text) => assert_eq!(text, "橘"),
            other => panic!("unexpected {other:?}"),
        }
        session.close().await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!process_alive(pid), "fixture process {pid} still alive");
    }

    fn process_alive(pid: u32) -> bool {
        #[cfg(windows)]
        {
            let output = std::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
                .output();
            let Ok(output) = output else {
                return false;
            };
            String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
        }
        #[cfg(not(windows))]
        {
            std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|status| status.success())
                .unwrap_or(false)
        }
    }
}
