use super::*;
use crate::theme::ActiveTheme;
use gpui::{FocusHandle, KeyDownEvent, point, px};

pub(super) struct Picker {
    pub open: bool,
    pub focus: FocusHandle,
    loading: bool,
    choices: Vec<(String, String)>,
    index: usize,
}
impl Picker {
    pub fn new(cx: &mut Context<Notebook>) -> Self {
        Self {
            open: false,
            focus: cx.focus_handle(),
            loading: false,
            choices: vec![],
            index: 0,
        }
    }
}
impl Notebook {
    pub(super) fn toggle_kernel_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.connecting || self.control_pending || self.file_busy {
            return;
        }
        self.kernel_menu.open = !self.kernel_menu.open;
        self.kernel_menu.focus.focus(window, cx);
        if self.kernel_menu.open && !self.kernel_menu.loading {
            self.kernel_menu.loading = true;
            let task = self.session.kernel_specs();
            cx.spawn(async move |this, cx| {
                let result = task
                    .await
                    .unwrap_or_else(|_| Err("Could not load installed kernels".into()));
                this.update(cx, |this, cx| {
                    this.kernel_menu.loading = false;
                    match result {
                        Ok(specs) => {
                            this.kernel_menu.choices = specs
                                .kernelspecs
                                .into_iter()
                                .map(|(name, entry)| (name, entry.spec.display_name))
                                .collect();
                            this.kernel_menu.index = this
                                .kernel_menu
                                .choices
                                .iter()
                                .position(|(name, _)| *name == this.session.kernel_name())
                                .unwrap_or(0);
                        }
                        Err(error) => {
                            this.kernel_menu.open = false;
                            this.execution_error = Some(error);
                            this.report_kernel_error(cx);
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }
    pub(super) fn kernel_menu_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = self.kernel_menu.choices.len();
        match event.keystroke.key.as_str() {
            "enter" | "space" | "down" | "up" if !self.kernel_menu.open => {
                self.toggle_kernel_menu(window, cx)
            }
            "escape" | "tab" => {
                self.kernel_menu.open = false;
                cx.notify();
                if event.keystroke.key == "tab" {
                    return;
                }
            }
            "down" if count > 0 => self.kernel_menu.index = (self.kernel_menu.index + 1) % count,
            "up" if count > 0 => {
                self.kernel_menu.index = (self.kernel_menu.index + count - 1) % count
            }
            "enter" | "space" if count > 0 && !self.kernel_menu.loading => {
                self.choose_kernel(self.kernel_menu.index, window, cx)
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
    fn choose_kernel(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.connecting || self.control_pending || self.file_busy || window.has_active_prompt() {
            return;
        }
        let Some((name, label)) = self.kernel_menu.choices.get(index).cloned() else {
            return;
        };
        self.kernel_menu.open = false;
        cx.notify();
        if name == self.session.kernel_name()
            && self.session.activity() != crate::execution::KernelActivity::Disconnected
        {
            return;
        }
        let response = window.prompt(gpui::PromptLevel::Warning, &format!("Switch to {label}?"),
            Some("The current kernel will stop and its variables will be lost. Running and queued cells will be stopped. Other notebooks using that kernel are also affected. Your notebook and outputs will be kept."),
            &["Cancel", "Switch Kernel"], cx);
        cx.spawn(async move |this, cx| {
            if response.await == Ok(1) {
                this.update(cx, |this, cx| this.switch_kernel(name, cx))
                    .ok();
            }
        })
        .detach();
    }
    fn switch_kernel(&mut self, name: String, cx: &mut Context<Self>) {
        if self.connecting || self.control_pending || self.file_busy {
            return;
        }
        self.runs.pending.clear();
        self.restart_run_all = false;
        for cell in &self.cells {
            cell.update(cx, |cell, cx| cell.set_queued(false, cx));
        }
        self.control_pending = true;
        self.control_label = "Switching…";
        self.execution_error = None;
        let task = self.session.change_kernel(name);
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Could not switch kernel".into()));
            this.update(cx, |this, cx| {
                this.control_pending = false;
                this.execution_error = result.err();
                this.report_kernel_error(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
    pub fn cancel_server_queue(&mut self, cx: &mut Context<Self>) {
        self.runs.pending.clear();
        self.restart_run_all = false;
        self.session.cancel_pending();
        for cell in &self.cells {
            cell.update(cx, |cell, cx| cell.set_queued(false, cx));
        }
        cx.notify();
    }

    pub fn recover_server_session(&mut self, cx: &mut Context<Self>) {
        if self.connecting || self.control_pending || self.kernel_id().is_none() {
            return;
        }
        self.cancel_server_queue(cx);
        self.connecting = true;
        let task = self.session.reconnect_existing();
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Could not reconnect to the kernel".into()));
            this.update(cx, |this, cx| {
                this.connecting = false;
                this.execution_error = result.err();
                this.report_kernel_error(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub fn begin_kernel_shutdown(&mut self, cx: &mut Context<Self>) -> bool {
        if self.control_pending || self.connecting {
            return false;
        }
        self.control_pending = true;
        self.control_label = "Shutting down…";
        self.runs.pending.clear();
        self.restart_run_all = false;
        self.session.cancel_pending();
        for cell in &self.cells {
            cell.update(cx, |cell, cx| cell.set_queued(false, cx));
        }
        cx.notify();
        true
    }
    pub fn finish_kernel_shutdown(&mut self, succeeded: bool, cx: &mut Context<Self>) {
        if !succeeded {
            self.control_pending = false;
            cx.notify();
            return;
        }
        let task = self.session.detach();
        cx.spawn(async move |this, cx| {
            let _ = task.await;
            this.update(cx, |this, cx| {
                this.control_pending = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
    pub fn kernel_id(&self) -> Option<String> {
        self.session
            .controller()
            .snapshot()
            .session
            .as_ref()
            .map(|s| s.kernel.id.clone())
    }
    pub(super) fn render_kernel_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let current = self.session.kernel_name();
        gpui::deferred(
            gpui::anchored()
                .offset(point(px(0.), px(4.)))
                .snap_to_window_with_margin(px(8.))
                .child(
                    div()
                        .id("kernel-menu")
                        .w(px(240.))
                        .max_h(px(320.))
                        .overflow_y_scroll()
                        .p_1()
                        .flex()
                        .flex_col()
                        .bg(t.popover.bg)
                        .text_color(t.popover.fg)
                        .text_size(t.font.xs)
                        .border_1()
                        .border_color(t.popover.border)
                        .rounded(t.radius.lg())
                        .shadow_lg()
                        .occlude()
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.kernel_menu.open = false;
                            cx.notify();
                        }))
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .text_color(t.popover.muted_fg)
                                .child("Installed kernels"),
                        )
                        .when(self.kernel_menu.loading, |menu| {
                            menu.child(div().px_2().py_1().child("Loading…"))
                        })
                        .when(
                            !self.kernel_menu.loading && self.kernel_menu.choices.is_empty(),
                            |menu| menu.child(div().px_2().py_1().child("No kernels installed")),
                        )
                        .when(!self.kernel_menu.loading, |menu| {
                            menu.children(self.kernel_menu.choices.iter().enumerate().map(
                                |(index, (name, label))| {
                                    div()
                                        .id(("kernel-choice", index))
                                        .px_2()
                                        .py_1()
                                        .rounded(t.radius.sm())
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .cursor_pointer()
                                        .when(index == self.kernel_menu.index, |s| {
                                            s.bg(t.popover.accent).text_color(t.popover.accent_fg)
                                        })
                                        .on_hover(cx.listener(move |this, hover, _, cx| {
                                            if *hover {
                                                this.kernel_menu.index = index;
                                                cx.notify();
                                            }
                                        }))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.choose_kernel(index, window, cx);
                                        }))
                                        .child(div().w(px(12.)).child(if *name == current {
                                            "✓"
                                        } else {
                                            ""
                                        }))
                                        .child(div().min_w_0().truncate().child(label.clone()))
                                },
                            ))
                        }),
                ),
        )
    }
}
