---
name: gpui
description: GPUI framework knowledge covering actions/keybindings, async/background tasks and tokio interop, context management (App/Window/Context<T>/AsyncApp), custom elements (low-level Element trait), entity state management, event system, focus handling, global state, layout/styling (flexbox/CSS-like), scrolling and lists (ScrollHandle/uniform_list), overlays (deferred/anchored/modals), text/fonts/SVG/assets, RenderOnce components, window chrome, and testing. Use when working with any GPUI framework concept, building GPUI applications, or needing guidance on GPUI-specific APIs and patterns.
---

## Project setup

Depend on gpui via the Zed git repo. `gpui_platform` ships with **no default features**, so the
platform features must be enabled explicitly. `font-kit` enables the macOS text system — without
it the app runs but renders no text. `wayland` and `x11` enable the Linux backends (no-ops on
macOS, keep them for portability):

```toml
[dependencies]
gpui_platform = { git = "https://github.com/zed-industries/zed.git", features = ["font-kit", "wayland", "x11"] }
```

## Navigation

Load the relevant reference file based on the task:

| Topic | File | When to load |
|-------|------|--------------|
| Actions & keybindings | [action.md](references/action.md) | `actions!`, `bind_keys`, `on_action`, `key_context` |
| Async, background tasks & tokio | [async.md](references/async.md) | `cx.spawn`, `background_spawn`, `Task`, debounce, tokio interop |
| Context management | [context.md](references/context.md) | `App`, `Window`, `Context<T>`, `AsyncApp` |
| Custom elements (low-level) | [element.md](references/element.md) | `Element` trait, `request_layout`, `prepaint`, `paint` |
| Entity state | [entity.md](references/entity.md) | `Entity<T>`, `WeakEntity`, state management |
| Events & subscriptions | [event.md](references/event.md) | `cx.emit`, `cx.subscribe`, `cx.observe`, `EventEmitter` |
| Focus & keyboard nav | [focus-handle.md](references/focus-handle.md) | `FocusHandle`, `track_focus`, `tab_index`, focus-on-render |
| Global state | [global.md](references/global.md) | `Global` trait, `cx.set_global`, app-wide config |
| Layout & styling | [layout-style.md](references/layout-style.md) | `div()`, flexbox, `min_w_0`, truncation, hover/groups, overflow |
| Scrolling & lists | [list-scroll.md](references/list-scroll.md) | `ScrollHandle`, `uniform_list`, stick-to-bottom, virtualization |
| Overlays | [overlay.md](references/overlay.md) | `deferred`, `anchored`, modals, menus, tooltips, stacking |
| Text, fonts, SVG & assets | [text-styling.md](references/text-styling.md) | `StyledText`, highlights, `add_fonts`, `svg()`, `AssetSource` |
| Components | [component.md](references/component.md) | `RenderOnce` vs `Render`, handler props, borrow idioms |
| Window chrome | [window.md](references/window.md) | `TitlebarOptions`, `WindowControlArea`, appearance/dark mode |
| ElementId | [element-id.md](references/element-id.md) | `ElementId`, `.id()`, uniqueness rules, stateful elements |
| Testing | [test.md](references/test.md) | `#[gpui::test]`, `TestAppContext`, `VisualTestContext` |

## Ground truth

When the project pins gpui via the Zed git repo, the vendored checkout under
`~/.cargo/git/checkouts/zed-*/<rev>/crates/gpui/src/` is ground truth — verify any nontrivial API
signature there (grep) before answering; several of this skill's own past errors came from guessing.

## Extended References

For deep-dive topics, additional reference files are available:

**Element trait:**
- [element-api.md](references/element-api.md) — complete API, hitbox system, event handling
- [element-patterns.md](references/element-patterns.md) — text, interactive, container, composite patterns
- [element-examples.md](references/element-examples.md) — full examples: text, interactive, complex elements
- [element-best-practices.md](references/element-best-practices.md) — performance, state, common pitfalls
- [element-advanced.md](references/element-advanced.md) — masonry/circular layouts, async updates, virtual lists

**Entity management:**
- [entity-api.md](references/entity-api.md) — complete Entity API, methods, lifecycle
- [entity-patterns.md](references/entity-patterns.md) — model-view, cross-entity communication, observer
- [entity-best-practices.md](references/entity-best-practices.md) — memory, performance, lifecycle, re-entrancy

**Testing:**
- [test-examples.md](references/test-examples.md) — testing examples and patterns
- [test-reference.md](references/test-reference.md) — complete testing API reference
