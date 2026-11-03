//! One connection picker / workspace per native window.
pub(crate) mod server;
use crate::{
    code_cell::stdin_prompt::{FieldSubmit, StdinPrompt as Field},
    execution::Runtime,
    theme::{ActiveTheme, Theme},
};
use ganymede_core::jupyter::ServerConfig;
use gpui::{App, Context, Entity, Focusable, Window, div, prelude::*, px};
use std::path::PathBuf;

#[derive(Clone)]
struct Recent {
    label: String,
    address: String,
    config: Option<ServerConfig>,
    local_info: Option<PathBuf>,
    local_id: Option<String>,
}
#[derive(Default)]
struct Connections {
    recent: Vec<Recent>,
    quitting: bool,
    managed: std::collections::HashMap<String, server::ManagedServer>,
}
impl gpui::Global for Connections {}

pub(crate) fn history_path() -> PathBuf {
    if let Some(path) = std::env::var_os("GANYMEDE_CONNECTIONS_FILE") {
        return path.into();
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    home.join(if cfg!(target_os = "macos") {
        "Library/Application Support/Ganymede/connections.json"
    } else {
        ".config/ganymede/connections.json"
    })
}
fn load_recent() -> Vec<Recent> {
    let data = std::fs::read(history_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    data.and_then(|data| data.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|item| {
            let (address, _) = server::parse_connection(item.get("url")?.as_str()?, "").ok()?;
            Some(Recent {
                label: item.get("label")?.as_str()?.into(),
                address,
                config: None,
                local_id: item
                    .get("local_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
                local_info: item
                    .get("local_info")
                    .and_then(|p| p.as_str())
                    .map(PathBuf::from),
            })
        })
        .take(10)
        .collect()
}
fn history_data(recent: &[Recent]) -> serde_json::Value {
    serde_json::Value::Array(
        recent
            .iter()
            .map(|r| serde_json::json!({"label":r.label,"url":r.address,"local_info":r.local_info,"local_id":r.local_id}))
            .collect(),
    )
}

fn remember(recent: Recent, cx: &mut App) {
    let connections = cx.global_mut::<Connections>();
    connections
        .recent
        .retain(|item| item.address != recent.address);
    connections.recent.insert(0, recent);
    connections.recent.truncate(10);
    // Never persist tokens. Local Jupyter servers maintain their own protected runtime file.
    let data = history_data(&connections.recent);
    let path = history_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(&data) {
        let _ = std::fs::write(path, bytes);
    }
    cx.refresh_windows();
}

pub fn init(cx: &mut App) {
    let mut recent = load_recent();
    if let Ok(config) = &cx.global::<Runtime>().config {
        let address = config.base_url().to_string();
        recent.retain(|r| r.address != address);
        recent.insert(
            0,
            Recent {
                label: "Configured server".into(),
                address,
                config: Some(config.clone()),
                local_info: None,
                local_id: None,
            },
        );
    }
    let ids = recent
        .iter()
        .filter_map(|r| r.local_id.clone())
        .collect::<Vec<_>>();
    let managed = server::load_servers(&ids);
    for server in managed.values() {
        let server = server.clone();
        let task = cx
            .global::<Runtime>()
            .handle
            .spawn(async move { server.reconcile().await });
        cx.spawn(async move |cx| {
            let _ = task.await;
            cx.update(|cx| cx.refresh_windows());
        })
        .detach();
    }
    cx.set_global(Connections {
        recent,
        quitting: false,
        managed,
    });
    cx.on_action(|_: &crate::NewWindow, cx| open(cx));
    cx.on_action(|_: &crate::Quit, cx| {
        if cx.global::<Connections>().quitting {
            return;
        }
        cx.global_mut::<Connections>().quitting = true;
        continue_quit(cx);
    });
    cx.on_window_closed(|cx, _| {
        if cx.global::<Connections>().quitting {
            cx.defer(continue_quit);
        }
    })
    .detach();
}
pub fn cancel_quit(cx: &mut App) {
    if let Some(connections) = cx.try_global::<Connections>()
        && connections.quitting
    {
        cx.global_mut::<Connections>().quitting = false;
    }
}
fn continue_quit(cx: &mut App) {
    if !cx.global::<Connections>().quitting {
        return;
    }
    let next = cx
        .windows()
        .into_iter()
        .find_map(|w| w.downcast::<ConnectionWindow>());
    if let Some(window) = next {
        let _ = window.update(cx, |view, window, cx| {
            window.activate_window();
            view.close(true, window, cx);
        });
    } else {
        cx.quit();
    }
}
pub fn reopen(cx: &mut App) {
    let windows: Vec<_> = cx
        .windows()
        .into_iter()
        .filter_map(|w| w.downcast::<ConnectionWindow>())
        .collect();
    let target = windows
        .iter()
        .find(|w| w.read(cx).is_ok_and(|v| v.workspace.is_some()))
        .or(windows.first());
    if let Some(window) = target {
        let _ = window.update(cx, |_, window, _| window.activate_window());
    } else {
        open(cx);
    }
    cx.activate(true);
}
pub fn open(cx: &mut App) {
    if cx.global::<Connections>().quitting {
        return;
    }
    let bounds = gpui::Bounds::centered(None, gpui::size(px(760.), px(560.)), cx);
    let result = cx.open_window(
        gpui::WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
            is_resizable: false,
            is_minimizable: false,
            window_min_size: Some(gpui::size(px(480.), px(320.))),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("Connect to Jupyter".into()),
                appears_transparent: true,
                traffic_light_position: Some(gpui::point(px(18.), px(18.))),
            }),
            ..Default::default()
        },
        |window, cx| cx.new(|cx| ConnectionWindow::new(window, cx)),
    );
    if result.is_err() {
        eprintln!("Could not open connection window");
    }
}

fn open_workspace(
    runtime: Runtime,
    label: &str,
    cx: &mut App,
) -> anyhow::Result<gpui::WindowHandle<ConnectionWindow>> {
    cx.open_window(
        gpui::WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(
                None,
                gpui::size(px(1000.), px(700.)),
                cx,
            ))),
            window_min_size: Some(gpui::size(px(480.), px(320.))),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some(format!("{label} — Ganymede").into()),
                appears_transparent: true,
                traffic_light_position: Some(gpui::point(px(12.), px(14.))),
            }),
            ..Default::default()
        },
        |window, cx| {
            cx.new(|cx| {
                let mut view = ConnectionWindow::new(window, cx);
                view.workspace =
                    Some(cx.new(|cx| crate::Ganymede::new(runtime, label, window, cx)));
                view
            })
        },
    )
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Recent,
    Existing,
    Local,
}
struct ConnectionWindow {
    workspace: Option<Entity<crate::Ganymede>>,
    page: Page,
    address: Entity<Field>,
    token: Entity<Field>,
    python: Entity<Field>,
    environment: server::Environment,
    environment_open: bool,
    environment_index: usize,
    environment_focus: gpui::FocusHandle,
    folder: Entity<Field>,
    busy: bool,
    error: Option<String>,
    focus: gpui::FocusHandle,
    task: gpui::Task<()>,
}
impl ConnectionWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe_global::<Theme>(|_, cx| cx.notify()).detach();
        cx.observe_window_appearance(window, |_, window, cx| {
            crate::theme::follow_system_appearance(window, cx)
        })
        .detach();
        let address =
            cx.new(|cx| Field::field("http://127.0.0.1:8888/".into(), false, "Server URL", cx));
        let token = cx.new(|cx| Field::field(String::new(), true, "Token", cx));
        let python = cx.new(|cx| Field::field("python3".into(), false, "Python executable", cx));
        let folder = cx.new(|cx| {
            Field::field(
                std::env::current_dir()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                false,
                "Project folder",
                cx,
            )
        });
        for input in [&address, &token, &python, &folder] {
            cx.subscribe_in(input, window, |this, _, _: &FieldSubmit, window, cx| {
                this.submit(window, cx)
            })
            .detach();
        }
        let weak = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            let _ = weak.update(cx, |this, cx| this.close(false, window, cx));
            false
        });
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            workspace: None,
            page: Page::Recent,
            address,
            token,
            python,
            environment: server::Environment::Global,
            environment_open: false,
            environment_index: 0,
            environment_focus: cx.focus_handle(),
            folder,
            busy: false,
            error: None,
            focus,
            task: gpui::Task::ready(()),
        }
    }
    fn close(&mut self, quit: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = &self.workspace {
            view.read(cx).workspace.clone().update(cx, |workspace, cx| {
                workspace.request_close(
                    if quit {
                        crate::workspace::CloseTarget::Quit
                    } else {
                        crate::workspace::CloseTarget::Window
                    },
                    window,
                    cx,
                )
            });
        } else {
            window.remove_window();
        }
    }
    fn page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        self.environment_open = false;
        self.error = None;
        match page {
            Page::Existing => self.address.read(cx).focus_handle(cx).focus(window, cx),
            Page::Local => self.folder.read(cx).focus_handle(cx).focus(window, cx),
            Page::Recent => self.focus.focus(window, cx),
        }
        cx.notify();
    }
    fn connect(
        &mut self,
        address: String,
        config: ServerConfig,
        label: String,
        info: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        let handle = cx.global::<Runtime>().handle.clone();
        let managed = managed_server(&address, cx);
        let task = handle.spawn(async move {
            if let Some(managed) = managed
                && managed.reconcile().await == server::LocalState::Stopped
            {
                managed.change(true).await?;
            }
            let kernel = server::validate(config.clone()).await?;
            Ok::<_, String>((address, config, label, kernel, info, None))
        });
        self.finish(task, window, cx);
        cx.notify();
    }
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.workspace.is_some() {
            return;
        }
        match self.page {
            Page::Existing => match server::parse_connection(
                self.address.read(cx).value(),
                self.token.read(cx).value(),
            ) {
                Ok((address, config)) => {
                    self.connect(address.clone(), config, address, None, window, cx)
                }
                Err(error) => {
                    self.error = Some(error);
                    cx.notify();
                }
            },
            Page::Local => {
                let python = self.python.read(cx).value().to_owned();
                let environment = self.environment;
                let folder = self.folder.read(cx).value().to_owned();
                self.busy = true;
                self.error = None;
                let handle = cx.global::<Runtime>().handle.clone();
                let storage = server::storage_directory();
                let task = handle.spawn(async move {
                    let label = PathBuf::from(&folder)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Local server".into());
                    let mut pending = tokio::task::spawn_blocking(move || {
                        server::start(environment, &python, &folder)
                    })
                    .await
                    .map_err(|_| "Could not start Jupyter.".to_owned())??;
                    let kernel = server::validate(pending.config.clone()).await?;
                    pending.persist(&storage).await?;
                    Ok((
                        pending.address.clone(),
                        pending.config.clone(),
                        label,
                        kernel,
                        Some(pending.info.clone()),
                        Some(pending),
                    ))
                });
                self.finish(task, window, cx);
                cx.notify();
            }
            Page::Recent => (),
        }
    }
    fn finish(
        &mut self,
        task: tokio::task::JoinHandle<Result<Connected, String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.task = cx.spawn_in(window, async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Connection interrupted.".into()));
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok((address, config, label, kernel, info, pending)) => {
                        let configured = cx.global::<Runtime>();
                        let kernel = if configured
                            .config
                            .as_ref()
                            .is_ok_and(|c| c.base_url().as_str() == address)
                        {
                            configured.kernel.clone()
                        } else {
                            kernel
                        };
                        let runtime = Runtime {
                            handle: cx.global::<Runtime>().handle.clone(),
                            config: Ok(config.clone()),
                            kernel,
                        };
                        // Native resize/minimize capabilities are fixed at window creation.
                        // Replace the fixed picker with a normal workspace window.
                        if open_workspace(runtime, &label, cx).is_err() {
                            this.error = Some("Could not open the workspace window.".into());
                            cx.notify();
                            return;
                        }
                        remember(
                            Recent {
                                label: label.clone(),
                                address: address.clone(),
                                config: Some(config),
                                local_info: managed_server(&address, cx)
                                    .map(|s| s.connection().1)
                                    .or(info),
                                local_id: pending
                                    .as_ref()
                                    .and_then(|p| p.id())
                                    .or_else(|| managed_server(&address, cx).and_then(|s| s.id())),
                            },
                            cx,
                        );
                        if let Some(pending) = pending {
                            cx.global_mut::<Connections>()
                                .managed
                                .insert(address, pending.accept());
                        }
                        this.token
                            .update(cx, |input, cx| input.set_value(String::new(), cx));
                        this.address
                            .update(cx, |input, cx| input.set_value(String::new(), cx));
                        window.remove_window();
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        });
    }
    fn recent(&mut self, mut recent: Recent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(server) = managed_server(&recent.address, cx) {
            let (config, info) = server.connection();
            recent.config = Some(config);
            recent.local_info = Some(info);
        }
        let config = recent.config.or_else(|| {
            recent
                .local_info
                .as_deref()
                .and_then(server::read_local)
                .map(|(_, c)| c)
        });
        if let Some(config) = config {
            self.connect(
                recent.address,
                config,
                recent.label,
                recent.local_info,
                window,
                cx,
            );
        } else {
            self.address
                .update(cx, |input, cx| input.set_value(recent.address, cx));
            self.token
                .update(cx, |input, cx| input.set_value(String::new(), cx));
            self.page(Page::Existing, window, cx);
            self.token.read(cx).focus_handle(cx).focus(window, cx);
        }
    }
    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(path) = paths.first()
            {
                let _ = this.update(cx, |this, cx| {
                    this.folder.update(cx, |input, cx| {
                        input.set_value(path.to_string_lossy().into_owned(), cx)
                    })
                });
            }
        })
        .detach();
    }
}
type Connected = (
    String,
    ServerConfig,
    String,
    String,
    Option<PathBuf>,
    Option<server::PendingServer>,
);

fn field(label: &'static str, input: Entity<Field>, cx: &App) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_color(cx.theme().app.muted_fg).child(label))
        .child(div().flex().child(input))
}
fn button(
    id: &'static str,
    label: &'static str,
    primary: bool,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    let t = cx.theme();
    div()
        .id(id)
        .tab_index(0)
        .px_2()
        .py_1()
        .text_size(t.font.xs)
        .rounded_full()
        .border_1()
        .border_color(t.app.border)
        .shadow(vec![t.body_shadow.clone()])
        .cursor_pointer()
        .bg(if primary {
            t.app.primary
        } else {
            t.launcher_sidebar.bg
        })
        .text_color(if primary {
            t.app.primary_fg
        } else {
            t.launcher_sidebar.fg
        })
        .hover(move |s| {
            if id == "connection-back" {
                s.bg(if t.is_dark { t.app.bg } else { t.card.bg })
                    .text_color(t.app.fg)
            } else {
                s.bg(if primary {
                    t.primary_hover
                } else {
                    t.secondary_hover
                })
            }
        })
        .focus(|s| s.border_color(t.app.ring))
        .when(id == "connection-back", |v| {
            v.border_0()
                .shadow(Vec::<gpui::BoxShadow>::new())
                .bg(if t.is_dark { t.app.bg } else { t.card.bg })
                .text_color(t.app.muted_fg)
                .px_0()
        })
        .child(label)
}
impl ConnectionWindow {
    fn page_button(
        &self,
        id: &'static str,
        label: &'static str,
        page: Page,
        primary: bool,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        button(id, label, primary, cx)
            .on_click(cx.listener(move |this, _, window, cx| this.page(page, window, cx)))
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        this.page(page, window, cx);
                        cx.stop_propagation();
                    }
                }),
            )
    }
    fn submit_button(&self, label: &'static str, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        button("submit-connection", label, true, cx)
            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    this.submit(window, cx);
                    cx.stop_propagation();
                }
            }))
    }
}

fn choice_row(
    title: &'static str,
    description: &'static str,
    action: impl IntoElement,
    cx: &App,
) -> gpui::Div {
    let t = cx.theme();
    div()
        .flex()
        .items_center()
        .gap_4()
        .p_3()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().font_weight(gpui::FontWeight::MEDIUM).child(title))
                .child(
                    div()
                        .text_size(t.font.xs)
                        .text_color(t.card.muted_fg)
                        .child(description),
                ),
        )
        .child(div().flex_shrink_0().child(action))
}

impl ConnectionWindow {
    fn choose_environment(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.environment = server::Environment::ALL[index];
        self.environment_index = index;
        self.environment_open = false;
        self.error = None;
        let value = if self.environment == server::Environment::Venv {
            ".venv"
        } else {
            "python3"
        };
        self.python
            .update(cx, |input, cx| input.set_value(value.into(), cx));
        self.environment_focus.focus(window, cx);
        cx.notify();
    }
    fn environment_control(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        div()
            .relative()
            .child(
                div()
                    .id("environment-select")
                    .track_focus(&self.environment_focus)
                    .tab_index(0)
                    .h(px(28.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(t.radius.lg())
                    .bg(t.launcher_sidebar.bg)
                    .text_color(t.launcher_sidebar.fg)
                    .border_1()
                    .border_color(t.app.border)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.app.accent).text_color(t.app.accent_fg))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.environment_open = !this.environment_open;
                        this.environment_index = server::Environment::ALL
                            .iter()
                            .position(|m| *m == this.environment)
                            .unwrap();
                        this.environment_focus.focus(window, cx);
                        cx.notify();
                    }))
                    .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, window, cx| {
                        match e.keystroke.key.as_str() {
                            "enter" | "space" if this.environment_open => {
                                this.choose_environment(this.environment_index, window, cx)
                            }
                            "enter" | "space" | "down" | "up" if !this.environment_open => {
                                this.environment_open = true
                            }
                            "down" => this.environment_index = (this.environment_index + 1) % 3,
                            "up" => this.environment_index = (this.environment_index + 2) % 3,
                            "escape" => this.environment_open = false,
                            "tab" => {
                                this.environment_open = false;
                                cx.notify();
                                return;
                            }
                            _ => return,
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .child(self.environment.label())
                    .child(div().flex_1())
                    .child(
                        crate::icon::Icon::CaretDown
                            .svg()
                            .size(px(12.))
                            .text_color(t.app.muted_fg),
                    ),
            )
            .when(self.environment_open, |v| {
                v.child(gpui::deferred(
                    gpui::anchored()
                        .offset(gpui::point(px(0.), px(4.)))
                        .snap_to_window_with_margin(px(8.))
                        .child(
                            div()
                                .id("environment-menu")
                                .w(px(220.))
                                .p_1()
                                .rounded(t.radius.lg())
                                .bg(t.popover.bg)
                                .text_color(t.popover.fg)
                                .border_1()
                                .border_color(t.popover.border)
                                .shadow(vec![t.toast_shadow.clone()])
                                .occlude()
                                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                    this.environment_open = false;
                                    cx.notify();
                                }))
                                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation()
                                })
                                .children(server::Environment::ALL.into_iter().enumerate().map(
                                    |(i, mode)| {
                                        div()
                                            .id(("environment-option", i))
                                            .px_2()
                                            .py_1()
                                            .rounded(t.radius.md())
                                            .cursor_pointer()
                                            .when(self.environment_index == i, |v| {
                                                v.bg(t.active_file_bg).text_color(t.active_file_fg)
                                            })
                                            .hover(|s| {
                                                s.bg(t.active_file_bg).text_color(t.active_file_fg)
                                            })
                                            .child(mode.label())
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.choose_environment(i, window, cx)
                                            }))
                                    },
                                )),
                        ),
                ))
            })
    }
}
impl Render for ConnectionWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(workspace) = &self.workspace {
            return div()
                .size_full()
                .track_focus(&self.focus)
                .child(workspace.clone())
                .into_any_element();
        }
        let t = cx.theme();
        let surface = t.launcher_sidebar;
        let background = if t.is_dark { t.app } else { t.card };
        let recent = cx.global::<Connections>().recent.clone();
        let sidebar = div()
            .w(px(220.))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(surface.bg)
            .text_color(surface.fg)
            .rounded(t.radius.xl2())
            .border_1()
            .border_color(t.launcher_border)
            .shadow(vec![t.launcher_shadow.clone()])
            .child(
                div()
                    .h(px(48.))
                    .flex_shrink_0()
                    .window_control_area(gpui::WindowControlArea::Drag),
            )
            .child(
                div()
                    .px_4()
                    .pt_2()
                    .pb_2()
                    .text_size(t.font.xs)
                    .text_color(surface.muted_fg)
                    .child("Recent connections"),
            )
            .child(
                div()
                    .id("recent-connections")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .when(recent.is_empty(), |v| {
                        v.child(
                            div()
                                .px_2()
                                .py_2()
                                .text_color(surface.muted_fg)
                                .child("Your connections will appear here."),
                        )
                    })
                    .children(recent.into_iter().enumerate().map(|(i, recent)| {
                        let click = recent.clone();
                        div()
                            .id(("recent", i))
                            .tab_index(0)
                            .px_2()
                            .py_1p5()
                            .mb_1()
                            .rounded(t.radius.xl2())
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_2()
                            .border_1()
                            .border_color(surface.bg)
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .size(px(24.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(t.radius.md())
                                    .bg(t.secondary)
                                    .text_color(t.secondary_fg)
                                    .child(
                                        crate::icon::Icon::Server
                                            .svg()
                                            .size(px(16.))
                                            .text_color(t.secondary_fg),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .text_size(t.font.sm)
                                            .truncate()
                                            .child(recent.label.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(t.font.xs)
                                            .text_color(surface.muted_fg)
                                            .truncate()
                                            .child(
                                                match managed_server(&recent.address, cx)
                                                    .map(|s| s.state())
                                                {
                                                    Some(server::LocalState::Stopped) => {
                                                        "Stopped · Click to start".to_owned()
                                                    }
                                                    Some(server::LocalState::Busy) => {
                                                        "Starting…".to_owned()
                                                    }
                                                    Some(server::LocalState::Unverified) => {
                                                        "Local server · Not verified".to_owned()
                                                    }
                                                    _ => recent.address.clone(),
                                                },
                                            ),
                                    ),
                            )
                            .when(!self.busy, |v| {
                                v.cursor_pointer()
                                    .hover(|s| {
                                        s.bg(t.launcher_recent_hover_bg)
                                            .text_color(surface.fg)
                                            .border_color(t.launcher_border)
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.recent(click.clone(), window, cx)
                                    }))
                                    .on_key_down(cx.listener(
                                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                                            if matches!(
                                                event.keystroke.key.as_str(),
                                                "enter" | "space"
                                            ) {
                                                this.recent(recent.clone(), window, cx);
                                                cx.stop_propagation();
                                            }
                                        },
                                    ))
                            })
                    })),
            );
        let group = || {
            div()
                .w_full()
                .bg(t.secondary)
                .text_color(t.secondary_fg)
                .border_1()
                .border_color(t.launcher_border)
                .rounded(t.radius.xl2())
                .overflow_hidden()
        };
        let divider = || div().mx_3().h(px(1.)).bg(t.launcher_border);
        let mut body = div().w_full().flex().flex_col().gap_3();
        if self.busy {
            body = body
                .items_center()
                .child(
                    div()
                        .text_color(t.app.muted_fg)
                        .child(if self.page == Page::Local {
                            "Starting Jupyter…"
                        } else {
                            "Connecting…"
                        }),
                )
                .child(
                    button("cancel-connect", "Cancel", false, cx)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.task = gpui::Task::ready(());
                            this.busy = false;
                            cx.notify();
                        }))
                        .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                this.task = gpui::Task::ready(());
                                this.busy = false;
                                cx.notify();
                                cx.stop_propagation();
                            }
                        })),
                );
        } else if self.page == Page::Recent {
            body = body.child(
                group()
                    .child(choice_row(
                        "Start a local server",
                        "Work in a folder using Python or a virtual environment.",
                        self.page_button("new-local", "Start…", Page::Local, true, cx),
                        cx,
                    ))
                    .child(divider())
                    .child(choice_row(
                        "Connect to a server",
                        "Open an existing Jupyter server using its URL and token.",
                        self.page_button("new-existing", "Connect…", Page::Existing, false, cx),
                        cx,
                    )),
            );
        } else {
            body = body
                .child(
                    div().flex().child(
                        self.page_button("connection-back", "Back", Page::Recent, false, cx)
                            .flex()
                            .flex_row_reverse()
                            .items_center()
                            .gap_1()
                            .child(
                                crate::icon::Icon::CaretLeft
                                    .svg()
                                    .size(px(12.))
                                    .text_color(t.secondary_fg),
                            ),
                    ),
                )
                .child(
                    div()
                        .text_size(t.font.base)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(if self.page == Page::Local {
                            "Start a local server"
                        } else {
                            "Connect to a server"
                        }),
                );
            if self.page == Page::Local {
                body = body.child(group().p_3().flex().flex_col().gap_3()
                    .child(div().flex().items_center().gap_2()
                        .child(div().flex_1().min_w_0().child(field("Project folder", self.folder.clone(), cx)))
                        .child(button("choose-folder", "Browse…", false, cx).mt(px(18.))
                            .on_click(cx.listener(|this, _, window, cx| this.choose_folder(window, cx)))
                            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                                if matches!(event.keystroke.key.as_str(), "enter" | "space") { this.choose_folder(window, cx); cx.stop_propagation(); }
                            }))))
                    .child(div().h(px(1.)).bg(t.launcher_border))
                    .child(div().text_size(t.font.xs).text_color(t.card.muted_fg).child("Environment"))
                    .child(self.environment_control(cx))
                    .when(matches!(self.environment, server::Environment::Venv | server::Environment::Custom), |v| v.child(field(
                        if self.environment == server::Environment::Venv { "Virtual environment folder" } else { "Python executable" }, self.python.clone(), cx)))
                    .child(div().text_size(t.font.xs).text_color(t.card.muted_fg).child(match self.environment {
                        server::Environment::Global => "Uses python3 from PATH. Requires jupyter-server and ipykernel.",
                        server::Environment::Venv => "Relative paths start at the project folder. Requires jupyter-server and ipykernel.",
                        server::Environment::Custom => "Choose a Python executable with jupyter-server and ipykernel installed.",
                    })))
                    .child(div().flex().justify_end().child(self.submit_button("Start server", cx)));
            } else {
                body = body
                    .child(
                        group()
                            .p_3()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(field("Server URL", self.address.clone(), cx))
                            .child(field("Token (optional)", self.token.clone(), cx))
                            .child(
                                div()
                                    .text_size(t.font.xs)
                                    .text_color(t.card.muted_fg)
                                    .child("Tokens are not saved to connection history."),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .child(self.submit_button("Connect", cx)),
                    );
            }
        }
        if let Some(error) = &self.error {
            body = body.child(div().text_color(t.destructive).child(error.clone()));
        }
        div()
            .size_full()
            .p_2()
            .track_focus(&self.focus)
            .flex()
            .bg(background.bg)
            .text_color(background.fg)
            .font_family(t.font.sans.clone())
            .text_size(t.font.sm)
            .on_action(
                cx.listener(|this, _: &crate::CloseTab, window, cx| this.close(false, window, cx)),
            )
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "tab" {
                    if event.keystroke.modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                    cx.stop_propagation();
                } else if event.keystroke.key == "escape" && !this.busy {
                    this.page(Page::Recent, window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(sidebar)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(28.))
                            .flex_shrink_0()
                            .window_control_area(gpui::WindowControlArea::Drag),
                    )
                    .child(
                        div()
                            .id("connection-content")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .px_6()
                            .pb_6()
                            .child(
                                div()
                                    .w_full()
                                    .max_w(px(480.))
                                    .mx_auto()
                                    .flex()
                                    .flex_col()
                                    .gap_3()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .items_center()
                                            .gap_2()
                                            .py_1()
                                            .child(
                                                div()
                                                    .text_size(t.font.xl)
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .child("Ganymede"),
                                            ),
                                    )
                                    .child(body),
                            ),
                    ),
            )
            .into_any_element()
    }
}

pub(crate) fn managed_server(address: &str, cx: &App) -> Option<server::ManagedServer> {
    cx.global::<Connections>().managed.get(address).cloned()
}

pub(crate) fn update_local_history(address: &str, cx: &mut App) {
    let Some(server) = managed_server(address, cx) else {
        return;
    };
    let (config, info) = server.connection();
    let recent = cx
        .global::<Connections>()
        .recent
        .iter()
        .find(|r| r.address == address)
        .cloned();
    if let Some(mut recent) = recent {
        recent.config = Some(config);
        recent.local_info = Some(info);
        remember(recent, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_history_never_persists_authentication() {
        let (address, config) =
            server::parse_connection("https://example.test/lab?token=private-token", "").unwrap();
        let data = history_data(&[Recent {
            label: "Server".into(),
            address,
            config: Some(config),
            local_info: None,
            local_id: None,
        }]);
        assert_eq!(data[0]["url"], "https://example.test/");
        assert!(!data.to_string().contains("private-token"));
        assert!(data[0].get("config").is_none());
    }

    #[gpui::test]
    fn quitting_checks_each_workspace_and_cancel_keeps_remaining_windows(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        cx.update(|cx| {
            cx.set_global(
                crate::theme::ThemeSpec::light().resolve(&crate::theme::Derivation::default()),
            );
            cx.set_global(crate::theme::AppearancePreference::default());
            cx.set_global(crate::autosave::AutosavePreference(false));
            cx.set_global(Runtime {
                handle: runtime.handle().clone(),
                config: Err("No test network".into()),
                kernel: "python3".into(),
            });
            init(cx);
        });
        cx.update(|cx| {
            open(cx);
            let picker = cx
                .windows()
                .into_iter()
                .find_map(|w| w.downcast::<ConnectionWindow>())
                .unwrap();
            picker
                .update(cx, |_, window, _| {
                    assert!(!window.is_resizable());
                    assert!(!window.is_minimizable());
                    window.remove_window();
                })
                .unwrap();
        });
        let mut windows = Vec::new();
        for _ in 0..2 {
            let window =
                cx.update(|cx| open_workspace(cx.global::<Runtime>().clone(), "Test", cx).unwrap());
            cx.update(|cx| {
                window
                    .update(cx, |view, window, cx| {
                        assert!(window.is_resizable());
                        assert!(window.is_minimizable());
                        view.workspace
                            .as_ref()
                            .unwrap()
                            .read(cx)
                            .workspace
                            .clone()
                            .update(cx, |ws, cx| ws.add_dirty_test_notebook(window, cx));
                    })
                    .unwrap()
            });
            windows.push(window);
        }
        cx.update(|cx| {
            cx.global_mut::<Connections>().quitting = true;
            continue_quit(cx);
        });
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();
        assert!(cx.has_pending_prompt());
        assert_eq!(
            cx.read(|cx| windows.iter().filter(|w| w.read(cx).is_ok()).count()),
            1
        );
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        cx.read(|cx| {
            assert!(!cx.global::<Connections>().quitting);
            assert_eq!(windows.iter().filter(|w| w.read(cx).is_ok()).count(), 1);
        });
    }
}
