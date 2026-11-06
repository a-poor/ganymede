use crate::theme::{self, ActiveTheme, Theme};
use gpui::{
    App, Bounds, Context, TitlebarOptions, Window, WindowBounds, WindowControlArea, WindowHandle,
    WindowOptions, div, point, prelude::*, px, size,
};

pub struct About;
#[derive(Default)]
struct AboutWindow(Option<WindowHandle<About>>);
impl gpui::Global for AboutWindow {}

pub fn open(cx: &mut App) {
    if let Some(handle) = cx.try_global::<AboutWindow>().and_then(|w| w.0)
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let bounds = Bounds::centered(None, size(px(420.), px(280.)), cx);
    match cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            is_resizable: false,
            titlebar: Some(TitlebarOptions {
                title: Some("About Ganymede".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(16.), px(16.))),
            }),
            ..Default::default()
        },
        |window, cx| {
            cx.new(|cx| {
                cx.observe_global::<Theme>(|_, cx| cx.notify()).detach();
                cx.observe_window_appearance(window, |_, window, cx| {
                    theme::follow_system_appearance(window, cx)
                })
                .detach();
                About
            })
        },
    ) {
        Ok(handle) => cx.set_global(AboutWindow(Some(handle))),
        Err(error) => eprintln!("Could not open About Ganymede: {error}"),
    }
}
impl Render for About {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.app.bg)
            .text_color(t.app.fg)
            .font_family(t.font.sans.clone())
            .text_size(t.font.sm)
            .child(
                div()
                    .h(px(48.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .window_control_area(WindowControlArea::Drag)
                    .child("About Ganymede"),
            )
            .child(
                div()
                    .p_6()
                    .flex()
                    .flex_col()
                    .items_center()
                    .text_center()
                    .gap_3()
                    .child(
                        div()
                            .font_family(t.font.brand.clone())
                            .text_size(t.font.brand_size * 0.5)
                            .font_weight(gpui::FontWeight::NORMAL)
                            .child("Ganymede"),
                    )
                    .child(
                        div()
                            .text_color(t.app.muted_fg)
                            .child(format!("Version {}", env!("CARGO_PKG_VERSION"))),
                    )
                    .child("A native workspace for Jupyter notebooks.")
                    .child(
                        div()
                            .flex()
                            .justify_center()
                            .gap_4()
                            .child(link(
                                "Source & documentation",
                                "https://github.com/a-poor/ganymede",
                                cx,
                            ))
                            .child(link(
                                "Report an issue",
                                "https://github.com/a-poor/ganymede/issues",
                                cx,
                            )),
                    ),
            )
    }
}
fn link(label: &'static str, url: &'static str, cx: &App) -> impl IntoElement {
    let t = cx.theme();
    div()
        .id(label)
        .tab_index(0)
        .cursor_pointer()
        .text_color(t.app.primary)
        .hover(|s| s.underline())
        .on_click(move |_, _, cx| cx.open_url(url))
        .on_key_down(move |e, _, cx| {
            if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                cx.open_url(url);
                cx.stop_propagation();
            }
        })
        .child(label)
}
