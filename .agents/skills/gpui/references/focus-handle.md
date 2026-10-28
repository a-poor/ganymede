# Focus & Keyboard Navigation

**Contents:** [Overview](#overview) · [Quick Start](#quick-start) · [Focus Events](#focus-events) · [Keyboard Navigation](#keyboard-navigation) · [Common Patterns](#common-patterns) · [Best Practices](#best-practices)

## Overview

GPUI's focus system enables keyboard navigation and focus management.

**Key Concepts:**
- **FocusHandle**: Reference to focusable element
- **Focus tracking**: Current focused element
- **Keyboard navigation**: Tab/Shift-Tab between tab stops — opt-in via `.tab_index()`, not automatic
- **Focus events**: `cx.on_focus` / `cx.on_blur` subscriptions (registered through `Context<T>`, not element methods)

## Quick Start

### Creating Focus Handles

```rust
struct FocusableComponent {
    focus_handle: FocusHandle,
}

impl FocusableComponent {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
        }
    }
}
```

### Making Elements Focusable

```rust
impl Render for FocusableComponent {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_enter))
            .child("Focusable content")
    }

    // cx.listener callbacks take four arguments: (this, event, window, cx)
    // (Context::listener, app/context.rs:252)
    fn on_enter(&mut self, _: &Enter, _window: &mut Window, cx: &mut Context<Self>) {
        // Handle Enter action when focused
        cx.notify();
    }
}
```

### Focus Management

```rust
impl MyComponent {
    // FocusHandle::focus takes the window and app context (window.rs:541);
    // it is sugar for window.focus(&handle, cx) (window.rs:2036)
    fn focus(&mut self, window: &mut Window, cx: &mut App) {
        self.focus_handle.focus(window, cx);
    }

    // is_focused reads from the Window, not the App (window.rs:545)
    fn is_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    // Remove focus from everything in the window (window.rs:2060)
    fn blur(&mut self, window: &mut Window) {
        window.blur();
    }

    // The currently focused handle, if any (window.rs:2030)
    fn focused(&self, window: &Window, cx: &App) -> Option<FocusHandle> {
        window.focused(cx)
    }
}
```

**Borrow gotcha:** `view.read(cx).focus_handle.focus(window, cx)` is E0502 — `cx` is still immutably borrowed by `read` when `focus` needs it mutably. Clone the handle into a local first:

```rust
let handle = view.read(cx).focus_handle.clone();
handle.focus(window, cx);
```

## Focus Events

`on_focus`/`on_blur` are not builder methods on `div()` — they are subscriptions registered through `Context<T>` (`Context::on_focus` app/context.rs:547, `Context::on_blur` app/context.rs:596). They need a `&mut Window`, so they are typically set up in a constructor that receives one. Keep the returned `Subscription`s alive (store them or the listener dies).

### Handling Focus Changes

```rust
struct MyInput {
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl MyInput {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let _subscriptions = vec![
            cx.on_focus(&focus_handle, window, |this, _window, cx| {
                // Focus gained
                cx.notify();
            }),
            cx.on_blur(&focus_handle, window, |this, _window, cx| {
                // Focus lost
                cx.notify();
            }),
        ];
        Self {
            focus_handle,
            _subscriptions,
        }
    }
}

impl Render for MyInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_focused = self.focus_handle.is_focused(window);

        div()
            .track_focus(&self.focus_handle)
            .when(is_focused, |el| el.bg(rgb(0x2a2a2a)))
            .child(self.render_content())
    }
}
```

Related variants (all on `Context<T>`): `on_focus_in`/`on_focus_out` fire when the handle *or any descendant* gains/loses focus; `on_focus_lost(window, ...)` fires when nothing in the window has focus (e.g. the focused node was removed) so you can restore a default.

## Keyboard Navigation

### Tab Order

Tab navigation is **not** automatic. `track_focus()` only makes the element focusable and applies focused styles — it does not put it in the tab order. Opt in with `.tab_index(isize)` (elements/div.rs:762-772), and move focus with `window.focus_next()` / `window.focus_prev()` (window.rs:2079/2090), typically from Tab/Shift-Tab action handlers.

```rust
div()
    .child(div().track_focus(&focus1).tab_index(0)) // tab stop 1
    .child(div().track_focus(&focus2).tab_index(1)) // tab stop 2
    .child(div().track_focus(&focus3).tab_index(2)) // tab stop 3
```

- `.tab_index(isize)` sets the ordering and marks the element focusable and a tab stop.
- `.tab_stop(false)` keeps the element in tab-index order but unreachable via keyboard — useful for containers: focus the container, then `window.focus_next(cx)` focuses the first tab stop inside it.
- `.tab_group()` gives a subtree its own place in the tab order while its children's tab indices restart at 0, so you can reorder within the group without renumbering the whole app.

```rust
// e.g. in an action handler
fn on_tab(&mut self, _: &Tab, window: &mut Window, cx: &mut Context<Self>) {
    window.focus_next(cx);
}
```

### Focus Within Containers

```rust
impl Container {
    fn focus_first(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(first) = self.children.first() {
            // Clone the handle out before calling focus (borrow gotcha above)
            let handle = first.read(cx).focus_handle.clone();
            handle.focus(window, cx);
        }
    }
}
```

## Common Patterns

### 1. Auto-focus on First Render

Entity constructors usually have no `&mut Window`, and `focus` needs one — there is no `on_mount` hook. Store a flag and consume it at the top of `render`, where a `Window` exists (live examples: `src/pages/chat.rs` and `src/pages/skills.rs` in this repo):

```rust
struct MyDialog {
    focus_handle: FocusHandle,
    focus_on_render: bool,
}

impl MyDialog {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            focus_on_render: true,
        }
    }
}

impl Render for MyDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_on_render {
            self.focus_on_render = false;
            window.focus(&self.focus_handle, cx);
        }
        div()
            .track_focus(&self.focus_handle)
            .child(self.render_content())
    }
}
```

### 2. Focus Trap (Modal)

```rust
impl Modal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                // KeyDownEvent carries a `keystroke: Keystroke` (interactive.rs:25);
                // the key name is keystroke.key: String
                if event.keystroke.key == "tab" {
                    // Cycle within the modal's tab stops
                    if event.keystroke.modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .child(self.render_content())
    }
}
```

### 3. Conditional Focus

Same flag technique: set the flag when the condition flips, consume it in `render`.

```rust
impl Searchable {
    fn activate_search(&mut self, cx: &mut Context<Self>) {
        self.search_active = true;
        self.focus_on_render = true;
        cx.notify();
    }
}

impl Render for Searchable {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.search_active && self.focus_on_render {
            self.focus_on_render = false;
            window.focus(&self.focus_handle, cx);
        }
        div()
            .track_focus(&self.focus_handle)
            .child(self.search_input())
    }
}
```

## Best Practices

### ✅ Track Focus on Interactive Elements

```rust
// ✅ Good: Track focus for keyboard interaction
div()
    .track_focus(&self.focus_handle)
    .on_action(cx.listener(Self::on_enter))
```

### ✅ Provide Visual Focus Indicators

```rust
let is_focused = self.focus_handle.is_focused(window);

div()
    .when(is_focused, |el| el.border_color(rgb(0x528bff)))
```

### ❌ Don't: Forget to Track Focus

```rust
// ❌ Bad: No track_focus, keyboard navigation won't work
div()
    .on_action(cx.listener(Self::on_enter))
```
