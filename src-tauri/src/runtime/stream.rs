use serde_json::{json, Value};

/** 把 HTTP 分片重组成完整的 SSE `data:` 事件；跨 chunk 的半截行和半截 UTF-8 都留在缓冲里。 */
#[derive(Default)]
pub struct SseBuffer {
    bytes: Vec<u8>,
    line: String,
    data_lines: Vec<String>,
}

impl SseBuffer {
    /** 追加一段网络分片，返回已经用空行结束的 data 载荷（不含 `data:` 前缀）。 */
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.bytes.extend_from_slice(bytes);
        let text = take_complete_utf8(&mut self.bytes);
        self.line.push_str(&text);
        self.drain_complete_events()
    }

    /** 连接结束后冲出最后一条未以空行结束的 data 事件。 */
    pub fn flush(&mut self) -> Vec<String> {
        if !self.line.is_empty() {
            let line = std::mem::take(&mut self.line);
            self.handle_line(line.trim_end_matches(['\r', '\n']));
        }
        self.take_data_event().into_iter().collect()
    }

    fn drain_complete_events(&mut self) -> Vec<String> {
        let mut events = Vec::new();
        while let Some(newline_at) = self.line.find('\n') {
            let mut line: String = self.line.drain(..=newline_at).collect();
            if line.ends_with('\n') {
                line.pop();
            }
            if line.ends_with('\r') {
                line.pop();
            }
            if let Some(event) = self.handle_line(&line) {
                events.push(event);
            }
        }
        events
    }

    fn handle_line(&mut self, line: &str) -> Option<String> {
        if line.is_empty() {
            return self.take_data_event();
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        if field == "data" {
            self.data_lines.push(value.to_owned());
        }
        None
    }

    fn take_data_event(&mut self) -> Option<String> {
        if self.data_lines.is_empty() {
            return None;
        }
        Some(self.data_lines.drain(..).collect::<Vec<_>>().join("\n"))
    }
}

/** 从 Chat Completions SSE delta 累积出的助手消息，结束时再还原成非流式 JSON。 */
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StreamedAssistant {
    pub content: String,
    pub reasoning: String,
    pub tool_calls: Vec<Value>,
    pub finish_reason: Option<String>,
}

impl StreamedAssistant {
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_empty() && self.reasoning.is_empty() && self.tool_calls.is_empty()
    }

    /** 应用一个 SSE JSON 对象：支持 `delta` 增量，也兼容把完整 `message` 塞进流里的代理。 */
    pub fn apply_chunk(&mut self, chunk: &Value) {
        let Some(choice) = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            self.capture_finish_reason(chunk);
            return;
        };

        self.capture_finish_reason(choice);
        if let Some(message) = choice.get("message") {
            self.apply_message_fields(message, false);
        }
        if let Some(delta) = choice.get("delta") {
            self.apply_message_fields(delta, true);
        }
    }

    /** 从非流式 chat.completion JSON 还原，供 provider 忽略 stream 时一次性回调。 */
    pub fn from_completion(value: &Value) -> Self {
        let mut assistant = Self::default();
        assistant.apply_chunk(value);
        assistant
    }

    /** 还原成现有 Agent loop 读取的 `choices[0].message` 形状。 */
    pub fn into_chat_completion(&self) -> Value {
        let content = if self.content.is_empty() {
            Value::Null
        } else {
            Value::String(self.content.clone())
        };
        let mut message = json!({
            "role": "assistant",
            "content": content,
        });
        if !self.tool_calls.is_empty() {
            message["tool_calls"] = Value::Array(self.tool_calls.clone());
        }
        if !self.reasoning.is_empty() {
            message["reasoning_content"] = json!(self.reasoning);
        }
        let finish_reason = self.finish_reason.clone().unwrap_or_else(|| {
            if self.tool_calls.is_empty() {
                "stop".to_owned()
            } else {
                "tool_calls".to_owned()
            }
        });

        json!({
            "choices": [{
                "finish_reason": finish_reason,
                "message": message
            }]
        })
    }

    fn capture_finish_reason(&mut self, value: &Value) {
        let reason = value
            .get("finish_reason")
            .or_else(|| value.get("stop_reason"))
            .or_else(|| value.get("native_finish_reason"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .map(|reason| reason.to_ascii_lowercase());
        if let Some(reason) = reason {
            self.finish_reason = Some(reason);
        }
    }

    fn apply_message_fields(&mut self, fields: &Value, append: bool) {
        if let Some(text) = extract_text(fields.get("content")) {
            if append {
                self.content.push_str(&text);
            } else {
                self.content = text;
            }
        }

        let reasoning = extract_text(fields.get("reasoning_content"))
            .or_else(|| extract_text(fields.get("reasoning")));
        if let Some(text) = reasoning {
            if append {
                self.reasoning.push_str(&text);
            } else {
                self.reasoning = text;
            }
        }

        if let Some(tool_calls) = fields.get("tool_calls").and_then(Value::as_array) {
            if append {
                for tool_call in tool_calls {
                    self.merge_tool_call_delta(tool_call);
                }
            } else {
                self.tool_calls = tool_calls.clone();
            }
        }
    }

    fn merge_tool_call_delta(&mut self, delta: &Value) {
        let index = delta.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        while self.tool_calls.len() <= index {
            self.tool_calls.push(json!({
                "id": "",
                "type": "function",
                "function": {
                    "name": "",
                    "arguments": ""
                }
            }));
        }

        let call = &mut self.tool_calls[index];
        if let Some(id) = delta
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            call["id"] = json!(id);
        }
        if let Some(call_type) = delta.get("type").and_then(Value::as_str) {
            call["type"] = json!(call_type);
        }
        let Some(function) = delta.get("function") else {
            return;
        };
        if let Some(name) = function
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
        {
            call["function"]["name"] = json!(name);
        }
        if let Some(fragment) = tool_arguments_fragment(function.get("arguments")) {
            let current = call["function"]["arguments"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            call["function"]["arguments"] = json!(format!("{current}{fragment}"));
        }
    }
}

/** 消费一段完整或分片的 SSE 正文。遇到 `data: [DONE]` 停止。流内 `error` 对象返回 Err。 */
#[cfg(test)]
pub fn consume_sse_body(body: &str) -> Result<StreamedAssistant, String> {
    let mut buffer = SseBuffer::default();
    let mut assistant = StreamedAssistant::default();
    apply_sse_events(&mut assistant, buffer.push(body.as_bytes()), |_| {})?;
    apply_sse_events(&mut assistant, buffer.flush(), |_| {})?;
    Ok(assistant)
}

/** 把 reqwest 响应体按到达顺序解析：SSE 边读边回调，完整 JSON 则一次性回调。 */
pub async fn read_chat_completion_response(
    mut response: reqwest::Response,
    mut on_progress: impl FnMut(&StreamedAssistant),
) -> Result<Value, String> {
    let mut buffer = SseBuffer::default();
    let mut assistant = StreamedAssistant::default();
    let mut pending = Vec::new();
    let mut json_mode: Option<bool> = None;

    while let Some(chunk) = response.chunk().await.map_err(|error| {
        crate::model_provider::redact_model_error_text(&format!("无法读取模型流式响应：{error}"))
    })? {
        if json_mode.is_none() {
            pending.extend_from_slice(&chunk);
            if looks_like_json_object(&pending) {
                json_mode = Some(true);
            } else if looks_like_sse(&pending) {
                json_mode = Some(false);
                if apply_sse_events(&mut assistant, buffer.push(&pending), &mut on_progress)? {
                    return Ok(assistant.into_chat_completion());
                }
                pending.clear();
            }
            continue;
        }

        if json_mode == Some(true) {
            pending.extend_from_slice(&chunk);
            continue;
        }

        if apply_sse_events(&mut assistant, buffer.push(&chunk), &mut on_progress)? {
            return Ok(assistant.into_chat_completion());
        }
    }

    if json_mode.unwrap_or_else(|| looks_like_json_object(&pending)) {
        let body =
            String::from_utf8(pending).map_err(|error| format!("无法解析模型响应：{error}"))?;
        let value: Value = serde_json::from_str(body.trim())
            .map_err(|error| format!("无法解析模型响应：{error}"))?;
        assistant = StreamedAssistant::from_completion(&value);
        on_progress(&assistant);
        return Ok(value);
    }

    if !pending.is_empty()
        && apply_sse_events(&mut assistant, buffer.push(&pending), &mut on_progress)?
    {
        return Ok(assistant.into_chat_completion());
    }
    if apply_sse_events(&mut assistant, buffer.flush(), &mut on_progress)? {
        return Ok(assistant.into_chat_completion());
    }
    if assistant.is_empty() && assistant.finish_reason.is_none() {
        return Err("模型流式响应为空。".to_owned());
    }
    on_progress(&assistant);
    Ok(assistant.into_chat_completion())
}

/** 把累积中的流式助手消息映射成过程区思考和用户可见回答。 */
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamUiProgress {
    pub thinking: String,
    pub content: String,
}

/** 有 tool_calls 时正文属于过程思考；否则 reasoning 进思考、content 进回答。 */
pub fn stream_ui_progress(streamed: &StreamedAssistant) -> StreamUiProgress {
    let visible = super::dsml::strip_dsml_tool_calls(&streamed.content);
    let visible = visible.trim();
    let reasoning = streamed.reasoning.trim();

    if streamed.has_tool_calls() {
        let thinking = [reasoning, visible]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        return StreamUiProgress {
            thinking,
            content: String::new(),
        };
    }

    StreamUiProgress {
        thinking: reasoning.to_owned(),
        content: visible.to_owned(),
    }
}

/** 首个非空白字节是 `{` 时，把响应当完整 JSON 而不是 SSE。 */
pub fn looks_like_json_object(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .find(|byte| !byte.is_ascii_whitespace())
        .copied()
        == Some(b'{')
}

fn looks_like_sse(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return bytes.iter().any(|byte| *byte == b'\n');
    };
    let trimmed = text.trim_start();
    trimmed.starts_with("data:")
        || trimmed.starts_with("event:")
        || trimmed.starts_with("id:")
        || trimmed.starts_with(':')
}

fn apply_sse_events(
    assistant: &mut StreamedAssistant,
    events: Vec<String>,
    mut on_progress: impl FnMut(&StreamedAssistant),
) -> Result<bool, String> {
    for event in events {
        let trimmed = event.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == "[DONE]" {
            on_progress(assistant);
            return Ok(true);
        }
        let value: Value = serde_json::from_str(trimmed)
            .map_err(|error| format!("无法解析模型流式响应：{error}"))?;
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
                .unwrap_or("模型流式响应返回错误");
            return Err(message.to_owned());
        }
        assistant.apply_chunk(&value);
        on_progress(assistant);
    }
    Ok(false)
}

fn take_complete_utf8(bytes: &mut Vec<u8>) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => {
            let text = text.to_owned();
            bytes.clear();
            text
        }
        Err(error) => {
            let valid_up_to = error.valid_up_to();
            let text = if valid_up_to == 0 {
                String::new()
            } else {
                std::str::from_utf8(&bytes[..valid_up_to])
                    .expect("valid_up_to 之前是合法 UTF-8")
                    .to_owned()
            };
            if let Some(error_len) = error.error_len() {
                bytes.drain(..valid_up_to + error_len);
            } else {
                bytes.drain(..valid_up_to);
            }
            text
        }
    }
}

fn extract_text(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if value.is_null() {
        return None;
    }
    if let Some(text) = value.as_str() {
        return Some(text.to_owned());
    }
    let parts = value.as_array()?;
    let mut text = String::new();
    for part in parts {
        if let Some(piece) = part.as_str() {
            text.push_str(piece);
        } else if let Some(piece) = part.get("text").and_then(Value::as_str) {
            text.push_str(piece);
        }
    }
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn tool_arguments_fragment(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if value.is_null() {
        return None;
    }
    if let Some(text) = value.as_str() {
        return Some(text.to_owned());
    }
    serde_json::to_string(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /** SSE 事件可能被 TCP 切成半截，缓冲必须跨分片拼回完整 data。 */
    #[test]
    fn sse_buffer_reassembles_events_split_across_chunks() {
        let mut buffer = SseBuffer::default();
        let first = buffer.push(r#"data: {"choices":[{"delta":{"content":"你"#.as_bytes());
        assert!(first.is_empty(), "incomplete event must stay buffered");

        let second = buffer.push(r#"好"}}]}"#.as_bytes());
        assert!(
            second.is_empty(),
            "event without blank line is not complete"
        );

        let third = buffer.push(b"\n\n");
        assert_eq!(
            third,
            vec![r#"{"choices":[{"delta":{"content":"你好"}}]}"#.to_owned()]
        );
    }

    /** `[DONE]` 和注释行不能当 JSON 解析；多行 data 要拼成一条载荷。 */
    #[test]
    fn sse_buffer_skips_comments_and_exposes_done() {
        let mut buffer = SseBuffer::default();
        let events =
            buffer.push(b": keep-alive\n\ndata: {\"a\":1}\ndata: {\"b\":2}\n\ndata: [DONE]\n\n");

        assert_eq!(
            events,
            vec!["{\"a\":1}\n{\"b\":2}".to_owned(), "[DONE]".to_owned()]
        );
    }

    /** 中文 token 可能卡在 UTF-8 码点中间，不能把半个字符当损失丢掉。 */
    #[test]
    fn sse_buffer_keeps_incomplete_utf8_until_next_chunk() {
        let mut buffer = SseBuffer::default();
        let nihao = "data: 你好\n\n".as_bytes();
        let split_at = nihao
            .iter()
            .position(|byte| *byte >= 0x80)
            .expect("fixture must contain a multibyte char");

        assert!(buffer.push(&nihao[..split_at + 1]).is_empty());
        assert_eq!(buffer.push(&nihao[split_at + 1..]), vec!["你好".to_owned()]);
    }

    /** 正文 delta 必须按到达顺序拼接，这是真流式而不是事后打字机的前提。 */
    #[test]
    fn streamed_assistant_concatenates_content_deltas() {
        let mut assistant = StreamedAssistant::default();
        assistant.apply_chunk(&json!({
            "choices": [{ "delta": { "role": "assistant", "content": "橘" } }]
        }));
        assistant.apply_chunk(&json!({
            "choices": [{ "delta": { "content": "记" }, "finish_reason": "stop" }]
        }));

        assert_eq!(assistant.content, "橘记");
        assert_eq!(assistant.finish_reason.as_deref(), Some("stop"));
        assert!(!assistant.has_tool_calls());
    }

    /** DeepSeek / OpenRouter 的思考字段要进 reasoning，不能和用户可见回答混在一起。 */
    #[test]
    fn streamed_assistant_reads_reasoning_aliases() {
        let mut assistant = StreamedAssistant::default();
        assistant.apply_chunk(&json!({
            "choices": [{ "delta": { "reasoning_content": "先检索" } }]
        }));
        assistant.apply_chunk(&json!({
            "choices": [{ "delta": { "reasoning": "笔记" } }]
        }));
        assistant.apply_chunk(&json!({
            "choices": [{ "delta": { "content": "找到了" } }]
        }));

        assert_eq!(assistant.reasoning, "先检索笔记");
        assert_eq!(assistant.content, "找到了");
    }

    /** 工具调用参数是分片字符串，必须按 index 拼回完整 arguments。 */
    #[test]
    fn streamed_assistant_merges_tool_call_argument_deltas() {
        let mut assistant = StreamedAssistant::default();
        assistant.apply_chunk(&json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "search", "arguments": "" }
                    }]
                }
            }]
        }));
        assistant.apply_chunk(&json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": { "arguments": "{\"query\":" }
                    }]
                }
            }]
        }));
        assistant.apply_chunk(&json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": { "arguments": "\"本地优先\"}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }));

        assert!(assistant.has_tool_calls());
        assert_eq!(assistant.tool_calls[0]["id"], "call_1");
        assert_eq!(assistant.tool_calls[0]["function"]["name"], "search");
        assert_eq!(
            assistant.tool_calls[0]["function"]["arguments"],
            "{\"query\":\"本地优先\"}"
        );
        assert_eq!(assistant.finish_reason.as_deref(), Some("tool_calls"));
    }

    /** 还原后的 JSON 必须能被现有 extract_tool_calls / parse_finish_reason 读取。 */
    #[test]
    fn streamed_assistant_reconstructs_chat_completion_json() {
        let mut assistant = StreamedAssistant::default();
        assistant.apply_chunk(&json!({
            "choices": [{
                "delta": {
                    "content": "先搜。",
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "search", "arguments": "{\"query\":\"q\"}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }));
        let response = assistant.into_chat_completion();

        assert_eq!(response["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(response["choices"][0]["message"]["role"], "assistant");
        assert_eq!(response["choices"][0]["message"]["content"], "先搜。");
        assert_eq!(
            response["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "search"
        );
    }

    /** 有的兼容服务把完整 message 放进流里，不能只认 delta。 */
    #[test]
    fn streamed_assistant_accepts_full_message_chunk() {
        let mut assistant = StreamedAssistant::default();
        assistant.apply_chunk(&json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "完整回答",
                    "reasoning_content": "思考过"
                },
                "finish_reason": "stop"
            }]
        }));

        assert_eq!(assistant.content, "完整回答");
        assert_eq!(assistant.reasoning, "思考过");
        assert_eq!(assistant.finish_reason.as_deref(), Some("stop"));
    }

    /** 标准 OpenAI SSE 样例必须能攒出完整回答，并在 [DONE] 处结束。 */
    #[test]
    fn consume_sse_body_reads_openai_chat_completion_stream() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"本\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"地\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let assistant = consume_sse_body(body).expect("sse body should parse");

        assert_eq!(assistant.content, "本地");
        assert_eq!(assistant.finish_reason.as_deref(), Some("stop"));
    }

    /** 流里的 error 对象必须变成失败，不能当成正常 delta 吞掉。 */
    #[test]
    fn consume_sse_body_surfaces_stream_error_object() {
        let body = "data: {\"error\":{\"message\":\"insufficient_quota\",\"type\":\"insufficient_quota\"}}\n\n";
        let error = consume_sse_body(body).expect_err("stream error must fail");

        assert!(error.contains("insufficient_quota"));
    }

    /** 非流式 JSON 响应的第一个非空白字节是 `{`，用来决定走整包解析。 */
    #[test]
    fn looks_like_json_object_ignores_leading_whitespace() {
        assert!(looks_like_json_object(b"  \n{ \"choices\": [] }"));
        assert!(!looks_like_json_object(b"data: {}\n\n"));
        assert!(!looks_like_json_object(b""));
    }

    /** 最终回答流式出现时，思考和正文要分开，不能把 token 当成工具过程。 */
    #[test]
    fn stream_ui_progress_keeps_answer_out_of_thinking_without_tools() {
        let mut assistant = StreamedAssistant::default();
        assistant.reasoning = "先组织语言。".to_owned();
        assistant.content = "这是回答。".to_owned();

        assert_eq!(
            stream_ui_progress(&assistant),
            StreamUiProgress {
                thinking: "先组织语言。".to_owned(),
                content: "这是回答。".to_owned(),
            }
        );
    }

    /** 一旦出现 tool_calls，已经流出的正文要改记为思考，终稿位置必须清空。 */
    #[test]
    fn stream_ui_progress_moves_content_to_thinking_when_tools_start() {
        let mut assistant = StreamedAssistant::default();
        assistant.reasoning = "需要检索。".to_owned();
        assistant.content = "我先去搜相关笔记。".to_owned();
        assistant.tool_calls = vec![json!({ "id": "call_1" })];

        assert_eq!(
            stream_ui_progress(&assistant),
            StreamUiProgress {
                thinking: "需要检索。\n\n我先去搜相关笔记。".to_owned(),
                content: String::new(),
            }
        );
    }

    /** provider 忽略 stream 返回完整 JSON 时，仍要能抽出 message 供一次性进度回调。 */
    #[test]
    fn from_completion_reads_blocking_chat_completion() {
        let assistant = StreamedAssistant::from_completion(&json!({
            "choices": [{
                "finish_reason": "stop",
                "message": {
                    "role": "assistant",
                    "content": "整包回答",
                    "reasoning_content": "整包思考"
                }
            }]
        }));

        assert_eq!(assistant.content, "整包回答");
        assert_eq!(assistant.reasoning, "整包思考");
        assert_eq!(assistant.finish_reason.as_deref(), Some("stop"));
    }
}
