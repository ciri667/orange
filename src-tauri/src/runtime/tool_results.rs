//! 工具结果回填：浏览器使用结构化分页，其它工具在长度预算内尽量保留 JSON 结构。

use crate::domain::AgentToolCall;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/** 单条工具结果的图片消息；调用方应在整批 tool 消息之后再追加，避免打断 tool 协议。 */
pub(super) fn append_tool_result_messages(
    messages: &mut Vec<Value>,
    tool_call_id: &str,
    result_text: &str,
    payload: &Value,
) -> Option<Value> {
    messages.push(json!({
        "role": "tool",
        "tool_call_id": tool_call_id,
        "content": result_text,
    }));
    let Some(images) = payload.get("documentImages").and_then(Value::as_array) else {
        return None;
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
        Some(json!({
            "role": "user",
            "content": parts,
            // 附件引用在请求前展开；落盘时主、子循环都去掉临时图片消息。
            "ephemeral": true,
        }))
    } else {
        None
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
        compact_tool_result_for_model(&call.name, payload, super::MAX_TOOL_RESULT_CHARS)
    }
}

/** 把超长工具结果收进预算：优先裁数组、再裁字符串，保留 locator/计数等结构。 */
fn compact_tool_result_for_model(name: &str, payload: &Value, max_chars: usize) -> String {
    let serialized = payload.to_string();
    if serialized.chars().count() <= max_chars {
        return serialized;
    }

    let original_arrays = top_level_array_lengths(payload);
    let mut value = payload.clone();
    // task 的 content 是 text 的 XML 包装，重复占预算；保留 text 即可。
    if name == "task" {
        if let Some(object) = value.as_object_mut() {
            object.remove("content");
        }
    }
    // 预留元数据/hint 的空间，避免裁到 9000 后加提示又超。
    let budget = max_chars.saturating_sub(700);

    if name == "exec" {
        compact_exec_strings(&mut value, budget);
    }
    trim_top_level_arrays(&mut value, budget);
    shrink_long_strings(&mut value, budget);

    let returned = returned_array_counts(&value, &original_arrays);
    if json_chars(&value) > budget {
        return structured_fallback(name, payload, max_chars, &returned);
    }

    if !returned.is_empty() {
        value["returned"] = json!(returned);
    }
    value["modelTruncated"] = json!(true);
    value["modelTruncatedAtChars"] = json!(max_chars);
    value["hint"] = json!(compact_hint(name, &returned));

    let mut text = value.to_string();
    if text.chars().count() > max_chars {
        // 元数据把结果顶过线时再收一轮；仍不行就退到结构化摘要。
        trim_top_level_arrays(&mut value, budget);
        shrink_long_strings(&mut value, budget);
        text = value.to_string();
        if text.chars().count() > max_chars {
            return structured_fallback(name, payload, max_chars, &returned);
        }
    }
    text
}

/** 顶层数组的原始长度，用于告诉模型“总共多少、给了多少”。 */
fn top_level_array_lengths(value: &Value) -> BTreeMap<String, usize> {
    value
        .as_object()
        .map(|object| {
            object
                .iter()
                .filter_map(|(key, value)| {
                    value
                        .as_array()
                        .map(|items| (key.clone(), items.len()))
                        .filter(|(_, len)| *len > 0)
                })
                .collect()
        })
        .unwrap_or_default()
}

/** 反复从当前最“占地方”的顶层数组尾部移除元素，直到落进预算或数组为空。 */
fn trim_top_level_arrays(value: &mut Value, budget: usize) {
    while json_chars(value) > budget {
        let Some(key) = largest_top_level_array(value) else {
            break;
        };
        let Some(array) = value.get_mut(&key).and_then(Value::as_array_mut) else {
            break;
        };
        // 至少保留一项，让模型还能看到一条结果；剩下的交给字符串裁剪。
        if array.len() <= 1 {
            break;
        }
        if array.pop().is_none() {
            break;
        }
    }
}

/** 找出序列化后最大的顶层数组；没有可裁数组时返回 None。 */
fn largest_top_level_array(value: &Value) -> Option<String> {
    value.as_object().and_then(|object| {
        object
            .iter()
            .filter_map(|(key, value)| {
                let array = value.as_array()?;
                if array.is_empty() {
                    return None;
                }
                let size: usize = array
                    .iter()
                    .map(|item| item.to_string().chars().count())
                    .sum();
                Some((key.clone(), size))
            })
            .max_by_key(|(_, size)| *size)
            .map(|(key, _)| key)
    })
}

/** 从长字符串开始逐步收紧，保证 JSON 仍然完整。 */
fn shrink_long_strings(value: &mut Value, budget: usize) {
    for limit in [4_000usize, 2_000, 1_000, 500, 200, 100] {
        if json_chars(value) <= budget {
            break;
        }
        truncate_json_strings_in_place(value, limit);
    }
}

/** 递归截断字符串值，保留对象/数组结构；键名不截断。 */
fn truncate_json_strings_in_place(value: &mut Value, max_chars: usize) {
    match value {
        Value::String(text) => {
            if text.chars().count() > max_chars {
                *text = truncate_text(text, max_chars);
            }
        }
        Value::Array(items) => {
            for item in items {
                truncate_json_strings_in_place(item, max_chars);
            }
        }
        Value::Object(object) => {
            for child in object.values_mut() {
                truncate_json_strings_in_place(child, max_chars);
            }
        }
        _ => {}
    }
}

/** exec 的 stdout/stderr 首尾各留一段，错误通常在尾部。 */
fn compact_exec_strings(value: &mut Value, budget: usize) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    let stdout_len = object
        .get("stdout")
        .and_then(Value::as_str)
        .map(|text| text.chars().count())
        .unwrap_or(0);
    let stderr_len = object
        .get("stderr")
        .and_then(Value::as_str)
        .map(|text| text.chars().count())
        .unwrap_or(0);
    let available = budget.saturating_sub(1_200).max(600);
    let total = stdout_len.saturating_add(stderr_len);
    if total <= available {
        return;
    }
    let stdout_budget = if total == 0 {
        0
    } else {
        available.saturating_mul(stdout_len) / total
    };
    let stderr_budget = available.saturating_sub(stdout_budget);

    for (key, limit, flag) in [
        ("stdout", stdout_budget, "stdoutTruncated"),
        ("stderr", stderr_budget, "stderrTruncated"),
    ] {
        if let Some(text) = object.get(key).and_then(Value::as_str) {
            if text.chars().count() > limit && limit > 0 {
                object.insert(key.to_owned(), json!(truncate_head_tail(text, limit)));
                object.insert(flag.to_owned(), json!(true));
            }
        }
    }
}

/** 保留首尾、省略中间的字符截断；用于命令输出。 */
fn truncate_head_tail(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let marker = "\n\n[…输出过长，已省略中间部分…]\n\n";
    let marker_chars = marker.chars().count();
    let remaining = max_chars.saturating_sub(marker_chars);
    let head_chars = remaining.saturating_mul(2) / 3;
    let tail_chars = remaining.saturating_sub(head_chars);
    let head: String = text.chars().take(head_chars).collect();
    let tail: String = text
        .chars()
        .rev()
        .take(tail_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}{marker}{tail}")
}

/** 头部截断并加短标记。 */
fn truncate_text(text: &str, max_chars: usize) -> String {
    let marker = "…（已截断）";
    let marker_chars = marker.chars().count();
    let head = max_chars.saturating_sub(marker_chars);
    let mut truncated: String = text.chars().take(head).collect();
    truncated.push_str(marker);
    truncated
}

/** 统计被裁掉的顶层数组，生成 returned 计数。 */
fn returned_array_counts(
    value: &Value,
    original: &BTreeMap<String, usize>,
) -> BTreeMap<String, usize> {
    original
        .iter()
        .filter_map(|(key, original_len)| {
            let current = value
                .get(key)
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            (current < *original_len).then_some((key.clone(), current))
        })
        .collect()
}

/** 结构化摘要兜底：极端情况下也不把原始 9000 字符中间态直接丢给模型。 */
fn structured_fallback(
    name: &str,
    payload: &Value,
    max_chars: usize,
    returned: &BTreeMap<String, usize>,
) -> String {
    let preview = payload.to_string().chars().take(1_200).collect::<String>();
    json!({
        "modelTruncated": true,
        "modelTruncatedAtChars": max_chars,
        "returned": returned,
        "summary": preview,
        "hint": compact_hint(name, returned),
    })
    .to_string()
}

/** 按工具类型给出可执行的收窄建议，避免提示模型继续提高 limit。 */
fn compact_hint(name: &str, returned: &BTreeMap<String, usize>) -> String {
    if name.starts_with("mcp__") {
        return "MCP 返回文本过长，已按上下文预算截断；请让服务端分页或缩小查询。".to_owned();
    }
    match name {
        "search" => {
            let kept = returned
                .get("citations")
                .or_else(|| returned.get("hits"))
                .or_else(|| returned.get("sources"))
                .copied()
                .unwrap_or(0);
            format!("结果过长，已保留前 {kept} 条并裁短字段；请缩小 query 或降低 limit。")
        }
        "list" => {
            let kept: usize = returned.values().sum();
            format!("结果过长，已保留 {kept} 项并保留 totals；请用 prefix、query 或 fileType 缩小范围。")
        }
        "exec" => "输出过长，已保留 stdout/stderr 的首尾片段；请把命令输出重定向到文件后用 read 分段读取。".to_owned(),
        _ => "结果过长，已按上下文预算保留结构；请缩小请求范围。".to_owned(),
    }
}

/** 当前 JSON 的字符数。 */
fn json_chars(value: &Value) -> usize {
    value.to_string().chars().count()
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
        let image_message = append_tool_result_messages(
            &mut messages,
            "read-image",
            &payload.to_string(),
            &payload,
        )
        .expect("image message should be returned");
        messages.push(image_message);

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
            let image = append_tool_result_messages(
                &mut messages,
                "read-image",
                &payload.to_string(),
                &payload,
            );
            assert!(image.is_none());
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0]["role"], "tool");
        }
    }

    /** 多工具批次：图片消息必须延后到所有 tool 消息之后，不能夹在中间。 */
    #[test]
    fn image_message_is_deferred_until_after_all_tool_messages() {
        let first = json!({ "documentImages": [{ "id": "image-1" }] });
        let second = json!({ "text": "第二个工具结果" });
        let mut messages = Vec::new();
        let image =
            append_tool_result_messages(&mut messages, "call-1", &first.to_string(), &first)
                .expect("first tool should return an image message");
        assert!(
            append_tool_result_messages(&mut messages, "call-2", &second.to_string(), &second)
                .is_none()
        );
        assert_eq!(
            messages
                .iter()
                .map(|message| message["role"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["tool", "tool"]
        );

        messages.push(image);
        assert_eq!(messages.last().unwrap()["role"], "user");
        assert_eq!(messages.last().unwrap()["ephemeral"], true);
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
        assert_eq!(result["modelTruncated"], true);
        assert!(result["text"]
            .as_str()
            .unwrap_or_default()
            .contains("已截断"));
    }

    /** 大 list 结果保留 totals 和结构，而不是退化成 400 字符前缀。 */
    #[test]
    fn large_list_payload_keeps_totals_and_trimmed_items() {
        let call = AgentToolCall {
            id: "list-large".to_owned(),
            name: "list".to_owned(),
            status: "completed".to_owned(),
            summary: String::new(),
            args: json!({}),
        };
        let documents = (0..120)
            .map(|index| {
                json!({
                    "id": format!("document-{index:04}-a1b2c3d4"),
                    "title": format!("季度产品研究文档 {index}"),
                    "path": format!("Materials/2026/Q3/季度产品研究文档-{index}.pdf"),
                    "knowledgeBaseId": "kb-6f1c2a9d8e7b4a3c",
                    "knowledgeBaseName": "产品研究知识库",
                    "fileType": "pdf",
                    "previewAvailable": true,
                    "agentReadable": true,
                    "readOnly": true,
                    "requiresVision": false
                })
            })
            .collect::<Vec<_>>();
        let payload = json!({
            "knowledgeBases": [{ "id": "kb-6f1c2a9d8e7b4a3c", "name": "产品研究知识库" }],
            "folders": [],
            "notes": [],
            "documents": documents,
            "totalFolders": 0,
            "totalNotes": 0,
            "totalDocuments": 120,
            "totalFiles": 120,
            "fileTypeCounts": { "markdown": 0, "txt": 0, "docx": 0, "pdf": 120, "image": 0 },
            "truncated": true,
            "limit": 120
        });

        let text = model_tool_result_text(&call, &payload);
        assert!(text.chars().count() <= super::super::MAX_TOOL_RESULT_CHARS);
        let result: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(result["modelTruncated"], true);
        assert_eq!(result["totalDocuments"], 120);
        let returned = result["documents"].as_array().unwrap();
        assert!(!returned.is_empty());
        assert!(returned.len() < 120);
        assert_eq!(result["returned"]["documents"], returned.len());
        assert!(result["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("缩小范围"));
    }

    /** 大 search 结果保留 citations 数组和 returned 计数。 */
    #[test]
    fn large_search_payload_keeps_structured_citations() {
        let call = AgentToolCall {
            id: "search-large".to_owned(),
            name: "search".to_owned(),
            status: "completed".to_owned(),
            summary: String::new(),
            args: json!({ "query": "隐私边界", "limit": 16 }),
        };
        let citations = (0..16)
            .map(|index| {
                json!({
                    "knowledgeBaseId": "kb-a",
                    "knowledgeBaseName": "知识库",
                    "noteId": format!("note-{index}"),
                    "title": format!("季度产品研究笔记 {index}"),
                    "path": format!("Notes/季度产品研究笔记-{index}.md"),
                    "snippet": "这".repeat(500),
                    "score": 0.9,
                    "sourceType": "note"
                })
            })
            .collect::<Vec<_>>();
        let payload = json!({
            "citations": citations,
            "mode": "chunk",
            "truncated": true,
            "limit": 16
        });

        let text = model_tool_result_text(&call, &payload);
        assert!(text.chars().count() <= super::super::MAX_TOOL_RESULT_CHARS);
        let result: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(result["modelTruncated"], true);
        let kept = result["citations"].as_array().unwrap();
        assert!(!kept.is_empty());
        assert!(kept.len() < 16);
        assert_eq!(result["returned"]["citations"], kept.len());
        assert!(result["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("降低 limit"));
    }

    /** 大 exec 输出保留退出码、shell 和 stdout/stderr 的尾部。 */
    #[test]
    fn large_exec_payload_keeps_exit_code_and_tail() {
        let call = AgentToolCall {
            id: "exec-large".to_owned(),
            name: "exec".to_owned(),
            status: "completed".to_owned(),
            summary: String::new(),
            args: json!({ "command": "build" }),
        };
        let payload = json!({
            "exitCode": 1,
            "stdout": format!("{}\nTAIL-STDOUT", "构建日志".repeat(3000)),
            "stderr": format!("HEAD-STDERR\n{}", "错误信息".repeat(1000)),
            "timedOut": false,
            "cancelled": false,
            "truncated": false,
            "shell": "pwsh.exe"
        });

        let text = model_tool_result_text(&call, &payload);
        assert!(text.chars().count() <= super::super::MAX_TOOL_RESULT_CHARS);
        let result: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(result["exitCode"], 1);
        assert_eq!(result["shell"], "pwsh.exe");
        assert!(result["stdoutTruncated"] == true || result["stderrTruncated"] == true);
        assert!(result["stdout"]
            .as_str()
            .unwrap_or_default()
            .contains("TAIL-STDOUT"));
        assert!(result["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("重定向"));
    }
}
