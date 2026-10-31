//! A standalone, in-memory code cell. Notebook persistence and execution can be
//! connected later without coupling the text-input machinery to a kernel.
use std::{ops::Range, time::Duration};

use crate::theme::ActiveTheme;
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, KeyBinding, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ScrollHandle,
    ShapedLine, Style, Subscription, Task, TextRun, UTF16Selection, UnderlineStyle, Window,
    actions, div, fill, point, prelude::*, px, size,
};
use unicode_segmentation::UnicodeSegmentation;

actions!(
    code_cell,
    [
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        Home,
        End,
        SelectHome,
        SelectEnd,
        SelectAll,
        Backspace,
        Delete,
        Enter,
        RunAndAdvance,
        SelectCell,
        EditCell,
        Tab,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo
    ]
);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", SelectCell, Some("CodeCell")),
        KeyBinding::new("enter", EditCell, Some("NotebookCell && !CodeCell")),
        KeyBinding::new("shift-enter", RunAndAdvance, Some("NotebookCell")),
    ]);
    let context = Some("CodeCell");
    cx.bind_keys([
        KeyBinding::new("left", Left, context),
        KeyBinding::new("right", Right, context),
        KeyBinding::new("up", Up, context),
        KeyBinding::new("down", Down, context),
        KeyBinding::new("shift-left", SelectLeft, context),
        KeyBinding::new("shift-right", SelectRight, context),
        KeyBinding::new("shift-up", SelectUp, context),
        KeyBinding::new("shift-down", SelectDown, context),
        KeyBinding::new("home", Home, context),
        KeyBinding::new("end", End, context),
        KeyBinding::new("shift-home", SelectHome, context),
        KeyBinding::new("shift-end", SelectEnd, context),
        KeyBinding::new("backspace", Backspace, context),
        KeyBinding::new("delete", Delete, context),
        KeyBinding::new("enter", Enter, context),
        KeyBinding::new("shift-enter", RunAndAdvance, context),
        KeyBinding::new("tab", Tab, context),
    ]);
    let modifier = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    cx.bind_keys([
        KeyBinding::new(&format!("{modifier}-a"), SelectAll, context),
        KeyBinding::new(&format!("{modifier}-c"), Copy, context),
        KeyBinding::new(&format!("{modifier}-x"), Cut, context),
        KeyBinding::new(&format!("{modifier}-v"), Paste, context),
        KeyBinding::new(&format!("{modifier}-z"), Undo, context),
        KeyBinding::new(&format!("{modifier}-shift-z"), Redo, context),
    ]);
    if cfg!(target_os = "macos") {
        cx.bind_keys([
            KeyBinding::new("cmd-left", Home, context),
            KeyBinding::new("cmd-right", End, context),
            KeyBinding::new("cmd-shift-left", SelectHome, context),
            KeyBinding::new("cmd-shift-right", SelectEnd, context),
        ]);
    }
}

#[derive(Clone, Default, Debug, PartialEq)]
struct Buffer {
    text: String,
    anchor: usize,
    head: usize,
}

impl Buffer {
    fn selection(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }
    fn select(&mut self, offset: usize, extend: bool) {
        self.head = offset;
        if !extend {
            self.anchor = offset;
        }
    }
    fn replace(&mut self, range: Range<usize>, text: &str) {
        self.text.replace_range(range.clone(), text);
        self.select(range.start + text.len(), false);
    }
    fn previous(&self) -> usize {
        self.text
            .grapheme_indices(true)
            .rev()
            .find_map(|(i, _)| (i < self.head).then_some(i))
            .unwrap_or(0)
    }
    fn next(&self) -> usize {
        self.text
            .grapheme_indices(true)
            .find_map(|(i, _)| (i > self.head).then_some(i))
            .unwrap_or(self.text.len())
    }
    fn line_start(&self) -> usize {
        self.text[..self.head].rfind('\n').map_or(0, |i| i + 1)
    }
    fn line_end(&self) -> usize {
        self.text[self.head..]
            .find('\n')
            .map_or(self.text.len(), |i| self.head + i)
    }
    fn position(&self, offset: usize) -> (usize, usize) {
        let before = &self.text[..offset];
        (
            before.bytes().filter(|b| *b == b'\n').count(),
            before.rfind('\n').map_or(offset, |i| offset - i - 1),
        )
    }
    fn to_utf16(&self, range: Range<usize>) -> Range<usize> {
        self.text[..range.start].encode_utf16().count()
            ..self.text[..range.end].encode_utf16().count()
    }
    fn utf16_range(&self, range: Range<usize>) -> Range<usize> {
        from_utf16(&self.text, range.start)..from_utf16(&self.text, range.end)
    }
}

fn from_utf16(text: &str, offset: usize) -> usize {
    let mut units = 0;
    for (i, ch) in text.char_indices() {
        if units >= offset {
            return i;
        }
        units += ch.len_utf16();
    }
    text.len()
}

#[derive(Clone, Copy)]
pub enum CellEvent {
    Changed,
    Duplicate,
    DeleteCell,
    MoveUp,
    MoveDown,
    InsertAbove,
    InsertBelow,
    Selected,
    RunAndAdvance,
    RunFinished,
    RunFailed,
    RunProgress,
}

impl gpui::EventEmitter<CellEvent> for CodeCell {}

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

pub struct CodeCell {
    current: bool,
    document: bool,
    kind: ganymede_core::notebook::CellKind,
    menu_open: bool,
    execution_count: Option<u64>,
    running_frame: Option<usize>,
    queued: bool,
    output: Vec<crate::execution::OutputBlock>,
    execution_task: Task<()>,
    buffer: Buffer,
    focus: FocusHandle,
    cell_focus: FocusHandle,
    marked: Option<Range<usize>>,
    undo: Vec<Buffer>,
    redo: Vec<Buffer>,
    composition_start: Option<Buffer>,
    lines: Vec<ShapedLine>,
    bounds: Option<Bounds<Pixels>>,
    line_height: Pixels,
    goal_x: Option<Pixels>,
    selecting: bool,
    scroll: ScrollHandle,
    cursor_visible: bool,
    blink: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl CodeCell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let cell_focus = cx.focus_handle();
        let subscriptions = vec![
            cx.on_focus(&cell_focus, window, |_, _, cx| {
                cx.emit(CellEvent::Selected);
                cx.notify();
            }),
            cx.on_blur(&cell_focus, window, |_, _, cx| {
                cx.notify();
            }),
            cx.on_focus(&focus, window, |this, _, cx| {
                this.reset_blink(cx);
                cx.emit(CellEvent::Selected);
            }),
            cx.on_blur(&focus, window, |this, _, cx| {
                this.blink = Task::ready(());
                this.cursor_visible = false;
                this.selecting = false;
                cx.notify();
            }),
        ];
        Self {
            current: false,
            document: false,
            kind: ganymede_core::notebook::CellKind::Code,
            menu_open: false,
            execution_count: None,
            running_frame: None,
            queued: false,
            output: Vec::new(),
            execution_task: Task::ready(()),
            buffer: Buffer::default(),
            focus,
            cell_focus,
            marked: None,
            undo: vec![],
            redo: vec![],
            composition_start: None,
            lines: vec![],
            bounds: None,
            line_height: cx.theme().font.line_height_tight,
            goal_x: None,
            selecting: false,
            scroll: ScrollHandle::new(),
            cursor_visible: true,
            blink: Task::ready(()),
            _subscriptions: subscriptions,
        }
    }

    pub fn new_document(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self::new(window, cx);
        this.document = true;
        this
    }

    pub fn set_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.buffer.text = text;
        cx.emit(CellEvent::Changed);
        cx.notify();
    }

    pub fn load(&mut self, cell: &ganymede_core::notebook::Cell, cx: &mut Context<Self>) {
        self.buffer.text = cell.source();
        self.kind = cell.kind();
        self.execution_count = cell.execution_count();
        self.output = crate::execution::output_blocks(cell.outputs());
        cx.notify();
    }

    pub fn text(&self) -> &str {
        &self.buffer.text
    }

    pub fn kind(&self) -> ganymede_core::notebook::CellKind {
        self.kind
    }

    pub fn set_kind(&mut self, kind: ganymede_core::notebook::CellKind, cx: &mut Context<Self>) {
        if self.kind != kind {
            self.kind = kind;
            self.clear_execution(cx);
            self.set_queued(false, cx);
            cx.emit(CellEvent::Changed);
            cx.notify();
        }
    }

    pub fn set_current(&mut self, current: bool, cx: &mut Context<Self>) {
        if self.current != current {
            self.current = current;
            cx.notify();
        }
    }

    pub fn running_spinner(&self) -> Option<char> {
        self.running_frame.map(|frame| SPINNER[frame])
    }

    pub fn set_queued(&mut self, queued: bool, cx: &mut Context<Self>) {
        self.queued = queued;
        cx.notify();
    }

    pub fn clear_execution(&mut self, cx: &mut Context<Self>) {
        self.execution_task = Task::ready(());
        self.execution_count = None;
        self.output.clear();
        self.running_frame = None;
        self.queued = false;
        cx.notify();
    }

    pub fn run(
        &mut self,
        session: crate::execution::Session,
        source: String,
        cx: &mut Context<Self>,
    ) {
        self.clear_execution(cx);
        self.running_frame = Some(0);
        let mut updates = session.run(cx.entity_id().as_u64(), source);
        self.execution_task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                let mut update = updates.borrow_and_update().clone();
                if updates.has_changed().is_err() && !update.done {
                    update.done = true;
                    update.failed = true;
                    update.blocks.push(crate::execution::OutputBlock {
                        text: "Execution connection ended unexpectedly.".into(),
                        execution_count: None,
                    });
                }
                let done = update.done;
                if this
                    .update(cx, |this, cx| {
                        this.execution_count = update.count;
                        this.output = update.blocks;
                        this.running_frame = if done {
                            None
                        } else {
                            Some((this.running_frame.unwrap_or(0) + 1) % SPINNER.len())
                        };
                        if done {
                            cx.emit(if update.failed {
                                CellEvent::RunFailed
                            } else {
                                CellEvent::RunFinished
                            });
                        } else {
                            cx.emit(CellEvent::RunProgress);
                        }
                        cx.notify();
                    })
                    .is_err()
                    || done
                {
                    break;
                }
            }
        });
        cx.notify();
    }

    fn reset_blink(&mut self, cx: &mut Context<Self>) {
        self.cursor_visible = true;
        self.blink = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        this.cursor_visible = !this.cursor_visible;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        cx.notify();
    }

    fn changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.goal_x = None;
        self.reset_blink(cx);
        // Resolve scrolling in the input handler, before the next layout pass.
        let (row, column) = self.buffer.position(self.buffer.head);
        let line = self.buffer.text.split('\n').nth(row).unwrap_or("");
        let theme = cx.theme();
        let run = TextRun {
            len: line.len(),
            font: gpui::font(theme.font.mono.clone()),
            color: theme.card.fg,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped =
            window
                .text_system()
                .shape_line(line.to_owned().into(), theme.font.code, &[run], None);
        let x = shaped.x_for_index(column);
        let y = self.line_height * row as f32;
        let viewport = self.scroll.bounds().size;
        if viewport.width > px(0.) && viewport.height > px(0.) {
            let mut offset = self.scroll.offset();
            if x + offset.x < px(0.) {
                offset.x = -x;
            }
            if x + offset.x + px(4.) > viewport.width {
                offset.x = viewport.width - x - px(4.);
            }
            if y + offset.y < px(0.) {
                offset.y = -y;
            }
            if y + offset.y + self.line_height > viewport.height {
                offset.y = viewport.height - y - self.line_height;
            }
            self.scroll.set_offset(offset);
        }
    }

    fn move_cursor(
        &mut self,
        offset: usize,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.finish_composition();
        self.buffer.select(offset, extend);
        self.changed(window, cx);
    }

    fn vertical(&mut self, down: bool, extend: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (row, col) = self.buffer.position(self.buffer.head);
        let Some(line) = self.lines.get(row) else {
            return;
        };
        let goal = self.goal_x.unwrap_or_else(|| line.x_for_index(col));
        let target = if down {
            (row + 1).min(self.lines.len() - 1)
        } else {
            row.saturating_sub(1)
        };
        let offset = if row == target {
            if down { self.buffer.text.len() } else { 0 }
        } else {
            self.buffer
                .text
                .split('\n')
                .take(target)
                .map(|s| s.len() + 1)
                .sum::<usize>()
                + self.lines[target].closest_index_for_x(goal)
        };
        self.move_cursor(offset, extend, window, cx);
        self.goal_x = Some(goal);
    }

    fn finish_composition(&mut self) {
        if let Some(before) = self.composition_start.take() {
            self.undo.push(before);
            self.redo.clear();
        }
        self.marked = None;
    }

    fn edit(&mut self, range: Range<usize>, text: &str) {
        self.undo.push(
            self.composition_start
                .take()
                .unwrap_or_else(|| self.buffer.clone()),
        );
        self.redo.clear();
        self.buffer.replace(range, text);
        self.marked = None;
    }

    fn index_at(&self, position: Point<Pixels>) -> usize {
        let Some(bounds) = self.bounds else {
            return 0;
        };
        if self.lines.is_empty() || position.y < bounds.top() {
            return 0;
        }
        let row = (((position.y - bounds.top()) / self.line_height).floor() as usize)
            .min(self.lines.len() - 1);
        let start: usize = self
            .buffer
            .text
            .split('\n')
            .take(row)
            .map(|s| s.len() + 1)
            .sum();
        (start + self.lines[row].closest_index_for_x(position.x - bounds.left()))
            .min(self.buffer.text.len())
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.focus.focus(window, cx);
        self.selecting = true;
        let offset = self.index_at(event.position);
        self.move_cursor(offset, event.modifiers.shift, window, cx);
    }
    fn mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            self.move_cursor(self.index_at(event.position), true, window, cx);
        }
    }
    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.buffer.selection().is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.buffer.text[self.buffer.selection()].to_owned(),
            ));
        }
    }
    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.buffer.selection().is_empty() {
            self.copy(&Copy, window, cx);
            self.replace_text_in_range(None, "", window, cx);
        }
    }
    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(
                None,
                &text
                    .replace("\r\n", "\n")
                    .replace('\r', "\n")
                    .replace('\t', "    "),
                window,
                cx,
            );
        }
    }
}

impl Focusable for CodeCell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EntityInputHandler for CodeCell {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.buffer.utf16_range(range);
        *actual = Some(self.buffer.to_utf16(range.clone()));
        Some(self.buffer.text[range].to_owned())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.buffer.to_utf16(self.buffer.selection()),
            reversed: self.buffer.head < self.buffer.anchor,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.clone().map(|r| self.buffer.to_utf16(r))
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.finish_composition();
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|r| self.buffer.utf16_range(r))
            .or(self.marked.clone())
            .unwrap_or_else(|| self.buffer.selection());
        self.edit(range, text);
        cx.emit(CellEvent::Changed);
        self.changed(window, cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|r| self.buffer.utf16_range(r))
            .or(self.marked.clone())
            .unwrap_or_else(|| self.buffer.selection());
        self.composition_start
            .get_or_insert_with(|| self.buffer.clone());
        self.buffer.replace(range.clone(), text);
        self.marked = (!text.is_empty()).then_some(range.start..range.start + text.len());
        if let Some(selection) = selection {
            self.buffer.anchor = range.start + from_utf16(text, selection.start);
            self.buffer.head = range.start + from_utf16(text, selection.end);
        }
        cx.emit(CellEvent::Changed);
        self.changed(window, cx);
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.buffer.utf16_range(range);
        let (row, col) = self.buffer.position(range.start);
        let (end_row, end_col) = self.buffer.position(range.end);
        let line = self.lines.get(row)?;
        let bounds = self.bounds?;
        let left = line.x_for_index(col);
        let right = if row == end_row {
            line.x_for_index(end_col)
        } else {
            line.width
        };
        Some(Bounds::new(
            point(
                bounds.left() + left,
                bounds.top() + self.line_height * row as f32,
            ),
            size((right - left).max(px(1.)), self.line_height),
        ))
    }
    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let index = self.index_at(position);
        Some(self.buffer.to_utf16(index..index).start)
    }
}

impl Render for CodeCell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let surface = theme.card;
        let cell_padding = px(4.);
        let height =
            theme.font.line_height_tight * self.buffer.text.split('\n').count().clamp(1, 16) as f32;
        let editor = div()
            .id("code-cell")
            .flex_1()
            .min_w_0()
            .flex_shrink_0()
            .key_context("CodeCell")
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .bg(surface.bg)
            .text_color(surface.fg)
            .border_1()
            .border_color(surface.border)
            .rounded(theme.radius.md())
            .focus(|style| style.border_color(surface.ring))
            .font_family(theme.font.mono.clone())
            .text_size(theme.font.code)
            .line_height(theme.font.line_height_tight)
            .p_3()
            .pr(px(44.))
            .on_action(cx.listener(|this, _: &Left, w, cx| {
                let offset = if this.buffer.selection().is_empty() {
                    this.buffer.previous()
                } else {
                    this.buffer.selection().start
                };
                this.move_cursor(offset, false, w, cx);
            }))
            .on_action(cx.listener(|this, _: &Right, w, cx| {
                let offset = if this.buffer.selection().is_empty() {
                    this.buffer.next()
                } else {
                    this.buffer.selection().end
                };
                this.move_cursor(offset, false, w, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectLeft, w, cx| {
                this.move_cursor(this.buffer.previous(), true, w, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectRight, w, cx| {
                this.move_cursor(this.buffer.next(), true, w, cx)
            }))
            .on_action(cx.listener(|this, _: &Up, w, cx| this.vertical(false, false, w, cx)))
            .on_action(cx.listener(|this, _: &Down, w, cx| this.vertical(true, false, w, cx)))
            .on_action(cx.listener(|this, _: &SelectUp, w, cx| this.vertical(false, true, w, cx)))
            .on_action(cx.listener(|this, _: &SelectDown, w, cx| this.vertical(true, true, w, cx)))
            .on_action(cx.listener(|this, _: &Home, w, cx| {
                this.move_cursor(this.buffer.line_start(), false, w, cx)
            }))
            .on_action(cx.listener(|this, _: &End, w, cx| {
                this.move_cursor(this.buffer.line_end(), false, w, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectHome, w, cx| {
                this.move_cursor(this.buffer.line_start(), true, w, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectEnd, w, cx| {
                this.move_cursor(this.buffer.line_end(), true, w, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectAll, w, cx| {
                this.buffer.anchor = 0;
                this.move_cursor(this.buffer.text.len(), true, w, cx);
            }))
            .on_action(cx.listener(|this, _: &Backspace, w, cx| {
                if this.buffer.selection().is_empty() {
                    this.buffer.anchor = this.buffer.previous();
                }
                this.replace_text_in_range(None, "", w, cx);
            }))
            .on_action(cx.listener(|this, _: &Delete, w, cx| {
                if this.buffer.selection().is_empty() {
                    this.buffer.anchor = this.buffer.next();
                }
                this.replace_text_in_range(None, "", w, cx);
            }))
            .on_action(cx.listener(|this, _: &Enter, w, cx| {
                let indent: String = this.buffer.text[this.buffer.line_start()..this.buffer.head]
                    .chars()
                    .take_while(|c| *c == ' ')
                    .collect();
                this.replace_text_in_range(None, &format!("\n{indent}"), w, cx);
            }))
            .on_action(
                cx.listener(|this, _: &Tab, w, cx| this.replace_text_in_range(None, "    ", w, cx)),
            )
            .on_action(cx.listener(|_, _: &RunAndAdvance, _, cx| {
                cx.emit(CellEvent::RunAndAdvance);
            }))
            .on_action(cx.listener(|this, _: &SelectCell, window, cx| {
                this.cell_focus.focus(window, cx);
            }))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(|this, _: &Undo, w, cx| {
                this.finish_composition();
                if let Some(previous) = this.undo.pop() {
                    this.redo
                        .push(std::mem::replace(&mut this.buffer, previous));
                    cx.emit(CellEvent::Changed);
                    this.changed(w, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Redo, w, cx| {
                if let Some(next) = this.redo.pop() {
                    this.undo.push(std::mem::replace(&mut this.buffer, next));
                    cx.emit(CellEvent::Changed);
                    this.changed(w, cx);
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .child(
                div()
                    .id("code-scroll")
                    .w_full()
                    .h(height)
                    .when(self.document, |s| s.h_full())
                    .overflow_scroll()
                    .track_scroll(&self.scroll)
                    .child(CellText { cell: cx.entity() }),
            );

        if self.document {
            return editor
                .h_full()
                .bg(theme.app.bg)
                .text_color(theme.app.fg)
                .border_0()
                .pr_3()
                .into_any_element();
        }
        let popover = theme.popover;
        let menu = div()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.menu_open = false;
                cx.notify();
            }))
            .id("cell-actions-menu")
            .w(px(190.))
            .p_1()
            .flex()
            .flex_col()
            .bg(popover.bg)
            .text_color(popover.fg)
            .border_1()
            .border_color(popover.border)
            .rounded(theme.radius.xl())
            .shadow_lg()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(
                [
                    ("Duplicate", crate::icon::Icon::Copy, CellEvent::Duplicate),
                    ("Move up", crate::icon::Icon::ArrowUp, CellEvent::MoveUp),
                    (
                        "Move down",
                        crate::icon::Icon::ArrowDown,
                        CellEvent::MoveDown,
                    ),
                    (
                        "Insert above",
                        crate::icon::Icon::RowsPlusTop,
                        CellEvent::InsertAbove,
                    ),
                    (
                        "Insert below",
                        crate::icon::Icon::RowsPlusBottom,
                        CellEvent::InsertBelow,
                    ),
                    ("Delete", crate::icon::Icon::Trash, CellEvent::DeleteCell),
                ]
                .into_iter()
                .map(|(label, icon, event)| {
                    div()
                        .id(label)
                        .group(label)
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded(theme.radius.lg())
                        .cursor_pointer()
                        .font_family(theme.font.sans.clone())
                        .text_size(theme.font.sm)
                        .hover(|s| s.bg(surface.primary).text_color(surface.primary_fg))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.menu_open = false;
                            cx.notify();
                            cx.emit(event);
                        }))
                        .child(
                            icon.svg()
                                .size(px(16.))
                                .text_color(popover.fg)
                                .group_hover(label, |s| s.text_color(surface.primary_fg)),
                        )
                        .child(label)
                }),
            );
        let editor = div()
            .relative()
            .flex_1()
            .min_w_0()
            .flex()
            .child(editor)
            .when(self.current, |editor| {
                editor.child(
                    div()
                        .absolute()
                        .right(px(8.))
                        .top(px(10.))
                        .child(
                            div()
                                .id("cell-actions")
                                .size(px(24.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .rounded(theme.radius.sm())
                                .hover(|s| s.bg(surface.accent).text_color(surface.accent_fg))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.menu_open = true;
                                    cx.notify();
                                }))
                                .child(
                                    crate::icon::Icon::DotsThree
                                        .svg()
                                        .size(px(20.))
                                        .text_color(if self.focus.is_focused(window) {
                                            surface.fg
                                        } else {
                                            surface.muted_fg
                                        })
                                        .group_hover("notebook-cell", |style| {
                                            style.text_color(surface.fg)
                                        }),
                                ),
                        )
                        .when(self.menu_open, |button| {
                            button.child(gpui::deferred(
                                gpui::anchored()
                                    .anchor(gpui::Anchor::TopRight)
                                    .offset(point(px(24.), px(4.)))
                                    .snap_to_window_with_margin(px(8.))
                                    .child(menu),
                            ))
                        }),
                )
            });

        let input = div()
            .w_full()
            .min_w_0()
            .flex()
            .items_stretch()
            .flex_shrink_0()
            // Reserve the marker gutter even for cells that are not current.
            .child(
                div()
                    .w(px(8.))
                    .flex_shrink_0()
                    .rounded_full()
                    .when(self.current, |bar| {
                        bar.bg(surface.primary).text_color(surface.primary_fg)
                    }),
            )
            .child(
                div()
                    .w(px(32.))
                    .ml_3()
                    .mr(px(6.))
                    .flex_shrink_0()
                    // Align with the editor's first line, including its border.
                    .pt(px(13.))
                    .font_family(theme.font.mono.clone())
                    .text_size(theme.font.code)
                    .line_height(theme.font.line_height_tight)
                    .text_color(if self.current {
                        surface.primary
                    } else {
                        theme.app.muted_fg
                    })
                    .text_right()
                    .child(match self.running_frame {
                        _ if self.kind != ganymede_core::notebook::CellKind::Code => String::new(),
                        Some(frame) => format!("[{}]:", SPINNER[frame]),
                        None if self.queued => "[*]:".to_owned(),
                        None => match self.execution_count {
                            Some(count) => format!("[{count}]:"),
                            None => "[ ]:".to_owned(),
                        },
                    }),
            )
            .child(editor);

        div()
            .id("notebook-cell")
            .group("notebook-cell")
            .key_context("NotebookCell")
            .track_focus(&self.cell_focus)
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .p(cell_padding)
            .px(px(6.))
            .pr(px(4.))
            .bg(theme.app.bg)
            .text_color(theme.app.fg)
            .border_1()
            .border_color(if self.current && self.cell_focus.is_focused(window) {
                surface.primary
            } else {
                theme.app.bg
            })
            .rounded(theme.radius.md() + cell_padding / 2. + px(1.))
            .on_mouse_down_out(cx.listener(|this, _, window, _| {
                if this.focus.is_focused(window) || this.cell_focus.is_focused(window) {
                    window.blur();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.cell_focus.focus(window, cx);
                }),
            )
            .on_action(cx.listener(|this, _: &EditCell, window, cx| {
                this.focus.focus(window, cx);
            }))
            .on_action(cx.listener(|_, _: &RunAndAdvance, _, cx| {
                cx.emit(CellEvent::RunAndAdvance);
            }))
            .gap(px(6.))
            .child(input)
            .when(!self.output.is_empty(), |cell| {
                cell.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .flex()
                        .items_stretch()
                        .child(
                            div()
                                .w(px(8.))
                                .flex_shrink_0()
                                .rounded_full()
                                .when(self.current, |bar| {
                                    bar.bg(surface.primary).text_color(surface.primary_fg)
                                }),
                        )
                        .child(div().flex_1().min_w_0().flex().flex_col().children(
                            self.output.iter().enumerate().map(|(index, output)| {
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .flex()
                                    .items_start()
                                    .font_family(theme.font.mono.clone())
                                    .text_size(theme.font.code)
                                    .line_height(theme.font.line_height_tight)
                                    .child(
                                        div()
                                            .w(px(32.))
                                            .ml_3()
                                            .mr(px(6.))
                                            .flex_shrink_0()
                                            .text_right()
                                            .text_color(theme.app.muted_fg)
                                            .child(
                                                output
                                                    .execution_count
                                                    .map(|count| format!("[{count}]:"))
                                                    .unwrap_or_default(),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id(("cell-output", index))
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_x_scroll()
                                            .px(px(13.))
                                            .text_color(theme.app.fg)
                                            // A stream's final newline terminates its last line;
                                            // it should not introduce an extra blank UI row.
                                            .child(
                                                output
                                                    .text
                                                    .strip_suffix('\n')
                                                    .unwrap_or(&output.text)
                                                    .to_owned(),
                                            ),
                                    )
                            }),
                        )),
                )
            })
            .into_any_element()
    }
}

struct CellText {
    cell: Entity<CodeCell>,
}
struct Layout {
    lines: Vec<ShapedLine>,
    starts: Vec<usize>,
    line_height: Pixels,
    cap_height: Pixels,
}
struct Prepaint {
    selection: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
}

impl IntoElement for CellText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for CellText {
    type RequestLayoutState = Layout;
    type PrepaintState = Prepaint;
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Layout) {
        let cell = self.cell.read(cx);
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let font_id = window.text_system().resolve_font(&style.font());
        let cap_height = window.text_system().cap_height(font_id, font_size);
        let mut starts = vec![];
        let mut start = 0;
        let lines: Vec<_> = cell
            .buffer
            .text
            .split('\n')
            .map(|text| {
                starts.push(start);
                let base = TextRun {
                    len: text.len(),
                    font: style.font(),
                    color: style.color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let mut runs = vec![base.clone()];
                if let Some(marked) = &cell.marked {
                    let left = marked.start.saturating_sub(start).min(text.len());
                    let right = marked.end.saturating_sub(start).min(text.len());
                    if left < right {
                        runs = vec![
                            TextRun {
                                len: left,
                                ..base.clone()
                            },
                            TextRun {
                                len: right - left,
                                underline: Some(UnderlineStyle {
                                    color: Some(style.color),
                                    thickness: px(1.),
                                    wavy: false,
                                }),
                                ..base.clone()
                            },
                            TextRun {
                                len: text.len() - right,
                                ..base
                            },
                        ];
                        runs.retain(|r| r.len > 0);
                    }
                }
                start += text.len() + 1;
                window
                    .text_system()
                    .shape_line(text.to_owned().into(), font_size, &runs, None)
            })
            .collect();
        let mut layout_style = Style::default();
        layout_style.size.width =
            (lines.iter().map(|l| l.width).fold(px(0.), Pixels::max) + px(4.)).into();
        layout_style.min_size.width = gpui::relative(1.).into();
        layout_style.size.height = (line_height * lines.len().max(1) as f32).into();
        (
            window.request_layout(layout_style, [], cx),
            Layout {
                lines,
                starts,
                line_height,
                cap_height,
            },
        )
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Layout,
        _: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let cell = self.cell.read(cx);
        let selected = cell.buffer.selection();
        let theme = cx.theme();
        let mut selection = vec![];
        for (row, line) in layout.lines.iter().enumerate() {
            let start = layout.starts[row];
            let end = start + line.len();
            if selected.start <= end && selected.end > start && !selected.is_empty() {
                let left = line.x_for_index(selected.start.saturating_sub(start).min(line.len()));
                let right = if selected.end > end {
                    line.width + px(8.)
                } else {
                    line.x_for_index(selected.end - start)
                };
                selection.push(fill(
                    Bounds::new(
                        point(
                            bounds.left() + left,
                            bounds.top() + layout.line_height * row as f32,
                        ),
                        size(right - left, layout.line_height),
                    ),
                    theme.card.accent,
                ));
            }
        }
        let (row, col) = cell.buffer.position(cell.buffer.head);
        let cursor = (cell.cursor_visible && selected.is_empty()).then(|| {
            let line = &layout.lines[row];
            // Match the baseline used by ShapedLine::paint, excluding line leading.
            let baseline = (layout.line_height - line.ascent - line.descent) / 2. + line.ascent;
            let caret_overhang = px(2.);
            fill(
                Bounds::new(
                    point(
                        bounds.left() + layout.lines[row].x_for_index(col),
                        bounds.top() + layout.line_height * row as f32 + baseline
                            - layout.cap_height
                            - caret_overhang,
                    ),
                    size(px(1.5), layout.cap_height + caret_overhang * 2.),
                ),
                theme.card.fg,
            )
        });
        Prepaint { selection, cursor }
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Layout,
        prepaint: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.cell.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.cell.clone()),
            cx,
        );
        for quad in prepaint.selection.drain(..) {
            window.paint_quad(quad);
        }
        for (row, line) in layout.lines.iter().enumerate() {
            line.paint(
                point(
                    bounds.left(),
                    bounds.top() + layout.line_height * row as f32,
                ),
                layout.line_height,
                gpui::TextAlign::Left,
                None,
                window,
                cx,
            )
            .expect("paint cell text");
        }
        if focus.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
        self.cell.update(cx, |cell, _| {
            cell.lines = std::mem::take(&mut layout.lines);
            cell.bounds = Some(bounds);
            cell.line_height = layout.line_height;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiline_selection_replaces_across_lines_and_keeps_trailing_line() {
        let mut buffer = Buffer {
            text: "one\ntwo\nthree".into(),
            anchor: 2,
            head: 9,
        };
        buffer.replace(buffer.selection(), "é\n");
        assert_eq!(buffer.text, "oné\nhree");
        assert_eq!(buffer.position(buffer.head), (1, 0));
        buffer.replace(0..buffer.text.len(), "\n");
        assert_eq!(buffer.position(buffer.head), (1, 0));
    }

    #[test]
    fn movement_and_deletion_respect_graphemes() {
        let mut buffer = Buffer {
            text: "a👩‍💻e\u{301}".into(),
            anchor: 1,
            head: 1,
        };
        let next = buffer.next();
        assert_eq!(&buffer.text[1..next], "👩‍💻");
        buffer.select(buffer.text.len(), false);
        let previous = buffer.previous();
        buffer.replace(previous..buffer.head, "");
        assert_eq!(buffer.text, "a👩‍💻");
    }

    #[test]
    fn utf16_ranges_handle_emoji_and_clamp_out_of_bounds() {
        let buffer = Buffer {
            text: "a😀é\nz".into(),
            ..Buffer::default()
        };
        assert_eq!(buffer.utf16_range(1..3), 1..5);
        assert_eq!(buffer.to_utf16(1..5), 1..3);
        assert_eq!(buffer.utf16_range(3..100), 5..buffer.text.len());
        assert_eq!(from_utf16("日本", 1), 3);
    }

    #[test]
    fn reversed_selection_keeps_anchor_when_crossing_it() {
        let mut buffer = Buffer {
            text: "abcdef".into(),
            anchor: 3,
            head: 1,
        };
        assert_eq!(buffer.selection(), 1..3);
        buffer.select(5, true);
        assert_eq!(buffer.selection(), 3..5);
        buffer.select(2, false);
        assert_eq!(buffer.selection(), 2..2);
    }
}
