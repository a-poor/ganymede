use super::*;
use crate::connection::server::LocalState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Checking,
    Connected,
    Disconnected,
    Available,
    Reconnecting,
}
impl Status {
    fn label(self) -> &'static str {
        match self {
            Self::Checking => "Checking…",
            Self::Connected => "Connected",
            Self::Disconnected => "Disconnected",
            Self::Available => "Reconnect needed",
            Self::Reconnecting => "Reconnecting…",
        }
    }
    fn checked(self, success: bool, reconnect: bool) -> Self {
        if !success {
            Self::Disconnected
        } else if reconnect || matches!(self, Self::Checking | Self::Connected) {
            Self::Connected
        } else {
            Self::Available
        }
    }
}

pub(super) struct Server {
    label: String,
    status: Status,
    checking: bool,
    menu: bool,
    selected: Option<usize>,
    focus: gpui::FocusHandle,
}
impl Server {
    pub fn new(cx: &mut App) -> Self {
        Self {
            label: "Jupyter server".into(),
            status: Status::Checking,
            checking: false,
            menu: false,
            selected: None,
            focus: cx.focus_handle(),
        }
    }
}
impl Workspace {
    fn local_server(&self, cx: &App) -> Option<crate::connection::server::ManagedServer> {
        self.runtime
            .config
            .as_ref()
            .ok()
            .and_then(|config| crate::connection::managed_server(config.base_url().as_str(), cx))
    }

    fn local_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(server) = self.local_server(cx) else {
            return;
        };
        let state = server.state();
        if state == LocalState::Busy || window.has_active_prompt() {
            return;
        }
        let restart = state == LocalState::Stopped;
        let answer = window.prompt(gpui::PromptLevel::Warning,
            if restart { "Restart local server?" } else { "Stop local server?" },
            Some(if restart { "The server will use the same project folder and Python environment. Cells will not run automatically." }
                 else { "This ends all kernels and running work on this server, including in other windows or browsers. Open documents and unsaved edits will be kept." }),
            &["Cancel", if restart { "Restart" } else { "Stop" }], cx);
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(1) {
                return;
            }
            let task = this.update(cx, |this, cx| {
                this.persist_recovery(cx);
                for doc in &this.documents {
                    if let Content::Notebook(view) = &doc.content {
                        view.update(cx, |view, cx| view.cancel_server_queue(cx));
                    }
                }
                let task = this
                    .runtime
                    .handle
                    .spawn(async move { server.change(restart).await });
                cx.refresh_windows();
                task
            });
            let Ok(task) = task else {
                return;
            };
            let result = task
                .await
                .unwrap_or_else(|_| Err("Server operation interrupted.".into()));
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) if restart => {
                        if let Ok(config) = &this.runtime.config {
                            crate::connection::update_local_history(config.base_url().as_str(), cx);
                        }
                        this.check_server(true, cx);
                    }
                    Ok(()) => {
                        this.server.status = Status::Disconnected;
                    }
                    Err(error) => this.toasts.update(cx, |host, cx| {
                        host.push(
                            crate::toast::Toast::new("Server operation failed")
                                .description(error)
                                .kind(crate::toast::ToastKind::Error),
                            cx,
                        );
                    }),
                }
                cx.refresh_windows();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn server_unavailable(&self) -> bool {
        self.server.status == Status::Disconnected
    }

    pub fn set_server_label(&mut self, label: &str) {
        self.server.label = label.into();
    }

    pub fn check_server(&mut self, reconnect: bool, cx: &mut Context<Self>) {
        if self.server.checking
            || self
                .local_server(cx)
                .is_some_and(|s| s.state() != LocalState::Running)
        {
            return;
        }
        self.server.checking = true;
        if reconnect {
            self.server.status = Status::Reconnecting;
        }
        let config = self.runtime.config.clone();
        let task = self.runtime.handle.spawn(async move {
            let mut config = config?;
            config.connect_timeout = Duration::from_secs(3);
            config.request_timeout = Duration::from_secs(5);
            // Sessions is authenticated; /api may succeed even when credentials expire.
            JupyterRestClient::new(config)
                .map_err(|e| e.to_string())?
                .list_sessions()
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|_| Err("Could not reach the server".into()));
            this.update(cx, |this, cx| {
                this.server.checking = false;
                if this.local_server(cx).is_some_and(|s| s.state() != LocalState::Running) {
                    cx.notify();
                    return;
                }
                let previous = this.server.status;
                this.server.status = previous.checked(result.is_ok(), reconnect);
                if result.is_err() && previous != Status::Disconnected {
                    for doc in &this.documents {
                        if let Content::Notebook(view) = &doc.content {
                            view.update(cx, |view, cx| view.cancel_server_queue(cx));
                        }
                    }
                    if !reconnect { this.toasts.update(cx, |host, cx| {
                        host.push(crate::toast::Toast::new("Server disconnected")
                            .description("Your open documents and unsaved edits are kept. Use the server menu to reconnect.")
                            .kind(crate::toast::ToastKind::Error), cx);
                    }); }
                }
                if reconnect {
                    match result {
                        Ok(()) => {
                            for doc in &this.documents {
                                if let Content::Notebook(view) = &doc.content {
                                    view.update(cx, |view, cx| view.recover_server_session(cx));
                                }
                            }
                            this.refresh_files(cx);
                            this.refresh_kernels(false, cx);
                        }
                        Err(_) => this.toasts.update(cx, |host, cx| {
                            host.push(crate::toast::Toast::new("Could not reconnect")
                                .description("Check that the server is running and reachable. If its address or token changed, open a new connection window.")
                                .kind(crate::toast::ToastKind::Error), cx);
                        }),
                    }
                }
                cx.notify();
            }).ok();
        }).detach();
        cx.notify();
    }

    fn server_action(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.server.menu = false;
        window.blur();
        match index {
            0 => self.check_server(true, cx),
            1 => {
                if let Ok(config) = &self.runtime.config {
                    cx.open_url(config.base_url().as_str());
                }
            }
            2 => crate::connection::open(cx),
            3 => self.local_action(window, cx),
            _ => (),
        }
        cx.notify();
    }

    pub fn server_button(entity: &Entity<Self>, cx: &App) -> impl IntoElement {
        let this = entity.read(cx);
        let local = this.local_server(cx).map(|s| s.state());
        let status_label = match local {
            Some(LocalState::Stopped) => "Stopped",
            Some(LocalState::Busy) => "Working…",
            _ => this.server.status.label(),
        };
        let mut actions = vec![
            ("Reconnect", Icon::Restart),
            ("Open server in browser", Icon::Browsers),
            ("New connection window…", Icon::AppWindow),
        ];
        if let Some(state) = local {
            actions.push(if state == LocalState::Stopped {
                ("Restart Server…", Icon::Restart)
            } else {
                ("Stop Server…", Icon::Stop)
            });
        }
        let action_count = actions.len();
        let t = cx.theme();
        let surface = t.sidebar;
        let pending = local == Some(LocalState::Busy)
            || (local != Some(LocalState::Stopped)
                && matches!(
                    this.server.status,
                    Status::Checking | Status::Reconnecting | Status::Available
                ));
        let error =
            local != Some(LocalState::Stopped) && this.server.status == Status::Disconnected;
        let (badge_bg, badge_fg) = if error {
            (t.dialog_destructive_bg, t.dialog_destructive_fg)
        } else if pending {
            (t.warning_subtle, t.warning_fg)
        } else {
            (t.secondary, t.secondary_fg)
        };
        let weak = entity.downgrade();
        let keys = weak.clone();
        let hover = weak.clone();
        let outside = weak.clone();
        let mut result = div()
            .relative()
            .w(px(32.))
            .flex_shrink_0()
            .px_1()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up_out(gpui::MouseButton::Left, move |_, _, cx| {
                outside
                    .update(cx, |this, cx| {
                        if this.server.menu {
                            this.server.menu = false;
                            cx.notify();
                        }
                    })
                    .ok();
            })
            .child(
                div()
                    .id("server-menu-button")
                    .track_focus(&this.server.focus)
                    .tab_index(0)
                    .aria_label(format!("{}: {}", this.server.label, status_label))
                    .relative()
                    .size(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(t.radius.md())
                    .cursor_pointer()
                    .hover(|s| s.bg(surface.accent))
                    .on_hover(move |over, _, cx| {
                        if *over {
                            hover
                                .update(cx, |this, cx| {
                                    if !this.server.menu {
                                        this.server.menu = true;
                                        this.server.selected = None;
                                        cx.notify();
                                    }
                                })
                                .ok();
                        }
                    })
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        weak.update(cx, |this, cx| {
                            this.server.menu = true;
                            this.server.selected = None;
                            this.server.focus.focus(window, cx);
                            cx.notify();
                        })
                        .ok();
                    })
                    .on_key_down(move |event: &gpui::KeyDownEvent, window, cx| {
                        keys.update(cx, |this, cx| {
                            match event.keystroke.key.as_str() {
                                "escape" | "tab" => this.server.menu = false,
                                "down" | "up" => {
                                    this.server.menu = true;
                                    this.server.selected = Some(match this.server.selected {
                                        None => 0,
                                        Some(i) if event.keystroke.key == "down" => {
                                            (i + 1) % action_count
                                        }
                                        Some(i) => (i + action_count - 1) % action_count,
                                    });
                                }
                                "enter" | "space" => {
                                    if this.server.menu {
                                        this.server_action(
                                            this.server.selected.unwrap_or(0),
                                            window,
                                            cx,
                                        );
                                    } else {
                                        this.server.menu = true;
                                        this.server.selected = Some(0);
                                    }
                                }
                                _ => return,
                            }
                            if event.keystroke.key != "tab" {
                                cx.stop_propagation();
                            }
                            cx.notify();
                        })
                        .ok();
                    })
                    .child(
                        Icon::Server
                            .svg()
                            .size(px(18.))
                            .text_color(surface.muted_fg),
                    )
                    .when(pending || error, |v| {
                        v.child(
                            div()
                                .absolute()
                                .top(px(1.))
                                .right(px(1.))
                                .size(px(8.))
                                .rounded_full()
                                .border_1()
                                .border_color(surface.bg)
                                .bg(if error { t.destructive } else { t.warning }),
                        )
                    }),
            );
        if this.server.menu {
            let address = this
                .runtime
                .config
                .as_ref()
                .map(|c| c.base_url().to_string())
                .unwrap_or_default();
            let copy_address = address.clone();
            let keyboard_address = address.clone();
            result = result.child(gpui::deferred(
                gpui::anchored()
                    .offset(gpui::point(px(0.), px(4.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .id("server-menu")
                            .occlude()
                            .w(px(250.))
                            .p_1()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .rounded(t.radius.md())
                            .bg(t.popover.bg)
                            .text_color(t.popover.fg)
                            .border_1()
                            .border_color(t.popover.border)
                            .shadow(vec![t.toast_shadow.clone()])
                            .child(
                                div()
                                    .px_2()
                                    .pt_1()
                                    .text_size(t.font.xs)
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(this.server.label.clone()),
                            )
                            .child(
                                div().px_2().py_1().flex().child(
                                    div()
                                        .px_2()
                                        .py(px(1.))
                                        .rounded_full()
                                        .text_size(t.font.xs)
                                        .font_weight(FontWeight::MEDIUM)
                                        .bg(badge_bg)
                                        .text_color(badge_fg)
                                        .child(status_label),
                                ),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .flex()
                                    .items_start()
                                    .gap_1()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .whitespace_normal()
                                            .text_size(t.font.xs)
                                            .text_color(t.popover.muted_fg)
                                            .child(wrappable_address(&address)),
                                    )
                                    .child(
                                        icon_button(
                                            "copy-server-address",
                                            "Copy server address",
                                            Icon::Copy,
                                            cx,
                                        )
                                        .on_click(move |_, _, cx| {
                                            cx.stop_propagation();
                                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                                copy_address.clone(),
                                            ));
                                        })
                                        .on_key_down(
                                            move |event: &gpui::KeyDownEvent, _, cx| {
                                                if matches!(
                                                    event.keystroke.key.as_str(),
                                                    "enter" | "space"
                                                ) {
                                                    cx.stop_propagation();
                                                    cx.write_to_clipboard(
                                                        gpui::ClipboardItem::new_string(
                                                            keyboard_address.clone(),
                                                        ),
                                                    );
                                                }
                                            },
                                        ),
                                    ),
                            )
                            .children(actions.into_iter().enumerate().map(
                                |(index, (label, icon))| {
                                    let click = entity.downgrade();
                                    let hover = click.clone();
                                    let selected = this.server.selected == Some(index);
                                    let fg = if selected {
                                        t.popover.accent_fg
                                    } else {
                                        t.popover.fg
                                    };
                                    div()
                                        .id(("server-action", index))
                                        .h(px(28.))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .rounded(t.radius.sm())
                                        .text_size(t.font.xs)
                                        .text_color(fg)
                                        .cursor_pointer()
                                        .when(selected, |v| v.bg(t.popover.accent))
                                        .when(
                                            (index == 0
                                                && (this.server.checking
                                                    || local.is_some_and(|s| {
                                                        s != LocalState::Running
                                                    })))
                                                || (index == 3 && local == Some(LocalState::Busy)),
                                            |v| v.opacity(0.4),
                                        )
                                        .child(icon.svg().size(px(14.)).text_color(fg))
                                        .child(label)
                                        .on_hover(move |over, _, cx| {
                                            hover
                                                .update(cx, |this, cx| {
                                                    if *over {
                                                        this.server.selected = Some(index);
                                                    } else if this.server.selected == Some(index) {
                                                        this.server.selected = None;
                                                    }
                                                    cx.notify();
                                                })
                                                .ok();
                                        })
                                        .on_click(move |_, window, cx| {
                                            cx.stop_propagation();
                                            click
                                                .update(cx, |this, cx| {
                                                    this.server_action(index, window, cx)
                                                })
                                                .ok();
                                        })
                                },
                            )),
                    ),
            ));
        }
        result
    }
}

// Add display-only break opportunities; clipboard/browser actions use the original URL.
fn wrappable_address(address: &str) -> String {
    let mut result = String::new();
    let mut run = 0;
    for ch in address.chars() {
        result.push(ch);
        run += 1;
        if matches!(ch, '/' | '.' | '-' | '_') || run >= 24 {
            result.push('\u{200b}');
            run = 0;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovered_server_requires_explicit_reconnect() {
        assert_eq!(
            Status::Connected.checked(false, false),
            Status::Disconnected
        );
        assert_eq!(Status::Disconnected.checked(true, false), Status::Available);
        assert_eq!(Status::Available.checked(true, false), Status::Available);
        assert_eq!(Status::Reconnecting.checked(true, true), Status::Connected);
        assert_eq!(
            Status::Reconnecting.checked(false, true),
            Status::Disconnected
        );
    }
}
