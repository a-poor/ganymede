//! Server-backed notebook editor.
use std::collections::VecDeque;

use gpui::{Context, Entity, Focusable, ScrollHandle, Window, div, prelude::*};

use ganymede_core::notebook::CellKind;

use crate::code_cell::{CellEvent, CodeCell};

#[derive(Default)]
struct SubmissionCounter(u64);

impl SubmissionCounter {
    fn run(&mut self, text: &str) -> Option<u64> {
        if text.trim().is_empty() {
            return None;
        }
        self.0 += 1;
        Some(self.0)
    }
}

/// FIFO requests retain the count and source captured when submitted, even if edited later.
#[derive(Default)]
struct RunQueue {
    active: Option<usize>,
    pending: VecDeque<(usize, u64, String)>,
}

impl RunQueue {
    fn start_next(&mut self) -> Option<(usize, u64, String)> {
        if self.active.is_some() {
            return None;
        }
        let run = self.pending.pop_front()?;
        self.active = Some(run.0);
        Some(run)
    }

    fn remap(&mut self, map: impl Fn(usize) -> usize) {
        self.active = self.active.map(&map);
        for (index, _, _) in &mut self.pending {
            *index = map(*index);
        }
    }

    fn remove(&mut self, index: usize) {
        self.finish(index);
        self.pending.retain(|run| run.0 != index);
        self.remap(|i| if i > index { i - 1 } else { i });
    }

    fn finish(&mut self, index: usize) {
        if self.active == Some(index) {
            self.active = None;
        }
    }

    fn is_queued(&self, index: usize) -> bool {
        self.pending.iter().any(|run| run.0 == index)
    }
}

pub struct Notebook {
    toasts: Entity<crate::toast::ToastHost>,
    cells: Vec<Entity<CodeCell>>,
    saving: bool,
    autosave: crate::autosave::AutosaveClock,
    autosave_suspended: bool,
    save_error: Option<String>,
    session: crate::execution::Session,
    control_pending: bool,
    connecting: bool,
    execution_error: Option<String>,
    selected: usize,
    largest_count: u64,
    type_menu_open: bool,
    type_menu_index: usize,
    type_focus: gpui::FocusHandle,
    counter: SubmissionCounter,
    runs: RunQueue,
    scroll: ScrollHandle,
}

impl Notebook {
    pub fn new(
        session: crate::execution::Session,
        toasts: Entity<crate::toast::ToastHost>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut notebook = Self {
            toasts,
            cells: vec![],
            session,
            saving: false,
            autosave: Default::default(),
            autosave_suspended: false,
            save_error: None,
            control_pending: false,
            connecting: true,
            execution_error: None,
            selected: 0,
            largest_count: 0,
            type_menu_open: false,
            type_menu_index: 0,
            type_focus: cx.focus_handle(),
            counter: SubmissionCounter::default(),
            runs: RunQueue::default(),
            scroll: ScrollHandle::new(),
        };
        if let Some(mut snapshots) = notebook.session.watch() {
            cx.spawn(async move |this, cx| {
                while snapshots.changed().await.is_ok() {
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        cx.observe_global::<crate::autosave::AutosavePreference>(|this, cx| {
            this.autosave.resume();
            cx.notify();
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        let document = this.session.document();
                        let enabled = cx.global::<crate::autosave::AutosavePreference>().0
                            && !this.autosave_suspended;
                        if this.autosave.due(
                            document.revision(),
                            document.is_dirty(),
                            enabled,
                            this.saving,
                            std::time::Instant::now(),
                        ) {
                            this.save(cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let connection = notebook.session.connect();
        cx.spawn(async move |this, cx| {
            let result = connection.await;
            this.update(cx, |this, cx| {
                this.connecting = false;
                this.execution_error = match result {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(error),
                    Err(_) => Some("Could not connect to the kernel".into()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
        let document = notebook.session.document();
        for loaded in document.cells() {
            notebook.append_cell(window, cx);
            let cell = notebook.cells.last().unwrap();
            notebook
                .session
                .bind(cell.entity_id().as_u64(), loaded.id().clone());
            cell.update(cx, |cell, cx| cell.load(loaded, cx));
        }
        if notebook.cells.is_empty() {
            notebook.append_cell(window, cx);
        }
        notebook.select(0, cx);
        notebook.cells[0]
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
        notebook
    }

    fn append_cell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cell = cx.new(|cx| CodeCell::new(window, cx));
        cx.subscribe_in(&cell, window, |this, cell, event, window, cx| {
            let Some(index) = this.cells.iter().position(|item| item == cell) else {
                return;
            };
            match event {
                CellEvent::Changed => {
                    this.sync(cx);
                    cx.notify();
                }
                CellEvent::RunProgress => cx.notify(),
                CellEvent::Selected => this.select(index, cx),
                CellEvent::DeleteCell => {
                    if this.runs.active == Some(index) {
                        return;
                    }
                    // A running cell stays in place until the kernel finishes.
                    cell.update(cx, |cell, cx| cell.clear_execution(cx));
                    this.runs.remove(index);
                    this.cells.remove(index);
                    if this.cells.is_empty() {
                        this.append_cell(window, cx);
                    }
                    let target = index.min(this.cells.len() - 1);
                    this.select(target, cx);
                    this.cells[target]
                        .read(cx)
                        .focus_handle(cx)
                        .focus(window, cx);
                    this.start_next_run(cx);
                    cx.notify();
                }
                CellEvent::Duplicate | CellEvent::InsertAbove | CellEvent::InsertBelow => {
                    let source = if matches!(event, CellEvent::Duplicate) {
                        cell.read(cx).text().to_owned()
                    } else {
                        String::new()
                    };
                    let kind = if matches!(event, CellEvent::Duplicate) {
                        cell.read(cx).kind()
                    } else {
                        CellKind::Code
                    };
                    let target = if matches!(event, CellEvent::InsertAbove) {
                        index
                    } else {
                        index + 1
                    };
                    this.append_cell(window, cx);
                    let added = this.cells.pop().unwrap();
                    added.update(cx, |cell, cx| {
                        cell.set_text(source, cx);
                        cell.set_kind(kind, cx);
                    });
                    this.runs.remap(|i| if i >= target { i + 1 } else { i });
                    this.cells.insert(target, added.clone());
                    this.select(target, cx);
                    added.read(cx).focus_handle(cx).focus(window, cx);
                    this.scroll.scroll_to_item(target);
                    cx.notify();
                }
                CellEvent::MoveUp | CellEvent::MoveDown => {
                    let target = if matches!(event, CellEvent::MoveUp) {
                        index.saturating_sub(1)
                    } else {
                        (index + 1).min(this.cells.len() - 1)
                    };
                    this.cells.swap(index, target);
                    this.runs.remap(|i| {
                        if i == index {
                            target
                        } else if i == target {
                            index
                        } else {
                            i
                        }
                    });
                    this.select(target, cx);
                    this.scroll.scroll_to_item(target);
                    cx.notify();
                }
                CellEvent::RunFailed => {
                    this.runs.pending.clear();
                    this.runs.finish(index);
                    this.start_next_run(cx);
                }
                CellEvent::RunFinished => {
                    this.runs.finish(index);
                    this.start_next_run(cx);
                }
                CellEvent::RunAndAdvance => {
                    if this.control_pending {
                        return;
                    }
                    let source = cell.read(cx).text().to_owned();
                    let count = (cell.read(cx).kind() == CellKind::Code)
                        .then(|| this.counter.run(&source))
                        .flatten();
                    if let Some(count) = count {
                        this.runs.pending.push_back((index, count, source));
                        this.start_next_run(cx);
                    } else if this.runs.active != Some(index) && !this.runs.is_queued(index) {
                        cell.update(cx, |cell, cx| cell.clear_execution(cx));
                        if cell.read(cx).kind() == CellKind::Code {
                            this.save_error =
                                this.session.clear_outputs(cell.entity_id().as_u64()).err();
                        }
                    }
                    if index + 1 == this.cells.len() {
                        this.append_cell(window, cx);
                    }
                    this.select(index + 1, cx);
                    this.cells[index + 1]
                        .read(cx)
                        .focus_handle(cx)
                        .focus(window, cx);
                    this.scroll.scroll_to_item(index + 1);
                    cx.notify();
                }
            }
        })
        .detach();
        self.cells.push(cell);
    }

    fn editors(&self, cx: &gpui::App) -> Vec<(u64, CellKind, String)> {
        self.cells
            .iter()
            .map(|cell| {
                (
                    cell.entity_id().as_u64(),
                    cell.read(cx).kind(),
                    cell.read(cx).text().to_owned(),
                )
            })
            .collect()
    }
    fn sync(&mut self, cx: &gpui::App) {
        if let Err(error) = self.session.sync(&self.editors(cx)) {
            self.save_error = Some(error);
        }
    }
    pub fn is_dirty(&self, cx: &gpui::App) -> bool {
        let document = self.session.document();
        document.is_dirty()
            || self.cells.len() != document.cells().len()
            || self
                .cells
                .iter()
                .zip(document.cells())
                .any(|(editor, cell)| {
                    editor.read(cx).text() != cell.source() || editor.read(cx).kind() != cell.kind()
                })
    }
    pub fn save(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        self.autosave.resume();
        self.save_error = None;
        self.sync(cx);
        if self.save_error.is_some() {
            self.autosave.failed();
            self.report_save_error(cx);
            cx.notify();
            return;
        }
        self.saving = true;
        let task = self.session.save();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.saving = false;
                this.save_error = match result {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(error),
                    Err(_) => Some("Could not save notebook".into()),
                };
                if this.save_error.is_some() {
                    this.autosave.failed();
                }
                this.report_save_error(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
    pub fn suspend_autosave(&mut self, suspended: bool) {
        self.autosave_suspended = suspended;
    }

    pub fn save_failed(&self) -> bool {
        self.save_error.is_some()
    }

    pub fn is_saving(&self) -> bool {
        self.saving
    }

    fn report_save_error(&self, cx: &mut Context<Self>) {
        if let Some(error) = &self.save_error {
            let toast = crate::toast::Toast::new("Could not save notebook")
                .description(error.clone())
                .kind(crate::toast::ToastKind::Error);
            self.toasts.update(cx, |host, cx| {
                host.push(toast, cx);
            });
        }
    }
    pub fn running_spinner(&self, cx: &gpui::App) -> Option<char> {
        self.runs
            .active
            .and_then(|index| self.cells.get(index))
            .and_then(|cell| cell.read(cx).running_spinner())
    }

    pub fn focus_selected(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(cell) = self.cells.get(self.selected) {
            cell.read(cx).focus_handle(cx).focus(window, cx);
        }
    }

    fn toolbar_action(
        &mut self,
        action: ToolbarAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            ToolbarAction::Add | ToolbarAction::Paste => {
                let text = if matches!(action, ToolbarAction::Paste) {
                    cx.read_from_clipboard()
                        .and_then(|item| item.text())
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                let target = self.selected + 1;
                self.append_cell(window, cx);
                let added = self.cells.pop().unwrap();
                added.update(cx, |cell, cx| cell.set_text(text, cx));
                self.runs.remap(|i| if i >= target { i + 1 } else { i });
                self.cells.insert(target, added);
                self.select(target, cx);
                self.scroll.scroll_to_item(target);
            }
            ToolbarAction::Copy | ToolbarAction::Cut => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                    self.cells[self.selected].read(cx).text().to_owned(),
                ));
                if matches!(action, ToolbarAction::Cut) {
                    if self.runs.active == Some(self.selected) {
                        return;
                    }
                    let index = self.selected;
                    self.cells[index].update(cx, |cell, cx| cell.clear_execution(cx));
                    self.runs.remove(index);
                    self.cells.remove(index);
                    if self.cells.is_empty() {
                        self.append_cell(window, cx);
                    }
                    self.select(index.min(self.cells.len() - 1), cx);
                    self.start_next_run(cx);
                }
            }
            ToolbarAction::Run | ToolbarAction::RunAll => {
                if self.control_pending {
                    return;
                }
                let indices = if matches!(action, ToolbarAction::RunAll) {
                    0..self.cells.len()
                } else {
                    self.selected..self.selected + 1
                };
                for index in indices {
                    if self.cells[index].read(cx).kind() != CellKind::Code {
                        continue;
                    }
                    let source = self.cells[index].read(cx).text().to_owned();
                    if let Some(count) = self.counter.run(&source) {
                        self.runs.pending.push_back((index, count, source));
                    }
                }
                self.start_next_run(cx);
            }
            ToolbarAction::Stop | ToolbarAction::Restart => {
                if self.control_pending {
                    return;
                }
                self.runs.pending.clear();
                for cell in &self.cells {
                    cell.update(cx, |cell, cx| cell.set_queued(false, cx));
                }
                self.control_pending = true;
                self.execution_error = None;
                let task = self
                    .session
                    .control(matches!(action, ToolbarAction::Restart));
                cx.spawn(async move |this, cx| {
                    let result = task.await;
                    this.update(cx, |this, cx| {
                        this.control_pending = false;
                        this.execution_error = match result {
                            Ok(Ok(())) => None,
                            Ok(Err(error)) => Some(error),
                            Err(_) => Some("Kernel operation failed".into()),
                        };
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
        }

        self.sync(cx);
        self.focus_selected(window, cx);
        cx.notify();
    }

    fn choose_cell_type(&mut self, kind: CellKind, window: &mut Window, cx: &mut Context<Self>) {
        if self.runs.active == Some(self.selected) {
            return;
        }
        if self.cells[self.selected].read(cx).kind() != kind {
            self.runs.finish(self.selected);
            self.runs.pending.retain(|run| run.0 != self.selected);
            self.cells[self.selected].update(cx, |cell, cx| cell.set_kind(kind, cx));
            self.start_next_run(cx);
        }
        self.type_menu_open = false;
        self.focus_selected(window, cx);
        cx.notify();
    }

    fn cell_type_control(&self, cx: &Context<Self>) -> impl IntoElement {
        use crate::{icon::Icon, theme::ActiveTheme};
        use gpui::{point, px};
        let t = cx.theme();
        let kinds = [CellKind::Code, CellKind::Markdown, CellKind::Raw];
        let labels = ["Code", "Markdown", "Raw"];
        let selected = kinds
            .iter()
            .position(|kind| *kind == self.cells[self.selected].read(cx).kind())
            .unwrap();
        div()
            .id("cell-type-control")
            .relative()
            .child(
                div()
                    .id("cell-type")
                    .track_focus(&self.type_focus)
                    .tab_index(0)
                    .h(px(26.))
                    .w(px(100.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(t.font.xs)
                    .text_color(t.app.fg)
                    .rounded(t.radius.sm())
                    .cursor_pointer()
                    .hover(|s| s.bg(t.app.accent).text_color(t.app.accent_fg))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.type_menu_open = true;
                        this.type_menu_index = selected;
                        this.type_focus.focus(window, cx);
                        cx.notify();
                    }))
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            match event.keystroke.key.as_str() {
                                "enter" | "space" if this.type_menu_open => {
                                    this.choose_cell_type(kinds[this.type_menu_index], window, cx);
                                }
                                "enter" | "space" | "down" | "up" if !this.type_menu_open => {
                                    this.type_menu_open = true;
                                    this.type_menu_index = selected;
                                }
                                "down" => this.type_menu_index = (this.type_menu_index + 1) % 3,
                                "up" => this.type_menu_index = (this.type_menu_index + 2) % 3,
                                "escape" => this.type_menu_open = false,
                                "tab" => {
                                    this.type_menu_open = false;
                                    cx.notify();
                                    return;
                                }
                                _ => return,
                            }
                            cx.stop_propagation();
                            cx.notify();
                        },
                    ))
                    .child(labels[selected])
                    .child(div().flex_1())
                    .child(
                        Icon::CaretDown
                            .svg()
                            .size(px(12.))
                            .text_color(t.app.muted_fg),
                    ),
            )
            .when(self.type_menu_open, |control| {
                control.child(gpui::deferred(
                    gpui::anchored()
                        .offset(point(px(0.), px(4.)))
                        .snap_to_window_with_margin(px(8.))
                        .child(
                            div()
                                .id("cell-type-menu")
                                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                    this.type_menu_open = false;
                                    cx.notify();
                                }))
                                .w(px(132.))
                                .p_1()
                                .flex()
                                .flex_col()
                                .bg(t.popover.bg)
                                .text_color(t.popover.fg)
                                .border_1()
                                .border_color(t.popover.border)
                                .rounded(t.radius.lg())
                                .shadow_lg()
                                .occlude()
                                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation()
                                })
                                .children(kinds.into_iter().enumerate().map(|(index, kind)| {
                                    div()
                                        .id(labels[index])
                                        .h(px(26.))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .text_size(t.font.xs)
                                        .rounded(t.radius.sm())
                                        .cursor_pointer()
                                        .when(index == self.type_menu_index, |s| {
                                            s.bg(t.popover.accent).text_color(t.popover.accent_fg)
                                        })
                                        .hover(|s| {
                                            s.bg(t.popover.accent).text_color(t.popover.accent_fg)
                                        })
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.choose_cell_type(kind, window, cx);
                                        }))
                                        .child(div().w(px(12.)).child(if index == selected {
                                            "✓"
                                        } else {
                                            ""
                                        }))
                                        .child(labels[index])
                                })),
                        ),
                ))
            })
    }

    fn toolbar(&self, cx: &Context<Self>) -> gpui::Div {
        use crate::execution::KernelActivity;
        use crate::{icon::Icon, theme::ActiveTheme, workspace::icon_button};
        let t = cx.theme();
        let activity = self.session.activity();
        let muted =
            self.connecting || self.control_pending || activity == KernelActivity::Disconnected;
        div()
            .h(gpui::px(36.))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap(gpui::px(2.))
            .border_b_1()
            .border_color(t.app.border)
            // The containing panel paints the background with rounded corners.
            // GPUI's overflow mask is rectangular, so an opaque toolbar would
            // paint over those corners.
            .text_color(t.app.fg)
            .child(
                icon_button("save-notebook", "Save notebook (⌘S)", Icon::Save, cx)
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx)))
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            this.save(cx);
                        }
                    })),
            )
            .child(
                div()
                    .h(gpui::px(16.))
                    .w(gpui::px(1.))
                    .mx_1()
                    .bg(t.app.border),
            )
            .children(
                [
                    (ToolbarAction::Add, "Insert cell", Icon::Plus),
                    (ToolbarAction::Cut, "Cut cell", Icon::Scissors),
                    (ToolbarAction::Copy, "Copy cell", Icon::Copy),
                    (ToolbarAction::Paste, "Paste cell", Icon::Clipboard),
                    (ToolbarAction::Run, "Run cell", Icon::Play),
                    (ToolbarAction::Stop, "Interrupt execution", Icon::Stop),
                    (ToolbarAction::Restart, "Restart kernel", Icon::Restart),
                    (ToolbarAction::RunAll, "Run all cells", Icon::RunAll),
                ]
                .into_iter()
                .map(|(action, label, icon)| {
                    icon_button(label, label, icon, cx)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.toolbar_action(action, window, cx)
                        }))
                        .on_key_down(cx.listener(
                            move |this, event: &gpui::KeyDownEvent, window, cx| {
                                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                    this.toolbar_action(action, window, cx);
                                    cx.stop_propagation();
                                }
                            },
                        ))
                }),
            )
            .child(
                div()
                    .h(gpui::px(16.))
                    .w(gpui::px(1.))
                    .mx_1()
                    .bg(t.app.border),
            )
            .child(self.cell_type_control(cx))
            .child(div().flex_1())
            .child(
                div()
                    .text_size(t.font.xs)
                    .text_color(t.app.muted_fg)
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(gpui::px(6.))
                    .child(if self.connecting {
                        "Connecting…".to_owned()
                    } else if self.control_pending {
                        "Updating kernel…".to_owned()
                    } else if self.runs.active.is_some()
                        && self.session.status().contains("not connected")
                    {
                        "Connecting…".to_owned()
                    } else {
                        self.session.status()
                    })
                    .child(
                        div()
                            .size(gpui::px(10.))
                            .flex_shrink_0()
                            .rounded_full()
                            .border_1()
                            .border_color(if muted { t.app.muted_fg } else { t.app.fg })
                            .when(muted, |circle| circle.opacity(0.45))
                            .when(!muted && activity == KernelActivity::Busy, |circle| {
                                circle.bg(t.app.fg).text_color(t.app.bg)
                            }),
                    ),
            )
    }

    fn start_next_run(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        if let Some((index, _request, source)) = self.runs.start_next() {
            self.sync(cx);
            self.execution_error = None;
            let session = self.session.clone();
            self.cells[index].update(cx, |cell, cx| cell.run(session, source, cx));
        }
        for (index, cell) in self.cells.iter().enumerate() {
            cell.update(cx, |cell, cx| {
                cell.set_queued(self.runs.is_queued(index), cx)
            });
        }
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        self.sync(cx);
        self.selected = index;
        self.type_menu_open = false;
        cx.notify();
        for (position, cell) in self.cells.iter().enumerate() {
            cell.update(cx, |cell, cx| cell.set_current(position == index, cx));
        }
    }
}

#[derive(Clone, Copy)]
enum ToolbarAction {
    Add,
    Cut,
    Copy,
    Paste,
    Run,
    Stop,
    Restart,
    RunAll,
}

impl Render for Notebook {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.largest_count = self.largest_count.max(
            self.cells
                .iter()
                .map(|cell| cell.read(cx).max_execution_count())
                .max()
                .unwrap_or(0),
        );
        for cell in &self.cells {
            cell.update(cx, |cell, cx| cell.set_count_hint(self.largest_count, cx));
        }
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .on_action(cx.listener(|this, _: &crate::Save, _, cx| this.save(cx)))
            .child(self.toolbar(cx))
            .when_some(self.execution_error.as_ref(), |view, error| {
                use crate::theme::ActiveTheme;
                let t = cx.theme();
                view.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_size(t.font.xs)
                        .text_color(t.destructive)
                        .child(error.clone()),
                )
            })
            .child(
                div()
                    .id("notebook-cells")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .pl_3()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(self.cells.iter().cloned()),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleting_running_cell_removes_its_reruns_and_advances_queue() {
        let mut queue = RunQueue::default();
        queue.pending.extend([
            (1, 1, "deleted".into()),
            (1, 2, "rerun".into()),
            (2, 3, "next".into()),
        ]);
        queue.start_next();
        queue.remove(1);
        assert_eq!(queue.active, None);
        assert_eq!(queue.start_next(), Some((1, 3, "next".into())));
    }

    #[test]
    fn queued_and_running_cells_follow_insertions_and_moves() {
        let mut queue = RunQueue::default();
        queue
            .pending
            .extend([(0, 1, "first".into()), (1, 2, "second".into())]);
        queue.start_next();
        queue.remap(|i| i + 1);
        assert_eq!(queue.active, Some(1));
        assert!(queue.is_queued(2));
        queue.remap(|i| match i {
            1 => 2,
            2 => 1,
            _ => i,
        });
        assert_eq!(queue.active, Some(2));
        queue.finish(2);
        assert_eq!(queue.start_next(), Some((1, 2, "second".into())));
    }

    #[test]
    fn runs_wait_for_completion_and_start_in_submission_order() {
        let mut queue = RunQueue::default();
        queue.pending.push_back((0, 1, "code".into()));
        assert_eq!(
            queue.start_next().map(|(index, count, _)| (index, count)),
            Some((0, 1))
        );
        queue.pending.extend([
            (1, 2, "code".into()),
            (2, 3, "code".into()),
            (0, 4, "code".into()),
        ]);
        assert!(queue.is_queued(1));
        assert_eq!(
            queue.start_next().map(|(index, count, _)| (index, count)),
            None
        );
        queue.finish(0);
        assert_eq!(
            queue.start_next().map(|(index, count, _)| (index, count)),
            Some((1, 2))
        );
        assert!(!queue.is_queued(1));
        queue.finish(1);
        assert_eq!(
            queue.start_next().map(|(index, count, _)| (index, count)),
            Some((2, 3))
        );
        queue.finish(2);
        assert_eq!(
            queue.start_next().map(|(index, count, _)| (index, count)),
            Some((0, 4))
        );
        queue.finish(0);
        assert_eq!(
            queue.start_next().map(|(index, count, _)| (index, count)),
            None
        );
    }

    #[test]
    fn repeated_requests_for_a_cell_remain_queued() {
        let mut queue = RunQueue::default();
        queue.pending.extend([
            (0, 1, "code".into()),
            (0, 2, "code".into()),
            (0, 3, "code".into()),
        ]);
        assert_eq!(
            queue.start_next().map(|(index, count, _)| (index, count)),
            Some((0, 1))
        );
        assert!(queue.is_queued(0));
        queue.finish(0);
        assert_eq!(
            queue.start_next().map(|(index, count, _)| (index, count)),
            Some((0, 2))
        );
        assert!(queue.is_queued(0));
        queue.finish(0);
        assert_eq!(
            queue.start_next().map(|(index, count, _)| (index, count)),
            Some((0, 3))
        );
        assert!(!queue.is_queued(0));
    }

    #[test]
    fn queued_runs_keep_the_source_submitted_before_later_edits() {
        let mut queue = RunQueue::default();
        let mut source = "first\n  second".to_owned();
        queue.pending.push_back((0, 1, source.clone()));
        source.clear();
        source.push_str("edited");
        queue.pending.push_back((0, 2, source));
        assert_eq!(queue.start_next(), Some((0, 1, "first\n  second".into())));
        queue.finish(0);
        assert_eq!(queue.start_next(), Some((0, 2, "edited".into())));
    }

    #[test]
    fn execution_numbers_increase_across_cells_and_reruns() {
        let mut counter = SubmissionCounter::default();
        assert_eq!(counter.run("print(1)"), Some(1));
        assert_eq!(counter.run("print(2)"), Some(2));
        assert_eq!(counter.run("print(1)"), Some(3));
    }

    #[test]
    fn empty_and_whitespace_cells_do_not_consume_numbers() {
        let mut counter = SubmissionCounter::default();
        assert_eq!(counter.run(""), None);
        assert_eq!(counter.run(" \n\t"), None);
        assert_eq!(counter.run("1"), Some(1));
        assert_eq!(counter.run(""), None);
        assert_eq!(counter.run("2"), Some(2));
    }
}
