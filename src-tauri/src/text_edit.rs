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
}
