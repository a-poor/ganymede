use crate::theme::{self, ActiveTheme, Theme};
use gpui::{
    App, Bounds, Context, TitlebarOptions, Window, WindowBounds, WindowControlArea, WindowHandle,
    WindowOptions, div, point, prelude::*, px, size,
};

pub struct About {
    focus: gpui::FocusHandle,
}
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
            app_id: Some("com.austinpoor.ganymede".into()),
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            is_resizable: false,
            #[cfg(target_os = "linux")]
            window_decorations: Some(gpui::WindowDecorations::Client),
            #[cfg(target_os = "linux")]
            window_background: gpui::WindowBackgroundAppearance::Transparent,
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
                let focus = cx.focus_handle();
                focus.focus(window, cx);
                About { focus }
            })
        },
    ) {
        Ok(handle) => cx.set_global(AboutWindow(Some(handle))),
        Err(error) => eprintln!("Could not open About Ganymede: {error}"),
    }
}
impl Render for About {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let client_frame = cfg!(target_os = "linux")
            && !window.is_fullscreen()
            && matches!(
                window.window_decorations(),
                gpui::Decorations::Client { .. }
            );
        let rounded = client_frame
            && !window.is_maximized()
            && !matches!(window.window_decorations(), gpui::Decorations::Client { tiling } if tiling.is_tiled());
        let inset = if rounded { px(5.) } else { px(0.) };
        if cfg!(target_os = "linux") {
            window.set_client_inset(inset);
        }
        let content = div()
            .id("about-root")
            .track_focus(&self.focus)
            .on_action(|_: &crate::CloseTab, window, _| window.remove_window())
            .on_key_down(|event, window, cx| {
                match event.keystroke.key.as_str() {
                    "tab" => {
                        if event.keystroke.modifiers.shift {
                            window.focus_prev(cx);
                        } else {
                            window.focus_next(cx);
                        }
                    }
                    "escape" => window.remove_window(),
                    _ => return,
                }
                cx.stop_propagation();
            })
            .when(rounded, |v| {
                v.rounded(t.radius.xl2()).p(t.radius.xl2() * 0.3)
            })
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
                    .flex_shrink_0()
                    .relative()
                    .px(px(64.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .window_control_area(WindowControlArea::Drag)
                    .when(client_frame, |header| {
                        header.on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                            window.start_window_move()
                        })
                    })
                    .child("About Ganymede")
                    .when(client_frame, |header| {
                        header.child(
                            crate::connection::button("about-close", "", false, cx)
                                .aria_label("Close About Ganymede")
                                .absolute()
                                .top(px(10.))
                                .right(px(18.))
                                .p_0()
                                .size(px(28.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation()
                                })
                                .on_click(|_, window, cx| {
                                    cx.stop_propagation();
                                    window.remove_window();
                                })
                                .on_key_down(|event, window, cx| {
                                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                        cx.stop_propagation();
                                        window.remove_window();
                                    }
                                })
                                .child(
                                    crate::icon::Icon::X
                                        .svg()
                                        .size(px(14.))
                                        .text_color(t.app.fg),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .id("about-content")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
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
            );
        div().size_full().p(inset).child(content)
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
