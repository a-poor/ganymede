# Scrolling & Lists

APIs verified against gpui source (`elements/div.rs`, `elements/uniform_list.rs`).

## Scrollable div recipe

```rust
div()
    .id("messages")               // scroll state is element state — requires an id
    .overflow_y_scroll()
    .track_scroll(&self.scroll)   // optional: ScrollHandle for programmatic control
    .flex_1()
    .min_h_0()                    // critical: without it the flex child grows instead of scrolling
    .children(rows)
```

The height must be constrained by the ancestor chain (`flex_1()`/`min_h_0()` all the way up, or a fixed
height). The most common "scrolling doesn't work" cause is a missing `.min_h_0()`/`.min_w_0()` on some
ancestor flex child — flex items default to `min-size: auto` and grow to content size instead of letting
the scroll container clip. The fix sometimes belongs on a *parent row*, not the scroll container itself.

## ScrollHandle

Create with `ScrollHandle::new()`, store on the entity, attach with `.track_scroll(&handle)`.

- `offset() -> Point<Pixels>` — **negative** as you scroll down (content moves up).
- `max_offset() -> Point<Pixels>` — positive scrollable extent.
- `scroll_to_item(ix)` / `scroll_to_top_of_item(ix)` — records an "active item" resolved during the next
  prepaint, so it's safe to call before the content has ever been laid out.
- `scroll_to_bottom()` — same deferred-flag mechanism; safe to call right after pushing new content.
- `bounds_for_item(ix) -> Option<Bounds<Pixels>>` — last-frame geometry.

### Gotchas

- `scroll_to_item(ix)` indexes the tracked container's **direct children**. A single wrapper div (e.g. a
  centering column holding all rows) silently makes the whole list "item 0". Either make each row a direct
  child of the tracked div, or track scroll on the inner column.
- A pending `scroll_to_item` and an auto-`scroll_to_bottom` in the same frame fight each other — suppress
  one.
- "Near bottom" check (remember offset is negative): `handle.max_offset().y + handle.offset().y <= px(8.)`.

### Stick-to-bottom while streaming

Read at-bottom-ness from the **last painted frame, before applying the new content**, then conditionally
re-pin after:

```rust
let was_at_bottom = self.scroll.max_offset().y + self.scroll.offset().y <= px(8.);
self.apply_event(event, cx);          // mutate content
if was_at_bottom {
    self.scroll.scroll_to_bottom();   // resolved at next layout
}
cx.notify();
```

## uniform_list — use it for long lists

A plain `overflow_y_scroll` div lays out **every** child each frame; a few hundred rows makes scrolling
visibly laggy. `uniform_list` lays out only the visible range:

```rust
uniform_list(
    "model-rows",                 // ElementId
    items.len(),
    move |visible_range: Range<usize>, _window, _cx| {
        visible_range.map(|ix| render_row(&items[ix], ix)).collect()
    },
)
.track_scroll(&self.list_scroll)  // takes a UniformListScrollHandle, not ScrollHandle
.flex_1()
```

- The closure is `'static`: capture owned/`Rc` copies of row data, cloned `Rc<dyn Fn>` handlers, and
  `Copy` colors — it cannot borrow `self`.
- Rows must be **uniform height** (it measures the first item and multiplies).
- The list scrolls itself. Give the *container* a fixed height or `flex_1()` + `min_h_0()`, and do **not**
  put `overflow_y_scroll` on an ancestor — scrolling lives inside the list.
- It has its own `UniformListScrollHandle` (with `scroll_to_item` etc.), distinct from `ScrollHandle`.

For variable-height virtualized content, gpui also has `list()`/`ListState` (`elements/list.rs`) — check
the source before using; it has its own item-measurement protocol.

## Manual scroll in a custom Element

When hand-painting (custom text editors etc.): keep `scroll_offset: Point<Pixels>` on the entity, mutate it
in `on_scroll_wheel` via `event.delta.pixel_delta(line_height)`, clamp against the shaped content size in
prepaint, and write the clamped value back during paint so the non-overflowing axis never drifts. Keep a
separate `scroll_to_cursor` flag set by *edits* (not by wheel) to drive scroll-into-view.
