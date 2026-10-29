# Text styling, fonts, SVG & assets

APIs verified against gpui source (`text_system.rs`, `elements/text.rs`, `elements/svg.rs`) and confirmed
by real bugs.

## StyledText & highlights

Style sub-ranges of one string with `StyledText::with_default_highlights`:

```rust
let base = window.text_style();          // start from the inherited style
StyledText::new(text).with_default_highlights(
    &base,
    vec![(range, HighlightStyle { font_weight: Some(FontWeight::BOLD), ..Default::default() })],
)
```

Bold via `font_weight`, italic via `font_style`, strikethrough via `strikethrough: StrikethroughStyle`,
links via `color` + `underline: UnderlineStyle`. Styles flow correctly through wrapped lines.

### Quirks (each caused a real bug)

- **`line_height` and `font_size` on the passed `TextStyle` are ignored** — StyledText takes them from the
  inherited *element* style. Set them on the wrapping div: `.line_height(...)`, `.text_size(...)`.
- **`HighlightStyle` backgrounds paint square, full-line-height quads.** No corner radius, no padding API.
  Rounded/padded "chip" backgrounds require a custom Element that paints its own quads behind the
  StyledText (use `text.layout().position_for_index(ix)` to find glyph positions, group by wrapped line).
- **Spans inside one StyledText can't be individually clickable.** Clickable links need their own
  interactive elements or custom hitboxes.
- To style a subrange at the run level (e.g. underline only an IME composition), split one logical run
  into multiple `TextRun`s (pre / marked / post) and filter zero-length runs.

## Caret / selection painting (custom editors)

`paint_line` centers the glyph box within the line box — size caret and selection quads to the glyph box
(ascent + descent), not the full line height, or the caret reads as sitting too high.

## Fonts

- **Embedding fonts does not register them.** Fonts shipped via an `AssetSource` (rust-embed etc.) must be
  fed to `cx.text_system().add_fonts(vec![bytes.into()])` at startup, or `font_family("...")` silently
  falls back to a system font. List them via `cx.asset_source().list("fonts")` + `.load(path)`.
- **Use static per-weight font files, not variable fonts.** GPUI registers only a variable font's default
  instance, so every `FontWeight` silently resolves to Regular.
- macOS system font: `FontWeight::MEDIUM` (500) also resolves to Regular — use `SEMIBOLD` for visible
  emphasis.
- Text styles cascade: set `.font_family(...)` once on the root div for an app-wide default; override
  locally (`.font_family("Geist Mono")` on code blocks).

## SVG

- **Every SVG needs its own `.text_color(...)`.** The parent's text color does *not* cascade into
  `svg()`. The renderer checks the SVG's own `style.text.color`; if it is absent, it skips drawing
  the icon, even when the SVG file is bundled correctly and its asset path is valid. Set an explicit
  foreground from the app's theme, e.g. `svg().path("icons/x.svg").size(px(16.)).text_color(icon_color)`.
  For a blank icon, check this before investigating asset loading.
- Hover recoloring must also be applied directly to the SVG (e.g. via `.group_hover(...)`); changing
  only the parent's `.text_color(...)` does not recolor it. Always set a base color as well so the icon
  renders when it is not hovered.
- Transforms: `.with_transformation(Transformation::rotate(percentage(0.25)))` (also `translate`/`scale`).
  This is the only high-level transform in gpui — divs have no CSS-style `transform` (see layout-style.md).

## Assets

Implement `AssetSource` (`load`/`list`) over an embedded bundle (e.g. rust-embed) and install it with
`Application::new().with_assets(Assets)` — `svg().path("icons/x.svg")` paths resolve through it. Path
prefixes must match the embed's folder structure exactly; a wrong prefix fails silently (blank icon).
