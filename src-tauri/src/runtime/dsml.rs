use crate::storage::create_id;
use serde_json::{json, Value};

/** DeepSeek 兼容服务有时把工具调用塞进正文 DSML 标签里，运行时需要兜底解析。 */
pub(crate) const DSML_TOOL_CALL_OPEN_MARKERS: [&str; 2] =
    ["<｜｜DSML｜｜tool_calls>", "<||DSML||tool_calls>"];

/** DSML 工具调用块结束标签，和 open marker 分开查找以兼容全角/半角竖线混用。 */
pub(crate) const DSML_TOOL_CALL_CLOSE_MARKERS: [&str; 2] =
    ["</｜｜DSML｜｜tool_calls>", "</||DSML||tool_calls>"];

/** 从模型正文提取出的 DSML 工具调用，同时保留可展示正文。 */
pub(crate) struct DsmlToolCallExtraction {
    pub(crate) visible_content: String,
    pub(crate) tool_calls: Vec<Value>,
}

/** 从模型 message 中提取标准 tool_calls，并兼容正文里的 DSML 伪工具调用。 */
pub(crate) fn extract_tool_calls_from_message(message: &Value) -> DsmlToolCallExtraction {
    let content = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let dsml_tool_calls = parse_dsml_tool_calls(content);

    tool_calls.extend(dsml_tool_calls);

    DsmlToolCallExtraction {
        visible_content: strip_dsml_tool_calls(content).trim().to_owned(),
        tool_calls,
    }
}

/** 把 DSML 解析出的工具调用补回 assistant message，便于后续 tool role 消息满足协议顺序。 */
pub(crate) fn normalize_assistant_tool_message(
    mut message: Value,
    tool_calls: &[Value],
    visible_content: &str,
) -> Value {
    if tool_calls.is_empty() {
        return message;
    }

    if let Some(message_object) = message.as_object_mut() {
        message_object.insert("tool_calls".to_owned(), Value::Array(tool_calls.to_vec()));
        message_object.insert(
            "content".to_owned(),
            if visible_content.trim().is_empty() {
                Value::Null
            } else {
                Value::String(visible_content.trim().to_owned())
            },
        );
    }

    message
}

/** 移除正文里的 DSML 工具调用块，避免标签泄露到用户可见回答。 */
pub(crate) fn strip_dsml_tool_calls(content: &str) -> String {
    let mut output = String::with_capacity(content.len());
    let mut cursor = 0usize;

    while let Some((open_offset, open_marker)) =
        find_next_marker(&content[cursor..], &DSML_TOOL_CALL_OPEN_MARKERS)
    {
        let open_start = cursor + open_offset;
        let block_start = open_start + open_marker.len();

        output.push_str(&content[cursor..open_start]);

        if let Some((close_offset, close_marker)) =
            find_next_marker(&content[block_start..], &DSML_TOOL_CALL_CLOSE_MARKERS)
        {
            cursor = block_start + close_offset + close_marker.len();
        } else {
            // 不完整 DSML 块通常来自模型截断；为了避免泄露伪标签，直接丢弃尾部。
            cursor = content.len();
        }
    }

    output.push_str(&content[cursor..]);
    output
}

/** 解析 DSML tool_calls 块，转换为 OpenAI-compatible tool_call 结构。 */
pub(crate) fn parse_dsml_tool_calls(content: &str) -> Vec<Value> {
    let mut tool_calls = Vec::new();
    let mut cursor = 0usize;

    while let Some((open_offset, open_marker)) =
        find_next_marker(&content[cursor..], &DSML_TOOL_CALL_OPEN_MARKERS)
    {
        let block_start = cursor + open_offset + open_marker.len();
        let Some((close_offset, close_marker)) =
            find_next_marker(&content[block_start..], &DSML_TOOL_CALL_CLOSE_MARKERS)
        else {
            break;
        };
        let block_end = block_start + close_offset;

        tool_calls.extend(parse_dsml_invokes(&content[block_start..block_end]));
        cursor = block_end + close_marker.len();
    }

    tool_calls
}

/** 在 DSML 工具块里解析一个或多个 invoke 标签。 */
pub(crate) fn parse_dsml_invokes(block: &str) -> Vec<Value> {
    let mut invokes = Vec::new();
    let mut cursor = 0usize;

    while let Some(open_tag) = find_dsml_open_tag(block, "invoke", cursor) {
        let Some(close_tag) = find_dsml_close_tag(block, "invoke", open_tag.end) else {
            break;
        };
        let invoke_body = &block[open_tag.end..close_tag.start];
        let Some(name) = parse_dsml_attribute(open_tag.attributes, "name")
            .filter(|value| !value.trim().is_empty())
        else {
            cursor = close_tag.end;
            continue;
        };
        let args = parse_dsml_parameters(invoke_body);
        let args_json = serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_owned());

        invokes.push(json!({
            "id": create_id("dsml-tool-call"),
            "type": "function",
            "function": {
                "name": name,
                "arguments": args_json
            }
        }));
        cursor = close_tag.end;
    }

    invokes
}

/** 在 invoke 内解析 parameter 标签，支持字符串和 JSON 数组/对象参数。 */
pub(crate) fn parse_dsml_parameters(invoke_body: &str) -> Value {
    let mut args = serde_json::Map::new();
    let mut cursor = 0usize;

    while let Some(open_tag) = find_dsml_open_tag(invoke_body, "parameter", cursor) {
        let Some(close_tag) = find_dsml_close_tag(invoke_body, "parameter", open_tag.end) else {
            break;
        };
        let raw_value = &invoke_body[open_tag.end..close_tag.start];

        if let Some(name) = parse_dsml_attribute(open_tag.attributes, "name")
            .filter(|value| !value.trim().is_empty())
        {
            args.insert(
                name,
                decode_dsml_parameter_value(raw_value, open_tag.attributes),
            );
        }

        cursor = close_tag.end;
    }

    Value::Object(args)
}

/** DSML 参数值统一去掉标签排版带来的外层空白，并按声明尝试解析 JSON。 */
pub(crate) fn decode_dsml_parameter_value(raw_value: &str, attributes: &str) -> Value {
    let decoded = html_unescape_minimal(raw_value).trim().to_owned();
    let is_string = parse_dsml_attribute(attributes, "string")
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !is_string {
        if let Ok(value) = serde_json::from_str::<Value>(&decoded) {
            return value;
        }
    }

    Value::String(decoded)
}

/** DSML 标签位置，start/end 是原字符串的字节索引。 */
pub(crate) struct DsmlTag<'a> {
    end: usize,
    attributes: &'a str,
}

/** 查找指定名称的 DSML 开始标签，兼容全角和半角竖线标记。 */
pub(crate) fn find_dsml_open_tag<'a>(
    content: &'a str,
    tag_name: &str,
    cursor: usize,
) -> Option<DsmlTag<'a>> {
    let mut search_start = cursor;

    while search_start < content.len() {
        let (prefix_offset, prefix) =
            find_next_marker(&content[search_start..], &["<｜｜DSML｜｜", "<||DSML||"])?;
        let start = search_start + prefix_offset;
        let name_start = start + prefix.len();

        if !content[name_start..].starts_with(tag_name) {
            search_start = name_start;
            continue;
        }

        let attributes_start = name_start + tag_name.len();
        let next_char = content[attributes_start..].chars().next();

        if !matches!(next_char, Some('>' | ' ' | '\t' | '\n' | '\r')) {
            search_start = attributes_start;
            continue;
        }

        let tag_end = content[attributes_start..].find('>')? + attributes_start;

        return Some(DsmlTag {
            end: tag_end + 1,
            attributes: &content[attributes_start..tag_end],
        });
    }

    None
}

/** 查找指定名称的 DSML 结束标签。 */
pub(crate) fn find_dsml_close_tag(
    content: &str,
    tag_name: &str,
    cursor: usize,
) -> Option<DsmlCloseTag> {
    let fullwidth_marker = format!("</｜｜DSML｜｜{tag_name}>");
    let ascii_marker = format!("</||DSML||{tag_name}>");
    let (offset, marker) = find_next_marker(
        &content[cursor..],
        &[fullwidth_marker.as_str(), ascii_marker.as_str()],
    )?;
    let start = cursor + offset;

    Some(DsmlCloseTag {
        start,
        end: start + marker.len(),
    })
}

/** DSML 结束标签位置。 */
pub(crate) struct DsmlCloseTag {
    start: usize,
    end: usize,
}

/** 查找多个 marker 中最靠前的一项。 */
pub(crate) fn find_next_marker<'a>(content: &str, markers: &[&'a str]) -> Option<(usize, &'a str)> {
    markers
        .iter()
        .filter_map(|marker| content.find(marker).map(|offset| (offset, *marker)))
        .min_by_key(|(offset, _)| *offset)
}

/** 从 DSML 标签属性里读取 name="value" 形式的值。 */
pub(crate) fn parse_dsml_attribute(attributes: &str, name: &str) -> Option<String> {
    let pattern = format!("{name}=");
    let pattern_start = attributes.find(&pattern)?;
    let mut value_source = attributes[pattern_start + pattern.len()..].trim_start();
    let quote = value_source.chars().next()?;

    if quote != '"' && quote != '\'' {
        return None;
    }

    value_source = &value_source[quote.len_utf8()..];
    let value_end = value_source.find(quote)?;

    Some(html_unescape_minimal(&value_source[..value_end]))
}

/** 极小 HTML 反转义，覆盖模型常见的 DSML 参数转义，不引入额外依赖。 */
pub(crate) fn html_unescape_minimal(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}
