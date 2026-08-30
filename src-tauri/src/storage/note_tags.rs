//! Markdown 笔记标签。
//!
//! 标签只来自文件正文，避免信息栏里的数量和正文脱节：
//! 1. 文首 YAML frontmatter 的 `tags` 字段；
//! 2. 不在代码围栏内、整行只由 `#标签` 组成的行（常见写法是文末一行）。
//!
//! 标题 `# 标题`、段落里的 `#词` 和代码注释都不会当成标签。

/// 单个标签最长字符数，避免把整段误粘贴进标签。
const MAX_TAG_CHARS: usize = 40;

/// 从 Markdown 正文提取去重后的标签，按 UTF-8 字节序排列。
pub fn extract_tags(content: &str) -> Vec<String> {
    let mut tags = parse_frontmatter_tags(content);
    tags.extend(parse_hashtag_line_tags(content));
    normalize_tags(tags)
}

/// 去掉空值、非法字符和重复项。
fn normalize_tags(tags: Vec<String>) -> Vec<String> {
    let mut normalized: Vec<String> = tags
        .into_iter()
        .filter_map(|tag| normalize_tag_name(&tag))
        .collect();
    normalized.sort();
    normalized.dedup();
    normalized
}

/// 把用户输入或 YAML 标量收成合法标签名；失败则丢弃。
fn normalize_tag_name(raw: &str) -> Option<String> {
    let mut name = raw.trim();
    if let Some(stripped) = name.strip_prefix('#') {
        if !stripped.starts_with('#') {
            name = stripped.trim();
        }
    }

    let name = name
        .trim_matches(|ch: char| {
            matches!(
                ch,
                ',' | '.'
                    | ';'
                    | ':'
                    | '!'
                    | '?'
                    | '，'
                    | '。'
                    | '；'
                    | '：'
                    | '！'
                    | '？'
                    | '"'
                    | '\''
                    | '“'
                    | '”'
            )
        })
        .trim();

    if name.is_empty() || name.chars().count() > MAX_TAG_CHARS {
        return None;
    }

    if name
        .chars()
        .any(|ch| ch.is_whitespace() || matches!(ch, '#' | '[' | ']' | '{' | '}' | ',' | ':'))
    {
        return None;
    }

    Some(name.to_owned())
}

/// 读取文首 YAML frontmatter 中的 tags 字段。
fn parse_frontmatter_tags(content: &str) -> Vec<String> {
    split_frontmatter(content)
        .map(|(yaml, _body)| parse_frontmatter_tags_from_yaml(&yaml))
        .unwrap_or_default()
}

/// 拆出闭合的 YAML frontmatter；未闭合或没有起始 `---` 时视为没有。
fn split_frontmatter(content: &str) -> Option<(String, String)> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let lines: Vec<&str> = content.split('\n').collect();
    if lines.first()?.trim_end_matches('\r').trim() != "---" {
        return None;
    }

    for (index, line) in lines.iter().enumerate().skip(1) {
        if line.trim_end_matches('\r').trim() == "---" {
            return Some((lines[1..index].join("\n"), lines[index + 1..].join("\n")));
        }
    }

    None
}

/// 解析 frontmatter 里的 `tags`，兼容数组、逗号列表和多行 `- item`。
fn parse_frontmatter_tags_from_yaml(yaml: &str) -> Vec<String> {
    let lines: Vec<&str> = yaml.split('\n').collect();
    let mut tags = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index].trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("tags:") {
            let rest = rest.trim();
            if rest.is_empty() {
                index += 1;
                while index < lines.len() {
                    let item_line = lines[index].trim_end_matches('\r');
                    if let Some(item) = parse_yaml_list_item(item_line) {
                        tags.push(item);
                        index += 1;
                    } else if item_line.trim().is_empty() {
                        index += 1;
                    } else {
                        break;
                    }
                }
            } else {
                tags.extend(parse_inline_tag_list(rest));
                index += 1;
            }
        } else {
            index += 1;
        }
    }

    tags
}

/// 读取缩进后的 YAML 列表项 `- value`。
fn parse_yaml_list_item(line: &str) -> Option<String> {
    let trimmed_end = line.trim_end();
    if !trimmed_end.starts_with(' ') && !trimmed_end.starts_with('\t') {
        return None;
    }

    let rest = trimmed_end.trim();
    let value = rest.strip_prefix("- ")?;
    Some(unquote_yaml_scalar(value.trim()))
}

/// 解析 `tags: a, b` 或 `tags: [a, b]`。
fn parse_inline_tag_list(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    let inner = if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() >= 2 {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };

    inner
        .split(',')
        .map(|part| unquote_yaml_scalar(part.trim()))
        .filter(|part| !part.is_empty())
        .collect()
}

/// 去掉 YAML 标量两端的引号，保留标签原文。
fn unquote_yaml_scalar(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        return trimmed[1..trimmed.len() - 1].to_owned();
    }

    trimmed.to_owned()
}

/// 收集正文中所有独立 `#标签` 行，跳过 frontmatter 和代码围栏。
fn parse_hashtag_line_tags(content: &str) -> Vec<String> {
    let lines: Vec<&str> = content.split('\n').collect();
    let in_fence = code_fence_mask(&lines);
    let start = frontmatter_body_start_line(&lines);
    let mut tags = Vec::new();

    for (index, line) in lines.iter().enumerate().skip(start) {
        if in_fence[index] {
            continue;
        }

        if let Some(line_tags) = parse_hashtag_only_line(line) {
            tags.extend(line_tags);
        }
    }

    tags
}

/// 正文起始行：有闭合 frontmatter 时从结束标记之后开始。
fn frontmatter_body_start_line(lines: &[&str]) -> usize {
    if lines
        .first()
        .is_none_or(|line| line.trim_end_matches('\r').trim() != "---")
    {
        return 0;
    }

    for (index, line) in lines.iter().enumerate().skip(1) {
        if line.trim_end_matches('\r').trim() == "---" {
            return index + 1;
        }
    }

    0
}

/// 标记每一行是否位于 fenced code 内，围栏行本身也视为代码。
fn code_fence_mask(lines: &[&str]) -> Vec<bool> {
    let mut in_fence = false;
    let mut mask = Vec::with_capacity(lines.len());

    for line in lines {
        let trimmed = line.trim_end_matches('\r').trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            mask.push(true);
            in_fence = !in_fence;
            continue;
        }

        mask.push(in_fence);
    }

    mask
}

/// 整行都是 `#标签` token 时返回这些标签，否则视为普通正文。
fn parse_hashtag_only_line(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim_end_matches('\r').trim();
    if trimmed.is_empty() || is_atx_heading(trimmed) {
        return None;
    }

    let mut tags = Vec::new();
    for token in trimmed.split_whitespace() {
        tags.push(hashtag_token_name(token)?);
    }

    if tags.is_empty() {
        None
    } else {
        Some(tags)
    }
}

/// CommonMark ATX 标题需要 `#` 后接空白；`#标签` 不是标题。
fn is_atx_heading(trimmed: &str) -> bool {
    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    if !(1..=6).contains(&hashes) {
        return false;
    }

    let rest = &trimmed[hashes..];
    rest.is_empty() || rest.starts_with(char::is_whitespace)
}

/// 解析单个 `#标签` token，拒绝 `##标题` 这种连续井号。
fn hashtag_token_name(token: &str) -> Option<String> {
    let rest = token.strip_prefix('#')?;
    if rest.starts_with('#') {
        return None;
    }

    normalize_tag_name(rest)
}

#[cfg(test)]
mod tests {
    use super::extract_tags;

    #[test]
    fn extracts_trailing_hashtags() {
        let content = "# 标题\n\n正文内容。\n\n#产品 #MVP #Agent\n";
        assert_eq!(extract_tags(content), vec!["Agent", "MVP", "产品"]);
    }

    #[test]
    fn extracts_frontmatter_list_and_inline_tags() {
        let list = "---\ntags:\n  - 研究\n  - 检索\n---\n\n# 标题\n";
        assert_eq!(extract_tags(list), vec!["检索", "研究"]);

        let inline = "---\ntags: [隐私, 架构]\n---\n\n正文\n";
        assert_eq!(extract_tags(inline), vec!["架构", "隐私"]);

        let csv = "---\ntags: 会议, 原型\n---\n\n正文\n";
        assert_eq!(extract_tags(csv), vec!["会议", "原型"]);
    }

    #[test]
    fn ignores_headings_paragraph_hashtags_and_code_fences() {
        let content =
            "# 标题\n\n段落里的 #看起来像标签 不是标签。\n\n```bash\n#todo\n```\n\n## 二级标题\n";
        assert!(extract_tags(content).is_empty());
    }

    #[test]
    fn removing_trailing_hashtags_clears_tags() {
        let with_tags = "# 标题\n\n正文\n\n#产品 #MVP\n";
        let without_tags = "# 标题\n\n正文\n";
        assert_eq!(extract_tags(with_tags), vec!["MVP", "产品"]);
        assert!(extract_tags(without_tags).is_empty());
    }

    #[test]
    fn unions_frontmatter_and_hashtag_lines() {
        let content = "---\ntags: [产品]\n---\n\n# 标题\n\n#会议\n";
        assert_eq!(extract_tags(content), vec!["产品", "会议"]);
    }

    #[test]
    fn hashtag_line_after_title_counts() {
        let content = "# 标题\n\n#产品 #MVP\n\n正文从这里开始。\n";
        assert_eq!(extract_tags(content), vec!["MVP", "产品"]);
    }
}
