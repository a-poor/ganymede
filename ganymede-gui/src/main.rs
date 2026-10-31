mod assets;
mod code_cell;
mod execution;
pub mod icon;
mod notebook;
mod settings;
pub mod theme;
pub mod toast;
mod workspace;

use gpui::{
    App, Bounds, Context, Entity, FontWeight, KeyBinding, Menu, MenuItem, TitlebarOptions, Window,
    WindowBounds, WindowControlArea, WindowOptions, actions, div, point, prelude::*, px, size,
};
use icon::Icon;
use std::time::{Duration, Instant};
use theme::{ActiveTheme, Theme};

actions!(ganymede, [Quit, NewTab, CloseTab, Save]);

#[derive(Clone, Copy, PartialEq, Eq)]
enum SidebarTab {
    Files,
    Kernels,
}

const SIDEBAR_DURATION: Duration = Duration::from_millis(220);

struct SidebarMotion {
    from: f32,
    to: f32,
    started: Instant,
}
impl SidebarMotion {
    fn value(&self, now: Instant) -> f32 {
        let progress = (now.duration_since(self.started).as_secs_f32()
            / SIDEBAR_DURATION.as_secs_f32())
        .min(1.);
        self.to + (self.from - self.to) * (1. - progress).powi(3)
    }
    fn set_open(&mut self, open: bool, now: Instant) {
        let target = if open { 1. } else { 0. };
        if self.to != target {
            self.from = self.value(now);
            self.to = target;
            self.started = now;
        }
    }
}

struct Ganymede {
    workspace: Entity<workspace::Workspace>,
    toasts: Entity<toast::ToastHost>,
    sidebar_open: bool,
    sidebar_motion: SidebarMotion,
    sidebar_tab: SidebarTab,
    settings_window: Option<gpui::WindowHandle<settings::Settings>>,
}

impl Ganymede {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        theme::AppearancePreference::default().apply(window.appearance(), cx);
        cx.observe_window_appearance(window, |_, window, cx| {
            theme::follow_system_appearance(window, cx);
        })
        .detach();
        cx.observe_global::<Theme>(|_, cx| cx.notify()).detach();
        let toasts = cx.new(|cx| toast::ToastHost::new(window, cx));
        let workspace = cx.new(|cx| workspace::Workspace::new(toasts.clone(), window, cx));
        cx.observe(&workspace, |_, _, cx| cx.notify()).detach();
        Self {
            workspace,
            toasts,
            sidebar_open: true,
            sidebar_motion: SidebarMotion {
                from: 1.,
                to: 1.,
                started: Instant::now(),
            },
            sidebar_tab: SidebarTab::Files,
            settings_window: None,
        }
    }

    fn set_sidebar_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.sidebar_open = open;
        self.sidebar_motion.set_open(open, Instant::now());
        cx.notify();
    }

    fn rail_button(
        &self,
        id: &'static str,
        icon: Icon,
        selected: bool,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = cx.theme();
        let surface = theme.sidebar;
        div()
            .id(id)
            .aria_label(id)
            .size(px(32.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.radius.md())
            .cursor_pointer()
            .tab_index(0)
            .text_color(surface.muted_fg)
            .when(selected, |el| {
                el.bg(theme.active_file_bg).text_color(theme.active_file_fg)
            })
            .hover(|s| {
                if selected {
                    s.bg(theme.active_file_bg).text_color(theme.active_file_fg)
                } else {
                    s.bg(theme.rail_hover_bg).text_color(surface.accent_fg)
                }
            })
            .active(|s| s.bg(theme.active_file_bg).text_color(theme.active_file_fg))
            .child(icon.svg().size(px(20.)).text_color(if selected {
                theme.active_file_fg
            } else {
                surface.muted_fg
            }))
    }

    fn sidebar_content(&self, cx: &Context<Self>) -> gpui::Div {
        let theme = cx.theme();
        let surface = theme.sidebar_content;
        let panel = div()
            .h_full()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .text_size(theme.font.sm)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(match self.sidebar_tab {
                        SidebarTab::Files => "Files",
                        SidebarTab::Kernels => "Kernels",
                    }),
            );
        match self.sidebar_tab {
            SidebarTab::Files => panel.child(workspace::Workspace::files(&self.workspace, cx)),
            SidebarTab::Kernels => {
                panel
                    .child(div().pt_3().child("Kernel management"))
                    .child(div().text_color(surface.muted_fg).child(
                    "Notebooks connect automatically. Kernel management will be added here later.",
                ))
            }
        }
    }

    fn open_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.settings_window
            && handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            return;
        }
        // A closed window leaves an invalid handle; replace it on the next request.
        self.settings_window = None;
        match settings::open(cx) {
            Ok(handle) => self.settings_window = Some(handle),
            Err(error) => eprintln!("Failed to open Settings: {error}"),
        }
    }
}

impl Render for Ganymede {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let reveal = if cx.reduce_motion() {
            self.sidebar_motion.to
        } else {
            self.sidebar_motion.value(now)
        };
        if !cx.reduce_motion()
            && now.duration_since(self.sidebar_motion.started) < SIDEBAR_DURATION
            && self.sidebar_motion.from != self.sidebar_motion.to
        {
            window.request_animation_frame();
        }
        let sidebar_width = 240. * reveal;
        let theme = cx.theme();
        let outer = theme.sidebar;
        let sidebar = theme.sidebar_content;
        let body = theme.app;
        div()
            .id("main-window")
            .on_action(cx.listener(|this, _: &NewTab, window, cx| {
                this.workspace
                    .update(cx, |workspace, cx| workspace.open_launcher(window, cx));
            }))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                this.workspace
                    .update(cx, |workspace, cx| workspace.close_active(window, cx));
            }))
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
            .relative()
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
                    .child(
                        div()
                            .id("toggle-sidebar")
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(theme.radius.md())
                            .cursor_pointer()
                            .hover(|s| s.bg(outer.accent).text_color(outer.accent_fg))
                            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation()
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.set_sidebar_open(!this.sidebar_open, cx);
                            }))
                            .child(
                                Icon::SidebarSimple
                                    .svg()
                                    .size(px(18.))
                                    .text_color(outer.muted_fg),
                            ),
                    )
                    .child(div().w(px((sidebar_width - 80.).max(0.))).flex_shrink_0())
                    .child(workspace::Workspace::header(&self.workspace, cx)),
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
                            .w(px(48.))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .items_center()
                            .px_2()
                            .py_1p5()
                            .gap_1()
                            .child(
                                self.rail_button(
                                    "Files",
                                    Icon::Folder,
                                    self.sidebar_open && self.sidebar_tab == SidebarTab::Files,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.sidebar_tab = SidebarTab::Files;
                                        this.set_sidebar_open(true, cx);
                                    },
                                )),
                            )
                            .child(
                                self.rail_button(
                                    "Kernels",
                                    Icon::Cpu,
                                    self.sidebar_open && self.sidebar_tab == SidebarTab::Kernels,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.sidebar_tab = SidebarTab::Kernels;
                                        this.set_sidebar_open(true, cx);
                                    },
                                )),
                            )
                            .child(div().flex_1())
                            .child(
                                self.rail_button("Settings", Icon::Gear, false, cx)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.open_settings(cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .rounded(theme.radius.xl())
                            .shadow(vec![theme.body_shadow.clone()])
                            .border_1()
                            .border_color(body.border)
                            .overflow_hidden()
                            .bg(outer.bg)
                            .text_color(outer.fg)
                            .when(sidebar_width > 0., |body| {
                                body.child(
                                    div()
                                        .w(px(sidebar_width))
                                        .flex_shrink_0()
                                        .h_full()
                                        .overflow_hidden()
                                        .bg(sidebar.bg)
                                        .text_color(sidebar.fg)
                                        .rounded_l(theme.radius.xl())
                                        .border_r_1()
                                        .border_color(sidebar.border)
                                        .child(
                                            div()
                                                .w(px(240.))
                                                .h_full()
                                                .opacity(reveal)
                                                .child(self.sidebar_content(cx)),
                                        ),
                                )
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .bg(body.bg)
                                    .text_color(body.fg)
                                    .rounded_r(theme.radius.xl())
                                    .when(sidebar_width == 0., |body| {
                                        body.rounded_l(theme.radius.xl())
                                    })
                                    .flex()
                                    .flex_col()
                                    .child(self.workspace.clone()),
                            ),
                    ),
            )
            .child(self.toasts.clone())
    }
}

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("create network runtime");
    let execution = execution::Runtime::new(runtime.handle().clone());
    gpui_platform::application()
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            cx.set_global(execution);
            assets::register_fonts(cx).expect("failed to register embedded fonts");
            code_cell::init(cx);
            cx.on_action(|_: &Quit, cx| cx.quit());
            let quit_key = if cfg!(target_os = "macos") {
                "cmd-q"
            } else {
                "ctrl-q"
            };
            cx.bind_keys([
                KeyBinding::new(
                    if cfg!(target_os = "macos") {
                        "cmd-s"
                    } else {
                        "ctrl-s"
                    },
                    Save,
                    None,
                ),
                KeyBinding::new(quit_key, Quit, None),
                KeyBinding::new(
                    if cfg!(target_os = "macos") {
                        "cmd-t"
                    } else {
                        "ctrl-t"
                    },
                    NewTab,
                    None,
                ),
                KeyBinding::new(
                    if cfg!(target_os = "macos") {
                        "cmd-w"
                    } else {
                        "ctrl-w"
                    },
                    CloseTab,
                    None,
                ),
            ]);
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

#[cfg(test)]
mod sidebar_tests {
    use super::*;
    #[test]
    fn reversing_sidebar_motion_preserves_current_position() {
        let start = Instant::now();
        let mut motion = SidebarMotion {
            from: 1.,
            to: 1.,
            started: start,
        };
        motion.set_open(false, start);
        let midway = start + SIDEBAR_DURATION / 2;
        let before = motion.value(midway);
        assert!(before > 0. && before < 1.);
        motion.set_open(true, midway);
        assert_eq!(motion.value(midway), before);
        assert_eq!(motion.value(midway + SIDEBAR_DURATION), 1.);
        motion.set_open(false, midway + SIDEBAR_DURATION);
        assert_eq!(motion.value(midway + SIDEBAR_DURATION * 2), 0.);
    }
}
