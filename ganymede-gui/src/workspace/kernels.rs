use super::*;
use ganymede_core::jupyter::models::{Kernel, Session as ServerSession};

#[derive(Default)]
pub(super) struct KernelList {
    items: Vec<Kernel>,
    sessions: Vec<ServerSession>,
    loading: bool,
    loaded: bool,
    error: Option<String>,
    shutting_down: HashSet<String>,
    list_view: bool,
    collapsed: HashSet<String>,
    names: std::collections::BTreeMap<String, String>,
}
impl Workspace {
    pub fn refresh_kernels(&mut self, report: bool, cx: &mut Context<Self>) {
        if self.kernels.loading {
            return;
        }
        self.kernels.loading = true;
        let config = self.runtime.config.clone();
        let task = self.runtime.handle.spawn(async move {
            let client = JupyterRestClient::new(config?).map_err(|e| e.to_string())?;
            let (kernels, sessions, specs) = tokio::try_join!(
                client.list_kernels(),
                client.list_sessions(),
                client.list_kernel_specs()
            )
            .map_err(|e| e.to_string())?;
            Ok::<_, String>((kernels, sessions, specs))
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Could not load running kernels".into()));
            this.update(cx, |this, cx| {
                this.kernels.loading = false;
                this.kernels.loaded = true;
                match result {
                    Ok((mut kernels, sessions, specs)) => {
                        this.kernels.names = specs
                            .kernelspecs
                            .into_iter()
                            .map(|(name, entry)| (name, entry.spec.display_name))
                            .collect();
                        kernels.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
                        this.kernels.items = kernels;
                        this.kernels.sessions = sessions;
                        this.kernels.error = None;
                    }
                    Err(error) => {
                        this.kernels.error = Some(error.clone());
                        if report {
                            this.kernel_error(error, cx);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
    fn kernel_error(&self, error: String, cx: &mut Context<Self>) {
        self.toasts.update(cx, |host, cx| {
            host.push(
                crate::toast::Toast::new("Kernel operation failed")
                    .description(error)
                    .kind(crate::toast::ToastKind::Error),
                cx,
            );
        });
    }
    pub(super) fn confirm_kernel_shutdown(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.kernels.shutting_down.contains(&id) || window.has_active_prompt() {
            return;
        }
        let paths = self
            .kernels
            .sessions
            .iter()
            .filter(|s| s.kernel.id == id)
            .map(|s| s.path.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let detail = format!(
            "All variables in this kernel will be lost and running code will stop. Notebook files and saved outputs will be kept.{}",
            if paths.is_empty() {
                String::new()
            } else {
                format!(" Affected notebooks: {paths}.")
            }
        );
        let response = window.prompt(
            gpui::PromptLevel::Warning,
            "Shut down kernel?",
            Some(&detail),
            &["Cancel", "Shut Down"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if response.await == Ok(1) {
                this.update(cx, |this, cx| this.shutdown_kernel(id, false, cx))
                    .ok();
            }
        })
        .detach();
    }
    fn shutdown_kernel(&mut self, id: String, unused_only: bool, cx: &mut Context<Self>) {
        if !self.kernels.shutting_down.insert(id.clone()) {
            return;
        }
        let notebooks: Vec<_> = self
            .documents
            .iter()
            .filter_map(|doc| match &doc.content {
                Content::Notebook(view) if view.read(cx).kernel_id().as_deref() == Some(&id) => {
                    Some(view.clone())
                }
                _ => None,
            })
            .collect();
        let mut prepared: Vec<Entity<Notebook>> = vec![];
        for view in notebooks {
            if !view.update(cx, |view, cx| view.begin_kernel_shutdown(cx)) {
                for view in prepared {
                    view.update(cx, |view, cx| view.finish_kernel_shutdown(false, cx));
                }
                self.kernels.shutting_down.remove(&id);
                self.kernel_error(
                    "Wait for the current kernel operation to finish, then try again.".into(),
                    cx,
                );
                return;
            }
            prepared.push(view);
        }
        let config = self.runtime.config.clone();
        let kernel_id = id.clone();
        let task = self.runtime.handle.spawn(async move {
            let client = JupyterRestClient::new(config?).map_err(|e| e.to_string())?;
            if unused_only {
                match client.get_kernel(&kernel_id).await {
                    Ok(kernel) if !is_unused(&kernel) => return Ok(false),
                    Ok(_) => {}
                    Err(ganymede_core::jupyter::Error::Api { status, .. })
                        if status.as_u16() == 404 =>
                    {
                        return Ok(true);
                    }
                    Err(error) => return Err(error.to_string()),
                }
            }
            match client.shutdown_kernel(&kernel_id).await {
                Ok(()) => Ok(true),
                Err(ganymede_core::jupyter::Error::Api { status, .. })
                    if status.as_u16() == 404 =>
                {
                    Ok(true)
                }
                Err(error) => Err(error.to_string()),
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Could not shut down kernel".into()));
            this.update(cx, |this, cx| {
                this.kernels.shutting_down.remove(&id);
                for view in prepared {
                    view.update(cx, |view, cx| {
                        view.finish_kernel_shutdown(matches!(result, Ok(true)), cx)
                    });
                }
                if let Err(error) = result {
                    this.kernel_error(error, cx);
                } else if result == Ok(true) {
                    this.kernels.items.retain(|kernel| kernel.id != id);
                }
                this.refresh_kernels(false, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
    pub(super) fn kernel_toolbar(
        &mut self,
        action: KernelAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            KernelAction::Refresh => self.refresh_kernels(true, cx),
            KernelAction::ToggleView => {
                self.kernels.list_view = !self.kernels.list_view;
                cx.notify();
            }
            KernelAction::Collapse => {
                self.kernels.collapsed.extend(
                    self.kernels
                        .items
                        .iter()
                        .flat_map(|k| [group_key(&k.name), k.id.clone()]),
                );
                cx.notify();
            }
            KernelAction::Unused | KernelAction::All => {
                if window.has_active_prompt() {
                    return;
                }
                let unused = matches!(action, KernelAction::Unused);
                let ids = self
                    .kernels
                    .items
                    .iter()
                    .filter(|k| {
                        (!unused || is_unused(k)) && !self.kernels.shutting_down.contains(&k.id)
                    })
                    .map(|k| k.id.clone())
                    .collect::<Vec<_>>();
                if ids.is_empty() {
                    return;
                }
                let detail = format!(
                    "{} kernel(s) will be shut down. All variables in these kernels will be lost and running code will stop. Notebook files and saved outputs will be kept.{}",
                    ids.len(),
                    if unused {
                        " Only kernels with no connected clients will be shut down."
                    } else {
                        " This also affects other clients connected to these kernels."
                    }
                );
                let response = window.prompt(
                    gpui::PromptLevel::Warning,
                    if unused {
                        "Shut down unused kernels?"
                    } else {
                        "Shut down all kernels?"
                    },
                    Some(&detail),
                    &["Cancel", "Shut Down"],
                    cx,
                );
                cx.spawn(async move |this, cx| {
                    if response.await == Ok(1) {
                        this.update(cx, |this, cx| {
                            for id in ids {
                                this.shutdown_kernel(id, unused, cx);
                            }
                        })
                        .ok();
                    }
                })
                .detach();
            }
        }
    }

    fn kernel_tree_toggle(workspace: &Entity<Self>, key: String, cx: &App) -> impl IntoElement {
        let collapsed = workspace.read(cx).kernels.collapsed.contains(&key);
        let weak = workspace.downgrade();
        let keyboard = weak.clone();
        let keyboard_key = key.clone();
        icon_button(
            SharedString::from(format!("toggle-{key}")),
            if collapsed { "Expand" } else { "Collapse" },
            if collapsed {
                Icon::CaretRight
            } else {
                Icon::CaretDown
            },
            cx,
        )
        .on_click(move |_, _, cx| {
            weak.update(cx, |this, cx| {
                toggle(&mut this.kernels.collapsed, key.clone());
                cx.notify();
            })
            .ok();
        })
        .on_key_down(move |event: &gpui::KeyDownEvent, _, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                keyboard
                    .update(cx, |this, cx| {
                        toggle(&mut this.kernels.collapsed, keyboard_key.clone());
                        cx.notify();
                    })
                    .ok();
                cx.stop_propagation();
            }
        })
    }

    pub fn kernels(workspace: &Entity<Self>, cx: &App) -> impl IntoElement {
        let this = workspace.read(cx);
        let t = cx.theme();
        let surface = t.sidebar_content;
        let tree = !this.kernels.list_view;
        let mut actions = vec![(
            KernelAction::Unused,
            "Shut down unused kernels",
            Icon::Broom,
            this.kernels.items.iter().any(is_unused),
        )];
        if tree {
            actions.push((
                KernelAction::Collapse,
                "Collapse all",
                Icon::Collapse,
                !this.kernels.items.is_empty(),
            ));
        }
        actions.extend([
            (
                KernelAction::ToggleView,
                if tree {
                    "Show list view"
                } else {
                    "Show tree view"
                },
                if tree { Icon::List } else { Icon::Tree },
                true,
            ),
            (
                KernelAction::All,
                "Shut down all kernels",
                Icon::Power,
                !this.kernels.items.is_empty(),
            ),
            (
                KernelAction::Refresh,
                "Refresh kernels",
                Icon::Restart,
                !this.kernels.loading,
            ),
        ]);
        let mut rows = Vec::new();
        let mut last_group = None;
        for kernel in &this.kernels.items {
            let group = group_key(&kernel.name);
            if tree && last_group != Some(&kernel.name) {
                last_group = Some(&kernel.name);
                rows.push(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(Self::kernel_tree_toggle(workspace, group.clone(), cx))
                        .child(Icon::Cpu.svg().size(px(14.)).text_color(surface.muted_fg))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(
                                    this.kernels
                                        .names
                                        .get(&kernel.name)
                                        .unwrap_or(&kernel.name)
                                        .clone(),
                                ),
                        )
                        .into_any_element(),
                );
            }
            if tree && this.kernels.collapsed.contains(&group) {
                continue;
            }
            let id = kernel.id.clone();
            let paths: Vec<_> = this
                .kernels
                .sessions
                .iter()
                .filter(|s| s.kernel.id == id)
                .map(|s| s.path.clone())
                .collect();
            let short_id = id.chars().take(8).collect::<String>();
            let label = if tree {
                paths
                    .first()
                    .and_then(|p| p.rsplit('/').next())
                    .unwrap_or(&short_id)
                    .to_owned()
            } else {
                this.kernels
                    .names
                    .get(&kernel.name)
                    .unwrap_or(&kernel.name)
                    .clone()
            };
            let weak = workspace.downgrade();
            let keyboard = weak.clone();
            let keyboard_id = id.clone();
            let stopping = this.kernels.shutting_down.contains(&id);
            let mut row = div()
                .id(SharedString::from(format!("kernel-{id}")))
                .mb_1()
                .when(tree, |v| v.ml_3())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .when(tree, |v| {
                            v.child(Self::kernel_tree_toggle(workspace, id.clone(), cx))
                        })
                        .child(
                            div()
                                .size(px(8.))
                                .flex_shrink_0()
                                .rounded_full()
                                .border_1()
                                .border_color(surface.fg)
                                .when(kernel.execution_state.as_deref() == Some("busy"), |v| {
                                    v.bg(surface.fg)
                                }),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(label))
                        .child(
                            icon_button(
                                SharedString::from(format!("shutdown-{id}")),
                                "Shut down kernel",
                                Icon::Stop,
                                cx,
                            )
                            .when(stopping, |v| v.opacity(0.4))
                            .on_click(move |_, window, cx| {
                                weak.update(cx, |this, cx| {
                                    this.confirm_kernel_shutdown(id.clone(), window, cx)
                                })
                                .ok();
                            })
                            .on_key_down(
                                move |event: &gpui::KeyDownEvent, window, cx| {
                                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                        keyboard
                                            .update(cx, |this, cx| {
                                                this.confirm_kernel_shutdown(
                                                    keyboard_id.clone(),
                                                    window,
                                                    cx,
                                                )
                                            })
                                            .ok();
                                        cx.stop_propagation();
                                    }
                                },
                            ),
                        ),
                )
                .child(
                    div()
                        .when(tree, |v| v.ml_8())
                        .text_size(t.font.xs)
                        .text_color(surface.muted_fg)
                        .child(if stopping {
                            "Shutting down…".into()
                        } else {
                            format!(
                                "{} · {short_id}",
                                kernel.execution_state.as_deref().unwrap_or("unknown")
                            )
                        }),
                );
            if !tree || !this.kernels.collapsed.contains(&kernel.id) {
                if paths.is_empty() {
                    row = row.child(
                        div()
                            .when(tree, |v| v.ml_8())
                            .text_size(t.font.xs)
                            .text_color(surface.muted_fg)
                            .child("No notebook session"),
                    );
                }
                for path in paths {
                    let weak = workspace.downgrade();
                    let open_path = path.clone();
                    row = row.child(
                        div()
                            .id(SharedString::from(format!("session-{}-{path}", kernel.id)))
                            .when(tree, |v| v.ml_8())
                            .py_1()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_size(t.font.xs)
                            .text_color(surface.muted_fg)
                            .rounded(t.radius.sm())
                            .when(path.ends_with(".ipynb"), |v| {
                                v.cursor_pointer()
                                    .hover(|s| s.bg(surface.accent).text_color(surface.accent_fg))
                                    .on_click(move |_, window, cx| {
                                        weak.update(cx, |this, cx| {
                                            this.open_notebook(Some(open_path.clone()), window, cx)
                                        })
                                        .ok();
                                    })
                            })
                            .child(
                                Icon::Notebook
                                    .svg()
                                    .size(px(13.))
                                    .flex_shrink_0()
                                    .text_color(surface.muted_fg),
                            )
                            .child(div().min_w_0().truncate().child(path)),
                    );
                }
            }
            rows.push(row.into_any_element());
        }
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .children(actions.into_iter().map(|(action, label, icon, enabled)| {
                        let weak = workspace.downgrade();
                        let keyboard = weak.clone();
                        icon_button(label, label, icon, cx)
                            .when(!enabled, |v| v.opacity(0.4))
                            .on_click(move |_, window, cx| {
                                if enabled {
                                    weak.update(cx, |this, cx| {
                                        this.kernel_toolbar(action, window, cx)
                                    })
                                    .ok();
                                }
                            })
                            .on_key_down(move |event: &gpui::KeyDownEvent, window, cx| {
                                if enabled
                                    && matches!(event.keystroke.key.as_str(), "enter" | "space")
                                {
                                    keyboard
                                        .update(cx, |this, cx| {
                                            this.kernel_toolbar(action, window, cx)
                                        })
                                        .ok();
                                    cx.stop_propagation();
                                }
                            })
                    })),
            )
            .when(this.kernels.error.is_some(), |v| {
                v.child(
                    div()
                        .text_size(t.font.xs)
                        .text_color(surface.muted_fg)
                        .child("Could not refresh kernels. Try refreshing again."),
                )
            })
            .when(
                this.kernels.items.is_empty() && this.kernels.error.is_none(),
                |v| {
                    v.child(
                        div()
                            .text_size(t.font.xs)
                            .text_color(surface.muted_fg)
                            .child(if this.kernels.loaded {
                                "No running kernels"
                            } else {
                                "Loading kernels…"
                            }),
                    )
                },
            )
            .child(
                div()
                    .id("running-kernels")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows),
            )
    }
}

#[derive(Clone, Copy)]
pub(super) enum KernelAction {
    Unused,
    Collapse,
    ToggleView,
    All,
    Refresh,
}

// JupyterLab uses the server's client connection count, not execution state or sessions.
fn is_unused(kernel: &Kernel) -> bool {
    kernel.connections == Some(0)
}
fn group_key(name: &str) -> String {
    format!("type:{name}")
}
fn toggle(collapsed: &mut HashSet<String>, key: String) {
    if !collapsed.remove(&key) {
        collapsed.insert(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unused_requires_an_explicit_zero_connection_count() {
        let mut kernel: Kernel = serde_json::from_value(
            serde_json::json!({"id":"one", "name":"python3", "execution_state":"idle"}),
        )
        .unwrap();
        assert!(!is_unused(&kernel));
        kernel.connections = Some(1);
        assert!(!is_unused(&kernel));
        kernel.connections = Some(0);
        assert!(is_unused(&kernel));
        kernel.execution_state = Some("busy".into());
        assert!(is_unused(&kernel));
    }
}
