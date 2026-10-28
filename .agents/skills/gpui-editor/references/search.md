# Search & replace

**Contents:** [Matcher](#the-matcher) · [Session state](#session-state) · [Rendering matches](#rendering-matches) · [The panel UI](#the-panel-ui) · [Replace](#replace) · [Scaling notes](#scaling-notes)

## The matcher

Keep matching in a self-contained object over the rope, separate from editor state (gpui-component's `SearchMatcher`, `crates/base/src/input/editor/search.rs`):

```rust
pub struct SearchMatcher { /* Aho-Corasick automaton + results */ }
impl SearchMatcher {
    pub fn update(&mut self, text: &Rope);                      // re-index after edits
    pub fn update_query(&mut self, query: &str, case_insensitive: bool);
    pub fn matched_ranges(&self) -> Rc<Vec<Range<usize>>>;      // BYTE ranges, sorted
    pub fn current_match_index(&self) -> usize;
    pub fn label(&self) -> String;                              // "3/17" for the panel
    pub fn update_cursor_by_offset(&mut self, offset: usize);   // "next match" starts from the cursor
}
```

Scoping call worth copying: **literal matching first** (Aho-Corasick; case-insensitivity via the automaton), regex later or never — gpui-component ships literal-only, case-insensitive by default. If/when you add regex, run it over rope chunks with the `regex` crate's streaming API or materialize per visible chunk; keep the same `matched_ranges` interface so nothing downstream changes.

## Session state

One struct on the editor, driven by actions:

```rust
pub struct SearchSession {
    pub open: bool,
    pub replace_mode: bool,
    pub case_insensitive: bool,     // default true
    pub query: String,
    pub replacement: String,
    pub matcher: SearchMatcher,
}
```

Public methods → `open_search(replace_mode)`, `close_search()`, `set_search_query(..)`, `next_search_match() / previous_search_match() -> Option<Range<usize>>` (select + autoscroll to the returned range), `replace_current_search_match(..) -> bool`, `replace_all_search_matches(..) -> usize`. Bind `Search` to cmd-f/ctrl-f, `Replace` to cmd-shift-f/ctrl-h, Escape closes (returning focus to the editor), Enter/shift-Enter = next/previous while the panel has focus. Seed the query from the current selection when opening.

## Rendering matches

- All matches → a dedicated **decoration collection** (decoration.md): background tint per range, queried per visible line.
- The *current* match → a stronger style in the same pass, and it's what next/previous moves the selection to.
- **Scrollbar markers:** paint a thin mark in the scrollbar track at `row/total_rows × track_height` per match — cheap and high-value; Zed uses the same lane for diagnostics and git hunks.
- Recompute on edit: matcher `update(text)` after each change; debounce query-typing (~100 ms) on large documents.

## The panel UI

The find/replace panel is *two more instances of your own single-line input* (query + replacement) laid over the editor's top-right — gpui-component literally reuses its input component for its search panel, a nice dogfooding test of your core. Wire-up notes:

- Panel visibility from `session.open`; render as an overlay row, don't reflow the document (or do — pick one and keep scroll anchored).
- Focus moves to the query input on open; the editor's key context is no longer active, so editor bindings don't fire — Escape and Enter belong to the panel's context.
- Show `matcher.label()` ("3/17"), a case toggle, and (in replace mode) replace-one / replace-all buttons.
- Lazy-create the panel state on first open and tear it down on close (gpui-component keeps an overlay registry keyed by editor entity id).

## Replace

Replace is just the mutation funnel: `replace_current` = select the current match range, `replace_text_in_range(Some(range), replacement)`, advance to next; `replace_all` = apply ranges **in reverse order** (so earlier offsets stay valid) inside **one undo transaction** (undo.md), return the count. Respect read-only: replace controls disabled, find still works.

## Scaling notes

- Matching is O(document) but off the render path — run it on edit/query-change, not per frame; for very large docs move it to the background executor with the same debounce pattern as parsing (large-text.md §Budgets).
- `matched_ranges` being sorted makes "next from cursor" a binary search.
- Store the current match by **anchor** (large-text.md) so edits while the panel is open don't strand the "3/17" pointer; matches themselves are recomputed anyway.
