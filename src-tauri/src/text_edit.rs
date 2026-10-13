/** 单处文本替换失败原因，调用方据此生成面向用户或模型的错误提示。 */
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UniqueReplacementError {
    EmptyOriginal,
    NotFound,
    Ambiguous { count: usize },
}

/** 指定第 N 次命中替换失败原因，用于重复片段去重等受控场景。 */
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OccurrenceReplacementError {
    EmptyOriginal,
    OccurrenceOutOfRange { requested: usize, count: usize },
}

/** UTF-8 BOM 字节序列，用于在匹配前剥离、写回时恢复。 */
pub const UTF8_BOM: &str = "\u{feff}";

/** 文本文件的物理形态：是否带 BOM、是否使用 CRLF。用于匹配前归一、写回时还原。 */
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextFileShape {
    pub has_bom: bool,
    pub uses_crlf: bool,
}

impl TextFileShape {
    /** 从磁盘原文探测文件形态；BOM 在前，换行风格按首个出现判定。 */
    pub fn detect(raw: &str) -> Self {
        let has_bom = raw.starts_with(UTF8_BOM);
        let body = raw.strip_prefix(UTF8_BOM).unwrap_or(raw);
        // 只需判定"是否 CRLF"，首个 \r\n 的位置即可代表全文件的换行风格。
        let uses_crlf = body.contains("\r\n");

        Self { has_bom, uses_crlf }
    }

    /** 归一化正文供匹配使用：剥离 BOM 并把 CRLF 折成 LF，让模型只需给 LF。 */
    pub fn normalize(self, raw: &str) -> String {
        let body = raw.strip_prefix(UTF8_BOM).unwrap_or(raw);

        if self.uses_crlf {
            body.replace("\r\n", "\n")
        } else {
            body.to_owned()
        }
    }

    /** 把归一化后的正文还原成文件原有形态：先恢复 CRLF，再补回 BOM。 */
    pub fn restore(self, normalized: &str) -> String {
        let body = if self.uses_crlf {
            normalized.replace('\n', "\r\n")
        } else {
            normalized.to_owned()
        };

        if self.has_bom {
            format!("{UTF8_BOM}{body}")
        } else {
            body
        }
    }
}

/** 统计 needle 在 haystack 中的非重叠命中次数，用于判断改写片段是否唯一。 */
pub fn count_non_overlapping_matches(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }

    haystack.match_indices(needle).count()
}

/** 确认 original 在 content 中恰好命中一次，并只替换这一处。 */
pub fn replace_unique(
    content: &str,
    original: &str,
    next: &str,
) -> Result<String, UniqueReplacementError> {
    if original.is_empty() {
        return Err(UniqueReplacementError::EmptyOriginal);
    }

    let match_count = count_non_overlapping_matches(content, original);

    match match_count {
        0 => Err(UniqueReplacementError::NotFound),
        1 => Ok(content.replacen(original, next, 1)),
        count => Err(UniqueReplacementError::Ambiguous { count }),
    }
}

/** 替换 original 的第 occurrence 次非重叠命中，occurrence 使用 1-based 计数。 */
pub fn replace_occurrence(
    content: &str,
    original: &str,
    next: &str,
    occurrence: usize,
) -> Result<String, OccurrenceReplacementError> {
    if original.is_empty() {
        return Err(OccurrenceReplacementError::EmptyOriginal);
    }

    let matches = content.match_indices(original).collect::<Vec<_>>();
    let requested = occurrence.max(1);
    let Some((match_start, _)) = matches.get(requested - 1).copied() else {
        return Err(OccurrenceReplacementError::OccurrenceOutOfRange {
            requested,
            count: matches.len(),
        });
    };
    let mut output = String::with_capacity(content.len() - original.len() + next.len());

    output.push_str(&content[..match_start]);
    output.push_str(next);
    output.push_str(&content[match_start + original.len()..]);

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    /** 命中一次时只替换目标片段，保留其他正文不变。 */
    #[test]
    fn replace_unique_replaces_single_match() {
        let result = replace_unique("before old after", "old", "new").unwrap();

        assert_eq!(result, "before new after");
    }

    /** 重复片段会被视为模糊定位，避免一次确认误改多处。 */
    #[test]
    fn replace_unique_rejects_ambiguous_matches() {
        let result = replace_unique("old and old", "old", "new");

        assert_eq!(result, Err(UniqueReplacementError::Ambiguous { count: 2 }));
    }

    /** 指定 occurrence 时允许精准替换重复片段中的某一次。 */
    #[test]
    fn replace_occurrence_replaces_requested_match() {
        let result = replace_occurrence("old and old and old", "old", "new", 2).unwrap();

        assert_eq!(result, "old and new and old");
    }

    /** occurrence 超出实际命中次数时必须拒绝。 */
    #[test]
    fn replace_occurrence_rejects_out_of_range() {
        let result = replace_occurrence("old and old", "old", "new", 3);

        assert_eq!(
            result,
            Err(OccurrenceReplacementError::OccurrenceOutOfRange {
                requested: 3,
                count: 2
            })
        );
    }

    /** CRLF 文件归一后可用 LF 片段匹配，还原时回到 CRLF。 */
    #[test]
    fn text_file_shape_round_trips_crlf() {
        let raw = "第一行\r\n第二行\r\n";
        let shape = TextFileShape::detect(raw);

        assert_eq!(
            shape,
            TextFileShape {
                has_bom: false,
                uses_crlf: true
            }
        );
        let normalized = shape.normalize(raw);
        assert_eq!(normalized, "第一行\n第二行\n");
        assert_eq!(shape.restore(&normalized), raw);
    }

    /** 带 BOM 的文件归一后剥离 BOM，还原时补回。 */
    #[test]
    fn text_file_shape_round_trips_bom() {
        let raw = "\u{feff}# 标题\n正文\n";
        let shape = TextFileShape::detect(raw);

        assert_eq!(
            shape,
            TextFileShape {
                has_bom: true,
                uses_crlf: false
            }
        );
        let normalized = shape.normalize(raw);
        assert_eq!(normalized, "# 标题\n正文\n");
        assert_eq!(shape.restore(&normalized), raw);
    }

    /** BOM 与 CRLF 同时存在时，两种形态都要正确往返。 */
    #[test]
    fn text_file_shape_round_trips_bom_and_crlf() {
        let raw = "\u{feff}第一行\r\n第二行";
        let shape = TextFileShape::detect(raw);
        let normalized = shape.normalize(raw);

        assert_eq!(
            shape,
            TextFileShape {
                has_bom: true,
                uses_crlf: true
            }
        );
        assert_eq!(normalized, "第一行\n第二行");
        assert_eq!(shape.restore(&normalized), raw);
    }

    /** 纯 LF 且无 BOM 的文件保持原样，避免无谓改写换行风格。 */
    #[test]
    fn text_file_shape_leaves_lf_untouched() {
        let raw = "第一行\n第二行\n";
        let shape = TextFileShape::detect(raw);

        assert_eq!(
            shape,
            TextFileShape {
                has_bom: false,
                uses_crlf: false
            }
        );
        assert_eq!(shape.normalize(raw), raw);
        assert_eq!(shape.restore(raw), raw);
    }
}
