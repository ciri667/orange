use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Map, Value};
use tauri::{AppHandle, Manager};
use tokio::sync::{mpsc, oneshot, watch, Semaphore};

use crate::agent_tools::ToolOutcome;
use crate::domain::{
    AgentSession, AgentToolCall, Citation, McpConfigIssue, McpOverview, McpProbeResult,
    McpServerDraft, McpServerView, McpToolView,
};
use crate::runtime::AgentCancel;
use crate::storage::create_id;

use super::client::{self, LiveSession};
use super::config::{
    self, load_mcp_config, tool_is_exposed, write_mcp_servers, LoadedMcpConfig, McpTransport,
    ResolvedMcpServer, CONNECT_CONCURRENCY, MAX_RECONNECT_ATTEMPTS, TURN_WAIT_SECONDS,
};
use super::naming::{assign_public_names, collides_with_host_tool};
use super::policy::{
    self, model_tool_schema, project_call, session_exposes_mcp, CatalogTool, ContentPiece,
};
use super::secrets;

const CALL_CHANNEL: usize = 8;

struct Slot {
    server: ResolvedMcpServer,
    state: &'static str,
    error: Option<String>,
    stderr_tail: String,
    tools: Vec<CatalogTool>,
    command: String,
    args: Vec<String>,
    url: String,
    cwd: String,
}

struct CallJob {
    raw_name: String,
    arguments: Map<String, Value>,
    timeout: Duration,
    cancel: AgentCancel,
    reply: oneshot::Sender<Result<(bool, Vec<ContentPiece>), String>>,
}

struct Shared {
    slots: Mutex<BTreeMap<String, Slot>>,
    issues: Mutex<Vec<McpConfigIssue>>,
    config_path: std::path::PathBuf,
    calls: Mutex<BTreeMap<String, mpsc::Sender<CallJob>>>,
    shutdown: Mutex<watch::Sender<bool>>,
    connect_limit: Arc<Semaphore>,
}

/** 进程内的 MCP 连接管理。关窗口不会停掉它，真正退出时要 shutdown。 */
#[derive(Clone)]
pub struct McpManager {
    shared: Arc<Shared>,
}

impl McpManager {
    fn from_loaded(config_path: std::path::PathBuf, loaded: LoadedMcpConfig) -> Self {
        let (shutdown, _) = watch::channel(false);
        let mut slots = BTreeMap::new();
        for server in loaded.servers {
            slots.insert(server.name.clone(), slot_from_server(server));
        }
        Self {
            shared: Arc::new(Shared {
                slots: Mutex::new(slots),
                issues: Mutex::new(loaded.issues),
                config_path,
                calls: Mutex::new(BTreeMap::new()),
                shutdown: Mutex::new(shutdown),
                connect_limit: Arc::new(Semaphore::new(CONNECT_CONCURRENCY)),
            }),
        }
    }

    pub async fn wait_initial(&self) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(TURN_WAIT_SECONDS);
        while tokio::time::Instant::now() < deadline {
            let connecting = self
                .shared
                .slots
                .lock()
                .expect("slots")
                .values()
                .any(|slot| slot.server.enabled && slot.state == "connecting");
            if !connecting {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub fn model_schemas(&self) -> Vec<Value> {
        self.shared
            .slots
            .lock()
            .expect("slots")
            .values()
            .filter(|slot| slot.state == "ready")
            .flat_map(|slot| slot.tools.iter().filter_map(model_tool_schema))
            .collect()
    }

    pub fn overview(&self) -> McpOverview {
        let slots = self.shared.slots.lock().expect("slots");
        let issues = self.shared.issues.lock().expect("issues");
        McpOverview {
            config_path: self.shared.config_path.display().to_string(),
            servers: slots.values().map(slot_view).collect(),
            issues: issues.clone(),
        }
    }

    pub async fn reload(&self) -> Result<McpOverview, String> {
        self.stop_tasks().await;
        let loaded = load_mcp_config(&self.shared.config_path, &config::process_env())?;
        {
            let mut slots = self.shared.slots.lock().expect("slots");
            slots.clear();
            for server in loaded.servers {
                slots.insert(server.name.clone(), slot_from_server(server));
            }
            *self.shared.issues.lock().expect("issues") = loaded.issues;
        }
        self.spawn_enabled();
        self.wait_initial().await;
        Ok(self.overview())
    }

    pub async fn probe(&self, name: &str) -> Result<McpProbeResult, String> {
        let server = self
            .shared
            .slots
            .lock()
            .expect("slots")
            .get(name)
            .map(|slot| slot.server.clone())
            .ok_or_else(|| format!("没有名为 {name} 的 MCP 服务器。"))?;
        let bearer = server
            .authorization_key_reference
            .as_deref()
            .and_then(secrets::load_secret);
        match client::connect(&server, bearer).await {
            Ok(session) => {
                // tool.name 是 Cow<str>，需显式收集为 Vec<String> 匹配 McpProbeResult.tool_names。
                let names: Vec<String> = session
                    .list_tools()
                    .await
                    .map(|tools| {
                        tools
                            .into_iter()
                            .map(|tool| tool.name.to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                session.close().await;
                Ok(McpProbeResult {
                    server_name: name.to_owned(),
                    ok: true,
                    message: format!("已连接，发现 {} 个工具。", names.len()),
                    tool_names: names,
                })
            }
            Err(error) => Ok(McpProbeResult {
                server_name: name.to_owned(),
                ok: false,
                message: error,
                tool_names: Vec::new(),
            }),
        }
    }

    pub async fn execute(
        &self,
        app: &AppHandle,
        session: &AgentSession,
        permit: &crate::operation_approval::Permit,
        public_name: &str,
        arguments: Value,
        cancel: &AgentCancel,
    ) -> ToolOutcome {
        if let Err(e)=permit.validate(app,session,public_name,&arguments,cancel) { return failed(public_name,&arguments,&e); }
        if !session_exposes_mcp(session.im_identity.is_none() && session.schedule_identity.is_none(), &session.security_level, session.plan_mode) {
            return failed(public_name, &arguments, "当前会话不能使用 MCP。");
        }
        let Some(tool) = self.find_tool(public_name) else {
            return failed(public_name, &arguments, "未知的 MCP 工具，已拒绝执行。");
        };
        if !tool.exposed {
            return failed(public_name, &arguments, "这个 MCP 工具已在设置里关闭。");
        }
        let call_result = self.dispatch_call(&tool, arguments.clone(), cancel.clone()).await;
        match call_result {
            Ok(projected) => outcome(public_name, arguments, projected.success, projected.text),
            Err(error) => failed(public_name, &arguments, &error),
        }
    }

    fn find_tool(&self, public_name: &str) -> Option<CatalogTool> {
        self.shared
            .slots
            .lock()
            .expect("slots")
            .values()
            .find_map(|slot| {
                slot.tools
                    .iter()
                    .find(|tool| tool.public_name == public_name)
                    .cloned()
            })
    }

    async fn dispatch_call(
        &self,
        tool: &CatalogTool,
        arguments: Value,
        cancel: AgentCancel,
    ) -> Result<policy::ProjectedCall, String> {
        let arguments = match arguments {
            Value::Object(map) => map,
            Value::Null => Map::new(),
            _ => return Err("MCP 工具参数必须是 JSON 对象。".to_owned()),
        };
        let timeout = self
            .shared
            .slots
            .lock()
            .expect("slots")
            .get(&tool.server_name)
            .map(|slot| Duration::from_secs(slot.server.timeout_seconds))
            .unwrap_or(Duration::from_secs(config::DEFAULT_TIMEOUT_SECONDS));
        let sender = self
            .shared
            .calls
            .lock()
            .expect("calls")
            .get(&tool.server_name)
            .cloned()
            .ok_or_else(|| format!("MCP 服务器 {} 还没有连上。", tool.server_name))?;
        let (reply_tx, reply_rx) = oneshot::channel();
        sender
            .send(CallJob {
                raw_name: tool.raw_name.clone(),
                arguments,
                timeout,
                cancel,
                reply: reply_tx,
            })
            .await
            .map_err(|_| format!("MCP 服务器 {} 已断开。", tool.server_name))?;
        let (is_error, pieces) = reply_rx
            .await
            .map_err(|_| format!("MCP 服务器 {} 没有返回结果。", tool.server_name))??;
        Ok(project_call(is_error, &pieces))
    }

    fn spawn_enabled(&self) {
        let shutdown = self.shared.shutdown.lock().expect("shutdown").subscribe();
        let names: Vec<String> = self
            .shared
            .slots
            .lock()
            .expect("slots")
            .values()
            .filter(|slot| slot.server.enabled)
            .map(|slot| slot.server.name.clone())
            .collect();
        for name in names {
            self.spawn_one(name, shutdown.clone());
        }
    }

    fn spawn_one(&self, name: String, mut shutdown: watch::Receiver<bool>) {
        let (tx, mut rx) = mpsc::channel(CALL_CHANNEL);
        self.shared
            .calls
            .lock()
            .expect("calls")
            .insert(name.clone(), tx);
        let shared = self.shared.clone();
        tokio::spawn(async move {
            let mut attempt = 0u32;
            loop {
                if *shutdown.borrow() {
                    break;
                }
                let Some(server) = shared
                    .slots
                    .lock()
                    .expect("slots")
                    .get(&name)
                    .filter(|slot| slot.server.enabled)
                    .map(|slot| slot.server.clone())
                else {
                    break;
                };
                set_state(&shared, &name, "connecting", None);
                let bearer = server
                    .authorization_key_reference
                    .as_deref()
                    .and_then(secrets::load_secret);
                let permit = shared
                    .connect_limit
                    .clone()
                    .acquire_owned()
                    .await
                    .unwrap_or_else(|_| panic!("MCP 连接信号量已关闭"));
                let connected = client::connect(&server, bearer).await;
                drop(permit);
                let mut session = match connected {
                    Ok(session) => session,
                    Err(error) => {
                        attempt += 1;
                        set_state(&shared, &name, "failed", Some(error));
                        if attempt >= MAX_RECONNECT_ATTEMPTS || *shutdown.borrow() {
                            break;
                        }
                        if wait_backoff(&mut shutdown, attempt).await {
                            break;
                        }
                        continue;
                    }
                };
                attempt = 0;
                if let Ok(raw) = session.list_tools().await {
                    publish_tools(&shared, &server, &raw, &session.stderr_tail());
                }
                let drive =
                    drive_session(&shared, &name, &mut session, &mut rx, &mut shutdown).await;
                let tail = session.stderr_tail();
                session.close().await;
                if drive == Drive::Stop || *shutdown.borrow() {
                    break;
                }
                attempt += 1;
                set_state(
                    &shared,
                    &name,
                    "failed",
                    Some(if tail.is_empty() {
                        "MCP 连接已断开。".to_owned()
                    } else {
                        format!("MCP 连接已断开。\n{tail}")
                    }),
                );
                if attempt >= MAX_RECONNECT_ATTEMPTS {
                    break;
                }
                if wait_backoff(&mut shutdown, attempt).await {
                    break;
                }
            }
            shared.calls.lock().expect("calls").remove(&name);
        });
    }

    pub fn save_server(&self, draft: &McpServerDraft) -> Result<(), String> {
        save_draft(&self.shared.config_path, draft)
    }

    pub fn remove_named(&self, name: &str) -> Result<(), String> {
        let loaded = load_mcp_config(&self.shared.config_path, &HashMap::new())?;
        let mut raw = loaded.raw_servers;
        if raw.remove(name).is_none() {
            return Err(format!("没有名为 {name} 的 MCP 服务器。"));
        }
        secrets::delete_secret(&secrets::authorization_reference(name));
        write_mcp_servers(&self.shared.config_path, &raw)
    }

    pub fn set_enabled_flag(&self, name: &str, enabled: bool) -> Result<(), String> {
        let loaded = load_mcp_config(&self.shared.config_path, &HashMap::new())?;
        let mut raw = loaded.raw_servers;
        let entry = raw
            .get_mut(name)
            .ok_or_else(|| format!("没有名为 {name} 的 MCP 服务器。"))?;
        let object = entry
            .as_object_mut()
            .ok_or_else(|| "服务器配置必须是对象。".to_owned())?;
        object.insert("enabled".to_owned(), Value::Bool(enabled));
        write_mcp_servers(&self.shared.config_path, &raw)
    }

    async fn stop_tasks(&self) {
        let sender = self.shared.shutdown.lock().expect("shutdown").clone();
        let _ = sender.send(true);
        let (next, _) = watch::channel(false);
        *self.shared.shutdown.lock().expect("shutdown") = next;
        self.shared.calls.lock().expect("calls").clear();
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[derive(PartialEq, Eq)]
enum Drive {
    Stop,
    Reconnect,
}

async fn drive_session(
    shared: &Shared,
    server_name: &str,
    session: &mut LiveSession,
    jobs: &mut mpsc::Receiver<CallJob>,
    shutdown: &mut watch::Receiver<bool>,
) -> Drive {
    loop {
        tokio::select! {
            _ = shutdown.changed() => return Drive::Stop,
            _ = session.closed().notified() => return Drive::Reconnect,
            _ = session.tools_changed().notified() => {
                if let Ok(raw) = session.list_tools().await {
                    let server = shared
                        .slots
                        .lock()
                        .expect("slots")
                        .get(server_name)
                        .map(|slot| slot.server.clone());
                    if let Some(server) = server {
                        publish_tools(shared, &server, &raw, &session.stderr_tail());
                    }
                }
            }
            job = jobs.recv() => {
                let Some(job) = job else { return Drive::Stop };
                let cancel = job.cancel.clone();
                let call = session.call_tool(&job.raw_name, Some(job.arguments), job.timeout);
                tokio::pin!(call);
                let result = tokio::select! {
                    result = &mut call => result,
                    _ = cancel.cancelled() => Err("用户已停止这个回合。".to_owned()),
                    _ = shutdown.changed() => Err("MCP 服务器正在关闭。".to_owned()),
                };
                let _ = job.reply.send(result);
                if *shutdown.borrow() {
                    return Drive::Stop;
                }
            }
        }
    }
}

async fn wait_backoff(shutdown: &mut watch::Receiver<bool>, attempt: u32) -> bool {
    let shift = attempt.min(6);
    let millis = 500u64.saturating_mul(1u64 << shift).min(30_000);
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_millis(millis)) => false,
        _ = shutdown.changed() => true,
    }
}

fn set_state(shared: &Shared, name: &str, state: &'static str, error: Option<String>) {
    if let Some(slot) = shared.slots.lock().expect("slots").get_mut(name) {
        slot.state = state;
        slot.error = error;
        if state != "ready" {
            slot.tools.clear();
        }
    }
}

fn publish_tools(
    shared: &Shared,
    server: &ResolvedMcpServer,
    raw: &[client::RawTool],
    stderr_tail: &str,
) {
    let names: Vec<&str> = raw.iter().map(|tool| tool.name.as_str()).collect();
    let named = assign_public_names(&server.name, &names);
    let tools = raw
        .iter()
        .zip(named)
        .map(|(tool, named)| {
            let collided = collides_with_host_tool(&named.public_name);
            let exposed = !collided
                && tool_is_exposed(
                    &tool.name,
                    server.enabled_tools.as_deref(),
                    &server.disabled_tools,
                );
            CatalogTool {
                server_name: server.name.clone(),
                raw_name: tool.name.clone(),
                public_name: named.public_name,
                description: tool.description.clone(),
                parameters: tool.input_schema.clone(),
                read_only: tool.read_only,
                exposed,
            }
        })
        .collect();
    if let Some(slot) = shared.slots.lock().expect("slots").get_mut(&server.name) {
        slot.tools = tools;
        slot.state = "ready";
        slot.error = None;
        slot.stderr_tail = stderr_tail.to_owned();
    }
}

fn slot_from_server(server: ResolvedMcpServer) -> Slot {
    let (command, args, url, cwd) = match &server.transport {
        McpTransport::Stdio {
            command, args, cwd, ..
        } => (
            command.clone(),
            args.clone(),
            String::new(),
            cwd.clone().unwrap_or_default(),
        ),
        McpTransport::Http { url, .. } => (String::new(), Vec::new(), url.clone(), String::new()),
    };
    let state = if server.enabled {
        "connecting"
    } else {
        "disabled"
    };
    Slot {
        server,
        state,
        error: None,
        stderr_tail: String::new(),
        tools: Vec::new(),
        command,
        args,
        url,
        cwd,
    }
}

fn slot_view(slot: &Slot) -> McpServerView {
    let exposed_tool_count = slot.tools.iter().filter(|tool| tool.exposed).count();
    McpServerView {
        name: slot.server.name.clone(),
        enabled: slot.server.enabled,
        transport: match slot.server.transport {
            McpTransport::Stdio { .. } => "stdio".to_owned(),
            McpTransport::Http { .. } => "http".to_owned(),
        },
        state: slot.state.to_owned(),
        command: slot.command.clone(),
        args: slot.args.clone(),
        url: slot.url.clone(),
        cwd: slot.cwd.clone(),
        timeout_seconds: slot.server.timeout_seconds,
        has_authorization: slot.server.authorization_key_reference.is_some(),
        error: slot.error.clone(),
        stderr_tail: slot.stderr_tail.clone(),
        exposed_tool_count,
        tools: slot
            .tools
            .iter()
            .map(|tool| McpToolView {
                raw_name: tool.raw_name.clone(),
                public_name: tool.public_name.clone(),
                description: tool.description.clone(),
                read_only: tool.read_only,
                exposed: tool.exposed,
            })
            .collect(),
    }
}

fn failed(name: &str, arguments: &Value, message: &str) -> ToolOutcome {
    outcome(name, arguments.clone(), false, message.to_owned())
}

fn outcome(name: &str, arguments: Value, success: bool, text: String) -> ToolOutcome {
    let summary = if text.chars().count() > 160 {
        let clipped: String = text.chars().take(160).collect();
        format!("{clipped}…")
    } else {
        text.clone()
    };
    ToolOutcome {
        call: AgentToolCall {
            id: create_id("tool"),
            name: name.to_owned(),
            status: if success { "completed" } else { "failed" }.to_owned(),
            summary,
            args: arguments,
        },
        payload: json!({ "content": text }),
        citations: Vec::<Citation>::new(),
        audit_fragment: Some(format!(
            "MCP {}：{}",
            name,
            if success { "完成" } else { "失败" }
        )),
    }
}

/** 应用启动后调用。重复调用直接返回已有管理器。 */
pub async fn install(app: &AppHandle) -> Result<McpManager, String> {
    if let Some(manager) = app.try_state::<McpManager>() {
        return Ok(manager.inner().clone());
    }
    let home = config::orange_home_dir()?;
    let path = config::mcp_config_path(&home);
    let loaded = load_mcp_config(&path, &config::process_env())?;
    let manager = McpManager::from_loaded(path, loaded);
    app.manage(manager.clone());
    manager.spawn_enabled();
    Ok(manager)
}

pub fn manager(app: &AppHandle) -> Option<McpManager> {
    app.try_state::<McpManager>()
        .map(|state| state.inner().clone())
}

pub async fn prepare_turn_schemas(app: &AppHandle, session: &AgentSession) -> Vec<Value> {
    let local_interactive = session.im_identity.is_none() && session.schedule_identity.is_none();
    // 规划模式只读：MCP 工具连 schema 都不注入，模型无从调用。
    if !session_exposes_mcp(
        local_interactive,
        &session.security_level,
        session.plan_mode,
    ) {
        return Vec::new();
    }
    let Some(manager) = manager(app) else {
        return Vec::new();
    };
    manager.wait_initial().await;
    manager.model_schemas()
}

pub async fn execute_model_tool(
    app: &AppHandle,
    session: &AgentSession,
    public_name: &str,
    arguments: Value,
    cancel: &AgentCancel,
    permit: &crate::operation_approval::Permit,
) -> ToolOutcome {
    // 执行侧兜底：即使历史 transcript 回放旧 MCP tool_call，规划模式也一律拒绝。
    if session.plan_mode {
        return failed(public_name, &arguments, "规划模式下不能使用 MCP 工具。");
    }
    let Some(manager) = manager(app) else {
        return failed(public_name, &arguments, "MCP 还没有启动。");
    };
    manager
        .execute(
            app,
            session,
            permit,
            public_name,
            arguments,
            cancel,
        )
        .await
}

/** 把设置页草稿写成 mcp.json 里的一条。令牌进 keyring。 */
pub fn upsert_draft(
    loaded_raw: &mut Map<String, Value>,
    draft: &McpServerDraft,
) -> Result<(), String> {
    validate_draft(draft)?;
    let mut object = Map::new();
    object.insert("enabled".to_owned(), Value::Bool(draft.enabled));
    object.insert(
        "timeout".to_owned(),
        json!(draft.timeout_seconds.clamp(1, config::MAX_TIMEOUT_SECONDS)),
    );
    if draft.transport == "http" {
        object.insert("url".to_owned(), Value::String(draft.url.trim().to_owned()));
        let headers = parse_pairs(&draft.headers_text)?;
        if !headers.is_empty() {
            object.insert("headers".to_owned(), map_value(headers));
        }
    } else {
        object.insert(
            "command".to_owned(),
            Value::String(draft.command.trim().to_owned()),
        );
        let args = split_lines(&draft.args_text);
        if !args.is_empty() {
            object.insert("args".to_owned(), json!(args));
        }
        let env = parse_pairs(&draft.env_text)?;
        if !env.is_empty() {
            object.insert("env".to_owned(), map_value(env));
        }
        if !draft.cwd.trim().is_empty() {
            object.insert("cwd".to_owned(), Value::String(draft.cwd.trim().to_owned()));
        }
    }
    if !draft.disabled_tools.is_empty() {
        object.insert("disabledTools".to_owned(), json!(draft.disabled_tools));
    }
    let reference = secrets::authorization_reference(&draft.name);
    match draft.authorization_token.as_deref() {
        Some(token) if token.trim().is_empty() => secrets::delete_secret(&reference),
        Some(token) => {
            secrets::save_secret(&reference, token.trim())?;
            object.insert(
                "authorizationKeyReference".to_owned(),
                Value::String(reference),
            );
        }
        None => {
            if loaded_raw
                .get(&draft.name)
                .and_then(|value| value.get("authorizationKeyReference"))
                .is_some()
            {
                object.insert(
                    "authorizationKeyReference".to_owned(),
                    Value::String(reference),
                );
            }
        }
    }
    loaded_raw.insert(draft.name.clone(), Value::Object(object));
    Ok(())
}

pub fn save_draft(path: &std::path::Path, draft: &McpServerDraft) -> Result<(), String> {
    let mut loaded = load_mcp_config(path, &HashMap::new()).unwrap_or(LoadedMcpConfig {
        servers: Vec::new(),
        issues: Vec::new(),
        raw_servers: Map::new(),
    });
    // 保存时不因别的坏条目失败；只校验这一条。插值放到重载时再做。
    let mut raw = loaded.raw_servers;
    // 草稿里的 ${VAR} 还没解析，不能走 resolve。直接写原始 JSON。
    let _ = &mut loaded.servers;
    upsert_draft(&mut raw, draft)?;
    write_mcp_servers(path, &raw)
}

fn validate_draft(draft: &McpServerDraft) -> Result<(), String> {
    if draft.name.trim().is_empty() {
        return Err("请填写服务器名。".to_owned());
    }
    if !(1..=64).contains(&draft.name.len())
        || !draft
            .name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err("服务器名只能包含字母、数字、_ 和 -。".to_owned());
    }
    if draft.transport == "http" {
        let url = draft.url.trim();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err("url 必须以 http:// 或 https:// 开头。".to_owned());
        }
    } else if draft.command.trim().is_empty() {
        return Err("请填写要启动的命令。".to_owned());
    }
    Ok(())
}

fn split_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

fn parse_pairs(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut pairs = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("这一行需要 KEY=VALUE：{line}"));
        };
        if key.trim().is_empty() {
            return Err("环境变量或请求头的名称不能为空。".to_owned());
        }
        pairs.insert(key.trim().to_owned(), value.to_owned());
    }
    Ok(pairs)
}

fn map_value(pairs: BTreeMap<String, String>) -> Value {
    Value::Object(
        pairs
            .into_iter()
            .map(|(key, value)| (key, Value::String(value)))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_rejects_a_shell_line_and_writes_stdio_entry() {
        let mut raw = Map::new();
        let error = upsert_draft(
            &mut raw,
            &McpServerDraft {
                name: "bad name".to_owned(),
                enabled: false,
                transport: "stdio".to_owned(),
                command: "npx".to_owned(),
                args_text: "-y\nserver".to_owned(),
                url: String::new(),
                cwd: String::new(),
                timeout_seconds: 60,
                env_text: String::new(),
                headers_text: String::new(),
                disabled_tools: Vec::new(),
                authorization_token: None,
            },
        );
        assert!(error.is_err());

        upsert_draft(
            &mut raw,
            &McpServerDraft {
                name: "docs".to_owned(),
                enabled: false,
                transport: "stdio".to_owned(),
                command: "npx".to_owned(),
                args_text: "-y\nserver".to_owned(),
                url: String::new(),
                cwd: String::new(),
                timeout_seconds: 60,
                env_text: "TOKEN=${TOKEN}".to_owned(),
                headers_text: String::new(),
                disabled_tools: vec!["delete".to_owned()],
                authorization_token: None,
            },
        )
        .expect("save");
        assert_eq!(raw["docs"]["command"], "npx");
        assert_eq!(raw["docs"]["enabled"], false);
        assert_eq!(raw["docs"]["env"]["TOKEN"], "${TOKEN}");
        assert_eq!(raw["docs"]["args"][1], "server");
    }
}
