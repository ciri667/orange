//! 测试用 MCP stdio 服务器。只回答 initialize、tools/list 和 tools/call。

use serde_json::{json, Value};
use std::io::{BufRead, Write};

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let id = message.get("id").cloned();
        if id.is_none() || method.starts_with("notifications/") {
            continue;
        }
        let result = match method {
            "initialize" => {
                let protocol = message
                    .pointer("/params/protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("2025-11-25");
                json!({
                    "protocolVersion": protocol,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "orange-mcp-fixture", "version": "0.0.1" }
                })
            }
            "tools/list" => json!({
                "tools": [{
                    "name": "echo",
                    "description": "把 message 原样返回",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "message": { "type": "string" } }
                    },
                    "annotations": { "readOnlyHint": true }
                }]
            }),
            "tools/call" => {
                let name = message
                    .pointer("/params/name")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if name != "echo" {
                    json!({
                        "content": [{ "type": "text", "text": "未知工具" }],
                        "isError": true
                    })
                } else {
                    let text = message
                        .pointer("/params/arguments/message")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    json!({
                        "content": [{ "type": "text", "text": text }],
                        "isError": false
                    })
                }
            }
            "ping" => json!({}),
            _ => {
                let error = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": format!("fixture 不支持 {method}") }
                });
                let _ = writeln!(stdout, "{error}");
                let _ = stdout.flush();
                continue;
            }
        };
        let response = json!({ "jsonrpc": "2.0", "id": id, "result": result });
        if writeln!(stdout, "{response}").is_err() || stdout.flush().is_err() {
            break;
        }
    }
}
