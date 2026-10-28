# Component recipes

Canonical token usage per widget. Copy the *structure*, not necessarily the exact sizes.

Contents: [Preamble](#preamble) · [Button](#button) · [Icon button](#icon-button) · [Input](#text-input) · [Card](#card--panel) · [List row](#list-row) · [Menu / popover](#menu--popover) · [Sidebar](#sidebar) · [Badge](#badge) · [Table](#table) · [Modal](#modal) · [Tooltip](#tooltip) · [Separator](#separator--empty-state) · [Anti-patterns](#anti-patterns)

## Preamble

```rust
use crate::theme::{ActiveTheme, SurfaceKind, Theme};
use gpui::{BoxShadow, prelude::*, div, point, px, FontWeight, SharedString};
```

Every recipe starts by resolving the surface it is drawn onto:

```rust
let t = cx.theme();
let s = t.surface(self.on);   // self.on: SurfaceKind, defaults to App
```

Two shared helpers — put them next to the theme module:

```rust
/// 3px focus ring. Zero blur, spread only, so it does not affect layout.
pub fn focus_ring(color: gpui::Hsla) -> smallvec::SmallVec<[BoxShadow; 2]> {
    smallvec::smallvec![BoxShadow {
        color,
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(3.),
    }]
}
```

## Button

Four variants, one shape. Only the color pair and hover source change.

```rust
#[derive(Clone, Copy, PartialEq, Default)]
pub enum ButtonVariant { Primary, #[default] Secondary, Ghost, Destructive }

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    variant: ButtonVariant,
    on: SurfaceKind,
    disabled: bool,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl RenderOnce for Button {
    fn render(self, _w: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = cx.theme();
        let s = t.surface(self.on);

        // (rest, hover, active, fg) — the only thing the variant decides.
        let (bg, hover, active, fg) = match self.variant {
            ButtonVariant::Primary =>
                (s.primary, t.primary_hover, t.primary_active, s.primary_fg),
            ButtonVariant::Secondary =>
                (t.secondary, t.secondary_hover, t.secondary_active, t.secondary_fg),
            ButtonVariant::Ghost =>
                (gpui::transparent_black(), s.accent, s.accent, s.fg),
            ButtonVariant::Destructive =>
                (t.destructive, t.destructive_hover, t.destructive_hover, t.destructive_fg),
        };
        let ring = t.ring_focus;
        let ring_border = s.ring;
        let radius = t.radius.md();
        let disabled_opacity = t.disabled_opacity;

        div()
            .id(self.id)
            .flex().items_center().justify_center()
            .h(px(30.)).px_3().gap_1p5()
            .rounded(radius)
            .bg(bg)
            .text_color(fg)
            .text_size(cx.theme().font.sm)
            .font_weight(FontWeight::MEDIUM)
            .when(!self.disabled, |this| {
                this.cursor_pointer()
                    .hover(|st| st.bg(hover))
                    .active(|st| st.bg(active))
                    .focus(|st| st.shadow(focus_ring(ring)).border_color(ring_border))
            })
            .when(self.disabled, |this| this.opacity(disabled_opacity).cursor_default())
            .child(self.label)
            .when_some(
                self.on_click.filter(|_| !self.disabled),
                |this, handler| this.on_click(move |e, w, cx| handler(e, w, cx)),
            )
    }
}
```

Points that generalize:

- **Ghost is transparent at rest and uses `s.accent` on hover** — that is what `accent` is for. It reads correctly on any surface because `accent` comes from the surface.
- Disabled is `opacity` + no handler + `cursor_default`. There are no disabled color tokens.
- Copy `Hsla`/`Pixels` out of `cx.theme()` into locals before the builder chain — `cx` is borrowed immutably by `theme()` and you may need it mutably for listeners.

Outline variant, if needed: transparent background, `border_1` + `border_color(s.border)`, hover to `s.accent`.

## Icon button

Same as ghost, square, icon inherits `text_color`:

```rust
div()
    .id(self.id)
    .size(px(28.)).flex().items_center().justify_center()
    .rounded(t.radius.sm())
    .text_color(s.muted_fg)
    .hover(|st| st.bg(s.accent).text_color(s.accent_fg))
    .child(svg().path(self.icon).size(px(15.)).text_color(gpui::Hsla::default()))
```

Icons in GPUI are `svg()` tinted by `.text_color()`. Set the color on the icon's *parent* and let it cascade; do not hardcode icon colors.

## Text input

The one place `t.input` is used instead of `s.border`.

```rust
let border = if self.invalid { t.destructive } else { t.input };
let ring   = if self.invalid { t.destructive_ring } else { t.ring_focus };

div()
    .id("field")
    .w_full().h(px(32.)).px_2p5()
    .flex().items_center()
    .rounded(t.radius.md())
    .bg(s.bg)
    .text_color(s.fg)
    .text_size(t.font.sm)
    .border_1().border_color(border)
    .focus(|st| st.shadow(focus_ring(ring)).border_color(s.ring))
    .track_focus(&self.focus_handle)
    .child(/* editor element, or placeholder */)
```

- Placeholder text is `s.muted_fg`. Never a lighter custom gray.
- Label above is `s.fg`; help text below is `s.muted_fg`; error text is `t.destructive`.
- Invalid state changes **border + ring only**. Do not tint the field background — that reads as disabled.

## Card / panel

Elevation is color plus a border, not a shadow.

```rust
let s = t.surface(SurfaceKind::Card);

div()
    .flex().flex_col()
    .rounded(t.radius.lg())
    .bg(s.bg)
    .text_color(s.fg)
    .border_1().border_color(s.border)
    .child(
        div()   // header
            .px_4().py_3()
            .border_b_1().border_color(s.border)
            .child(div().text_size(t.font.base).font_weight(FontWeight::SEMIBOLD).child("Title"))
            .child(div().text_size(t.font.sm).text_color(s.muted_fg).child("Supporting copy")),
    )
    .child(div().p_4().child(/* body */))
```

Everything inside the card that needs a surface should read `SurfaceKind::Card`, not `App` — that is how the "cascade" is expressed here.

## List row

Selection is `accent` and is *persistent*; hover is the same color and is transient. Both come from the surface.

```rust
div()
    .id(("row", ix))
    .flex().items_center().gap_2()
    .px_2().py_1p5()
    .rounded(t.radius.sm())
    .text_color(s.fg)
    .when(self.selected, |this| this.bg(s.accent).text_color(s.accent_fg))
    .when(!self.selected, |this| this.hover(|st| st.bg(s.accent)))
    .child(div().flex_1().truncate().child(self.title.clone()))
    .child(div().text_size(t.font.xs).text_color(s.muted_fg).child(self.meta.clone()))
```

Use `.group("row")` + `.group_hover("row", |st| …)` when a child (a delete button, a timestamp) needs to react to the row's hover.

## Menu / popover

The only place shadows belong.

```rust
let s = t.surface(SurfaceKind::Popover);

div()
    .min_w(px(180.)).py_1()
    .rounded(t.radius.md())
    .bg(s.bg)
    .text_color(s.fg)
    .border_1().border_color(s.border)
    .shadow_lg()
    .children(items.into_iter().map(|item| {
        div()
            .id(item.id)
            .flex().items_center().gap_2()
            .px_2p5().py_1p5().mx_1()
            .rounded(t.radius.sm())
            .text_size(t.font.sm)
            .hover(|st| st.bg(s.accent).text_color(s.accent_fg))
            .when(item.destructive, |this| {
                this.text_color(t.destructive)
                    .hover(|st| st.bg(t.destructive_subtle).text_color(t.destructive))
            })
            .child(item.label)
            .child(div().flex_1())
            .child(div().text_color(s.muted_fg).text_size(t.font.xs).child(item.shortcut))
    }))
    .child(div().h(px(1.)).my_1().bg(s.border))   // separator
```

A destructive menu item is `destructive` **text** on the normal surface, hovering to `destructive_subtle`. Do not fill it solid — that is for buttons.

Wrap in `deferred(anchored(...))` for correct stacking; see the `gpui` skill's overlay reference.

## Sidebar

The region that most often diverges from the content area — which is why it has its own `Surface`.

```rust
let s = t.surface(SurfaceKind::Sidebar);

div()
    .flex().flex_col()
    .w(px(240.)).h_full()
    .bg(s.bg)
    .text_color(s.fg)
    .border_r_1().border_color(s.border)
    .child(
        div().px_3().py_2()
            .text_size(t.font.xs).font_weight(FontWeight::MEDIUM)
            .text_color(s.muted_fg)
            .child("WORKSPACE"),           // section header: muted_fg, small, medium
    )
    .children(nav_items.into_iter().map(|item| {
        div()
            .id(item.id)
            .flex().items_center().gap_2()
            .mx_2().px_2().py_1p5()
            .rounded(t.radius.sm())
            .text_size(t.font.sm)
            .when(item.active, |this| this.bg(s.accent).text_color(s.accent_fg))
            .when(!item.active, |this| {
                this.text_color(s.muted_fg).hover(|st| st.bg(s.accent).text_color(s.accent_fg))
            })
            .child(item.label)
    }))
```

If the sidebar should match the content area, make `ThemeSpec::sidebar == background` and the same code produces a flat look — no component change.

Titlebars, toolbars, and status bars default to `SurfaceKind::Sidebar` for the same reason: they are chrome, not content.

## Badge

```rust
div()
    .px_2().py_0p5()
    .rounded(t.radius.sm())
    .text_size(t.font.xs).font_weight(FontWeight::MEDIUM)
    .bg(t.secondary).text_color(t.secondary_fg)          // neutral
    // .bg(s.primary).text_color(s.primary_fg)           // emphasis
    // .bg(t.destructive_subtle).text_color(t.destructive)  // error
    .child(self.label)
```

`destructive_subtle` + `destructive` text is the standard low-noise status treatment — reuse it for error banners and invalid-field messages.

## Table

```rust
// header row — `muted` is the inert tinted fill
div().flex().bg(t.muted).text_color(s.muted_fg)
    .text_size(t.font.xs).font_weight(FontWeight::MEDIUM)
    .px_3().py_2()
    .border_b_1().border_color(s.border)

// body rows
div().flex().px_3().py_2()
    .border_b_1().border_color(s.border)
    .text_size(t.font.sm)
    .hover(|st| st.bg(s.accent))
```

Do not zebra-stripe *and* rule — pick one. `border` for rules, `muted` for stripes.

## Modal

```rust
// scrim — the one allowed literal-ish value, and it lives in the theme
div().absolute().inset_0().bg(t.scrim)
    .child(
        div()
            .m_auto().w(px(420.))
            .rounded(t.radius.xl())
            .bg(t.surface(SurfaceKind::Card).bg)
            .text_color(t.surface(SurfaceKind::Card).fg)
            .border_1().border_color(t.surface(SurfaceKind::Card).border)
            .shadow_2xl()
            .child(/* … */)
    )
```

Modal bodies are `Card`, not `Popover` — they are content, not a transient layer. Footers put the primary action last (right) and use `Ghost` for cancel.

## Separator / empty state

```rust
// horizontal rule
div().h(px(1.)).w_full().bg(s.border)

// empty state
div().flex().flex_col().items_center().gap_1().py_8()
    .text_color(s.muted_fg)
    .child(div().text_size(t.font.sm).child("No results"))
    .child(div().text_size(t.font.xs).child("Try a different search"))
```

## Anti-patterns

| Don't | Do |
|---|---|
| `.bg(rgb(0x1e1e2e))` | `.bg(s.bg)` |
| `.rounded_md()` | `.rounded(t.radius.md())` |
| `.text_sm()` | `.text_size(t.font.sm)` |
| `.text_color(rgb(0x888888))` for captions | `.text_color(s.muted_fg)` |
| `.bg(t.primary).text_color(s.fg)` | `.bg(s.primary).text_color(s.primary_fg)` |
| Computing hover with `alpha(t.primary, 0.9)` at a call site | `t.primary_hover` |
| `if t.is_dark { … } else { … }` inside a component | Encode the difference as a token difference in the two `ThemeSpec`s |
| `.shadow_md()` on a card | Border + surface color |
| A third gray for tertiary text | `s.muted_fg` (there are two text colors, not three) |
| Reading `SurfaceKind::App` inside a popover | Pass the popover's surface down |
| Adding `button_bg` / `panel_border` tokens | Use an existing role, or add a full semantic triple |
