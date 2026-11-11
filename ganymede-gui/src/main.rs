mod about;
mod assets;
mod autosave;
mod code_cell;
mod confirm;
mod connection;
mod execution;
mod features;
mod find;
pub mod icon;
mod image_loader;
mod menus;
mod notebook;
mod preferences;
mod rich;
mod settings;
mod storage;
mod syntax;
mod text_file;
pub mod theme;
pub mod toast;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod tray;
mod updates;
mod workspace;

use gpui::{
    App, Context, Entity, FontWeight, KeyBinding, Window, WindowControlArea, actions, div,
    prelude::*, px,
};
use icon::Icon;
use std::time::{Duration, Instant};
use theme::{ActiveTheme, Theme};

actions!(ganymede, [Quit, NewWindow, NewTab, CloseTab, Save]);

#[derive(Clone, Copy, PartialEq, Eq)]
enum SidebarTab {
    Files,
    Kernels,
    Git,
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

#[derive(Default)]
struct SettingsWindow(Option<gpui::WindowHandle<settings::Settings>>);
impl gpui::Global for SettingsWindow {}

struct Ganymede {
    app_menu: Entity<menus::in_window::WindowMenu>,
    focus: gpui::FocusHandle,
    workspace: Entity<workspace::Workspace>,
    toasts: Entity<toast::ToastHost>,
    sidebar_open: bool,
    sidebar_motion: SidebarMotion,
    sidebar_tab: SidebarTab,
}

impl Ganymede {
    fn new(
        runtime: execution::Runtime,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe_window_appearance(window, |_, window, cx| {
            theme::follow_system_appearance(window, cx);
        })
        .detach();
        cx.observe_global::<Theme>(|_, cx| cx.notify()).detach();
        let toasts = cx.new(|cx| toast::ToastHost::new(window, cx));
        cx.observe(&toasts, |_, _, cx| cx.notify()).detach();
        let workspace = cx.new(|cx| workspace::Workspace::new(runtime, toasts.clone(), window, cx));
        workspace.update(cx, |workspace, _| workspace.set_server_label(label));
        cx.observe(&workspace, |_, _, cx| cx.notify()).detach();
        let (sidebar_open, sidebar_tab) = workspace.read(cx).sidebar_state();
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            app_menu: cx
                .new(|cx| menus::in_window::WindowMenu::new(menus::in_window::Kind::Workspace, cx)),
            focus,
            workspace,
            toasts,
            sidebar_open,
            sidebar_motion: SidebarMotion {
                from: if sidebar_open { 1. } else { 0. },
                to: if sidebar_open { 1. } else { 0. },
                started: Instant::now(),
            },
            sidebar_tab,
        }
    }

    fn set_sidebar_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.sidebar_open = open;
        self.sidebar_motion.set_open(open, Instant::now());
        self.workspace.update(cx, |workspace, _| {
            workspace.set_sidebar_state(open, self.sidebar_tab)
        });
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
            .when(self.sidebar_tab != SidebarTab::Files, |panel| {
                panel.child(div().font_weight(FontWeight::SEMIBOLD).child(
                    if self.sidebar_tab == SidebarTab::Git {
                        "Git"
                    } else {
                        "Kernels"
                    },
                ))
            });
        match self.sidebar_tab {
            SidebarTab::Files => panel.child(workspace::Workspace::files(&self.workspace, cx)),
            SidebarTab::Git => panel.child(
                div()
                    .text_size(theme.font.xs)
                    .text_color(surface.muted_fg)
                    .child("Git integration is coming later."),
            ),
            SidebarTab::Kernels => panel.child(workspace::Workspace::kernels(&self.workspace, cx)),
        }
    }
}

fn open_settings(cx: &mut App) {
    if let Some(handle) = cx.global::<SettingsWindow>().0
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    // A closed window leaves an invalid handle; replace it on the next request.
    cx.global_mut::<SettingsWindow>().0 = None;
    match settings::open(cx) {
        Ok(handle) => cx.global_mut::<SettingsWindow>().0 = Some(handle),
        Err(error) => eprintln!("Failed to open Settings: {error}"),
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
        let header = workspace::Workspace::header(&self.workspace, window, cx);
        let theme = cx.theme();
        let outer = theme.sidebar;
        let sidebar = theme.sidebar_content;
        let body = theme.app;
        menus::handlers(div().id("main-window"), self, cx)
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape"
                    && this
                        .workspace
                        .update(cx, |workspace, cx| workspace.cancel_tab_drag(window, cx))
                {
                    cx.stop_propagation();
                }
            }))
            .on_action(
                cx.listener(|this, _: &menus::in_window::OpenMenu, window, cx| {
                    this.app_menu
                        .update(cx, |menu, cx| menu.open_first(window, cx));
                }),
            )
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
                    .when(
                        cfg!(target_os = "linux")
                            && !window.is_fullscreen()
                            && matches!(
                                window.window_decorations(),
                                gpui::Decorations::Client { .. }
                            ),
                        |bar| bar.pr(px(128.)),
                    )
                    .text_size(theme.font.sm)
                    .text_color(outer.muted_fg)
                    .window_control_area(WindowControlArea::Drag)
                    .when(cfg!(any(target_os = "linux", target_os = "macos")), |bar| {
                        // The workspace owns titlebar dragging on macOS as well.
                        // Tabs and controls stop propagation before this handler.
                        bar.on_mouse_down(gpui::MouseButton::Left, |event, window, _| {
                            if event.click_count == 2 {
                                if cfg!(target_os = "macos") {
                                    window.titlebar_double_click();
                                } else {
                                    window.zoom_window();
                                }
                            } else {
                                window.start_window_move();
                            }
                        })
                    })
                    .on_click(|event, window, _| {
                        if !cfg!(any(target_os = "linux", target_os = "macos"))
                            && event.click_count() == 2
                        {
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
                    .child(
                        div()
                            .w(px((sidebar_width - 80.).max(32.)))
                            .flex_shrink_0()
                            .child(workspace::Workspace::server_button(&self.workspace, cx)),
                    )
                    .child(header),
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
                                        this.workspace.update(cx, |workspace, cx| {
                                            workspace.refresh_kernels(false, cx)
                                        });
                                        this.set_sidebar_open(true, cx);
                                    },
                                )),
                            )
                            .when(features::UNFINISHED, |rail| {
                                rail.child(
                                    self.rail_button(
                                        "Git",
                                        Icon::GitBranch,
                                        self.sidebar_open && self.sidebar_tab == SidebarTab::Git,
                                        cx,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.sidebar_tab = SidebarTab::Git;
                                            this.set_sidebar_open(true, cx);
                                        },
                                    )),
                                )
                            })
                            .child(div().flex_1())
                            .child(
                                self.rail_button("Settings", Icon::Gear, false, cx)
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        open_settings(cx);
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
                                    .when(cfg!(target_os = "linux"), |body| {
                                        body.child(self.app_menu.clone())
                                    })
                                    .child(div().flex_1().min_h_0().child(self.workspace.clone())),
                            ),
                    ),
            )
            .child(self.toasts.clone())
    }
}

fn main() {
    if let Err(error) = storage::Paths::current().migrate() {
        eprintln!("Could not migrate application storage; original files were retained: {error}");
        std::process::exit(1);
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("create network runtime");
    let execution = execution::Runtime::new(runtime.handle().clone());
    let app = gpui_platform::application().with_assets(assets::Assets);
    app.on_reopen(connection::reopen);
    app.run(move |cx: &mut App| {
        preferences::init(cx);
        cx.set_global(execution);
        cx.set_global(SettingsWindow::default());
        confirm::init(cx);
        assets::register_fonts(cx).expect("failed to register embedded fonts");
        code_cell::init(cx);
        notebook::init(cx);
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
                    "cmd-shift-n"
                } else {
                    "ctrl-shift-n"
                },
                NewWindow,
                None,
            ),
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
        menus::init(cx);
        connection::init(cx);
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        tray::init(cx);
        connection::open(cx);
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
