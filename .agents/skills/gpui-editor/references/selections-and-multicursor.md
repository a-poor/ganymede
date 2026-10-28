# Selections, multi-cursor, collaboration

**Contents:** [Selection model](#the-selection-model) · [Select modes](#select-modes-wordlineall) · [Multi-cursor](#multi-cursor) · [Rendering many selections](#rendering-many-selections) · [Collaboration](#collaboration)

## The selection model

One selection (core-editing.md) generalizes cleanly if you start with the right shape — this is Zed's `Selection<T>` (`crates/text/src/selection.rs`), generic over coordinate type:

```rust
pub struct Selection<T> {
    pub id: usize,            // stable identity across edits (newest/oldest tracking)
    pub start: T,             // normalized: start <= end always
    pub end: T,
    pub reversed: bool,       // which end is the HEAD (the moving end)
    pub goal: SelectionGoal,  // pixel goal-x for vertical movement (multiline-and-wrap.md)
}
// head() = moving end, tail() = fixed end; set_head flips `reversed` on crossover
pub enum SelectionGoal { None, HorizontalPosition(f64), /* wrapped variants */ }
```

A collapsed selection is a caret. Keeping `goal` *on the selection* is what makes per-cursor vertical movement correct with multiple cursors.

## Select modes (word/line/all)

`MouseDownEvent.click_count`: 1 → character, 2 → word (`unicode_segmentation::split_word_bound_indices` around the offset), 3 → line, 4 → all. The subtle part: **remember the mode until mouse-up** — while a word-select drag is live, every drag extension snaps to word boundaries (re-apply the mode as a floor on both ends), and shift-click after a double-click extends by words. Zed: `SelectMode::{Character, Word(range), Line(range), All}` carried in a `pending` selection.

## Multi-cursor

The design that makes multi-cursor nearly free (Zed, `crates/editor/src/selections_collection.rs`) — adopt the shape even if you ship one cursor first:

```rust
pub struct SelectionsCollection {
    disjoint: Arc<[Selection<Anchor>]>,   // committed selections: sorted, non-overlapping, ANCHOR-stored
    pending: Option<PendingSelection>,    // the live mouse drag + its select mode
}
```

- **Anchor storage** (large-text.md §Anchors): selections survive edits from any source without manual patching. Reads resolve against a snapshot: `all::<Point>()`, `newest()`, `oldest()`.
- **The pending drag is merged at read time**, absorbing any committed selection it overlaps — nothing commits until mouse-up.
- **Every command is written once against a single selection** and applied to all via combinators; after the closure runs, the collection re-sorts, merges overlaps, and re-anchors:

```rust
// move = move_with + collapse_to; select = move_heads_with
pub fn move_left(&mut self, _: &MoveLeft, window: &mut Window, cx: &mut Context<Self>) {
    self.change_selections(Default::default(), window, cx, |s| {
        s.move_with(&mut |map, selection| {
            let cursor = if selection.is_empty() { movement::left(map, selection.start) }
                         else { selection.start };
            selection.collapse_to(cursor, SelectionGoal::None);
        });
    })
}
pub fn select_left(&mut self, ..) {
    self.change_selections(Default::default(), window, cx, |s| {
        s.move_heads_with(&mut |map, head, _| (movement::left(map, head), SelectionGoal::None));
    })
}
```

  Write `move_left` this way on day one and "add multi-cursor" later is: add cursors, done — every existing command already works N-fold.
- **`change_selections` is the single mutation gate**: it records selection history (for "undo restores selections" — undo.md), then applies effects — autoscroll by default, `no_scroll()` on the IME path.
- **Edits fan out**: collect `(range, text)` per selection, apply as one batched buffer edit, recompute selections from the edit deltas. Adding cursors: cmd-click (toggle a caret), "select next occurrence" (search for the current selection's text, add a selection at the next hit), cmd-shift-L (a selection per line of the current selection), column/rectangle select (a selection per row between drag start and end).

Movement functions live apart from selection code as pure functions `fn(&Snapshot, DisplayPoint, ..) -> DisplayPoint` in display space (multiline-and-wrap.md §Movement) — that separation is what keeps commands one-liners.

## Rendering many selections

- Cursor quads: one per selection head, only within the visible row range; skip the blink-hidden phase for all of them together.
- Selection quads: per selection, per row (multiline-and-wrap.md §5). Zed batches per *player* — `Vec<(PlayerColor, Vec<SelectionLayout>)>` — so local and remote selections paint identically with different colors, and draws each selection as one rounded path.
- Cursor shapes beyond the bar (block/underline/hollow for vim modes): block cursors shape the grapheme under the cursor and repaint it inverted on top of the block quad.

## Collaboration

What real-time collaborative editing actually requires — and why it's Zed's biggest complexity cliff:

- **The problem is anchors under concurrent edits.** The simple `(offset, bias)` anchor scheme breaks when a *remote* edit lands that your local anchors never saw in order. Zed's answer: text as a CRDT — every insertion gets a `(replica_id, lamport_timestamp)` id; the buffer is a tree of fragments; deletions are tombstones (fragments flipped invisible, never removed); an anchor is `(insertion id, offset within that insertion, bias)` — valid on every replica regardless of edit arrival order.
- Version vectors (`clock::Global`) answer "what has this replica seen"; `edits_since(version)` diffs two versions — also the mechanism display layers sync on.
- Remote presence = the same selection machinery: peers exchange anchor-based selection sets; render them as another `(PlayerColor, selections)` batch with name tags.
- Undo in a CRDT is op-based (counters flipping fragment visibility), so concurrent undos converge.

**Scope decision:** single-user → skip all of this (simple anchors, inverse-edit undo). Collaborative → the CRDT wants to be the buffer's core from the start (retrofitting is a rewrite); study `crates/text/src/text.rs` (`Fragment`, `Locator`, `UndoMap`) or use an existing CRDT crate (e.g. the automerge/loro/yrs family) as the document store beneath the same editor architecture. gpui-component doesn't attempt collaboration — consistent with its scope.
