//! 工具结果回填：浏览器使用结构化分页，其它工具继续沿用通用长度限制。

use crate::domain::AgentToolCall;
use serde_json::Value;

/** 已分页的浏览器快照必须原样保留，字符串截断会丢失 reference 和下一页偏移。 */
pub(super) fn model_tool_result_text(call: &AgentToolCall, payload: &Value) -> String {
    let serialized = payload.to_string();
    if call.name == "browser" || (call.name == "run" && call.args.get("kind").and_then(Value::as_str) == Some("browser")) {
        serialized
    } else {
        super::context::truncate_tool_result_for_model(&serialized, super::MAX_TOOL_RESULT_CHARS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn large_browser_result_keeps_element_references_and_paging_metadata() {
        let call = AgentToolCall {
            id: "browser-snapshot".to_owned(),
            name: "browser".to_owned(),
            status: "completed".to_owned(),
            summary: String::new(),
            args: json!({ "kind": "browser", "operation": "page.elements.snapshot" }),
        };
        let payload = json!({
            "ok": true,
            "operation": "page.elements.snapshot",
            "snapshot": {
                "snapshotId": "snap-test", "pageGeneration": 1,
                "elements": (0..20).map(|index| json!({
                    "reference": format!("snap-test-{index}"),
                    "name": "中文字段".repeat(60), "text": "页面内容".repeat(60)
                })).collect::<Vec<_>>(),
                "truncated": true, "nextElementOffset": 20
            }
        });
        assert!(payload.to_string().chars().count() > super::super::MAX_TOOL_RESULT_CHARS);
        let result: Value = serde_json::from_str(&model_tool_result_text(&call, &payload)).unwrap();
        assert_eq!(result, payload);
        assert_eq!(
            result["snapshot"]["elements"][19]["reference"],
            "snap-test-19"
        );
        assert_eq!(result["snapshot"]["nextElementOffset"], 20);
    }

    #[test]
    fn ordinary_tool_results_still_use_the_existing_limit() {
        let call = AgentToolCall {
            id: "read".to_owned(),
            name: "read".to_owned(),
            status: "completed".to_owned(),
            summary: String::new(),
            args: json!({}),
        };
        let payload = json!({ "text": "x".repeat(super::super::MAX_TOOL_RESULT_CHARS + 1) });
        let result: Value = serde_json::from_str(&model_tool_result_text(&call, &payload)).unwrap();
        assert_eq!(result["truncated"], true);
    }
}
