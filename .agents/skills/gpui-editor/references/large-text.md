# Performance with large text

**Contents:** [Storage: rope](#storage-use-a-rope) · [Virtualization](#virtualization-shape-only-visible-lines) · [Anchors](#anchors-positions-that-survive-edits) · [The layered display map](#the-layered-display-map-contract) · [Incremental work budgets](#incremental-work-budgets) · [Calibration points](#calibration-points)

What separates a text *field* from a text *editor* is that none of the per-frame or per-edit work may scale with document size. Four techniques, in order of importance: rope storage, visible-range-only shaping, stable anchors, and (when you add folds/wrap/virtual text) the layered-map contract.

## Storage: use a rope

`String` + full re-scan is O(n) per edit and per coordinate query. A rope makes edits and offset↔(row, col) conversions O(log n).

- **Pragmatic choice:** the `ropey` crate (what gpui-component uses). Add an extension trait for the conversions an editor needs constantly: `offset_to_point`, `point_to_offset`, `line_start_offset`, `line_end_offset`, `line_len`, plus LSP `Position` and UTF-16 variants (lsp.md).
- **Zed's design** (`crates/rope/`): `Rope { chunks: SumTree<Chunk> }` — ≤128-byte leaves aggregated by a summary monoid:

```rust
pub struct TextSummary {
    pub len: usize, pub chars: usize, pub len_utf16: OffsetUtf16,
    pub lines: Point,                 // (rows, cols-in-last-line) = EOF position
    pub longest_row: u32, pub longest_row_chars: u32,   // → max horizontal scroll in O(1)
    /* first/last line lengths for cross-chunk stitching */
}
```

  Every conversion is one tree seek + a leaf query; `longest_row` in the summary means horizontal scroll extent never requires a scan. Zed's leaves also carry `u128` bitmaps (char starts, newlines, tabs) so leaf queries are bit-twiddling — a constant-factor optimization you can skip.
- Keep **cursor/selection as byte offsets** into the rope; line/character positions are derived presentation (see lsp.md for the conversion zoo).
- Offer both read APIs and document the cost: `text() -> &Rope` (zero-copy, use everywhere) and `value() -> SharedString` (materializes — occasional use only).

## Virtualization: shape only visible lines

The single highest-leverage rule: **per-frame cost is O(viewport), never O(document)**.

- Compute `visible_range: Range<row>` from scroll position + viewport height in prepaint — clipped by the parent's content mask, so an editor embedded in a scrolling list only shapes rows that are truly on screen (Zed does this via `window.content_mask().bounds`).
- Shape only those rows into the layout cache (`Vec<ShapedLine>` + the range they cover); the mouse/IME hit-testing described in core-editing.md indexes into this cache with `row - visible_range.start`.
- Query highlighter styles, decorations, and diagnostics **per visible range**, not per document (highlighting.md, decoration.md).
- Never re-shape to scroll: vertical scroll changes `visible_range`; horizontal scroll subtracts a pixel offset from paint origins.
- Gutter line numbers are shaped per visible row too (decoration.md §Gutter).
- Cap pathological lines: Zed truncates shaping at a max line length so a one-line 10 MB file can't stall a frame.

Both references do exactly this: gpui-component's `calculate_visible_range` (`crates/base/src/input/base/element.rs` — *"The lines only contains the visible lines in the viewport"*), Zed's `visible_display_row_range` in `EditorElement::prepaint`.

## Anchors: positions that survive edits

Everything that outlives one edit — selections, scroll top, decorations, folds, diagnostics, bookmarks — must not be a raw offset, or every edit invalidates it.

**Simple scheme (single-user; use this):** store `(offset: usize, bias: Bias)` and patch all live anchors on every edit (an edit at `[start, end) → new_len` shifts anchors after it; an anchor *at* the edit point resolves by bias — `Left` sticks to preceding text, `Right` to following). Adequate, small, and it's what gpui-component effectively does with its decoration stickiness rules.

**Zed's scheme:** an anchor is `(insertion op id, offset within that insertion, bias)` resolved on demand against a CRDT of text fragments (deletes are tombstones, not removals). This buys anchors that stay valid across **concurrent remote edits** — i.e. collaboration (selections-and-multicursor.md §Collaboration). It is the single biggest complexity cliff in Zed's text stack; don't pay it for a single-user editor.

Either way, the payoff compounds: anchor-based scroll position (multiline-and-wrap.md §Scrolling) kills "the view jumped" bugs; anchor-based decorations track edits for free (decoration.md); anchor-based selections survive edits from any source (selections-and-multicursor.md).

## The layered display map contract

When display-transforming features arrive — soft wrap, tab expansion, **folding**, virtual/inlay text, injected widget rows — resist ad-hoc coordinate math. Zed's design (`crates/editor/src/display_map.rs`, module doc at lines 1–70) makes each transform a layer with a uniform contract:

```rust
// each layer:
fn sync(lower_snapshot, Vec<Edit<LowerCoord>>) -> (OwnSnapshot, Vec<Edit<OwnCoord>>)
// plus: its own coordinate newtype, a chunk iterator, and point-conversion both ways (threading Bias)
```

```
buffer → InlayMap → FoldMap → TabMap → WrapMap → BlockMap → display
         virtual    folds ⇒    \t ⇒     soft      full-width
         text       "⋯"        spaces   wrap      widget rows
```

Notes per layer, briefly: inlay transforms have empty input but non-empty output (text from nowhere); folds are stored as anchor ranges so they survive edits; tab expansion must widen incoming edits to cover later tabs on the same line (a tab's width depends on its column); wrap is the only async layer (needs font measurement); blocks inject non-text rows (decoration.md §Blocks). Edits flow up the chain as *invalidation ranges*; snapshot types `Deref` down the chain so `snapshot.line_len(row)` resolves at whichever layer defines it.

**Scaling guidance:** v1 needs only one layer (wrap, with tab expansion folded in) — but shape that one layer to the `sync` contract, and adding folds or inlays later is mechanical instead of a rewrite. gpui-component's two-layer version (`Buffer → WrapMap → FoldMap → DisplayMap` facade, with a prefix-sum cache for O(1) row lookups and identity/no-alloc fold mapping when nothing is folded) is the scaled-down existence proof.

## Incremental work budgets

Numbers from shipping implementations, reusable as-is:

- **Syntax reparse** (gpui-component's tree-sitter adapter): parse synchronously only if doc ≤ 256 KiB *and* it finishes within a 2 ms budget; otherwise record the edit and reparse on the **background executor** after a 150 ms debounce, cancel-on-drop. Zed: interpolate existing syntax layers' ranges immediately (cheap), full incremental reparse on a background task.
- **Undo grouping**: coalesce edits < ~300 ms apart (undo.md).
- **Async soft wrap** (Zed): background task, yield every 100 rows, snapshots marked `interpolated` until results land.
- **Theme changes never re-parse**: resolve highlight *names* → colors at paint time (highlighting.md).

## Calibration points

- gpui-component's editor — rope + visible-range shaping + a two-layer display map, **no** SumTree-of-everything — states its ceiling in a doc comment: *"handles large text up to about 50K lines."* That's what the simple architecture buys.
- Zed's full stack (summary-tree rope, anchor CRDT, five-layer map, background wrap/parse) is what "open a 1 GB log file" requires.
- Decide your target early; the jump from the first architecture to the second is a rewrite of the coordinate layer, not an increment.
