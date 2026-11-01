//! Theme-independent Python captures. Unchanged source reuses the cached spans.
use std::{ops::Range, rc::Rc, sync::OnceLock};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

pub const NAMES: [&str; 8] = [
    "keyword", "string", "comment", "number", "function", "type", "constant", "operator",
];
pub type Span = (Range<usize>, usize);

fn configuration() -> &'static HighlightConfiguration {
    static CONFIG: OnceLock<HighlightConfiguration> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let mut config = HighlightConfiguration::new(
            tree_sitter_python::LANGUAGE.into(),
            "python",
            tree_sitter_python::HIGHLIGHTS_QUERY,
            "",
            "",
        )
        .expect("bundled Python highlight query");
        config.configure(&NAMES);
        config
    })
}

pub fn python(text: &str) -> Vec<Span> {
    // Bound synchronous work; large cells remain fully editable as plain text.
    if text.len() > 256 * 1024 {
        return vec![];
    }
    let mut highlighter = Highlighter::new();
    let Ok(events) = highlighter.highlight(configuration(), text.as_bytes(), None, |_| None) else {
        return vec![];
    };
    let mut stack = vec![];
    let mut spans = vec![];
    for event in events {
        match event {
            Ok(HighlightEvent::HighlightStart(style)) => stack.push(style.0),
            Ok(HighlightEvent::HighlightEnd) => {
                stack.pop();
            }
            Ok(HighlightEvent::Source { start, end }) => {
                if let Some(&style) = stack.last() {
                    spans.push((start..end, style));
                }
            }
            Err(_) => return vec![],
        }
    }
    spans
}

#[derive(Default)]
pub struct Cache {
    source: String,
    spans: Rc<Vec<Span>>,
}
impl Cache {
    pub fn get(&mut self, text: &str) -> Rc<Vec<Span>> {
        if self.source != text {
            self.spans = Rc::new(python(text));
            self.source = text.to_owned();
        }
        self.spans.clone()
    }
}

/// Split a visual row at syntax and composition boundaries, retaining absolute offsets.
pub fn segments(
    range: Range<usize>,
    spans: &[Span],
    marked: Option<&Range<usize>>,
) -> Vec<(Range<usize>, Option<usize>, bool)> {
    let relevant = &spans[spans.partition_point(|(r, _)| r.end <= range.start)..];
    let mut cuts = vec![range.start, range.end];
    for (r, _) in relevant.iter().take_while(|(r, _)| r.start < range.end) {
        cuts.extend([r.start.max(range.start), r.end.min(range.end)]);
    }
    if let Some(r) = marked {
        cuts.extend([
            r.start.clamp(range.start, range.end),
            r.end.clamp(range.start, range.end),
        ]);
    }
    cuts.sort_unstable();
    cuts.dedup();
    cuts.windows(2)
        .map(|pair| {
            let start = pair[0];
            let style = relevant
                .iter()
                .take_while(|(r, _)| r.start <= start)
                .find(|(r, _)| r.contains(&start))
                .map(|(_, style)| *style);
            (
                start..pair[1],
                style,
                marked.is_some_and(|r| r.contains(&start)),
            )
        })
        .collect()
}

/// Keep leading whitespace; add one level for Python suites and bracket continuations.
pub fn newline(text: &str, offset: usize, python_code: bool) -> String {
    let prefix = &text[..offset];
    let line = prefix.rsplit('\n').next().unwrap_or("");
    let indent: String = line
        .chars()
        .take_while(|c| matches!(c, ' ' | '\t'))
        .collect();
    if !python_code {
        return format!("\n{indent}");
    }
    let spans = python(prefix);
    let mut brackets = vec![];
    let mut last = None;
    let line_start = prefix.rfind('\n').map_or(0, |i| i + 1);
    let mut span_index = 0;
    for (index, ch) in prefix.char_indices() {
        while spans.get(span_index).is_some_and(|(r, _)| r.end <= index) {
            span_index += 1;
        }
        if spans
            .get(span_index)
            .is_some_and(|(r, style)| matches!(*style, 1 | 2) && r.contains(&index))
        {
            continue;
        }
        match ch {
            '(' | '[' | '{' => brackets.push(index),
            ')' | ']' | '}' => {
                brackets.pop();
            }
            _ => {}
        }
        if index >= line_start && !ch.is_whitespace() {
            last = Some(ch);
        }
    }
    if let Some(&opener) = brackets.last() {
        let opening_start = prefix[..opener].rfind('\n').map_or(0, |i| i + 1);
        let opening_indent: String = prefix[opening_start..opener]
            .chars()
            .take_while(|c| matches!(c, ' ' | '\t'))
            .collect();
        let unit = if opening_indent.contains('\t') {
            "\t"
        } else {
            "    "
        };
        let continuation = format!("{opening_indent}{unit}");
        return format!(
            "\n{}",
            if indent.len() > continuation.len() {
                &indent
            } else {
                &continuation
            }
        );
    }
    let extra = if last == Some(':') {
        if indent.contains('\t') { "\t" } else { "    " }
    } else {
        ""
    };
    format!("\n{indent}{extra}")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn python_captures_strings_comments_numbers_and_incomplete_code() {
        let text =
            "def café(x):\n    s = '''hello\nworld'''\n    # note\n    return 42 + x\nif True:\n";
        let spans = python(text);
        for (token, name) in [
            ("def", "keyword"),
            ("café", "function"),
            ("world", "string"),
            ("# note", "comment"),
            ("42", "number"),
        ] {
            let i = text.find(token).unwrap();
            assert!(
                spans
                    .iter()
                    .any(|(r, style)| r.contains(&i) && NAMES[*style] == name),
                "{token}"
            );
        }
    }
    #[test]
    fn wrapped_highlights_keep_ime_and_utf8_boundaries() {
        let spans = vec![(0..8, 1)];
        let runs = segments(2..8, &spans, Some(&(4..6)));
        assert_eq!(
            runs,
            vec![
                (2..4, Some(1), false),
                (4..6, Some(1), true),
                (6..8, Some(1), false)
            ]
        );
    }
    #[test]
    fn newline_ignores_colons_in_comments_and_strings() {
        for (text, expected) in [
            ("if True: # yes", "\n    "),
            ("    print('x:') # :", "\n    "),
            ("    values = [", "\n        "),
            ("\tif True:", "\n\t\t"),
        ] {
            assert_eq!(newline(text, text.len(), true), expected);
        }
        assert_eq!(newline("text:", 5, false), "\n");
        let continuation = "values = [\n    1,";
        assert_eq!(newline(continuation, continuation.len(), true), "\n    ");
        let blank = "if True:\n    ";
        assert_eq!(newline(blank, blank.len(), true), "\n    ");
    }
}
