use gpui::{
    App, Bounds, Context, FontWeight, TitlebarOptions, Window, WindowBounds, WindowControlArea,
    WindowHandle, WindowOptions, div, point, prelude::*, px, size,
};

use crate::{
    icon::Icon,
    theme::{self, ActiveTheme, AppearancePreference, Theme},
};

pub struct Settings;

pub fn open(cx: &mut App) -> anyhow::Result<WindowHandle<Settings>> {
    let bounds = Bounds::centered(None, size(px(520.), px(240.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(480.), px(220.))),
            titlebar: Some(TitlebarOptions {
                title: Some("Settings".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(16.), px(16.))),
            }),
            ..Default::default()
        },
        |window, cx| {
            cx.new(|cx| {
                cx.observe_global::<Theme>(|_, cx| cx.notify()).detach();
                cx.observe_global::<AppearancePreference>(|_, cx| cx.notify())
                    .detach();
                cx.observe_window_appearance(window, |_, window, cx| {
                    theme::follow_system_appearance(window, cx);
                })
                .detach();
                cx.observe_global::<crate::autosave::AutosavePreference>(|_, cx| cx.notify())
                    .detach();
                cx.observe_global::<crate::preferences::Persistence>(|_, cx| cx.notify())
                    .detach();
                Settings
            })
        },
    )
}

impl Settings {
    fn autosave_toggle(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let enabled = cx.global::<crate::autosave::AutosavePreference>().0;
        div()
            .id("autosave-toggle")
            .aria_label(if enabled {
                "Autosave on"
            } else {
                "Autosave off"
            })
            .tab_index(0)
            .w(px(32.))
            .h(px(20.))
            .p(px(2.))
            .rounded_full()
            .flex()
            .items_center()
            .flex_shrink_0()
            .when(enabled, |s| {
                s.bg(t.card.primary)
                    .text_color(t.card.primary_fg)
                    .justify_end()
            })
            .when(!enabled, |s| {
                s.bg(t.secondary).text_color(t.secondary_fg).justify_start()
            })
            .cursor_pointer()
            .focus(|s| s.border_1().border_color(t.card.ring))
            .on_click(|_, _, cx| {
                cx.update_global::<crate::autosave::AutosavePreference, _>(|setting, _| {
                    setting.0 = !setting.0
                });
                crate::preferences::save(cx);
            })
            .on_key_down(|event, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.update_global::<crate::autosave::AutosavePreference, _>(|setting, _| {
                        setting.0 = !setting.0
                    });
                    crate::preferences::save(cx);
                    cx.stop_propagation();
                }
            })
            .child(
                div()
                    .size(px(16.))
                    .rounded_full()
                    .bg(t.selected_segment_bg)
                    .text_color(t.selected_segment_fg),
            )
    }
    fn appearance_selector(&self, cx: &Context<Self>) -> gpui::Div {
        let t = cx.theme();
        let preference = *cx.global::<AppearancePreference>();
        let options = [
            (AppearancePreference::System, "System", Icon::Desktop),
            (AppearancePreference::Light, "Light", Icon::Sun),
            (AppearancePreference::Dark, "Dark", Icon::Moon),
        ];
        div()
            .flex()
            .items_center()
            .flex_shrink_0()
            .gap(px(2.))
            .p(px(3.))
            .rounded_full()
            .bg(t.segment_track_bg)
            .text_color(t.sidebar.accent_fg)
            .children(
                options
                    .into_iter()
                    .enumerate()
                    .map(|(index, (choice, label, icon))| {
                        let selected = preference == choice;
                        let fg = if selected {
                            t.selected_segment_fg
                        } else {
                            t.sidebar.accent_fg
                        };
                        div()
                            .id(label)
                            .aria_label(label)
                            .tab_index(index as isize)
                            .h(px(26.))
                            .px_2()
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap_1()
                            .rounded_full()
                            .cursor_pointer()
                            .text_color(fg)
                            .when(selected, |s| {
                                s.bg(t.selected_segment_bg)
                                    .text_color(t.selected_segment_fg)
                                    .shadow(vec![
                                        t.selected_segment_shadow.clone(),
                                        t.selected_segment_edge.clone(),
                                    ])
                            })
                            .when(!selected, |s| {
                                s.hover(|s| s.bg(t.sidebar.bg).text_color(t.sidebar.fg))
                            })
                            .on_click(move |_, window, cx| {
                                choice.apply(window.appearance(), cx);
                                crate::preferences::save(cx);
                            })
                            .on_key_down(move |event, window, cx| {
                                let selected_index = options
                                    .iter()
                                    .position(|(value, _, _)| {
                                        value == cx.global::<AppearancePreference>()
                                    })
                                    .unwrap_or(index);
                                let next = match event.keystroke.key.as_str() {
                                    "enter" | "space" => Some(choice),
                                    "left" => Some(options[(selected_index + 2) % 3].0),
                                    "right" => Some(options[(selected_index + 1) % 3].0),
                                    _ => None,
                                };
                                if let Some(next) = next {
                                    next.apply(window.appearance(), cx);
                                    crate::preferences::save(cx);
                                    cx.stop_propagation();
                                }
                            })
                            .child(icon.svg().size(px(14.)).text_color(fg))
                            .child(label)
                    }),
            )
    }
}

impl Render for Settings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        div()
            .id("settings-root")
            .on_key_down(|event, window, cx| {
                if event.keystroke.key == "tab" {
                    if event.keystroke.modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                    cx.stop_propagation();
                }
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
                    .flex()
                    .items_center()
                    .pl(px(84.))
                    .window_control_area(WindowControlArea::Drag)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Settings"),
            )
            .child(
                div()
                    .id("settings-content")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .rounded(t.radius.lg())
                            .bg(t.card.bg)
                            .text_color(t.card.fg)
                            .border_1()
                            .border_color(t.card.border)
                            .child(
                                div()
                                    .p_3()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child("Theme")
                                    .child(self.appearance_selector(cx)),
                            )
                            .child(div().mx_3().h(px(1.)).bg(t.card.border))
                            .child(
                                div()
                                    .p_3()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child(
                                        div().flex().flex_col().gap_1().child("Autosave").child(
                                            div()
                                                .text_size(t.font.xs)
                                                .text_color(t.card.muted_fg)
                                                .child("Save after a short pause in editing."),
                                        ),
                                    )
                                    .child(self.autosave_toggle(cx)),
                            ),
                    ),
            )
            .when_some(
                cx.global::<crate::preferences::Persistence>().error.clone(),
                |view, error| {
                    view.child(
                        div()
                            .px_3()
                            .pb_3()
                            .text_size(t.font.xs)
                            .text_color(t.destructive)
                            .child(error),
                    )
                },
            )
    }
}
