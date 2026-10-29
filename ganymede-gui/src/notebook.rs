//! Disposable notebook interactions for the GUI demo; no kernel or persistence.
use std::collections::VecDeque;

use gpui::{Context, Entity, Focusable, ScrollHandle, Window, div, prelude::*};

use crate::code_cell::{CellEvent, CodeCell};

#[derive(Default)]
struct ExecutionCounter(u64);

impl ExecutionCounter {
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
    cells: Vec<Entity<CodeCell>>,
    counter: ExecutionCounter,
    runs: RunQueue,
    scroll: ScrollHandle,
}

impl Notebook {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut notebook = Self {
            cells: vec![],
            counter: ExecutionCounter::default(),
            runs: RunQueue::default(),
            scroll: ScrollHandle::new(),
        };
        notebook.append_cell(window, cx);
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
                CellEvent::Selected => this.select(index, cx),
                CellEvent::DeleteCell => {
                    // Cancel the disposable simulation before removing the entity.
                    cell.update(cx, |cell, cx| cell.simulate_run(None, String::new(), cx));
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
                    let target = if matches!(event, CellEvent::InsertAbove) {
                        index
                    } else {
                        index + 1
                    };
                    this.append_cell(window, cx);
                    let added = this.cells.pop().unwrap();
                    added.update(cx, |cell, cx| cell.set_text(source, cx));
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
                    this.scroll.scroll_to_item(target);
                    cx.notify();
                }
                CellEvent::RunFinished => {
                    this.runs.finish(index);
                    this.start_next_run(cx);
                }
                CellEvent::RunAndAdvance => {
                    let source = cell.read(cx).text().to_owned();
                    let count = this.counter.run(&source);
                    if let Some(count) = count {
                        this.runs.pending.push_back((index, count, source));
                        this.start_next_run(cx);
                    } else if this.runs.active != Some(index) && !this.runs.is_queued(index) {
                        cell.update(cx, |cell, cx| cell.simulate_run(None, String::new(), cx));
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

    fn start_next_run(&mut self, cx: &mut Context<Self>) {
        if let Some((index, count, source)) = self.runs.start_next() {
            self.cells[index].update(cx, |cell, cx| cell.simulate_run(Some(count), source, cx));
        }
        for (index, cell) in self.cells.iter().enumerate() {
            cell.update(cx, |cell, cx| {
                cell.set_queued(self.runs.is_queued(index), cx)
            });
        }
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        for (position, cell) in self.cells.iter().enumerate() {
            cell.update(cx, |cell, cx| cell.set_current(position == index, cx));
        }
    }
}

impl Render for Notebook {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("notebook-cells")
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_3()
            .p_6()
            .pl_4()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children(self.cells.iter().cloned())
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
        let mut counter = ExecutionCounter::default();
        assert_eq!(counter.run("print(1)"), Some(1));
        assert_eq!(counter.run("print(2)"), Some(2));
        assert_eq!(counter.run("print(1)"), Some(3));
    }

    #[test]
    fn empty_and_whitespace_cells_do_not_consume_numbers() {
        let mut counter = ExecutionCounter::default();
        assert_eq!(counter.run(""), None);
        assert_eq!(counter.run(" \n\t"), None);
        assert_eq!(counter.run("1"), Some(1));
        assert_eq!(counter.run(""), None);
        assert_eq!(counter.run("2"), Some(2));
    }
}
