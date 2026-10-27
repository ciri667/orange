use crate::domain::McpConfigIssue;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

/** 单次工具调用超时的默认秒数，和 pi / dsh 的 60 秒一致。 */
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 60;

/** 单次工具调用超时上限，避免一个挂起的服务器占住回合。 */
pub const MAX_TIMEOUT_SECONDS: u64 = 300;

/** 启动握手和 tools/list 的等待上限。 */
pub const STARTUP_TIMEOUT_SECONDS: u64 = 30;

/** 回合开始时最多再等首次连接这么久。 */
pub const TURN_WAIT_SECONDS: u64 = 3;

/** 断线重连次数。用尽后从模型工具列表移除，直到用户重试。 */
pub const MAX_RECONNECT_ATTEMPTS: u32 = 5;

/** 同时进行的首次连接数。 */
pub const CONNECT_CONCURRENCY: usize = 4;

const SECRET_MARKERS: [&str; 4] = ["KEY", "PASSWORD", "SECRET", "TOKEN"];

/** 通过校验、可以连接的一台服务器。环境变量已经插值，但不写日志。 */
#[derive(Clone, Debug)]
pub struct ResolvedMcpServer {
    pub name: String,
    pub enabled: bool,
    pub transport: McpTransport,
    pub timeout_seconds: u64,
    /** None 表示不设白名单。Some(空) 表示一个工具都不暴露。 */
    pub enabled_tools: Option<Vec<String>>,
    pub disabled_tools: Vec<String>,
    pub authorization_key_reference: Option<String>,
}

/** stdio 是本机命令，http 是 streamable HTTP。 */
#[derive(Clone, Debug)]
pub enum McpTransport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: Option<String>,
    },
    Http {
        url: String,
        headers: BTreeMap<String, String>,
    },
}

/** 读配置的结果。坏条目在 issues 里，不会挡住其他服务器。 */
#[derive(Clone, Debug)]
pub struct LoadedMcpConfig {
    pub servers: Vec<ResolvedMcpServer>,
    pub issues: Vec<McpConfigIssue>,
    /** 写回文件时保留的原始对象，避免保存一台服务器时丢掉其余条目。 */
    pub raw_servers: Map<String, Value>,
}

/** `~/.orange/mcp.json`。测试可以传入别的路径。 */
pub fn mcp_config_path(orange_home: &Path) -> PathBuf {
    orange_home.join("mcp.json")
}

/** 用户目录下的橘记配置根。 */
pub fn orange_home_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| "无法定位用户目录，MCP 配置没有落点。".to_owned())?;
    Ok(home.join(".orange"))
}

/** 读取并校验 mcp.json。文件不存在时当作没有服务器。 */
pub fn load_mcp_config(
    path: &Path,
    env: &HashMap<String, String>,
) -> Result<LoadedMcpConfig, String> {
    if !path.exists() {
        return Ok(LoadedMcpConfig {
            servers: Vec::new(),
            issues: Vec::new(),
            raw_servers: Map::new(),
        });
    }
    let text = fs::read_to_string(path).map_err(|error| format!("无法读取 MCP 配置：{error}"))?;
    parse_mcp_config(&text, env)
}

/** 解析 JSON 文本。整份文件不是对象时才返回错误。 */
pub fn parse_mcp_config(
    text: &str,
    env: &HashMap<String, String>,
) -> Result<LoadedMcpConfig, String> {
    let root: Value =
        serde_json::from_str(text).map_err(|error| format!("MCP 配置不是合法 JSON：{error}"))?;
    let root_object = root
        .as_object()
        .ok_or_else(|| "MCP 配置的顶层必须是对象。".to_owned())?;
    let Some(servers_value) = root_object.get("mcpServers") else {
        return Ok(LoadedMcpConfig {
            servers: Vec::new(),
            issues: Vec::new(),
            raw_servers: Map::new(),
        });
    };
    let servers_object = servers_value
        .as_object()
        .ok_or_else(|| "mcpServers 必须是对象。".to_owned())?;

    let mut servers = Vec::new();
    let mut issues = Vec::new();
    let mut namespaces = BTreeMap::<String, String>::new();
    for (name, value) in servers_object {
        match resolve_server(name, value, env, &mut namespaces) {
            Ok(server) => servers.push(server),
            Err(message) => issues.push(McpConfigIssue {
                server_name: name.clone(),
                message,
            }),
        }
    }
    Ok(LoadedMcpConfig {
        servers,
        issues,
        raw_servers: servers_object.clone(),
    })
}

/** 把内存中的服务器对象写回 mcp.json。先写临时文件再替换。 */
pub fn write_mcp_servers(path: &Path, servers: &Map<String, Value>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("无法创建 MCP 配置目录：{error}"))?;
    }
    let body = serde_json::to_vec_pretty(&serde_json::json!({ "mcpServers": servers }))
        .map_err(|error| format!("无法序列化 MCP 配置：{error}"))?;
    let temp_path = path.with_extension("json.tmp");
    fs::write(&temp_path, body).map_err(|error| format!("无法写入 MCP 配置：{error}"))?;
    fs::rename(&temp_path, path).map_err(|error| format!("无法替换 MCP 配置：{error}"))?;
    Ok(())
}

/** 从父进程环境去掉看起来像密钥的变量，再叠上配置里显式给出的 env。 */
pub fn scrub_parent_env(parent: &HashMap<String, String>) -> BTreeMap<String, String> {
    parent
        .iter()
        .filter(|(key, _)| !looks_like_secret(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/** `${NAME}` 整段或夹在文本中都可以。缺失变量是配置错误，不用空字符串顶上。 */
pub fn interpolate(value: &str, env: &HashMap<String, String>) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.starts_with('!') {
        return Err("不支持 !command 配置插值。请改用 ${环境变量}。".to_owned());
    }
    let mut output = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err("环境变量插值缺少 }。".to_owned());
        };
        let name = &after[..end];
        if name.is_empty()
            || !name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            return Err(format!("环境变量名无效：{name}"));
        }
        let resolved = env
            .get(name)
            .ok_or_else(|| format!("环境变量 {name} 未设置。"))?;
        output.push_str(resolved);
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

/** 按原始工具名过滤。写了 enabledTools 时只认白名单，否则用 disabledTools 排除。 */
pub fn tool_is_exposed(
    raw_name: &str,
    enabled_tools: Option<&[String]>,
    disabled_tools: &[String],
) -> bool {
    if let Some(enabled) = enabled_tools {
        return enabled.iter().any(|name| name == raw_name);
    }
    !disabled_tools.iter().any(|name| name == raw_name)
}

fn looks_like_secret(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    SECRET_MARKERS.iter().any(|marker| upper.contains(marker))
}

fn resolve_server(
    name: &str,
    value: &Value,
    env: &HashMap<String, String>,
    namespaces: &mut BTreeMap<String, String>,
) -> Result<ResolvedMcpServer, String> {
    validate_server_name(name)?;
    let namespace = name.replace('-', "_");
    if let Some(existing) = namespaces.get(&namespace) {
        return Err(format!("与服务器 {existing} 只相差 - 和 _，不能同时存在。"));
    }
    namespaces.insert(namespace, name.to_owned());

    let object = value
        .as_object()
        .ok_or_else(|| "服务器配置必须是对象。".to_owned())?;
    if object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.eq_ignore_ascii_case("sse"))
    {
        return Err("不支持旧的 SSE 传输。请改用 streamable HTTP 的 url。".to_owned());
    }

    let enabled = object
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let timeout_seconds = match object.get("timeout") {
        None => DEFAULT_TIMEOUT_SECONDS,
        Some(Value::Number(number)) => {
            let seconds = number
                .as_u64()
                .ok_or_else(|| "timeout 必须是正整数秒。".to_owned())?;
            if !(1..=MAX_TIMEOUT_SECONDS).contains(&seconds) {
                return Err(format!(
                    "timeout 必须在 1 到 {MAX_TIMEOUT_SECONDS} 秒之间。"
                ));
            }
            seconds
        }
        Some(_) => return Err("timeout 必须是数字。".to_owned()),
    };

    let command = optional_string(object, "command")?;
    let url = optional_string(object, "url")?;
    let transport = match (command, url) {
        (Some(command), None) => {
            if command.trim().is_empty() {
                return Err("command 不能为空。".to_owned());
            }
            if command.contains('\n') || command.contains('\r') {
                return Err("command 必须是一个可执行文件，不能是 shell 脚本。".to_owned());
            }
            McpTransport::Stdio {
                command: command.trim().to_owned(),
                args: string_list(object, "args")?,
                env: string_map(object, "env", env)?,
                cwd: optional_string(object, "cwd")?.filter(|cwd| !cwd.trim().is_empty()),
            }
        }
        (None, Some(url)) => {
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err("url 必须以 http:// 或 https:// 开头。".to_owned());
            }
            McpTransport::Http {
                url,
                headers: string_map(object, "headers", env)?,
            }
        }
        (Some(_), Some(_)) => return Err("command 和 url 只能填一个。".to_owned()),
        (None, None) => return Err("需要 command（stdio）或 url（streamable HTTP）。".to_owned()),
    };

    let disabled_tools = if object.contains_key("disabledTools") {
        string_list(object, "disabledTools")?
    } else {
        string_list(object, "disabled_tools")?
    };
    let enabled_tools = if object.contains_key("enabledTools") {
        Some(string_list(object, "enabledTools")?)
    } else if object.contains_key("enabled_tools") {
        Some(string_list(object, "enabled_tools")?)
    } else {
        None
    };
    let authorization_key_reference = optional_string(object, "authorizationKeyReference")?;

    Ok(ResolvedMcpServer {
        name: name.to_owned(),
        enabled,
        transport,
        timeout_seconds,
        enabled_tools,
        disabled_tools,
        authorization_key_reference,
    })
}

fn validate_server_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 {
        return Err("服务器名长度必须在 1 到 64 之间。".to_owned());
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err("服务器名只能包含字母、数字、_ 和 -。".to_owned());
    }
    Ok(())
}

fn optional_string(object: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("{key} 必须是字符串。")),
    }
}

fn string_list(object: &Map<String, Value>, key: &str) -> Result<Vec<String>, String> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("{key} 里的每一项都必须是字符串。"))
            })
            .collect(),
        Some(_) => Err(format!("{key} 必须是字符串数组。")),
    }
}

fn string_map(
    object: &Map<String, Value>,
    key: &str,
    env: &HashMap<String, String>,
) -> Result<BTreeMap<String, String>, String> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(BTreeMap::new()),
        Some(Value::Object(entries)) => {
            let mut resolved = BTreeMap::new();
            for (entry_key, entry_value) in entries {
                let raw = entry_value
                    .as_str()
                    .ok_or_else(|| format!("{key}.{entry_key} 必须是字符串。"))?;
                let value = interpolate(raw, env)?;
                resolved.insert(entry_key.clone(), value);
            }
            Ok(resolved)
        }
        Some(_) => Err(format!("{key} 必须是对象。")),
    }
}

/** 当前进程环境，供真正连接时插值。测试传入自己的表。 */
pub fn process_env() -> HashMap<String, String> {
    std::env::vars().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn skips_sse_and_keeps_the_other_server() {
        let text = r#"{
            "mcpServers": {
                "old": { "type": "sse", "url": "https://example.com/sse", "enabled": true },
                "docs": { "url": "https://example.com/mcp", "enabled": true }
            }
        }"#;
        let loaded = parse_mcp_config(text, &HashMap::new()).expect("parse");
        assert_eq!(loaded.servers.len(), 1);
        assert_eq!(loaded.servers[0].name, "docs");
        assert!(loaded.servers[0].enabled);
        assert_eq!(loaded.issues.len(), 1);
        assert_eq!(loaded.issues[0].server_name, "old");
    }

    #[test]
    fn missing_enabled_stays_off_and_env_is_required() {
        let text = r#"{
            "mcpServers": {
                "github": {
                    "command": "npx",
                    "args": ["-y", "server"],
                    "env": { "GITHUB_TOKEN": "${GITHUB_TOKEN}" }
                }
            }
        }"#;
        let missing = parse_mcp_config(text, &HashMap::new()).expect("parse");
        assert!(missing.servers.is_empty());
        assert!(missing.issues[0].message.contains("GITHUB_TOKEN"));

        let loaded = parse_mcp_config(text, &env(&[("GITHUB_TOKEN", "secret")])).expect("parse");
        assert_eq!(loaded.servers.len(), 1);
        assert!(!loaded.servers[0].enabled);
        match &loaded.servers[0].transport {
            McpTransport::Stdio {
                command, args, env, ..
            } => {
                assert_eq!(command, "npx");
                assert_eq!(args, &vec!["-y".to_owned(), "server".to_owned()]);
                assert_eq!(env.get("GITHUB_TOKEN").map(String::as_str), Some("secret"));
            }
            McpTransport::Http { .. } => panic!("stdio"),
        }
    }

    #[test]
    fn rejects_command_interpolation_and_hyphen_alias() {
        let text = r#"{
            "mcpServers": {
                "a-b": { "command": "tool", "enabled": true },
                "a_b": { "command": "tool", "enabled": true },
                "bang": { "command": "tool", "env": { "X": "!echo hi" }, "enabled": true }
            }
        }"#;
        let loaded = parse_mcp_config(text, &HashMap::new()).expect("parse");
        assert_eq!(loaded.servers.len(), 1);
        assert_eq!(loaded.servers[0].name, "a-b");
        assert_eq!(loaded.issues.len(), 2);
    }

    #[test]
    fn scrub_drops_secret_looking_parent_env() {
        let parent = env(&[
            ("PATH", "/usr/bin"),
            ("GITHUB_TOKEN", "nope"),
            ("API_KEY", "nope"),
        ]);
        let scrubbed = scrub_parent_env(&parent);
        assert_eq!(scrubbed.get("PATH").map(String::as_str), Some("/usr/bin"));
        assert!(!scrubbed.contains_key("GITHUB_TOKEN"));
        assert!(!scrubbed.contains_key("API_KEY"));
    }

    #[test]
    fn tool_filter_prefers_allow_list() {
        let enabled = vec!["read".to_owned()];
        let disabled = vec!["read".to_owned(), "write".to_owned()];
        assert!(!tool_is_exposed("write", Some(&enabled), &disabled));
        assert!(tool_is_exposed("read", Some(&enabled), &disabled));
        assert!(tool_is_exposed("read", None, &[]));
        assert!(!tool_is_exposed("write", None, &disabled));
        assert!(!tool_is_exposed("read", Some(&[]), &[]));
    }
}
