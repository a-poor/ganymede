# Overlays: menus, modals, tooltips

APIs verified against gpui source (`elements/deferred.rs`, `elements/anchored.rs`, `elements/div.rs`).

## Dropdown / context menu: `deferred(anchored(...))`

`deferred(child)` paints the child **after** (above) all siblings; `anchored()` positions it relative to
its logical location and can constrain it to the window. Together they let a menu escape parent clipping
and stacking:

```rust
.child(
    deferred(
        anchored()
            .anchor(Anchor::TopRight)            // which corner of the menu sits at the anchor point
            .offset(point(px(0.), px(4.)))
            .snap_to_window_with_margin(px(8.))  // keep fully on-screen
            .child(
                div()
                    .occlude()                    // block mouse from reaching what's underneath
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.menu_open = false;
                        cx.notify();
                    }))
                    .child(menu_items),
            ),
    )
)
```

- Open/closed state lives on the owning view (a `bool`/`Option` field), toggled with `cx.notify()`.
- `.on_mouse_down_out(...)` is the click-away dismissal hook.
- `.occlude()` blocks hover/click on everything the menu covers (sets `HitboxBehavior::BlockMouse`).

## Modal: last child of a `.relative()` root

No framework modal layer is needed. Render the modal conditionally as the **last child** of a `.relative()`
container so it paints above the page:

```rust
div()
    .relative()
    .size_full()
    .child(page_content)
    .when(self.confirm.is_some(), |el| {
        el.child(
            div()
                .id("modal-backdrop")
                .occlude()
                .absolute()
                .inset_0()
                .bg(rgba(0x00000088))
                .on_click(cx.listener(|this, _, _, cx| { this.confirm = None; cx.notify(); }))
                .child(
                    // the panel
                    div()
                        .occlude()
                        .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                        .child(panel_contents),
                ),
        )
    })
```

- Backdrop click cancels; the panel's own `.occlude()` + `stop_propagation` keep inner clicks from
  falling through to the backdrop's dismiss handler.
- A modal rebuilt every frame can be a stateless `RenderOnce` component (see component.md) — but any state
  that must survive frames (a filter input's text) belongs in a **caller-owned** `Entity` passed in.

## Tooltips

```rust
.tooltip(move |_window, cx| cx.new(|_| TextTooltip { text: text.clone() }).into())
```

The closure builds an `AnyView` — a tiny dedicated `Render` entity per tooltip works fine. Requires a
stateful element (`.id()`).

## Stacking in general

There is no `z_index()` in gpui's `Styled` API. Stacking is composition order: later siblings paint above
earlier ones; `.absolute()` children paint above in-flow content; `deferred()` escapes to after the whole
subtree. Reach for `deferred` only when normal sibling ordering can't express it (menus escaping clipped
ancestors).
