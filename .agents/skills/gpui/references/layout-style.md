# Layout & Styling

**Contents:** [Overview](#overview) · [Quick Start](#quick-start) · [Common Patterns](#common-patterns) · [Styling Methods](#styling-methods) · [h_flex / v_flex](#h_flex--v_flex-helpers) · [Tailwind Shorthands](#tailwind-style-shorthand) · [Overflow & Scroll](#overflow-and-scroll) · [Absolute Positioning](#absolute-positioning) · [Stacking Order](#stacking-order) · [Theme Integration](#theme-integration) · [Conditional Styling](#conditional-styling) · [Text Styling](#text-styling)

## Overview

GPUI provides CSS-like styling with Rust type safety.

**Key Concepts:**

- Flexbox layout system
- Styled trait for chaining styles
- Size units: `px()`, `rems()`, `relative()`
- Colors, borders, shadows

## Quick Start

### Basic Styling

```rust
use gpui::*;

div()
    .w(px(200.))
    .h(px(100.))
    .bg(rgb(0x2196F3))
    .text_color(rgb(0xFFFFFF))
    .rounded(px(8.))
    .p(px(16.))
    .child("Styled content")
```

### Flexbox Layout

```rust
div()
    .flex()
    .flex_row()  // or flex_col() for column
    .gap(px(8.))
    .items_center()
    .justify_between()
    .children([
        div().child("Item 1"),
        div().child("Item 2"),
        div().child("Item 3"),
    ])
```

### Size Units

```rust
div()
    .w(px(200.))           // Pixels
    .h(rems(10.))          // Relative to font size
    .w(relative(0.5))      // 50% of parent
    .min_w(px(100.))
    .max_w(px(400.))
```

## Common Patterns

### Centered Content

```rust
div()
    .flex()
    .items_center()
    .justify_center()
    .size_full()
    .child("Centered")
```

### Card Layout

```rust
div()
    .w(px(300.))
    .bg(rgb(0x1E1E2E))
    .rounded(px(8.))
    .shadow_md()
    .p(px(16.))
    .gap(px(12.))
    .flex()
    .flex_col()
    .child(heading())
    .child(content())
```

### Responsive Spacing

```rust
div()
    .p(px(16.))           // Padding all sides
    .px(px(20.))          // Padding horizontal
    .py(px(12.))          // Padding vertical
    .pt(px(8.))           // Padding top
    .gap(px(8.))          // Gap between children
```

## Styling Methods

### Dimensions

```rust
.w(px(200.))              // Width
.h(px(100.))              // Height
.size(px(200.))           // Width and height
.min_w(px(100.))          // Min width
.max_w(px(400.))          // Max width
```

### Colors

```rust
.bg(rgb(0x2196F3))        // Background
.text_color(rgb(0xFFFFFF)) // Text color
.border_color(rgb(0x000000)) // Border color
```

### Borders

```rust
.border(px(1.))           // Border width
.rounded(px(8.))          // Border radius
.rounded_t(px(8.))        // Top corners
.border_color(rgb(0x000000))
```

### Spacing

```rust
.p(px(16.))               // Padding
.m(px(8.))                // Margin
.gap(px(8.))              // Gap between flex children
```

### Flexbox

```rust
.flex()                   // Enable flexbox
.flex_row()               // Row direction
.flex_col()               // Column direction
.items_center()           // Align items center
.justify_between()        // Space between items
.flex_grow_1()              // Grow to fill space
```

## h_flex / v_flex Helpers

gpui-component provides shorthand helpers (import from `gpui_component`):

```rust
use gpui_component::{h_flex, v_flex};

// h_flex() = div().flex().flex_row().items_center()
h_flex()
    .gap_2()
    .child(icon)
    .child(label)

// v_flex() = div().flex().flex_col()
v_flex()
    .gap_4()
    .p_4()
    .child(input1)
    .child(input2)
    .child(submit_btn)
```

These come from the third-party **gpui-component** crate, not core gpui — only use them in projects that
depend on it. In a plain-gpui project, write `div().flex().flex_row().items_center()` (or define your own
one-line helpers).

## Tailwind-style Shorthand

GPUI provides Tailwind-style spacing/sizing shorthands:

```rust
// Spacing (0=0, 1=4px, 2=8px, 3=12px, 4=16px, ...)
.p_2()    // padding: 8px
.px_4()   // padding x: 16px
.py_3()   // padding y: 12px
.m_2()    // margin: 8px
.gap_3()  // gap: 12px

// Size
.size_full()   // width: 100%, height: 100%
.size_4()      // width: 16px, height: 16px
.w_full()      // width: 100%
.h_full()      // height: 100%
.flex_1()      // flex: 1 1 0 (fill remaining space)
.flex_shrink_0() // prevent shrinking
```

## Overflow and Scroll

```rust
div()
    .id("scroll-area")          // scroll state needs an ElementId
    .overflow_hidden()          // clip content
    .overflow_y_scroll()        // scroll on y axis
    .overflow_scroll()          // scroll both axes
```

See [list-scroll.md](list-scroll.md) for `ScrollHandle`, `uniform_list`, and the min-size gotchas that
make scrolling actually work.

## The min-size family (most common layout bug)

Flex items default to `min-width/min-height: auto` — they refuse to shrink below their content size.
Nearly every scroll or truncation failure traces to this:

- A scroll container that "pushes the layout down" instead of scrolling needs `.min_h_0()` on itself
  **and possibly on ancestor flex children** — the fix sometimes belongs on a parent row, not the
  container.
- Truncating text: `.text_ellipsis()` alone does nothing. Use `.truncate()` (bundles nowrap +
  overflow-hidden + ellipsis) **plus** `.min_w_0()` on the flex child so it's allowed to shrink.
- A flex item wrapping a single-line input grows to the text's intrinsic width (pushing siblings off the
  page) unless the wrapper gets `.min_w_0()`.
- The inverse: fixed-size children (icon next to truncating text, a titlebar, list rows) need
  `.flex_shrink_0()` or the flex algorithm squeezes them.
- Clickable/hoverable rows need `.w_full()` or their hit target and hover highlight shrink to content
  width.

## Hover, active & groups

```rust
div().id("row")                       // hover/active styling needs a stateful element
    .hover(|s| s.bg(rgb(0x333333)))
    .active(|s| s.opacity(0.8))

// Reveal a child when the mouse is anywhere over a parent "group":
div()
    .group("chat-row")                       // registers the group (a hitbox)
    .child(title)
    .child(
        menu_button
            .invisible()
            .group_hover("chat-row", |el| el.visible())
            .when(menu_open, |el| el.visible()),  // pin while its menu is open
    )
```

- Group names are **window-global**: every element sharing one name forms a single group, so repeated rows
  need per-row names (`format!("row-{id}")`) or all rows reveal together. The styled element doesn't have
  to be a descendant of the group.
- `svg()` does not inherit the parent's text color — mirror hover recoloring onto it via `group_hover`
  (see [text-styling.md](text-styling.md)).
- `.tab_group()` is an unrelated namesake (keyboard tab scoping), not a style group.

## Transforms & animation

Divs have **no** CSS-style `transform` — nothing on `Styled` translates/rotates/scales. What exists:

- `svg().with_transformation(Transformation::rotate(percentage(0.25)))` (also `translate`/`scale`).
- `TransformationMatrix` in low-level custom-element painting.
- Positional "translation": state + `.relative().left(px(...))` + `cx.notify()`.
- Animation re-applies styles per frame:
  `.with_animation(id, Animation::new(duration).with_easing(...), |el, delta| el.left(px(delta * 40.)))`
  — this is how Zed does slides/fades.

## Absolute Positioning

```rust
div()
    .relative()                 // position: relative (container)
    .child(
        div()
            .absolute()         // position: absolute
            .top_0()
            .right_0()
            .child("badge")
    )

// Inset helpers
div().absolute().inset_0()      // top/right/bottom/left: 0 (fill parent)
div().absolute().top(px(8.)).left(px(8.))
```

## Stacking Order

```rust
div()
    .relative()
    .child(content)
    .child(
        div()
            .absolute()
            .top_0()
            .right_0()
            .child("badge")
    ) // later children are typically painted above earlier siblings
```

GPUI's general `Styled` API does **not** provide a `z_index(...)` method.

For normal elements, stacking is usually controlled by:

- Parent/child composition
- Absolute positioning
- Render order of siblings (later siblings paint above earlier ones)

If you see a `z_index(...)` method in this repository, make sure it belongs to the specific component you are using. For example, `TileItem::z_index(...)` in the dock tiles system is a custom component API, not a general GPUI `Div` styling method.

## Theme Integration

`cx.theme()` is a **gpui-component** API — core gpui has no built-in theme. The core-gpui pattern is a
palette struct stored as a `Global`, read via `cx` in render (see [global.md](global.md)):

```rust
let style = MyStyle::get(cx);   // Global accessor; copy it out if you need &mut cx later
div()
    .bg(style.surface)
    .text_color(style.foreground)
    .border_color(style.border)
```

For deemphasized variants, derive alpha-tweaked colors inline rather than growing the palette:
`Rgba { a: style.muted_foreground.a * 0.5, ..style.muted_foreground }`.

## Conditional Styling

```rust
use gpui::prelude::FluentBuilder as _;

div()
    .when(is_active, |el| el.bg(style.primary))
    .when(!is_active, |el| el.opacity(0.5))
    .when_some(optional_color.as_ref(), |el, color| el.bg(*color))
```

## Text Styling

```rust
div()
    .text_sm()          // font-size: small
    .text_base()        // font-size: base
    .text_lg()          // font-size: large
    .font_bold()        // font-weight: bold
    .line_height_snug() // tighter line height
    .truncate()         // overflow: ellipsis, single line
    .whitespace_nowrap()
```
