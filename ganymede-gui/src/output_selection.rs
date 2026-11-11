//! Read-only selection over the native text layouts of one cell's outputs.
use crate::theme::ActiveTheme;
use gpui::{
    AnyElement, App, Bounds, ClipboardItem, CursorStyle, DispatchPhase, FocusHandle, KeyBinding,
    MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Point, StyledText, TextLayout, actions,
    canvas, div, fill, point, prelude::*, px, size,
};
use std::{cell::RefCell, ops::Range, rc::Rc};
use unicode_segmentation::UnicodeSegmentation;

use crate::code_cell::{Copy, SelectAll};
actions!(output_selection, [Clear]);

pub fn init(cx: &mut App) {
    let mac = cfg!(target_os = "macos");
    cx.bind_keys([
        KeyBinding::new(
            if mac { "cmd-c" } else { "ctrl-c" },
            Copy,
            Some("OutputText"),
        ),
        KeyBinding::new(
            if mac { "cmd-a" } else { "ctrl-a" },
            SelectAll,
            Some("OutputText"),
        ),
        KeyBinding::new("escape", Clear, Some("OutputText")),
    ]);
}

#[derive(Clone)]
pub struct Selection(Rc<RefCell<State>>);
struct Segment {
    start: usize,
    len: usize,
    layout: Option<TextLayout>,
}
struct State {
    focus: FocusHandle,
    text: String,
    segments: Vec<Segment>,
    next: usize,
    anchor: usize,
    head: usize,
    dragging: bool,
}
impl State {
    fn range(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }
    fn hit(&self, position: Point<Pixels>) -> Option<usize> {
        self.segments
            .iter()
            .filter_map(|s| {
                let layout = s.layout.as_ref()?;
                let bounds = layout.bounds();
                let dx = (bounds.left() - position.x)
                    .max(px(0.))
                    .max(position.x - bounds.right());
                let dy = (bounds.top() - position.y)
                    .max(px(0.))
                    .max(position.y - bounds.bottom());
                let distance = f32::from(dx).powi(2) + f32::from(dy).powi(2);
                let index = layout
                    .index_for_position(position)
                    .unwrap_or_else(|i| i)
                    .min(s.len);
                Some((distance, s.start + index))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, i)| i)
    }
}
impl Selection {
    pub fn new(cx: &mut App) -> Self {
        Self(Rc::new(RefCell::new(State {
            focus: cx.focus_handle(),
            text: String::new(),
            segments: vec![],
            next: 0,
            anchor: 0,
            head: 0,
            dragging: false,
        })))
    }
    #[cfg(test)]
    pub fn position_for_index(&self, index: usize) -> Point<Pixels> {
        self.0.borrow().segments[0]
            .layout
            .as_ref()
            .unwrap()
            .position_for_index(index)
            .unwrap()
    }
    pub fn contains(&self, position: Point<Pixels>) -> bool {
        self.0.borrow().segments.iter().any(|s| {
            s.layout
                .as_ref()
                .is_some_and(|l| l.bounds().contains(&position))
        })
    }
    /// Preserve byte anchors only when existing text is unchanged (including append-only streams).
    pub fn prepare(&self, pieces: Vec<(String, &'static str)>) {
        let mut state = self.0.borrow_mut();
        let mut text = String::new();
        let mut segments = Vec::new();
        for (piece, separator) in pieces {
            segments.push(Segment {
                start: text.len(),
                len: piece.len(),
                layout: None,
            });
            text.push_str(&piece);
            text.push_str(separator);
        }
        if !text.starts_with(&state.text)
            || text.get(..state.anchor.max(state.head))
                != state.text.get(..state.anchor.max(state.head))
        {
            state.anchor = 0;
            state.head = 0;
            state.dragging = false;
        }
        state.text = text;
        state.segments = segments;
        state.next = 0;
    }
    pub fn text(&self, text: StyledText, cx: &App) -> AnyElement {
        self.linked_text(text, Vec::new(), cx)
    }
    pub fn linked_text(
        &self,
        text: StyledText,
        links: Vec<(Range<usize>, String)>,
        cx: &App,
    ) -> AnyElement {
        let layout = text.layout().clone();
        let (index, start, len) = {
            let mut state = self.0.borrow_mut();
            let index = state.next;
            state.next += 1;
            let segment = &mut state.segments[index];
            segment.layout = Some(layout.clone());
            (index, segment.start, segment.len)
        };
        let paint = self.clone();
        let down = self.clone();
        let color = cx.theme().card.accent;
        div()
            .id(("output-text", index))
            .relative()
            .cursor(CursorStyle::IBeam)
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                let mut state = down.0.borrow_mut();
                let Some(index) = state.hit(event.position) else {
                    return;
                };
                if (event.modifiers.platform || event.modifiers.control)
                    && let Some((_, url)) = links
                        .iter()
                        .find(|(range, _)| range.contains(&index.saturating_sub(start)))
                {
                    cx.open_url(url);
                    cx.stop_propagation();
                    return;
                }
                state.focus.focus(window, cx);
                if !event.modifiers.shift {
                    state.anchor = index;
                }
                state.head = index;
                if event.click_count == 2 {
                    if let Some((start, end)) = state
                        .text
                        .unicode_word_indices()
                        .find(|(start, word)| *start <= index && index < *start + word.len())
                        .map(|(start, word)| (start, start + word.len()))
                    {
                        state.anchor = start;
                        state.head = end;
                    }
                } else if event.click_count >= 3 {
                    state.anchor = state.text[..index].rfind('\n').map_or(0, |i| i + 1);
                    state.head = state.text[index..]
                        .find('\n')
                        .map_or(state.text.len(), |i| index + i + 1);
                }
                state.dragging = true;
                cx.stop_propagation();
                window.refresh();
            })
            .child(
                canvas(
                    move |_, _, _| {},
                    move |_, _, window, _| {
                        let state = paint.0.borrow();
                        if !state.focus.is_focused(window) {
                            return;
                        }
                        let selected = state.range();
                        let selected = selected.start.max(start)..selected.end.min(start + len);
                        if selected.is_empty() {
                            return;
                        }
                        let selected = selected.start - start..selected.end - start;
                        let mut line_start = 0;
                        let mut y = layout.bounds().top();
                        for line in layout.line_layouts() {
                            let mut boundaries = vec![(0, px(0.))];
                            boundaries.extend(line.wrap_boundaries.iter().map(|boundary| {
                                let glyph = &line.unwrapped_layout.runs[boundary.run_ix].glyphs
                                    [boundary.glyph_ix];
                                (glyph.index, glyph.position.x)
                            }));
                            boundaries.push((line.len(), line.unwrapped_layout.width));
                            for row in boundaries.windows(2) {
                                let a = selected.start.max(line_start + row[0].0);
                                let b = selected.end.min(line_start + row[1].0);
                                if a < b {
                                    let left = line.unwrapped_layout.x_for_index(a - line_start);
                                    let right = line.unwrapped_layout.x_for_index(b - line_start);
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            point(layout.bounds().left() + left - row[0].1, y),
                                            size(right - left, layout.line_height()),
                                        ),
                                        color,
                                    ));
                                }
                                y += layout.line_height();
                            }
                            line_start += line.len() + 1;
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
            .child(text)
            .into_any_element()
    }
    pub fn wrap(&self, content: impl IntoElement) -> AnyElement {
        let focus = self.0.borrow().focus.clone();
        let copy = self.clone();
        let all = self.clone();
        let clear = self.clone();
        let events = self.clone();
        div()
            .key_context("OutputText")
            // Text, images, and blank space within the output body belong to
            // output interactions, never to the enclosing cell's drag gesture.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .track_focus(&focus)
            .min_w_0()
            .flex_1()
            .on_action(move |_: &Copy, _, cx| {
                let state = copy.0.borrow();
                if !state.range().is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        state.text[state.range()].to_owned(),
                    ));
                }
                cx.stop_propagation();
            })
            .on_action(move |_: &SelectAll, window, cx| {
                let mut state = all.0.borrow_mut();
                state.anchor = 0;
                state.head = state.text.len();
                window.refresh();
                cx.stop_propagation();
            })
            .on_action(move |_: &Clear, window, cx| {
                let mut state = clear.0.borrow_mut();
                state.anchor = state.head;
                state.dragging = false;
                window.refresh();
                cx.stop_propagation();
            })
            .child(
                canvas(
                    move |_, _, _| {},
                    move |_, _, window, _| {
                        let moving = events.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase != DispatchPhase::Bubble {
                                return;
                            }
                            let mut state = moving.0.borrow_mut();
                            if state.dragging {
                                if event.pressed_button != Some(MouseButton::Left) {
                                    state.dragging = false;
                                    return;
                                }
                                if let Some(index) = state.hit(event.position) {
                                    state.head = index;
                                }
                                window.refresh();
                                cx.stop_propagation();
                            }
                        });
                        let up = events.clone();
                        window.on_mouse_event(move |event: &MouseUpEvent, _, _, _| {
                            if event.button == MouseButton::Left {
                                up.0.borrow_mut().dragging = false;
                            }
                        });
                    },
                )
                .absolute()
                .size_0(),
            )
            .child(content)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Selection, init};
    use crate::{
        rich::Rich,
        theme::{ActiveTheme, Derivation, ThemeSpec},
    };
    use gpui::{
        AppContext, Context, MouseButton, Render, TestAppContext, VisualTestContext, Window, div,
        point, prelude::*, px, size,
    };

    struct Preview {
        rich: Rich,
        selection: Selection,
        scroll: gpui::ScrollHandle,
    }
    impl Render for Preview {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.selection.prepare(self.rich.selection_pieces());
            let t = cx.theme();
            div()
                .id("output-viewport")
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .size_full()
                .bg(t.app.bg)
                .text_color(t.app.fg)
                .font_family(t.font.mono.clone())
                .text_size(t.font.code)
                .line_height(px(24.))
                .child(self.selection.wrap(self.rich.render_selectable(
                    window,
                    cx,
                    &Default::default(),
                    Some(&self.selection),
                )))
        }
    }

    #[gpui::test]
    fn pointer_selection_copies_unicode_and_survives_append_but_not_replacement(
        cx: &mut TestAppContext,
    ) {
        for dark in [false, true] {
            let window = cx.update(|cx| {
                cx.set_global(
                    if dark {
                        ThemeSpec::dark()
                    } else {
                        ThemeSpec::light()
                    }
                    .resolve(&Derivation::default()),
                );
                init(cx);
                cx.open_window(Default::default(), |_, cx| {
                    cx.new(|cx| Preview {
                        rich: Rich::output(None, "first café 😀\nsecond\tline\n"),
                        selection: Selection::new(cx),
                        scroll: Default::default(),
                    })
                })
                .unwrap()
            });
            cx.simulate_window_resize(window.into(), size(px(400.), px(240.)));
            let view = cx.update(|cx| window.root(cx).unwrap());
            let mut visual = VisualTestContext::from_window(window.into(), cx);
            visual.run_until_parked();
            let selection = visual.update(|_, cx| view.read(cx).selection.clone());
            let positions = || {
                let state = selection.0.borrow();
                let layout = state.segments[0].layout.as_ref().unwrap();
                (
                    layout.position_for_index(6).unwrap() + point(px(0.2), px(8.)),
                    layout
                        .position_for_index("first café 😀\nsecond".len())
                        .unwrap()
                        + point(px(0.2), px(8.)),
                )
            };
            let (start, end) = positions();
            visual.simulate_mouse_down(start, MouseButton::Left, Default::default());
            visual.simulate_mouse_move(end, MouseButton::Left, Default::default());
            visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
            visual.simulate_keystrokes(if cfg!(target_os = "macos") {
                "cmd-c"
            } else {
                "ctrl-c"
            });
            visual.update(|_, cx| {
                assert_eq!(
                    cx.read_from_clipboard().unwrap().text().unwrap(),
                    "café 😀\nsecond"
                )
            });
            visual.simulate_input("must not edit");
            visual.simulate_keystrokes("backspace");
            assert_eq!(selection.0.borrow().text, "first café 😀\nsecond\tline\n");
            visual.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.rich = Rich::output(None, "first café 😀\nsecond\tline\nappended");
                    cx.notify();
                })
            });
            visual.run_until_parked();
            assert_eq!(
                &selection.0.borrow().text[selection.0.borrow().range()],
                "café 😀\nsecond"
            );
            visual.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.rich = Rich::output(None, "replacement 😀");
                    cx.notify();
                })
            });
            visual.run_until_parked();
            assert!(selection.0.borrow().range().is_empty());
            visual.simulate_keystrokes(if cfg!(target_os = "macos") {
                "cmd-a cmd-c"
            } else {
                "ctrl-a ctrl-c"
            });
            visual.update(|_, cx| {
                assert_eq!(
                    cx.read_from_clipboard().unwrap().text().unwrap(),
                    "replacement 😀"
                )
            });
        }
    }

    #[gpui::test]
    fn rich_table_selection_copies_tabs_and_rows_without_markup(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.set_global(ThemeSpec::light().resolve(&Derivation::default()));
            init(cx);
            cx.open_window(Default::default(), |_, cx| cx.new(|cx| Preview {
                rich: Rich::output(Some(&serde_json::from_value(serde_json::json!({"text/html": "<p>Hello <code>café</code></p><table><tr><th>Name</th><th>Value</th></tr><tr><td>😀</td><td>42</td></tr></table>"})).unwrap()), "fallback"),
                selection: Selection::new(cx),
                        scroll: Default::default(),
            })).unwrap()
        });
        let view = cx.update(|cx| window.root(cx).unwrap());
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let selection = visual.update(|_, cx| view.read(cx).selection.clone());
        let (start, end) = {
            let state = selection.0.borrow();
            (
                state.segments[0].layout.as_ref().unwrap().bounds().origin + point(px(0.2), px(8.)),
                state
                    .segments
                    .last()
                    .unwrap()
                    .layout
                    .as_ref()
                    .unwrap()
                    .position_for_index(2)
                    .unwrap()
                    + point(px(0.2), px(8.)),
            )
        };
        visual.simulate_mouse_down(start, MouseButton::Left, Default::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Default::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
        visual.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-c"
        } else {
            "ctrl-c"
        });
        visual.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().unwrap(),
                "Hello café\nName\tValue\n😀\t42"
            )
        });
    }
    #[gpui::test]
    fn selection_extends_through_wrapping_and_scrolling_and_releases_outside(
        cx: &mut TestAppContext,
    ) {
        let source = (0..80)
            .map(|n| format!("row {n} café 😀 with words that wrap across the viewport\n"))
            .collect::<String>();
        let window = cx.update(|cx| {
            cx.set_global(ThemeSpec::dark().resolve(&Derivation::default()));
            init(cx);
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| Preview {
                    rich: Rich::output(None, &source),
                    selection: Selection::new(cx),
                    scroll: Default::default(),
                })
            })
            .unwrap()
        });
        cx.simulate_window_resize(window.into(), size(px(220.), px(180.)));
        let view = cx.update(|cx| window.root(cx).unwrap());
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let selection = visual.update(|_, cx| view.read(cx).selection.clone());
        let start = selection.position_for_index(0) + point(px(0.2), px(6.));
        visual.simulate_mouse_down(start, MouseButton::Left, Default::default());
        visual.simulate_event(gpui::ScrollEvent {
            position: point(px(100.), px(100.)),
            delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-300.))),
            ..Default::default()
        });
        visual.run_until_parked();
        visual.update(|_, cx| assert!(view.read(cx).scroll.offset().y < px(0.)));
        let end = point(px(100.), px(140.));
        visual.simulate_mouse_move(end, MouseButton::Left, Default::default());
        let selected = {
            let state = selection.0.borrow();
            assert!(
                state.head > 80,
                "drag should reach text revealed by scrolling"
            );
            assert_eq!(state.anchor, 0);
            source[state.range()].to_owned()
        };
        visual.simulate_mouse_up(
            point(px(500.), px(500.)),
            MouseButton::Left,
            Default::default(),
        );
        assert!(!selection.0.borrow().dragging);
        visual.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-c"
        } else {
            "ctrl-c"
        });
        visual.update(|_, cx| {
            assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), selected)
        });
        visual.simulate_keystrokes("escape");
        assert!(selection.0.borrow().range().is_empty());
    }
}
