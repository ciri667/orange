use sha2::{Digest, Sha256};
use std::collections::HashMap;

/** OpenAI 兼容接口接受的函数名长度。超长会让整次请求 400。 */
const MAX_PUBLIC_NAME_CHARS: usize = 64;

/** 和宿主闭集撞名时不暴露。`mcp__` 前缀下几乎不会发生，仍按名字拒绝。 */
const HOST_TOOL_NAMES: [&str; 7] = ["search", "read", "list", "edit", "write", "run", "task"];

/** 一个已分配公开名的 MCP 工具。调用服务器时仍用 raw_name。 */
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedMcpTool {
    pub raw_name: String,
    pub public_name: String,
}

/** 把服务器名或工具名收成供应商接受的字符。 */
pub fn sanitize_name_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/** 未截断、未处理冲突时的公开名。 */
pub fn natural_public_name(server: &str, raw_tool: &str) -> String {
    format!(
        "mcp__{}__{}",
        sanitize_name_component(server),
        sanitize_name_component(raw_tool)
    )
}

/**
 * 为一台服务器的原始工具名分配公开名。
 *
 * 清洗后撞车，或自然名超过 64 字符时，追加稳定 hash。公开名不能反解析回原始名。
 */
pub fn assign_public_names(server: &str, raw_names: &[impl AsRef<str>]) -> Vec<NamedMcpTool> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let naturals: Vec<(String, String)> = raw_names
        .iter()
        .map(|raw| {
            let raw = raw.as_ref().to_owned();
            let natural = natural_public_name(server, &raw);
            *counts.entry(natural.clone()).or_insert(0) += 1;
            (raw, natural)
        })
        .collect();

    let mut assigned = Vec::with_capacity(naturals.len());
    let mut used = std::collections::HashSet::new();
    for (raw, natural) in naturals {
        let collided = counts.get(&natural).copied().unwrap_or(0) > 1;
        let base = if collided || natural.len() > MAX_PUBLIC_NAME_CHARS {
            clamp_with_hash(&natural, server, &raw)
        } else {
            natural
        };
        let mut public_name = base.clone();
        let mut extra = 2u32;
        while !used.insert(public_name.clone()) {
            let suffix = format!("_{extra}");
            extra += 1;
            let keep = MAX_PUBLIC_NAME_CHARS.saturating_sub(suffix.len());
            let prefix_len = base.len().min(keep);
            public_name = format!("{}{}", &base[..prefix_len], suffix);
        }
        assigned.push(NamedMcpTool {
            raw_name: raw,
            public_name,
        });
    }
    assigned
}

/** 公开名是否会盖住宿主工具。 */
pub fn collides_with_host_tool(public_name: &str) -> bool {
    HOST_TOOL_NAMES.contains(&public_name)
}

/** 过程区标题：`MCP · server · tool`。服务器名不含 `__`，所以从第一个分隔符切开。 */
pub fn trace_label(public_name: &str) -> Option<String> {
    let rest = public_name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some(format!("MCP · {server} · {tool}"))
}

fn clamp_with_hash(natural: &str, server: &str, raw: &str) -> String {
    let suffix = format!("_{}", hash8(server, raw));
    let keep = MAX_PUBLIC_NAME_CHARS.saturating_sub(suffix.len());
    let prefix_len = natural.len().min(keep);
    format!("{}{}", &natural[..prefix_len], suffix)
}

fn hash8(server: &str, raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(server.as_bytes());
    hasher.update([0]);
    hasher.update(raw.as_bytes());
    let digest = hasher.finalize();
    digest.iter().take(4).map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_non_ascii_identifiers() {
        assert_eq!(sanitize_name_component("get.item/v2"), "get_item_v2");
        assert_eq!(natural_public_name("my-server", "get.item"), "mcp__my_server__get_item");
    }

    #[test]
    fn long_names_are_clamped_to_64_with_stable_hash() {
        let raw = "x".repeat(80);
        let named = assign_public_names("server", &[raw.as_str()]);
        assert_eq!(named.len(), 1);
        assert!(named[0].public_name.len() <= 64);
        assert!(named[0].public_name.contains('_'));
        let again = assign_public_names("server", &[raw.as_str()]);
        assert_eq!(named[0].public_name, again[0].public_name);
        assert_eq!(named[0].raw_name, raw);
    }

    #[test]
    fn colliding_raw_names_do_not_share_a_public_name() {
        let named = assign_public_names("docs", &["get.item", "get_item"]);
        assert_ne!(named[0].public_name, named[1].public_name);
        assert_eq!(named[0].raw_name, "get.item");
        assert_eq!(named[1].raw_name, "get_item");
        assert!(named.iter().all(|tool| tool.public_name.len() <= 64));
    }

    #[test]
    fn trace_label_splits_server_and_tool() {
        assert_eq!(
            trace_label("mcp__docs__search"),
            Some("MCP · docs · search".to_owned())
        );
        assert_eq!(trace_label("search"), None);
    }
}
