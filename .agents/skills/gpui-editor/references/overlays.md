# Overlays: popovers, menus, and cursor-anchored UI

**Contents:** [Anchoring to text](#anchoring-ui-to-a-text-position) · [GPUI overlay machinery](#gpui-overlay-machinery) · [Completion menu](#completion-menu) · [Hover popovers](#hover-popovers) · [Context menu](#context-menu) · [Lifecycle & registry](#lifecycle--registry) · [Key handling while open](#key-handling-while-an-overlay-is-open)

Editor overlays — completion menus, hover cards, diagnostic popovers, find panels, context menus — are ordinary GPUI overlay UI with one editor-specific ingredient: **anchoring to a text position**.

## Anchoring UI to a text position

You already have the math from the IME work (core-editing.md §`bounds_for_range`): the screen rectangle of a byte range is

```rust
fn range_to_bounds(&self, range: &Range<usize>) -> Option<Bounds<Pixels>> {
    // from the prepaint cache: visible lines + element bounds + scroll offsets
    let (row, col)  = self.offset_to_row_col(range.start);
    let line        = self.visible_line(row)?;                       // None if scrolled out
    let x = self.bounds.left() + line.x_for_index(col) - self.scroll_x;
    let y = self.bounds.top() + self.line_height * (row as f32 - self.scroll_top_rows);
    Some(Bounds::new(point(x, y), size(/* … */, self.line_height)))
}
```

Expose it as a public method on the editor state (gpui-component does: `range_to_bounds`, plus `text_bounds()`, `line_height()`, `visible_row_range()` — all `Option`, `None` before first layout). Every overlay then anchors the same way: **below the rect, flipped above when there's no room, clamped to the window**. Reuse rules: completion menu → anchor at the *start of the word being completed* (not the cursor, so the menu doesn't jitter while typing); hover/diagnostic cards → the hovered range; comment threads (decoration.md) → the decorated range.

The OS IME candidate window is this same anchoring done *for* you — you just return the rect from `bounds_for_range`.

## GPUI overlay machinery

The rendering side is standard GPUI (see the main gpui skill's overlay reference for depth): `deferred(anchored().position(point).child(menu))` — `anchored()` positions an element at window coordinates with automatic flipping/clamping (`snap_to_window_with_margin`), and `deferred()` paints it after (over) the normal content. Render the overlay as part of the editor's element tree so it participates in the editor's lifecycle, and give it its own focus/key handling as needed.

Occlusion: `.occlude()` on the overlay stops mouse events leaking to the editor beneath; a click outside should close (mouse-down-out listener or a window-level dismiss).

## Completion menu

The interaction contract (both references converge on it):

- **Trigger:** after an insertion, ask the provider `is_completion_trigger(offset, new_text)` (typed `.`? identifier char?); if yes, debounce (~100–300 ms) then fetch async — `Task<Result<CompletionResponse>>` (lsp.md). Never block typing on it.
- **Filtering:** as the user keeps typing, filter/re-rank the already-fetched list against the word prefix locally (fuzzy match); re-query only when the trigger context changes.
- **The menu is a `uniform_list`** of items (icon + label + detail), selected index highlighted, scrolled to keep selection visible, capped height (~10 rows) and width.
- **Keys while open:** Up/Down move the selection, Tab/Enter accept, Escape dismisses — see §Key handling. Accepting = one transaction: replace the word-so-far range with the completion text (snippet placeholders later).
- **Dismiss on:** cursor moved out of the word, edit that invalidates the context, blur, Escape.
- Ghost-text ("inline completion") is a different surface: virtual text after the cursor (decoration.md §Inline widgets) with Tab-to-accept, alongside or instead of the menu.

## Hover popovers

- **Delayed trigger:** hover ≥ ~300 ms over a symbol/diagnostic range before fetching + showing; cancel on mouse-out (cancel-on-drop `Task`).
- Content is markdown-ish (LSP `Hover`) — render with your markdown element or plain styled text.
- Keep it open while the pointer is *inside the popover* (it may contain links/scroll); close when the pointer leaves both the range and the popover.
- Diagnostic popovers are the same surface fed from the diagnostic set at the hovered offset (`for_offset(o)` — lsp.md); when both hover info and a diagnostic exist, stack or merge them (Zed stacks).
- Cmd-hover underline + cmd-click for go-to-definition uses the same range machinery with a definition provider (lsp.md).

## Context menu

Right-click menu (Cut/Copy/Paste/Select All + feature entries like Go to Definition). Position at the mouse point, not a text anchor. Note from gpui-component's API: their menu-customization hook hands the app an **empty** menu rather than pre-filled defaults — if you offer customization, decide explicitly whether defaults are included (and document it). Right-click on an unselected position should move the cursor there first (platform convention), right-click inside a selection keeps it.

## Lifecycle & registry

- **Lazy state:** create menu/popover state on first open, drop on close. gpui-component keeps an overlay host per editor in a registry keyed by the editor's `EntityId`, torn down when nothing is open — n editors don't pay for n overlay hosts.
- **One overlay at a time** per kind, and define the precedence when several *could* show (completion menu suppresses hover; context menu suppresses both).
- Overlays live in **window/anchor coordinates computed each frame** — scrolling with an overlay open either re-anchors it (menus follow the text) or dismisses it (simpler; fine for v1). If the anchor row scrolls out of the visible range, dismiss.

## Key handling while an overlay is open

The editor and the menu compete for Up/Down/Enter/Escape. Two clean mechanisms:

1. **Key-context flag** (Zed): the editor's `key_context` gains a flag while a menu is open (e.g. `menu`, `showing_completions`), and bindings for `Up`/`Enter` in that context target menu actions — the keymap disambiguates, no code branching.
2. **Action interception**: the editor's action handlers check "menu open?" first and route (`if self.completion_menu.is_some() { return self.menu_up(cx) }`).

Either way: **Escape closes the topmost overlay only** (menu → search panel → clear selection… an explicit dismiss stack), and plain typing keeps flowing to the editor (the menu filters on the side) — focus stays on the editor for completion menus, unlike the find panel where focus genuinely moves to the query input (search.md).
