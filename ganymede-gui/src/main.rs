mod assets;
pub mod icon;
pub mod theme;

use gpui::{
    App, Bounds, Context, FontWeight, KeyBinding, Menu, MenuItem, TitlebarOptions, Window,
    WindowBounds, WindowControlArea, WindowOptions, actions, div, point, prelude::*, px, size,
};
use icon::Icon;
use theme::{ActiveTheme, Theme};

actions!(ganymede, [Quit]);

struct Ganymede;

impl Ganymede {
    fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Keep the reference palette consistent while exploring the window shell.
        cx.set_global(theme::ThemeSpec::dark().resolve(&theme::Derivation::default()));
        cx.observe_global::<Theme>(|_, cx| cx.notify()).detach();
        Self
    }
}

impl Render for Ganymede {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let outer = theme.sidebar;
        let sidebar = theme.card;
        let body = theme.app;
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(outer.bg)
            .text_color(outer.fg)
            .font_family(theme.font.sans.clone())
            .text_size(theme.font.base)
            .child(
                div()
                    .id("titlebar")
                    .h(px(44.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .pl(px(88.))
                    .text_size(theme.font.sm)
                    .text_color(outer.muted_fg)
                    .window_control_area(WindowControlArea::Drag)
                    .on_click(|event, window, _| {
                        if event.click_count() == 2 {
                            window.titlebar_double_click();
                        }
                    })
                    .child("Ganymede"),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .pr(px(4.))
                    .pb(px(4.))
                    .child(
                        div()
                            .w(px(52.))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .items_center()
                            .py_2()
                            .gap_3()
                            .child(
                                div()
                                    .size(px(36.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(theme.radius.md())
                                    .bg(outer.accent)
                                    .text_color(outer.accent_fg)
                                    .text_size(theme.font.xl)
                                    .child(
                                        Icon::SquaresFour
                                            .svg()
                                            .size(theme.font.xl)
                                            .text_color(outer.accent_fg),
                                    ),
                            )
                            .child(
                                Icon::Clock
                                    .svg()
                                    .size(theme.font.xl)
                                    .text_color(outer.muted_fg),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .rounded(theme.radius.xl())
                            .shadow(vec![theme.body_shadow.clone()])
                            .when(theme.is_dark, |el| el.border_1().border_color(body.border))
                            .overflow_hidden()
                            .bg(outer.bg)
                            .text_color(outer.fg)
                            .child(
                                div()
                                    .w(px(240.))
                                    .flex_shrink_0()
                                    .h_full()
                                    .flex()
                                    .flex_col()
                                    .gap_6()
                                    .p_4()
                                    .bg(sidebar.bg)
                                    .text_color(sidebar.fg)
                                    .rounded_l(theme.radius.xl())
                                    .border_r_1()
                                    .border_color(sidebar.border)
                                    .child(
                                        div().font_weight(FontWeight::SEMIBOLD).child("Workspace"),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_3()
                                            .text_size(theme.font.sm)
                                            .text_color(sidebar.muted_fg)
                                            .child("Notebooks")
                                            .child("No notebooks open"),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .bg(body.bg)
                                    .text_color(body.fg)
                                    .rounded_r(theme.radius.xl())
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .justify_center()
                                    .gap_3()
                                    .p_6()
                                    .child(
                                        div()
                                            .text_size(theme.font.xl)
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("Ganymede"),
                                    )
                                    .child(
                                        div()
                                            .text_color(body.muted_fg)
                                            .child("A native workspace for Jupyter notebooks."),
                                    ),
                            ),
                    ),
            )
    }
}

fn main() {
    gpui_platform::application()
        .with_assets(assets::Assets)
        .run(|cx: &mut App| {
            assets::register_fonts(cx).expect("failed to register embedded fonts");
            cx.on_action(|_: &Quit, cx| cx.quit());
            let quit_key = if cfg!(target_os = "macos") {
                "cmd-q"
            } else {
                "ctrl-q"
            };
            cx.bind_keys([KeyBinding::new(quit_key, Quit, None)]);
            cx.set_menus(vec![
                Menu::new("Ganymede").items([MenuItem::action("Quit Ganymede", Quit)]),
            ]);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let bounds = Bounds::centered(None, size(px(1000.), px(700.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(480.), px(320.))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Ganymede".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(12.), px(14.))),
                    }),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Ganymede::new(window, cx)),
            )
            .expect("failed to open the Ganymede window");
            cx.activate(true);
        });
}
