//! 工具结果回填：浏览器使用结构化分页，其它工具继续沿用通用长度限制。

use crate::domain::AgentToolCall;
use serde_json::{json, Value};

/** 主循环和子循环共用回填：图片必须放在独立 user 消息中，不能只返回附件元数据。 */
pub(super) fn append_tool_result_messages(
    messages: &mut Vec<Value>,
    tool_call_id: &str,
    result_text: &str,
    payload: &Value,
) {
    messages.push(json!({
        "role": "tool",
        "tool_call_id": tool_call_id,
        "content": result_text,
    }));
    let Some(images) = payload.get("documentImages").and_then(Value::as_array) else {
        return;
    };
    let mut parts = vec![json!({
        "type": "text",
        "text": "以下是本次工具返回的图片（PDF 页面、DOCX 内嵌图、图片文件或笔记插图）。请直接读图回答，并在引用时指明对应页码或图注。"
    })];
    for image in images {
        if let Some(id) = image.get("id").and_then(Value::as_str) {
            parts.push(json!({ "type": crate::storage::MODEL_IMAGE_REF_TYPE, "id": id }));
        }
    }
    if parts.len() > 1 {
        messages.push(json!({
            "role": "user",
            "content": parts,
            // 附件引用在请求前展开；落盘时主、子循环都去掉临时图片消息。
            "ephemeral": true,
        }));
    }
}

/** 已分页的浏览器快照必须原样保留，字符串截断会丢失 reference 和下一页偏移。 */
pub(super) fn model_tool_result_text(call: &AgentToolCall, payload: &Value) -> String {
    let serialized = payload.to_string();
    if call.name == "browser"
        || (call.name == "run" && call.args.get("kind").and_then(Value::as_str) == Some("browser"))
    {
        serialized
    } else {
        super::context::truncate_tool_result_for_model(&serialized, super::MAX_TOOL_RESULT_CHARS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /** 工具生成的附件必须成为真实 image_url，并在落盘前剔除临时图片消息。 */
    #[test]
    fn tool_images_reach_model_as_pixels_and_stay_out_of_transcripts() {
        let dir = tempfile::tempdir().unwrap();
        let attachments = dir.path().join("attachments");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([10, 20, 30, 255]))
            .save(dir.path().join("diagram.png"))
            .unwrap();
        let image =
            crate::storage::attach_local_image(dir.path(), "diagram.png", &attachments, "图示")
                .unwrap();
        let payload = json!({ "documentImages": [image.to_payload()] });
        let mut messages = Vec::new();
        append_tool_result_messages(&mut messages, "read-image", &payload.to_string(), &payload);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "tool");
        assert_eq!(messages[0]["tool_call_id"], "read-image");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"][1]["id"], image.attachment.id);
        assert_eq!(messages[1]["ephemeral"], true);
        let hydrated = crate::storage::hydrate_model_images(&attachments, &messages);
        assert_eq!(hydrated[1]["content"][1]["type"], "image_url");
        assert!(hydrated[1]["content"][1]["image_url"]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
        assert_eq!(
            super::super::persistent_model_messages(&messages),
            vec![messages[0].clone()]
        );
    }

    /** 只有元数据、空图片列表或坏引用时，不能构造暗示“已附图”的 user 消息。 */
    #[test]
    fn metadata_only_tool_results_do_not_append_image_messages() {
        for payload in [
            json!({ "visionUnavailable": true }),
            json!({ "documentImages": [] }),
            json!({ "documentImages": [{}] }),
        ] {
            let mut messages = Vec::new();
            append_tool_result_messages(
                &mut messages,
                "read-image",
                &payload.to_string(),
                &payload,
            );
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0]["role"], "tool");
        }
    }

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
