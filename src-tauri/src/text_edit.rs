/** 单处文本替换失败原因，调用方据此生成面向用户或模型的错误提示。 */
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UniqueReplacementError {
    EmptyOriginal,
    NotFound,
    Ambiguous { count: usize },
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
}
