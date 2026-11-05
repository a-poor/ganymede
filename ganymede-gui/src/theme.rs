//! Semantic design tokens shared by the application views.

use gpui::{App, BoxShadow, Global, Hsla, Pixels, SharedString, px};

// ---------------------------------------------------------------- color utils

/// Same color at a different alpha.
pub fn alpha(c: Hsla, a: f32) -> Hsla {
    Hsla {
        a: a.clamp(0.0, 1.0),
        ..c
    }
}

/// Shift a fill's lightness for a hover/active state.
///
/// Moves *toward* the surface when the fill contrasts strongly with it (the
/// CSS `bg-primary/90` behaviour), and *away* from the surface when it does
/// not — so the state change stays visible for low-contrast fills.
pub fn state_shift(c: Hsla, surface_bg: Hsla, amount: f32) -> Hsla {
    let d = surface_bg.l - c.l;
    let dir = if d.abs() >= 0.25 {
        d.signum()
    } else {
        -d.signum()
    };
    let dir = if dir == 0.0 { 1.0 } else { dir };
    Hsla {
        l: (c.l + dir * amount).clamp(0.0, 1.0),
        ..c
    }
}

/// OKLCH -> `Hsla`. Author themes in OKLCH; GPUI stores HSL.
///
/// `l` 0.0..1.0 perceptual lightness, `c` chroma (0.0..~0.37), `h` hue in degrees.
pub fn oklch(l: f32, c: f32, h: f32) -> Hsla {
    oklch_a(l, c, h, 1.0)
}

#[allow(clippy::excessive_precision)] // Published OKLab conversion coefficients.
pub fn oklch_a(l: f32, c: f32, h: f32, a: f32) -> Hsla {
    let hr = h.to_radians();
    let (oa, ob) = (c * hr.cos(), c * hr.sin());

    let l_ = l + 0.3963377774 * oa + 0.2158037573 * ob;
    let m_ = l - 0.1055613458 * oa - 0.0638541728 * ob;
    let s_ = l - 0.0894841775 * oa - 1.2914855480 * ob;
    let (lc, mc, sc) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);

    let lr = 4.0767416621 * lc - 3.3077115913 * mc + 0.2309699292 * sc;
    let lg = -1.2684380046 * lc + 2.6097574011 * mc - 0.3413193965 * sc;
    let lb = -0.0041960863 * lc - 0.7034186147 * mc + 1.7076147010 * sc;

    rgb_to_hsla(gamma(lr), gamma(lg), gamma(lb), a)
}

fn gamma(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn ungamma(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn rgb_to_hsla(r: f32, g: f32, b: f32, a: f32) -> Hsla {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-6 {
        return Hsla {
            h: 0.0,
            s: 0.0,
            l,
            a,
        };
    }
    let s = if l < 0.5 {
        d / (max + min)
    } else {
        d / (2.0 - max - min)
    };
    let h = if max == r {
        (g - b) / d
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    Hsla {
        h: (h / 6.0).rem_euclid(1.0),
        s,
        l,
        a,
    }
}

fn hsla_to_rgb(c: Hsla) -> (f32, f32, f32) {
    if c.s < 1e-6 {
        return (c.l, c.l, c.l);
    }
    let q = if c.l < 0.5 {
        c.l * (1.0 + c.s)
    } else {
        c.l + c.s - c.l * c.s
    };
    let p = 2.0 * c.l - q;
    let f = |mut t: f32| {
        t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    (f(c.h + 1.0 / 3.0), f(c.h), f(c.h - 1.0 / 3.0))
}

/// WCAG relative luminance.
pub fn luminance(c: Hsla) -> f32 {
    let (r, g, b) = hsla_to_rgb(c);
    0.2126 * ungamma(r) + 0.7152 * ungamma(g) + 0.0722 * ungamma(b)
}

/// WCAG contrast ratio, 1.0..21.0. Body text wants >= 4.5, large text >= 3.0.
pub fn contrast(a: Hsla, b: Hsla) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    let (hi, lo) = if x > y { (x, y) } else { (y, x) };
    (hi + 0.05) / (lo + 0.05)
}

// ------------------------------------------------------------------- surfaces

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SurfaceKind {
    /// Window root, main content, editor pane.
    #[default]
    App,
    /// Panels, cards, dialog bodies, list containers.
    Card,
    /// Floating layers: menus, dropdowns, tooltips, command palettes.
    Popover,
    /// Navigation chrome: rails, activity bars, file trees, titlebars.
    Sidebar,
}

/// The tokens that vary by region. One instance per `SurfaceKind`.
#[derive(Clone, Copy, Debug)]
pub struct Surface {
    pub bg: Hsla,
    pub fg: Hsla,
    pub muted_fg: Hsla,
    pub border: Hsla,
    pub accent: Hsla,
    pub accent_fg: Hsla,
    pub primary: Hsla,
    pub primary_fg: Hsla,
    pub ring: Hsla,
}

// --------------------------------------------------------------- shape & type

#[derive(Clone, Copy, Debug)]
pub struct Radii {
    pub base: Pixels,
}

impl Radii {
    pub fn sm(&self) -> Pixels {
        self.base * 0.6
    }
    pub fn md(&self) -> Pixels {
        self.base * 0.8
    }
    pub fn lg(&self) -> Pixels {
        self.base
    }
    pub fn xl(&self) -> Pixels {
        self.base * 1.4
    }
    pub fn xl2(&self) -> Pixels {
        self.base * 1.8
    }
}

#[derive(Clone, Debug)]
pub struct FontTokens {
    pub sans: SharedString,
    pub mono: SharedString,
    pub brand: SharedString,
    pub brand_size: Pixels,
    pub brand_tracking: Pixels,
    pub code: Pixels,
    pub xs: Pixels,
    pub sm: Pixels,
    pub base: Pixels,
    pub lg: Pixels,
    pub xl: Pixels,
    pub line_height_tight: Pixels,
    pub line_height_normal: Pixels,
}

impl Default for FontTokens {
    fn default() -> Self {
        Self {
            // Registered from the embedded asset bundle during startup.
            sans: "Inter".into(),
            mono: "JetBrains Mono".into(),
            brand: "Geist Pixel".into(),
            brand_size: px(84.),
            brand_tracking: px(-2.52),
            code: px(12.),
            xs: px(11.),
            sm: px(13.),
            base: px(14.),
            lg: px(16.),
            xl: px(20.),
            line_height_tight: px(18.),
            line_height_normal: px(21.),
        }
    }
}

// ----------------------------------------------------------------- derivation

/// The interaction-state rules. Tunable, but tuned in ONE place.
#[derive(Clone, Copy, Debug)]
pub struct Derivation {
    pub hover_shift: f32,
    pub active_shift: f32,
    pub ring_alpha: f32,
    pub subtle_alpha: f32,
    pub disabled_opacity: f32,
}

impl Default for Derivation {
    fn default() -> Self {
        Self {
            hover_shift: 0.05,
            active_shift: 0.09,
            ring_alpha: 0.50,
            subtle_alpha: 0.12,
            disabled_opacity: 0.50,
        }
    }
}

// ---------------------------------------------------------------------- theme

/// Resolved tokens. This is what components read. Never constructed by hand.
#[derive(Clone, Debug)]
pub struct Theme {
    pub is_dark: bool,
    pub syntax: [Hsla; 8],
    pub find_match: Hsla,
    pub find_active: Hsla,

    pub app: Surface,
    pub card: Surface,
    pub popover: Surface,
    pub sidebar: Surface,
    pub sidebar_content: Surface,
    pub rail_hover_bg: Hsla,

    pub secondary: Hsla,
    pub secondary_fg: Hsla,
    pub muted: Hsla,
    pub destructive: Hsla,
    pub destructive_fg: Hsla,
    pub warning: Hsla,
    pub warning_fg: Hsla,
    pub warning_subtle: Hsla,
    pub input: Hsla,
    pub scrim: Hsla,
    pub dialog_action_bg: Hsla,
    pub dialog_action_fg: Hsla,
    pub dialog_destructive_bg: Hsla,
    pub dialog_destructive_fg: Hsla,
    pub dialog_destructive_hover: Hsla,
    pub dialog_destructive_active: Hsla,
    pub launcher_art_bg: Hsla,
    pub launcher_art_fg: Hsla,
    pub launcher_sidebar: Surface,
    pub launcher_recent_hover_bg: Hsla,
    pub launcher_option_hover_bg: Hsla,
    pub launcher_border: Hsla,
    pub launcher_shadow: BoxShadow,
    pub body_shadow: BoxShadow,
    pub active_tab_edges: [BoxShadow; 2],
    pub toast_shadow: BoxShadow,
    pub segment_track_bg: Hsla,
    pub selected_segment_bg: Hsla,
    pub selected_segment_fg: Hsla,
    pub selected_segment_shadow: BoxShadow,
    pub selected_segment_edge: BoxShadow,
    pub active_file_bg: Hsla,
    pub active_file_fg: Hsla,
    pub chart: [Hsla; 5],

    // derived — see the `Derivation` rules
    pub primary_hover: Hsla,
    pub primary_active: Hsla,
    pub secondary_hover: Hsla,
    pub secondary_active: Hsla,
    pub destructive_hover: Hsla,
    pub destructive_subtle: Hsla,
    pub destructive_ring: Hsla,
    pub ring_focus: Hsla,
    pub disabled_opacity: f32,

    pub radius: Radii,
    pub font: FontTokens,
}

impl Global for Theme {}

impl Theme {
    pub fn surface(&self, kind: SurfaceKind) -> &Surface {
        match kind {
            SurfaceKind::App => &self.app,
            SurfaceKind::Card => &self.card,
            SurfaceKind::Popover => &self.popover,
            SurfaceKind::Sidebar => &self.sidebar,
        }
    }
}

/// Read the active theme: `cx.theme().app.bg`.
///
/// `Context<T>` derefs to `App`, so this works from `Render` impls too, as long
/// as `ActiveTheme` is in scope.
pub trait ActiveTheme {
    fn theme(&self) -> &Theme;
}

impl ActiveTheme for App {
    fn theme(&self) -> &Theme {
        self.global::<Theme>()
    }
}

// ------------------------------------------------------------------ authoring

/// The hand-authored layer. Small on purpose — everything else is derived.
#[derive(Clone, Debug)]
pub struct ThemeSpec {
    pub is_dark: bool,

    pub background: Hsla,
    pub foreground: Hsla,
    pub muted: Hsla,
    pub muted_foreground: Hsla,
    pub border: Hsla,

    pub card: Hsla,
    pub popover: Hsla,
    pub sidebar: Hsla,
    pub sidebar_foreground: Hsla,
    pub sidebar_border: Hsla,
    pub sidebar_accent: Hsla,

    pub primary: Hsla,
    pub primary_foreground: Hsla,
    pub secondary: Hsla,
    pub secondary_foreground: Hsla,
    pub accent: Hsla,
    pub accent_foreground: Hsla,
    pub destructive: Hsla,
    pub destructive_foreground: Hsla,
    pub warning: Hsla,
    pub warning_foreground: Hsla,

    pub input: Hsla,
    pub ring: Hsla,
    pub chart: [Hsla; 5],

    pub radius: Pixels,
    pub font: FontTokens,
}

impl ThemeSpec {
    pub fn resolve(&self, d: &Derivation) -> Theme {
        let mk = |bg: Hsla, fg: Hsla, border: Hsla, accent: Hsla, accent_fg: Hsla| Surface {
            bg,
            fg,
            muted_fg: self.muted_foreground,
            border,
            accent,
            accent_fg,
            primary: self.primary,
            primary_fg: self.primary_foreground,
            ring: self.ring,
        };

        let app = mk(
            self.background,
            self.foreground,
            self.border,
            self.accent,
            self.accent_foreground,
        );
        let card = mk(
            self.card,
            self.foreground,
            self.border,
            self.accent,
            self.accent_foreground,
        );
        let popover = mk(
            self.popover,
            self.foreground,
            self.border,
            self.accent,
            self.accent_foreground,
        );
        let sidebar = mk(
            self.sidebar,
            self.sidebar_foreground,
            self.sidebar_border,
            self.sidebar_accent,
            self.sidebar_foreground,
        );

        Theme {
            is_dark: self.is_dark,
            find_match: alpha(self.ring, 0.18),
            find_active: alpha(self.ring, 0.38),
            // keyword, string, comment, number, function, type, constant, operator
            syntax: if self.is_dark {
                [
                    0xc4a7e7, 0xa6da95, 0x9aa5b5, 0xf5a97f, 0x8aadf4, 0x8bd5ca, 0xf5a97f, 0xc4a7e7,
                ]
                .map(|c| gpui::rgb(c).into())
            } else {
                [
                    0x7533a5, 0x27632e, 0x626b78, 0x985000, 0x2355a0, 0x006b68, 0x985000, 0x7533a5,
                ]
                .map(|c| gpui::rgb(c).into())
            },

            primary_hover: state_shift(self.primary, app.bg, d.hover_shift),
            primary_active: state_shift(self.primary, app.bg, d.active_shift),
            secondary_hover: state_shift(self.secondary, app.bg, d.hover_shift),
            secondary_active: state_shift(self.secondary, app.bg, d.active_shift),
            destructive_hover: state_shift(self.destructive, app.bg, d.hover_shift),
            destructive_subtle: alpha(self.destructive, d.subtle_alpha),
            warning: self.warning,
            warning_fg: self.warning_foreground,
            warning_subtle: alpha(self.warning, d.subtle_alpha),
            destructive_ring: alpha(self.destructive, d.ring_alpha),
            ring_focus: alpha(self.ring, d.ring_alpha),
            disabled_opacity: d.disabled_opacity,

            app,
            card,
            popover,
            sidebar,
            rail_hover_bg: sidebar.bg.blend(alpha(sidebar.accent, 0.5)),
            sidebar_content: Surface {
                bg: sidebar.bg.blend(alpha(app.bg, 0.5)),
                ..sidebar
            },

            secondary: self.secondary,
            secondary_fg: self.secondary_foreground,
            muted: self.muted,
            destructive: self.destructive,
            destructive_fg: self.destructive_foreground,
            input: self.input,
            dialog_action_bg: app.fg,
            dialog_action_fg: app.bg,
            dialog_destructive_bg: card.bg.blend(alpha(self.destructive, 0.10)),
            dialog_destructive_hover: card.bg.blend(alpha(self.destructive, 0.18)),
            dialog_destructive_active: card.bg.blend(alpha(self.destructive, 0.24)),
            dialog_destructive_fg: if self.is_dark {
                oklch(0.780, 0.130, 27.)
            } else {
                oklch(0.490, 0.200, 27.)
            },
            scrim: Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.0,
                a: 0.16,
            },
            chart: self.chart,
            toast_shadow: BoxShadow::new(px(0.), px(4.), alpha(gpui::black(), 0.10))
                .blur_radius(px(14.)),
            // Preserve PlanetRise at night; reverse its ink/paper relationship in light mode.
            launcher_art_bg: if self.is_dark {
                gpui::rgb(0x07080a).into()
            } else {
                app.bg
            },
            launcher_art_fg: if self.is_dark {
                gpui::rgb(0xe6e1d6).into()
            } else {
                app.fg
            },
            launcher_sidebar: if self.is_dark {
                card
            } else {
                Surface {
                    bg: self.secondary,
                    ..sidebar
                }
            },
            launcher_option_hover_bg: popover
                .bg
                .blend(alpha(self.primary, if self.is_dark { 0.18 } else { 0.08 })),
            launcher_recent_hover_bg: {
                if self.is_dark {
                    card.bg.blend(alpha(card.fg, 0.035))
                } else {
                    app.bg
                }
            },
            launcher_border: app.bg.blend(alpha(app.border, 0.55)),
            launcher_shadow: BoxShadow::new(
                px(0.),
                px(4.),
                alpha(gpui::black(), if self.is_dark { 0.18 } else { 0.06 }),
            )
            .blur_radius(px(18.)),
            body_shadow: BoxShadow::new(
                px(0.),
                px(0.5),
                Hsla {
                    h: 0.,
                    s: 0.,
                    l: 0.,
                    a: if self.is_dark { 0.12 } else { 0.05 },
                },
            )
            .blur_radius(px(2.)),

            active_tab_edges: [
                BoxShadow::new(
                    px(0.75),
                    px(0.75),
                    alpha(gpui::white(), if self.is_dark { 0.10 } else { 0.85 }),
                )
                .blur_radius(px(0.))
                .inset(),
                BoxShadow::new(
                    px(-0.75),
                    px(-0.75),
                    alpha(gpui::black(), if self.is_dark { 0.20 } else { 0.07 }),
                )
                .blur_radius(px(0.))
                .inset(),
            ],
            segment_track_bg: card.bg.blend(alpha(sidebar.accent, 0.4)),
            selected_segment_bg: gpui::rgb(0xffffff).into(),
            selected_segment_fg: gpui::rgb(0x242830).into(),
            selected_segment_shadow: BoxShadow::new(
                px(0.),
                px(0.75),
                Hsla {
                    h: 0.,
                    s: 0.,
                    l: 0.,
                    a: 0.12,
                },
            )
            .blur_radius(px(3.)),
            selected_segment_edge: BoxShadow::new(
                px(0.),
                px(0.),
                Hsla {
                    h: 0.,
                    s: 0.,
                    l: 1.,
                    a: 0.9,
                },
            )
            .spread_radius(px(1.))
            .inset(),
            active_file_bg: if self.is_dark {
                oklch(0.32, 0.08, 259.)
            } else {
                oklch(0.93, 0.04, 259.)
            },
            active_file_fg: if self.is_dark {
                oklch(0.93, 0.04, 259.)
            } else {
                oklch(0.40, 0.14, 259.)
            },
            radius: Radii { base: self.radius },
            font: self.font.clone(),
        }
    }
}
impl ThemeSpec {
    pub fn light() -> Self {
        Self {
            is_dark: false,
            background: oklch(1.000, 0.000, 264.),
            foreground: oklch(0.180, 0.010, 264.),
            muted: oklch(0.960, 0.006, 264.),
            muted_foreground: oklch(0.535, 0.015, 264.),
            border: oklch(0.900, 0.008, 264.),

            card: oklch(0.990, 0.002, 264.),
            popover: oklch(1.000, 0.000, 264.),
            sidebar: oklch(0.975, 0.004, 264.),
            sidebar_foreground: oklch(0.180, 0.010, 264.),
            sidebar_border: oklch(0.895, 0.008, 264.),
            sidebar_accent: oklch(0.930, 0.014, 264.),

            primary: oklch(0.520, 0.200, 259.),
            primary_foreground: oklch(0.985, 0.003, 259.),
            secondary: oklch(0.960, 0.006, 264.),
            secondary_foreground: oklch(0.250, 0.015, 264.),
            accent: oklch(0.955, 0.012, 264.),
            accent_foreground: oklch(0.220, 0.020, 264.),
            destructive: oklch(0.550, 0.200, 27.),
            destructive_foreground: oklch(0.990, 0.005, 27.),
            warning: oklch(0.740, 0.160, 65.),
            warning_foreground: oklch(0.440, 0.100, 55.),

            input: oklch(0.900, 0.008, 264.),
            ring: oklch(0.520, 0.200, 259.),
            chart: [
                oklch(0.62, 0.16, 259.),
                oklch(0.62, 0.14, 175.),
                oklch(0.68, 0.14, 70.),
                oklch(0.60, 0.18, 320.),
                oklch(0.64, 0.16, 30.),
            ],
            radius: px(8.),
            font: FontTokens::default(),
        }
    }

    pub fn dark() -> Self {
        Self {
            is_dark: true,
            background: gpui::rgb(0x181818).into(),
            foreground: oklch(0.970, 0.004, 264.),
            muted: oklch(0.240, 0.010, 264.),
            muted_foreground: oklch(0.700, 0.015, 264.),
            border: gpui::rgb(0x343434).into(),

            card: gpui::rgb(0x222222).into(),
            popover: oklch(0.220, 0.009, 264.),
            sidebar: gpui::rgb(0x333333).into(),
            sidebar_foreground: oklch(0.940, 0.004, 264.),
            sidebar_border: oklch(0.260, 0.012, 264.),
            sidebar_accent: gpui::rgb(0x454545).into(),

            primary: oklch(0.680, 0.160, 259.), // lighter, less chroma
            primary_foreground: oklch(0.170, 0.020, 259.),
            secondary: oklch(0.270, 0.012, 264.),
            secondary_foreground: oklch(0.960, 0.004, 264.),
            accent: oklch(0.300, 0.016, 264.),
            accent_foreground: oklch(0.970, 0.004, 264.),
            destructive: oklch(0.550, 0.200, 27.),
            destructive_foreground: oklch(0.990, 0.005, 27.),
            warning: oklch(0.780, 0.140, 65.),
            warning_foreground: oklch(0.820, 0.120, 65.),

            input: oklch(0.300, 0.012, 264.),
            ring: oklch(0.680, 0.160, 259.),
            chart: [
                oklch(0.72, 0.15, 259.),
                oklch(0.72, 0.13, 175.),
                oklch(0.78, 0.13, 70.),
                oklch(0.70, 0.16, 320.),
                oklch(0.74, 0.15, 30.),
            ],
            radius: px(8.),
            font: FontTokens::default(),
        }
    }
}

/// Kept independently from the resolved palette so System remains selected
/// when the operating system changes appearance. Preserved across settings windows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppearancePreference {
    System,
    #[default]
    Light,
    Dark,
}

impl Global for AppearancePreference {}

impl AppearancePreference {
    fn is_dark(self, appearance: gpui::WindowAppearance) -> bool {
        match self {
            Self::System => matches!(
                appearance,
                gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
            ),
            Self::Light => false,
            Self::Dark => true,
        }
    }

    pub fn apply(self, _appearance: gpui::WindowAppearance, cx: &mut App) {
        cx.set_global(self);
        // Keep native window edges in sync with the content theme. Clearing the
        // override first also restores the actual OS appearance for System.
        cx.set_window_appearance(match self {
            Self::System => None,
            Self::Light => Some(gpui::WindowAppearance::Light),
            Self::Dark => Some(gpui::WindowAppearance::Dark),
        });
        let spec = if self.is_dark(cx.window_appearance()) {
            ThemeSpec::dark()
        } else {
            ThemeSpec::light()
        };
        cx.set_global(spec.resolve(&Derivation::default()));
    }
}

pub fn follow_system_appearance(window: &gpui::Window, cx: &mut App) {
    if *cx.global::<AppearancePreference>() == AppearancePreference::System {
        AppearancePreference::System.apply(window.appearance(), cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(theme: &Theme, name: &str) {
        let pairs: &[(&str, Hsla, Hsla)] = &[
            ("app", theme.app.bg, theme.app.fg),
            (
                "selected segment",
                theme.selected_segment_bg,
                theme.selected_segment_fg,
            ),
            ("app/muted", theme.app.bg, theme.app.muted_fg),
            ("card", theme.card.bg, theme.card.fg),
            ("popover", theme.popover.bg, theme.popover.fg),
            ("sidebar", theme.sidebar.bg, theme.sidebar.fg),
            (
                "sidebar contents",
                theme.sidebar_content.bg,
                theme.sidebar_content.fg,
            ),
            ("active file", theme.active_file_bg, theme.active_file_fg),
            ("primary", theme.app.primary, theme.app.primary_fg),
            ("secondary", theme.secondary, theme.secondary_fg),
            (
                "warning badge",
                theme.popover.bg.blend(theme.warning_subtle),
                theme.warning_fg,
            ),
            ("accent", theme.app.accent, theme.app.accent_fg),
            ("destructive", theme.destructive, theme.destructive_fg),
            (
                "dialog destructive",
                theme.dialog_destructive_bg,
                theme.dialog_destructive_fg,
            ),
            (
                "dialog destructive hover",
                theme.dialog_destructive_hover,
                theme.dialog_destructive_fg,
            ),
            (
                "dialog destructive active",
                theme.dialog_destructive_active,
                theme.dialog_destructive_fg,
            ),
        ];
        for (label, bg, fg) in pairs {
            let ratio = contrast(*bg, *fg);
            assert!(ratio >= 4.5, "{name}: {label} contrast {ratio:.2} < 4.5");
        }
        for color in theme.syntax {
            assert!(
                contrast(theme.card.bg, color) >= 4.5,
                "{name}: syntax contrast"
            );
        }
        // Borders need to be visible, not readable.
        assert!(
            contrast(theme.app.bg, theme.app.border) >= 1.2,
            "{name}: border invisible"
        );
    }

    #[test]
    fn system_follows_os_while_explicit_choices_override_it() {
        use gpui::WindowAppearance::{Dark, Light, VibrantDark, VibrantLight};
        for appearance in [Light, VibrantLight, Dark, VibrantDark] {
            assert!(!AppearancePreference::Light.is_dark(appearance));
            assert!(AppearancePreference::Dark.is_dark(appearance));
            assert_eq!(
                AppearancePreference::System.is_dark(appearance),
                matches!(appearance, Dark | VibrantDark)
            );
        }
    }

    #[test]
    fn themes_are_legible() {
        let d = Derivation::default();
        check(&ThemeSpec::light().resolve(&d), "light");
        check(&ThemeSpec::dark().resolve(&d), "dark");
    }
}
