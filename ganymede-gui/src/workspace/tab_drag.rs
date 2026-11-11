//! In-window tab dragging. Only a valid drop mutates the tab order.
use super::*;
use gpui::{Bounds, EntityId, Pixels, Point, Task, point};

#[derive(Clone)]
pub(super) struct DragTab {
    pub workspace: EntityId,
    pub id: u64,
    pub name: SharedString,
    pub icon: Icon,
}

impl Render for DragTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        div()
            .w(px(180.))
            .h(px(28.))
            .px_2()
            .flex()
            .items_center()
            .gap_1p5()
            .rounded(t.radius.md())
            .bg(t.popover.bg)
            .text_color(t.popover.fg)
            .border_1()
            .border_color(t.popover.border)
            .text_size(t.font.xs)
            .opacity(0.85)
            .child(self.icon.svg().size(px(14.)).text_color(t.popover.fg))
            .child(div().min_w_0().truncate().child(self.name.clone()))
    }
}

#[derive(Default)]
pub(super) struct TabDrag {
    pub id: Option<u64>,
    task: Option<Task<()>>,
    // GPUI a19e194's test platform cannot simulate native window hover changes.
    // Keep this explicit and per-workspace so tests can exercise both states.
    #[cfg(test)]
    pub simulated_window_hover: Option<bool>,
}

impl TabDrag {
    fn window_hovered(&self, window: &Window) -> bool {
        #[cfg(test)]
        if let Some(hovered) = self.simulated_window_hover {
            return hovered;
        }
        window.is_window_hovered()
    }
}

impl Tabs {
    // `gap` is an insertion boundary in the original order, including both ends.
    fn reorder(&mut self, id: u64, gap: usize) -> bool {
        let Some(source) = self.open.iter().position(|tab| *tab == id) else {
            return false;
        };
        if gap > self.open.len() {
            return false;
        }
        let target = gap - usize::from(source < gap);
        if source == target {
            return false;
        }
        self.open.remove(source);
        self.open.insert(target, id);
        true
    }
}

// Positive values scroll toward the first tab; negative toward the last.
fn edge_scroll(bounds: Bounds<Pixels>, position: Point<Pixels>) -> Pixels {
    if !bounds.contains(&position) {
        return px(0.);
    }
    let edge = px(32.).min(bounds.size.width / 2.);
    if edge <= px(0.) {
        return px(0.);
    }
    if position.x < bounds.left() + edge {
        px(12.) * (1. - (position.x - bounds.left()) / edge)
    } else if position.x > bounds.right() - edge {
        -px(12.) * (1. - (bounds.right() - position.x) / edge)
    } else {
        px(0.)
    }
}

impl Workspace {
    pub(super) fn begin_tab_drag(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_tab = None;
        // Settle opening/closing widths so drop geometry stays stable while dragging.
        self.sync_header_tabs(Instant::now(), true);
        self.tab_drag.id = Some(id);
        self.tab_drag.task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let keep_going = this.update_in(cx, |this, window, cx| {
                    if !cx.has_active_drag()
                        || !this.tabs.open.contains(&id)
                        || this.tab_drag.id != Some(id)
                    {
                        this.tab_drag.id = None;
                        cx.notify();
                        return false;
                    }
                    if this.tab_drag.window_hovered(window) {
                        let scroll = &this.tab_scroll;
                        let step = edge_scroll(scroll.bounds(), window.mouse_position());
                        let offset = scroll.offset();
                        let x = (offset.x + step).clamp(-scroll.max_offset().x, px(0.));
                        if x != offset.x {
                            scroll.set_offset(point(x, offset.y));
                            cx.notify();
                        }
                    }
                    true
                });
                if !matches!(keep_going, Ok(true)) {
                    break;
                }
            }
        }));
        cx.notify();
    }

    pub fn cancel_tab_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.tab_drag.id.take().is_none() {
            return false;
        }
        self.tab_drag.task = None;
        let canceled = cx.stop_active_drag(window);
        cx.notify();
        canceled
    }

    pub(super) fn tab_drop_position(&self, position: Point<Pixels>) -> Option<(usize, Pixels)> {
        let viewport = self.tab_scroll.bounds();
        if !viewport.contains(&position) {
            return None;
        }
        let offset = self.tab_scroll.offset().x;
        let mut end = viewport.left();
        for (index, id) in self.tabs.open.iter().enumerate() {
            let visual_index = self.header_tabs.iter().position(|tab| tab.id == *id)?;
            let bounds = self.tab_scroll.bounds_for_item(visual_index)?;
            let left = bounds.left() + offset;
            end = bounds.right() + offset;
            if position.x < left + bounds.size.width / 2. {
                return Some((index, left.max(viewport.left())));
            }
        }
        Some((self.tabs.open.len(), end.min(viewport.right() - px(2.))))
    }

    pub(super) fn drop_tab(&mut self, drag: &DragTab, window: &mut Window, cx: &mut Context<Self>) {
        if drag.workspace != cx.entity_id() || self.tab_drag.id != Some(drag.id) {
            return;
        }
        let target = self.tab_drop_position(window.mouse_position());
        self.tab_drag.id = None;
        self.tab_drag.task = None;
        if let Some((gap, _)) = target
            && self.tabs.reorder(drag.id, gap)
        {
            self.header_tabs
                .sort_by_key(|tab| self.tabs.open.iter().position(|id| *id == tab.id));
            self.persist_layout(cx);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reorder_keeps_active_identity_and_handles_both_ends_and_noops() {
        let mut tabs = Tabs {
            open: vec![1, LAUNCHER, 2, 3],
            active: 2,
        };
        assert!(tabs.reorder(1, 4));
        assert_eq!(tabs.open, [LAUNCHER, 2, 3, 1]);
        assert!(tabs.reorder(3, 0));
        assert_eq!(tabs.open, [3, LAUNCHER, 2, 1]);
        assert!(tabs.reorder(LAUNCHER, 3));
        assert_eq!(tabs.open, [3, 2, LAUNCHER, 1]);
        assert_eq!(tabs.active, 2);
        assert!(!tabs.reorder(2, 1));
        assert!(!tabs.reorder(2, 2));
        assert!(!tabs.reorder(99, 0));
        assert!(!tabs.reorder(2, 5));
        assert_eq!(tabs.open, [3, 2, LAUNCHER, 1]);
        let mut single = Tabs::default();
        assert!(!single.reorder(LAUNCHER, 1));
    }

    #[test]
    fn edge_scrolling_only_runs_inside_the_strip_and_toward_the_near_edge() {
        let bounds = Bounds::new(point(px(100.), px(20.)), gpui::size(px(200.), px(28.)));
        assert!(edge_scroll(bounds, point(px(101.), px(30.))) > px(0.));
        assert!(edge_scroll(bounds, point(px(299.), px(30.))) < px(0.));
        assert_eq!(edge_scroll(bounds, point(px(200.), px(30.))), px(0.));
        assert_eq!(edge_scroll(bounds, point(px(99.), px(30.))), px(0.));
        assert_eq!(edge_scroll(bounds, point(px(299.), px(49.))), px(0.));
    }
}
