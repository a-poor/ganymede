//! Compact, themed confirmation prompts with a separate action footer.
use crate::theme::ActiveTheme;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const ENTER_DURATION: Duration = Duration::from_millis(150);
const EXIT_DURATION: Duration = Duration::from_millis(110);
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, FontWeight, PromptResponse, SharedString,
    Window, div, prelude::*, px,
};

/// Ask how to resolve an external notebook change before saving.
pub fn save_conflict(
    path: &str,
    window: &mut Window,
    cx: &mut App,
) -> impl std::future::Future<Output = usize> + use<> {
    conflict_prompt(path, "Notebook", "notebook", window, cx)
}

pub fn file_save_conflict(
    path: &str,
    window: &mut Window,
    cx: &mut App,
) -> impl std::future::Future<Output = usize> + use<> {
    conflict_prompt(path, "File", "file", window, cx)
}

fn conflict_prompt(
    path: &str,
    title: &str,
    kind: &str,
    window: &mut Window,
    cx: &mut App,
) -> impl std::future::Future<Output = usize> + use<> {
    let detail = format!(
        "{path} has changed on the server. Reload discards your local changes. Save Copy preserves them in a separate {kind}. Overwrite replaces the server version."
    );
    let response = window.prompt(
        gpui::PromptLevel::Warning,
        &format!("{title} changed on the server"),
        Some(&detail),
        &["Cancel", "Reload", "Save Copy", "Overwrite"],
        cx,
    );
    async move { response.await.unwrap_or(0) }
}

struct NameField {
    editor: gpui::Entity<crate::code_cell::CodeCell>,
    label: SharedString,
    allow_path: bool,
    result: Arc<Mutex<Option<String>>>,
}
#[derive(Default)]
struct PendingName(Option<NameField>);
impl gpui::Global for PendingName {}

pub struct NamePrompt<'a> {
    pub title: &'a str,
    pub detail: &'a str,
    pub label: &'a str,
    pub initial: String,
    pub allow_path: bool,
    pub action: &'a str,
}
pub fn name_prompt(
    options: NamePrompt<'_>,
    window: &mut Window,
    cx: &mut App,
) -> impl std::future::Future<Output = Option<String>> + use<> {
    let NamePrompt {
        title,
        detail,
        label,
        initial,
        allow_path,
        action,
    } = options;
    let result = Arc::new(Mutex::new(None));
    let editor = cx.new(|cx| crate::code_cell::CodeCell::new_name_input(initial, window, cx));
    cx.global_mut::<PendingName>().0 = Some(NameField {
        editor,
        label: label.to_owned().into(),
        allow_path,
        result: result.clone(),
    });
    // The builder consumes this field synchronously while constructing the prompt.
    let response = window.prompt(
        gpui::PromptLevel::Info,
        title,
        Some(detail),
        &["Cancel", action],
        cx,
    );
    async move {
        if response.await.ok() == Some(1) {
            result.lock().unwrap().take()
        } else {
            None
        }
    }
}

pub fn init(cx: &mut App) {
    cx.set_global(PendingName::default());
    cx.set_prompt_builder(|_, message, detail, buttons, handle, window, cx| {
        let name = cx.global_mut::<PendingName>().0.take();
        let input = name.as_ref().map(|field| field.editor.clone());
        let view = cx.new(|cx| Confirmation {
            name,
            input_error: None,
            title: message.to_owned().into(),
            detail: detail.map(|text| SharedString::from(text.to_owned())),
            buttons: buttons
                .iter()
                .map(|button| button.label().clone())
                .collect(),
            focus: cx.focus_handle(),
            selected: 0,
            keyboard: false,
            opened: Instant::now(),
            closing: None,
        });
        if let Some(input) = input {
            view.update(cx, |_, cx| {
                cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
                    crate::code_cell::CellEvent::RunAndAdvance => this.respond(1, cx),
                    crate::code_cell::CellEvent::DeleteCell => this.respond(0, cx),
                    crate::code_cell::CellEvent::FocusNext => {
                        this.selected = 0;
                        this.keyboard = true;
                        this.focus.focus(window, cx);
                        cx.notify();
                    }
                    crate::code_cell::CellEvent::Changed => {
                        this.input_error = None;
                        cx.notify();
                    }
                    _ => (),
                })
                .detach();
            });
        }
        handle.with_view(view, window, cx)
    });
}
struct Confirmation {
    name: Option<NameField>,
    input_error: Option<String>,
    title: SharedString,
    detail: Option<SharedString>,
    buttons: Vec<SharedString>,
    focus: FocusHandle,
    selected: usize,
    keyboard: bool,
    opened: Instant,
    closing: Option<(Instant, f32)>,
}
impl Confirmation {
    fn visibility(&self, now: Instant) -> f32 {
        if let Some((started, from)) = self.closing {
            let progress =
                (now.duration_since(started).as_secs_f32() / EXIT_DURATION.as_secs_f32()).min(1.);
            from * (1. - progress * progress)
        } else {
            let progress = (now.duration_since(self.opened).as_secs_f32()
                / ENTER_DURATION.as_secs_f32())
            .min(1.);
            1. - (1. - progress).powi(3)
        }
    }

    fn respond(&mut self, answer: usize, cx: &mut Context<Self>) {
        if self.closing.is_some() {
            return;
        }
        if answer == 1
            && let Some(field) = &self.name
        {
            let value = field.editor.read(cx).text().to_owned();
            let valid = if field.allow_path {
                ganymede_core::files::validate_path(&value)
            } else {
                ganymede_core::files::child_path("", &value).map(|_| ())
            };
            if let Err(error) = valid {
                self.input_error = Some(error.to_string());
                cx.notify();
                return;
            }
            *field.result.lock().unwrap() = Some(value);
        }
        let now = Instant::now();
        self.closing = Some((now, self.visibility(now)));
        if cx.reduce_motion() {
            cx.emit(PromptResponse(answer));
            return;
        }
        // Keep focus and the modal barrier until the exit has finished. Only
        // then dispatch the answer, so a double click cannot trigger two actions.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(EXIT_DURATION).await;
            this.update(cx, |_, cx| cx.emit(PromptResponse(answer)))
                .ok();
        })
        .detach();
        cx.notify();
    }
}
impl EventEmitter<PromptResponse> for Confirmation {}
impl Focusable for Confirmation {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.name
            .as_ref()
            .map(|field| field.editor.read(cx).focus_handle(cx))
            .unwrap_or_else(|| self.focus.clone())
    }
}
impl Render for Confirmation {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let visibility = if cx.reduce_motion() {
            1.
        } else {
            self.visibility(now)
        };
        if !cx.reduce_motion()
            && (self.closing.is_some() || now.duration_since(self.opened) < ENTER_DURATION)
        {
            window.request_animation_frame();
        }
        let t = cx.theme();
        div()
            .id("confirmation-backdrop")
            .size_full()
            .opacity(visibility)
            .occlude()
            .track_focus(&self.focus)
            .bg(t.scrim)
            .text_color(t.popover.fg)
            .font_family(t.font.sans.clone())
            .text_size(t.font.sm)
            .flex()
            .items_center()
            .justify_center()
            .p_4()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if this.closing.is_some() {
                    cx.stop_propagation();
                    return;
                }
                if this
                    .name
                    .as_ref()
                    .is_some_and(|field| field.editor.read(cx).focus_handle(cx).is_focused(window))
                {
                    return; // Let the text input handle editing, Enter, Escape, and Tab.
                }
                match event.keystroke.key.as_str() {
                    "escape" => this.respond(0, cx),
                    "enter" | "space" => this.respond(this.selected, cx),
                    "tab" | "left" | "right" => {
                        let back = event.keystroke.key == "left" || event.keystroke.modifiers.shift;
                        if event.keystroke.key == "tab"
                            && let Some(field) = &this.name
                            && ((!back && this.selected == this.buttons.len() - 1)
                                || (back && this.selected == 0))
                        {
                            field.editor.read(cx).focus_handle(cx).focus(window, cx);
                            cx.stop_propagation();
                            return;
                        }
                        this.selected = (this.selected
                            + if back { this.buttons.len() - 1 } else { 1 })
                            % this.buttons.len();
                        this.keyboard = true;
                        cx.notify();
                    }
                    _ => (),
                }
                cx.stop_propagation();
            }))
            .child(
                div()
                    .relative()
                    .top(px(6. * (1. - visibility)))
                    .w(px(420.))
                    .max_w_full()
                    .rounded(t.radius.xl())
                    .overflow_hidden()
                    .border_1()
                    .border_color(t.popover.border)
                    .bg(t.popover.bg)
                    .text_color(t.popover.fg)
                    .shadow(vec![t.toast_shadow.clone()])
                    .child(
                        div()
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(self.title.clone()),
                                    )
                                    .when(self.name.is_some(), |view| {
                                        view.child(
                                            crate::workspace::icon_button(
                                                "close-name-dialog",
                                                "Cancel",
                                                crate::icon::Icon::X,
                                                cx,
                                            )
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.respond(0, cx)),
                                            ),
                                        )
                                    }),
                            )
                            .when_some(self.detail.clone(), |view, detail| {
                                view.child(div().text_color(t.popover.muted_fg).child(detail))
                            })
                            .when_some(self.name.as_ref(), |view, field| {
                                view.child(
                                    div()
                                        .mt_2()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .child(field.label.clone())
                                        .child(div().flex().w_full().child(field.editor.clone())),
                                )
                            })
                            .when_some(self.input_error.clone(), |view, error| {
                                view.child(
                                    div()
                                        .text_size(t.font.xs)
                                        .text_color(t.destructive)
                                        .child(error),
                                )
                            }),
                    )
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .rounded_b(t.radius.xl())
                            .border_t_1()
                            .border_color(t.popover.border)
                            .bg(t.card.bg)
                            .text_color(t.card.fg)
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .children(self.buttons.iter().enumerate().map(|(index, label)| {
                                let destructive =
                                    matches!(label.as_ref(), "Overwrite" | "Delete" | "Shut Down");
                                let primary = index == self.buttons.len() - 1 && !destructive;
                                div()
                                    .id(("confirm-answer", index))
                                    .aria_label(label.clone())
                                    .h(px(30.))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(t.radius.md())
                                    .border_1()
                                    .border_color(if self.keyboard && self.selected == index {
                                        t.popover.ring
                                    } else {
                                        if destructive {
                                            t.dialog_destructive_bg
                                        } else {
                                            t.popover.border
                                        }
                                    })
                                    .when(primary, |view| {
                                        view.bg(t.dialog_action_bg).text_color(t.dialog_action_fg)
                                    })
                                    .when(!primary && !destructive, |view| {
                                        view.bg(t.popover.bg).text_color(t.popover.fg)
                                    })
                                    .cursor_pointer()
                                    .when(destructive, |view| {
                                        view.bg(t.dialog_destructive_bg)
                                            .text_color(t.dialog_destructive_fg)
                                            .hover(|view| view.bg(t.dialog_destructive_hover))
                                            .active(|view| view.bg(t.dialog_destructive_active))
                                    })
                                    .when(!destructive, |view| {
                                        view.hover(|view| view.opacity(0.85))
                                            .active(|view| view.opacity(0.7))
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.respond(index, cx);
                                        cx.stop_propagation();
                                    }))
                                    .child(label.clone())
                            })),
                    ),
            )
    }
}
