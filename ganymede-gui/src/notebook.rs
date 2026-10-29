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
