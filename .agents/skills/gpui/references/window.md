# Window chrome & appearance

## Custom titlebar (macOS-first, cross-platform notes below)

```rust
WindowOptions {
    titlebar: Some(TitlebarOptions {
        appears_transparent: true,
        traffic_light_position: Some(point(px(9.), px(9.))),
        ..Default::default()
    }),
    ..Default::default()
}
```

Then draw your own bar:

- `.window_control_area(WindowControlArea::Drag)` on the bar keeps native drag behavior.
- Pad left (`.pl(px(80.))`) to clear the macOS traffic lights.
- Double-click to zoom: in the bar's `on_click`, check `event.click_count() == 2` and call
  `window.titlebar_double_click()`.
- Buttons *inside* the draggable bar must call `cx.stop_propagation()` in their handlers or their clicks
  also hit the bar's handlers.
- Windows: draw your own caption buttons tagged `WindowControlArea::Close`/`Min`/`Max`.
  Linux: `WindowDecorations::Client` + `window.start_window_move()`.

Known upstream macOS bug: with a transparent titlebar, the traffic lights vanish while the window is
unfocused (they reappear on hover) — `window_did_change_key_status` only repositions them when active.
App-side workaround: `cx.observe_window_activation(...)` → `window.set_traffic_light_position(...)`,
deferring one frame (`window.on_next_frame`) if it races.

## Reacting to OS appearance (light/dark)

Entity constructors have no `Window`, and appearance lives on the window. Pattern:

- Resolve the initial palette inside `open_window`'s callback (a real `Window` exists there) from
  `window.appearance()`.
- In the root view's first `render`, lazily create and store an `Option<Subscription>` from
  `window.observe_window_appearance(...)`; the callback re-resolves the palette (typically stored in a
  `Global`) and calls `cx.refresh_windows()` to repaint everything.

The same lazy-subscription-in-first-render trick applies to `cx.observe_window_activation` (e.g. pausing
a cursor-blink timer while the window is inactive).
