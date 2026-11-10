//! Theme-independent language captures. Unchanged source reuses the cached spans.
use std::{ops::Range, rc::Rc, sync::OnceLock};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

pub const NAMES: [&str; 8] = [
    "keyword", "string", "comment", "number", "function", "type", "constant", "operator",
];
pub type Span = (Range<usize>, usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Python,
    Markdown,
    Toml,
    Json,
    Yaml,
    Sql,
}
impl Language {
    pub fn for_path(path: &str) -> Option<Self> {
        match std::path::Path::new(path)
            .extension()?
            .to_str()?
            .to_ascii_lowercase()
            .as_str()
        {
            "py" | "pyw" | "pyi" => Some(Self::Python),
            "md" | "markdown" => Some(Self::Markdown),
            "toml" => Some(Self::Toml),
            "json" => Some(Self::Json),
            "yaml" | "yml" => Some(Self::Yaml),
            "sql" => Some(Self::Sql),
            _ => None,
        }
    }
}

fn configuration(language: Language) -> &'static HighlightConfiguration {
    static PYTHON: OnceLock<HighlightConfiguration> = OnceLock::new();
    static MARKDOWN: OnceLock<HighlightConfiguration> = OnceLock::new();
    static TOML: OnceLock<HighlightConfiguration> = OnceLock::new();
    static YAML: OnceLock<HighlightConfiguration> = OnceLock::new();
    static SQL: OnceLock<HighlightConfiguration> = OnceLock::new();
    static JSON: OnceLock<HighlightConfiguration> = OnceLock::new();
    let (cache, grammar, name, query, injections) = match language {
        Language::Python => (
            &PYTHON,
            tree_sitter_python::LANGUAGE,
            "python",
            tree_sitter_python::HIGHLIGHTS_QUERY,
            "",
        ),
        Language::Markdown => (
            &MARKDOWN,
            tree_sitter_md::LANGUAGE,
            "markdown",
            tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
            r#"
            (fenced_code_block
              (info_string (language) @injection.language)
              (code_fence_content) @injection.content
              (#set! injection.include-children))
            ((inline) @injection.content
              (#set! injection.language "markdown_inline")
              (#set! injection.include-children))
            "#,
        ),
        Language::Toml => (
            &TOML,
            tree_sitter_toml_ng::LANGUAGE,
            "toml",
            tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
            "",
        ),
        Language::Yaml => (
            &YAML,
            tree_sitter_yaml::LANGUAGE,
            "yaml",
            tree_sitter_yaml::HIGHLIGHTS_QUERY,
            "",
        ),
        Language::Sql => (
            &SQL,
            tree_sitter_sequel::LANGUAGE,
            "sql",
            tree_sitter_sequel::HIGHLIGHTS_QUERY,
            "",
        ),
        Language::Json => (
            &JSON,
            tree_sitter_json::LANGUAGE,
            "json",
            tree_sitter_json::HIGHLIGHTS_QUERY,
            "",
        ),
    };
    cache.get_or_init(|| {
        let mut config = HighlightConfiguration::new(
            grammar.into(),
            name,
            &normalize_query(query),
            injections,
            "",
        )
        .expect("bundled highlight query");
        config.configure(&NAMES);
        config
    })
}

// Map grammar capture vocabularies onto our semantic theme palette.
fn normalize_query(query: &str) -> String {
    // Spell-check captures are not visual styles and must not mask comments.
    let mut query = query.replace(" @spell", "");
    for (from, to) in [
        ("@string.special.key", "@type"),
        ("@property", "@type"),
        ("@boolean", "@constant"),
        ("@float", "@number"),
        ("@field", "@type"),
        ("@label", "@type"),
        ("@attribute", "@keyword"),
        ("@conditional", "@keyword"),
        ("@storageclass", "@keyword"),
        ("@escape", "@string"),
        ("@text.title", "@function"),
        ("@text.literal", "@string"),
        ("@text.uri", "@string"),
        ("@text.reference", "@function"),
        ("@text.emphasis", "@keyword"),
        ("@text.strong", "@keyword"),
        ("@punctuation.special", "@operator"),
    ] {
        query = query.replace(from, to);
    }
    // The JSON grammar lists keys before generic strings; put the specific
    // capture last so it wins when both patterns target the same string node.
    if query.contains("key: (_) @type") {
        query.push_str("\n(pair key: (string) @type)\n");
    }
    query
}

fn inline_configuration() -> &'static HighlightConfiguration {
    static INLINE: OnceLock<HighlightConfiguration> = OnceLock::new();
    INLINE.get_or_init(|| {
        let mut config = HighlightConfiguration::new(
            tree_sitter_md::INLINE_LANGUAGE.into(),
            "markdown_inline",
            &normalize_query(tree_sitter_md::HIGHLIGHT_QUERY_INLINE),
            "",
            "",
        )
        .expect("bundled Markdown inline query");
        config.configure(&NAMES);
        config
    })
}

pub fn python(text: &str) -> Vec<Span> {
    highlight(Language::Python, text)
}

fn highlight(language: Language, text: &str) -> Vec<Span> {
    // Bound synchronous work; large cells remain fully editable as plain text.
    if text.len() > 256 * 1024 {
        return vec![];
    }
    let mut highlighter = Highlighter::new();
    let Ok(events) = highlighter.highlight(
        configuration(language),
        text.as_bytes(),
        None,
        |name| match name {
            "markdown_inline" => Some(inline_configuration()),
            "python" | "py" => Some(configuration(Language::Python)),
            "json" => Some(configuration(Language::Json)),
            "toml" => Some(configuration(Language::Toml)),
            "yaml" | "yml" => Some(configuration(Language::Yaml)),
            "sql" => Some(configuration(Language::Sql)),
            _ => None,
        },
    ) else {
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
    language: Option<Language>,
    source: String,
    spans: Rc<Vec<Span>>,
}
impl Cache {
    pub fn get(&mut self, language: Language, text: &str) -> Rc<Vec<Span>> {
        if self.source != text || self.language != Some(language) {
            self.language = Some(language);
            self.spans = Rc::new(highlight(language, text));
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
    fn file_languages_capture_literals_keys_and_markdown_injections() {
        for (language, text, expected) in [
            (
                Language::Yaml,
                "# settings\nname: café\nenabled: true\ncount: 42\nratio: 1.5\nmessage: |\n  hello world\ndefaults: &base\n  timeout: 30\ncopy: *base\n",
                vec![
                    ("# settings", "comment"),
                    ("name", "type"),
                    ("café", "string"),
                    ("true", "constant"),
                    ("42", "number"),
                    ("1.5", "number"),
                    ("hello world", "string"),
                    ("base", "type"),
                ],
            ),
            (
                Language::Sql,
                "-- report\nSELECT name, COUNT(*) FROM users WHERE active = true AND score > 1.5 AND name = 'café' GROUP BY name LIMIT 42;",
                vec![
                    ("-- report", "comment"),
                    ("SELECT", "keyword"),
                    ("COUNT", "function"),
                    ("true", "constant"),
                    ("1.5", "number"),
                    ("café", "string"),
                    ("42", "number"),
                ],
            ),
            (
                Language::Json,
                "{\"café\": true, \"count\": 42}",
                vec![("café", "type"), ("true", "constant"), ("42", "number")],
            ),
            (
                Language::Toml,
                "[project]\nname = \"café\"\nenabled = true\n# note\ncount = 42\n",
                vec![
                    ("project", "type"),
                    ("café", "string"),
                    ("true", "constant"),
                    ("# note", "comment"),
                    ("42", "number"),
                ],
            ),
            (
                Language::Markdown,
                "# Heading\n\nSome **bold** and `code` with [link](https://example.com).\n\n```python\ndef café():\n    return 42\n```\n",
                vec![
                    ("Heading", "function"),
                    ("bold", "keyword"),
                    ("code", "string"),
                    ("https", "string"),
                    ("def", "keyword"),
                    ("café", "function"),
                    ("42", "number"),
                ],
            ),
        ] {
            let spans = highlight(language, text);
            for (token, name) in expected {
                let offset = text.find(token).unwrap();
                assert!(
                    spans
                        .iter()
                        .any(|(range, style)| range.contains(&offset) && NAMES[*style] == name),
                    "{language:?}: {token}: {spans:?}"
                );
            }
            assert!(
                spans
                    .windows(2)
                    .all(|pair| pair[0].0.end <= pair[1].0.start)
            );
            assert!(
                spans
                    .iter()
                    .all(|(range, _)| text.is_char_boundary(range.start)
                        && text.is_char_boundary(range.end))
            );
        }
    }

    #[test]
    fn yaml_and_sql_fences_and_incomplete_edits_remain_highlighted() {
        for (language, source, token) in [
            (Language::Yaml, "name: \"unfinished", "name"),
            (Language::Sql, "select name from users where", "select"),
        ] {
            let spans = highlight(language, source);
            assert!(!spans.is_empty());
            let fence = if language == Language::Yaml {
                "yml"
            } else {
                "sql"
            };
            let markdown = format!("```{fence}\n{source}\n```\n");
            let offset = markdown.find(token).unwrap();
            assert!(
                highlight(Language::Markdown, &markdown)
                    .iter()
                    .any(|(r, _)| r.contains(&offset))
            );
        }
    }

    #[test]
    fn cache_reuses_source_but_invalidates_on_language_or_edit() {
        let mut cache = Cache::default();
        let first = cache.get(Language::Python, "True");
        assert!(Rc::ptr_eq(&first, &cache.get(Language::Python, "True")));
        assert!(!Rc::ptr_eq(&first, &cache.get(Language::Markdown, "True")));
        assert!(!Rc::ptr_eq(&first, &cache.get(Language::Python, "False")));
        for (path, language) in [
            ("src/demo.PY", Some(Language::Python)),
            ("README.md", Some(Language::Markdown)),
            ("pyproject.toml", Some(Language::Toml)),
            ("data.json", Some(Language::Json)),
            ("environment.yaml", Some(Language::Yaml)),
            ("workflow.YML", Some(Language::Yaml)),
            ("query.sql", Some(Language::Sql)),
            ("notes.txt", None),
            ("README", None),
        ] {
            assert_eq!(Language::for_path(path), language);
        }
        assert!(highlight(Language::Json, &" ".repeat(256 * 1024 + 1)).is_empty());
        assert!(!highlight(Language::Json, "{\"unfinished\": 42,").is_empty());
    }

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
