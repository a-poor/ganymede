use gpui::{
    App, Bounds, Context, FontWeight, TitlebarOptions, Window, WindowBounds, WindowControlArea,
    WindowHandle, WindowOptions, div, point, prelude::*, px, size,
};

use crate::{
    icon::Icon,
    theme::{self, ActiveTheme, AppearancePreference, Theme},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Appearance,
    Notebooks,
}
pub struct Settings {
    section: Section,
}

pub fn open(cx: &mut App) -> anyhow::Result<WindowHandle<Settings>> {
    let bounds = Bounds::centered(None, size(px(620.), px(400.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(560.), px(300.))),
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
                Settings {
                    section: Section::Appearance,
                }
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
                })
            })
            .on_key_down(|event, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.update_global::<crate::autosave::AutosavePreference, _>(|setting, _| {
                        setting.0 = !setting.0
                    });
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
                            .on_click(move |_, window, cx| choice.apply(window.appearance(), cx))
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
            .gap_1()
            .p_1()
            .bg(t.app.bg)
            .text_color(t.app.fg)
            .font_family(t.font.sans.clone())
            .text_size(t.font.sm)
            .child(
                div()
                    .w(px(152.))
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .bg(t.sidebar.bg)
                    .text_color(t.sidebar.fg)
                    .border_1()
                    .border_color(t.sidebar.border)
                    .rounded(t.radius.xl2())
                    .child(
                        div()
                            .h(px(44.))
                            .flex_shrink_0()
                            .window_control_area(WindowControlArea::Drag),
                    )
                    .children(
                        [
                            (Section::Appearance, "Appearance", Icon::Palette),
                            (Section::Notebooks, "Notebooks", Icon::Notebook),
                        ]
                        .into_iter()
                        .map(|(section, label, icon)| {
                            let selected = section == self.section;
                            div()
                                .id(label)
                                .mx_1()
                                .h(px(28.))
                                .px_2()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .rounded(t.radius.md())
                                .cursor_pointer()
                                .tab_index(0)
                                .when(selected, |s| {
                                    s.bg(t.active_file_bg).text_color(t.active_file_fg)
                                })
                                .when(!selected, |s| {
                                    s.text_color(t.sidebar.fg).hover(|s| s.bg(t.rail_hover_bg))
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.section = section;
                                    cx.notify();
                                }))
                                .on_key_down(cx.listener(
                                    move |this, event: &gpui::KeyDownEvent, _, cx| {
                                        if matches!(event.keystroke.key.as_str(), "enter" | "space")
                                        {
                                            this.section = section;
                                            cx.notify();
                                        }
                                    },
                                ))
                                .child(icon.svg().size(px(14.)).text_color(if selected {
                                    t.active_file_fg
                                } else {
                                    t.sidebar.fg
                                }))
                                .child(label)
                        }),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(44.))
                            .flex_shrink_0()
                            .px_2()
                            .flex()
                            .items_center()
                            .window_control_area(WindowControlArea::Drag)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if self.section == Section::Appearance {
                                "Appearance"
                            } else {
                                "Notebooks"
                            }),
                    )
                    .child(
                        div()
                            .id("appearance-content")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .px_2()
                            .pb_2()
                            .child(
                                div()
                                    .p_2()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .rounded(t.radius.lg())
                                    .bg(t.card.bg)
                                    .text_color(t.card.fg)
                                    .border_1()
                                    .border_color(t.card.border)
                                    .when(self.section == Section::Appearance, |view| {
                                        view.child("Theme").child(self.appearance_selector(cx))
                                    })
                                    .when(self.section == Section::Notebooks, |view| {
                                        view.child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap_1()
                                                .child("Autosave")
                                                .child(
                                                    div()
                                                        .text_size(t.font.xs)
                                                        .text_color(t.card.muted_fg)
                                                        .child(
                                                            "Save after a short pause in editing.",
                                                        ),
                                                ),
                                        )
                                        .child(self.autosave_toggle(cx))
                                    }),
                            ),
                    ),
            )
    }
}
