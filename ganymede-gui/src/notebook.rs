//! Server-backed notebook editor.
mod list_row;
use std::collections::VecDeque;

use gpui::{Context, Entity, Focusable, ListAlignment, ListState, Window, div, prelude::*};

use ganymede_core::notebook::CellKind;

use crate::code_cell::{CellEvent, CodeCell};

gpui::actions!(notebook, [FindNotebook, CloseFind]);
pub fn init(cx: &mut gpui::App) {
    let key = if cfg!(target_os = "macos") {
        "cmd-f"
    } else {
        "ctrl-f"
    };
    cx.bind_keys([
        gpui::KeyBinding::new(key, FindNotebook, Some("Notebook")),
        gpui::KeyBinding::new("escape", CloseFind, Some("Notebook && !CodeCell")),
    ]);
}

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
    file_busy: bool,
    save_conflict: bool,
    autosave: crate::autosave::AutosaveClock,
    autosave_suspended: bool,
    save_error: Option<String>,
    session: crate::execution::Session,
    control_pending: bool,
    control_label: &'static str,
    connecting: bool,
    restart_run_all: bool,
    execution_error: Option<String>,
    selected: usize,
    largest_count: u64,
    applied_count_hint: Option<u64>,
    type_menu_open: bool,
    type_menu_index: usize,
    type_focus: gpui::FocusHandle,
    counter: SubmissionCounter,
    runs: RunQueue,
    scroll: ListState,
    listed_cells: Vec<gpui::EntityId>,
    reveal_cell: Option<usize>,
    find_open: bool,
    find_input: Entity<CodeCell>,
    find_hits: Vec<crate::find::Hit>,
    find_current: Option<usize>,
    find_dirty: bool,
    find_reveal: bool,
    find_pending: bool,
    find_task: gpui::Task<()>,
}

impl Notebook {
    pub fn new(
        session: crate::execution::Session,
        toasts: Entity<crate::toast::ToastHost>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let find_input = cx.new(|cx| CodeCell::new_search(window, cx));
        cx.subscribe_in(
            &find_input,
            window,
            |this, _, event, window, cx| match event {
                CellEvent::Changed => {
                    this.find_reveal = true;
                    this.find_dirty = true;
                    cx.notify();
                }
                CellEvent::MoveDown => this.navigate_find(false, cx),
                CellEvent::MoveUp => this.navigate_find(true, cx),
                CellEvent::DeleteCell => this.close_find(window, cx),
                CellEvent::FocusNext => window.focus_next(cx),
                _ => (),
            },
        )
        .detach();
        let mut notebook = Self {
            toasts,
            cells: vec![],
            session,
            saving: false,
            file_busy: false,
            save_conflict: false,
            autosave: Default::default(),
            autosave_suspended: false,
            save_error: None,
            control_pending: false,
            control_label: "",
            connecting: true,
            restart_run_all: false,
            execution_error: None,
            selected: 0,
            largest_count: 0,
            applied_count_hint: None,
            type_menu_open: false,
            type_menu_index: 0,
            type_focus: cx.focus_handle(),
            counter: SubmissionCounter::default(),
            runs: RunQueue::default(),
            scroll: ListState::new(0, ListAlignment::Top, gpui::px(200.)),
            listed_cells: vec![],
            reveal_cell: None,
            find_open: false,
            find_input,
            find_hits: vec![],
            find_current: None,
            find_dirty: false,
            find_reveal: false,
            find_pending: false,
            find_task: gpui::Task::ready(()),
        };
        cx.observe_global::<crate::theme::Theme>(|this, cx| {
            this.scroll.remeasure();
            cx.notify();
        })
        .detach();
        if let Some(mut snapshots) = notebook.session.watch() {
            cx.spawn(async move |this, cx| {
                while snapshots.changed().await.is_ok() {
                    if this
                        .update(cx, |this, cx| {
                            if this.session.activity()
                                == crate::execution::KernelActivity::Disconnected
                            {
                                this.runs.pending.clear();
                                for cell in &this.cells {
                                    cell.update(cx, |cell, cx| cell.set_queued(false, cx));
                                }
                            }
                            cx.notify();
                        })
                        .is_err()
                    {
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
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
                if this
                    .update_in(cx, |this, window, cx| {
                        let document = this.session.document();
                        let enabled = cx.global::<crate::autosave::AutosavePreference>().0
                            && !this.autosave_suspended
                            && !this.file_busy
                            && !this.save_conflict;
                        if this.autosave.due(
                            document.revision(),
                            document.is_dirty(),
                            enabled,
                            this.saving,
                            std::time::Instant::now(),
                        ) {
                            this.save(window, cx);
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
                if this.execution_error.is_some() {
                    this.runs.pending.clear();
                }
                this.start_next_run(cx);
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
            notebook.largest_count = notebook
                .largest_count
                .max(cell.read(cx).max_execution_count());
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
        cell.update(cx, |cell, cx| cell.set_count_hint(self.largest_count, cx));
        cx.subscribe_in(&cell, window, |this, cell, event, window, cx| {
            let Some(index) = this.cells.iter().position(|item| item == cell) else {
                return;
            };
            match event {
                CellEvent::Changed => {
                    this.find_dirty = true;
                    this.sync(cx);
                    cx.notify();
                }
                CellEvent::FocusNext => (),
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
                    this.reveal_cell = Some(target);
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
                    this.reveal_cell = Some(target);
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
                    cell.update(cx, |cell, cx| cell.preview_markdown(cx));
                    if this.control_pending || this.restart_run_all {
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
                    this.reveal_cell = Some(index + 1);
                    cx.notify();
                }
            }
        })
        .detach();
        // Heights can change outside the viewport (execution, Markdown preview,
        // edits, or a theme change). Keep the list's measurements current without
        // resetting its logical scroll anchor.
        cx.observe(&cell, |this, cell, cx| {
            this.largest_count = this.largest_count.max(cell.read(cx).max_execution_count());
            if let Some(index) = this
                .listed_cells
                .iter()
                .position(|id| *id == cell.entity_id())
            {
                this.scroll.remeasure_items(index..index + 1);
            }
            cx.notify();
        })
        .detach();
        self.cells.push(cell);
    }

    fn sync_list(&mut self, cx: &gpui::App) {
        let ids: Vec<_> = self.cells.iter().map(Entity::entity_id).collect();
        if let Some((old, new)) = changed_items(&self.listed_cells, &ids) {
            self.scroll.splice_focusable(
                old,
                self.cells[new]
                    .iter()
                    .map(|cell| Some(cell.read(cx).list_focus_handle())),
            );
            self.listed_cells = ids;
            self.find_dirty = true;
        }
        if let Some(index) = self.reveal_cell.take() {
            // An unmeasured destination has no reliable pixel position yet. A
            // logical item offset reveals it without measuring preceding cells.
            let index = index.min(self.cells.len().saturating_sub(1));
            if self.scroll.bounds_for_item(index).is_some() {
                self.scroll.scroll_to_reveal_item(index);
            } else {
                self.scroll.scroll_to(gpui::ListOffset {
                    item_ix: index,
                    offset_in_item: gpui::px(0.),
                });
            }
        }
    }

    fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let seed = self
            .cells
            .get(self.selected)
            .map(|cell| cell.read(cx).selected_text().to_owned())
            .unwrap_or_default();
        self.find_open = true;
        self.find_reveal = true;
        if !seed.is_empty() && !seed.contains('\n') {
            self.find_input
                .update(cx, |input, cx| input.set_text(seed, cx));
        }
        self.find_input
            .update(cx, |input, cx| input.select_all_input(cx));
        self.find_input.read(cx).focus_handle(cx).focus(window, cx);
        self.find_dirty = true;
        cx.notify();
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(hit) = self.find_current.and_then(|i| self.find_hits.get(i))
            && let Some(cell) = self
                .cells
                .iter()
                .find(|cell| cell.entity_id().as_u64() == hit.cell)
        {
            cell.update(cx, |cell, cx| cell.finish_find(&hit.range, cx));
        }
        self.find_open = false;
        self.find_task = gpui::Task::ready(());
        self.find_pending = false;
        self.find_hits.clear();
        self.find_current = None;
        self.apply_find(false, cx);
        if let Some(cell) = self.cells.get(self.selected) {
            cell.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    fn schedule_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_dirty = false;
        self.find_pending = true;
        let reveal = std::mem::take(&mut self.find_reveal);
        // Cancel the previous debounce/worker before taking a new snapshot.
        self.find_task = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(100))
                .await;
            let Ok((query, sources)) = this.update(cx, |this, cx| {
                (
                    this.find_input.read(cx).text().to_owned(),
                    this.cells
                        .iter()
                        .map(|cell| (cell.entity_id().as_u64(), cell.read(cx).text().to_owned()))
                        .collect::<Vec<_>>(),
                )
            }) else {
                return;
            };
            let hits = cx
                .background_executor()
                .spawn(async move { crate::find::matches(&query, &sources) })
                .await;
            this.update(cx, |this, cx| {
                if !this.find_open || this.find_dirty {
                    return;
                }
                let previous = this
                    .find_current
                    .and_then(|i| this.find_hits.get(i))
                    .cloned();
                this.find_current = previous
                    .as_ref()
                    .and_then(|old| hits.iter().position(|hit| hit == old))
                    .or_else(|| (!hits.is_empty()).then_some(0));
                this.find_hits = hits;
                this.find_pending = false;
                this.apply_find(reveal, cx);
                cx.notify();
            })
            .ok();
        });
    }

    fn navigate_find(&mut self, backwards: bool, cx: &mut Context<Self>) {
        if self.find_pending {
            return;
        }
        self.find_current = crate::find::step(self.find_current, self.find_hits.len(), backwards);
        self.apply_find(true, cx);
        cx.notify();
    }

    fn apply_find(&mut self, reveal: bool, cx: &mut Context<Self>) {
        let active = self.find_current.and_then(|i| self.find_hits.get(i));
        let mut hit_index = 0;
        for (index, cell) in self.cells.iter().enumerate() {
            let id = cell.entity_id().as_u64();
            let start = hit_index;
            while self
                .find_hits
                .get(hit_index)
                .is_some_and(|hit| hit.cell == id)
            {
                hit_index += 1;
            }
            let ranges = self.find_hits[start..hit_index]
                .iter()
                .map(|hit| hit.range.clone())
                .collect();
            let current = active
                .filter(|hit| hit.cell == id)
                .map(|hit| hit.range.clone());
            if current.is_some() && reveal {
                self.reveal_cell = Some(index);
                self.selected = index;
            }
            cell.update(cx, |cell, cx| {
                cell.set_search(ranges, current.clone(), reveal && current.is_some(), cx)
            });
        }
        if reveal && active.is_some() {
            self.select(self.selected, cx);
        }
    }

    fn find_bar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        use crate::{icon::Icon, theme::ActiveTheme, workspace::icon_button};
        let t = cx.theme();
        let label = if self.find_pending {
            "…".to_owned()
        } else if self.find_input.read(cx).text().is_empty() {
            String::new()
        } else if self.find_hits.is_empty() {
            "0/0".to_owned()
        } else {
            format!(
                "{}/{}",
                self.find_current.map_or(0, |i| i + 1),
                self.find_hits.len()
            )
        };
        div()
            .id("notebook-find-panel")
            .occlude()
            .absolute()
            .top(gpui::px(44.))
            .right(gpui::px(8.))
            .w(gpui::px(340.))
            .max_w_full()
            .flex()
            .items_center()
            .gap(gpui::px(2.))
            .p_1()
            .bg(t.popover.bg)
            .text_color(t.popover.fg)
            .border_1()
            .border_color(t.popover.border)
            .rounded(t.radius.lg())
            .shadow(vec![t.toast_shadow.clone()])
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(self.find_input.clone())
                    .child(
                        div()
                            .flex_shrink_0()
                            .pr_2()
                            .text_size(t.font.xs)
                            .text_color(t.popover.muted_fg)
                            .child(label),
                    ),
            )
            .child(
                div()
                    .w(gpui::px(1.))
                    .h(gpui::px(20.))
                    .mr_1()
                    .bg(t.popover.border),
            )
            .child(
                icon_button(
                    "find-previous",
                    "Previous match (Shift+Enter)",
                    Icon::ArrowUp,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.navigate_find(true, cx)))
                .on_key_down(cx.listener(
                    |this, event: &gpui::KeyDownEvent, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            this.navigate_find(true, cx);
                            cx.stop_propagation();
                        }
                    },
                )),
            )
            .child(
                icon_button("find-next", "Next match (Enter)", Icon::ArrowDown, cx)
                    .on_click(cx.listener(|this, _, _, cx| this.navigate_find(false, cx)))
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            this.navigate_find(false, cx);
                            cx.stop_propagation();
                        }
                    })),
            )
            .child(
                icon_button("find-close", "Close find (Escape)", Icon::X, cx)
                    .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx)))
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            this.close_find(window, cx);
                            cx.stop_propagation();
                        }
                    })),
            )
            .into_any_element()
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
    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.file_busy {
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
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let conflict = matches!(
                result,
                Ok(Err(ganymede_core::notebook::Error::SaveConflict))
            );
            this.update_in(cx, |this, _, cx| {
                if conflict {
                    this.save_conflict = true;
                    this.autosave.failed();
                } else {
                    this.finish_save(
                        result
                            .map_err(|_| "Could not save notebook".to_owned())
                            .and_then(|result| result.map_err(|error| error.to_string())),
                        cx,
                    );
                }
                cx.notify();
            })
            .ok();
            if !conflict {
                return;
            }
            // Serialize prompts across notebooks, including saves requested during quit.
            loop {
                let response = this.update_in(cx, |this, window, cx| {
                    if window.has_active_prompt() {
                        return None;
                    }
                    Some(crate::confirm::save_conflict(
                        &this.session.path(),
                        window,
                        cx,
                    ))
                });
                match response {
                    Ok(Some(response)) => {
                        let choice = response.await;
                        this.update_in(cx, |this, window, cx| {
                            this.resolve_conflict(choice, window, cx)
                        })
                        .ok();
                        break;
                    }
                    Ok(None) => {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(100))
                            .await
                    }
                    Err(_) => break,
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn finish_save(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        self.saving = false;
        self.save_error = result.err();
        if self.save_error.is_some() {
            self.autosave.failed();
        } else {
            self.save_conflict = false;
            self.autosave.resume();
        }
        self.report_save_error(cx);
        cx.notify();
    }

    fn resolve_conflict(&mut self, choice: usize, window: &mut Window, cx: &mut Context<Self>) {
        if choice == 0 {
            self.saving = false;
            // Keep autosave paused and prevent Save-and-close from discarding local work.
            self.save_error = Some("Save conflict unresolved".into());
            cx.notify();
            return;
        }
        if choice == 2 {
            self.sync(cx);
            let task = self.session.save_copy();
            cx.spawn_in(window, async move |this, cx| {
                let result = task.await;
                this.update_in(cx, |this, _, cx| {
                    this.saving = false;
                    this.save_error = Some("Original notebook still has a save conflict".into());
                    match result {
                        Ok(Ok(path)) => { this.toasts.update(cx, |host, cx| {
                            host.push(crate::toast::Toast::new("Notebook copy saved").description(format!("Saved as {path}. The original is still open; autosave remains paused.")), cx);
                        }); }
                        result => {
                            this.save_error = Some(match result { Ok(Err(error)) => error.to_string(), _ => "Could not save notebook copy".into() });
                            this.report_save_error(cx);
                        }
                    }
                    cx.notify();
                }).ok();
            }).detach();
            return;
        }
        if choice == 1 && (self.runs.active.is_some() || !self.runs.pending.is_empty()) {
            self.finish_save(
                Err("Finish or interrupt execution before reloading the notebook".into()),
                cx,
            );
            return;
        }
        self.sync(cx);
        let task = if choice == 1 {
            self.session.reload()
        } else {
            self.session.overwrite()
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = task
                .await
                .map_err(|_| "Could not resolve save conflict".to_owned())
                .and_then(|result| result.map_err(|error| error.to_string()));
            this.update_in(cx, |this, window, cx| {
                if choice == 1 && result.is_ok() {
                    this.cells.clear();
                    this.session.clear_bindings();
                    this.runs = RunQueue::default();
                    this.selected = 0;
                    for loaded in this.session.document().cells() {
                        this.append_cell(window, cx);
                        let cell = this.cells.last().unwrap();
                        this.session
                            .bind(cell.entity_id().as_u64(), loaded.id().clone());
                        cell.update(cx, |cell, cx| cell.load(loaded, cx));
                    }
                    if this.cells.is_empty() {
                        this.append_cell(window, cx);
                    }
                    this.select(0, cx);
                }
                this.finish_save(result, cx);
            })
            .ok();
        })
        .detach();
    }
    pub fn file_operation_pending(&self) -> bool {
        self.saving || self.connecting || self.control_pending
    }
    pub fn set_file_busy(&mut self, busy: bool) {
        self.file_busy = busy;
    }
    pub fn file_controller(
        &mut self,
        cx: &mut Context<Self>,
    ) -> ganymede_core::notebook::NotebookController {
        self.sync(cx);
        self.session.controller()
    }
    pub fn file_saved_as(&mut self, cx: &mut Context<Self>) {
        self.save_conflict = false;
        self.save_error = None;
        self.autosave.resume();
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
                self.reveal_cell = Some(target);
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
            ToolbarAction::Run => {
                self.cells[self.selected].update(cx, |cell, cx| cell.preview_markdown(cx));
                if self.control_pending || self.restart_run_all {
                    return;
                }
                let indices = self.selected..self.selected + 1;
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
            ToolbarAction::Stop => self.kernel_control(false, false, cx),
            ToolbarAction::Restart | ToolbarAction::RunAll => {
                if self.control_pending || self.connecting || window.has_active_prompt() {
                    return;
                }
                let run_all = matches!(action, ToolbarAction::RunAll);
                let response = window.prompt(
                    gpui::PromptLevel::Warning,
                    if run_all { "Restart kernel and run all cells?" } else { "Restart kernel?" },
                    Some("All variables in memory will be lost. Any running execution and queued cells will be stopped. Your notebook and saved outputs will be kept."),
                    &["Cancel", if run_all { "Restart and Run All" } else { "Restart" }],
                    cx,
                );
                cx.spawn(async move |this, cx| {
                    if response.await == Ok(1) {
                        this.update(cx, |this, cx| this.kernel_control(true, run_all, cx))
                            .ok();
                    }
                })
                .detach();
            }
        }

        self.sync(cx);
        self.focus_selected(window, cx);
        cx.notify();
    }

    fn kernel_control(&mut self, restart: bool, run_all: bool, cx: &mut Context<Self>) {
        if self.control_pending || self.connecting || self.file_busy {
            return;
        }
        self.runs.pending.clear();
        self.restart_run_all = false;
        for cell in &self.cells {
            cell.update(cx, |cell, cx| cell.set_queued(false, cx));
        }
        self.control_pending = true;
        self.control_label = if restart {
            "Restarting…"
        } else {
            "Interrupting…"
        };
        self.execution_error = None;
        let task = self.session.control(restart);
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Kernel operation failed".into()));
            this.update(cx, |this, cx| {
                this.control_pending = false;
                this.restart_run_all = result.is_ok() && run_all;
                this.execution_error = result.err();
                this.report_kernel_error(cx);
                this.start_next_run(cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn reconnect(&mut self, cx: &mut Context<Self>) {
        if self.connecting || self.control_pending || self.file_busy {
            return;
        }
        self.connecting = true;
        self.execution_error = None;
        let task = self.session.connect();
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Could not reconnect to the kernel".into()));
            this.update(cx, |this, cx| {
                this.connecting = false;
                this.execution_error = result.err();
                this.report_kernel_error(cx);
                if this.execution_error.is_some() {
                    this.runs.pending.clear();
                }
                this.start_next_run(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn report_kernel_error(&self, cx: &mut Context<Self>) {
        if let Some(error) = &self.execution_error {
            let toast = crate::toast::Toast::new("Kernel operation failed")
                .description(error.clone())
                .kind(crate::toast::ToastKind::Error);
            self.toasts.update(cx, |host, cx| {
                host.push(toast, cx);
            });
        }
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
                    .on_click(cx.listener(|this, _, window, cx| this.save(window, cx)))
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            this.save(window, cx);
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
                    (ToolbarAction::Stop, "Interrupt kernel", Icon::Stop),
                    (ToolbarAction::Restart, "Restart kernel", Icon::Restart),
                    (
                        ToolbarAction::RunAll,
                        "Restart kernel and run all cells",
                        Icon::RunAll,
                    ),
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
            .when(
                activity == KernelActivity::Disconnected
                    && !self.connecting
                    && !self.control_pending,
                |bar| {
                    bar.child(
                        icon_button("reconnect-kernel", "Reconnect kernel", Icon::Restart, cx)
                            .on_click(cx.listener(|this, _, _, cx| this.reconnect(cx)))
                            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                    this.reconnect(cx);
                                    cx.stop_propagation();
                                }
                            })),
                    )
                },
            )
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
                    } else if self.control_pending || self.restart_run_all {
                        self.control_label.to_owned()
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
        if self.control_pending || self.connecting {
            return;
        }
        // Let the old execution observer finish before starting the restarted batch.
        if self.restart_run_all && self.runs.active.is_none() {
            self.restart_run_all = false;
            for (index, cell) in self.cells.iter().enumerate() {
                let cell = cell.read(cx);
                if cell.kind() == CellKind::Code {
                    let source = cell.text().to_owned();
                    if let Some(count) = self.counter.run(&source) {
                        self.runs.pending.push_back((index, count, source));
                    }
                }
            }
        }
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

// Preserve unchanged prefix/suffix measurements when cells are inserted,
// removed, or reordered. The changed middle is the only splice required.
fn changed_items<T: PartialEq>(
    old: &[T],
    new: &[T],
) -> Option<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    if prefix == old.len() && prefix == new.len() {
        return None;
    }
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    Some((prefix..old.len() - suffix, prefix..new.len() - suffix))
}

impl Render for Notebook {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.applied_count_hint != Some(self.largest_count) {
            for cell in &self.cells {
                cell.update(cx, |cell, cx| cell.set_count_hint(self.largest_count, cx));
            }
            self.applied_count_hint = Some(self.largest_count);
        }
        self.sync_list(cx);
        if self.find_open && self.find_dirty {
            self.schedule_find(window, cx);
        }
        let notebook = cx.entity().downgrade();
        let row_order = std::rc::Rc::new(list_row::RowOrder::default());
        let row_scroll = self.scroll.clone();
        div()
            .relative()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .key_context("Notebook")
            .on_action(cx.listener(|this, _: &CloseFind, window, cx| {
                if this.find_open {
                    this.close_find(window, cx);
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &FindNotebook, window, cx| this.open_find(window, cx)))
            .on_action(cx.listener(|this, _: &crate::Save, window, cx| this.save(window, cx)))
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
                gpui::list(self.scroll.clone(), move |index, _, cx| {
                    let cell = notebook
                        .upgrade()
                        .and_then(|notebook| notebook.read(cx).cells.get(index).cloned());
                    // Padding belongs to each row, since virtual lists don't use
                    // a flex gap. Keep cell entities alive independently of rows.
                    list_row::ListRow {
                        index,
                        order: row_order.clone(),
                        scroll: row_scroll.clone(),
                        child: div()
                            .w_full()
                            .min_w_0()
                            .pb_3()
                            .children(cell)
                            .into_any_element(),
                    }
                    .into_any_element()
                })
                .flex_1()
                .min_h_0()
                .p_4()
                .pl_3(),
            )
            .when(self.find_open, |view| view.child(self.find_bar(cx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_splices_preserve_unchanged_cells() {
        assert_eq!(changed_items(&[1, 2, 3], &[1, 4, 2, 3]), Some((1..1, 1..2)));
        assert_eq!(changed_items(&[1, 2, 3], &[1, 3]), Some((1..2, 1..1)));
        assert_eq!(changed_items(&[1, 2, 3], &[2, 1, 3]), Some((0..2, 0..2)));
        assert_eq!(changed_items(&[1, 2], &[1, 2]), None);
        assert_eq!(changed_items::<usize>(&[], &[1]), Some((0..0, 0..1)));
        assert_eq!(changed_items::<usize>(&[1], &[]), Some((0..1, 0..0)));
    }

    #[gpui::test]
    fn virtual_notebook_renders_a_bounded_window_and_preserves_growing_cell_anchor(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::{point, px, size};
        use std::{cell::Cell, rc::Rc};
        let cx = cx.add_empty_window();
        let state = ListState::new(10_000, ListAlignment::Top, px(200.));
        let calls = Rc::new(Cell::new(0));
        let extra_height = Rc::new(Cell::new(0.));
        struct TestView {
            state: ListState,
            calls: Rc<Cell<usize>>,
            extra_height: Rc<Cell<f32>>,
        }
        impl Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let calls = self.calls.clone();
                let extra_height = self.extra_height.clone();
                gpui::list(self.state.clone(), move |index, _, _| {
                    calls.set(calls.get() + 1);
                    div()
                        .w_full()
                        .h(px(60.
                            + (index % 4) as f32 * 40.
                            + if index == 5000 {
                                extra_height.get()
                            } else {
                                0.
                            }))
                        .into_any_element()
                })
                .w_full()
                .h_full()
            }
        }
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.draw(point(px(0.), px(0.)), size(px(800.), px(600.)), |_, cx| {
                cx.new(|_| TestView {
                    state: state.clone(),
                    calls: calls.clone(),
                    extra_height: extra_height.clone(),
                })
                .into_any_element()
            });
        };
        draw(cx);
        assert!(
            calls.get() < 64,
            "initial render built {} rows",
            calls.get()
        );
        calls.set(0);
        state.scroll_to(gpui::ListOffset {
            item_ix: 5000,
            offset_in_item: px(20.),
        });
        draw(cx);
        assert!(calls.get() < 64, "jump rendered {} rows", calls.get());
        assert_eq!(state.logical_scroll_top().item_ix, 5000);
        extra_height.set(400.);
        state.remeasure_items(5000..5001);
        calls.set(0);
        draw(cx);
        assert_eq!(state.logical_scroll_top().item_ix, 5000);
        assert_eq!(state.logical_scroll_top().offset_in_item, px(20.));
        assert!(calls.get() < 64);
        state.splice(0..0, 1);
        assert_eq!(state.logical_scroll_top().item_ix, 5001);
        state.splice(0..1, 0);
        assert_eq!(state.logical_scroll_top().item_ix, 5000);
    }

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
