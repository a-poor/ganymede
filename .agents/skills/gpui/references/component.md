# Components: Render vs RenderOnce

## Choosing

- **`Render` (entity views)** — anything with state that must survive frames: pages, text inputs, anything
  subscribed to events. Created with `cx.new(...)`, addressed as `Entity<T>`.
- **`RenderOnce` (stateless components)** — display components rebuilt every frame: rows, cards, buttons,
  modals whose state lives elsewhere. Construction is cheap (`SharedString`/`Arc` clones), so building
  them per-frame is idiomatic — this matches Zed's own `ui` crate conventions.

## The RenderOnce pattern

```rust
#[derive(IntoElement)]
pub struct ChatRow {
    id: ElementId,
    title: SharedString,
    on_click: Option<Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
}

impl ChatRow {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self { /* ... */ }
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for ChatRow {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .id(self.id)
            .when_some(self.on_click, |el, f| el.on_click(move |ev, w, cx| f(ev, w, cx)))
            .child(self.title)
    }
}
```

Key points:

- `render` **consumes `self`** and gets `&mut App`, not `Context<Self>` — no `cx.listener`, no
  `cx.notify`. To reach back into an entity, capture `cx.entity()` (or a clone of the `Entity`) in the
  handler and call `entity.update(cx, |this, cx| { ...; cx.emit(...); cx.notify(); })`.
- Handler props: `Box<dyn Fn>` for a single use; **`Rc<dyn Fn>` when the same handler is cloned into
  several rows/menu items** — a `Box` moves into the first closure and rows 2+ are a use-after-move
  compile error. `ClickEvent` carries no payload; per-row data flows only by closure capture.
- Attach optional handlers with `.when_some(...)`, configure builder-style (`with_*` methods).
- State that must survive the component being rebuilt every frame (a filter input's text, scroll position)
  belongs in a **caller-owned `Entity`** passed in, not in the RenderOnce struct.
- Read shared style/config from a `Global` via `cx` rather than threading it through props.

## Borrow idioms

- Copy a `Global` out before code needs `&mut cx` again: `let style = *MyStyle::get(cx);` (or clone) —
  the getter borrows `cx` immutably.
- Clone a `FocusHandle` to a local before focusing: `view.read(cx).focus_handle.focus(window, cx)` is
  E0502 (immutable + mutable borrow of `cx`).
- In `render`, clone `Arc`/`Rc`-shared collections out of `self` before iterating if the loop body needs
  `&mut self` (e.g. to fill a cache).
