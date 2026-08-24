use super::*;

/** 对记忆条目内容做敏感信息脱敏：拦截手机号、身份证、API key/Token、密码明文。
 * 返回脱敏后的内容；命中才会触发调用方记录 info 日志（只记命中条数）。 */
pub fn redact_memory_secrets(content: &str) -> String {
    let credential_redacted = redact_memory_credential_tokens(content);
    redact_memory_identifier_runs(&credential_redacted)
}

/** 单个非空白 token 的凭证脱敏动作。 */
pub(crate) enum MemoryCredentialRedaction {
    RedactToken,
    RedactTokenAndNext,
}

/** 按空白 token 扫描凭证类敏感信息，同时保留原始空白，避免保存后文本结构大幅变化。 */
pub(crate) fn redact_memory_credential_tokens(content: &str) -> String {
    let mut redacted = String::with_capacity(content.len());
    let mut token = String::new();
    let mut redact_next_token = false;

    for character in content.chars() {
        if character.is_whitespace() {
            flush_memory_credential_token(&mut redacted, &mut token, &mut redact_next_token);
            redacted.push(character);
        } else {
            token.push(character);
        }
    }

    flush_memory_credential_token(&mut redacted, &mut token, &mut redact_next_token);
    redacted
}

/** 写入当前 token 的脱敏结果；当上一 token 是 Bearer/password 等标签时会连带替换后续值。 */
pub(crate) fn flush_memory_credential_token(
    redacted: &mut String,
    token: &mut String,
    redact_next_token: &mut bool,
) {
    if token.is_empty() {
        return;
    }

    if *redact_next_token {
        // Authorization: Bearer abc 这类三段式凭证中，Bearer 本身也会继续要求替换下一段值。
        let should_continue = matches!(
            classify_memory_credential_token(token),
            Some(MemoryCredentialRedaction::RedactTokenAndNext)
        );
        redacted.push_str("[已脱敏]");
        *redact_next_token = should_continue;
        token.clear();
        return;
    }

    match classify_memory_credential_token(token) {
        Some(MemoryCredentialRedaction::RedactToken) => redacted.push_str("[已脱敏]"),
        Some(MemoryCredentialRedaction::RedactTokenAndNext) => {
            redacted.push_str("[已脱敏]");
            *redact_next_token = true;
        }
        None => redacted.push_str(token),
    }
    token.clear();
}

/** 识别 API key、Bearer、Authorization、password/密码 等凭证 token。 */
pub(crate) fn classify_memory_credential_token(token: &str) -> Option<MemoryCredentialRedaction> {
    if contains_known_secret_prefix(token) {
        return Some(MemoryCredentialRedaction::RedactToken);
    }

    let Some((key, has_inline_value)) = secret_assignment_state(token) else {
        return None;
    };

    if has_inline_value {
        if key == "authorization" {
            return Some(MemoryCredentialRedaction::RedactTokenAndNext);
        }

        return Some(MemoryCredentialRedaction::RedactToken);
    }

    Some(MemoryCredentialRedaction::RedactTokenAndNext)
}

/** 判断 token 中是否出现常见密钥前缀，支持 `key=sk-...` 和带括号/引号的写法。 */
pub(crate) fn contains_known_secret_prefix(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    [
        "sk-", "sess-", "ghp_", "gho_", "ghs_", "ghu_", "ghr_", "aiza",
    ]
    .iter()
    .any(|prefix| {
        lower.match_indices(prefix).any(|(index, _)| {
            index == 0
                || lower[..index]
                    .chars()
                    .last()
                    .is_some_and(is_secret_prefix_boundary)
        })
    })
}

/** 密钥前缀前允许出现赋值符号、引号或括号，避免误扫普通单词中间片段。 */
pub(crate) fn is_secret_prefix_boundary(character: char) -> bool {
    character.is_whitespace()
        || matches!(
            character,
            '=' | ':' | '：' | '"' | '\'' | '`' | '(' | '[' | '{' | '<' | '（' | '【' | '《'
        )
}

/** 返回凭证赋值 token 的 key 和是否已在同一 token 中携带值。 */
pub(crate) fn secret_assignment_state(token: &str) -> Option<(&'static str, bool)> {
    let trimmed = token.trim_start_matches(is_secret_outer_punctuation);
    let lower = trimmed.to_ascii_lowercase();

    if lower == "bearer" {
        return Some(("bearer", false));
    }

    if let Some(has_value) = assignment_value_state(&lower, "authorization") {
        return Some(("authorization", has_value));
    }

    for key in ["api_key", "apikey", "token", "password"] {
        if let Some(has_value) = assignment_value_state(&lower, key) {
            return Some((key, has_value));
        }
    }

    if let Some(has_value) = assignment_value_state(trimmed, "密码") {
        return Some(("密码", has_value));
    }

    None
}

/** 判断 key 后面是空标签、赋值符但缺值，还是赋值符后已带值。 */
pub(crate) fn assignment_value_state(token: &str, key: &str) -> Option<bool> {
    if token == key {
        return Some(false);
    }

    let rest = token.strip_prefix(key)?;
    let mut rest_chars = rest.chars();
    let separator = rest_chars.next()?;

    if !matches!(separator, '=' | ':' | '：') {
        return None;
    }

    Some(rest_chars.any(|character| !is_secret_outer_punctuation(character)))
}

/** 凭证 token 两侧常见包裹符号，脱敏识别时忽略。 */
pub(crate) fn is_secret_outer_punctuation(character: char) -> bool {
    character.is_ascii_punctuation() || "，。！？；：、（）《》「」【】“”‘’".contains(character)
}

/** 扫描整段文本中的手机号和身份证号片段，避免 `手机号：138...` 因非空白 token 漏脱敏。 */
pub(crate) fn redact_memory_identifier_runs(content: &str) -> String {
    let mut redacted = String::with_capacity(content.len());
    let mut run = String::new();

    for character in content.chars() {
        if character.is_ascii_alphanumeric() {
            run.push(character);
        } else {
            flush_memory_identifier_run(&mut redacted, &mut run);
            redacted.push(character);
        }
    }

    flush_memory_identifier_run(&mut redacted, &mut run);
    redacted
}

/** 写入当前 ASCII 字母数字片段；命中手机号或身份证号时只写占位符。 */
pub(crate) fn flush_memory_identifier_run(redacted: &mut String, run: &mut String) {
    if run.is_empty() {
        return;
    }

    if looks_like_mainland_phone(run) || looks_like_id_card(run) {
        redacted.push_str("[已脱敏]");
    } else {
        redacted.push_str(run);
    }
    run.clear();
}

/** 判断片段是否为中国大陆手机号（1 开头 11 位纯数字）。 */
pub(crate) fn looks_like_mainland_phone(token: &str) -> bool {
    token.len() == 11 && token.starts_with('1') && token.chars().all(|c| c.is_ascii_digit())
}

/** 判断 token 是否为 18 位身份证号：前 17 位为数字，末位为数字或 X。 */
pub(crate) fn looks_like_id_card(token: &str) -> bool {
    if token.len() != 18 {
        return false;
    }
    let mut chars = token.chars();
    let last = chars.next_back();
    let first_seventeen_all_digits = chars.all(|c| c.is_ascii_digit());
    first_seventeen_all_digits
        && matches!(last, Some(c) if c.is_ascii_digit() || c == 'X' || c == 'x')
}

/** 归一化记忆集合：保证 knowledgeBaseId 与传入一致、对每条 content 做脱敏与长度截断、按上限裁剪条目数。 */
pub fn normalize_knowledge_base_memory(
    memory: &mut KnowledgeBaseMemory,
    knowledge_base_id: &str,
) -> usize {
    memory.knowledge_base_id = knowledge_base_id.to_owned();
    let now = format_local_datetime();
    memory.updated_at = now.clone();

    let mut redacted_hits = 0usize;
    for entry in memory.entries.iter_mut() {
        let redacted = redact_memory_secrets(&entry.content);
        if redacted != entry.content {
            redacted_hits += 1;
        }
        entry.category = normalize_memory_category(&entry.category);
        entry.source = normalize_memory_source(&entry.source);
        entry.content = truncate_chars_owned(redacted, MAX_KB_MEMORY_ENTRY_CHARS);
        if entry.updated_at.trim().is_empty() {
            entry.updated_at = now.clone();
        }
    }

    if memory.entries.len() > MAX_KB_MEMORY_ENTRIES {
        memory
            .entries
            .sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        memory.entries.truncate(MAX_KB_MEMORY_ENTRIES);
    }

    redacted_hits
}

/** 归一化跨会话记忆分类，未知值降级为 other，避免污染 prompt 与设置页统计。 */
pub(crate) fn normalize_memory_category(category: &str) -> String {
    match category {
        MEMORY_CATEGORY_NOTE_STRUCTURE
        | MEMORY_CATEGORY_TAG_CONVENTION
        | MEMORY_CATEGORY_ORGANIZATION
        | MEMORY_CATEGORY_CONVENTION
        | MEMORY_CATEGORY_OTHER => category.to_owned(),
        _ => MEMORY_CATEGORY_OTHER.to_owned(),
    }
}

/** 归一化跨会话记忆来源，当前只允许 user/auto，未知值按用户录入处理。 */
pub(crate) fn normalize_memory_source(source: &str) -> String {
    match source {
        MEMORY_SOURCE_USER | MEMORY_SOURCE_AUTO => source.to_owned(),
        _ => MEMORY_SOURCE_USER.to_owned(),
    }
}

/** 把字符串截断到不超过 max_chars 个 Unicode 字符，避免用户写入过长偏好挤占上下文。 */
pub(crate) fn truncate_chars_owned(value: String, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value;
    }
    value.chars().take(max_chars).collect()
}

/** 保存单个知识库的跨会话记忆；写入前归一化并脱敏，返回归一化后的 memory。 */
pub fn save_knowledge_base_memory(
    app: &AppHandle,
    knowledge_base_id: &str,
    mut memory: KnowledgeBaseMemory,
) -> Result<KnowledgeBaseMemory, String> {
    let redacted_hits = normalize_knowledge_base_memory(&mut memory, knowledge_base_id);
    if redacted_hits > 0 {
        log::info!(
            target: "agent_memory",
            "保存跨会话记忆时命中脱敏规则：knowledge_base_id_chars={} redacted_entries={}",
            knowledge_base_id.chars().count(),
            redacted_hits
        );
    }

    let payload_json =
        serde_json::to_string(&memory).map_err(|error| format!("无法序列化跨会话记忆：{error}"))?;

    let connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    connection
        .execute(
            "INSERT OR REPLACE INTO knowledge_base_memories (knowledge_base_id, payload_json, updated_at) VALUES (?1, ?2, ?3)",
            params![knowledge_base_id, payload_json, memory.updated_at],
        )
        .map_err(|error| format!("无法保存跨会话记忆：{error}"))?;

    Ok(memory)
}

/** 读取单个知识库的跨会话记忆；不存在时返回 None。 */
pub fn load_knowledge_base_memory(
    app: &AppHandle,
    knowledge_base_id: &str,
) -> Result<Option<KnowledgeBaseMemory>, String> {
    let connection = open_database(app)?;
    let payload_json_result = connection.query_row(
        "SELECT payload_json FROM knowledge_base_memories WHERE knowledge_base_id = ?1",
        params![knowledge_base_id],
        |row| row.get::<_, String>(0),
    );

    match payload_json_result {
        Ok(payload_json) => {
            let memory: KnowledgeBaseMemory = serde_json::from_str(&payload_json)
                .map_err(|error| format!("无法解析跨会话记忆：{error}"))?;
            Ok(Some(memory))
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(format!("无法读取跨会话记忆：{error}")),
    }
}

/** 读取全部知识库的跨会话记忆，供设置页列表展示。 */
pub fn load_knowledge_base_memories(app: &AppHandle) -> Result<Vec<KnowledgeBaseMemory>, String> {
    let connection = open_database(app)?;
    let mut statement = connection
        .prepare("SELECT payload_json FROM knowledge_base_memories ORDER BY updated_at DESC")
        .map_err(|error| format!("无法读取跨会话记忆列表：{error}"))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("无法读取跨会话记忆列表：{error}"))?;

    let mut memories = Vec::new();
    for row in rows {
        let payload_json = row.map_err(|error| format!("无法读取跨会话记忆行：{error}"))?;
        let memory: KnowledgeBaseMemory = serde_json::from_str(&payload_json)
            .map_err(|error| format!("无法解析跨会话记忆：{error}"))?;
        memories.push(memory);
    }
    Ok(memories)
}

/** 删除单个知识库的跨会话记忆。 */
pub fn delete_knowledge_base_memory(
    app: &AppHandle,
    knowledge_base_id: &str,
) -> Result<(), String> {
    let connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    connection
        .execute(
            "DELETE FROM knowledge_base_memories WHERE knowledge_base_id = ?1",
            params![knowledge_base_id],
        )
        .map_err(|error| format!("无法删除跨会话记忆：{error}"))?;
    Ok(())
}
