//! Work around the pinned GPUI list painting its offscreen focus keeper after
//! the visible rows. Preserve its focus dispatch, but clip that extra rendering.
use gpui::{
    AnyElement, App, Bounds, ContentMask, Element, ElementId, GlobalElementId, InspectorElementId,
    LayoutId, Pixels, Window,
};
use std::{cell::Cell, rc::Rc};

#[derive(Default)]
pub(super) struct RowOrder {
    first: Cell<Option<usize>>,
    previous: Cell<Option<usize>>,
}
impl RowOrder {
    fn visible(&self, index: usize) -> bool {
        if self.first.get().is_none() || self.first.get() == Some(index) {
            // Prepaint can retry after an autoscroll request.
            self.first.set(Some(index));
            self.previous.set(Some(index));
            return true;
        }
        if self
            .previous
            .get()
            .is_some_and(|previous| index == previous + 1)
        {
            self.previous.set(Some(index));
            true
        } else {
            false
        }
    }
}

pub(super) struct ListRow {
    pub index: usize,
    pub order: Rc<RowOrder>,
    pub child: AnyElement,
    pub scroll: gpui::ListState,
}
impl Element for ListRow {
    type RequestLayoutState = ();
    type PrepaintState = bool;
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let visible = self.order.visible(self.index);
        if visible {
            self.child.prepaint(window, cx);
        } else {
            window.with_content_mask(
                Some(ContentMask {
                    bounds: Bounds::default(),
                }),
                |window| {
                    self.child.prepaint(window, cx);
                },
            );
            // Its synthetic position is not a valid autoscroll destination.
            if window.take_autoscroll().is_some() {
                let scroll = self.scroll.clone();
                let index = self.index;
                cx.defer(move |cx| {
                    scroll.scroll_to(gpui::ListOffset {
                        item_ix: index,
                        offset_in_item: gpui::px(0.),
                    });
                    cx.refresh_windows();
                });
            }
        }
        visible
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        visible: &mut bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        if *visible {
            self.child.paint(window, cx);
        } else {
            window.with_content_mask(
                Some(ContentMask {
                    bounds: Bounds::default(),
                }),
                |window| {
                    self.child.paint(window, cx);
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clips_appended_focus_keeper_and_allows_prepaint_retry() {
        let order = RowOrder::default();
        for index in 8..12 {
            assert!(order.visible(index));
        }
        assert!(!order.visible(0));
        for index in 8..12 {
            assert!(order.visible(index));
        }
        assert!(!order.visible(19));
    }
    #[gpui::test]
    fn focused_first_row_is_clipped_when_scrolled_to_last_row(cx: &mut gpui::TestAppContext) {
        use gpui::{ListAlignment, ListState, canvas, div, point, prelude::*, px, size};
        use std::cell::RefCell;
        let cx = cx.add_empty_window();
        let focus = cx.update(|_, cx| cx.focus_handle());
        let state = ListState::new(0, ListAlignment::Top, px(100.));
        state.splice_focusable(0..0, (0..30).map(|i| (i == 0).then(|| focus.clone())));
        let painted = Rc::new(RefCell::new(Vec::new()));
        type PaintedRows = Rc<RefCell<Vec<(usize, gpui::Size<Pixels>)>>>;
        struct TestView {
            state: ListState,
            focus: gpui::FocusHandle,
            painted: PaintedRows,
        }
        impl gpui::Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
                let order = Rc::new(RowOrder::default());
                let painted = self.painted.clone();
                let focus = self.focus.clone();
                let scroll = self.state.clone();
                gpui::list(self.state.clone(), move |index, _, _| {
                    let painted = painted.clone();
                    ListRow {
                        index,
                        order: order.clone(),
                        scroll: scroll.clone(),
                        child: div()
                            .id(index)
                            .w_full()
                            .h(px(50.))
                            .when(index == 0, |row| row.track_focus(&focus))
                            .child(
                                canvas(
                                    |_, _, _| (),
                                    move |_, _, window, _| {
                                        painted
                                            .borrow_mut()
                                            .push((index, window.content_mask().bounds.size));
                                    },
                                )
                                .size_full(),
                            )
                            .into_any_element(),
                    }
                    .into_any_element()
                })
                .size_full()
            }
        }
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.draw(
                point(px(0.), px(0.)),
                size(px(500.), px(300.)),
                |window, cx| {
                    focus.focus(window, cx);
                    cx.new(|_| TestView {
                        state: state.clone(),
                        focus: focus.clone(),
                        painted: painted.clone(),
                    })
                    .into_any_element()
                },
            );
        };
        draw(cx);
        state.scroll_to_end();
        painted.borrow_mut().clear();
        draw(cx);
        let painted = painted.borrow();
        assert!(
            painted
                .iter()
                .any(|(i, size)| *i == 29 && size.height > px(0.))
        );
        assert!(
            painted
                .iter()
                .any(|(i, size)| *i == 0 && size.height == px(0.))
        );
    }
}
