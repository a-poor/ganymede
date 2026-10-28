# Authoring a theme — choosing actual values

Author in OKLCH, store `Hsla`. The `oklch(l, c, h)` helper in `theme.rs` converts at construction time, so theme files stay readable.

Contents: [Why OKLCH](#why-oklch) · [Decision order](#decision-order) · [The lightness ladder](#the-lightness-ladder) · [Dark mode](#dark-mode-is-re-authored-not-inverted) · [Worked example](#worked-example) · [Charts](#chart-colors) · [Validating](#validating)

## Why OKLCH

`oklch(L C H)` — perceptual lightness 0–1, chroma (saturation, 0–~0.37), hue in degrees.

The payoff is authoring, not fidelity:

- Equal `L` means equal *perceived* lightness across hues. A blue and a yellow at `L = 0.6` look equally light — false in HSL. Contrast becomes predictable arithmetic.
- You can build a coherent theme by **fixing the hue, varying L, and keeping C tiny**. That whisper of chroma in the neutrals is what makes a theme look designed rather than gray-plus-a-brand-color.
- Dark mode becomes "flip the L values around the midpoint, then hand-correct the chroma."

Useful reference points for neutrals (contrast against white):

| `L` | ≈ contrast vs. white | Use for |
|---|---|---|
| 0.99 | 1.0 | page background |
| 0.96 | 1.1 | muted fill, secondary button |
| 0.90 | 1.3 | borders (visible, not readable) |
| 0.56 | 4.5 | **the floor for secondary text on white** |
| 0.50 | 5.7 | comfortable secondary text |
| 0.18 | 19 | primary text |

Keep `muted_foreground` at `L ≤ 0.54` in light mode — check it against your *lightest* surface (usually `card`) and your *darkest* light surface (usually `sidebar`), not just `background`. It is the single most common accessibility failure in hand-rolled themes.

## Decision order

Work in dependency order — later choices depend on earlier ones.

1. **Neutral hue + temperature.** Pick a hue angle and a chroma of 0.002–0.015. ~264° reads cool/slate, ~80° warm/stone, `c = 0` pure gray. This sets the app's feel before any brand color exists.
2. **Light lightness ladder.** Five stops: page → muted fill → border → secondary text → primary text. Every neutral is a point on it.
3. **Elevation policy.** Do `card` and `popover` differ from `background`? Common choices: (a) background slightly off-white, card pure white; (b) all identical, separated by `border` alone. Decide once — it governs whether the UI reads as layered paper or flat regions.
4. **Brand → `primary`.** Then author `primary_foreground` and check that pair first; it is the most-viewed combination in the product.
5. **`ring`.** Usually `= primary`. Split it out if the brand color is too low-contrast to serve as a focus indicator.
6. **`destructive`** (+ any `success`/`warning`/`info` you decide to add).
7. **`accent` / `secondary`.** Points on the neutral ladder with a hint of the brand hue. `accent` sits just below `muted` in lightness; `secondary` at roughly `muted`.
8. **`radius`.** One number. `px(0.)` = sharp and technical, `px(6.)` = neutral, `px(10.)` = friendly, `px(16.)` = soft. Highest personality-per-keystroke in the system.
9. **Fonts.** Sans + mono family, then the size scale.
10. **Charts.** Five hues that are mutually distinguishable and look like they belong.
11. **Dark mode.** Re-author (below).
12. **Sidebar.** Only if it should diverge; otherwise clone the app surface values.

**The 80/20:** `primary`, `background`, `muted`, `border`, and `radius` do most of the visible work. Get those five right and the theme already reads as intentional.

## The lightness ladder

Light mode, hue `H`, small chroma:

```
background        L 0.99   c 0.002
card / popover    L 1.00   c 0.000     ← raised surfaces get lighter
sidebar           L 0.975  c 0.004     ← chrome recedes slightly
muted / secondary L 0.96   c 0.006
accent            L 0.955  c 0.012     ← hover/selected, faintly brand-tinted
border / input    L 0.90   c 0.008
muted_foreground  L 0.535  c 0.015
foreground        L 0.18   c 0.010
```

Dark mode compresses into a narrower band and inverts the elevation direction:

```
background        L 0.16   c 0.008     ← darkest
sidebar           L 0.185  c 0.008
card              L 0.20   c 0.009
popover           L 0.22   c 0.009     ← raised surfaces still get lighter
muted / secondary L 0.24   c 0.010
border / input    L 0.28   c 0.012
accent            L 0.30   c 0.016
muted_foreground  L 0.70   c 0.015
foreground        L 0.97   c 0.004
```

Note the range: light mode spans 0.18–1.00, dark spans 0.16–0.97 but with all the *surfaces* packed into 0.16–0.30. Dark UIs use a much tighter surface band; spreading them out looks cheap.

## Dark mode is re-authored, not inverted

Never generate dark by flipping lightness. Three corrections are mandatory:

1. **Raise the brand's lightness, lower its chroma.** A color that reads as confident on white vibrates on near-black. `oklch(0.55 0.20 259)` → `oklch(0.68 0.16 259)`.
2. **Compress the surface range.** See the ladder above.
3. **Raise the alpha on low-alpha effects.** A 12%-alpha subtle fill disappears on a dark surface — bump `Derivation::subtle_alpha` for the dark theme if it looks weak.

An optional refinement: make dark-mode borders semi-transparent white (`alpha(white, 0.10)`) rather than an opaque gray, so one value reads correctly against every surface in the elevation ladder.

## Worked example

Cool-neutral base (hue 264°) with a blue brand (259°). Paste into `theme.rs`.

```rust
impl ThemeSpec {
    pub fn light() -> Self {
        Self {
            is_dark: false,
            background:          oklch(0.990, 0.002, 264.),
            foreground:          oklch(0.180, 0.010, 264.),
            muted:               oklch(0.960, 0.006, 264.),
            muted_foreground:    oklch(0.535, 0.015, 264.),
            border:              oklch(0.900, 0.008, 264.),

            card:                oklch(1.000, 0.000, 264.),
            popover:             oklch(1.000, 0.000, 264.),
            sidebar:             oklch(0.975, 0.004, 264.),
            sidebar_foreground:  oklch(0.180, 0.010, 264.),
            sidebar_border:      oklch(0.895, 0.008, 264.),
            sidebar_accent:      oklch(0.930, 0.014, 264.),

            primary:             oklch(0.520, 0.200, 259.),
            primary_foreground:  oklch(0.985, 0.003, 259.),
            secondary:           oklch(0.960, 0.006, 264.),
            secondary_foreground:oklch(0.250, 0.015, 264.),
            accent:              oklch(0.955, 0.012, 264.),
            accent_foreground:   oklch(0.220, 0.020, 264.),
            destructive:         oklch(0.550, 0.200,  27.),
            destructive_foreground: oklch(0.990, 0.005, 27.),

            input:               oklch(0.900, 0.008, 264.),
            ring:                oklch(0.520, 0.200, 259.),
            chart: [
                oklch(0.62, 0.16, 259.),
                oklch(0.62, 0.14, 175.),
                oklch(0.68, 0.14,  70.),
                oklch(0.60, 0.18, 320.),
                oklch(0.64, 0.16,  30.),
            ],
            radius: px(8.),
            font: FontTokens::default(),
        }
    }

    pub fn dark() -> Self {
        Self {
            is_dark: true,
            background:          oklch(0.160, 0.008, 264.),
            foreground:          oklch(0.970, 0.004, 264.),
            muted:               oklch(0.240, 0.010, 264.),
            muted_foreground:    oklch(0.700, 0.015, 264.),
            border:              oklch(0.280, 0.012, 264.),

            card:                oklch(0.200, 0.009, 264.),
            popover:             oklch(0.220, 0.009, 264.),
            sidebar:             oklch(0.185, 0.008, 264.),
            sidebar_foreground:  oklch(0.940, 0.004, 264.),
            sidebar_border:      oklch(0.260, 0.012, 264.),
            sidebar_accent:      oklch(0.290, 0.016, 264.),

            primary:             oklch(0.680, 0.160, 259.),   // lighter, less chroma
            primary_foreground:  oklch(0.170, 0.020, 259.),
            secondary:           oklch(0.270, 0.012, 264.),
            secondary_foreground:oklch(0.960, 0.004, 264.),
            accent:              oklch(0.300, 0.016, 264.),
            accent_foreground:   oklch(0.970, 0.004, 264.),
            destructive:         oklch(0.550, 0.200,  27.),
            destructive_foreground: oklch(0.990, 0.005, 27.),

            input:               oklch(0.300, 0.012, 264.),
            ring:                oklch(0.680, 0.160, 259.),
            chart: [
                oklch(0.72, 0.15, 259.),
                oklch(0.72, 0.13, 175.),
                oklch(0.78, 0.13,  70.),
                oklch(0.70, 0.16, 320.),
                oklch(0.74, 0.15,  30.),
            ],
            radius: px(8.),
            font: FontTokens::default(),
        }
    }
}
```

To reskin: change the two hue numbers (`264.` neutral, `259.` brand) and the two `primary` lightness/chroma values. Everything else follows.

## Chart colors

Not paired, not semantic. The only contract is that the five are mutually distinguishable and look like they belong to the theme.

- Space hues roughly evenly, but avoid putting two neighbours in the 200–260° range (blues are hard to tell apart at small sizes).
- Hold `L` and `C` roughly constant across the set so no series looks more important than the others — variation should come from hue alone.
- Light mode `L ≈ 0.62`, dark mode `L ≈ 0.72`, chroma 0.13–0.18.
- Check the set in grayscale: if two series collapse, nudge one's `L` by 0.06.

## Validating

The contrast test in `theme-module.md` is the arbiter — run it after every value change. Beyond it, check by eye:

- **Borders visible but quiet.** If you notice the border before the content, drop its `L` contrast.
- **Hover perceptible but not jumpy.** Tune `Derivation::hover_shift` (default 0.05), not individual colors.
- **Focus ring visible on every surface**, including on top of `accent` and `primary`.
- **Disabled reads as disabled, not as a different variant.** If 0.5 opacity looks broken on a surface, that surface's contrast is probably too low to begin with.
- **Squint test.** Blur your eyes: the primary action should be the first thing you see on any screen. If two things compete, something that should be `secondary` is using `primary`.
