//! Ephemeral kernel input. Values never enter the notebook or output history.
use super::*;
use gpui::{ClipboardItem, ContentMask, canvas};

pub(super) struct StdinPrompt {
    pub id: String,
    prompt: String,
    password: bool,
    session: crate::execution::Session,
    generation: u64,
    buffer: Buffer,
    marked: Option<Range<usize>>,
    focus: FocusHandle,
    parent_focus: FocusHandle,
    focus_pending: bool,
    reveal: bool,
    submitting: bool,
    error: Option<String>,
    line: Option<ShapedLine>,
    bounds: Option<Bounds<Pixels>>,
    scroll_x: Pixels,
    selecting: bool,
    undo: Vec<Buffer>,
    redo: Vec<Buffer>,
}
impl StdinPrompt {
    pub fn new(
        input: ganymede_core::notebook::PendingInput,
        session: crate::execution::Session,
        parent_focus: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            id: input.id,
            prompt: input.prompt,
            password: input.password,
            generation: session.input_generation(),
            session,
            buffer: Buffer::default(),
            marked: None,
            focus: cx.focus_handle(),
            parent_focus,
            focus_pending: true,
            reveal: true,
            submitting: false,
            error: None,
            line: None,
            bounds: None,
            scroll_x: px(0.),
            selecting: false,
            undo: vec![],
            redo: vec![],
        }
    }
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.submitting || self.marked.is_some() {
            return;
        }
        self.submitting = true;
        self.error = None;
        let value = std::mem::take(&mut self.buffer.text);
        self.buffer = Buffer::default();
        self.undo.clear();
        self.redo.clear();
        self.line = None;
        self.scroll_x = px(0.);
        let task = self
            .session
            .reply_input(self.id.clone(), value, self.generation);
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Input request interrupted".into()));
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    this.submitting = false;
                    this.error = Some(error);
                }
                cx.notify();
            });
        })
        .detach();
        // Keep keyboard focus until the matching pending request disappears.
        self.focus.focus(window, cx);
        cx.notify();
    }
    fn select(&mut self, offset: usize, extend: bool, cx: &mut Context<Self>) {
        if self.submitting {
            return;
        }
        self.buffer.select(offset, extend);
        self.reveal = true;
        self.marked = None;
        cx.notify();
    }
    fn edit(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        if self.submitting {
            return;
        }
        self.reveal = true;
        self.undo.push(self.buffer.clone());
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.buffer.replace(range, &text.replace(['\r', '\n'], " "));
        self.marked = None;
        cx.notify();
    }
    fn display_offset(&self, offset: usize) -> usize {
        if self.password {
            self.buffer.text[..offset].graphemes(true).count() * '•'.len_utf8()
        } else {
            offset
        }
    }
    fn source_offset(&self, offset: usize) -> usize {
        if self.password {
            self.buffer
                .text
                .grapheme_indices(true)
                .nth(offset / '•'.len_utf8())
                .map_or(self.buffer.text.len(), |(i, _)| i)
        } else {
            offset.min(self.buffer.text.len())
        }
    }
    fn index_at(&self, point: Point<Pixels>) -> usize {
        self.line
            .as_ref()
            .zip(self.bounds)
            .map_or(0, |(line, bounds)| {
                self.source_offset(
                    line.closest_index_for_x(point.x - bounds.left() + self.scroll_x),
                )
            })
    }
    fn copy(&self, cx: &mut Context<Self>) {
        if !self.password && !self.buffer.selection().is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.buffer.text[self.buffer.selection()].to_owned(),
            ));
        }
    }
}
impl Focusable for StdinPrompt {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl EntityInputHandler for StdinPrompt {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.buffer.utf16_range(range);
        *actual = Some(self.buffer.to_utf16(range.clone()));
        Some(if self.password {
            "•".repeat(self.buffer.text[range].encode_utf16().count())
        } else {
            self.buffer.text[range].to_owned()
        })
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        (!self.submitting).then(|| UTF16Selection {
            range: self.buffer.to_utf16(self.buffer.selection()),
            reversed: self.buffer.head < self.buffer.anchor,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.clone().map(|r| self.buffer.to_utf16(r))
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|r| self.buffer.utf16_range(r))
            .or(self.marked.clone())
            .unwrap_or_else(|| self.buffer.selection());
        self.edit(range, text, cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.submitting {
            return;
        }
        let range = range
            .map(|r| self.buffer.utf16_range(r))
            .or(self.marked.clone())
            .unwrap_or_else(|| self.buffer.selection());
        let text = text.replace(['\r', '\n'], " ");
        self.reveal = true;
        self.buffer.replace(range.clone(), &text);
        self.marked = (!text.is_empty()).then_some(range.start..range.start + text.len());
        if let Some(selection) = selection {
            self.buffer.anchor = range.start + from_utf16(&text, selection.start);
            self.buffer.head = range.start + from_utf16(&text, selection.end);
        }
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.buffer.utf16_range(range);
        let bounds = self.bounds?;
        let line = self.line.as_ref()?;
        let left = line.x_for_index(self.display_offset(range.start));
        let right = line.x_for_index(self.display_offset(range.end));
        Some(Bounds::new(
            point(bounds.left() + left - self.scroll_x, bounds.top()),
            size((right - left).max(px(1.)), bounds.size.height),
        ))
    }
    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let index = self.index_at(point);
        Some(self.buffer.to_utf16(index..index).start)
    }
}
impl Render for StdinPrompt {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_pending {
            self.focus_pending = false;
            self.focus.focus(window, cx);
        }
        let t = cx.theme();
        let entity = cx.entity();
        let paint_entity = entity.clone();
        let input = div()
            .id("stdin-input")
            .key_context("CodeCell")
            .track_focus(&self.focus)
            .aria_label(if self.password {
                "Kernel password input"
            } else {
                "Kernel input"
            })
            .flex_1()
            .min_w_0()
            .max_w(px(420.))
            .h(px(28.))
            .px_2()
            .py_1()
            .font_family(t.font.mono.clone())
            .text_size(t.font.code)
            .bg(t.card.bg)
            .text_color(t.card.fg)
            .border_1()
            .border_color(t.card.border)
            .rounded(t.radius.md())
            .focus(|s| s.border_color(t.card.ring))
            .cursor(gpui::CursorStyle::IBeam)
            .on_action(cx.listener(|this, _: &Enter, window, cx| this.submit(window, cx)))
            .on_action(cx.listener(|this, _: &RunAndAdvance, window, cx| this.submit(window, cx)))
            .on_action(
                cx.listener(|this, _: &SelectCell, window, cx| this.parent_focus.focus(window, cx)),
            )
            .on_action(cx.listener(|_, _: &Tab, window, cx| window.focus_next(cx)))
            .on_action(cx.listener(|_, _: &Outdent, window, cx| window.focus_prev(cx)))
            .on_action(cx.listener(|this, _: &Left, _, cx| {
                this.select(
                    if this.buffer.selection().is_empty() {
                        this.buffer.previous()
                    } else {
                        this.buffer.selection().start
                    },
                    false,
                    cx,
                )
            }))
            .on_action(cx.listener(|this, _: &Right, _, cx| {
                this.select(
                    if this.buffer.selection().is_empty() {
                        this.buffer.next()
                    } else {
                        this.buffer.selection().end
                    },
                    false,
                    cx,
                )
            }))
            .on_action(cx.listener(|this, _: &SelectLeft, _, cx| {
                this.select(this.buffer.previous(), true, cx)
            }))
            .on_action(
                cx.listener(|this, _: &SelectRight, _, cx| {
                    this.select(this.buffer.next(), true, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &Up, _, cx| this.select(0, false, cx)))
            .on_action(
                cx.listener(|this, _: &Down, _, cx| this.select(this.buffer.text.len(), false, cx)),
            )
            .on_action(cx.listener(|this, _: &SelectUp, _, cx| this.select(0, true, cx)))
            .on_action(cx.listener(|this, _: &SelectDown, _, cx| {
                this.select(this.buffer.text.len(), true, cx)
            }))
            .on_action(cx.listener(|this, _: &Home, _, cx| this.select(0, false, cx)))
            .on_action(
                cx.listener(|this, _: &End, _, cx| this.select(this.buffer.text.len(), false, cx)),
            )
            .on_action(cx.listener(|this, _: &SelectHome, _, cx| this.select(0, true, cx)))
            .on_action(cx.listener(|this, _: &SelectEnd, _, cx| {
                this.select(this.buffer.text.len(), true, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                this.buffer.anchor = 0;
                this.select(this.buffer.text.len(), true, cx);
            }))
            .on_action(cx.listener(|this, _: &Backspace, _, cx| {
                let mut r = this.buffer.selection();
                if r.is_empty() {
                    r.start = this.buffer.previous();
                }
                this.edit(r, "", cx);
            }))
            .on_action(cx.listener(|this, _: &Delete, _, cx| {
                let mut r = this.buffer.selection();
                if r.is_empty() {
                    r.end = this.buffer.next();
                }
                this.edit(r, "", cx);
            }))
            .on_action(cx.listener(|this, _: &Copy, _, cx| this.copy(cx)))
            .on_action(cx.listener(|this, _: &Cut, _, cx| {
                this.copy(cx);
                this.edit(this.buffer.selection(), "", cx);
            }))
            .on_action(cx.listener(|this, _: &Paste, _, cx| {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    this.edit(this.buffer.selection(), &text, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Undo, _, cx| {
                if !this.submitting
                    && let Some(previous) = this.undo.pop()
                {
                    this.redo
                        .push(std::mem::replace(&mut this.buffer, previous));
                    this.marked = None;
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &Redo, _, cx| {
                if !this.submitting
                    && let Some(next) = this.redo.pop()
                {
                    this.undo.push(std::mem::replace(&mut this.buffer, next));
                    this.marked = None;
                    cx.notify();
                }
            }))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.focus.focus(window, cx);
                    this.selecting = true;
                    this.select(this.index_at(event.position), event.modifiers.shift, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if this.selecting {
                    this.select(this.index_at(event.position), true, cx);
                }
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, _| this.selecting = false),
            )
            .on_mouse_up_out(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, _| this.selecting = false),
            )
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let this = entity.read(cx);
                        if this.reveal {
                            window.request_autoscroll(bounds);
                        }
                        let display = if this.password {
                            "•".repeat(this.buffer.text.graphemes(true).count())
                        } else {
                            this.buffer.text.clone()
                        };
                        let style = window.text_style();
                        let font_size = style.font_size.to_pixels(window.rem_size());
                        let len = display.len();
                        let line = window.text_system().shape_line(
                            display.into(),
                            font_size,
                            &[style.to_run(len)],
                            None,
                        );
                        (line, bounds)
                    },
                    move |_, (line, bounds), window, cx| {
                        let focus = paint_entity.read(cx).focus.clone();
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, paint_entity.clone()),
                            cx,
                        );
                        paint_entity.update(cx, |this, cx| {
                            let caret = line.x_for_index(this.display_offset(this.buffer.head));
                            this.scroll_x = this
                                .scroll_x
                                .min(caret)
                                .max(caret - bounds.size.width + px(2.))
                                .max(px(0.));
                            let origin = point(bounds.left() - this.scroll_x, bounds.top());
                            let surface = cx.theme().card;
                            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                                let selection = this.buffer.selection();
                                if !selection.is_empty() {
                                    let left =
                                        line.x_for_index(this.display_offset(selection.start));
                                    let right =
                                        line.x_for_index(this.display_offset(selection.end));
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            point(origin.x + left, origin.y),
                                            size(right - left, bounds.size.height),
                                        ),
                                        surface.accent,
                                    ));
                                }
                                let _ = line.paint(
                                    origin,
                                    bounds.size.height,
                                    gpui::TextAlign::Left,
                                    None,
                                    window,
                                    cx,
                                );
                                if this.focus.is_focused(window) && !this.submitting {
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            point(origin.x + caret, origin.y + px(1.)),
                                            size(px(1.5), bounds.size.height - px(2.)),
                                        ),
                                        surface.fg,
                                    ));
                                }
                            });
                            if window.content_mask().bounds.size.width > px(0.) {
                                this.reveal = false;
                            }
                            this.line = Some(line);
                            this.bounds = Some(bounds);
                        });
                    },
                )
                .size_full(),
            );
        div()
            .flex()
            .flex_col()
            .gap_1()
            .min_w_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(t.font.code)
                            .font_family(t.font.mono.clone())
                            .text_color(t.app.fg)
                            .child(if self.prompt.is_empty() {
                                "Input:".to_owned()
                            } else {
                                self.prompt.clone()
                            }),
                    )
                    .child(input)
                    .child(
                        div()
                            .text_size(t.font.xs)
                            .text_color(t.app.muted_fg)
                            .child(if self.submitting { "Sending…" } else { "↵" }),
                    ),
            )
            .when_some(self.error.clone(), |v, error| {
                v.child(
                    div()
                        .text_size(t.font.sm)
                        .text_color(t.destructive)
                        .child(error),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn password_masks_unicode_geometry_accessibility_and_clipboard(cx: &mut gpui::TestAppContext) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let cx = cx.add_empty_window();
        let input = cx.update(|_, cx| {
            cx.set_global(
                crate::theme::ThemeSpec::light().resolve(&crate::theme::Derivation::default()),
            );
            crate::assets::register_fonts(cx).unwrap();
            let config = ganymede_core::jupyter::ServerConfig::new("http://127.0.0.1:1").unwrap();
            let controller = ganymede_core::notebook::NotebookController::new(
                config.clone(),
                "input.ipynb",
                ganymede_core::notebook::NotebookDocument::new(),
            )
            .unwrap();
            let id = controller
                .insert_cell(0, ganymede_core::notebook::CellKind::Code, "")
                .unwrap();
            let session = crate::execution::Session::from_controller(
                controller,
                &crate::execution::Runtime {
                    handle: runtime.handle().clone(),
                    config: Ok(config),
                    kernel: "python3".into(),
                },
            );
            let parent = cx.focus_handle();
            cx.new(|cx| {
                StdinPrompt::new(
                    ganymede_core::notebook::PendingInput {
                        id: "request".into(),
                        cell_id: id,
                        prompt: "Password:".into(),
                        password: true,
                    },
                    session,
                    parent,
                    cx,
                )
            })
        });
        input.update(cx, |input, cx| {
            input.edit(0..0, "é😀e\u{301}", cx);
            assert_eq!(input.display_offset(input.buffer.text.len()), 9);
            assert_eq!(input.source_offset(3), "é".len());
            input.buffer.anchor = 0;
            cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
            input.copy(cx);
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().unwrap(),
                "unchanged"
            );
        });
        cx.draw(point(px(0.), px(0.)), size(px(600.), px(40.)), |_, _| {
            input.clone().into_any_element()
        });
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                assert_eq!(input.line.as_ref().unwrap().text.as_ref(), "•••");
                let mut actual = None;
                assert_eq!(
                    input.text_for_range(0..5, &mut actual, window, cx).unwrap(),
                    "•••••"
                );
                input.submitting = true;
                let before = input.buffer.text.clone();
                input.replace_text_in_range(None, "no", window, cx);
                assert_eq!(input.buffer.text, before);
            })
        });
    }
}
