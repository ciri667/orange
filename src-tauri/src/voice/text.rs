/** SenseVoice 会在正文前插入语言、情绪和事件标记，插入输入框前要去掉。 */
pub fn normalize_transcript(text: &str) -> String {
    let stripped = strip_sensevoice_tags(text);
    collapse_cjk_spaces(stripped.trim())
}

/** 去掉 `<|...|>` 标记。不成对的开头标记按普通文本保留。 */
fn strip_sensevoice_tags(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<|") {
        cleaned.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        if let Some(end) = after_open.find("|>") {
            rest = &after_open[end + 2..];
        } else {
            cleaned.push_str(&rest[start..]);
            rest = "";
        }
    }
    cleaned.push_str(rest);
    cleaned
}

/** 只删掉夹在两个汉字之间的空白，英文单词之间的空格保留。 */
fn collapse_cjk_spaces(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut cleaned = String::with_capacity(text.len());
    for (index, character) in chars.iter().copied().enumerate() {
        if character.is_whitespace() {
            let previous = chars[..index]
                .iter()
                .rev()
                .find(|item| !item.is_whitespace());
            let next = chars[index + 1..].iter().find(|item| !item.is_whitespace());
            if matches!((previous, next), (Some(left), Some(right)) if is_cjk(*left) && is_cjk(*right))
            {
                continue;
            }
        }
        cleaned.push(character);
    }
    cleaned
}

/** 常用中日韩统一表意文字区，够用来判断听写结果里的汉字间距。 */
fn is_cjk(character: char) -> bool {
    matches!(character, '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_sensevoice_markup_and_cjk_spaces() {
        let raw = "<|zh|><|NEUTRAL|><|Speech|><|withitn|>你 好，世界";
        assert_eq!(normalize_transcript(raw), "你好，世界");
    }

    #[test]
    fn keeps_spaces_between_english_words() {
        assert_eq!(normalize_transcript("hello world"), "hello world");
        assert_eq!(normalize_transcript("你好 world"), "你好 world");
    }
}
