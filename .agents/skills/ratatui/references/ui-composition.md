# UI Composition, Input, and Text

Use this reference for layout, widgets, focus, forms, popups, text wrapping, Unicode, styling, or custom rendering.

## Compose From the Supplied Area

Every renderer receives a `Rect`. Treat it as the whole available world:

- Do not assume origin `(0, 0)`.
- Use nested layouts and `Block::inner` instead of unchecked coordinate arithmetic.
- Guard zero width/height before direct buffer access.
- Use saturating addition/subtraction and clamp custom-widget iteration to `buf.area`.
- Render an intentional compact/fallback state when required chrome does not fit.

For a fixed number of regions, version-compatible `Layout::vertical`/`horizontal` with array destructuring is clear. Use dynamic splitting when region count varies. In releases with `areas`, the requested array length must match the constraints; use a fallible alternative when it may not.

Remember that `Percentage` and `Ratio` apply to the parent, not to the remainder after fixed constraints. Fixed chrome plus `Fill`/`Min` content is often more predictable. Avoid contradictory constraints.

See the [layout concepts](https://ratatui.rs/concepts/layout/) and version-matched [`Layout`](https://docs.rs/ratatui/latest/ratatui/layout/struct.Layout.html) docs.

## Widgets and Persistent State

Widget values are usually rebuilt each frame. Keep these in application/component state:

- selected row/item/tab
- table/list/scrollbar state and offsets
- text value, grapheme cursor, selection
- focus and interaction mode
- expanded/collapsed state
- cached wrapping or expensive derived view data

For reusable immutable widgets, the stable default is often `Widget for &MyWidget`. Use `StatefulWidget` when render state such as selection/offset persists independently. `WidgetRef` and `StatefulWidgetRef` are unstable/feature-gated in current releases; do not enable them casually.

Custom widgets should render only inside `area.intersection(buf.area)` and should tolerate tiny rectangles. Prefer Ratatui's buffer/string APIs over manual cell loops when they already handle clipping and grapheme width.

See [widget concepts](https://ratatui.rs/concepts/widgets/).

## Z-Order and Popups

Render from back to front. Many widgets do not paint every cell, so a popup can leave background glyphs visible. A common order is:

1. Base screen.
2. Optional dimming/background treatment.
3. `Clear` over the popup rectangle.
4. Popup block and content.

Store the active overlay in state. Give it first refusal on input. Test partially off-center and very small screens; popup sizing and centering must not underflow.

See Ratatui's [overwrite-regions recipe](https://ratatui.rs/recipes/render/overwrite-regions/).

## Focus and Input Routing

Represent focus explicitly with an enum or stable component ID. Route in this order unless product semantics dictate otherwise:

1. Active modal/overlay.
2. Focused editor/control.
3. Active screen/component.
4. Global shortcuts.

This prevents global commands from stealing text input and prevents hidden layers from responding through a modal.

Define key bindings in terms of intent, not scattered state mutation. Decide how `Press`, `Repeat`, and `Release` are handled. Keep paste separate. Make focus visible through more than color alone and provide predictable keyboard traversal.

Mouse hit-testing should use rectangles from the current rendered layout. After resize or mode changes, avoid relying on stale geometry. Ignore clicks outside the viewport/component.

## Cursor Correctness

Use `Frame::set_cursor_position` on compatible versions for the focused editable control. If no cursor is set during a frame, Ratatui hides it.

Cursor math must use terminal display columns of the visible grapheme prefix. These are wrong:

```rust
let column = text.len(); // bytes
let column = text.chars().count(); // scalar values
```

Use Ratatui text/line width methods or the repository's compatible Unicode-width/grapheme utilities. Account for horizontal scrolling, the input area's origin, borders, and clipping. Keep the cursor inside the visible buffer.

Before adding a direct `unicode-width`, `unicode-segmentation`, or text-wrapping dependency, inspect `Cargo.lock` and the Ratatui dependency graph. Older Ratatui releases may require an exact helper-crate version; reuse the resolved compatible version or avoid the extra direct dependency when Ratatui/project APIs already suffice.

## Text and Unicode

Ratatui's hierarchy is `Text -> Line -> Span`, rendered in terminal cells. Start with strings and promote only when styling or alignment requires it.

- Wrap based on display width, not bytes or character count.
- Preserve semantic styles when wrapping styled lines.
- Test combining marks, CJK, emoji/ZWJ sequences, and any relevant RTL content.
- Expect some emoji widths to vary by terminal/font; provide resilient clipping and fallback behavior.
- Do not split a grapheme to make a cell budget fit.
- Normalize newlines at the input boundary if the text model requires a single representation.

Use the repository's established wrapping helpers. Generic text wrapping can lose `Span` boundaries or styles.

See the [text API](https://docs.rs/ratatui/latest/ratatui/text/) and [`Buffer`](https://docs.rs/ratatui/latest/ratatui/buffer/struct.Buffer.html).

## Styles and Accessibility

`Style` is patched/merged. `Style::default()` applies no changes; `Style::reset()` actively resets values. Understand the existing style stack before replacing it.

- Prefer terminal-default foreground/background unless a theme requires explicit colors.
- Use semantic style helpers/tokens rather than repeated color literals.
- Show selection, focus, warnings, and disabled state with text, shape, borders, or modifiers in addition to color.
- Do not require Nerd Fonts or a particular Unicode border set for comprehension.
- Offer ASCII/plain-text alternatives when an icon carries meaning.
- Keep key hints discoverable and update them by mode.
- Use a real terminal cursor for editing where possible so terminal behavior remains understandable.

Ratatui does not currently provide a stable semantic accessibility layer, so keyboard access, focus order, contrast, and non-color cues are application responsibilities.

## Responsive Screen Checklist

Exercise at least:

- minimum supported size
- one column/row below that minimum
- common 80x24
- wide and short
- narrow and tall
- empty collections
- very long unbroken content
- double-width and combining text
- open overlay at each size
- focused editor at horizontal and vertical scroll boundaries

Prefer hiding secondary chrome, shortening labels, or changing layout mode over allowing important content to underflow or panic.
