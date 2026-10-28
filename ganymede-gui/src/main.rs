pub mod theme;

use gpui::{
    App, Bounds, Context, FontWeight, KeyBinding, Menu, MenuItem, Subscription, TitlebarOptions,
    Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, size,
};
use theme::{ActiveTheme, Theme};

actions!(ganymede, [Quit]);

struct Ganymede {
    _appearance: Subscription,
}

impl Ganymede {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        theme::set_appearance(window.appearance(), cx);
        cx.observe_global::<Theme>(|_, cx| cx.notify()).detach();
        Self {
            _appearance: window.observe_window_appearance(|window, cx| {
                theme::set_appearance(window.appearance(), cx);
            }),
        }
    }
}

impl Render for Ganymede {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let surface = theme.app;
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .p_8()
            .bg(surface.bg)
            .text_color(surface.fg)
            .font_family(theme.font.sans.clone())
            .text_size(theme.font.base)
            .child(
                div()
                    .text_size(theme.font.xl)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Ganymede"),
            )
            .child(
                div()
                    .text_color(surface.muted_fg)
                    .child("A native workspace for Jupyter notebooks."),
            )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
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
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Ganymede::new(window, cx)),
        )
        .expect("failed to open the Ganymede window");
        cx.activate(true);
    });
}
