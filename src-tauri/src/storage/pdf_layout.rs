//! 基于字形坐标的 PDF 阅读顺序重建。
//!
//! `pdf-extract` 只按内容流顺序输出文本；双栏或乱序内容流会出现「左右交错」。
//! 这里用 hayro 解释页面，收集每个字形的页面坐标，再按行分组、识别双栏，
//! 输出符合阅读顺序的文本。解析失败时调用方应回退到 `pdf-extract` 的结果。

use hayro::hayro_interpret::font::GlyphRun;
use hayro::hayro_interpret::{
    interpret_page, BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image,
    ImageDrawProps, InterpreterCache, InterpreterSettings, SoftMask,
};
use hayro::kurbo::{Affine, BezPath, Point, Rect};

/// 单个字形的页面坐标（PDF 坐标系，原点在左下）。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PositionedText {
    pub x: f64,
    pub y: f64,
    /// 字形推进宽度；同一次绘制的下一个字形 x 减去当前 x。
    pub width: f64,
    /// 估算字号，用于行分组容差和空格判定。
    pub size: f64,
    pub text: String,
}

/// 收集字形坐标的解释设备；只关心文本，绘制指令全部忽略。
#[derive(Default)]
struct TextCollector {
    items: Vec<PositionedText>,
}

impl<'a> Device<'a> for TextCollector {
    fn draw_path(&mut self, _path: &BezPath, _props: DrawProps<'a>, _draw_mode: &DrawMode) {}

    fn push_clip_path(&mut self, _clip_path: &ClipPath) {}

    fn pop_clip(&mut self) {}

    fn push_transparency_group(
        &mut self,
        _opacity: f32,
        _mask: Option<SoftMask<'a>>,
        _blend_mode: BlendMode,
    ) {
    }

    fn pop_transparency_group(&mut self) {}

    fn draw_image(&mut self, _image: Image<'a, '_>, _props: ImageDrawProps<'a>) {}

    fn draw_glyph_run(
        &mut self,
        glyph_run: &GlyphRun<'_, 'a>,
        props: DrawProps<'a>,
        _draw_mode: &DrawMode,
    ) {
        // 同一 run 内，字形推进宽度用下一个字形的 x 近似，比按字号估算更准。
        let mut run_items: Vec<PositionedText> = Vec::new();
        for glyph in glyph_run.glyphs() {
            let Some(unicode) = glyph.as_unicode() else {
                continue;
            };
            let text = match unicode {
                hayro::hayro_interpret::hayro_cmap::BfString::Char(value) => value.to_string(),
                hayro::hayro_interpret::hayro_cmap::BfString::String(value) => value,
            };
            if text.is_empty() {
                continue;
            }
            // 字形矩阵叠加绘制矩阵，得到页面坐标与近似字号。
            let matrix = props.transform * glyph.transform();
            let origin = matrix * Point::new(0.0, 0.0);
            let coeffs = matrix.as_coeffs();
            let size = (coeffs[0].hypot(coeffs[1]) + coeffs[2].hypot(coeffs[3])) / 2.0;
            run_items.push(PositionedText {
                x: origin.x,
                y: origin.y,
                width: 0.0,
                size: size.abs().max(1.0),
                text,
            });
        }
        for index in 0..run_items.len() {
            let fallback = run_items[index].size * 0.5;
            run_items[index].width = run_items
                .get(index + 1)
                .map(|next| next.x - run_items[index].x)
                .filter(|advance| *advance > 0.0)
                .unwrap_or(fallback);
        }
        self.items.extend(run_items);
    }
}

/// 单页的布局抽取结果。
#[derive(Clone, Debug)]
pub(crate) struct LayoutPageText {
    pub page: usize,
    pub text: String,
}

/**
 * 用 hayro 解释整篇 PDF，返回每页按阅读顺序重建的文本。
 *
 * 一页内没有可映射 Unicode 的字形时该页文本为空，调用方需要自行判断是否回退。
 */
pub(crate) fn extract_pdf_layout_pages(bytes: Vec<u8>) -> Result<Vec<LayoutPageText>, String> {
    let pdf = hayro::hayro_syntax::Pdf::new(bytes)
        .map_err(|error| format!("PDF 无法解析字形坐标：{error:?}"))?;
    let pages = pdf.pages();
    let cache = InterpreterCache::new();
    let settings = InterpreterSettings::default();
    let mut result = Vec::with_capacity(pages.len());

    for (index, page) in pages.iter().enumerate() {
        let (width, height) = page.render_dimensions();
        let mut context = Context::new(
            Affine::IDENTITY,
            Rect::new(0.0, 0.0, width as f64, height as f64),
            &cache,
            page.xref(),
            settings.clone(),
        );
        let mut collector = TextCollector::default();
        interpret_page(page, &mut context, &mut collector);
        result.push(LayoutPageText {
            page: index + 1,
            text: assemble_reading_order(&collector.items, width as f64),
        });
    }

    Ok(result)
}

/// 一行文本：纵向中心、左右边界和拼接后的内容。
#[derive(Clone, Debug)]
struct LayoutLine {
    y: f64,
    min_x: f64,
    max_x: f64,
    text: String,
}

/**
 * 把字形按阅读顺序拼成文本。
 *
 * 步骤：先按 y 分行，再判断是否为双栏；双栏时先输出左栏整列，再输出右栏。
 */
pub(crate) fn assemble_reading_order(items: &[PositionedText], page_width: f64) -> String {
    if items.is_empty() {
        return String::new();
    }
    let mut sorted = items.to_vec();
    // PDF 的 y 轴向上，阅读顺序是从大 y 到小 y。
    sorted.sort_by(|left, right| {
        right
            .y
            .total_cmp(&left.y)
            .then(left.x.total_cmp(&right.x))
    });

    let median_size = median(sorted.iter().map(|item| item.size).collect());
    let tolerance = (median_size * 0.6).max(1.0);
    let mut lines: Vec<LayoutLine> = Vec::new();
    for item in &sorted {
        let merge_target = lines
            .last_mut()
            .filter(|line| (line.y - item.y).abs() <= tolerance);
        if let Some(line) = merge_target {
            let cursor = line.max_x;
            line.min_x = line.min_x.min(item.x);
            push_with_spacing(&mut line.text, &cursor, item);
            line.max_x = line.max_x.max(item.x + item.width);
            continue;
        }
        let mut text = String::new();
        let item_width = item.width;
        push_with_spacing(&mut text, &item.x, item);
        lines.push(LayoutLine {
            y: item.y,
            min_x: item.x,
            max_x: item.x + item_width,
            text,
        });
    }

    if let Some((left, right)) = split_columns(&lines, page_width) {
        return format!(
            "{}\n{}",
            left.iter().map(|line| line.text.as_str()).collect::<Vec<_>>().join("\n"),
            right.iter().map(|line| line.text.as_str()).collect::<Vec<_>>().join("\n")
        );
    }

    lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/**
 * 判断分栏并分出左右两列。
 *
 * 只用左边界的最大的间隔做切分，并要求两列在纵向上有明显重叠；
 * 这能避免把「标题在上、正文在下」误判成两栏。
 */
fn split_columns(lines: &[LayoutLine], page_width: f64) -> Option<(Vec<LayoutLine>, Vec<LayoutLine>)> {
    if lines.len() < 4 || page_width <= 0.0 {
        return None;
    }
    let mut left_edges = lines.iter().map(|line| line.min_x).collect::<Vec<_>>();
    left_edges.sort_by(f64::total_cmp);
    let mut best_gap = 0.0;
    let mut split_at = 0.0;
    for pair in left_edges.windows(2) {
        let gap = pair[1] - pair[0];
        if gap > best_gap {
            best_gap = gap;
            split_at = (pair[0] + pair[1]) / 2.0;
        }
    }
    if best_gap < page_width * 0.18 {
        return None;
    }

    let (left, right): (Vec<LayoutLine>, Vec<LayoutLine>) = lines
        .iter()
        .cloned()
        .partition(|line| line.min_x < split_at);
    if left.len() < 2 || right.len() < 2 {
        return None;
    }

    // 两列必须并排：纵向范围重叠要足够，否则只是上下两个区块。
    let left_span = span(&left);
    let right_span = span(&right);
    let overlap = (left_span.0.max(right_span.0) - left_span.1.min(right_span.1)).max(0.0);
    let shorter = (left_span.0 - left_span.1).min(right_span.0 - right_span.1);
    if shorter <= 0.0 || overlap / shorter < 0.3 {
        return None;
    }

    Some((left, right))
}

/// 一列文本的纵向范围 (max_y, min_y)。
fn span(lines: &[LayoutLine]) -> (f64, f64) {
    let max_y = lines.iter().map(|line| line.y).fold(f64::MIN, f64::max);
    let min_y = lines.iter().map(|line| line.y).fold(f64::MAX, f64::min);
    (max_y, min_y)
}

/// 按上一个字形的右边界决定是否补空格；中日韩字符之间不补。
fn push_with_spacing(text: &mut String, cursor: &f64, item: &PositionedText) {
    let previous = text.chars().last();
    let gap = item.x - *cursor;
    let needs_space = gap > item.size * 0.25
        && !text.is_empty()
        && !previous.is_some_and(is_cjk)
        && !item.text.chars().next().is_some_and(is_cjk);
    if needs_space {
        text.push(' ');
    }
    text.push_str(&item.text);
}

/// 中日韩统一表意文字与全角标点：这些字符之间不需要空格。
fn is_cjk(value: char) -> bool {
    matches!(value as u32,
        0x3000..=0x303F | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF)
}

/// 取中位数；空集合返回 1.0。
fn median(mut values: Vec<f64>) -> f64 {
    if values.is_empty() {
        return 1.0;
    }
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// 页面级入口，便于测试时直接喂坐标。
#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(x: f64, y: f64, text: &str) -> PositionedText {
        PositionedText {
            x,
            y,
            // 测试里用固定推进宽度，聚焦拼接与分栏逻辑。
            width: 5.0,
            size: 10.0,
            text: text.to_owned(),
        }
    }

    /// 内容流先出现右栏时，也要先输出左栏。
    #[test]
    fn assembles_two_column_reading_order() {
        let mut items = vec![
            glyph(320.0, 700.0, "右栏第一行"),
            glyph(320.0, 680.0, "右栏第二行"),
            glyph(320.0, 660.0, "右栏第三行"),
            glyph(60.0, 700.0, "左栏第一行"),
            glyph(60.0, 680.0, "左栏第二行"),
            glyph(60.0, 660.0, "左栏第三行"),
        ];
        let text = assemble_reading_order(&items, 600.0);
        let left = text.find("左栏第一行").expect("应包含左栏");
        let right = text.find("右栏第一行").expect("应包含右栏");
        assert!(left < right, "左栏应排在右栏之前：{text}");

        // 单栏内容不应被误判成两栏。
        items = vec![
            glyph(60.0, 700.0, "标题"),
            glyph(60.0, 680.0, "正文第一行"),
            glyph(60.0, 660.0, "正文第二行"),
            glyph(60.0, 640.0, "正文第三行"),
        ];
        let single = assemble_reading_order(&items, 600.0);
        assert!(single.find("标题").unwrap() < single.find("正文第一行").unwrap());
        assert_eq!(single.lines().count(), 4);
    }

    /// 英文单词之间要补空格，中文之间不补。
    #[test]
    fn inserts_spacing_between_latin_words_only() {
        let items = vec![
            glyph(60.0, 700.0, "Hello"),
            glyph(90.0, 700.0, "World"),
            glyph(120.0, 700.0, "世界"),
            glyph(140.0, 700.0, "和平"),
        ];
        let text = assemble_reading_order(&items, 600.0);
        assert!(text.contains("Hello World"), "{text}");
        assert!(text.contains("世界和平"), "{text}");
    }
}
