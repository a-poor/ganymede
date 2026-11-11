//! Dragging is provisional: only a valid same-notebook drop records an edit.
use super::*;
use crate::theme::ActiveTheme;
use gpui::{Bounds, EntityId, Pixels, Point, Task, px};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
    time::Duration,
};

#[derive(Clone)]
pub(super) struct DragCells {
    notebook: EntityId,
    source: EntityId,
    count: usize,
}
impl Render for DragCells {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        div()
            .px_2()
            .py_1()
            .rounded(t.radius.md())
            .bg(t.popover.bg)
            .text_color(t.popover.fg)
            .border_1()
            .border_color(t.popover.border)
            .text_size(t.font.xs)
            .child(format!(
                "{} cell{}",
                self.count,
                if self.count == 1 { "" } else { "s" }
            ))
    }
}

#[derive(Default)]
pub(super) struct CellDrag {
    pub order: Option<Vec<EntityId>>,
    range: Option<std::ops::Range<usize>>,
    pub viewport: Rc<Cell<Bounds<Pixels>>>,
    pub rows: Rc<RefCell<BTreeMap<usize, Bounds<Pixels>>>>,
    task: Option<Task<()>>,
    #[cfg(test)]
    pub simulated_window_hover: Option<bool>,
}

fn edge_scroll(bounds: Bounds<Pixels>, position: Point<Pixels>) -> Pixels {
    if !bounds.contains(&position) {
        return px(0.);
    }
    let edge = px(40.).min(bounds.size.height / 2.);
    if edge <= px(0.) {
        return px(0.);
    }
    if position.y < bounds.top() + edge {
        -px(16.) * (1. - (position.y - bounds.top()) / edge)
    } else if position.y > bounds.bottom() - edge {
        px(16.) * (1. - (bounds.bottom() - position.y) / edge)
    } else {
        px(0.)
    }
}

impl Notebook {
    pub(super) fn drag_row(&self, index: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let cell = self.cells[index].clone();
        let source = cell.entity_id();
        let book = cx.entity();
        let rows = self.cell_drag.rows.clone();
        let drag = DragCells {
            notebook: cx.entity_id(),
            source,
            count: 1,
        };
        // Editor, rendered content, and controls consume their mouse-downs.
        // Only the cell's surrounding surface can arm this drag listener.
        div()
            .id(("cell-drag", source))
            .relative()
            .w_full()
            .min_w_0()
            .when(!self.file_busy, |row| {
                row.on_drag(drag, move |drag, _, window, cx| {
                    let count =
                        book.update(cx, |book, cx| book.begin_cell_drag(source, window, cx));
                    cx.new(|_| DragCells {
                        count,
                        ..drag.clone()
                    })
                })
            })
            .child(cell)
            .child(
                gpui::canvas(
                    move |bounds, window, _| {
                        // Use painted geometry: focus/selection notifications may mark
                        // ListState measurements unknown between mouse-down and drop.
                        // The focus-keeper row is clipped and must not become a target.
                        if window.content_mask().bounds.size.height > px(0.) {
                            rows.borrow_mut().insert(index, bounds);
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .into_any_element()
    }

    fn begin_cell_drag(
        &mut self,
        source: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        let Some(index) = self
            .cells
            .iter()
            .position(|cell| cell.entity_id() == source)
        else {
            return 0;
        };
        if self.file_busy {
            return 0;
        }
        if !self.selected_range().contains(&index) {
            self.select(index, cx);
        }
        // Move into cell mode so undo and navigation apply to the dragged group.
        // The input prompt/editor entity stays alive, including its pending value.
        self.cells[self.selected]
            .read(cx)
            .list_focus_handle()
            .focus(window, cx);
        self.reveal_cell = None;
        self.cell_drag.range = Some(self.selected_range());
        self.cell_drag.order = Some(self.cells.iter().map(Entity::entity_id).collect());
        self.cell_drag.task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let keep = this.update_in(cx, |book, window, cx| {
                    if !cx.has_active_drag() || !book.cell_drag_valid() {
                        book.cell_drag.order = None;
                        book.cell_drag.range = None;
                        cx.notify();
                        return false;
                    }
                    let hovered = window.is_window_hovered();
                    #[cfg(test)]
                    let hovered = book.cell_drag.simulated_window_hover.unwrap_or(hovered);
                    if hovered {
                        let step =
                            edge_scroll(book.cell_drag.viewport.get(), window.mouse_position());
                        book.scroll.scroll_by(step);
                    }
                    // Repaint the indicator even when the pointer moves without scrolling.
                    cx.notify();
                    true
                });
                if !matches!(keep, Ok(true)) {
                    break;
                }
            }
        }));
        cx.notify();
        self.selected_range().len()
    }

    fn cell_drag_valid(&self) -> bool {
        !self.file_busy
            && self.cell_drag.order.as_ref().is_some_and(|order| {
                order
                    .iter()
                    .copied()
                    .eq(self.cells.iter().map(Entity::entity_id))
            })
            && self.cell_drag.range.as_ref() == Some(&self.selected_range())
    }

    pub(super) fn cancel_cell_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.cell_drag.order.take().is_none() {
            return false;
        }
        self.cell_drag.range = None;
        self.cell_drag.task = None;
        cx.stop_active_drag(window);
        cx.notify();
        true
    }

    pub(super) fn cell_drop_position(&self, position: Point<Pixels>) -> Option<(usize, Pixels)> {
        let viewport = self.cell_drag.viewport.get();
        if !viewport.contains(&position) {
            return None;
        }
        let mut last = None;
        for (&index, &bounds) in self.cell_drag.rows.borrow().iter() {
            if bounds.bottom() < viewport.top() || bounds.top() > viewport.bottom() {
                continue;
            }
            if position.y < bounds.center().y {
                return Some((index, bounds.top().max(viewport.top())));
            }
            last = Some((index + 1, bounds.bottom().min(viewport.bottom() - px(2.))));
        }
        last
    }

    pub(super) fn drop_cells(
        &mut self,
        drag: &DragCells,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if drag.notebook != cx.entity_id()
            || !self.cell_drag_valid()
            || !self.cells[self.selected_range()]
                .iter()
                .any(|cell| cell.entity_id() == drag.source)
        {
            return;
        }
        let target = self.cell_drop_position(window.mouse_position());
        self.cell_drag.order = None;
        self.cell_drag.range = None;
        self.cell_drag.task = None;
        if let Some((gap, _)) = target {
            self.move_selection_to_gap(gap, cx);
        }
        cx.notify();
    }

    /// Insertion boundary in the original order, including both ends.
    pub(super) fn move_selection_to_gap(&mut self, gap: usize, cx: &mut Context<Self>) {
        let range = self.selected_range();
        if self.file_busy || gap > self.cells.len() || (range.start..=range.end).contains(&gap) {
            return;
        }
        if !self.record_structure(None, cx) {
            return;
        }

        let moving: Vec<_> = self.cells.drain(range.clone()).collect();
        let target = if gap > range.end {
            gap - range.len()
        } else {
            gap
        };
        self.cells.splice(target..target, moving);
        let map = |index| {
            if range.contains(&index) {
                target + index - range.start
            } else if gap < range.start && (gap..range.start).contains(&index) {
                index + range.len()
            } else if gap > range.end && (range.end..gap).contains(&index) {
                index - range.len()
            } else {
                index
            }
        };
        self.runs.remap(map);
        self.selected = map(self.selected);
        self.selection_anchor = map(self.selection_anchor);
        self.sync(cx);
        self.reveal_cell = Some(self.selected);
        cx.notify();
    }
}
