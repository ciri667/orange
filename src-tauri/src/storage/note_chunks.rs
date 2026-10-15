use jieba_rs::Jieba;
use std::sync::OnceLock;

/** Markdown 检索 chunk，行号为 1-based 闭区间，便于引用直接定位到编辑器。 */
#[derive(Clone, Debug)]
pub(crate) struct NoteChunk {
    pub(crate) chunk_index: usize,
    pub(crate) heading_path: String,
    pub(crate) content: String,
    pub(crate) line_start: usize,
    pub(crate) line_end: usize,
}

/** 单个 Markdown chunk 的最大字符数；超过后按行继续拆分，但共享同一标题路径。 */
const MAX_CHUNK_CHARS: usize = 1_200;

static JIEBA: OnceLock<Jieba> = OnceLock::new();

/** 把 Markdown 按标题层级切成可检索 chunk；空笔记生成一个元数据 chunk，保证 tags 仍可召回。 */
pub(crate) fn chunk_markdown(content: &str, tags: &[String]) -> Vec<NoteChunk> {
    let lines: Vec<&str> = content.split('\n').collect();
    let fence_mask = code_fence_mask(&lines);
    let body_start = frontmatter_body_start_line(&lines);
    let mut chunks = Vec::new();
    let mut heading_stack: Vec<(usize, String)> = Vec::new();
    let mut current_heading_path = String::new();
    let mut current_start_line: Option<usize> = None;
    let mut current_lines: Vec<String> = Vec::new();

    for line_index in body_start..lines.len() {
        let raw_line = lines[line_index].trim_end_matches('\r');
        let trimmed_line = raw_line.trim();

        if !fence_mask[line_index] && is_atx_heading(trimmed_line) {
            flush_chunk(
                &mut chunks,
                current_start_line.take(),
                current_lines.drain(..).collect(),
                &current_heading_path,
            );

            let level = trimmed_line.chars().take_while(|ch| *ch == '#').count();
            let title = trimmed_line[level..].trim().to_owned();
            heading_stack.retain(|(existing_level, _)| *existing_level < level);
            heading_stack.push((level, title));
            current_heading_path = heading_stack
                .iter()
                .map(|(_, title)| title.as_str())
                .collect::<Vec<_>>()
                .join(" / ");
            current_start_line = Some(line_index + 1);
            current_lines.push(raw_line.to_owned());
            continue;
        }

        if current_start_line.is_none() {
            current_start_line = Some(line_index + 1);
            current_heading_path = heading_stack
                .iter()
                .map(|(_, title)| title.as_str())
                .collect::<Vec<_>>()
                .join(" / ");
        }

        let current_chars = current_lines
            .iter()
            .map(|line| line.chars().count())
            .sum::<usize>();
        let line_chars = raw_line.chars().count();
        if current_chars > 0 && current_chars + line_chars > MAX_CHUNK_CHARS {
            flush_chunk(
                &mut chunks,
                current_start_line.take(),
                current_lines.drain(..).collect(),
                &current_heading_path,
            );
        }

        // 超长单行也要拆成多个 chunk，否则一个没有换行的段落仍会超过上下文预算。
        if line_chars >= MAX_CHUNK_CHARS {
            let characters: Vec<char> = raw_line.chars().collect();
            for start in (0..characters.len()).step_by(MAX_CHUNK_CHARS) {
                let end = (start + MAX_CHUNK_CHARS).min(characters.len());
                chunks.push(NoteChunk {
                    chunk_index: chunks.len(),
                    heading_path: if current_heading_path.is_empty() {
                        "开头".to_owned()
                    } else {
                        current_heading_path.clone()
                    },
                    content: characters[start..end].iter().collect(),
                    line_start: line_index + 1,
                    line_end: line_index + 1,
                });
            }
            continue;
        }

        current_lines.push(raw_line.to_owned());
    }

    flush_chunk(
        &mut chunks,
        current_start_line.take(),
        current_lines,
        &current_heading_path,
    );

    if chunks.is_empty() {
        chunks.push(NoteChunk {
            chunk_index: 0,
            heading_path: "元数据".to_owned(),
            content: tags.join(" "),
            line_start: 1,
            line_end: 1,
        });
    }

    chunks
}

/** 把当前行集合落成一个 chunk；全空内容不产生空 chunk。 */
fn flush_chunk(
    chunks: &mut Vec<NoteChunk>,
    start_line: Option<usize>,
    lines: Vec<String>,
    heading_path: &str,
) {
    let Some(start_line) = start_line else {
        return;
    };
    let mut lines = lines;
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    let content = lines.join("\n");
    if content.trim().is_empty() {
        return;
    }

    let line_end = start_line + lines.len().saturating_sub(1);
    chunks.push(NoteChunk {
        chunk_index: chunks.len(),
        heading_path: if heading_path.is_empty() {
            "开头".to_owned()
        } else {
            heading_path.to_owned()
        },
        content,
        line_start: start_line,
        line_end,
    });
}

/** 用 jieba 生成 FTS/query token；只保留含字母、数字或 CJK 的词，避免标点进入 MATCH 语法。 */
pub(crate) fn tokenize_text(text: &str) -> Vec<String> {
    let jieba = JIEBA.get_or_init(Jieba::new);
    jieba
        .cut(text, true)
        .into_iter()
        .map(|token| token.word.trim())
        .filter(|token| !token.is_empty())
        .filter(|token| token.chars().any(|ch| ch.is_alphanumeric()))
        .map(|token| token.to_lowercase())
        .collect()
}

/** 生成 FTS5 索引正文；token 用空白连接，交由查询侧统一加引号。 */
pub(crate) fn build_fts_tokens(text: &str) -> String {
    tokenize_text(text).join(" ")
}

/** 标记每行是否位于 fenced code 内，围栏行本身也视为代码。 */
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

/** 有闭合 frontmatter 时返回结束标记后的正文起始行。 */
fn frontmatter_body_start_line(lines: &[&str]) -> usize {
    if lines
        .first()
        .is_none_or(|line| line.trim_end_matches('\r').trim() != "---")
    {
        return 0;
    }

    for index in 1..lines.len() {
        if lines[index].trim_end_matches('\r').trim() == "---" {
            return index + 1;
        }
    }

    0
}

/** CommonMark ATX 标题需要 `#` 后接空白；`#标签` 不是标题。 */
fn is_atx_heading(trimmed: &str) -> bool {
    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    if !(1..=6).contains(&hashes) {
        return false;
    }

    let rest = &trimmed[hashes..];
    rest.is_empty() || rest.starts_with(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_markdown_by_heading_path_and_line_range() {
        let content = "# 方案\n\n第一段\n\n## 数据同步\n\n冲突处理细节\n";
        let chunks = chunk_markdown(content, &[]);

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].heading_path, "方案");
        assert_eq!(chunks[0].line_start, 1);
        assert_eq!(chunks[0].line_end, 3);
        assert_eq!(chunks[1].heading_path, "方案 / 数据同步");
        assert_eq!(chunks[1].line_start, 5);
        assert_eq!(chunks[1].line_end, 7);
        assert!(chunks[1].content.contains("冲突处理细节"));
    }

    #[test]
    fn ignores_hash_symbols_inside_code_fence() {
        let content = "# 标题\n\n```markdown\n# 不是标题\n```\n\n正文\n";
        let chunks = chunk_markdown(content, &[]);

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].heading_path, "标题");
        assert!(chunks[0].content.contains("# 不是标题"));
    }

    #[test]
    fn tokenizes_chinese_phrases_and_filters_punctuation() {
        let tokens = tokenize_text("本地知识库，Rust 2.0！");

        assert!(tokens.contains(&"知识库".to_owned()));
        assert!(tokens.contains(&"rust".to_owned()));
        assert!(!tokens.iter().any(|token| token == "，"));
    }

    #[test]
    fn splits_overlong_paragraph_while_sharing_heading_path() {
        let body = "长".repeat(2_500);
        let content = format!("# 长文\n\n{body}");
        let chunks = chunk_markdown(&content, &[]);

        assert!(chunks.len() >= 2);
        assert!(chunks.iter().all(|chunk| chunk.heading_path == "长文"));
        assert!(chunks
            .iter()
            .all(|chunk| chunk.content.chars().count() <= 1_200));
    }

    #[test]
    fn skips_frontmatter_from_body_chunks() {
        let content = "---\ntitle: 首页\n---\n\n# 首页\n\n正文\n";
        let chunks = chunk_markdown(content, &["索引".to_owned()]);

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].heading_path, "首页");
        assert!(!chunks[0].content.contains("title:"));
    }
}
