//! Read-only output actions. Menu focus never changes the cell selection.
use super::*;

gpui::actions!(
    output_menu,
    [Previous, Next, First, Last, Activate, Dismiss]
);
pub(super) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", Previous, Some("OutputMenu")),
        KeyBinding::new("down", Next, Some("OutputMenu")),
        KeyBinding::new("home", First, Some("OutputMenu")),
        KeyBinding::new("end", Last, Some("OutputMenu")),
        KeyBinding::new("enter", Activate, Some("OutputMenu")),
        KeyBinding::new("space", Activate, Some("OutputMenu")),
        KeyBinding::new("escape", Dismiss, Some("OutputMenu")),
        KeyBinding::new("tab", Dismiss, Some("OutputMenu")),
    ]);
}

pub(super) struct Menu {
    pub focus: FocusHandle,
    pub open: Option<OpenMenu>,
}
pub(super) struct OpenMenu {
    position: Point<Pixels>,
    selected: Option<usize>,
    // Capture the representation of the output that was right-clicked, even
    // if subsequent execution replaces or appends output while the menu is open.
    fallback: Option<String>,
}
impl Menu {
    pub fn new(cx: &mut App) -> Self {
        Self {
            focus: cx.focus_handle(),
            open: None,
        }
    }
}

impl CodeCell {
    pub(super) fn open_output_menu(
        &mut self,
        index: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(output) = self.output.get(index) else {
            return;
        };
        let fallback = output
            .data
            .as_ref()
            .and_then(|data| {
                ["text/plain", "text/latex"]
                    .iter()
                    .find_map(|mime| data.get(*mime).and_then(crate::rich::mime_text))
            })
            .map(|text| crate::rich::plain_text(&text))
            .filter(|text| !text.is_empty());
        self.output_menu.open = Some(OpenMenu {
            position,
            selected: None,
            fallback,
        });
        let selected = (0..1).find(|index| self.output_menu_enabled(*index));
        self.output_menu.open.as_mut().unwrap().selected = selected;
        self.menu_open = false;
        self.output_selection.set_menu_open(true);
        self.output_menu.focus.focus(window, cx);
        cx.notify();
    }

    fn output_menu_enabled(&self, index: usize) -> bool {
        match index {
            0 => {
                self.output_selection.has_text()
                    || self
                        .output_menu
                        .open
                        .as_ref()
                        .is_some_and(|menu| menu.fallback.is_some())
            }
            _ => false,
        }
    }

    pub(super) fn dismiss_output_menu(
        &mut self,
        restore: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.output_menu.open = None;
        self.output_selection.set_menu_open(false);
        if restore && self.output_menu.focus.is_focused(window) {
            if self.output.is_empty() {
                self.cell_focus.focus(window, cx);
            } else {
                self.output_selection.focus(window, cx);
            }
        }
        cx.notify();
    }

    fn activate_output_menu(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.output_menu.open.is_none() || !self.output_menu_enabled(index) {
            return;
        }
        if self.output_selection.has_text() {
            self.output_selection.copy_all(cx);
        } else if let Some(text) = self
            .output_menu
            .open
            .as_ref()
            .and_then(|menu| menu.fallback.clone())
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        self.dismiss_output_menu(true, window, cx);
    }

    fn output_menu_command(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = &self.output_menu.open else {
            return;
        };
        let selected = menu.selected;
        match key {
            "escape" | "tab" => self.dismiss_output_menu(true, window, cx),
            "enter" | "space" => {
                if let Some(index) = selected {
                    self.activate_output_menu(index, window, cx);
                }
            }
            "up" | "down" | "home" | "end" => {
                let enabled: Vec<_> = (0..1)
                    .filter(|index| self.output_menu_enabled(*index))
                    .collect();
                let current = enabled.iter().position(|index| Some(*index) == selected);
                let next = match key {
                    "home" => enabled.first().copied(),
                    "end" => enabled.last().copied(),
                    "up" => current
                        .map(|i| enabled[(i + enabled.len() - 1) % enabled.len()])
                        .or_else(|| enabled.last().copied()),
                    _ => current
                        .map(|i| enabled[(i + 1) % enabled.len()])
                        .or_else(|| enabled.first().copied()),
                };
                self.output_menu.open.as_mut().unwrap().selected = next;
                cx.notify();
            }
            _ => {}
        }
        // Do not route typing or notebook/editor shortcuts through the menu.
        window.prevent_default();
        cx.stop_propagation();
    }

    pub(super) fn render_output_menu(&self, cx: &Context<Self>) -> Option<gpui::AnyElement> {
        let menu = self.output_menu.open.as_ref()?;
        let t = cx.theme();
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position(menu.position)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .id("output-context-menu")
                            .debug_selector(|| "output-context-menu".into())
                            .occlude()
                            .track_focus(&self.output_menu.focus)
                            .key_context("OutputMenu OutputText")
                            .w(px(220.))
                            .p_1()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .rounded(t.radius.md())
                            .border_1()
                            .border_color(t.popover.border)
                            .bg(t.popover.bg)
                            .text_color(t.popover.fg)
                            .font_family(t.font.sans.clone())
                            .text_size(t.font.sm)
                            .shadow_lg()
                            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                                this.dismiss_output_menu(true, window, cx)
                            }))
                            .on_action(cx.listener(|this, _: &Previous, w, cx| {
                                this.output_menu_command("up", w, cx)
                            }))
                            .on_action(cx.listener(|this, _: &Next, w, cx| {
                                this.output_menu_command("down", w, cx)
                            }))
                            .on_action(cx.listener(|this, _: &First, w, cx| {
                                this.output_menu_command("home", w, cx)
                            }))
                            .on_action(cx.listener(|this, _: &Last, w, cx| {
                                this.output_menu_command("end", w, cx)
                            }))
                            .on_action(cx.listener(|this, _: &Activate, w, cx| {
                                this.output_menu_command("enter", w, cx)
                            }))
                            .on_action(cx.listener(|this, _: &Dismiss, w, cx| {
                                this.output_menu_command("escape", w, cx)
                            }))
                            .on_action(cx.listener(
                                |this, _: &crate::output_selection::Clear, w, cx| {
                                    this.output_menu_command("escape", w, cx)
                                },
                            ))
                            .on_action(cx.listener(|this, _: &Copy, w, cx| {
                                this.activate_output_menu(0, w, cx);
                                cx.stop_propagation();
                            }))
                            .on_action(|_: &SelectAll, _, cx| cx.stop_propagation())
                            .on_key_down(|_, window, cx| {
                                window.prevent_default();
                                cx.stop_propagation();
                            })
                            .children(["Copy output"].into_iter().enumerate().map(
                                |(index, label)| {
                                    let enabled = self.output_menu_enabled(index);
                                    let selected = menu.selected == Some(index) && enabled;
                                    div()
                                        .id(("output-menu-action", index))
                                        .debug_selector(move || {
                                            format!("output-menu-action-{index}")
                                        })
                                        .px_2()
                                        .py_1()
                                        .rounded(t.radius.sm())
                                        .when(selected, |row| {
                                            row.bg(t.popover.accent).text_color(t.popover.accent_fg)
                                        })
                                        .when(!enabled, |row| row.opacity(0.5).cursor_default())
                                        .when(enabled, |row| {
                                            row.cursor_pointer()
                                                .on_hover(cx.listener(
                                                    move |this, hovered, _, cx| {
                                                        if *hovered
                                                            && let Some(menu) =
                                                                &mut this.output_menu.open
                                                        {
                                                            menu.selected = Some(index);
                                                            cx.notify();
                                                        }
                                                    },
                                                ))
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.activate_output_menu(
                                                            index, window, cx,
                                                        );
                                                        cx.stop_propagation();
                                                    },
                                                ))
                                        })
                                        .child(label)
                                },
                            )),
                    ),
            )
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn copy_output_copies_all_text_without_editing_source(cx: &mut gpui::TestAppContext) {
        let window = cx.update(|cx| {
            cx.set_global(
                crate::theme::ThemeSpec::light().resolve(&crate::theme::Derivation::default()),
            );
            crate::code_cell::init(cx);
            crate::output_selection::init(cx);
            cx.open_window(Default::default(), |window, cx| {
                cx.new(|cx| {
                    let mut cell = CodeCell::new(window, cx);
                    cell.buffer.text = "unchanged".into();
                    cell.set_output(vec![crate::execution::OutputBlock {
                        text: "café 😀\nsecond line\n".into(),
                        data: None,
                        execution_count: None,
                    }]);
                    cell
                })
            })
            .unwrap()
        });
        let cell = cx.update(|cx| window.root(cx).unwrap());
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let position = visual.debug_bounds("cell-output-0").unwrap().center();
        visual.simulate_mouse_down(position, MouseButton::Right, Default::default());
        visual.simulate_mouse_up(position, MouseButton::Right, Default::default());
        assert!(visual.debug_bounds("output-menu-action-0").is_some());
        assert!(visual.debug_bounds("output-menu-action-1").is_none());
        visual.simulate_keystrokes("x backspace shift-enter enter");
        visual.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().unwrap(),
                "café 😀\nsecond line\n"
            );
            assert_eq!(cell.read(cx).text(), "unchanged");
            assert!(cell.read(cx).output_menu.open.is_none());
        });
    }
}
