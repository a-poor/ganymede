//! Window-local notifications. Keep one ToastHost entity in the window's root.
//! push returns an id; resolve replaces that toast in place (including loading toasts).
use crate::{
    icon::Icon,
    theme::{ActiveTheme, Theme},
};
use gpui::{
    Animation, AnimationExt, App, Context, FontWeight, SharedString, Task, Transformation, Window,
    div, prelude::*, px,
};
use std::{
    cell::RefCell,
    future::Future,
    rc::Rc,
    time::{Duration, Instant},
};

const ENTER_DURATION: Duration = Duration::from_millis(260);
const EXIT_DURATION: Duration = Duration::from_millis(200);
const SLIDE_DISTANCE: f32 = 24.;

pub type ToastId = u64;
type Action = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Plain,
    Info,
    Success,
    Error,
    Loading,
}

pub struct Toast {
    pub title: SharedString,
    pub description: Option<SharedString>,
    pub kind: ToastKind,
    pub duration: Duration,
    action: Option<(SharedString, Action)>,
}

impl Toast {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            description: None,
            kind: ToastKind::Plain,
            duration: Duration::from_secs(5),
            action: None,
        }
    }
    pub fn description(mut self, text: impl Into<SharedString>) -> Self {
        self.description = Some(text.into());
        self
    }
    pub fn kind(mut self, kind: ToastKind) -> Self {
        self.kind = kind;
        self
    }
    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }
    pub fn action(
        mut self,
        label: impl Into<SharedString>,
        callback: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.action = Some((label.into(), Rc::new(callback)));
        self
    }
}

struct Entry {
    id: ToastId,
    toast: Toast,
    remaining: Duration,
    age: Duration,
    exiting: Option<Duration>,
    position: Rc<RefCell<Option<PositionMotion>>>,
}
#[derive(Default)]
struct Queue {
    entries: Vec<Entry>,
    next_id: ToastId,
}
impl Queue {
    fn push(&mut self, toast: Toast) -> ToastId {
        self.next_id += 1;
        self.entries.push(Entry {
            id: self.next_id,
            remaining: toast.duration,
            age: Duration::ZERO,
            exiting: None,
            position: Rc::default(),
            toast,
        });
        self.next_id
    }
    fn resolve(&mut self, id: ToastId, toast: Toast) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.id == id && entry.exiting.is_none())
        {
            entry.remaining = toast.duration;
            entry.toast = toast;
        }
    }
    fn dismiss(&mut self, id: ToastId) -> bool {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.id == id && entry.exiting.is_none())
        {
            entry.exiting = Some(Duration::ZERO);
            true
        } else {
            false
        }
    }
    fn animating(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.age < ENTER_DURATION || entry.exiting.is_some())
    }
    fn advance(&mut self, elapsed: Duration, paused: bool) {
        self.entries.retain_mut(|entry| {
            entry.age += elapsed;
            if let Some(exit_age) = &mut entry.exiting {
                *exit_age += elapsed;
                return *exit_age < EXIT_DURATION;
            }
            if !paused && entry.toast.kind != ToastKind::Loading {
                if elapsed >= entry.remaining {
                    let exit_age = elapsed - entry.remaining;
                    entry.remaining = Duration::ZERO;
                    entry.exiting = Some(exit_age);
                    return exit_age < EXIT_DURATION;
                }
                entry.remaining -= elapsed;
            }
            true
        });
    }
}

pub struct ToastHost {
    queue: Queue,
    hovered: bool,
    was_expanded: bool,
    collapse_started: Option<Instant>,
    focus: gpui::FocusHandle,
    focused: bool,
    last_tick: Instant,
    timer: Option<Task<()>>,
}
impl ToastHost {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        cx.observe_global::<Theme>(|_, cx| cx.notify()).detach();
        cx.on_focus_in(&focus, window, |this, _, cx| {
            this.advance();
            this.focused = true;
            cx.notify();
        })
        .detach();
        cx.on_focus_out(&focus, window, |this, _, _, cx| {
            this.advance();
            this.focused = false;
            cx.notify();
        })
        .detach();
        Self {
            queue: Queue::default(),
            hovered: false,
            was_expanded: false,
            collapse_started: None,
            focus,
            focused: false,
            last_tick: Instant::now(),
            timer: None,
        }
    }
    fn advance(&mut self) {
        let now = Instant::now();
        self.queue.advance(
            now.duration_since(self.last_tick),
            self.hovered || self.focused,
        );
        self.last_tick = now;
    }
    pub fn push(&mut self, toast: Toast, cx: &mut Context<Self>) -> ToastId {
        self.advance();
        let id = self.queue.push(toast);
        self.start_timer(cx);
        cx.notify();
        id
    }
    pub fn resolve(&mut self, id: ToastId, toast: Toast, cx: &mut Context<Self>) {
        self.advance();
        self.queue.resolve(id, toast);
        self.start_timer(cx);
        cx.notify();
    }
    pub fn dismiss(&mut self, id: ToastId, cx: &mut Context<Self>) -> bool {
        self.advance();
        let dismissed = self.queue.dismiss(id);
        cx.notify();
        dismissed
    }
    /// Await an executor-compatible future; Tokio I/O must be run on a Tokio runtime.
    /// Dismissing the notification does not cancel the underlying work.
    pub fn track<F>(&mut self, loading: Toast, future: F, cx: &mut Context<Self>) -> ToastId
    where
        F: Future<Output = Result<Toast, Toast>> + 'static,
    {
        let id = self.push(loading.kind(ToastKind::Loading), cx);
        cx.spawn(async move |this, cx| {
            let toast = match future.await {
                Ok(toast) => toast.kind(ToastKind::Success),
                Err(toast) => toast.kind(ToastKind::Error),
            };
            this.update(cx, |this, cx| this.resolve(id, toast, cx)).ok();
        })
        .detach();
        id
    }
    fn start_timer(&mut self, cx: &mut Context<Self>) {
        if self.timer.is_some() {
            return;
        }
        self.timer = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let keep_running = this
                    .update(cx, |this, cx| {
                        let count = this.queue.entries.len();
                        let animating = this.queue.animating();
                        this.advance();
                        if animating || this.queue.animating() || count != this.queue.entries.len()
                        {
                            cx.notify();
                        }
                        if this.queue.entries.is_empty() {
                            this.hovered = false;
                            this.timer = None;
                            false
                        } else {
                            true
                        }
                    })
                    .unwrap_or(false);
                if !keep_running {
                    break;
                }
            }
        }));
    }
    fn card(&self, entry: &Entry, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        let t = cx.theme();
        let s = t.popover;
        let id = entry.id;
        let toast = &entry.toast;
        let icon = match toast.kind {
            ToastKind::Plain => None,
            ToastKind::Info => Some(Icon::Info),
            ToastKind::Success => Some(Icon::CheckCircle),
            ToastKind::Error => Some(Icon::WarningCircle),
            ToastKind::Loading => Some(Icon::SpinnerGap),
        };
        let entering = (entry.age.as_secs_f32() / ENTER_DURATION.as_secs_f32()).min(1.);
        let (offset, opacity) = if let Some(exit_age) = entry.exiting {
            let progress = (exit_age.as_secs_f32() / EXIT_DURATION.as_secs_f32()).min(1.);
            // Smooth acceleration down; preserve the current position if dismissed during entry.
            let eased = progress * progress;
            let start = (1. - entering).powi(3);
            (SLIDE_DISTANCE * (start + (1. - start) * eased), 1. - eased)
        } else {
            let remaining = (1. - entering).powi(3);
            (SLIDE_DISTANCE * remaining, 1. - remaining)
        };
        div()
            .id(("toast", id))
            .relative()
            .top(px(if cx.reduce_motion() { 0. } else { offset }))
            .opacity(if cx.reduce_motion() {
                if entry.exiting.is_some() { 0. } else { 1. }
            } else {
                opacity
            })
            .flex()
            .items_center()
            .gap_2()
            .p_3()
            .min_h(px(48.))
            .w_full()
            .flex_shrink_0()
            .rounded(t.radius.lg())
            .border_1()
            .border_color(s.border)
            .bg(s.bg)
            .text_color(s.fg)
            .shadow(vec![t.toast_shadow.clone()])
            .when_some(icon, |el, icon| {
                let svg = icon.svg().size(px(16.)).flex_shrink_0().text_color(s.fg);
                if toast.kind == ToastKind::Loading {
                    el.child(svg.with_animation(
                        ("spinner", id),
                        Animation::new(Duration::from_millis(900)).repeat(),
                        |svg, delta| {
                            svg.with_transformation(Transformation::rotate(gpui::radians(
                                delta * std::f32::consts::TAU,
                            )))
                        },
                    ))
                } else {
                    el.child(svg)
                }
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(toast.title.clone()),
                    )
                    .when_some(toast.description.clone(), |el, text| {
                        el.child(
                            div()
                                .text_color(s.muted_fg)
                                .text_size(t.font.xs)
                                .child(text),
                        )
                    }),
            )
            .when_some(toast.action.clone(), |el, (label, callback)| {
                let click = callback.clone();
                el.child(
                    div()
                        .id(("toast-action", id))
                        .tab_index(0)
                        .cursor_pointer()
                        .px_2()
                        .py_1()
                        .rounded(t.radius.sm())
                        .bg(s.fg)
                        .text_color(s.bg)
                        .hover(|s| s.opacity(0.85))
                        .active(|s| s.opacity(0.7))
                        .focus(|style| style.border_1().border_color(s.ring))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if this.dismiss(id, cx) {
                                click(window, cx);
                            }
                        }))
                        .on_key_down(cx.listener(
                            move |this, event: &gpui::KeyDownEvent, window, cx| {
                                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                    if this.dismiss(id, cx) {
                                        callback(window, cx);
                                    }
                                    cx.stop_propagation();
                                }
                            },
                        ))
                        .child(label),
                )
            })
            .child(
                div()
                    .id(("dismiss-toast", id))
                    .aria_label("Dismiss notification")
                    .tab_index(0)
                    .size(px(18.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .rounded(t.radius.sm())
                    .hover(|style| style.bg(s.accent))
                    .focus(|style| style.border_1().border_color(s.ring))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.dismiss(id, cx);
                    }))
                    .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            this.dismiss(id, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(Icon::X.svg().size(px(12.)).text_color(s.muted_fg)),
            )
    }
}
impl Render for ToastHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let expanded = self.hovered || self.focused;
        let count = self.queue.entries.len();
        let now = Instant::now();
        if !expanded && self.was_expanded && !cx.reduce_motion() {
            self.collapse_started = Some(now);
        }
        let reversing = expanded && self.collapse_started.is_some();
        if expanded {
            self.collapse_started = None;
        }
        let collapsing = self
            .collapse_started
            .is_some_and(|start| now.duration_since(start) < COLLAPSE_DURATION);
        if collapsing {
            window.request_animation_frame();
        } else {
            self.collapse_started = None;
        }
        let show_all = expanded || collapsing;
        let collapse_y = if collapsing {
            self.queue
                .entries
                .last()
                .and_then(|entry| entry.position.borrow().as_ref().map(|motion| motion.target))
        } else {
            None
        };
        if expanded && !self.was_expanded && !reversing {
            // Hidden cards have no layout while collapsed. Seed their first visible
            // frame from the peeking stack instead of their expanded destination
            // (or a stale position from an earlier hover).
            let now = Instant::now();
            let front_y = self.queue.entries.last().and_then(|entry| {
                entry
                    .position
                    .borrow()
                    .as_ref()
                    .map(|motion| motion.value(now))
            });
            if let Some(front_y) = front_y {
                for (index, entry) in self
                    .queue
                    .entries
                    .iter()
                    .enumerate()
                    .take(count.saturating_sub(1))
                {
                    let depth = (count - 1 - index).min(2);
                    let y = front_y - depth as f32 * 8.;
                    *entry.position.borrow_mut() = Some(PositionMotion {
                        duration: REFLOW_DURATION,
                        target: y,
                        from: y,
                        started: now,
                    });
                }
            }
        }
        self.was_expanded = expanded;
        let width = (window.viewport_size().width - px(32.)).min(px(356.));
        div()
            .absolute()
            .bottom(px(16.))
            .right(px(16.))
            .w(width)
            .font_family(t.font.sans.clone())
            .text_size(t.font.sm)
            .when(count > 0, |root| {
                root.child(
                    div()
                        .id("toast-stack")
                        .track_focus(&self.focus)
                        .occlude()
                        .on_hover(cx.listener(|this, hovered, _, cx| {
                            this.advance();
                            this.hovered = *hovered;
                            cx.notify();
                        }))
                        .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                            match event.keystroke.key.as_str() {
                                "escape" => {
                                    window.blur();
                                    this.focused = false;
                                    cx.notify();
                                }
                                "tab" => {
                                    if event.keystroke.modifiers.shift {
                                        window.focus_prev(cx);
                                    } else {
                                        window.focus_next(cx);
                                    }
                                    cx.stop_propagation();
                                }
                                _ => {}
                            }
                        }))
                        .relative()
                        .pt(px(if show_all {
                            0.
                        } else {
                            count.saturating_sub(1).min(2) as f32 * 8.
                        }))
                        .when(!show_all, |stack| {
                            (1..count.min(3)).rev().fold(stack, |stack, depth| {
                                stack.child(
                                    div()
                                        .absolute()
                                        .top(px((count.min(3) - 1 - depth) as f32 * 8.))
                                        .left(px(depth as f32 * 8.))
                                        .right(px(depth as f32 * 8.))
                                        .h(px(48.))
                                        .rounded(t.radius.lg())
                                        .border_1()
                                        .border_color(t.popover.border)
                                        .bg(t.popover.bg)
                                        .text_color(t.popover.fg)
                                        .shadow(vec![t.toast_shadow.clone()]),
                                )
                            })
                        })
                        .child(
                            div()
                                .id("toast-list")
                                .relative()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .max_h((window.viewport_size().height - px(80.)).max(px(48.)))
                                .when(show_all, |list| {
                                    list.p(px(16.)).m(px(-16.)).overflow_y_scroll()
                                })
                                .children(
                                    self.queue
                                        .entries
                                        .iter()
                                        .enumerate()
                                        .skip(if show_all { 0 } else { count.saturating_sub(1) })
                                        .map(|(index, entry)| SlidePosition {
                                            child: div()
                                                .w_full()
                                                .flex_shrink_0()
                                                .child(self.card(entry, cx))
                                                .into_any_element(),
                                            position: entry.position.clone(),
                                            target_y: collapse_y.map(|y| {
                                                y - (count - 1 - index).min(2) as f32 * 8.
                                            }),
                                            duration: if collapsing {
                                                COLLAPSE_DURATION
                                            } else {
                                                REFLOW_DURATION
                                            },
                                        }),
                                ),
                        ),
                )
            })
    }
}

// Animate measured positions rather than guessing row heights: descriptions and
// actions can wrap, and multiple removals can happen during an existing transition.
const COLLAPSE_DURATION: Duration = Duration::from_millis(140);
const REFLOW_DURATION: Duration = Duration::from_millis(240);

struct PositionMotion {
    duration: Duration,
    target: f32,
    from: f32,
    started: Instant,
}
impl PositionMotion {
    fn value(&self, now: Instant) -> f32 {
        let progress =
            (now.duration_since(self.started).as_secs_f32() / self.duration.as_secs_f32()).min(1.);
        self.target + (self.from - self.target) * (1. - progress).powi(3)
    }
    fn retarget(&mut self, target: f32, now: Instant) {
        if self.target != target {
            self.from = self.value(now);
            self.target = target;
            self.started = now;
        }
    }
}

struct SlidePosition {
    target_y: Option<f32>,
    duration: Duration,
    child: gpui::AnyElement,
    position: Rc<RefCell<Option<PositionMotion>>>,
}
impl IntoElement for SlidePosition {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl gpui::Element for SlidePosition {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<gpui::ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<gpui::Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let now = Instant::now();
        let layout_y = f32::from(bounds.origin.y);
        let target = self.target_y.unwrap_or(layout_y);
        let offset = {
            let mut state = self.position.borrow_mut();
            let motion = state.get_or_insert(PositionMotion {
                duration: self.duration,
                target,
                from: target,
                started: now,
            });
            if cx.reduce_motion() {
                motion.target = target;
                motion.from = target;
            } else {
                motion.retarget(target, now);
                motion.duration = self.duration;
            }
            let offset = motion.value(now) - layout_y;
            if (motion.value(now) - target).abs() > 0.1 {
                window.request_animation_frame();
            }
            offset
        };
        // Move the child's prepaint as well as its pixels so buttons remain clickable
        // at their animated positions.
        window.with_element_offset(gpui::point(px(0.), px(offset)), |window| {
            self.child.prepaint(window, cx);
        });
    }
    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: gpui::Bounds<gpui::Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hover_preserves_remaining_time_for_all_toasts() {
        let mut q = Queue::default();
        q.push(Toast::new("one"));
        q.push(Toast::new("two"));
        q.advance(Duration::from_secs(2), false);
        q.advance(Duration::from_secs(20), true);
        assert!(
            q.entries
                .iter()
                .all(|entry| entry.remaining == Duration::from_secs(3))
        );
        q.advance(Duration::from_secs(3), false);
        assert!(q.entries.iter().all(|entry| entry.exiting.is_some()));
        q.advance(EXIT_DURATION, true);
        assert!(q.entries.is_empty());
    }
    #[test]
    fn loading_waits_until_resolved_and_gets_a_fresh_duration() {
        let mut q = Queue::default();
        let id = q.push(Toast::new("loading").kind(ToastKind::Loading));
        q.advance(Duration::from_secs(100), false);
        assert_eq!(q.entries.len(), 1);
        q.resolve(id, Toast::new("done").kind(ToastKind::Success));
        q.advance(Duration::from_secs(4), false);
        assert_eq!(q.entries.len(), 1);
        q.advance(Duration::from_secs(1), false);
        q.advance(EXIT_DURATION, false);
        assert!(q.entries.is_empty());
    }
    #[test]
    fn late_completion_does_not_resurrect_dismissed_toast() {
        let mut q = Queue::default();
        let id = q.push(Toast::new("loading").kind(ToastKind::Loading));
        assert!(q.dismiss(id));
        assert!(!q.dismiss(id));
        q.resolve(id, Toast::new("done"));
        assert_eq!(q.entries[0].toast.kind, ToastKind::Loading);
        q.advance(EXIT_DURATION, true);
        assert!(q.entries.is_empty());
    }
    #[test]
    fn hover_pauses_countdown_but_not_entry_or_exit_motion() {
        let mut q = Queue::default();
        let id = q.push(Toast::new("hello"));
        q.advance(ENTER_DURATION, true);
        assert_eq!(q.entries[0].remaining, Duration::from_secs(5));
        assert!(!q.animating());
        q.dismiss(id);
        q.advance(EXIT_DURATION / 2, true);
        assert_eq!(q.entries.len(), 1);
        assert!(q.animating());
        q.advance(EXIT_DURATION / 2, true);
        assert!(q.entries.is_empty());
    }
    #[test]
    fn reflow_continues_from_current_position_when_more_toasts_leave() {
        let start = Instant::now();
        let mut motion = PositionMotion {
            duration: REFLOW_DURATION,
            target: 100.,
            from: 100.,
            started: start,
        };
        motion.retarget(200., start);
        assert_eq!(motion.value(start), 100.);
        let halfway = start + REFLOW_DURATION / 2;
        let current = motion.value(halfway);
        assert!(current > 100. && current < 200.);
        motion.retarget(260., halfway);
        assert_eq!(motion.value(halfway), current);
        assert_eq!(motion.value(halfway + REFLOW_DURATION), 260.);
    }
    #[test]
    fn durations_are_independent() {
        let mut q = Queue::default();
        q.push(Toast::new("short").duration(Duration::from_secs(1)));
        let long = q.push(Toast::new("long").duration(Duration::from_secs(10)));
        q.advance(Duration::from_secs(2), false);
        assert_eq!(q.entries.len(), 1);
        assert_eq!(q.entries[0].id, long);
    }
}
