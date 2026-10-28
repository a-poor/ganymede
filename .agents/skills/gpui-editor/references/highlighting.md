# Syntax highlighting & styled text

**Contents:** [Styled runs](#styled-runs-the-primitive) · [Read-only shortcut](#read-only-shortcut-styledtext) · [The highlighter seam](#the-highlighter-seam) · [Tree-sitter pipeline](#the-tree-sitter-pipeline) · [Themes & capture names](#themes--capture-names) · [Language registry](#language-registry) · [Merging style layers](#merging-style-layers)

## Styled runs: the primitive

All inline styling — syntax colors, the IME composition underline, search-match backgrounds, rich-text spans — is expressed the same way: a line is shaped with a list of `TextRun`s that **tile the string contiguously in bytes**:

```rust
pub struct TextRun {
    pub len: usize,                          // BYTE length covered by this run
    pub font: Font,                          // family/weight/style (size is per shape_line call)
    pub color: Hsla,
    pub background_color: Option<Hsla>,
    pub underline: Option<UnderlineStyle>,   // { color, thickness, wavy } — wavy for squiggles
    pub strikethrough: Option<StrikethroughStyle>,
}
// invariant: sum(run.len) == line.len();  drop zero-length runs before shaping
let line = window.text_system().shape_line(text, font_size, &runs, None);
```

Example — the IME composition underline is just three runs (before / marked / after):

```rust
let runs = if let Some(marked) = marked_range {
    vec![
        TextRun { len: marked.start, ..base.clone() },
        TextRun { len: marked.end - marked.start,
                  underline: Some(UnderlineStyle { color: Some(base.color), thickness: px(1.), wavy: false }),
                  ..base.clone() },
        TextRun { len: text.len() - marked.end, ..base },
    ].into_iter().filter(|r| r.len > 0).collect()
} else { vec![base] };
```

So "add syntax highlighting" concretely means: *per visible line, convert styled spans into a run list, then shape as usual*. The rest of this file is about where the spans come from.

## Read-only shortcut: `StyledText`

For non-editable styled text, skip the custom element entirely:

```rust
StyledText::new("ABCD").with_highlights([
    (0..1, FontWeight::EXTRA_BOLD.into()),
    (2..3, FontStyle::Italic.into()),
])   // inside any div(); wrapping/clamping via the Styled DSL
```

## The highlighter seam

Don't couple the editor core to a parser. gpui-component's trait seam (`crates/base/src/input/editor/highlighting.rs`) is the design to copy — the editor talks to a trait, and tree-sitter / syntect / regex / server-side highlighters are interchangeable behind it:

```rust
pub trait InputHighlighter {
    fn language(&self) -> SharedString;
    /// Called on every edit (tree-sitter-shaped InputEdit: start_byte, old_end_byte,
    /// new_end_byte + row/col points) so the engine can update incrementally.
    fn update(&mut self, edit: Option<InputEdit>, text: &Rope, folding: bool,
              window: &mut Window, cx: &mut Context<EditorState>);
    /// Ordered, non-overlapping style runs fully covering `range` — queried PER VISIBLE RANGE.
    fn styles(&self, range: &Range<usize>, resolver: &dyn HighlightStyleResolver)
        -> Vec<(Range<usize>, HighlightStyle)>;
    fn fold_ranges(&self, text: &Rope) -> Vec<FoldRange>;   // folding piggybacks on syntax
}
pub type InputHighlighterFactory = Rc<dyn Fn(&str) -> Option<Box<dyn InputHighlighter>>>;

pub trait HighlightStyleResolver: Send + Sync {
    fn style(&self, name: &str) -> Option<HighlightStyle>;   // "keyword" → theme color
}
```

Three properties this buys (all proven in their code):

1. **Swappable engine** — their wasm build (no tree-sitter C grammars) drops in a syntect highlighter through the same trait.
2. **Names, not colors, in the engine** — styles resolve by capture name at paint time, so theme switches restyle without re-parsing.
3. **Incremental + budgeted** — see large-text.md §Budgets (2 ms sync / 256 KiB cap / 150 ms debounced background reparse).

## The tree-sitter pipeline

How the two references drive tree-sitter, high level:

- **Parse:** keep the tree; on edit, call `tree.edit(InputEdit)` then reparse incrementally (tree-sitter reuses unchanged subtrees). Zed additionally *interpolates* — shifts existing syntax-layer ranges immediately for cheap correctness — and lets the real reparse land async (`crates/language/src/syntax_map.rs`).
- **Injections:** a document is a stack of language layers (markdown → fenced rust block; html → script js). Zed's `SyntaxMap` is a tree of layers; gpui-component recomputes injection layers off-thread. Skippable for v1.
- **Query:** run the grammar's `highlights.scm` query over the visible range → captures with names like `"function"`, `"string.escape"`.
- **Resolve:** capture name → style. Zed builds a `HighlightMap` (capture index → `HighlightId`) once per (grammar, theme) — **re-theming never re-parses** — and streams `Chunk { text, highlight_id, .. }` slices to the renderer.

## Themes & capture names

Both references standardize on Zed's capture vocabulary and theme JSON format — adopt it and existing Zed themes work as-is. The ~41 names a highlighter should emit and a theme should resolve:

> attribute, boolean, comment, comment.doc, constant, constructor, embedded, emphasis, emphasis.strong, enum, function, hint, keyword, label, link_text, link_uri, number, operator, predictive, preproc, primary, property, punctuation, punctuation.bracket, punctuation.delimiter, punctuation.list_marker, punctuation.special, string, string.escape, string.regex, string.special, string.special.symbol, tag, tag.doctype, text.code.span, text.literal, title, type, variable, variable.special, variant

Theme-side, gpui-component's `HighlightTheme` also carries editor chrome tokens (`editor.background`, `editor.active_line.background`, `editor.line_number`, `editor.active_line_number`, `editor.invisible`, `editor.gutter.background`) plus a `syntax` map of name → `{ color, font_style, font_weight }` — a sensible schema to reuse.

## Language registry

Map language names to grammar + queries through a registry with alias support:

```rust
LanguageRegistry::singleton().register("navi", &LanguageConfig::new(
    "navi",
    tree_sitter_navi::LANGUAGE.into(),
    vec![],                              // injection languages
    tree_sitter_navi::HIGHLIGHTS_QUERY,  // highlights.scm
    "",                                  // injections.scm
    "",                                  // locals.scm
));
// aliases: "rs"→rust, "py"→python, "ts"→typescript …
// plain text = a config with no grammar → never parsed, editor still fully functional
```

(gpui-component: `crates/ui/src/highlighter/registry.rs`. Grammar crates are heavy — they gate each behind a cargo feature; a "plain" fallback keeps the editor working with no grammars compiled at all.)

## Merging style layers

Syntax colors are one of several style sources for the same text; the render path merges, in a fixed priority order per byte range:

```
syntax highlight  <  semantic tokens (LSP)  <  decorations (search, lint underlines)  <  selection/IME overlays
```

Zed streams this as chunk iterators — each display-map layer transforms `Chunk { text, syntax_highlight_id, diagnostic_severity, .. }` slices, and the element merges syntax + diagnostic + overlay styles while building runs. For a from-scratch editor: collect `(Range, HighlightStyle)` spans per visible line from each source, sort by start, split at overlap boundaries, merge with the priority rule, emit runs. See decoration.md for the decoration sources.
