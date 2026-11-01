//! Inert native rendering of notebook MIME bundles and Markdown. No web view,
//! scripts, stylesheets, or implicit network requests are involved.
use crate::theme::ActiveTheme;
use base64::{Engine, engine::general_purpose::STANDARD};
use gpui::{
    AnyElement, App, FontStyle, FontWeight, HighlightStyle, Image, ImageFormat, InteractiveText,
    StyledText, Window, div, img, prelude::*, px,
};
use scraper::{ElementRef, Html};
use serde_json::{Map, Value};
use std::{ops::Range, sync::Arc};

const MAX_IMAGE: usize = 32 * 1024 * 1024;
const MAX_HTML: usize = 2 * 1024 * 1024;

#[derive(Clone, Default)]
pub struct Inline {
    text: String,
    spans: Vec<(Range<usize>, InlineStyle)>,
}
#[derive(Clone, Default)]
struct InlineStyle {
    bold: bool,
    italic: bool,
    code: bool,
    link: Option<String>,
    color: Option<u8>,
}
impl Inline {
    fn push(&mut self, text: &str, style: &InlineStyle) {
        let start = self.text.len();
        self.text.push_str(text);
        if start != self.text.len() {
            self.spans.push((start..self.text.len(), style.clone()));
        }
    }
    fn push_flow(&mut self, source: &str, style: &InlineStyle) {
        let mut text = String::new();
        let mut space = self.text.ends_with(char::is_whitespace);
        for ch in source.chars() {
            if ch.is_whitespace() {
                if !space {
                    text.push(' ');
                }
                space = true;
            } else {
                text.push(ch);
                space = false;
            }
        }
        self.push(&text, style);
    }
    fn trim(&mut self) {
        let start = self.text.len() - self.text.trim_start().len();
        let end = self.text.trim_end().len();
        if end < start {
            self.text.clear();
            self.spans.clear();
            return;
        }
        self.text = self.text[start..end].to_owned();
        self.spans = self
            .spans
            .drain(..)
            .filter_map(|(r, s)| {
                let a = r.start.max(start);
                let b = r.end.min(end);
                (a < b).then(|| (a - start..b - start, s))
            })
            .collect();
    }
    fn render(&self, id: usize, cx: &App) -> AnyElement {
        let t = cx.theme();
        let highlights = self.spans.iter().map(|(range, style)| {
            let color = style
                .color
                .map(|c| match c % 8 {
                    0 => t.app.muted_fg,
                    1 => t.destructive,
                    2 => t.chart[1],
                    3 => t.chart[2],
                    4 => t.app.primary,
                    5 => t.chart[3],
                    6 => t.chart[4],
                    _ => t.app.fg,
                })
                .or_else(|| style.link.as_ref().map(|_| t.app.primary));
            (
                range.clone(),
                HighlightStyle {
                    color,
                    font_weight: style.bold.then_some(FontWeight::BOLD),
                    font_style: style.italic.then_some(FontStyle::Italic),
                    background_color: style.code.then_some(t.muted),
                    ..Default::default()
                },
            )
        });
        // Resolve runs during layout, after parent font styles have been applied.
        let text = StyledText::new(self.text.clone()).with_highlights(highlights);
        let links: Vec<_> = self
            .spans
            .iter()
            .filter_map(|(r, s)| s.link.as_ref().map(|u| (r.clone(), u.clone())))
            .collect();
        InteractiveText::new(("rich-inline", id), text)
            .on_click(
                links.iter().map(|(r, _)| r.clone()).collect(),
                move |i, _, cx| {
                    cx.stop_propagation();
                    cx.open_url(&links[i].1);
                },
            )
            .into_any_element()
    }
}

#[derive(Clone)]
pub enum Block {
    Text(Inline, u8),
    Code(String),
    Table(Vec<Vec<(String, bool)>>),
    Image(Arc<Image>),
    Rule,
    Math(String),
}
#[derive(Clone, Default)]
pub struct Rich {
    blocks: Vec<Block>,
    prose: bool,
}

pub fn mime_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Array(lines) => lines
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()
            .map(|v| v.concat()),
        _ => None,
    }
}
fn image_data(data: &Map<String, Value>) -> Option<Arc<Image>> {
    for (mime, format) in [
        ("image/png", ImageFormat::Png),
        ("image/jpeg", ImageFormat::Jpeg),
        ("image/gif", ImageFormat::Gif),
        ("image/webp", ImageFormat::Webp),
    ] {
        if let Some(encoded) = data.get(mime).and_then(mime_text) {
            if encoded.len() > MAX_IMAGE * 4 / 3 + 1024 {
                continue;
            }
            let encoded: String = encoded
                .chars()
                .filter(|c| !c.is_ascii_whitespace())
                .collect();
            if let Ok(bytes) = STANDARD.decode(encoded) {
                // Check signatures so malformed preferred MIME data can fall back to text.
                let valid = match format {
                    ImageFormat::Png => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
                    ImageFormat::Jpeg => bytes.starts_with(b"\xff\xd8\xff"),
                    ImageFormat::Gif => bytes.starts_with(b"GIF8"),
                    ImageFormat::Webp => {
                        bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP")
                    }
                    _ => false,
                };
                let dimensions = image::ImageReader::new(std::io::Cursor::new(&bytes))
                    .with_guessed_format()
                    .ok()
                    .and_then(|reader| reader.into_dimensions().ok());
                let bounded = dimensions.is_some_and(|(w, h)| {
                    w > 0 && h > 0 && u64::from(w) * u64::from(h) <= 32_000_000
                });
                if valid && bounded && bytes.len() <= MAX_IMAGE {
                    return Some(Arc::new(Image::from_bytes(format, bytes)));
                }
            }
        }
    }
    None
}
impl Rich {
    pub fn output(data: Option<&Map<String, Value>>, text: &str) -> Self {
        if let Some(data) = data {
            if let Some(image) = image_data(data) {
                return Self {
                    blocks: vec![Block::Image(image)],
                    prose: false,
                };
            }
            if let Some(html) = data
                .get("text/html")
                .and_then(mime_text)
                .filter(|s| s.len() <= MAX_HTML)
            {
                let rich = Self::html(&html, None);
                if !rich.blocks.is_empty() {
                    return rich;
                }
            }
            if let Some(latex) = data.get("text/latex").and_then(mime_text) {
                return Self {
                    blocks: vec![math(&latex)],
                    prose: true,
                };
            }
            if let Some(md) = data
                .get("text/markdown")
                .and_then(mime_text)
                .filter(|s| s.len() <= MAX_HTML)
            {
                return Self::markdown(&md, None);
            }
        }
        Self {
            prose: false,
            blocks: vec![Block::Text(
                ansi(text.strip_suffix('\n').unwrap_or(text)),
                0,
            )],
        }
    }
    pub fn markdown(source: &str, attachments: Option<&Map<String, Value>>) -> Self {
        if source.len() > MAX_HTML {
            return Self {
                blocks: vec![Block::Code(source.to_owned())],
                prose: false,
            };
        }
        let options = pulldown_cmark::Options::ENABLE_TABLES
            | pulldown_cmark::Options::ENABLE_STRIKETHROUGH
            | pulldown_cmark::Options::ENABLE_TASKLISTS
            | pulldown_cmark::Options::ENABLE_MATH;
        let mut html = String::new();
        pulldown_cmark::html::push_html(
            &mut html,
            pulldown_cmark::Parser::new_ext(source, options),
        );
        Self::html(&html, attachments)
    }
    fn html(html: &str, attachments: Option<&Map<String, Value>>) -> Self {
        let document = Html::parse_fragment(html);
        let mut blocks = vec![];
        collect(document.root_element(), &mut blocks, attachments, 0);
        if blocks.len() >= 2048 {
            blocks.push(Block::Code(
                "[Preview limited to 2,048 blocks; edit the source to see all content]".into(),
            ));
        }
        Self {
            blocks,
            prose: true,
        }
    }
    pub fn render(&self, _window: &Window, cx: &App) -> AnyElement {
        let t = cx.theme();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .min_w_0()
            .when(self.prose, |v| {
                v.font_family(t.font.sans.clone()).text_size(t.font.base)
            })
            .children(self.blocks.iter().enumerate().map(|(id, block)| {
                match block {
                    Block::Text(text, level) => div()
                        .min_w_0()
                        .when(*level > 0, |v| {
                            v.font_family(t.font.sans.clone())
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(if *level == 1 {
                                    t.font.xl
                                } else if *level == 2 {
                                    t.font.lg
                                } else {
                                    t.font.base
                                })
                        })
                        .child(text.render(id, cx))
                        .into_any_element(),
                    Block::Code(text) => div()
                        .id(("rich-code", id))
                        .overflow_x_scroll()
                        .px_2()
                        .py_1()
                        .bg(t.card.bg)
                        .text_color(t.card.fg)
                        .rounded(t.radius.sm())
                        .font_family(t.font.mono.clone())
                        .text_size(t.font.code)
                        .child(text.clone())
                        .into_any_element(),
                    Block::Image(image) => img(image.clone())
                        .max_w_full()
                        .object_fit(gpui::ObjectFit::Contain)
                        .with_fallback(|| {
                            div()
                                .child("[Image could not be decoded]")
                                .into_any_element()
                        })
                        .into_any_element(),
                    Block::Math(svg) => {
                        let rgb = t.app.fg.to_rgb();
                        let color = format!(
                            "#{:02x}{:02x}{:02x}",
                            (rgb.r * 255.) as u8,
                            (rgb.g * 255.) as u8,
                            (rgb.b * 255.) as u8
                        );
                        let image = Arc::new(Image::from_bytes(
                            ImageFormat::Svg,
                            svg.replace("#000000", &color).into_bytes(),
                        ));
                        div()
                            .py_1()
                            .child(img(image).max_w_full())
                            .into_any_element()
                    }
                    Block::Rule => div().h(px(1.)).bg(t.app.border).into_any_element(),
                    Block::Table(rows) => {
                        let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
                        // Shared widths keep header/index columns aligned across rows.
                        let widths: Vec<_> = (0..columns)
                            .map(|col| {
                                rows.iter()
                                    .filter_map(|r| r.get(col))
                                    .map(|(s, _)| s.chars().count())
                                    .max()
                                    .unwrap_or(0)
                                    .clamp(4, 36) as f32
                                    * 8.
                                    + 16.
                            })
                            .collect();
                        div()
                            .id(("rich-table", id))
                            .overflow_x_scroll()
                            .font_family(t.font.sans.clone())
                            .text_size(t.font.sm)
                            .children(rows.iter().map(|row| {
                                div().flex().children(row.iter().enumerate().map(
                                    |(col, (text, header))| {
                                        div()
                                            .w(px(widths[col]))
                                            .flex_shrink_0()
                                            .px_2()
                                            .py_1()
                                            .border_b_1()
                                            .border_color(t.app.border)
                                            .when(*header, |v| {
                                                v.font_weight(FontWeight::SEMIBOLD)
                                                    .bg(t.card.bg)
                                                    .text_color(t.card.fg)
                                            })
                                            .child(text.clone())
                                    },
                                ))
                            }))
                            .into_any_element()
                    }
                }
            }))
            .into_any_element()
    }
}
fn allowed_link(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://") || url.starts_with("mailto:")
}
fn hidden(name: &str) -> bool {
    matches!(
        name,
        "script" | "style" | "iframe" | "object" | "embed" | "svg" | "head" | "noscript"
    )
}
fn inline(element: ElementRef<'_>, text: &mut Inline, style: InlineStyle) {
    inline_depth(element, text, style, 0);
}
fn inline_depth(element: ElementRef<'_>, text: &mut Inline, mut style: InlineStyle, depth: usize) {
    if depth > 32 {
        return;
    }
    let name = element.value().name();
    if hidden(name) {
        return;
    }
    match name {
        "strong" | "b" | "th" => style.bold = true,
        "em" | "i" => style.italic = true,
        "code" => style.code = true,
        "a" => {
            style.link = element
                .value()
                .attr("href")
                .filter(|s| allowed_link(s))
                .map(str::to_owned)
        }
        "br" => {
            text.push("\n", &style);
            return;
        }
        "img" => {
            text.push(element.value().attr("alt").unwrap_or("[Image]"), &style);
            return;
        }
        _ => (),
    }
    for child in element.children() {
        if let Some(s) = child.value().as_text() {
            text.push_flow(s, &style);
        } else if let Some(e) = ElementRef::wrap(child) {
            inline_depth(e, text, style.clone(), depth + 1);
        }
    }
}
fn collect(
    element: ElementRef<'_>,
    blocks: &mut Vec<Block>,
    attachments: Option<&Map<String, Value>>,
    depth: usize,
) {
    if depth > 32 || blocks.len() >= 2048 {
        return;
    }
    let name = element.value().name();
    if hidden(name) {
        return;
    }
    if element.value().attr("class").is_some_and(|c| {
        c.split_whitespace()
            .any(|c| c == "math-display" || c == "math-inline")
    }) {
        blocks.push(math(&element.text().collect::<String>()));
        return;
    }
    match name {
        "table" => {
            let rows = scraper::Selector::parse("tr").unwrap();
            let cells = scraper::Selector::parse("th,td").unwrap();
            let table = element
                .select(&rows)
                .take(500)
                .map(|row| {
                    row.select(&cells)
                        .take(64)
                        .map(|cell| {
                            let mut text = Inline::default();
                            inline(cell, &mut text, Default::default());
                            (text.text.trim().to_owned(), cell.value().name() == "th")
                        })
                        .collect()
                })
                .collect();
            blocks.push(Block::Table(table));
            if element.select(&rows).count() > 500
                || element
                    .select(&rows)
                    .any(|row| row.select(&cells).count() > 64)
            {
                blocks.push(Block::Code(
                    "[Table preview limited to 500 rows and 64 columns]".into(),
                ));
            }
            return;
        }
        "pre" => {
            blocks.push(Block::Code(
                element
                    .text()
                    .collect::<String>()
                    .trim_end_matches('\n')
                    .to_owned(),
            ));
            return;
        }
        "hr" => {
            blocks.push(Block::Rule);
            return;
        }
        "img" => {
            let src = element.value().attr("src").unwrap_or_default();
            let attachment = src
                .strip_prefix("attachment:")
                .and_then(|n| attachments?.get(n)?.as_object());
            if let Some(image) = attachment.and_then(image_data) {
                blocks.push(Block::Image(image));
            } else {
                let mut text = Inline::default();
                text.push(
                    element
                        .value()
                        .attr("alt")
                        .filter(|s| !s.is_empty())
                        .unwrap_or("[External image not loaded]"),
                    &Default::default(),
                );
                blocks.push(Block::Text(text, 0));
            }
            return;
        }
        "ol" | "ul" => {
            let mut number = element
                .value()
                .attr("start")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(1);
            for item in element
                .children()
                .filter_map(ElementRef::wrap)
                .filter(|e| e.value().name() == "li")
            {
                let mut text = Inline::default();
                text.push(
                    &if name == "ol" {
                        format!("{number}. ")
                    } else {
                        "• ".into()
                    },
                    &Default::default(),
                );
                for child in item.children() {
                    if let Some(s) = child.value().as_text() {
                        text.push_flow(s, &Default::default());
                    } else if let Some(e) = ElementRef::wrap(child) {
                        if matches!(e.value().name(), "ol" | "ul") {
                            continue;
                        }
                        inline(e, &mut text, Default::default());
                    }
                }
                text.trim();
                blocks.push(Block::Text(text, 0));
                for list in item
                    .children()
                    .filter_map(ElementRef::wrap)
                    .filter(|e| matches!(e.value().name(), "ol" | "ul"))
                {
                    let start = blocks.len();
                    collect(list, blocks, attachments, depth + 1);
                    for block in &mut blocks[start..] {
                        if let Block::Text(text, _) = block {
                            text.text.insert_str(0, "    ");
                            for (range, _) in &mut text.spans {
                                range.start += 4;
                                range.end += 4;
                            }
                        }
                    }
                }
                number += 1;
            }
            return;
        }
        "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "li" | "blockquote" => {
            // Standalone Markdown attachment images are paragraphs around img.
            if name == "p"
                && element.children().filter_map(ElementRef::wrap).any(|e| {
                    e.value().name() == "img"
                        || e.value().attr("class").is_some_and(|c| {
                            c.contains("math-display") || c.contains("math-inline")
                        })
                })
            {
                let mut text = Inline::default();
                for child in element.children() {
                    if let Some(s) = child.value().as_text() {
                        text.push_flow(s, &Default::default());
                    } else if let Some(e) = ElementRef::wrap(child) {
                        if e.value().name() == "img"
                            || e.value().attr("class").is_some_and(|c| {
                                c.contains("math-display") || c.contains("math-inline")
                            })
                        {
                            if !text.text.trim().is_empty() {
                                blocks.push(Block::Text(std::mem::take(&mut text), 0));
                            }
                            collect(e, blocks, attachments, depth + 1);
                        } else {
                            inline(e, &mut text, Default::default());
                        }
                    }
                }
                if !text.text.trim().is_empty() {
                    blocks.push(Block::Text(text, 0));
                }
                return;
            }
            let mut text = Inline::default();
            if name == "li" {
                text.push("• ", &Default::default());
            }
            if name == "blockquote" {
                text.push("│ ", &Default::default());
            }
            inline(element, &mut text, Default::default());
            let level = name
                .strip_prefix('h')
                .and_then(|n| n.parse::<u8>().ok())
                .unwrap_or(0);
            text.trim();
            if !text.text.is_empty() {
                blocks.push(Block::Text(text, level));
            }
            return;
        }
        _ => (),
    }
    for child in element.children() {
        if let Some(e) = ElementRef::wrap(child) {
            collect(e, blocks, attachments, depth + 1);
        } else if let Some(text) = child.value().as_text()
            && !text.trim().is_empty()
        {
            blocks.push(Block::Text(ansi(text), 0));
        }
    }
}

fn nearest_color(r: u8, g: u8, b: u8) -> u8 {
    let max = r.max(g).max(b);
    if max == 0 {
        return 0;
    }
    u8::from(u16::from(r) * 2 >= u16::from(max))
        | (u8::from(u16::from(g) * 2 >= u16::from(max)) << 1)
        | (u8::from(u16::from(b) * 2 >= u16::from(max)) << 2)
}

/// Parse SGR colors/emphasis and discard terminal control sequences (including OSC links).
fn ansi(source: &str) -> Inline {
    let mut text = Inline::default();
    let mut style = InlineStyle::default();
    let mut chars = source.chars().peekable();
    let mut chunk = String::new();
    while let Some(ch) = chars.next() {
        if ch != '\x1b' {
            if ch >= ' ' || ch == '\n' || ch == '\t' {
                chunk.push(ch);
            }
            continue;
        }
        text.push(&chunk, &style);
        chunk.clear();
        match chars.next() {
            Some('[') => {
                let mut params = String::new();
                for ch in chars.by_ref() {
                    if ('@'..='~').contains(&ch) {
                        if ch == 'm' {
                            let mut codes =
                                params.split(';').map(|v| v.parse::<u16>().unwrap_or(0));
                            while let Some(n) = codes.next() {
                                match n {
                                    0 => style = Default::default(),
                                    1 => style.bold = true,
                                    3 => style.italic = true,
                                    22 => style.bold = false,
                                    23 => style.italic = false,
                                    30..=37 => style.color = Some((n - 30) as u8),
                                    90..=97 => style.color = Some((n - 90) as u8),
                                    39 => style.color = None,
                                    38 | 48 => {
                                        let color = match codes.next() {
                                            Some(5) => codes.next().map(|n| {
                                                if n < 16 {
                                                    (n % 8) as u8
                                                } else if n < 232 {
                                                    let n = n - 16;
                                                    nearest_color(
                                                        (n / 36) as u8,
                                                        ((n / 6) % 6) as u8,
                                                        (n % 6) as u8,
                                                    )
                                                } else {
                                                    7
                                                }
                                            }),
                                            Some(2) => {
                                                match (codes.next(), codes.next(), codes.next()) {
                                                    (Some(r), Some(g), Some(b)) => {
                                                        Some(nearest_color(
                                                            r.min(255) as u8,
                                                            g.min(255) as u8,
                                                            b.min(255) as u8,
                                                        ))
                                                    }
                                                    _ => None,
                                                }
                                            }
                                            _ => None,
                                        };
                                        // Map foreground colors to theme-aware terminal roles.
                                        if n == 38 {
                                            style.color = color;
                                        }
                                    }
                                    _ => (),
                                }
                            }
                        }
                        break;
                    }
                    params.push(ch);
                }
            }
            Some(']') => {
                while let Some(ch) = chars.next() {
                    if ch == '\x07' || ch == '\x1b' && chars.next() == Some('\\') {
                        break;
                    }
                }
            }
            _ => (),
        }
    }
    text.push(&chunk, &style);
    text
}

fn math(source: &str) -> Block {
    let source = source
        .trim()
        .trim_start_matches("$$")
        .trim_end_matches("$$")
        .trim_start_matches("\\[")
        .trim_end_matches("\\]")
        .trim();
    if source.len() <= 8192
        && let Ok(font) = latex_rust::MathFont::stix_two_math()
    {
        let mut options = latex_rust::SvgOptions::new();
        options.display = true;
        if let Ok(svg) = latex_rust::latex_to_svg(source, &font, &options) {
            return Block::Math(svg);
        }
    }
    Block::Code(source.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn strings(rich: &Rich) -> String {
        rich.blocks
            .iter()
            .filter_map(|block| match block {
                Block::Text(t, _) => Some(t.text.as_str()),
                Block::Code(s) => Some(s.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    #[test]
    fn mime_priority_falls_back_from_bad_images_and_preserves_tables() {
        let data = serde_json::from_value(serde_json::json!({
            "image/png": "not base64", "text/html": "<style>hidden</style><table><tr><th>City</th><th>Count</th></tr><tr><td>A &amp; B</td><td>42</td></tr></table>", "text/plain": "fallback"
        })).unwrap();
        let rich = Rich::output(Some(&data), "fallback");
        let Block::Table(rows) = &rich.blocks[0] else {
            panic!("expected table")
        };
        assert_eq!(rows[0][0], ("City".into(), true));
        assert_eq!(rows[1][0], ("A & B".into(), false));
        let data =
            serde_json::from_value(serde_json::json!({"text/html":"<script>bad()</script>"}))
                .unwrap();
        assert_eq!(strings(&Rich::output(Some(&data), "fallback")), "fallback");
    }
    #[test]
    fn markdown_preserves_lists_styles_links_and_code_without_active_html() {
        let rich = Rich::markdown(
            "# Title\n\nA **bold** and *italic* [link](https://example.com).\n\n3. Third\n4. Fourth\n   - Child\n\n```python\nprint(42)\n```\n<script>bad()</script>",
            None,
        );
        assert!(matches!(rich.blocks[0], Block::Text(_, 1)));
        let text = strings(&rich);
        assert!(
            text.contains("3. Third") && text.contains("4. Fourth") && text.contains("    • Child")
        );
        assert!(!text.contains("bad()"));
        assert!(
            rich.blocks
                .iter()
                .any(|b| matches!(b, Block::Code(s) if s == "print(42)"))
        );
        assert!(!allowed_link("javascript:alert(1)"));
        assert!(!allowed_link("file:///etc/passwd"));
        assert!(
            rich.blocks
                .iter()
                .any(|b| matches!(b, Block::Text(t,_) if t.spans.iter().any(|(_,s)| s.bold)))
        );
    }
    #[test]
    fn ansi_preserves_unicode_styles_and_strips_osc_commands() {
        let text =
            ansi("α\x1b[1;31mred\x1b[0m plain\x1b]8;;https://example.com\x07link\x1b]8;;\x1b\\");
        assert_eq!(text.text, "αred plainlink");
        assert!(
            text.spans
                .iter()
                .any(|(range, s)| &text.text[range.clone()] == "red"
                    && s.bold
                    && s.color == Some(1))
        );
        assert!(!text.text.contains('\x1b'));
        let indexed = ansi("\x1b[38;5;196mred\x1b[0m\x1b[38;2;0;255;0mgreen");
        assert_eq!(indexed.spans[0].1.color, Some(1));
        assert_eq!(indexed.spans[1].1.color, Some(2));
    }
    #[test]
    fn equations_render_as_paths_and_invalid_latex_keeps_source() {
        let rich = Rich::markdown("$$\\frac{1}{2} + x^2$$", None);
        assert!(
            rich.blocks
                .iter()
                .any(|b| matches!(b, Block::Math(s) if s.contains("<path")))
        );
        assert!(matches!(math("\\notARealCommand{x}"), Block::Code(_)));
    }
    #[test]
    fn unsupported_mime_and_empty_text_have_readable_fallbacks() {
        let data =
            serde_json::from_value(serde_json::json!({"application/x-widget": {"state": 1}}))
                .unwrap();
        assert_eq!(
            strings(&Rich::output(Some(&data), "[Unsupported output]")),
            "[Unsupported output]"
        );
        assert_eq!(
            strings(&Rich::output(None, "first\nsecond\n")),
            "first\nsecond"
        );
    }
}
