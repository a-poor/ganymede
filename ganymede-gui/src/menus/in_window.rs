//! Linux menus reuse native action definitions and return focus before dispatch.
use super::*;
use crate::{icon::Icon, theme::ActiveTheme};
use gpui::{
    Action, Anchor, FocusHandle, MouseButton, ScrollHandle, SharedString, Window, anchored,
    deferred, div, point, px,
};
use std::collections::HashSet;

gpui::actions!(window_menu, [OpenMenu]);

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Workspace,
    Connection,
    Settings,
}

struct Row {
    label: SharedString,
    action: Option<Box<dyn Action>>,
    disabled: bool,
    enabled: bool,
    shortcut: String,
}
struct Group {
    label: SharedString,
    rows: Vec<Row>,
}

fn flatten(items: Vec<MenuItem>, prefix: &str, disabled: bool, rows: &mut Vec<Row>) {
    for item in items {
        match item {
            MenuItem::Action {
                name,
                action,
                disabled: item_disabled,
                ..
            } => rows.push(Row {
                label: format!("{prefix}{name}").into(),
                action: Some(action),
                disabled: disabled || item_disabled,
                enabled: false,
                shortcut: String::new(),
            }),
            MenuItem::Separator => rows.push(Row {
                label: "".into(),
                action: None,
                disabled: true,
                enabled: false,
                shortcut: String::new(),
            }),
            MenuItem::Submenu(menu) => flatten(
                menu.items,
                &format!("{prefix}{} ", menu.name),
                disabled || menu.disabled,
                rows,
            ),
            MenuItem::SystemMenu(_) => {}
        }
    }
}

fn groups(kind: Kind) -> Vec<Group> {
    use MenuItem as I;
    let menus = if kind == Kind::Workspace {
        super::definitions()
            .into_iter()
            .filter(|m| m.name != "Ganymede")
            .map(|mut menu| {
                if menu.name == "File" {
                    menu.items.extend([
                        I::separator(),
                        I::action("Settings…", OpenSettings),
                        I::action("Quit Ganymede", crate::Quit),
                    ]);
                }
                if menu.name == "Help" {
                    menu.items.extend([
                        I::separator(),
                        I::action("Check for Updates…", CheckForUpdates),
                        I::action("About Ganymede", About),
                    ]);
                }
                menu
            })
            .collect()
    } else {
        let mut items = vec![I::action("New Connection…", crate::NewWindow)];
        if kind == Kind::Connection {
            items.push(I::action("Settings…", OpenSettings));
        }
        items.extend([
            I::separator(),
            I::action("Check for Updates…", CheckForUpdates),
            I::action("Ganymede Documentation", Documentation),
            I::action("Report an Issue…", ReportIssue),
            I::action("About Ganymede", About),
            I::separator(),
            I::action("Quit Ganymede", crate::Quit),
        ]);
        vec![Menu::new("Application menu").items(items)]
    };
    menus
        .into_iter()
        .map(|menu| {
            let mut rows = Vec::new();
            flatten(menu.items, "", menu.disabled, &mut rows);
            Group {
                label: menu.name,
                rows,
            }
        })
        .collect()
}

pub struct WindowMenu {
    kind: Kind,
    groups: Vec<Group>,
    open: Option<usize>,
    selected: Option<usize>,
    focus: FocusHandle,
    return_focus: Option<FocusHandle>,
    scroll: ScrollHandle,
    trigger_bounds: gpui::Bounds<gpui::Pixels>,
}

impl WindowMenu {
    pub fn new(kind: Kind, cx: &mut Context<Self>) -> Self {
        Self {
            kind,
            groups: groups(kind),
            open: None,
            selected: None,
            focus: cx.focus_handle(),
            return_focus: None,
            scroll: ScrollHandle::new(),
            trigger_bounds: gpui::Bounds::default(),
        }
    }

    pub fn open_first(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_group(0, window, cx);
    }

    fn open_group(&mut self, group: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.open.is_none() {
            self.return_focus = window.focused(cx);
            let available: HashSet<_> = window
                .available_actions(cx)
                .iter()
                .map(|a| a.name().to_owned())
                .collect();
            for group in &mut self.groups {
                for row in &mut group.rows {
                    row.enabled = !row.disabled
                        && row
                            .action
                            .as_ref()
                            .is_some_and(|a| available.contains(a.name()));
                    row.shortcut = row
                        .action
                        .as_ref()
                        .and_then(|a| window.highest_precedence_binding_for_action(a.as_ref()))
                        .map(|binding| {
                            binding
                                .keystrokes()
                                .iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                        .unwrap_or_default();
                }
            }
        }
        self.open = Some(group);
        self.selected = self.groups[group].rows.iter().position(|row| row.enabled);
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = None;
        self.selected = None;
        if self.focus.is_focused(window) {
            if let Some(focus) = self.return_focus.take() {
                focus.focus(window, cx);
            } else {
                window.blur();
            }
        }
        cx.notify();
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(group) = self.open else { return };
        let row = &self.groups[group].rows[index];
        if !row.enabled {
            return;
        }
        let action = row.action.as_ref().unwrap().boxed_clone();
        self.dismiss(window, cx);
        window.dispatch_action(action, cx);
    }

    fn key(&mut self, event: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(group) = self.open else { return };
        match event.keystroke.key.as_str() {
            "escape" | "tab" => self.dismiss(window, cx),
            "enter" | "space" => {
                if let Some(index) = self.selected {
                    self.activate(index, window, cx);
                }
            }
            "left" | "right" => {
                let len = self.groups.len();
                let next = if event.keystroke.key == "left" {
                    (group + len - 1) % len
                } else {
                    (group + 1) % len
                };
                self.open_group(next, window, cx);
            }
            "up" | "down" | "home" | "end" => {
                let enabled: Vec<_> = self.groups[group]
                    .rows
                    .iter()
                    .enumerate()
                    .filter_map(|(i, r)| r.enabled.then_some(i))
                    .collect();
                if !enabled.is_empty() {
                    let pos = enabled
                        .iter()
                        .position(|i| Some(*i) == self.selected)
                        .unwrap_or(0);
                    let pos = match event.keystroke.key.as_str() {
                        "home" => 0,
                        "end" => enabled.len() - 1,
                        "up" => (pos + enabled.len() - 1) % enabled.len(),
                        _ => (pos + 1) % enabled.len(),
                    };
                    self.selected = Some(enabled[pos]);
                    self.scroll.scroll_to_item(enabled[pos]);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl WindowMenu {
    fn row(&self, group: usize, index: usize, cx: &Context<Self>) -> gpui::AnyElement {
        let t = cx.theme();
        let row = &self.groups[group].rows[index];
        if row.action.is_none() {
            return div()
                .my_1()
                .h(px(1.))
                .bg(t.popover.border)
                .into_any_element();
        }
        div()
            .id(("menu-row", index))
            .debug_selector(|| format!("menu-item-{}", row.label))
            .h(px(26.))
            .px_2()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .rounded(t.radius.sm())
            .aria_label(row.label.clone())
            .when(!row.enabled, |v| v.opacity(0.45))
            .when(row.enabled, |v| {
                v.cursor_pointer()
                    .when(self.selected == Some(index), |v| {
                        v.bg(t.popover.accent).text_color(t.popover.accent_fg)
                    })
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if this.selected != Some(index) {
                            this.selected = Some(index);
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.activate(index, window, cx);
                    }))
            })
            .child(row.label.clone())
            .child(
                div()
                    .text_color(t.popover.muted_fg)
                    .child(row.shortcut.clone()),
            )
            .into_any_element()
    }

    fn popup(&self, group: usize, window: &Window, cx: &Context<Self>) -> gpui::AnyElement {
        let t = cx.theme();
        let compact = self.kind != Kind::Workspace;
        let panel = div()
            .id("menu-popup")
            .debug_selector(|| "menu-popup".into())
            .occlude()
            .w(px(280.))
            .max_h((window.viewport_size().height - px(80.)).max(px(80.)))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .p_1()
            .rounded(t.radius.md())
            .border_1()
            .border_color(t.popover.border)
            .bg(t.popover.bg)
            .text_color(t.popover.fg)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_mouse_down_out(
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    if !this.trigger_bounds.contains(&event.position) {
                        this.dismiss(window, cx);
                    }
                }),
            )
            .children((0..self.groups[group].rows.len()).map(|index| self.row(group, index, cx)));
        div()
            .absolute()
            .top_full()
            .when(compact, |v| v.right_0())
            .when(!compact, |v| v.left_0())
            .child(
                deferred(
                    anchored()
                        .anchor(if compact {
                            Anchor::TopRight
                        } else {
                            Anchor::TopLeft
                        })
                        .offset(point(px(0.), px(3.)))
                        .snap_to_window_with_margin(px(6.))
                        .child(panel),
                )
                .with_priority(1),
            )
            .into_any_element()
    }

    fn trigger(&self, index: usize, window: &Window, cx: &Context<Self>) -> gpui::Div {
        let t = cx.theme();
        let compact = self.kind != Kind::Workspace;
        let open = self.open == Some(index);
        let group = &self.groups[index];
        let trigger = div()
            .id(("menu-trigger", index))
            .aria_label(group.label.clone())
            .debug_selector(|| format!("menu-trigger-{}", group.label))
            .h(px(28.))
            .px_2()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .rounded(t.radius.sm())
            .text_color(t.app.fg)
            .when(compact, |v| {
                v.w(px(28.))
                    .p_0()
                    .rounded_full()
                    .border_1()
                    .border_color(t.app.border)
                    .bg(t.app.bg)
                    .text_color(t.app.fg)
                    .shadow(vec![t.body_shadow.clone()])
            })
            .when(open, |v| v.bg(t.app.accent).text_color(t.app.accent_fg))
            .hover(|s| s.bg(t.app.accent).text_color(t.app.accent_fg))
            .active(|s| s.bg(t.app.accent))
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                cx.stop_propagation();
                window.prevent_default();
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                if this.open == Some(index) {
                    this.dismiss(window, cx);
                } else {
                    this.open_group(index, window, cx);
                }
            }))
            .on_mouse_move(cx.listener(move |this, _, window, cx| {
                if this.open.is_some() && this.open != Some(index) {
                    this.open_group(index, window, cx);
                }
            }))
            .child(if compact {
                Icon::List
                    .svg()
                    .size(px(14.))
                    .text_color(t.app.fg)
                    .into_any_element()
            } else {
                div().child(group.label.clone()).into_any_element()
            });
        div()
            .relative()
            .child(trigger)
            .when(open, |v| v.child(self.popup(index, window, cx)))
    }
}

impl Render for WindowMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let compact = self.kind != Kind::Workspace;
        let entity = cx.entity().downgrade();
        div()
            .id("in-window-menu")
            .track_focus(&self.focus)
            .relative()
            .child(
                gpui::canvas(
                    move |bounds, _, cx| {
                        let _ = entity.update(cx, |this, _| this.trigger_bounds = bounds);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .flex()
            .flex_shrink_0()
            .items_center()
            .when(!compact, |v| {
                v.h(px(28.))
                    .px_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(t.app.border)
            })
            .font_family(t.font.sans.clone())
            .font_weight(gpui::FontWeight::NORMAL)
            .text_size(t.font.xs)
            .on_key_down(cx.listener(Self::key))
            .children((0..self.groups.len()).map(|index| self.trigger(index, window, cx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Entity, TestAppContext};

    struct Harness {
        menu: Entity<WindowMenu>,
        editor_focus: FocusHandle,
        copies: usize,
    }
    impl Render for Harness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(self.menu.clone())
                .child(
                    div()
                        .id("editor")
                        .h(px(100.))
                        .track_focus(&self.editor_focus)
                        .on_action(
                            cx.listener(|this, _: &crate::code_cell::Copy, _, _| this.copies += 1),
                        )
                        .child("Editor selection"),
                )
        }
    }
    fn harness(window: &mut Window, cx: &mut Context<Harness>) -> Harness {
        cx.set_global(
            crate::theme::ThemeSpec::light().resolve(&crate::theme::Derivation::default()),
        );
        let editor_focus = cx.focus_handle();
        editor_focus.focus(window, cx);
        Harness {
            menu: cx.new(|cx| WindowMenu::new(Kind::Workspace, cx)),
            editor_focus,
            copies: 0,
        }
    }

    #[gpui::test]
    fn menu_restores_editor_focus_before_dispatch_and_skips_disabled_commands(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(harness);
        let menu = view.read_with(cx, |v, _| v.menu.clone());
        cx.update(|window, cx| menu.update(cx, |menu, cx| menu.open_group(1, window, cx)));
        cx.run_until_parked();
        menu.read_with(cx, |menu, _| {
            let rows = &menu.groups[1].rows;
            assert_eq!(rows[menu.selected.unwrap()].label, "Copy");
            assert!(!rows.iter().find(|r| r.label == "Undo").unwrap().enabled);
        });
        cx.simulate_keystrokes("down enter");
        assert_eq!(view.read_with(cx, |view, _| view.copies), 1);
        cx.update(|window, cx| {
            assert!(view.read(cx).editor_focus.is_focused(window));
            assert!(menu.read(cx).open.is_none());
        });
    }

    #[gpui::test]
    fn mouse_toggle_switching_and_escape_preserve_the_original_focus(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(harness);
        let menu = view.read_with(cx, |v, _| v.menu.clone());
        // The first trigger is in the first 40px of the row.
        cx.simulate_click(point(px(20.), px(14.)), Default::default());
        assert_eq!(menu.read_with(cx, |menu, _| menu.open), Some(0));
        cx.simulate_click(point(px(20.), px(14.)), Default::default());
        assert!(menu.read_with(cx, |menu, _| menu.open.is_none()));
        cx.simulate_click(point(px(20.), px(14.)), Default::default());
        cx.simulate_keystrokes("right");
        assert_eq!(menu.read_with(cx, |menu, _| menu.open), Some(1));
        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| assert!(view.read(cx).editor_focus.is_focused(window)));
        assert_eq!(view.read_with(cx, |v, _| v.copies), 0);
    }

    #[test]
    fn compact_windows_expose_only_relevant_commands() {
        for kind in [Kind::Connection, Kind::Settings] {
            let groups = groups(kind);
            assert_eq!(groups.len(), 1);
            let rows = &groups[0].rows;
            assert!(rows.iter().any(|r| r.label == "Quit Ganymede"));
            assert!(rows.iter().any(|r| r.label == "Check for Updates…"));
            assert_eq!(
                rows.iter().any(|r| r.label == "Settings…"),
                kind == Kind::Connection
            );
            assert!(
                !rows
                    .iter()
                    .any(|r| r.label == "Save" || r.label == "Hide Ganymede")
            );
        }
    }

    #[gpui::test]
    fn clicking_a_menu_command_dispatches_to_the_editor_and_click_away_dismisses(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(harness);
        let menu = view.read_with(cx, |v, _| v.menu.clone());
        let trigger = cx.debug_bounds("menu-trigger-Edit").unwrap().center();
        cx.simulate_click(trigger, Default::default());
        let copy = cx.debug_bounds("menu-item-Copy").unwrap().center();
        cx.simulate_click(copy, Default::default());
        assert_eq!(view.read_with(cx, |v, _| v.copies), 1);
        assert!(menu.read_with(cx, |menu, _| menu.open.is_none()));
        cx.simulate_click(trigger, Default::default());
        cx.simulate_click(point(px(600.), px(100.)), Default::default());
        assert!(menu.read_with(cx, |menu, _| menu.open.is_none()));
        cx.update(|window, cx| assert!(view.read(cx).editor_focus.is_focused(window)));
    }
}
