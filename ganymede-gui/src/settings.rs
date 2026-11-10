use gpui::{
    App, Bounds, Context, FontWeight, TitlebarOptions, Window, WindowBounds, WindowControlArea,
    WindowHandle, WindowOptions, div, point, prelude::*, px, size,
};

use crate::{
    icon::Icon,
    theme::{self, ActiveTheme, AppearancePreference, Theme},
};

pub struct Settings {
    focus: gpui::FocusHandle,
    app_menu: gpui::Entity<crate::menus::in_window::WindowMenu>,
    dropdown: Option<Setting>,
    selected: usize,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Setting {
    Indentation,
    FontSize,
    RunCell,
}
impl Setting {
    fn label(self) -> &'static str {
        match self {
            Self::Indentation => "Indentation",
            Self::FontSize => "Code font size",
            Self::RunCell => "Run cell shortcut",
        }
    }
    fn choices(self) -> &'static [&'static str] {
        match self {
            Self::Indentation => &["2 spaces", "4 spaces", "8 spaces"],
            Self::FontSize => &["10", "12", "14", "16", "18", "20", "24"],
            Self::RunCell => &["Enter", "Shift+Enter"],
        }
    }
    fn index(self, cx: &App) -> usize {
        let p = crate::preferences::editor(cx);
        match self {
            Self::Indentation => match p.indentation {
                crate::preferences::Indentation::Two => 0,
                crate::preferences::Indentation::Four => 1,
                crate::preferences::Indentation::Eight => 2,
            },
            Self::FontSize => [10, 12, 14, 16, 18, 20, 24]
                .iter()
                .position(|v| *v == p.font_size)
                .unwrap_or(2),
            Self::RunCell => usize::from(!p.run_cell_on_enter),
        }
    }
    fn apply(self, index: usize, cx: &mut App) {
        cx.update_global::<crate::preferences::EditorPreferences, _>(|p, _| match self {
            Self::Indentation => {
                p.indentation = [
                    crate::preferences::Indentation::Two,
                    crate::preferences::Indentation::Four,
                    crate::preferences::Indentation::Eight,
                ][index]
            }
            Self::FontSize => p.font_size = [10, 12, 14, 16, 18, 20, 24][index],
            Self::RunCell => p.run_cell_on_enter = index == 0,
        });
        crate::preferences::save(cx);
    }
}

#[derive(Clone, Copy)]
enum ToggleSetting {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    MenuBar,
    Autosave,
    LineNumbers,
    AutoCloseBrackets,
}
impl ToggleSetting {
    fn enabled(self, cx: &App) -> bool {
        match self {
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            Self::MenuBar => cx.global::<crate::preferences::MenuBarPreference>().0,
            Self::Autosave => cx.global::<crate::autosave::AutosavePreference>().0,
            Self::LineNumbers => crate::preferences::editor(cx).line_numbers,
            Self::AutoCloseBrackets => crate::preferences::editor(cx).auto_close_brackets,
        }
    }
    fn toggle(self, cx: &mut App) {
        match self {
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            Self::MenuBar => {
                cx.update_global::<crate::preferences::MenuBarPreference, _>(|p, _| p.0 = !p.0);
                crate::tray::refresh(cx);
            }
            Self::Autosave => {
                cx.update_global::<crate::autosave::AutosavePreference, _>(|p, _| p.0 = !p.0)
            }
            Self::AutoCloseBrackets => cx
                .update_global::<crate::preferences::EditorPreferences, _>(|p, _| {
                    p.auto_close_brackets = !p.auto_close_brackets
                }),
            Self::LineNumbers => {
                cx.update_global::<crate::preferences::EditorPreferences, _>(|p, _| {
                    p.line_numbers = !p.line_numbers
                })
            }
        }
        crate::preferences::save(cx);
    }
}

pub fn open(cx: &mut App) -> anyhow::Result<WindowHandle<Settings>> {
    let bounds = Bounds::centered(None, size(px(520.), px(420.)), cx);
    cx.open_window(
        WindowOptions {
            app_id: Some("com.austinpoor.ganymede".into()),
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            #[cfg(target_os = "linux")]
            window_decorations: Some(gpui::WindowDecorations::Client),
            #[cfg(target_os = "linux")]
            window_background: gpui::WindowBackgroundAppearance::Transparent,
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
                cx.observe_global::<crate::preferences::EditorPreferences>(|_, cx| cx.notify())
                    .detach();
                let focus = cx.focus_handle();
                focus.focus(window, cx);
                Settings {
                    focus,
                    app_menu: cx.new(|cx| {
                        crate::menus::in_window::WindowMenu::new(
                            crate::menus::in_window::Kind::Settings,
                            cx,
                        )
                    }),
                    dropdown: None,
                    selected: 0,
                }
            })
        },
    )
}

impl Settings {
    fn setting_row(&self, setting: Setting, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let open = self.dropdown == Some(setting);
        let active = setting.index(cx);
        div()
            .p_3()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .child(setting.label())
            .child(
                div()
                    .id(setting.label())
                    .relative()
                    .child(
                        div()
                            .id("setting-selector")
                            .aria_label(setting.label())
                            .tab_index(0)
                            .w(px(148.))
                            .h(px(28.))
                            .px_2()
                            .flex()
                            .items_center()
                            .justify_between()
                            .rounded(t.radius.md())
                            .border_1()
                            .border_color(t.input)
                            .bg(t.card.bg)
                            .text_color(t.card.fg)
                            .cursor_pointer()
                            .hover(|s| s.bg(t.card.accent).text_color(t.card.accent_fg))
                            .focus_visible(|s| s.border_color(t.card.ring))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.dropdown = if open { None } else { Some(setting) };
                                this.selected = active;
                                cx.notify();
                            }))
                            .on_key_down(cx.listener(move |this, e: &gpui::KeyDownEvent, _, cx| {
                                let count = setting.choices().len();
                                match e.keystroke.key.as_str() {
                                    "escape" | "tab" => this.dropdown = None,
                                    "up" | "down" => {
                                        if this.dropdown != Some(setting) {
                                            this.dropdown = Some(setting);
                                            this.selected = active;
                                        } else {
                                            this.selected = (this.selected
                                                + if e.keystroke.key == "up" {
                                                    count - 1
                                                } else {
                                                    1
                                                })
                                                % count;
                                        }
                                    }
                                    "enter" | "space" => {
                                        if this.dropdown == Some(setting) {
                                            setting.apply(this.selected, cx);
                                            this.dropdown = None;
                                        } else {
                                            this.dropdown = Some(setting);
                                            this.selected = active;
                                        }
                                    }
                                    _ => return,
                                }
                                if e.keystroke.key != "tab" {
                                    cx.stop_propagation();
                                }
                                cx.notify();
                            }))
                            .child(setting.choices()[active])
                            .child(
                                Icon::CaretDown
                                    .svg()
                                    .size(px(12.))
                                    .text_color(t.card.muted_fg),
                            ),
                    )
                    .when(open, |view| {
                        view.child(gpui::deferred(
                            gpui::anchored().snap_to_window_with_margin(px(8.)).child(
                                div()
                                    .id("setting-options")
                                    .on_mouse_down_out(cx.listener(move |this, _, _, cx| {
                                        if this.dropdown == Some(setting) {
                                            this.dropdown = None;
                                            cx.notify();
                                        }
                                    }))
                                    .occlude()
                                    .w(px(148.))
                                    .p_1()
                                    .flex()
                                    .flex_col()
                                    .rounded(t.radius.md())
                                    .bg(t.popover.bg)
                                    .text_color(t.popover.fg)
                                    .border_1()
                                    .border_color(t.popover.border)
                                    .shadow(vec![t.toast_shadow.clone()])
                                    .children(setting.choices().iter().enumerate().map(
                                        |(index, label)| {
                                            div()
                                                .id(index)
                                                .h(px(28.))
                                                .px_2()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .rounded(t.radius.sm())
                                                .cursor_pointer()
                                                .when(self.selected == index, |s| {
                                                    s.bg(t.popover.accent)
                                                        .text_color(t.popover.accent_fg)
                                                })
                                                .on_hover(cx.listener(
                                                    move |this, hovered, _, cx| {
                                                        if *hovered {
                                                            this.selected = index;
                                                            cx.notify();
                                                        }
                                                    },
                                                ))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    setting.apply(index, cx);
                                                    this.dropdown = None;
                                                    cx.notify();
                                                }))
                                                .child(div().w(px(12.)).when(
                                                    active == index,
                                                    |s| {
                                                        s.child(
                                                            Icon::Check
                                                                .svg()
                                                                .size(px(12.))
                                                                .text_color(t.popover.fg),
                                                        )
                                                    },
                                                ))
                                                .child(*label)
                                        },
                                    )),
                            ),
                        ))
                    }),
            )
    }
    fn toggle(&self, setting: ToggleSetting, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let enabled = setting.enabled(cx);
        let (id, label) = match setting {
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            ToggleSetting::MenuBar => ("menu-bar-toggle", crate::tray::setting_label()),
            ToggleSetting::Autosave => ("autosave-toggle", "Autosave"),
            ToggleSetting::LineNumbers => ("line-numbers-toggle", "Line numbers"),
            ToggleSetting::AutoCloseBrackets => {
                ("auto-close-brackets-toggle", "Auto close brackets")
            }
        };
        div()
            .id(id)
            .aria_label(format!("{label} {}", if enabled { "on" } else { "off" }))
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
            .on_click(move |_, _, cx| setting.toggle(cx))
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        let content = self.render_content(rounded, cx);
        div()
            .size_full()
            .p(inset)
            .map(|frame| {
                #[cfg(target_os = "linux")]
                let frame = frame.when(rounded, |frame| {
                    frame.on_mouse_down(gpui::MouseButton::Left, move |event, window, _| {
                        if let Some(edge) = crate::connection::linux_frame::resize_edge(
                            event.position,
                            window.viewport_size(),
                            inset,
                        ) {
                            window.start_window_resize(edge);
                        }
                    })
                });
                frame
            })
            .child(content)
    }
}

impl Settings {
    fn render_content(&self, rounded: bool, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        div()
            .id("settings-root")
            .when(rounded, |root| {
                root.rounded(t.radius.xl2()).p(t.radius.xl2() * 0.3)
            })
            .track_focus(&self.focus)
            .on_action(
                cx.listener(|this, _: &crate::menus::in_window::OpenMenu, window, cx| {
                    this.app_menu
                        .update(cx, |menu, cx| menu.open_first(window, cx));
                }),
            )
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
                    .pl(px(if cfg!(target_os = "linux") { 16. } else { 84. }))
                    .window_control_area(WindowControlArea::Drag)
                    .when(cfg!(target_os = "linux"), |bar| {
                        bar.on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                            window.start_window_move()
                        })
                    })
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Settings")
                    .when(cfg!(target_os = "linux"), |bar| {
                        bar.child(
                            div()
                                .ml_auto()
                                .mr_4()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(self.app_menu.clone())
                                .child(
                                    crate::connection::button("settings-close", "", false, cx)
                                        .aria_label("Close Settings")
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
                                            if matches!(
                                                event.keystroke.key.as_str(),
                                                "enter" | "space"
                                            ) {
                                                cx.stop_propagation();
                                                window.remove_window();
                                            }
                                        })
                                        .child(Icon::X.svg().size(px(14.)).text_color(t.app.fg)),
                                ),
                        )
                    }),
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
                            .children({
                                #[cfg(any(target_os = "macos", target_os = "linux"))]
                                {
                                    Some(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .child(div().mx_3().h(px(1.)).bg(t.card.border))
                                            .child(
                                                div()
                                                    .p_3()
                                                    .flex()
                                                    .items_center()
                                                    .justify_between()
                                                    .child(crate::tray::setting_label())
                                                    .child(self.toggle(ToggleSetting::MenuBar, cx)),
                                            ),
                                    )
                                }
                                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                                {
                                    None::<gpui::Div>
                                }
                            })
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
                                    .child(self.toggle(ToggleSetting::Autosave, cx)),
                            )
                            .child(div().mx_3().h(px(1.)).bg(t.card.border))
                            .child(
                                div()
                                    .p_3()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child("Line numbers")
                                    .child(self.toggle(ToggleSetting::LineNumbers, cx)),
                            )
                            .child(div().mx_3().h(px(1.)).bg(t.card.border))
                            .child(
                                div()
                                    .p_3()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child("Auto close brackets")
                                    .child(self.toggle(ToggleSetting::AutoCloseBrackets, cx)),
                            )
                            .children([Setting::Indentation, Setting::FontSize].into_iter().map(
                                |setting| {
                                    div()
                                        .child(div().mx_3().h(px(1.)).bg(t.card.border))
                                        .child(self.setting_row(setting, cx))
                                },
                            ))
                            .child(div().mx_3().h(px(1.)).bg(t.card.border))
                            .child(self.setting_row(Setting::RunCell, cx))
                            .child(
                                div()
                                    .px_3()
                                    .pb_3()
                                    .text_size(t.font.xs)
                                    .text_color(t.card.muted_fg)
                                    .child(if crate::preferences::editor(cx).run_cell_on_enter {
                                        "While editing: Enter runs; Shift+Enter adds a line."
                                    } else {
                                        "While editing: Shift+Enter runs; Enter adds a line."
                                    }),
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
