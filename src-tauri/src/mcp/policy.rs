use serde_json::{json, Map, Value};

use super::naming::collides_with_host_tool;

/** 模型可见的一个 MCP 工具。raw_name 只用于发回服务器。 */
#[derive(Clone, Debug)]
pub struct CatalogTool {
    pub server_name: String,
    pub raw_name: String,
    pub public_name: String,
    pub description: String,
    pub parameters: Value,
    pub read_only: bool,
    pub exposed: bool,
}

/** 工具结果里橘记真正认识的内容块。图片和音频不进模型上下文。 */
#[derive(Clone, Debug)]
pub enum ContentPiece {
    Text(String),
    Image { mime: String },
    Audio { mime: String },
    Resource,
    Other,
}

/** 投影后的工具结果。success 为 false 时模型看到的是失败。 */
#[derive(Clone, Debug)]
pub struct ProjectedCall {
    pub success: bool,
    pub text: String,
}

/** 只有本机交互会话的进阶或完全级别能看见 MCP 工具；规划模式一律隐藏。 */
pub fn session_exposes_mcp(local_interactive: bool, security_level: &str, plan_mode: bool) -> bool {
    !plan_mode && local_interactive && matches!(security_level, "advanced" | "autonomous")
}

/** 完全级别不再打断。其余级别只放行显式标注只读的工具。 */
pub fn needs_user_approval(security_level: &str, read_only: bool) -> bool {
    security_level != "autonomous" && !read_only
}

/** 补齐供应商要求的 object schema，并去掉不会被调用的元字段。 */
pub fn normalize_input_schema(schema: Value) -> Value {
    let mut object = match schema {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    object.remove("$schema");
    object.remove("$id");
    if !object.contains_key("type") {
        object.insert("type".to_owned(), Value::String("object".to_owned()));
    }
    if object.get("type").and_then(Value::as_str) == Some("object")
        && !object.contains_key("properties")
    {
        object.insert("properties".to_owned(), Value::Object(Map::new()));
    }
    Value::Object(object)
}

/** 转成 OpenAI-compatible function tool。不暴露被关掉或与宿主撞名的工具。 */
pub fn model_tool_schema(tool: &CatalogTool) -> Option<Value> {
    if !tool.exposed || collides_with_host_tool(&tool.public_name) {
        return None;
    }
    Some(json!({
        "type": "function",
        "function": {
            "name": tool.public_name,
            "description": tool.description,
            "parameters": tool.parameters
        }
    }))
}

/** 把 MCP 内容块收成一段文本。is_error 为真时调用记失败。 */
pub fn project_call(is_error: bool, pieces: &[ContentPiece]) -> ProjectedCall {
    let mut parts = Vec::new();
    for piece in pieces {
        match piece {
            ContentPiece::Text(text) => {
                if !text.is_empty() {
                    parts.push(text.clone());
                }
            }
            ContentPiece::Image { mime } => parts.push(format!("（图片 {mime} 未送入上下文）")),
            ContentPiece::Audio { mime } => parts.push(format!("（音频 {mime} 未送入上下文）")),
            ContentPiece::Resource => parts.push("（嵌入资源未送入上下文）".to_owned()),
            ContentPiece::Other => parts.push("（未支持的内容块未送入上下文）".to_owned()),
        }
    }
    let text = if parts.is_empty() {
        if is_error {
            "MCP 工具返回了错误，但没有文本说明。".to_owned()
        } else {
            "MCP 工具没有返回文本。".to_owned()
        }
    } else {
        parts.join("\n")
    };
    ProjectedCall {
        success: !is_error,
        text,
    }
}

/** 确认弹窗里展示的参数预览，避免把超大 JSON 铺满对话框。 */
pub fn arguments_preview(arguments: &Value) -> String {
    let text = arguments.to_string();
    let mut chars = text.chars();
    let preview: String = chars.by_ref().take(500).collect();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_and_remote_sessions_hide_mcp() {
        assert!(!session_exposes_mcp(true, "basic", false));
        assert!(!session_exposes_mcp(false, "advanced", false));
        assert!(!session_exposes_mcp(false, "autonomous", false));
        assert!(session_exposes_mcp(true, "advanced", false));
        assert!(session_exposes_mcp(true, "autonomous", false));
    }

    /** 规划模式下任何级别的本机会话都不得暴露 MCP。 */
    #[test]
    fn plan_mode_hides_mcp() {
        assert!(!session_exposes_mcp(true, "advanced", true));
        assert!(!session_exposes_mcp(true, "autonomous", true));
    }

    #[test]
    fn approval_skips_readonly_and_autonomous() {
        assert!(needs_user_approval("advanced", false));
        assert!(!needs_user_approval("advanced", true));
        assert!(!needs_user_approval("autonomous", false));
    }

    #[test]
    fn schema_defaults_to_object_and_error_is_not_success() {
        let schema = normalize_input_schema(json!({ "description": "args" }));
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"].is_object());

        let image = project_call(
            false,
            &[ContentPiece::Image {
                mime: "image/png".to_owned(),
            }],
        );
        assert!(image.success);
        assert!(image.text.contains("未送入上下文"));

        let failed = project_call(true, &[ContentPiece::Text("nope".to_owned())]);
        assert!(!failed.success);
        assert_eq!(failed.text, "nope");
    }

    #[test]
    fn host_collision_is_omitted_from_model_schema() {
        let tool = CatalogTool {
            server_name: "docs".to_owned(),
            raw_name: "search".to_owned(),
            public_name: "search".to_owned(),
            description: "冲突".to_owned(),
            parameters: json!({ "type": "object", "properties": {} }),
            read_only: true,
            exposed: true,
        };
        assert!(model_tool_schema(&tool).is_none());
    }
}
