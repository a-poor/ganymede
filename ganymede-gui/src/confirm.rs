//! Compact, themed confirmation prompts with a separate action footer.
use crate::theme::ActiveTheme;
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, FontWeight, PromptResponse, SharedString,
    Window, div, prelude::*, px,
};

pub fn init(cx: &mut App) {
    cx.set_prompt_builder(|_, message, detail, buttons, handle, window, cx| {
        let view = cx.new(|cx| Confirmation {
            title: message.to_owned().into(),
            detail: detail.map(|text| SharedString::from(text.to_owned())),
            buttons: buttons
                .iter()
                .map(|button| button.label().clone())
                .collect(),
            focus: cx.focus_handle(),
            selected: 0,
            keyboard: false,
        });
        handle.with_view(view, window, cx)
    });
}
struct Confirmation {
    title: SharedString,
    detail: Option<SharedString>,
    buttons: Vec<SharedString>,
    focus: FocusHandle,
    selected: usize,
    keyboard: bool,
}
impl EventEmitter<PromptResponse> for Confirmation {}
impl Focusable for Confirmation {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for Confirmation {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        div()
            .id("confirmation-backdrop")
            .size_full()
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
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => cx.emit(PromptResponse(0)),
                    "enter" | "space" => cx.emit(PromptResponse(this.selected)),
                    "tab" | "left" | "right" => {
                        let back = event.keystroke.key == "left" || event.keystroke.modifiers.shift;
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
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(self.title.clone()),
                            )
                            .when_some(self.detail.clone(), |view, detail| {
                                view.child(div().text_color(t.popover.muted_fg).child(detail))
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
                                let primary = index == self.buttons.len() - 1;
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
                                        t.popover.border
                                    })
                                    .when(primary, |view| {
                                        view.bg(t.dialog_action_bg).text_color(t.dialog_action_fg)
                                    })
                                    .when(!primary, |view| {
                                        view.bg(t.popover.bg).text_color(t.popover.fg)
                                    })
                                    .cursor_pointer()
                                    .hover(|view| view.opacity(0.85))
                                    .active(|view| view.opacity(0.7))
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(PromptResponse(index));
                                        cx.stop_propagation();
                                    }))
                                    .child(label.clone())
                            })),
                    ),
            )
    }
}
