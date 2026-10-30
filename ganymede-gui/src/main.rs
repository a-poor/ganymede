mod assets;
mod code_cell;
pub mod icon;
mod notebook;
mod settings;
pub mod theme;
pub mod toast;

use gpui::{
    App, Bounds, Context, Entity, FontWeight, KeyBinding, Menu, MenuItem, TitlebarOptions, Window,
    WindowBounds, WindowControlArea, WindowOptions, actions, div, point, prelude::*, px, size,
};
use icon::Icon;
use theme::{ActiveTheme, Theme};

actions!(ganymede, [Quit]);

#[derive(Clone, Copy, PartialEq, Eq)]
enum SidebarTab {
    Files,
    Kernels,
}

struct Ganymede {
    notebook: Entity<notebook::Notebook>,
    toasts: Entity<toast::ToastHost>,
    sidebar_open: bool,
    sidebar_tab: SidebarTab,
    settings_window: Option<gpui::WindowHandle<settings::Settings>>,
    notebooks_expanded: bool,
    folder_focus: gpui::FocusHandle,
    file_focus: gpui::FocusHandle,
}

impl Ganymede {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        theme::AppearancePreference::default().apply(window.appearance(), cx);
        cx.observe_window_appearance(window, |_, window, cx| {
            theme::follow_system_appearance(window, cx);
        })
        .detach();
        cx.observe_global::<Theme>(|_, cx| cx.notify()).detach();
        let notebook = cx.new(|cx| notebook::Notebook::new(window, cx));
        let toasts = cx.new(|cx| toast::ToastHost::new(window, cx));
        Self {
            notebook,
            toasts,
            sidebar_open: true,
            sidebar_tab: SidebarTab::Files,
            settings_window: None,
            notebooks_expanded: true,
            folder_focus: cx.focus_handle(),
            file_focus: cx.focus_handle(),
        }
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
            // The tree starts at the server root; Notebooks is an example child folder,
            // not a mandatory container for server files.
            SidebarTab::Files => panel.child(
                div()
                    .id("file-tree")
                    .p(px(4.))
                    .gap(px(2.))
                    .text_size(theme.font.xs)
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .id("notebooks-folder")
                            .track_focus(&self.folder_focus)
                            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                                if this.folder_focus.is_focused(window) {
                                    window.blur();
                                    cx.notify();
                                }
                            }))
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .h(px(24.))
                            .px_1()
                            .rounded(theme.radius.md())
                            .cursor_pointer()
                            .tab_index(0)
                            .hover(|s| s.bg(surface.accent).text_color(surface.accent_fg))
                            .active(|s| s.bg(surface.accent).text_color(surface.accent_fg))
                            .focus(|s| s.border_1().border_color(surface.ring))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.notebooks_expanded = !this.notebooks_expanded;
                                cx.notify();
                            }))
                            .child(
                                if self.notebooks_expanded {
                                    Icon::FolderOpen
                                } else {
                                    Icon::Folder
                                }
                                .svg()
                                .size(px(14.))
                                .text_color(surface.fg),
                            )
                            .child("Notebooks"),
                    )
                    .when(self.notebooks_expanded, |tree| {
                        tree.child(
                            div()
                                .id("scratch-notebook")
                                .track_focus(&self.file_focus)
                                .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                                    if this.file_focus.is_focused(window) {
                                        window.blur();
                                        cx.notify();
                                    }
                                }))
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .h(px(24.))
                                .pl(px(16.))
                                .pr_2()
                                .rounded(theme.radius.md())
                                .cursor_pointer()
                                .tab_index(0)
                                .bg(theme.active_file_bg)
                                .text_color(theme.active_file_fg)
                                .hover(|s| {
                                    s.bg(theme.active_file_bg).text_color(theme.active_file_fg)
                                })
                                .active(|s| s.bg(surface.accent).text_color(surface.accent_fg))
                                .focus(|s| s.border_1().border_color(surface.ring))
                                .child(
                                    Icon::File
                                        .svg()
                                        .size(px(14.))
                                        .text_color(theme.active_file_fg),
                                )
                                .child("Untitled.ipynb"),
                        )
                    }),
            ),
            SidebarTab::Kernels => panel
                .child(div().pt_3().child("No connected kernels"))
                .child(
                    div()
                        .text_color(surface.muted_fg)
                        .child("Connect to a Jupyter server to see its kernels here."),
                ),
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let outer = theme.sidebar;
        let sidebar = theme.sidebar_content;
        let body = theme.app;
        div()
            .id("main-window")
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
                                this.sidebar_open = !this.sidebar_open;
                                cx.notify();
                            }))
                            .child(
                                Icon::SidebarSimple
                                    .svg()
                                    .size(px(18.))
                                    .text_color(outer.muted_fg),
                            ),
                    ),
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
                                        this.sidebar_open = true;
                                        cx.notify();
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
                                        this.sidebar_open = true;
                                        cx.notify();
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
                            .when(self.sidebar_open, |body| {
                                body.child(
                                    div()
                                        .w(px(240.))
                                        .flex_shrink_0()
                                        .h_full()
                                        .flex()
                                        .flex_col()
                                        .bg(sidebar.bg)
                                        .text_color(sidebar.fg)
                                        .rounded_l(theme.radius.xl())
                                        .border_r_1()
                                        .border_color(sidebar.border)
                                        .child(self.sidebar_content(cx)),
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
                                    .when(!self.sidebar_open, |body| {
                                        body.rounded_l(theme.radius.xl())
                                    })
                                    .flex()
                                    .flex_col()
                                    .child(self.notebook.clone()),
                            ),
                    ),
            )
            .child(self.toasts.clone())
    }
}

fn main() {
    gpui_platform::application()
        .with_assets(assets::Assets)
        .run(|cx: &mut App| {
            assets::register_fonts(cx).expect("failed to register embedded fonts");
            code_cell::init(cx);
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
