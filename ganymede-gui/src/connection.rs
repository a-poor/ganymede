//! One connection picker / workspace per native window.
mod credentials;
mod discovery;
#[cfg(target_os = "linux")]
pub(crate) mod linux_frame;
mod planet;
pub(crate) mod server;
pub(crate) mod ssh;
use crate::{
    code_cell::stdin_prompt::{FieldSubmit, StdinPrompt as Field},
    execution::Runtime,
    theme::{ActiveTheme, Theme},
};
use ganymede_core::jupyter::ServerConfig;
use gpui::{App, Context, Entity, Focusable, Window, div, prelude::*, px};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Clone)]
struct Recent {
    label: String,
    address: String,
    config: Option<ServerConfig>,
    local_info: Option<PathBuf>,
    local_id: Option<String>,
    saved_token: bool,
    ssh_host: Option<String>,
}
impl Recent {
    fn key(&self) -> String {
        ssh::identity(&self.address, self.ssh_host.as_deref())
    }
}
#[derive(Default)]
struct Connections {
    recent: Vec<Recent>,
    quitting: bool,
    managed: std::collections::HashMap<String, server::ManagedServer>,
}
impl gpui::Global for Connections {}

pub(crate) fn history_path() -> PathBuf {
    crate::storage::Paths::current().connections
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
                ssh_host: match item.get("ssh_host").and_then(|v| v.as_str()) {
                    Some(value) => ssh::host(value).ok()?,
                    None => None,
                },
                saved_token: item
                    .get("saved_token")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
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
            .map(|r| serde_json::json!({"label":r.label,"url":r.address,"local_info":r.local_info,"local_id":r.local_id,"saved_token":r.saved_token,"ssh_host":r.ssh_host}))
            .collect(),
    )
}

fn remember(recent: Recent, cx: &mut App) {
    let connections = cx.global_mut::<Connections>();
    connections.recent.retain(|item| item.key() != recent.key());
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
        recent.retain(|r| r.address != address || r.ssh_host.is_some());
        recent.insert(
            0,
            Recent {
                label: address.clone(),
                address,
                config: Some(config.clone()),
                local_info: None,
                local_id: None,
                saved_token: false,
                ssh_host: None,
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
        // Menu/keyboard dispatch borrows the active window. Close it only after
        // that dispatch finishes so WindowHandle::update can borrow it again.
        cx.defer(continue_quit);
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
pub fn check_updates(cx: &mut App) {
    let mut target = cx
        .active_window()
        .and_then(|w| w.downcast::<ConnectionWindow>())
        .or_else(|| {
            cx.windows()
                .into_iter()
                .find_map(|w| w.downcast::<ConnectionWindow>())
        });
    if target.is_none() {
        open(cx);
        target = cx
            .windows()
            .into_iter()
            .find_map(|w| w.downcast::<ConnectionWindow>());
    }
    if let Some(target) = target {
        let _ = target.update(cx, |view, window, cx| {
            window.activate_window();
            let host = view
                .workspace
                .as_ref()
                .map(|w| w.read(cx).toasts.clone())
                .unwrap_or_else(|| view.toasts.clone());
            crate::updates::check(host, cx);
        });
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
            is_minimizable: true,
            window_min_size: Some(gpui::size(px(480.), px(320.))),
            #[cfg(target_os = "linux")]
            window_decorations: Some(gpui::WindowDecorations::Client),
            #[cfg(target_os = "linux")]
            window_background: gpui::WindowBackgroundAppearance::Transparent,
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("Ganymede".into()),
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
            #[cfg(target_os = "linux")]
            window_decorations: Some(gpui::WindowDecorations::Client),
            #[cfg(target_os = "linux")]
            window_background: gpui::WindowBackgroundAppearance::Transparent,
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
const PAGE_TRANSITION_DURATION: Duration = Duration::from_millis(240);
#[derive(Clone, Copy)]
struct PageTransition {
    from: Page,
    started: Instant,
}
struct ConnectionWindow {
    #[cfg(target_os = "linux")]
    app_menu: Entity<crate::menus::in_window::WindowMenu>,
    toasts: Entity<crate::toast::ToastHost>,
    workspace: Option<Entity<crate::Ganymede>>,
    planet: Option<Entity<planet::Planet>>,
    page: Page,
    transition: Option<PageTransition>,
    address: Entity<Field>,
    token: Entity<Field>,
    ssh_host: Entity<Field>,
    remember_token: bool,
    reading_token: bool,
    advanced: bool,
    recent_menu: Option<usize>,
    save_token: Option<(String, String)>,
    python: Entity<Field>,
    environment: server::Environment,
    detected: Vec<discovery::Environment>,
    discovering: bool,
    discovery_key: (String, String),
    discovery_task: gpui::Task<()>,
    environment_chosen: bool,
    detected_choice: Option<(PathBuf, String)>,
    installing: bool,
    creating_environment: bool,
    install_task: gpui::Task<()>,
    environment_open: bool,
    environment_index: usize,
    environment_keyboard: bool,
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
        let address = cx.new(|cx| {
            Field::field("http://127.0.0.1:8888/".into(), false, "Server URL", cx).monospace()
        });
        let ssh_host =
            cx.new(|cx| Field::field(String::new(), false, "SSH host (optional)", cx).monospace());
        let token = cx.new(|cx| Field::field(String::new(), true, "Token", cx));
        let python =
            cx.new(|cx| Field::field("python3".into(), false, "Python executable", cx).monospace());
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
            .monospace()
        });
        for input in [&address, &token, &ssh_host, &python, &folder] {
            cx.subscribe_in(input, window, |this, _, _: &FieldSubmit, window, cx| {
                this.submit(window, cx)
            })
            .detach();
        }
        cx.observe(&folder, |this, _, cx| this.refresh_environments(cx))
            .detach();
        cx.observe(&python, |this, _, cx| {
            if this.selected_python(cx).to_string_lossy() != this.discovery_key.1 {
                this.environment_chosen = true;
                this.detected_choice = None;
            }
            this.refresh_environments(cx);
        })
        .detach();
        let discovery_view = cx.entity().downgrade();
        cx.defer(move |cx| {
            let _ = discovery_view.update(cx, |this, cx| this.refresh_environments(cx));
        });
        let weak = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            let _ = weak.update(cx, |this, cx| this.close(false, window, cx));
            false
        });
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let toasts = cx.new(|cx| crate::toast::ToastHost::new(window, cx));
        cx.observe(&toasts, |_, _, cx| cx.notify()).detach();
        Self {
            #[cfg(target_os = "linux")]
            app_menu: cx.new(|cx| {
                crate::menus::in_window::WindowMenu::new(
                    crate::menus::in_window::Kind::Connection,
                    cx,
                )
            }),
            toasts,
            workspace: None,
            planet: None,
            page: Page::Recent,
            transition: None,
            address,
            token,
            ssh_host,
            remember_token: true,
            reading_token: false,
            advanced: false,
            recent_menu: None,
            save_token: None,
            python,
            environment: server::Environment::Global,
            detected: Vec::new(),
            discovering: false,
            discovery_key: (String::new(), String::new()),
            discovery_task: gpui::Task::ready(()),
            environment_chosen: false,
            detected_choice: None,
            installing: false,
            creating_environment: false,
            install_task: gpui::Task::ready(()),
            environment_open: false,
            environment_index: 0,
            environment_keyboard: false,
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
        if page == self.page || self.transition.is_some() || self.installing {
            return;
        }
        if !cx.reduce_motion() {
            self.transition = Some(PageTransition {
                from: self.page,
                started: Instant::now(),
            });
        }
        self.page = page;
        self.environment_open = false;
        self.error = None;
        if self.transition.is_some() {
            self.focus.focus(window, cx);
        } else {
            self.focus_page(window, cx);
        }
        cx.notify();
    }
    fn focus_page(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.page {
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
        let ssh_host = match ssh::host(self.ssh_host.read(cx).value()) {
            Ok(host) => host,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        self.busy = true;
        self.error = None;
        let handle = cx.global::<Runtime>().handle.clone();
        let managed = if ssh_host.is_none() {
            managed_server(&address, cx)
        } else {
            None
        };
        let task = handle.spawn(async move {
            if let Some(managed) = managed
                && managed.reconcile().await == server::LocalState::Stopped
            {
                managed.change(true).await?;
            }
            let mut config = config;
            let tunnel = if let Some(host) = ssh_host {
                let identity = ssh::identity(&address, Some(&host));
                let tunnel = ssh::Tunnel::open(host).await?;
                config = config.with_socks_proxy(tunnel.endpoint(), identity).map_err(|_| "Could not configure SSH forwarding.".to_owned())?;
                Some(tunnel)
            } else { None };
            let kernel = server::validate(config.clone()).await.map_err(|error| {
                if tunnel.is_some() { "SSH connected, but Jupyter is unreachable or rejected the token. Use the server URL as seen from the SSH host.".into() } else { error }
            })?;
            Ok::<_, String>((address, config, label, kernel, info, None, tunnel))
        });
        self.finish(task, window, cx);
        cx.notify();
    }
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.installing || self.workspace.is_some() {
            return;
        }
        match self.page {
            Page::Existing => match server::parse_connection(
                self.address.read(cx).value(),
                self.token.read(cx).value(),
            ) {
                Ok((address, config)) => {
                    let host = match ssh::host(self.ssh_host.read(cx).value()) {
                        Ok(host) => host,
                        Err(error) => {
                            self.error = Some(error);
                            cx.notify();
                            return;
                        }
                    };
                    let key = ssh::identity(&address, host.as_deref());
                    self.save_token = if self.remember_token {
                        credentials::token(
                            self.address.read(cx).value(),
                            self.token.read(cx).value(),
                        )
                        .map(|token| (key, token))
                    } else {
                        None
                    };
                    let label = host
                        .map(|host| format!("{address} · {host}"))
                        .unwrap_or_else(|| address.clone());
                    self.connect(address.clone(), config, label, None, window, cx)
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
                        None,
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
        let save_token = self.save_token.take();
        self.task = cx.spawn_in(window, async move |this, cx| {
            let mut result = task
                .await
                .unwrap_or_else(|_| Err("Connection interrupted.".into()));
            let mut saved = false;
            if result.is_ok() {
                match credentials::save(save_token, |key, token| {
                    let write =
                        cx.update(|_, cx| cx.write_credentials(&key, "token", token.as_bytes()));
                    async move { write?.await }
                })
                .await
                {
                    Ok(value) => saved = value,
                    Err(error) => result = Err(error),
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok((address, config, label, kernel, info, pending, tunnel)) => {
                        let ssh_host = tunnel.as_ref().map(|t| t.host());
                        let key = ssh::identity(&address, ssh_host.as_deref());
                        if let Some(tunnel) = &tunnel {
                            tunnel.register_quit(cx);
                        }
                        let configured = cx.global::<Runtime>();
                        let kernel = if ssh_host.is_none()
                            && configured
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
                            tunnel,
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
                                saved_token: saved
                                    || cx
                                        .global::<Connections>()
                                        .recent
                                        .iter()
                                        .any(|r| r.key() == key && r.saved_token),
                                ssh_host: ssh_host.clone(),
                                local_info: if ssh_host.is_none() {
                                    managed_server(&address, cx)
                                        .map(|s| s.connection().1)
                                        .or(info)
                                } else {
                                    None
                                },
                                local_id: pending.as_ref().and_then(|p| p.id()).or_else(|| {
                                    if ssh_host.is_none() {
                                        managed_server(&address, cx).and_then(|s| s.id())
                                    } else {
                                        None
                                    }
                                }),
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
        if self.busy {
            return;
        }
        self.save_token = None;
        self.ssh_host.update(cx, |input, cx| {
            input.set_value(recent.ssh_host.clone().unwrap_or_default(), cx)
        });
        if recent.saved_token
            && (recent.ssh_host.is_some() || managed_server(&recent.address, cx).is_none())
        {
            self.edit_recent(&recent, window, cx);
            self.busy = true;
            self.reading_token = true;
            let read = cx.read_credentials(&credentials::key(&recent.key()));
            self.task = cx.spawn_in(window, async move |this, cx| {
                let result = read.await;
                let _ = this.update_in(cx, |this, window, cx| {
                    this.busy = false;
                    this.reading_token = false;
                    if let Some(token) = this.restore_token(result, window, cx) {
                        match server::parse_connection(&recent.address, &token) {
                            Ok((address, config)) => {
                                this.connect(address, config, recent.label, None, window, cx)
                            }
                            Err(error) => this.error = Some(error),
                        }
                    }
                    cx.notify();
                });
            });
            cx.notify();
            return;
        }
        if recent.ssh_host.is_none()
            && let Some(server) = managed_server(&recent.address, cx)
        {
            let (config, info) = server.connection();
            recent.config = Some(config);
            recent.local_info = Some(info);
        }
        let config = recent.config.clone().or_else(|| {
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
            self.edit_recent(&recent, window, cx);
        }
    }
    fn edit_recent(&mut self, recent: &Recent, window: &mut Window, cx: &mut Context<Self>) {
        self.recent_menu = None;
        if recent.ssh_host.is_none()
            && let Some(server) = managed_server(&recent.address, cx)
        {
            let (mode, python, folder) = server.launch_options();
            self.environment_chosen = true;
            self.detected_choice = None;
            self.environment = mode;
            self.python
                .update(cx, |input, cx| input.set_value(python, cx));
            self.folder
                .update(cx, |input, cx| input.set_value(folder, cx));
            self.page(Page::Local, window, cx);
            return;
        }
        self.address
            .update(cx, |input, cx| input.set_value(recent.address.clone(), cx));
        self.token
            .update(cx, |input, cx| input.set_value(String::new(), cx));
        self.ssh_host.update(cx, |input, cx| {
            input.set_value(recent.ssh_host.clone().unwrap_or_default(), cx)
        });
        self.advanced = recent.ssh_host.is_some();
        self.remember_token = true;
        self.page(Page::Existing, window, cx);
        self.token.read(cx).focus_handle(cx).focus(window, cx);
    }
    fn remove_recent(&mut self, key: &str, cx: &mut Context<Self>) {
        self.recent_menu = None;
        let mut recent = cx.global::<Connections>().recent.clone();
        recent.retain(|r| r.key() != key);
        let result = serde_json::to_vec_pretty(&history_data(&recent))
            .map_err(|e| e.to_string())
            .and_then(|bytes| std::fs::write(history_path(), bytes).map_err(|e| e.to_string()));
        if result.is_ok() {
            cx.global_mut::<Connections>().recent = recent;
        } else {
            self.error = Some("Could not remove the recent connection. Try again.".into());
        }
        cx.refresh_windows();
    }
    fn edit_saved(&mut self, recent: &Recent, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_recent(recent, window, cx);
        if self.page != Page::Existing || !recent.saved_token {
            return;
        }
        self.busy = true;
        self.reading_token = true;
        let read = cx.read_credentials(&credentials::key(&recent.key()));
        self.task = cx.spawn_in(window, async move |this, cx| {
            let result = read.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                this.reading_token = false;
                this.restore_token(result, window, cx);
                this.token.read(cx).focus_handle(cx).focus(window, cx);
                cx.notify();
            });
        });
    }
    fn restore_token(
        &mut self,
        result: credentials::StoredToken,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        match credentials::restored(result) {
            Ok(token) => {
                self.token
                    .update(cx, |input, cx| input.set_value(token.clone(), cx));
                self.error = None;
                Some(token)
            }
            Err(error) => {
                // Manual entry must not immediately retry the failing store.
                // This does not claim to delete an existing saved credential.
                self.remember_token = false;
                self.error = Some(error);
                self.token.read(cx).focus_handle(cx).focus(window, cx);
                None
            }
        }
    }
    fn forget_token(&mut self, address: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        let delete = cx.delete_credentials(&credentials::key(&address));
        self.task = cx.spawn_in(window, async move |this, cx| {
            let result = delete.await;
            let _ = this.update_in(cx, |this, _, cx| {
                this.finish_forgetting(&address, result, cx);
            });
        });
        cx.notify();
    }
    fn finish_forgetting(
        &mut self,
        address: &str,
        result: anyhow::Result<()>,
        cx: &mut Context<Self>,
    ) {
        self.busy = false;
        if credentials::deleted(&result) {
            let recent = cx
                .global::<Connections>()
                .recent
                .iter()
                .find(|r| r.key() == address)
                .cloned();
            if let Some(mut recent) = recent {
                recent.saved_token = false;
                recent.config = None;
                remember(recent, cx);
            }
            self.remember_token = false;
            self.token
                .update(cx, |input, cx| input.set_value(String::new(), cx));
            self.error = None;
        } else {
            self.error = Some(credentials::forget_error());
        }
        cx.notify();
    }
    fn remember_control(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        div()
            .id("remember-token")
            .debug_selector(|| "remember-token".into())
            .aria_label("Remember token")
            .tab_index(0)
            .flex()
            .items_center()
            .gap_2()
            .text_size(t.font.sm)
            .cursor_pointer()
            .child(
                div()
                    .size(px(14.))
                    .border_1()
                    .border_color(t.input)
                    .rounded(t.radius.sm())
                    .bg(if self.remember_token {
                        t.card.primary
                    } else {
                        t.card.bg
                    })
                    .text_color(if self.remember_token {
                        t.card.primary_fg
                    } else {
                        t.card.fg
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(if self.remember_token { "✓" } else { "" }),
            )
            .child("Remember token")
            .focus_visible(|s| s.bg(t.card.accent).text_color(t.card.accent_fg))
            .on_click(cx.listener(|this, _, _, cx| {
                this.remember_token = !this.remember_token;
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "space" | "enter") {
                    this.remember_token = !this.remember_token;
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
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
    Option<ssh::Tunnel>,
);

fn field(label: &'static str, input: Entity<Field>, cx: &App) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_color(cx.theme().app.muted_fg).child(label))
        .child(div().flex().child(input))
}
pub(crate) fn button(
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
        .bg(if primary { t.app.primary } else { t.app.bg })
        .text_color(if primary { t.app.primary_fg } else { t.app.fg })
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
                .px_2()
                .flex_shrink_0()
                .whitespace_nowrap()
                .focus_visible(|s| s.bg(t.card.accent).text_color(t.card.accent_fg))
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
    fn cancel_install(&mut self, cx: &mut Context<Self>) {
        self.install_task = gpui::Task::ready(());
        self.installing = false;
        self.error = Some(if self.creating_environment {
            "Setup cancelled. The environment folder may be partially created; it has been kept."
        } else {
            "Installation cancelled. Some packages may already be installed."
        }.into());
        self.creating_environment = false;
        self.rescan_environments(cx);
    }
    fn missing_venv(&self, cx: &App) -> Option<PathBuf> {
        if self.environment != server::Environment::Venv {
            return None;
        }
        let python = self.selected_python(cx);
        let folder = python.parent()?.parent()?;
        match std::fs::symlink_metadata(folder) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Some(folder.to_path_buf())
            }
            _ => None,
        }
    }
    fn install_missing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.installing || self.busy || self.discovering || window.has_active_prompt() {
            return;
        }
        let python = self.selected_python(cx);
        let create_folder = self.missing_venv(cx);
        let found = self
            .detected
            .iter()
            .find(|e| e.path == python && !e.missing.is_empty());
        if found.is_none() && create_folder.is_none() {
            return;
        }
        let packages = found.map(|e| e.missing.clone()).unwrap_or_default();
        let detail = if let Some(folder) = &create_folder {
            format!(
                "Create a virtual environment using python3 from PATH at:\n{}\n\nThen download and install jupyter-server, ipykernel, and their dependencies. Existing folders will not be overwritten.",
                folder.display()
            )
        } else {
            format!(
                "Install {} and their dependencies into:\n{}\n\nThis downloads packages and modifies this Python environment.",
                packages.join(" and "),
                python.display()
            )
        };
        let prompt = window.prompt(
            gpui::PromptLevel::Info,
            if create_folder.is_some() {
                "Create environment?"
            } else {
                "Install missing packages?"
            },
            Some(&detail),
            &[
                "Cancel",
                if create_folder.is_some() {
                    "Create"
                } else {
                    "Install"
                },
            ],
            cx,
        );
        self.install_task = cx.spawn_in(window, async move |this, cx| {
            if prompt.await != Ok(1) {
                return;
            }
            let task = this
                .update(cx, |this, cx| {
                    if this.selected_python(cx) != python {
                        return None;
                    }
                    this.installing = true;
                    this.creating_environment = create_folder.is_some();
                    this.environment_chosen = true;
                    this.error = None;
                    this.environment_open = false;
                    this.discovery_task = gpui::Task::ready(());
                    cx.notify();
                    Some(cx.global::<Runtime>().handle.spawn(async move {
                        if let Some(folder) = create_folder {
                            discovery::create_environment(folder).await
                        } else {
                            discovery::install(python, packages).await
                        }
                    }))
                })
                .ok()
                .flatten();
            let Some(task) = task else {
                return;
            };
            struct Abort(tokio::task::AbortHandle);
            impl Drop for Abort {
                fn drop(&mut self) {
                    self.0.abort();
                }
            }
            let _abort = Abort(task.abort_handle());
            let result = task
                .await
                .unwrap_or_else(|_| Err("Installation interrupted.".into()));
            let _ = this.update(cx, |this, cx| {
                this.installing = false;
                this.creating_environment = false;
                this.error = result.err();
                this.rescan_environments(cx);
            });
        });
    }
    fn rescan_environments(&mut self, cx: &mut Context<Self>) {
        self.discovery_key = (String::new(), String::new());
        self.refresh_environments(cx);
    }
    fn selected_python(&self, cx: &App) -> PathBuf {
        server::launch_command(
            self.environment,
            self.python.read(cx).value(),
            &PathBuf::from(self.folder.read(cx).value().trim()),
        )
        .map(|(path, _)| PathBuf::from(path))
        .unwrap_or_default()
    }
    fn refresh_environments(&mut self, cx: &mut Context<Self>) {
        let key = (
            self.folder.read(cx).value().trim().to_string(),
            self.selected_python(cx).to_string_lossy().into_owned(),
        );
        if key == self.discovery_key {
            return;
        }
        self.discovery_key = key.clone();
        self.detected.clear();
        self.discovering = true;
        let Some(runtime) = cx.try_global::<Runtime>() else {
            return;
        };
        let handle = runtime.handle.clone();
        self.discovery_task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let folder = PathBuf::from(&key.0);
            let selected = PathBuf::from(&key.1);
            let task = handle.spawn(discovery::discover(folder, selected));
            // Abort Tokio work as well when a field edit replaces this GPUI task.
            struct Abort(tokio::task::AbortHandle);
            impl Drop for Abort {
                fn drop(&mut self) {
                    self.0.abort();
                }
            }
            let _abort = Abort(task.abort_handle());
            let results = task.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| this.finish_discovery(key, results, cx));
        });
        cx.notify();
    }
    fn finish_discovery(
        &mut self,
        key: (String, String),
        results: Vec<discovery::Environment>,
        cx: &mut Context<Self>,
    ) {
        if self.discovery_key != key
            || self.folder.read(cx).value().trim() != key.0
            || self.selected_python(cx).to_string_lossy() != key.1
        {
            return;
        }
        self.discovering = false;
        if !self.environment_chosen
            && !self.busy
            && let Some(preferred) = results.iter().find(|e| e.ready)
        {
            self.environment = server::Environment::Custom;
            self.detected_choice = Some((preferred.path.clone(), preferred.label.clone()));
            let value = preferred.path.to_string_lossy().into_owned();
            self.discovery_key.1 = value.clone();
            self.python
                .update(cx, |field, cx| field.set_value(value, cx));
        }
        self.detected = results;
        cx.notify();
    }
    fn detected_options(&self) -> impl Iterator<Item = &discovery::Environment> {
        self.detected.iter().filter(|e| {
            (e.ready || !e.missing.is_empty()) && e.path != std::path::Path::new("python3")
        })
    }
    fn environment_options(&self) -> usize {
        3 + self.detected_options().count()
    }
    fn selected_environment_index(&self, _cx: &App) -> usize {
        if let Some((path, _)) = &self.detected_choice
            && let Some(i) = self.detected_options().position(|e| &e.path == path)
        {
            return i + 1;
        }
        if self
            .detected_choice
            .as_ref()
            .is_some_and(|(path, _)| path == &PathBuf::from("python3"))
        {
            return 0;
        }
        match self.environment {
            server::Environment::Global => 0,
            server::Environment::Venv => self.environment_options() - 2,
            server::Environment::Custom => self.environment_options() - 1,
        }
    }
    fn environment_option_label(&self, index: usize) -> String {
        if index == 0 {
            return "python3 (PATH)".into();
        }
        if let Some(found) = self.detected_options().nth(index - 1) {
            return found.label.clone();
        }
        if index == self.environment_options() - 2 {
            "Virtual environment".into()
        } else {
            "Custom Python path".into()
        }
    }
    fn choose_environment(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.environment_chosen = true;
        self.environment_index = index;
        self.environment_open = false;
        self.error = None;
        let detected = index
            .checked_sub(1)
            .and_then(|i| self.detected_options().nth(i))
            .cloned();
        let value = if let Some(found) = detected {
            self.environment = server::Environment::Custom;
            self.detected_choice = Some((found.path.clone(), found.label.clone()));
            found.path.to_string_lossy().into_owned()
        } else {
            self.detected_choice = None;
            self.environment = if index == 0 {
                server::Environment::Global
            } else if index == self.environment_options() - 2 {
                server::Environment::Venv
            } else {
                server::Environment::Custom
            };
            if self.environment == server::Environment::Venv {
                ".venv".into()
            } else {
                "python3".into()
            }
        };
        self.python
            .update(cx, |input, cx| input.set_value(value, cx));
        self.refresh_environments(cx);
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
                    .bg(t.card.bg)
                    .text_color(t.card.fg)
                    .border_1()
                    .border_color(t.app.border)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.app.accent).text_color(t.app.accent_fg))
                    .on_click(cx.listener(|this, event: &gpui::ClickEvent, window, cx| {
                        if event.is_keyboard() {
                            return;
                        }
                        this.environment_open = !this.environment_open;
                        this.environment_keyboard = false;
                        this.environment_index = this.selected_environment_index(cx);
                        this.environment_focus.focus(window, cx);
                        cx.notify();
                    }))
                    .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, window, cx| {
                        this.environment_keyboard = true;
                        match e.keystroke.key.as_str() {
                            "enter" | "space" if this.environment_open => {
                                this.choose_environment(this.environment_index, window, cx)
                            }
                            "enter" | "space" | "down" | "up" if !this.environment_open => {
                                this.environment_open = true
                            }
                            "down" => {
                                this.environment_index =
                                    (this.environment_index + 1) % this.environment_options()
                            }
                            "up" => {
                                this.environment_index =
                                    (this.environment_index + this.environment_options() - 1)
                                        % this.environment_options()
                            }
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
                    .child(
                        self.detected_choice
                            .as_ref()
                            .filter(|(path, _)| path != &PathBuf::from("python3"))
                            .map(|(_, label)| label.clone())
                            .unwrap_or_else(|| {
                                self.environment_option_label(self.selected_environment_index(cx))
                            }),
                    )
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
                                .on_mouse_move(cx.listener(|this, _, _, cx| {
                                    if this.environment_keyboard {
                                        this.environment_keyboard = false;
                                        cx.notify();
                                    }
                                }))
                                .w(px(390.))
                                .max_h(px(280.))
                                .overflow_y_scroll()
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
                                .child(
                                    div()
                                        .px_2()
                                        .py_1()
                                        .text_size(t.font.xs)
                                        .text_color(t.popover.muted_fg)
                                        .child("Python runtimes"),
                                )
                                .children((0..self.environment_options()).map(|i| {
                                    div()
                                        .when(i == self.environment_options() - 2, |v| {
                                            v.child(div().my_1().h(px(1.)).bg(t.popover.border))
                                        })
                                        .child(
                                            div()
                                                .id(("environment-option", i))
                                                .px_2()
                                                .py_1()
                                                .rounded(t.radius.md())
                                                .cursor_pointer()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .when(
                                                    self.environment_keyboard
                                                        && self.environment_index == i,
                                                    |v| v.bg(t.launcher_option_hover_bg),
                                                )
                                                .hover(|s| s.bg(t.launcher_option_hover_bg))
                                                .child(div().size(px(12.)).flex_shrink_0().when(
                                                    self.selected_environment_index(cx) == i,
                                                    |v| {
                                                        v.child(
                                                            crate::icon::Icon::Check
                                                                .svg()
                                                                .size(px(12.))
                                                                .text_color(t.popover.fg),
                                                        )
                                                    },
                                                ))
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .flex()
                                                        .flex_col()
                                                        .child(self.environment_option_label(i))
                                                        .when_some(
                                                            self.detected_options()
                                                                .nth(i.wrapping_sub(1)),
                                                            |v, e| {
                                                                v.child(
                                                                    div()
                                                                        .text_size(t.font.xs)
                                                                        .font_family(
                                                                            t.font.mono.clone(),
                                                                        )
                                                                        .text_color(
                                                                            t.popover.muted_fg,
                                                                        )
                                                                        .truncate()
                                                                        .child(
                                                                            e.path
                                                                                .to_string_lossy()
                                                                                .into_owned(),
                                                                        ),
                                                                )
                                                            },
                                                        ),
                                                )
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.choose_environment(i, window, cx)
                                                    },
                                                )),
                                        )
                                })),
                        ),
                ))
            })
    }
}
impl ConnectionWindow {
    fn page_layer(
        &self,
        page: Page,
        offset: f32,
        opacity: f32,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        let t = cx.theme();
        let group = || {
            div()
                .w_full()
                .bg(if t.is_dark { t.card.bg } else { t.app.bg })
                .text_color(if t.is_dark { t.card.fg } else { t.app.fg })
                .border_1()
                .border_color(t.launcher_border)
                .rounded(t.radius.xl2())
                .shadow(vec![t.launcher_shadow.clone()])
                .overflow_hidden()
        };
        let divider = || div().mx_3().h(px(1.)).bg(t.launcher_border);
        let mut body = div().w_full().flex().flex_col().gap_3();
        if self.installing {
            body = body.child(
                group()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(div().font_weight(gpui::FontWeight::MEDIUM).child(
                        if self.creating_environment {
                            "Creating environment and installing packages…"
                        } else {
                            "Installing packages…"
                        },
                    ))
                    .child(
                        div()
                            .font_family(t.font.mono.clone())
                            .text_size(t.font.xs)
                            .text_color(if t.is_dark {
                                t.card.muted_fg
                            } else {
                                t.app.muted_fg
                            })
                            .w_full()
                            .truncate()
                            .child(self.selected_python(cx).to_string_lossy().into_owned()),
                    )
                    .child(
                        div().w_full().flex().justify_end().child(
                            button("cancel-install", "Cancel", false, cx)
                                .on_click(cx.listener(|this, _, _, cx| this.cancel_install(cx))),
                        ),
                    ),
            );
        } else if self.busy {
            body = body.child(
                group()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(div().font_weight(gpui::FontWeight::MEDIUM).child(
                        if page == Page::Local {
                            "Starting Jupyter…"
                        } else if self.reading_token {
                            "Loading saved token…"
                        } else {
                            "Connecting…"
                        },
                    ))
                    .child(
                        div().w_full().flex().justify_end().child(
                            button("cancel-connect", "Cancel", false, cx)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.task = gpui::Task::ready(());
                                    this.reading_token = false;
                                    this.busy = false;
                                    cx.notify();
                                }))
                                .on_key_down(cx.listener(
                                    |this, event: &gpui::KeyDownEvent, _, cx| {
                                        if matches!(event.keystroke.key.as_str(), "enter" | "space")
                                        {
                                            this.task = gpui::Task::ready(());
                                            this.reading_token = false;
                                            this.busy = false;
                                            cx.notify();
                                            cx.stop_propagation();
                                        }
                                    },
                                )),
                        ),
                    ),
            );
        } else if page == Page::Recent {
            body = body.child(
                group()
                    .child(choice_row(
                        "Start a local server",
                        "Work in a folder using Python or a virtual environment.",
                        self.page_button("new-local", "Create", Page::Local, true, cx),
                        cx,
                    ))
                    .child(divider())
                    .child(choice_row(
                        "Connect to a server",
                        "Open an existing Jupyter server using its URL and token.",
                        self.page_button("new-existing", "Connect", Page::Existing, false, cx),
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
                                    .flex_shrink_0()
                                    .text_color(t.secondary_fg),
                            ),
                    ),
                )
                .child(
                    div()
                        .text_size(t.font.base)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(if page == Page::Local {
                            "Start a local server"
                        } else {
                            "Connect to a server"
                        }),
                );
            if page == Page::Local {
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
                    .child(div().flex().items_center().gap_2()
                        .child(div().flex_1().min_w_0().text_size(t.font.xs).text_color(t.card.muted_fg).child(
                            if self.discovering { "Checking environments…".to_string() }
                            else if self.missing_venv(cx).is_some() { "No environment here yet. Create one with python3.".into() }
                            else if let Some(found) = self.detected.iter().find(|e| e.path == self.selected_python(cx)) {
                                if found.ready { "Ready to start".into() }
                                else if !found.missing.is_empty() { found.status.clone() }
                                else { format!("{}. Choose another Python or refresh to retry.", found.status) }
                            } else { "Choose an environment or enter a custom Python path.".into() }))
                        .when(!self.discovering && self.detected.iter().any(|e| e.path == self.selected_python(cx) && !e.missing.is_empty()), |v| v
                            .child(button("install-packages", "Install…", false, cx).flex_shrink_0()
                                .on_click(cx.listener(|this, _, window, cx| this.install_missing(window, cx)))))
                        .when(!self.discovering && self.missing_venv(cx).is_some(), |v| v
                            .child(button("create-environment", "Create environment…", false, cx).flex_shrink_0()
                                .on_click(cx.listener(|this, _, window, cx| this.install_missing(window, cx)))))
                        .child(button("refresh-environments", "Refresh", false, cx).flex_shrink_0()
                            .on_click(cx.listener(|this, _, _, cx| this.rescan_environments(cx)))))
                    .when(matches!(self.environment, server::Environment::Venv | server::Environment::Custom), |v| v.child(field(
                        if self.environment == server::Environment::Venv { "Virtual environment folder" } else { "Python executable" }, self.python.clone(), cx)))
                    .child(div().text_size(t.font.xs).text_color(t.card.muted_fg).child(match self.environment {
                        server::Environment::Global => "Uses python3 from PATH.",
                        server::Environment::Venv => "Relative paths start at the project folder.",
                        server::Environment::Custom => "Uses this Python interpreter directly, including uv-managed Python.",
                    })))
                    .child(div().flex().justify_end().child(self.submit_button("Start server", cx)));
            } else {
                body = body
                    .child(
                        group()
                            .p_3()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(field("Server URL", self.address.clone(), cx))
                            .child(field("Token (optional)", self.token.clone(), cx))
                            .child(div().w_full().max_w(px(420.)).flex().items_center().justify_between()
                            .child(self.remember_control(cx))
                            .when_some(server::parse_connection(self.address.read(cx).value(), "").ok().and_then(|(a, _)| ssh::host(self.ssh_host.read(cx).value()).ok().map(|host| ssh::identity(&a, host.as_deref()))).filter(|address| cx.global::<Connections>().recent.iter().any(|r| r.key() == *address && r.saved_token)), |v, address| {
                                let key_address = address.clone();
                                v.child(div().flex().child(div().id("forget-token").tab_index(0).py_1().text_size(t.font.xs).text_color(t.app.muted_fg).cursor_pointer().hover(|s| s.underline().text_color(t.app.fg)).focus_visible(|s| s.underline().text_color(t.app.fg)).child("Forget token")
                                    .on_click(cx.listener(move |this, _, window, cx| this.forget_token(address.clone(), window, cx)))
                                    .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                            cx.stop_propagation(); this.forget_token(key_address.clone(), window, cx);
                                        }
                                    }))))
                            })
                            )
                            .child(
                                div()
                                    .text_size(t.font.xs)
                                    .text_color(t.card.muted_fg)
                                    .child(credentials::help()),
                            ),
                    )
                    .child(div().flex().flex_col().gap_2()
                        .child(div().flex().child(button("advanced-connection",
                            if self.ssh_host.read(cx).value().trim().is_empty() { "Advanced" } else { "Advanced · SSH enabled" }, false, cx)
                            .flex().items_center().gap_1()
                            .child(if self.advanced { crate::icon::Icon::CaretDown } else { crate::icon::Icon::CaretRight }.svg().size(px(12.)).text_color(t.app.fg))
                            .on_click(cx.listener(|this, _, _, cx| { this.advanced = !this.advanced; cx.notify(); }))
                            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| { if matches!(event.keystroke.key.as_str(), "enter" | "space") { cx.stop_propagation(); this.advanced = !this.advanced; cx.notify(); } }))))
                        .when(self.advanced, |v| v.child(group().p_3().flex().flex_col().gap_2()
                            .child(field("SSH host (optional)", self.ssh_host.clone(), cx))
                            .child(div().text_size(t.font.xs).text_color(t.card.muted_fg).child("SSH alias or user@host. Server URL is relative to this host.")))))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .child(self.submit_button("Connect", cx)),
                    );
            }
        }
        if let Some(error) = &self.error {
            body = body.child(
                div()
                    .p_3()
                    .rounded(t.radius.lg())
                    .bg(t.dialog_destructive_bg)
                    .text_color(t.dialog_destructive_fg)
                    .child(error.clone()),
            );
        }
        div()
            .id(match page {
                Page::Recent => "welcome-page",
                Page::Local => "local-page",
                Page::Existing => "existing-page",
            })
            .absolute()
            .inset_0()
            .left(px(offset))
            .right(px(-offset))
            .opacity(opacity)
            .flex()
            .flex_col()
            .when(page == Page::Recent && !self.busy, |v| {
                v.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(40.))
                        .flex()
                        .justify_center()
                        .font_family(t.font.brand.clone())
                        .text_size(t.font.brand_size)
                        .text_color(t.launcher_art_fg)
                        .children("Ganymede".chars().enumerate().map(|(i, ch)| {
                            div()
                                .relative()
                                .when(i < 7, |v| v.mr(t.font.brand_tracking))
                                .child(
                                    div()
                                        .absolute()
                                        .left(px(2.))
                                        .top(px(3.))
                                        .text_color(t.launcher_art_bg)
                                        .child(ch.to_string()),
                                )
                                .child(div().relative().child(ch.to_string()))
                        })),
                )
            })
            .child(
                div()
                    .h(px(28.))
                    .flex_shrink_0()
                    .window_control_area(gpui::WindowControlArea::Drag)
                    .when(cfg!(target_os = "linux"), |region| {
                        region.on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                            window.start_window_move();
                        })
                    }),
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
                            .when(page == Page::Recent && !self.busy, |v| v.pt(px(240.)))
                            .child(body),
                    ),
            )
            .into_any_element()
    }
}
impl Render for ConnectionWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = self.render_content(window, cx);
        #[cfg(target_os = "linux")]
        return self.linux_frame(content, window, cx);
        #[cfg(not(target_os = "linux"))]
        content
    }
}

impl ConnectionWindow {
    fn render_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        if let Some(workspace) = &self.workspace {
            self.planet = None;
            return div()
                .size_full()
                .track_focus(&self.focus)
                .child(workspace.clone())
                .into_any_element();
        }
        let progress = self
            .transition
            .map(|transition| {
                let linear = (transition.started.elapsed().as_secs_f32()
                    / PAGE_TRANSITION_DURATION.as_secs_f32())
                .min(1.);
                1. - (1. - linear).powi(3)
            })
            .unwrap_or(1.);
        if self.transition.is_some() {
            if progress >= 1. || cx.reduce_motion() || self.busy {
                self.transition = None;
                self.focus_page(window, cx);
            } else {
                window.request_animation_frame();
            }
        }
        let progress = if self.transition.is_none() {
            1.
        } else {
            progress
        };
        let planet = self
            .planet
            .get_or_insert_with(|| cx.new(|cx| planet::Planet::new(window, cx)))
            .clone();
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
                    .window_control_area(gpui::WindowControlArea::Drag)
                    .when(cfg!(target_os = "linux"), |region| {
                        region.on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                            window.start_window_move();
                        })
                    }),
            )
            .child(
                div()
                    .px_4()
                    .pt_2()
                    .pb_2()
                    .text_size(t.font.xs)
                    .text_color(surface.fg)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
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
                                .text_size(t.font.xs)
                                .opacity(0.85)
                                .child("Nothing yet"),
                        )
                    })
                    .children(recent.into_iter().enumerate().map(|(i, recent)| {
                        let click = recent.clone();
                        let settings = recent.clone();
                        let remove = recent.clone();
                        let edit_key = recent.clone();
                        let remove_key = recent.key();
                        let address_only = recent.ssh_host.is_none()
                            && recent.local_info.is_none()
                            && recent.local_id.is_none()
                            && (recent.label == "Configured server" || recent.label == recent.address);
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
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .text_size(t.font.sm)
                                            .truncate()
                                            .child(if address_only || recent.ssh_host.is_some() { recent.address.clone() } else { recent.label.clone() }),
                                    )
                                    .when(!address_only, |v| v.child(
                                        div()
                                            .text_size(t.font.xs)
                                            .text_color(surface.muted_fg)
                                            .truncate()
                                            .child(
                                                match if recent.ssh_host.is_none() {
                                                    managed_server(&recent.address, cx)
                                                        .map(|s| s.state())
                                                } else {
                                                    None
                                                } {
                                                    Some(server::LocalState::Stopped) => {
                                                        "Stopped · Click to start".to_owned()
                                                    }
                                                    Some(server::LocalState::Busy) => {
                                                        "Starting…".to_owned()
                                                    }
                                                    Some(server::LocalState::Unverified) => {
                                                        "Local server · Not verified".to_owned()
                                                    }
                                                    _ => recent
                                                        .ssh_host
                                                        .as_ref()
                                                        .map(|host| {
                                                            format!("SSH · {host}")
                                                        })
                                                        .unwrap_or_else(|| recent.address.clone()),
                                                },
                                            ),
                                    )),
                            )
                            .when(!self.busy, |v| v.child(
                                div().relative().child(
                                    div().id(("recent-settings", i)).tab_index(0).p_1()
                                        .rounded(t.radius.md()).cursor_pointer()
                                        .hover(|s| s.bg(surface.accent).text_color(surface.accent_fg))
                                        .child(crate::icon::Icon::DotsThree.svg().size(px(16.)).text_color(surface.muted_fg))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.recent_menu = if this.recent_menu == Some(i) { None } else { Some(i) };
                                            cx.notify();
                                        }))
                                        .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                                cx.stop_propagation(); this.recent_menu = Some(i); cx.notify();
                                            }
                                        }))
                                ).when(self.recent_menu == Some(i), |v| v.child(gpui::deferred(
                                    gpui::anchored().snap_to_window_with_margin(px(8.)).child(
                                        div().id(("recent-menu", i)).occlude().w(px(160.)).p_1()
                                            .rounded(t.radius.md()).bg(t.popover.bg).text_color(t.popover.fg)
                                            .border_1().border_color(t.popover.border).shadow(vec![t.toast_shadow.clone()])
                                            .on_mouse_down_out(cx.listener(|this, _, _, cx| { this.recent_menu = None; cx.notify(); }))
                                            .child(div().id("edit-recent").tab_index(0).px_2().py_1().text_size(t.font.xs)
                                                .rounded(t.radius.sm()).cursor_pointer().hover(|s| s.bg(t.popover.accent).text_color(t.popover.accent_fg))
                                                .child("Edit connection…")
                                                .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| { if matches!(event.keystroke.key.as_str(), "enter" | "space") { cx.stop_propagation(); this.edit_saved(&edit_key, window, cx); } }))
                                                .on_click(cx.listener(move |this, _, window, cx| { cx.stop_propagation(); this.edit_saved(&settings, window, cx); })))
                                            .child(div().id("remove-recent").tab_index(0).px_2().py_1().text_size(t.font.xs)
                                                .rounded(t.radius.sm()).cursor_pointer().hover(|s| s.bg(t.popover.accent).text_color(t.popover.accent_fg))
                                                .child("Remove from recents")
                                                .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| { if matches!(event.keystroke.key.as_str(), "enter" | "space") { cx.stop_propagation(); this.remove_recent(&remove_key, cx); } }))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation(); this.remove_recent(&remove.key(), cx);
                                                })))
                                    )
                                )))
                            ))
                            .when(!self.busy && !self.installing, |v| {
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
        div()
            .size_full()
            .p_2()
            .relative()
            .overflow_hidden()
            .track_focus(&self.focus)
            .flex()
            .bg(t.launcher_art_bg)
            .text_color(background.fg)
            .font_family(t.font.sans.clone())
            .text_size(t.font.sm)
            .on_action(
                cx.listener(|this, _: &crate::CloseTab, window, cx| this.close(false, window, cx)),
            )
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if this.transition.is_some() {
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "tab" {
                    if event.keystroke.modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                    cx.stop_propagation();
                } else if event.keystroke.key == "escape" && !this.busy && !this.installing {
                    if this.recent_menu.take().is_none() {
                        this.page(Page::Recent, window, cx);
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .absolute()
                    .left(px(228.))
                    .right_0()
                    .top_0()
                    .bottom_0()
                    .child(planet),
            )
            .child(sidebar.relative())
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .when_some(self.transition, |v, transition| {
                        let direction = if self.page == Page::Recent { -1. } else { 1. };
                        v.child(self.page_layer(
                            transition.from,
                            -direction * 36. * progress,
                            1. - progress,
                            cx,
                        ))
                    })
                    .child(self.page_layer(
                        self.page,
                        if self.transition.is_some() {
                            let direction = if self.page == Page::Recent { -1. } else { 1. };
                            direction * 36. * (1. - progress)
                        } else {
                            0.
                        },
                        progress,
                        cx,
                    ))
                    .when(self.transition.is_some(), |v| {
                        v.child(div().absolute().inset_0().occlude())
                    }),
            )
            .child(self.toasts.clone())
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
        .find(|r| r.key() == address)
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

    fn credential_form(
        window: &mut Window,
        cx: &mut Context<ConnectionWindow>,
    ) -> ConnectionWindow {
        cx.set_global(
            crate::theme::ThemeSpec::light().resolve(&crate::theme::Derivation::default()),
        );
        cx.set_global(crate::theme::AppearancePreference::default());
        cx.set_global(Connections::default());
        let mut view = ConnectionWindow::new(window, cx);
        view.page = Page::Existing;
        view
    }

    #[gpui::test]
    fn remember_token_is_visible_and_can_be_disabled(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(credential_form);
        let bounds = cx
            .debug_bounds("remember-token")
            .expect("Remember token must be available on Linux too");
        assert!(view.read_with(cx, |v, _| v.remember_token));
        cx.simulate_click(bounds.center(), Default::default());
        assert!(!view.read_with(cx, |v, _| v.remember_token));
    }

    #[gpui::test]
    fn unavailable_saved_credentials_allow_manual_entry_without_retrying_storage(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(credential_form);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                assert!(
                    view.restore_token(Err(anyhow::anyhow!("locked: private-token")), window, cx)
                        .is_none()
                );
                assert!(!view.remember_token);
                assert!(view.token.read(cx).focus_handle(cx).is_focused(window));
                assert!(!view.error.as_ref().unwrap().contains("private-token"));
            })
        });
        cx.simulate_input("manual-token");
        view.read_with(cx, |view, cx| {
            assert_eq!(view.token.read(cx).value(), "manual-token");
            assert!(!view.remember_token);
        });
    }

    #[gpui::test]
    fn restored_credentials_populate_the_token_field(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(credential_form);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.error = Some("Previous error".into());
                assert_eq!(
                    view.restore_token(
                        Ok(Some(("token".into(), b"saved-token".to_vec()))),
                        window,
                        cx
                    )
                    .as_deref(),
                    Some("saved-token")
                );
                assert_eq!(view.token.read(cx).value(), "saved-token");
                assert!(view.error.is_none());
            })
        });
    }

    #[gpui::test]
    fn failed_forget_keeps_the_saved_record_and_cached_credentials(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(credential_form);
        cx.update(|_, cx| {
            let (address, config) =
                server::parse_connection("https://example.test/", "test-token").unwrap();
            cx.global_mut::<Connections>().recent.push(Recent {
                label: "Test".into(),
                address: address.clone(),
                config: Some(config),
                saved_token: true,
                ssh_host: None,
                local_info: None,
                local_id: None,
            });
            view.update(cx, |view, cx| {
                view.busy = true;
                view.finish_forgetting(&address, Err(anyhow::anyhow!("locked: test-token")), cx);
                assert!(!view.busy);
                assert!(view.remember_token);
                assert!(!view.error.as_ref().unwrap().contains("test-token"));
            });
            let recent = &cx.global::<Connections>().recent[0];
            assert!(recent.saved_token);
            assert!(recent.config.is_some());
        });
    }

    #[gpui::test]
    fn forgetting_an_already_missing_token_clears_the_form(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(credential_form);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.token
                    .update(cx, |input, cx| input.set_value("test-token".into(), cx));
                // GPUI's test credential backend returns success for an absent item.
                view.forget_token("https://example.test/".into(), window, cx);
            })
        });
        cx.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert!(!view.busy);
            assert!(!view.remember_token);
            assert!(view.token.read(cx).value().is_empty());
            assert!(view.error.is_none());
        });
    }

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
            saved_token: true,
            ssh_host: None,
        }]);
        assert_eq!(data[0]["url"], "https://example.test/");
        assert!(!data.to_string().contains("private-token"));
        assert!(data[0].get("config").is_none());
    }

    #[gpui::test]
    fn environment_keyboard_selection_closes_the_menu(cx: &mut gpui::TestAppContext) {
        let window = cx.update(|cx| {
            cx.set_global(
                crate::theme::ThemeSpec::light().resolve(&crate::theme::Derivation::default()),
            );
            cx.set_global(crate::theme::AppearancePreference::default());
            cx.set_global(Connections::default());
            let window = cx
                .open_window(gpui::WindowOptions::default(), |window, cx| {
                    cx.new(|cx| ConnectionWindow::new(window, cx))
                })
                .unwrap();
            window
                .update(cx, |this, window, cx| {
                    this.page = Page::Local;
                    this.environment_open = true;
                    this.environment_index = 0;
                    this.environment_focus.focus(window, cx);
                    cx.notify();
                })
                .unwrap();
            window
        });
        cx.run_until_parked();
        cx.simulate_keystrokes(window.into(), "down enter");
        cx.update(|cx| {
            window
                .update(cx, |this, _, _| {
                    assert!(this.environment == server::Environment::Venv);
                    assert!(!this.environment_open);
                })
                .unwrap()
        });
    }

    #[gpui::test]
    fn discovery_preserves_manual_choices_and_rejects_stale_results(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(
                crate::theme::ThemeSpec::light().resolve(&crate::theme::Derivation::default()),
            );
            cx.set_global(crate::theme::AppearancePreference::default());
            cx.set_global(Connections::default());
            let window = cx
                .open_window(gpui::WindowOptions::default(), |window, cx| {
                    cx.new(|cx| ConnectionWindow::new(window, cx))
                })
                .unwrap();
            window
                .update(cx, |this, window, cx| {
                    let key = (
                        this.folder.read(cx).value().trim().to_string(),
                        "python3".into(),
                    );
                    this.discovery_key = key.clone();
                    let ready = discovery::Environment {
                        path: "/project/.venv/bin/python".into(),
                        label: "Project .venv".into(),
                        status: "Ready".into(),
                        ready: true,
                        missing: Vec::new(),
                        project: true,
                    };
                    this.environment_chosen = true;
                    this.finish_discovery(key.clone(), vec![ready.clone()], cx);
                    assert_eq!(this.selected_python(cx), PathBuf::from("python3"));
                    this.environment_chosen = false;
                    this.finish_discovery(key.clone(), vec![ready.clone()], cx);
                    assert_eq!(this.selected_python(cx), ready.path);
                    this.choose_environment(0, window, cx);
                    let global = discovery::Environment {
                        path: "python3".into(),
                        ..ready.clone()
                    };
                    this.finish_discovery(this.discovery_key.clone(), vec![global], cx);
                    assert!(this.environment == server::Environment::Global);
                    assert_eq!(this.selected_environment_index(cx), 0);
                    assert!(this.detected_choice.is_none());
                    this.detected.insert(
                        0,
                        discovery::Environment {
                            path: "/missing/python".into(),
                            ready: false,
                            status: "Python was not found".into(),
                            ..ready.clone()
                        },
                    );
                    assert_eq!(this.environment_options(), 3);
                    this.detected.push(ready.clone());
                    assert_eq!(this.environment_options(), 4);
                    assert_eq!(this.detected_options().next().unwrap().path, ready.path);
                    this.choose_environment(1, window, cx);
                    assert_eq!(this.selected_python(cx), ready.path);
                    this.finish_discovery(this.discovery_key.clone(), vec![ready.clone()], cx);
                    assert_eq!(this.selected_environment_index(cx), 1);
                    this.choose_environment(2, window, cx);
                    assert!(this.environment == server::Environment::Venv);
                    this.finish_discovery(this.discovery_key.clone(), vec![ready.clone()], cx);
                    assert_eq!(this.selected_environment_index(cx), 2);
                    this.choose_environment(3, window, cx);
                    assert!(this.environment == server::Environment::Custom);
                    this.finish_discovery(this.discovery_key.clone(), vec![ready.clone()], cx);
                    assert_eq!(this.selected_environment_index(cx), 3);
                    // A new folder makes an old result stale, even before its observer runs.
                    this.folder.update(cx, |field, cx| {
                        field.set_value("/different/project".into(), cx)
                    });
                    this.detected.clear();
                    this.discovery_key = key.clone();
                    this.finish_discovery(key, vec![ready], cx);
                    assert!(this.detected.is_empty());
                })
                .unwrap();
        });
    }

    #[gpui::test]
    fn cancelling_setup_prompts_does_not_install_or_create_a_folder(cx: &mut gpui::TestAppContext) {
        let window = cx.update(|cx| {
            cx.set_global(
                crate::theme::ThemeSpec::light().resolve(&crate::theme::Derivation::default()),
            );
            cx.set_global(crate::theme::AppearancePreference::default());
            cx.set_global(Connections::default());
            cx.open_window(gpui::WindowOptions::default(), |window, cx| {
                cx.new(|cx| ConnectionWindow::new(window, cx))
            })
            .unwrap()
        });
        cx.run_until_parked();
        cx.update(|cx| {
            window
                .update(cx, |this, window, cx| {
                    this.discovering = false;
                    this.detected = vec![discovery::Environment {
                        path: "python3".into(),
                        label: "Python".into(),
                        status: "Missing ipykernel".into(),
                        ready: false,
                        project: false,
                        missing: vec!["ipykernel"],
                    }];
                    this.install_missing(window, cx);
                })
                .unwrap()
        });
        cx.run_until_parked();
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        cx.update(|cx| {
            window
                .update(cx, |this, _, _| assert!(!this.installing))
                .unwrap()
        });
        let target = std::env::temp_dir().join(format!("ganymede-cancel-{}", uuid::Uuid::new_v4()));
        cx.update(|cx| {
            window
                .update(cx, |this, window, cx| {
                    this.environment = server::Environment::Venv;
                    this.environment_chosen = true;
                    this.python.update(cx, |field, cx| {
                        field.set_value(target.to_string_lossy().into_owned(), cx)
                    });
                    this.discovering = false;
                    assert_eq!(this.missing_venv(cx).as_ref(), Some(&target));
                    this.install_missing(window, cx);
                })
                .unwrap();
        });
        cx.run_until_parked();
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert!(!target.exists());
        cx.update(|cx| {
            window
                .update(cx, |this, _, _| {
                    assert!(!this.installing);
                    assert!(!this.creating_environment);
                })
                .unwrap();
        });
    }

    #[gpui::test]
    fn update_menu_starts_a_check_from_the_launcher(cx: &mut gpui::TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        cx.update(|cx| {
            cx.set_global(
                crate::theme::ThemeSpec::light().resolve(&crate::theme::Derivation::default()),
            );
            cx.set_global(crate::theme::AppearancePreference::default());
            cx.set_global(Runtime {
                tunnel: None,
                handle: runtime.handle().clone(),
                config: Err("No test network".into()),
                kernel: "python3".into(),
            });
            crate::menus::init(cx);
            init(cx);
            open(cx);
            let window = cx
                .windows()
                .into_iter()
                .find_map(|w| w.downcast::<ConnectionWindow>())
                .unwrap();
            window
                .update(cx, |_, window, cx| {
                    window.dispatch_action(Box::new(crate::menus::CheckForUpdates), cx)
                })
                .unwrap();
        });
        cx.run_until_parked();
        cx.read(|cx| {
            assert!(
                cx.try_global::<crate::updates::CheckState>()
                    .is_some_and(|s| s.running)
            )
        });
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
                tunnel: None,
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
                    assert!(window.is_minimizable());
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
            windows[0]
                .update(cx, |_, window, cx| {
                    window.dispatch_action(Box::new(crate::Quit), cx);
                })
                .unwrap();
        });
        cx.run_until_parked();
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
