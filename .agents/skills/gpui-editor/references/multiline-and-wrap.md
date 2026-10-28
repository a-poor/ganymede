# Multi-line layout, soft wrap, movement, scrolling

**Contents:** [Multi-line extension](#multi-line-extension) · [Coordinate spaces](#coordinate-spaces-buffer-vs-display) · [Soft wrap](#soft-wrap-linewrapper) · [Vertical movement & goal columns](#vertical-movement--goal-columns) · [Word & line movement](#word--line-movement) · [Scrolling](#scrolling) · [Autoscroll](#autoscroll)

Prereq: core-editing.md. Reference implementation: `crates/gpui/examples/view_example/example_editor.rs` in the Zed repo (multi-line from scratch, one file).

## Multi-line extension

The single-line element from core-editing.md extends to multi-line in seven mechanical steps:

1. **Layout height** = `window.line_height() * line_count` (`content.split('\n').count().max(1)`), or viewport height + vertical scroll for documents.
2. **Shape one `ShapedLine` per logical line** in prepaint — only visible lines for documents (large-text.md):

```rust
let lines: Vec<ShapedLine> = content.split('\n').map(|line_str| {
    let text: SharedString = line_str.to_string().into();
    let run = TextRun { len: text.len(), font: style.font(), color, ..default_run() };
    window.text_system().shape_line(text, font_size, &[run], None)
}).collect();
```

3. **Cursor geometry:** offset → (row, col) via a scan for `'\n'` (or cached line-start offsets); `x = lines[row].x_for_index(col)`, `y = line_height * row`.
4. **Paint per line**, y-stepped: `line.paint(point(bounds.left(), bounds.top() + line_height * i as f32), line_height, ..)`.
5. **Multi-line selection** = one quad per row: first row `start_x → line end`, middle rows full width, last row `0 → end_x`. (Zed draws the rounded version as a single path from per-row `[start_x, end_x]` spans — `HighlightedRange::paint`, `crates/editor/src/element.rs`.)
6. **Hit testing:** `row = ((y - bounds.top()) / line_height) as usize` clamped, then `lines[row].closest_index_for_x(x - bounds.left())` + the row's start offset.
7. **Enter** → an `insert_newline` action; bind it only in the multi-line wrapper's key context, and the same editor core serves single-line fields too (the view_example does exactly this — `TextArea` wrapper binds Enter, `Input` wrapper doesn't).

## Coordinate spaces: buffer vs display

The moment soft wrap (or folding) exists, one buffer line ≠ one visual row. **Keep "buffer position" and "display position" as distinct Rust types** — `Point { row, column }` vs `DisplayPoint` — even if your only transform is wrapping. Zed's `DisplayPoint` is a newtype the compiler enforces; conversions thread a `Bias` (Left/Right) because display positions are many-to-one with buffer positions (a wrap boundary has two valid answers).

All movement functions should operate in **display space** (`fn(&Snapshot, DisplayPoint) -> DisplayPoint`) so they respect wrap/folds for free — that's how Zed's entire `movement.rs` is written. Distinguishing "column 0 of a wrapped continuation" from "real line start" requires converting down to buffer space (Zed's `saturating_left` does this check).

Zed's full pipeline generalizes this to five layers (inlay → fold → tab → wrap → block) with a uniform `sync` contract — see large-text.md. gpui-component ships a two-layer version (WrapMap → FoldMap) proving it scales down.

## Soft wrap: `LineWrapper`

`shape_line` never wraps. To soft-wrap, compute break points first, then shape one `ShapedLine` per **visual** row:

```rust
let mut wrapper = window.text_system().line_wrapper(style.font(), font_size);
let mut prev = 0;
for boundary in wrapper.wrap_line(&[LineFragment::text(line_str)], wrap_width) {
    let visual_row = &line_str[prev..boundary.ix];   // boundary.ix: byte offset
    prev = boundary.ix;                               // boundary.next_indent: hanging indent
    // shape_line(visual_row, ...) as usual
}
```

- `LineFragment::element(width, len)` interleaves fixed-width inline elements (inlay chips, fold placeholders) into the wrap computation.
- `wrapper.should_truncate_line(text, max_width, "…", TruncateFrom::End)` handles single-line truncation.
- Wrap width comes from layout: the element measures its text area in prepaint and **feeds the width back** into the wrap state, re-snapshotting if it changed (Zed: `editor.set_wrap_width(..)` inside `EditorElement::prepaint`). Gutter width participates: text width = element width − gutter − margins.
- Wrapping is the expensive part of edits on long lines. Zed runs it as an async background pass (`WrapMap` is an `Entity` with a background task yielding every 100 rows), marking snapshots `interpolated` until real wrap results land. A v1 can wrap synchronously per visible line; adopt async wrapping only when profiling says so.
- Wrap-continuation indent is a design choice: same-as-line-start (gpui-component's `WrappingIndent::Same` default) vs hanging indent via `boundary.next_indent`.
- This is exactly how Zed's `WrapMap` consumes the same API — `crates/editor/src/display_map/wrap_map.rs`.

Read-only wrapped text needs none of this: `div().w_full().child(text)` wraps, `.truncate()` / `.line_clamp(n)` clamp.

## Vertical movement & goal columns

Up/Down needs a **goal column stored in pixels, not characters** — tabs, ligatures, inlays, and proportional fonts make column ≠ x. The algorithm (Zed `movement.rs::up_by_rows`):

1. First vertical move: measure `goal_x = x_for(current position)` and store it on the selection (`SelectionGoal::HorizontalPosition(x)`).
2. Each subsequent up/down: target row = current ± 1 (clamped); column = `column_for_x(target_row, goal_x)`; **keep the stored goal** unchanged.
3. Any horizontal move clears the goal.
4. Up at the first row → position 0; Down at the last row → end.

Measuring x outside the paint pass: either use the prepaint cache (`last_layout` lines) when the row is visible, or re-shape just that row on demand (Zed bundles `text_system + style + rem_size` as `TextLayoutDetails` so movement code can call `layout_row` anywhere). PageUp/PageDown = up/down by `visible_line_count`, same goal mechanism.

## Word & line movement

- Word boundaries: `unicode_segmentation::split_word_bound_indices` for a v1. Zed goes further with a language-aware `CharClassifier` (`-` is a word char in CSS) — `movement.rs::{previous_word_start, next_word_end, previous_subword_start, next_subword_end}` (subword = camelCase/snake_case parts).
- Line begin/end should take `stop_at_soft_boundaries: bool`: Home goes to the *wrap-row* start first, pressed again → real line start. Also `indented_line_beginning` (Home toggles between first non-whitespace and column 0).
- Paragraph movement: scan for blank lines.

## Scrolling

Track vertical scroll and shape only visible rows (large-text.md §Virtualization). The non-obvious part is **what "scroll position" is stored as**. A raw row number jumps whenever text above the viewport changes (edit, fold, async wrap landing). Zed's fix — **anchor-based scroll** (`crates/editor/src/scroll.rs`), ~30 lines to copy:

```rust
pub struct ScrollAnchor {
    pub anchor: Anchor,               // stable text position ≈ top of viewport (see large-text.md §Anchors)
    pub offset: gpui::Point<f64>,     // fractional rows below the anchor; x is raw em units, not anchored
}
// resolve each frame: scroll_top_row = anchor.to_display_point(snapshot).row + offset.y
```

When text above the viewport changes, the anchor moves with the text and the viewport stays visually put. Also from their design: fractional row offsets (smooth scrolling), `scroll_beyond_last_line` (default: half a viewport of overscroll), trackpad axis-locking on the ongoing scroll gesture.

Scrollbar: thumb = `(visible / total) × track`, clamped to a minimum size; the track doubles as a marker lane (search matches, diagnostics — see search.md). Zed's minimap is literally a second editor entity rendered at a tiny font — skip it.

## Autoscroll

Keep the cursor visible after commands. **Where you resolve the request depends on who owns the scroll state:**

- **Zed's editor** owns scroll position as its own state feeding its own paint, so it queues a request (as a side effect of selection changes) and consumes it once per frame in prepaint where bounds/line_height are known.
- **A from-scratch editor inside a `ScrollHandle`-tracked scrollable div must NOT do that.** gpui silently drops `cx.notify()` (and `window.refresh()`) while a draw is in progress — `WindowInvalidator::invalidate_view` is gated on `draw_phase == DrawPhase::None` — so a `set_offset` from prepaint stores the offset but schedules **no frame**; the jump only appears at the next incidental redraw (an idle editor's cursor-blink tick, up to ~500ms later — reads as "cmd-Home/paste lands late"). The parent div has also already applied the *old* offset to your bounds this frame. Instead, resolve autoscroll **synchronously in handler context** (action/mouse handlers, `replace_text_in_range` — they run at `DrawPhase::None`, before the frame draws): compute the cursor's row from the cached wrap, adjust `scroll_handle.offset()`, done — the action's own frame renders at the destination, zero extra latency. Two gotchas: handler-context `window.text_style()` is the **root** style, not your element's — stash font/font_size (and line_height) in the layout snapshot at paint time and reuse them for handler-side wrap/shaping, or the wrap cache misses *and* disagrees with what's on screen; and keep a consume-in-prepaint fallback only for requests queued before the first paint. (`ScrollHandle::set_offset` doesn't clamp; the div clamps when applying at draw time, so overshooting a stale max is safe.)

Zed's strategies (`scroll/autoscroll.rs`):

- `Fit` — minimal scroll to fit **all** cursors (fall back to newest if they don't fit); scroll only if outside `[top + margin, bottom − margin]` (`vertical_scroll_margin`, "cursor surrounding lines")
- `Newest` — minimal scroll for the newest cursor (the default effect of most edits)
- `Center` / `Top` / `Bottom` — explicit positioning; `Next` cycles Center→Top→Bottom on repeated presses (the ctrl-l behavior)

Two details that prevent jank: the IME path uses *no-scroll* selection effects (composition updates shouldn't yank the viewport), and horizontal autoscroll runs after vertical, using the shaped lines' `x_for_index`. Horizontal scroll itself is a pure origin translate at paint time — **never a re-shape**.
