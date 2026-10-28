# Text decoration, gutter, and inline widgets

**Contents:** [Decoration collections](#decoration-collections) · [Gutter & line numbers](#gutter--line-numbers) · [Invisibles & indent guides](#invisibles--indent-guides) · [Folding](#folding) · [Inline widgets](#inline-widgets-text-that-isnt-text) · [Blocks: full-width widget rows](#blocks-full-width-widget-rows) · [Comments & annotations](#comments--annotations-a-composition)

"Decoration" covers everything drawn *about* the text without being the text: highlight ranges, underlines, gutter artifacts, inline chips, and injected widget rows.

## Decoration collections

The general mechanism (Monaco popularized it; gpui-component implements it in `crates/base/src/input/editor/decorations.rs`): a decoration is a byte range + a style, owned in named collections so independent features don't clobber each other:

```rust
pub struct TextDecoration { pub range: Range<usize>, pub style: HighlightStyle }

let search_hits = editor.create_decorations_collection(vec![...]);  // returns a HANDLE
search_hits.set(new_decorations);    // replace this collection's ranges
search_hits.append(more);
search_hits.clear();
```

Design rules worth copying:

- **Handle-based ownership.** Search, lint, "occurrences of symbol", and collaborator-selection features each own a collection handle and mutate only theirs. Collections layer in insertion order; first wins per conflicting style property.
- **Ranges track edits.** Store them as anchors (large-text.md §Anchors) with an explicit stickiness rule — gpui-component uses *never-grows-when-typing-at-edges* (typing at a decoration boundary doesn't extend it), which is the right default for search hits and lint squiggles.
- **Queried per visible range** at render time, merged into the run list by the priority order in highlighting.md §Merging.
- Styles are `HighlightStyle` — background for search hits, `UnderlineStyle { wavy: true, color }` for squiggles, strikethrough, color. Diagnostics severity → color mapping lives in the theme (lsp.md).

Zed's equivalent: `text_highlights` keyed by a `HighlightKey` (type-per-feature), plus `background_highlights` — same idea, key-per-feature instead of handle-per-feature. The IME composition underline is itself just a highlight under a dedicated key.

## Gutter & line numbers

The gutter is a second column painted left of the text, sharing the text's vertical geometry:

- **Sizing:** gutter width = width of the widest line number (digits of `line_count`) + padding; compute in prepaint *before* the text area, because text width (and therefore wrap width) = element width − gutter width. Zed bundles this as `GutterDimensions { left_padding, right_padding, width, margin }` and feeds the remainder into `set_wrap_width`.
- **Shape each visible number as its own `ShapedLine`**, right-aligned in the gutter, y = same `line_height * visual_row` as the text. With soft wrap, continuation rows get **no number** (skip rows whose display row doesn't start a buffer line); same for widget-block rows.
- Active-line number gets a distinct color (`editor.active_line_number` theme token); Zed also supports relative numbers (vim) computed from the cursor row, and gives each number its own hitbox (click → select line; hover → restyle).
- Gutter is also where fold arrows, diff marks, breakpoints, and comment indicators live — each is one more small element positioned at a row y. Reserve horizontal slots per feature.
- Active-line background: one full-width quad behind the text at the cursor row (`editor.active_line.background`), painted before text.

## Invisibles & indent guides

- **Whitespace glyphs:** pre-shape one `ShapedLine` for `·` and one for `→` at the current font, then stamp them at each space/tab x-position of visible rows (Zed shapes them once per frame, not per occurrence — `layout.space_invisible`/`tab_invisible`). Modes: always / only inside selections / off.
- **Indent guides:** vertical 1 px quads at `indent_level * tab_size * em_advance` for each visible row's leading-whitespace depth; active guide (containing the cursor) gets a stronger color.
- **Tab expansion:** display-time only — the buffer keeps `\t`; the display layer expands to the next tab stop (a tab's width depends on its column, which is why tab expansion is a display-map layer in both references — large-text.md).

## Folding

Folding is a *display transform*, not a decoration: folded ranges collapse to a placeholder chip ("⋯") and every downstream coordinate shifts. Implementation lives in the display-map layer (large-text.md §Layered map). Decoration-adjacent parts:

- Fold ranges come from the syntax seam (`fold_ranges`: named nodes spanning ≥ 2 rows — highlighting.md) or indentation analysis.
- Store folds as **anchor ranges** so they survive edits.
- The chip is an inline widget (below); the gutter arrow is a per-row gutter element toggling the fold.
- Clicking the chip unfolds; cursor movement into a fold should unfold or skip it (pick one; both references unfold on edit inside).

## Inline widgets: text that isn't text

Chips *inside* a line — fold placeholders, color swatches next to hex values, LSP inlay hints, mention pills in rich text. The pattern (Zed's `LineFragment`, `element.rs`):

A display line is a **sequence of fragments**: shaped text runs interleaved with sized elements.

```rust
enum LineFragment {
    Text(ShapedLine),
    Element { element: Option<AnyElement>, size: Size<Pixels>, len: usize },  // len = bytes it replaces (0 for pure insertions)
}
```

- Measure the widget in prepaint, give the wrapper its width (`LineFragment::element(width, len)` participates in soft-wrap computation — multiline-and-wrap.md).
- Painting walks fragments left to right, advancing x by `line.width` or `size.width`.
- `x_for_index` / `index_for_x` sum across fragments so cursor math and clicks still work.
- Virtual text (inlays) that occupies display space but no buffer space is the same idea upstream: a transform whose input length is 0 and output length is the hint text (Zed's `InlayMap`).

## Blocks: full-width widget rows

Rows *between* lines that aren't text at all — expanded diagnostics, review-comment threads, file headers in concatenated views. Zed's `BlockMap` (`display_map/block_map.rs`) is the reference:

```rust
enum BlockPlacement<T> { Above(T), Below(T), Near(T), Replace(Range<T>) }  // T: anchor
type RenderBlock = Box<dyn Fn(&mut BlockContext) -> AnyElement>;           // arbitrary GPUI content
```

- A block occupies N display rows at an anchored position; the display map shifts everything below.
- Height may not be known until the block's element is laid out — Zed re-runs prepaint (bounded depth) when block sizes change; simpler: fix block heights in line-height units.
- `Replace` blocks hide a range of buffer rows entirely (fold-with-widget).
- Gutter rows for a block show no line number.

This is heavyweight machinery — only take it when a feature genuinely needs full-width injected rows. For v1 of "comments", prefer the composition below.

## Comments & annotations: a composition

A Google-Docs-style commenting feature is not one mechanism but three existing ones:

1. **The commented range** → a decoration collection (background tint; anchor-stored so it tracks edits; stickiness: don't grow at edges).
2. **The indicator** → a gutter element on the range's first row (icon + count), or a right-margin lane.
3. **The thread UI** → either an overlay anchored to the range (overlays.md — click indicator → popover at `range_to_bounds`), or a `Below`-placed block row for an always-visible inline thread.

Store comment threads outside the text (id → anchor range + messages); the editor only renders the three surfaces above. Deleting all text under an anchor range leaves a collapsed range — decide whether the comment survives (attach to nearest position) or is orphaned (hide + list in a panel).
