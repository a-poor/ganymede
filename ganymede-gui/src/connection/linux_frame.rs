//! Linux supplies no macOS traffic lights; both window types share this frame.
use super::{ConnectionWindow, button};
use crate::{icon::Icon, theme::ActiveTheme};
use gpui::{
    AnyElement, Context, Decorations, MouseButton, Pixels, Point, ResizeEdge, Size, Window, div,
    prelude::*, px,
};

#[derive(Clone, Copy)]
enum WindowAction {
    Minimize,
    Maximize,
    Close,
}

impl ConnectionWindow {
    fn window_action(&mut self, action: WindowAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            WindowAction::Minimize => window.minimize_window(),
            WindowAction::Maximize => window.zoom_window(),
            // Preserve the existing unsaved-document confirmation flow.
            WindowAction::Close => self.close(false, window, cx),
        }
    }

    pub(super) fn linux_frame(
        &self,
        content: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if window.is_fullscreen() || matches!(window.window_decorations(), Decorations::Server) {
            window.set_client_inset(px(0.));
            return content;
        }
        let t = cx.theme();
        let surface = t.sidebar;
        let tiled = window.is_maximized()
            || matches!(window.window_decorations(), Decorations::Client { tiling } if tiling.is_tiled());
        let inset = if tiled { px(0.) } else { px(5.) };
        window.set_client_inset(inset);
        let workspace = self.workspace.is_some();
        let mut actions = Vec::new();
        if workspace {
            actions.push(("linux-minimize", Icon::Minus, WindowAction::Minimize));
            actions.push((
                "linux-maximize",
                if window.is_maximized() {
                    Icon::Copy
                } else {
                    Icon::Square
                },
                WindowAction::Maximize,
            ));
        }
        actions.push(("linux-close", Icon::X, WindowAction::Close));
        // Float over the upper-right corner; the workspace toolbar reserves
        // this width so its actions stay clear of the window controls.
        let controls = div()
            .id("linux-window-controls")
            .absolute()
            .top_0()
            .right_0()
            .h(px(48.))
            .w(px(if workspace { 128. } else { 96. }))
            .on_mouse_down(MouseButton::Left, move |event, window, _| {
                if workspace && event.click_count == 2 {
                    window.zoom_window();
                } else {
                    window.start_window_move();
                }
            })
            .on_mouse_down(MouseButton::Right, |event, window, _| {
                window.show_window_menu(event.position);
            })
            .child(
                div()
                    .absolute()
                    .top(px(14.))
                    .right(px(18.))
                    .flex()
                    .gap_1()
                    .when(!workspace, |row| row.child(self.app_menu.clone()))
                    .children(actions.into_iter().map(|(id, icon, action)| {
                        button(id, "", false, cx)
                            .p_0()
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .active(|s| s.bg(t.secondary_active))
                            .occlude()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(icon.svg().size(px(14.)).text_color(t.app.fg))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.window_action(action, window, cx);
                            }))
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, window, cx| {
                                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                        cx.stop_propagation();
                                        this.window_action(action, window, cx);
                                    }
                                },
                            ))
                    })),
            );
        div()
            .id("linux-frame")
            .when(!workspace, |frame| {
                frame.on_action(cx.listener(
                    |this, _: &crate::menus::in_window::OpenMenu, window, cx| {
                        this.app_menu
                            .update(cx, |menu, cx| menu.open_first(window, cx));
                    },
                ))
            })
            .size_full()
            .p(inset)
            .when(workspace && !tiled, |frame| {
                frame.on_mouse_down(MouseButton::Left, move |event, window, _| {
                    if let Some(edge) = resize_edge(event.position, window.viewport_size(), inset) {
                        window.start_window_resize(edge);
                    }
                })
            })
            .child(
                div()
                    .size_full()
                    .relative()
                    .flex()
                    .flex_col()
                    .bg(if workspace {
                        surface.bg
                    } else {
                        t.launcher_art_bg
                    })
                    .text_color(surface.fg)
                    .font_family(t.font.sans.clone())
                    .text_size(t.font.xs)
                    .when(!tiled, |frame| frame.rounded(t.radius.xl2()))
                    // GPUI clips children to rectangles. Keep opaque child backgrounds
                    // inside the curved perimeter instead of relying on rounded clipping.
                    .p(t.radius.xl2() * 0.3)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .overflow_hidden()
                            .child(content),
                    )
                    .child(controls),
            )
            .into_any_element()
    }
}

pub(crate) fn resize_edge(
    pos: Point<Pixels>,
    size: Size<Pixels>,
    inset: Pixels,
) -> Option<ResizeEdge> {
    let left = pos.x < inset;
    let right = pos.x > size.width - inset;
    let top = pos.y < inset;
    let bottom = pos.y > size.height - inset;
    match (left, right, top, bottom) {
        (true, _, true, _) => Some(ResizeEdge::TopLeft),
        (_, true, true, _) => Some(ResizeEdge::TopRight),
        (true, _, _, true) => Some(ResizeEdge::BottomLeft),
        (_, true, _, true) => Some(ResizeEdge::BottomRight),
        (true, _, _, _) => Some(ResizeEdge::Left),
        (_, true, _, _) => Some(ResizeEdge::Right),
        (_, _, true, _) => Some(ResizeEdge::Top),
        (_, _, _, true) => Some(ResizeEdge::Bottom),
        _ => None,
    }
}
