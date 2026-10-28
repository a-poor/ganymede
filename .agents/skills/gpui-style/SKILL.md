---
name: gpui-style
description: Design-token system for GPUI (Zed's Rust UI framework) modeled on shadcn/ui, providing semantic surface/foreground token pairs, a `Theme` global read via `cx.theme()`, per-region surfaces (app/card/popover/sidebar), one radius base that derives the whole scale, and derived hover/active/focus/disabled states. Use whenever writing or reviewing GPUI render code that sets colors, borders, radii, or fonts — including any `.bg(`, `.text_color(`, `.border_color(`, `.rounded`, `rgb(0x…)`, or `hsla(…)` inside a `Render`/`RenderOnce` impl. Also use when the user asks to add a theme, dark mode, a palette, or a design system to a GPUI app, mentions tokens like primary/muted/accent/destructive/foreground/ring, asks why their GPUI UI looks inconsistent or unstyled, or wants to port shadcn/ui or Tailwind theming to Rust. Trigger even without the word "theme" — "style this panel", "make this button look better", or "add a sidebar" in a GPUI codebase should pull this in.
---

# GPUI theming with semantic design tokens

A port of the shadcn/ui token model to GPUI. The point of the system: **render code names roles, never colors.** A button says "primary surface, primary foreground" — the `Theme` global answers what color that is right now.

## The one rule

> **No color, radius, or font literal ever appears in a `Render` or `RenderOnce` impl.**
> Every visual value comes from `cx.theme()`.

If you are about to write `rgb(0x1e1e2e)`, `hsla(...)`, `.rounded_md()`, or `.text_sm()` inside render code, stop and use a token instead. `.rounded_md()` is a hardcoded Tailwind constant; it does not follow the theme's radius.

## Workflow

1. **Check for an existing theme.** Look for a type implementing `gpui::Global` with token fields (`grep -rn "impl Global for" src/`). If one exists, use it and match its naming — do not introduce a second system.
2. **If none exists**, scaffold it from `references/theme-module.md` (complete, drop-in `theme.rs`). Do not hand-roll a partial version.
3. **Identify the region** you are rendering into and pick its surface from the table in §3.
4. **Build with tokens only**, following §2 (pairing), §4 (states), §5 (shape/type).
5. **Self-check** against §7 before finishing.

## 1. The token set (names are locked — do not rename or invent)

Colors are `gpui::Hsla`. Tokens divide into **per-surface** tokens (they differ by region) and **global** tokens (identical everywhere).

**Per-surface** — the `Surface` struct, one instance per region:

| Field | Role |
|---|---|
| `bg` | the surface itself |
| `fg` | primary text and icons on it |
| `muted_fg` | secondary text on it: captions, placeholders, help text, timestamps |
| `border` | separating lines drawn on it: edges, dividers, rules |
| `accent` / `accent_fg` | hover / selected background for rows and ghost controls on it |
| `primary` / `primary_fg` | the emphasis action as it appears on it |
| `ring` | focus indicator on it |

**Global** — the `Theme` struct:

| Field | Role |
|---|---|
| `secondary` / `secondary_fg` | filled but non-committal control (secondary button) |
| `muted` | inert tinted fill: table headers, code blocks, skeletons, wells |
| `destructive` / `destructive_fg` | danger: delete actions, error state |
| `input` | border color of form controls (usually `border`, split out so fields can be more or less prominent than dividers) |
| `chart[5]` | ordered categorical series for data viz |
| `radius` | one base length; the whole scale derives from it |
| `font` | family + size scale |

**Derived state fields** (`primary_hover`, `primary_active`, `ring_focus`, `destructive_subtle`, …) are computed once by `ThemeSpec::resolve()`. **Never author them by hand and never compute them at a call site.** See §4.

Notes on the roles that are easy to get wrong:

- `muted_fg` is the second-most-used token in a real app. Every caption, placeholder, and secondary label. There is no third gray — if text is not `fg` it is `muted_fg`.
- `secondary` is *a variant you choose*; `accent` is *a state that happens to you* (hover, selection). They often have similar values; they are not interchangeable in code.
- `ring` is separate from `primary` so focus visibility survives any brand color change.
- There is deliberately no `success`/`warning`/`info`. Add them only if the app needs them, and add them as full `(color, fg, subtle)` triples matching `destructive`.

## 2. The pairing convention (the load-bearing rule)

> Every token that paints a **surface** ships with the foreground that sits **on** it. Set both, from the same pair, in the same builder chain.

```rust
// correct — pair set together
div().bg(t.primary).text_color(t.primary_fg)

// wrong — foreground from a different pair, contrast not guaranteed
div().bg(t.primary).text_color(s.fg)

// wrong — a surface color used as text
div().text_color(t.primary_fg)   // primary_fg only ever sits on primary
```

This is what makes contrast correct by construction across every theme and both modes, without any component knowing the palette.

`border`, `input`, and `ring` have no foreground pair — they are line colors, never surfaces.

`.text_color()` cascades to children in GPUI, so set it once on the container that owns the surface rather than on every leaf.

## 3. Regions → surfaces (GPUI has no CSS cascade, so this is explicit)

CSS gets region-scoped theming free from the cascade. GPUI does not, so a surface is passed explicitly. `Theme::surface(SurfaceKind)` returns the right one.

| What you are rendering | `SurfaceKind` | Also apply |
|---|---|---|
| Window root, main content area, editor pane | `App` | — |
| Panel, settings group, list container, dialog body, card | `Card` | `border_1` + `border_color(s.border)`, `rounded(t.radius.lg())` |
| Dropdown, context menu, tooltip, popover, command palette, autocomplete | `Popover` | `border_1`, `rounded(t.radius.md())`, `shadow_lg()` |
| Left nav rail, activity bar, file tree, project panel, tab strip | `Sidebar` | `border_r_1` + `border_color(s.border)` |
| Titlebar, toolbar, status bar | `Sidebar` | chrome shares the nav palette by default |
| Modal scrim | — | `hsla(0., 0., 0., 0.5)` — the one allowed literal, or `t.scrim` |

**Elevation is expressed as color, not shadow.** In light mode raised surfaces get *lighter*; in dark mode they also get lighter (the background is darkest). Use `shadow_lg()` only for genuinely floating layers (popovers, modals) — never on cards.

Component signature convention: stateless components take the surface they render onto.

```rust
#[derive(IntoElement)]
struct Button { label: SharedString, variant: ButtonVariant, on: SurfaceKind }

impl Button {
    fn new(label: impl Into<SharedString>) -> Self {
        Self { label: label.into(), variant: ButtonVariant::Secondary, on: SurfaceKind::App }
    }
    fn on(mut self, kind: SurfaceKind) -> Self { self.on = kind; self }
}
```

Default to `SurfaceKind::App`; a container that owns a non-default surface passes it down. This replaces shadcn's flat `--sidebar-*` prefix hack with one structure that works for any region.

## 4. States are derived, never authored

Do not write per-state color values into the theme spec or into components. `resolve()` computes them from the base tokens using fixed rules:

| State | Rule |
|---|---|
| **Hover, filled** | base color moved ~10% toward the surface background → `t.primary_hover`, `t.secondary_hover`, `t.destructive_hover` |
| **Hover, transparent** (ghost button, menu row, list item) | paint `s.accent` behind it; text becomes `s.accent_fg` |
| **Active / pressed** | base moved ~18% toward the surface background → `*_active` |
| **Selected** | `s.accent` + `s.accent_fg`, persistent (not a hover style) |
| **Focus** | `s.ring` at 50% alpha as a 3px spread box-shadow — `t.ring_focus`; keyboard focus only |
| **Disabled** | `.opacity(0.5)` on the whole element + drop the click handler + `.cursor_default()`. One rule for every component; no disabled color tokens. |
| **Invalid** | `.border_color(t.destructive)` + focus ring becomes `t.destructive_ring` |
| **Subtle status fill** | `t.destructive_subtle` (destructive at ~10–15% alpha) for error banners |

In GPUI these are style closures on a stateful `div()`:

```rust
div()
    .id("save")
    .px_3().py_1p5()
    .rounded(t.radius.md())
    .bg(t.primary).text_color(t.primary_fg)
    .hover(|st| st.bg(t.primary_hover))
    .active(|st| st.bg(t.primary_active))
    .cursor_pointer()
```

`.hover()` needs no id; `.active()` and `.focus()` require `.id(...)` (it turns `Div` into `Stateful<Div>`).

Focus ring recipe — GPUI has no CSS `outline`, so use a zero-blur spread shadow, which does not affect layout:

```rust
use gpui::{BoxShadow, point, px};
.focus(|st| st.shadow(smallvec::smallvec![BoxShadow {
    color: t.ring_focus,
    offset: point(px(0.), px(0.)),
    blur_radius: px(0.),
    spread_radius: px(3.),
}]).border_color(t.app.ring))
```

Why derived rather than alpha-at-call-site: shadcn writes `bg-primary/90` and lets CSS composite over whatever parent happens to be there. That works in CSS and is fragile in GPUI, where you would recompute it in every component. Deriving once in `resolve()` puts the rule in one place and makes it tunable (see §6).

## 5. Shape and type

**Radius is one number.** The scale derives from it by fixed multipliers, so `radius: px(0.)` makes the whole app sharp and `px(16.)` softens it, proportionally, in one edit.

```rust
t.radius.sm()   // base * 0.6
t.radius.md()   // base * 0.8
t.radius.lg()   // base       ← the default for cards and panels
t.radius.xl()   // base * 1.4
```

Use `.rounded(t.radius.md())`. **Never** `.rounded_md()` / `.rounded_lg()` — those are fixed constants that ignore the theme.

Rough assignment: badges/inputs/buttons `md`, cards/panels/popovers `lg`, small chips `sm`, avatars `.rounded_full()`.

**Type.** GPUI's `.text_sm()`/`.text_base()` constants are usable but do not follow the theme's font settings, so route sizes through the theme too:

```rust
.font_family(t.font.sans.clone())
.text_size(t.font.sm)
.line_height(t.font.line_height_normal)
```

Weight comes from `FontWeight::{NORMAL, MEDIUM, SEMIBOLD}` directly — not a token. Monospace content uses `t.font.mono`.

**Spacing is not tokenized.** Use GPUI's numeric scale (`.p_2()`, `.gap_3()`, `.px_4()`) — it is already Tailwind's 0.25rem step and is consistent by construction. Adding spacing tokens buys nothing and enlarges the authoring surface.

## 6. What is locked vs. what the developer configures

| Locked by this skill (invariants) | Configured by the developer |
|---|---|
| Token names and roles (§1) | The ~24 base color values |
| Surface/foreground pairing (§2) | `radius` base length |
| Region → surface mapping defaults (§3) | Font families and size scale |
| States are derived, not authored (§4) | Derivation constants (`hover_mix`, `active_mix`, `ring_alpha`, `disabled_opacity`) via `Derivation` |
| Radius derives from one base (§5) | Extra semantic colors (`success`/`warning`/`info`) |
| No literals in render code | Which surfaces diverge (sidebar can just clone `app`) |

**Two-layer design.** `ThemeSpec` is the small, `serde`-friendly authoring struct (base values only). `Theme` is the resolved struct held in the global (all tokens + derived states). `ThemeSpec::resolve(&Derivation) -> Theme` is the single place the design rules live.

That split is what makes both stories work:

- **Locked at compile time** — ship `ThemeSpec::light()` / `ThemeSpec::dark()` as consts; users get no choice.
- **Configurable at runtime** — deserialize `ThemeSpec` from a TOML/JSON settings file, `resolve()`, `cx.set_global(theme)`.

Either way components are identical, because they only ever see the resolved `Theme`.

**Light and dark are two independently authored `ThemeSpec`s, not one spec plus an invert.** A chroma that reads as confident on white vibrates on near-black: dark themes raise the brand's lightness and *lower* its chroma, and compress the whole lightness range. Never generate dark mode by flipping lightness.

## 7. Self-check before finishing

- [ ] Zero `rgb(0x…)` / `rgba(0x…)` / `hsla(…)` literals in render code (scrim excepted).
- [ ] Every `.bg()` on a surface has a matching `.text_color()` from the same pair.
- [ ] Radius comes from `t.radius.*()`, not `.rounded_md()` / `.rounded_lg()`.
- [ ] Secondary text uses `s.muted_fg`; no third gray was invented.
- [ ] The region uses the surface from §3's table, and its border comes from that surface.
- [ ] Interactive elements have hover, active, focus, and disabled handled per §4 — not just hover.
- [ ] Shadows only on popovers/modals; cards separate by color + border.
- [ ] The view redraws on theme change (`cx.observe_global::<Theme>(|_, cx| cx.notify()).detach()` in the root view's constructor).

**Auditing an existing codebase:**

```bash
rg -n 'rgb\(0x|rgba\(0x|hsla\(' src/ --glob '!**/theme*'
rg -n '\.rounded_(sm|md|lg|xl|2xl|3xl)\(\)' src/
rg -n '\.text_(xs|sm|base|lg|xl)\(\)' src/
```

## References

Load these as needed — do not read all three up front.

| File | Read when |
|---|---|
| `references/theme-module.md` | Scaffolding the theme: complete `theme.rs` with `Theme`/`ThemeSpec`/`Surface`/`Derivation`, the `Global` impl, the `ActiveTheme` extension trait, OKLCH→`Hsla` conversion, contrast validation, and light/dark switching. |
| `references/component-recipes.md` | Building or restyling a specific widget: button variants, input, card, list row, menu, sidebar, badge, table, modal, tooltip — each as correct token usage. |
| `references/authoring-themes.md` | Choosing actual color values: the decision checklist in dependency order, the OKLCH lightness ladder, dark-mode adjustment rules, and a worked light/dark pair. |

## Ground truth

GPUI is pre-1.0 and its APIs move between commits. Before relying on any nontrivial signature (`Hsla` helpers, `BoxShadow` fields, `refresh_windows`, `WindowAppearance`), verify against the pinned revision — the vendored checkout under `~/.cargo/git/checkouts/zed-*/<rev>/crates/gpui/src/` is authoritative. The reference implementation deliberately defines its own `alpha`/`mix_toward` helpers rather than depending on `Hsla`'s inherent methods, so it compiles across revisions.
