# ElementId

`ElementId` is a unique identifier for a GPUI element. It is required for elements that need:
- Mouse event handling (`on_click`, `on_hover`, etc.)
- State storage via `window.use_keyed_state`
- Interaction tracking

## Making an Element Stateful

Call `.id()` on a `div()` to create a `Stateful<Div>`:

```rust
div().id("my-element")          // ElementId from &str
div().id(42usize)               // ElementId from usize
div().id(ElementId::from(idx))  // Explicit
```

The precise rule: plain divs (`InteractiveElement`) can take `.hover(...)` styling and raw mouse handlers
(`on_mouse_down` etc.), but **`.on_click`, `.active()`, `.tooltip()`, `.track_focus()`, scroll-state
tracking (`overflow_*_scroll`), and any hover state that must persist across frames require
`StatefulInteractiveElement`** — i.e. an `.id()`. The id is what keys the element's persistent state from
frame to frame (a common symptom of a missing/unstable id: hover or click state that never updates).

## Accepted Types

```rust
impl Into<ElementId> for &str      // "my-id"
impl Into<ElementId> for String    // String::from("my-id")
impl Into<ElementId> for usize     // 0, 1, 2, ...
impl Into<ElementId> for u64
impl Into<ElementId> for SharedString
impl Into<ElementId> for (&str, usize)  // ("row", ix) — name + integer tuples
```

Gotcha: tuples only combine a name with an **integer** — `(&str, SharedString)` is not convertible
(compile error). If your key is a string, pass the `SharedString` directly (`.id(chat_id.clone())`) or
format a combined string.

## Uniqueness Rules

IDs must be unique within the same **stateful parent's scope** — not globally. GPUI builds a `GlobalElementId` by chaining parent IDs:

```rust
div().id("app").child(
    div().id("list1").children(vec![
        div().id(1usize).child("Item 1"),  // GlobalId: ["app", "list1", 1]
        div().id(2usize).child("Item 2"),  // GlobalId: ["app", "list1", 2]
    ])
).child(
    div().id("list2").children(vec![
        div().id(1usize).child("Item 1"),  // GlobalId: ["app", "list2", 1] — no conflict
    ])
)
```

Items in different parent scopes can reuse simple IDs (integers, short strings).

## In Component Structs

Components always store `id: ElementId` and pass it in `new()`:

```rust
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    base: Stateful<Div>,
    // ...
}

impl Button {
    pub fn new(id: impl Into<ElementId>) -> Self {
        let id = id.into();
        Self {
            id: id.clone(),
            base: div().id(id),  // id applied to base
            // ...
        }
    }
}

impl RenderOnce for Button {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.base  // already has .id() applied
            .on_click(/* ... */)
    }
}
```

## Usage at Call Sites

```rust
// Use unique string IDs for named components
Button::new("save-btn").label("Save")
Button::new("cancel-btn").label("Cancel")

// Use index-based IDs in lists
for (i, item) in items.iter().enumerate() {
    div().id(i)  // unique within this parent
}

// Use descriptive IDs for debugging
Input::new("search-input")
Select::new("country-select")
```
