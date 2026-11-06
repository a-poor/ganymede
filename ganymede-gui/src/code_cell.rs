//! A standalone, in-memory code cell. Notebook persistence and execution can be
//! connected later without coupling the text-input machinery to a kernel.
pub(crate) mod stdin_prompt;
use std::{cell::RefCell, ops::Range, rc::Rc, time::Duration};

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
        Outdent,
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
        KeyBinding::new("shift-tab", Outdent, context),
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
    fn indented(&self, outdent: bool, indentation: &str) -> Self {
        let mut result = self.clone();
        let selection = self.selection();
        let first = self.text[..selection.start]
            .rfind('\n')
            .map_or(0, |i| i + 1);
        let mut edits = vec![];
        let mut start = first;
        loop {
            let line = &self.text[start..];
            let remove = if outdent {
                if line.starts_with('\t') {
                    1
                } else {
                    line.bytes()
                        .take(indentation.len())
                        .take_while(|b| *b == b' ')
                        .count()
                }
            } else {
                0
            };
            edits.push((start, remove));
            let Some(next) = line.find('\n').map(|i| start + i + 1) else {
                break;
            };
            // A selection ending at the next line's start excludes that line.
            if next >= selection.end {
                break;
            }
            start = next;
        }
        for (start, remove) in edits.into_iter().rev() {
            let insert = if outdent { "" } else { indentation };
            result.text.replace_range(start..start + remove, insert);
            for position in [&mut result.anchor, &mut result.head] {
                if *position >= start {
                    *position = start + (*position - start).saturating_sub(remove) + insert.len();
                }
            }
        }
        result
    }

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
    InputRequested,
    FocusNext,
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
    filter_input: bool,
    name_input: bool,
    search_input: bool,
    search_matches: Vec<Range<usize>>,
    search_active: Option<Range<usize>>,
    search_preview: bool,
    reveal_search: bool,
    count_hint: u64,
    kind: ganymede_core::notebook::CellKind,
    menu_open: bool,
    menu_hovered: Option<usize>,
    execution_count: Option<u64>,
    running_frame: Option<usize>,
    queued: bool,
    output: Vec<crate::execution::OutputBlock>,
    rich_output: Vec<crate::rich::Rich>,
    markdown_preview: Option<crate::rich::Rich>,
    input_prompt: Option<Entity<stdin_prompt::StdinPrompt>>,
    input_return_focus: Option<FocusHandle>,
    image_session: Option<crate::execution::Session>,
    image_path: String,
    images: crate::image_loader::Images,
    image_generation: u64,
    attachments: serde_json::Map<String, serde_json::Value>,
    execution_task: Task<()>,
    buffer: Buffer,
    syntax: RefCell<crate::syntax::Cache>,
    focus: FocusHandle,
    cell_focus: FocusHandle,
    marked: Option<Range<usize>>,
    undo: Vec<Buffer>,
    redo: Vec<Buffer>,
    composition_start: Option<Buffer>,
    lines: Rc<Vec<ShapedLine>>,
    starts: Rc<Vec<usize>>,
    text_layouts: Rc<RefCell<TextLayouts>>,
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
    pub fn view_offset(&self) -> (f32, f32) {
        let offset = self.scroll.offset();
        (offset.x.into(), offset.y.into())
    }

    pub fn restore_offset(&mut self, x: f32, y: f32) {
        self.scroll.set_offset(point(px(x.min(0.)), px(y.min(0.))));
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe_global::<crate::preferences::EditorPreferences>(|_, cx| cx.notify())
            .detach();
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
            filter_input: false,
            name_input: false,
            search_input: false,
            search_matches: vec![],
            search_active: None,
            search_preview: false,
            reveal_search: false,
            count_hint: 0,
            kind: ganymede_core::notebook::CellKind::Code,
            menu_open: false,
            menu_hovered: None,
            execution_count: None,
            running_frame: None,
            queued: false,
            output: Vec::new(),
            rich_output: Vec::new(),
            markdown_preview: None,
            input_prompt: None,
            input_return_focus: None,
            image_session: None,
            image_path: String::new(),
            images: Default::default(),
            image_generation: 0,
            attachments: Default::default(),
            execution_task: Task::ready(()),
            buffer: Buffer::default(),
            syntax: RefCell::default(),
            focus,
            cell_focus,
            marked: None,
            undo: vec![],
            redo: vec![],
            composition_start: None,
            lines: Rc::default(),
            starts: Rc::default(),
            text_layouts: Rc::default(),
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

    pub fn new_name_input(value: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self::new_filter(window, cx);
        this.name_input = true;
        this.set_text(value, cx);
        this.buffer.anchor = 0;
        this.buffer.head = this.buffer.text.len();
        this
    }
    pub fn new_search(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self::new_name_input(String::new(), window, cx);
        this.search_input = true;
        this
    }

    pub fn select_all_input(&mut self, cx: &mut Context<Self>) {
        self.buffer.anchor = 0;
        self.buffer.head = self.buffer.text.len();
        cx.notify();
    }

    pub fn selected_text(&self) -> &str {
        &self.buffer.text[self.buffer.selection()]
    }

    pub fn finish_find(&mut self, range: &Range<usize>, cx: &mut Context<Self>) {
        if range.end <= self.buffer.text.len()
            && self.buffer.text.is_char_boundary(range.start)
            && self.buffer.text.is_char_boundary(range.end)
        {
            self.buffer.anchor = range.start;
            self.buffer.head = range.end;
            cx.notify();
        }
    }

    pub fn set_search(
        &mut self,
        matches: Vec<Range<usize>>,
        active: Option<Range<usize>>,
        reveal: bool,
        cx: &mut Context<Self>,
    ) {
        if self.search_matches == matches && self.search_active == active && !reveal {
            return;
        }
        self.search_matches = matches;
        self.search_active = active;
        self.reveal_search = reveal;
        if self.search_active.is_some() && self.markdown_preview.is_some() {
            self.search_preview = true;
            self.markdown_preview = None;
        } else if self.search_active.is_none() && self.search_preview {
            self.search_preview = false;
            self.preview_markdown(cx);
        }
        cx.notify();
    }

    pub fn new_filter(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self::new(window, cx);
        this.filter_input = true;
        this
    }
    pub fn list_focus_handle(&self) -> FocusHandle {
        self.cell_focus.clone()
    }

    pub fn max_execution_count(&self) -> u64 {
        self.output
            .iter()
            .filter_map(|output| output.execution_count)
            .chain(self.execution_count)
            .max()
            .unwrap_or(0)
    }
    pub fn set_count_hint(&mut self, count: u64, cx: &mut Context<Self>) {
        if self.count_hint != count {
            self.count_hint = count;
            cx.notify();
        }
    }
    pub fn set_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.buffer = Buffer {
            text,
            ..Default::default()
        };
        self.marked = None;
        self.undo.clear();
        self.redo.clear();
        cx.emit(CellEvent::Changed);
        cx.notify();
    }

    pub fn load(&mut self, cell: &ganymede_core::notebook::Cell, cx: &mut Context<Self>) {
        self.buffer.text = cell.source();
        self.kind = cell.kind();
        self.markdown_preview = None;
        self.execution_count = cell.execution_count();
        self.set_output(crate::execution::output_blocks(cell.outputs()));
        self.attachments = cell
            .extra()
            .get("attachments")
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default();
        if self.kind == ganymede_core::notebook::CellKind::Markdown {
            self.preview_markdown(cx);
        }
        cx.notify();
    }

    fn set_output(&mut self, outputs: Vec<crate::execution::OutputBlock>) {
        self.rich_output = outputs
            .iter()
            .enumerate()
            .map(|(i, output)| {
                if self.output.get(i) == Some(output) {
                    self.rich_output[i].clone()
                } else {
                    crate::rich::Rich::output(output.data.as_ref(), &output.text)
                }
            })
            .collect();
        self.output = outputs;
    }
    pub fn set_image_session(&mut self, session: crate::execution::Session) {
        self.image_session = Some(session);
    }

    fn load_images(&mut self, cx: &mut Context<Self>) {
        let path = self
            .image_session
            .as_ref()
            .map(|s| s.path())
            .unwrap_or_default();
        if path != self.image_path {
            self.image_path = path.clone();
            self.images.clear();
            self.image_generation += 1;
        }
        let sources: std::collections::HashSet<String> = self
            .markdown_preview
            .iter()
            .chain(self.rich_output.iter())
            .flat_map(|rich| rich.image_sources())
            .take(32)
            .map(str::to_owned)
            .collect();
        // Keep images while a Markdown cell is being edited, so previewing again
        // doesn't redownload them; discard obsolete sources on the next preview.
        if self.markdown_preview.is_some()
            || self.kind != ganymede_core::notebook::CellKind::Markdown
        {
            self.images.retain(|source, _| sources.contains(source));
        }
        let Some(runtime) = cx.try_global::<crate::execution::Runtime>() else {
            return;
        };
        let handle = runtime.handle.clone();
        let config = self
            .image_session
            .as_ref()
            .map(|session| session.controller().server_config())
            .ok_or_else(|| "No notebook server connection".to_owned());
        for source in sources {
            if self.images.contains_key(&source) {
                continue;
            }
            self.images.insert(source.clone(), None);
            let request_source = source.clone();
            let path = path.clone();
            let config = config.clone();
            let generation = self.image_generation;
            let task = handle.spawn(async move {
                crate::image_loader::load(&request_source, &path, config).await
            });
            cx.spawn(async move |this, cx| {
                let result = task
                    .await
                    .unwrap_or_else(|_| Err("Image request interrupted".into()));
                let _ = this.update(cx, |this, cx| {
                    if this.image_generation == generation && this.images.contains_key(&source) {
                        this.images.insert(source, Some(result));
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    pub fn preview_markdown(&mut self, cx: &mut Context<Self>) {
        if self.kind == ganymede_core::notebook::CellKind::Markdown {
            self.markdown_preview = Some(crate::rich::Rich::markdown(
                &self.buffer.text,
                Some(&self.attachments),
            ));
            cx.notify();
        }
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
            self.markdown_preview = None;
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

    fn set_pending_input(
        &mut self,
        input: Option<ganymede_core::notebook::PendingInput>,
        session: &crate::execution::Session,
        cx: &mut Context<Self>,
    ) {
        if self.input_prompt.as_ref().map(|p| p.read(cx).id.as_str())
            == input.as_ref().map(|p| p.id.as_str())
        {
            return;
        }
        if let Some(previous) = self.input_prompt.take() {
            self.input_return_focus = Some(previous.read(cx).focus_handle(cx));
        }
        if let Some(input) = input {
            let prompt = cx.new(|cx| {
                stdin_prompt::StdinPrompt::new(input, session.clone(), self.cell_focus.clone(), cx)
            });
            cx.observe(&prompt, |_, _, cx| cx.notify()).detach();
            self.input_prompt = Some(prompt);
            cx.emit(CellEvent::InputRequested);
        }
        cx.notify();
    }

    pub fn clear_execution(&mut self, cx: &mut Context<Self>) {
        self.execution_task = Task::ready(());
        if let Some(prompt) = self.input_prompt.take() {
            self.input_return_focus = Some(prompt.read(cx).focus_handle(cx));
        }
        self.execution_count = None;
        self.output.clear();
        self.rich_output.clear();
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
                        data: None,
                    });
                }
                let done = update.done;
                if this
                    .update(cx, |this, cx| {
                        this.set_pending_input(
                            if done { None } else { update.input },
                            &session,
                            cx,
                        );
                        this.execution_count = update.count;
                        this.set_output(update.blocks);
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

    fn python_code(&self) -> bool {
        !self.document && !self.filter_input && self.kind == ganymede_core::notebook::CellKind::Code
    }

    fn indent(&mut self, outdent: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_composition();
        let next = self
            .buffer
            .indented(outdent, crate::preferences::editor(cx).indentation.text());
        if next == self.buffer {
            return;
        }
        self.edit(0..self.buffer.text.len(), &next.text);
        self.buffer.anchor = next.anchor;
        self.buffer.head = next.head;
        cx.emit(CellEvent::Changed);
        self.changed(window, cx);
    }

    fn changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.goal_x = None;
        self.reset_blink(cx);
        // Notebook cells wrap and grow; only single-line inputs and full document
        // editors own a scrolling viewport.
        if !self.filter_input && !self.document {
            self.scroll.set_offset(point(px(0.), px(0.)));
            return;
        }
        // Resolve scrolling in the input handler, before the next layout pass.
        let (row, column) = self.buffer.position(self.buffer.head);
        let line = self.buffer.text.split('\n').nth(row).unwrap_or("");
        let theme = cx.theme();
        let font_size = if self.filter_input {
            theme.font.xs
        } else {
            px(crate::preferences::editor(cx).font_size as f32)
        };
        let run = TextRun {
            len: line.len(),
            font: gpui::font(if self.filter_input {
                theme.font.sans.clone()
            } else {
                theme.font.mono.clone()
            }),
            color: theme.card.fg,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped =
            window
                .text_system()
                .shape_line(line.to_owned().into(), font_size, &[run], None);
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
            if self.document {
                if y + offset.y < px(0.) {
                    offset.y = -y;
                }
                if y + offset.y + self.line_height > viewport.height {
                    offset.y = viewport.height - y - self.line_height;
                }
            } else {
                // Notebook cells grow with their source, including newly inserted lines.
                offset.y = px(0.);
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
        let (row, col) = visual_position(&self.starts, self.buffer.head);
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
            self.starts[target] + self.lines[target].closest_index_for_x(goal)
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
        let start = self.starts[row];
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
                    .replace('\t', crate::preferences::editor(cx).indentation.text()),
                window,
                cx,
            );
        }
    }
}

impl Focusable for CodeCell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        if self.markdown_preview.is_some() {
            self.cell_focus.clone()
        } else {
            self.focus.clone()
        }
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
        let single_line;
        let text = if self.filter_input {
            single_line = text.replace(['\r', '\n'], " ");
            &single_line
        } else {
            text
        };
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
        let single_line;
        let text = if self.filter_input {
            single_line = text.replace(['\r', '\n'], " ");
            &single_line
        } else {
            text
        };
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
        let (row, col) = visual_position(&self.starts, range.start);
        let (end_row, end_col) = visual_position(&self.starts, range.end);
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
        if let Some(focus) = self.input_return_focus.take()
            && focus.is_focused(window)
        {
            self.cell_focus.focus(window, cx);
        }
        self.load_images(cx);
        let theme = cx.theme();
        let surface = theme.card;
        let prompt = match self.running_frame {
            _ if self.kind != ganymede_core::notebook::CellKind::Code => String::new(),
            Some(frame) => format!("[{}]:", SPINNER[frame]),
            None if self.queued => "[*]:".to_owned(),
            None => match self.execution_count {
                Some(count) => format!("[{count}]:"),
                None => "[ ]:".to_owned(),
            },
        };
        // Use the widest input/output prompt for this cell so prints and returns
        // remain aligned even when execution counts gain another digit.
        let gutter_width = std::iter::once(prompt.clone())
            .chain(std::iter::once(format!("[{}]:", self.count_hint)))
            .chain(
                self.output
                    .iter()
                    .filter_map(|output| output.execution_count.map(|count| format!("[{count}]:"))),
            )
            .fold(px(32.), |width, text| {
                let run = TextRun {
                    len: text.len(),
                    font: gpui::font(theme.font.mono.clone()),
                    color: surface.fg,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                width.max(
                    window
                        .text_system()
                        .shape_line(
                            text.into(),
                            px(crate::preferences::editor(cx).font_size as f32),
                            &[run],
                            None,
                        )
                        .width
                        + px(1.),
                )
            });
        let cell_padding = px(4.);
        let editor = div()
            .id("code-cell")
            .when(self.search_input, |view| {
                view.aria_label("Find in notebook source")
            })
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
            .text_size(px(crate::preferences::editor(cx).font_size as f32))
            .line_height(
                theme
                    .font
                    .line_height_tight
                    .max(px(crate::preferences::editor(cx).font_size as f32 * 1.5)),
            )
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
                    let prefix = &this.buffer.text[this.buffer.line_start()..this.buffer.head];
                    this.buffer.anchor = if !this.filter_input
                        && !prefix.is_empty()
                        && prefix.bytes().all(|b| b == b' ')
                    {
                        this.buffer.head
                            - ((prefix.len() - 1)
                                % crate::preferences::editor(cx).indentation.width()
                                + 1)
                    } else {
                        this.buffer.previous()
                    };
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
                if this.filter_input {
                    if this.search_input {
                        cx.emit(CellEvent::MoveDown);
                    } else if this.name_input {
                        cx.emit(CellEvent::RunAndAdvance);
                    }
                    return;
                }
                let offset = this.buffer.selection().start;
                let newline = crate::syntax::newline(&this.buffer.text, offset, this.python_code());
                this.replace_text_in_range(None, &newline, w, cx);
            }))
            .on_action(cx.listener(|this, _: &Tab, w, cx| {
                if this.name_input {
                    cx.emit(CellEvent::FocusNext);
                } else if this.filter_input {
                    w.focus_next(cx);
                } else {
                    if this.buffer.selection().is_empty() {
                        let width = crate::preferences::editor(cx).indentation.width();
                        let column = this.buffer.text[this.buffer.line_start()..this.buffer.head]
                            .chars()
                            .fold(0, |col, ch| {
                                if ch == '\t' {
                                    col + width - col % width
                                } else {
                                    col + 1
                                }
                            });
                        this.replace_text_in_range(
                            None,
                            &" ".repeat(width - column % width),
                            w,
                            cx,
                        );
                    } else {
                        this.indent(false, w, cx);
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &Outdent, w, cx| {
                if this.filter_input {
                    w.focus_prev(cx);
                } else {
                    this.indent(true, w, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &RunAndAdvance, window, cx| {
                if this.search_input {
                    cx.emit(CellEvent::MoveUp);
                } else if this.document {
                    this.replace_text_in_range(None, "\n", window, cx);
                } else if !this.filter_input {
                    cx.emit(CellEvent::RunAndAdvance);
                }
            }))
            .on_action(cx.listener(|this, _: &SelectCell, window, cx| {
                if this.filter_input {
                    cx.emit(CellEvent::DeleteCell);
                } else if !this.document {
                    this.cell_focus.focus(window, cx);
                }
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
                    .min_h(theme.font.line_height_tight)
                    .when(self.document, |s| s.h_full())
                    .when(self.document || self.filter_input, |s| {
                        s.overflow_x_scroll()
                    })
                    .when(self.document, |s| s.overflow_y_scroll())
                    .track_scroll(&self.scroll)
                    .child(CellText { cell: cx.entity() }),
            );

        if self.filter_input {
            return div()
                .on_mouse_down_out(cx.listener(|this, _, window, _| {
                    if !this.name_input && this.focus.is_focused(window) {
                        window.blur();
                    }
                }))
                .relative()
                .flex_1()
                .min_w_0()
                .child(
                    editor
                        .px_2()
                        .py(px(4.))
                        .pr_2()
                        .font_family(theme.font.sans.clone())
                        .text_size(theme.font.xs)
                        .line_height(theme.font.line_height_tight)
                        .when(self.search_input, |view| {
                            view.border_0()
                                .bg(theme.popover.bg)
                                .text_color(theme.popover.fg)
                        }),
                )
                .when(
                    self.buffer.text.is_empty() && (!self.name_input || self.search_input),
                    |view| {
                        view.child(
                            div()
                                .absolute()
                                // Match the editor's padding, border, font, and
                                // line box so placeholder and caret share a baseline.
                                .left(px(if self.search_input { 8. } else { 9. }))
                                .top(px(if self.search_input { 4. } else { 5. }))
                                .font_family(theme.font.sans.clone())
                                .text_size(theme.font.xs)
                                .line_height(
                                    theme.font.line_height_tight.max(px(
                                        crate::preferences::editor(cx).font_size as f32 * 1.5,
                                    )),
                                )
                                .text_color(surface.muted_fg)
                                .child(if self.search_input {
                                    "Find in cell source…"
                                } else {
                                    "Filter files…"
                                }),
                        )
                    },
                )
                .into_any_element();
        }
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
                .enumerate()
                .map(|(index, (label, icon, event))| {
                    let hovered = self.menu_hovered == Some(index);
                    let fg = if hovered {
                        popover.primary_fg
                    } else {
                        popover.fg
                    };
                    div()
                        .id(label)
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded(theme.radius.lg())
                        .cursor_pointer()
                        .font_family(theme.font.sans.clone())
                        .text_size(theme.font.sm)
                        .text_color(fg)
                        .when(hovered, |s| s.bg(popover.primary))
                        .on_hover(cx.listener(move |this, hovered, _, cx| {
                            if *hovered {
                                this.menu_hovered = Some(index);
                            } else if this.menu_hovered == Some(index) {
                                this.menu_hovered = None;
                            }
                            cx.notify();
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.menu_open = false;
                            cx.notify();
                            cx.emit(event);
                        }))
                        .child(icon.svg().size(px(16.)).text_color(fg))
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
                                    this.menu_hovered = None;
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
                    .w(gutter_width)
                    .whitespace_nowrap()
                    .ml_3()
                    .mr(px(6.))
                    .flex_shrink_0()
                    // Align with the editor's first line, including its border.
                    .pt(px(13.))
                    .font_family(theme.font.mono.clone())
                    .text_size(px(crate::preferences::editor(cx).font_size as f32))
                    .line_height(
                        theme
                            .font
                            .line_height_tight
                            .max(px(crate::preferences::editor(cx).font_size as f32 * 1.5)),
                    )
                    .text_color(if self.current {
                        surface.primary
                    } else {
                        theme.app.muted_fg
                    })
                    .text_right()
                    .child(prompt),
            )
            .child(if let Some(preview) = &self.markdown_preview {
                div()
                    .id("markdown-preview")
                    .flex_1()
                    .min_w_0()
                    .px_3()
                    .py_2()
                    .font_family(theme.font.sans.clone())
                    .text_size(theme.font.base)
                    .child(preview.render(window, cx, &self.images))
                    .on_click(cx.listener(|this, event: &gpui::ClickEvent, window, cx| {
                        cx.emit(CellEvent::Selected);
                        if event.click_count() == 2 {
                            this.markdown_preview = None;
                            this.focus.focus(window, cx);
                            cx.notify();
                        }
                    }))
                    .into_any_element()
            } else {
                editor.into_any_element()
            });

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
                this.markdown_preview = None;
                this.focus.focus(window, cx);
                cx.notify();
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
                                    .text_size(px(crate::preferences::editor(cx).font_size as f32))
                                    .line_height(theme.font.line_height_tight.max(px(
                                        crate::preferences::editor(cx).font_size as f32 * 1.5,
                                    )))
                                    .child(
                                        div()
                                            .w(gutter_width)
                                            .whitespace_nowrap()
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
                                            .child(self.rich_output[index].render(
                                                window,
                                                cx,
                                                &self.images,
                                            )),
                                    )
                            }),
                        )),
                )
            })
            .when_some(self.input_prompt.as_ref(), |cell, prompt| {
                cell.child(
                    div()
                        .ml(gutter_width + px(39.))
                        .mr(px(13.))
                        .child(prompt.clone()),
                )
            })
            .into_any_element()
    }
}

// Soft boundaries belong to the following visual row; source bytes are unchanged.
fn visual_position(starts: &[usize], offset: usize) -> (usize, usize) {
    let row = starts
        .partition_point(|start| *start <= offset)
        .saturating_sub(1);
    (
        row,
        offset.saturating_sub(starts.get(row).copied().unwrap_or(0)),
    )
}

// Keep two width variants: flex measurement may ask for intrinsic and constrained
// widths in the same frame. Scrolling and cursor blinking do not invalidate either.
#[derive(Clone, PartialEq)]
struct LayoutKey {
    source: Rc<str>,
    style: gpui::TextStyle,
    font_size: Pixels,
    line_height: Pixels,
    width: Option<Pixels>,
    marked: Option<Range<usize>>,
    syntax: Option<[gpui::Hsla; 8]>,
    gutter: Pixels,
}

#[derive(Default)]
struct TextLayouts {
    entries: Vec<(LayoutKey, Rc<Layout>)>,
}
impl TextLayouts {
    fn get(&self, key: &LayoutKey) -> Option<Rc<Layout>> {
        self.entries
            .iter()
            .find(|(cached, _)| cached == key)
            .map(|(_, layout)| layout.clone())
    }
    fn insert(&mut self, key: LayoutKey, layout: Rc<Layout>) {
        if self.entries.len() == 2 {
            self.entries.remove(0);
        }
        self.entries.push((key, layout));
    }
}

fn visible_rows(
    top: Pixels,
    line_height: Pixels,
    count: usize,
    clip_top: Pixels,
    clip_bottom: Pixels,
) -> Range<usize> {
    let first = (((clip_top - top) / line_height).floor().max(0.) as usize).min(count);
    let last = (((clip_bottom - top) / line_height).ceil().max(0.) as usize).min(count);
    first..last.max(first)
}

struct CellText {
    cell: Entity<CodeCell>,
}
struct Layout {
    lines: Rc<Vec<ShapedLine>>,
    starts: Rc<Vec<usize>>,
    numbers: Vec<Option<usize>>,
    size: gpui::Size<Pixels>,
    line_height: Pixels,
    cap_height: Pixels,
    empty_baseline: Pixels,
    gutter: Pixels,
}
struct Prepaint {
    visible: Range<usize>,
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
    type RequestLayoutState = Rc<RefCell<Rc<Layout>>>;
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
    ) -> (LayoutId, Rc<RefCell<Rc<Layout>>>) {
        let cell = self.cell.read(cx);
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let font_id = window.text_system().resolve_font(&style.font());
        let cap_height = window.text_system().cap_height(font_id, font_size);
        // Native font metrics and shaped-line metrics can use different descent
        // conventions. Measure a glyph through the same path as typed text.
        let caret_metrics =
            window
                .text_system()
                .shape_line("M".into(), font_size, &[style.to_run(1)], None);
        let empty_baseline = (line_height - caret_metrics.ascent - caret_metrics.descent) / 2.
            + caret_metrics.ascent;
        let highlights = if cell.python_code() {
            cell.syntax.borrow_mut().get(&cell.buffer.text)
        } else {
            Rc::new(vec![])
        };
        let colors = cx.theme().syntax;
        let layouts = cell.text_layouts.clone();
        // Reuse the stored source allocation on ordinary scroll/blink frames.
        let text: Rc<str> = layouts
            .borrow()
            .entries
            .last()
            .filter(|(key, _)| key.source.as_ref() == cell.buffer.text)
            .map(|(key, _)| key.source.clone())
            .unwrap_or_else(|| Rc::from(cell.buffer.text.as_str()));
        let gutter = if !cell.filter_input && crate::preferences::editor(cx).line_numbers {
            let digits = cell.buffer.text.split('\n').count().to_string().len();
            caret_metrics.width * digits as f32 + px(12.)
        } else {
            px(0.)
        };
        let key = LayoutKey {
            gutter,
            source: text.clone(),
            style: style.clone(),
            font_size,
            line_height,
            width: None,
            marked: cell.marked.clone(),
            syntax: cell.python_code().then_some(colors),
        };
        let marked = cell.marked.clone();
        let wrap = !cell.document && !cell.filter_input;
        let state = Rc::new(RefCell::new(Rc::new(Layout {
            lines: Rc::default(),
            starts: Rc::default(),
            numbers: vec![],
            size: size(px(0.), px(0.)),
            line_height,
            cap_height,
            empty_baseline,
            gutter,
        })));
        let measured = state.clone();
        let mut layout_style = Style::default();
        layout_style.min_size.width = gpui::relative(1.).into();
        let id =
            window.request_measured_layout(layout_style, move |known, available, window, _| {
                let wrap_width = if wrap {
                    known.width.or(match available.width {
                        gpui::AvailableSpace::Definite(width) => Some(width),
                        _ => None,
                    })
                } else {
                    None
                };
                let mut key = key.clone();
                key.width = wrap_width;
                if let Some(layout) = layouts.borrow().get(&key) {
                    let size = layout.size;
                    *measured.borrow_mut() = layout;
                    return size;
                }
                let mut starts = vec![];
                let mut numbers = vec![];
                let mut start = 0;
                let mut lines = vec![];
                for (logical_index, logical) in text.split('\n').enumerate() {
                    let mut ends = vec![];
                    if let Some(width) = wrap_width {
                        let mut wrapper =
                            window.text_system().line_wrapper(style.font(), font_size);
                        ends.extend(
                            wrapper
                                .wrap_line(
                                    &[gpui::LineFragment::text(logical)],
                                    (width - gutter - px(4.)).max(px(1.)),
                                )
                                .map(|boundary| boundary.ix),
                        );
                    }
                    ends.push(logical.len());
                    let mut previous = 0;
                    for end in ends {
                        let text = &logical[previous..end];
                        starts.push(start);
                        numbers.push((previous == 0).then_some(logical_index + 1));
                        let base = TextRun {
                            len: text.len(),
                            font: style.font(),
                            color: style.color,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let runs: Vec<_> = crate::syntax::segments(
                            start..start + text.len(),
                            &highlights,
                            marked.as_ref(),
                        )
                        .into_iter()
                        .map(|(range, syntax, composing)| TextRun {
                            len: range.len(),
                            color: syntax.map_or(base.color, |index| colors[index]),
                            underline: composing.then_some(UnderlineStyle {
                                color: Some(base.color),
                                thickness: px(1.),
                                wavy: false,
                            }),
                            ..base.clone()
                        })
                        .collect();

                        lines.push(window.text_system().shape_line(
                            text.to_owned().into(),
                            font_size,
                            &runs,
                            None,
                        ));
                        start += text.len();
                        previous = end;
                    }
                    start += 1;
                }
                let width = wrap_width.unwrap_or_else(|| {
                    lines.iter().map(|l| l.width).fold(px(0.), Pixels::max) + gutter + px(4.)
                });
                let height = line_height * lines.len().max(1) as f32;
                let layout = Rc::new(Layout {
                    lines: Rc::new(lines),
                    starts: Rc::new(starts),
                    numbers,
                    size: size(width, height),
                    line_height,
                    cap_height,
                    empty_baseline,
                    gutter,
                });
                layouts.borrow_mut().insert(key, layout.clone());
                *measured.borrow_mut() = layout;
                size(width, height)
            });
        (id, state)
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Rc<RefCell<Rc<Layout>>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let layout = state.borrow();
        let bounds = Bounds::new(
            point(bounds.left() + layout.gutter, bounds.top()),
            size(bounds.size.width - layout.gutter, bounds.size.height),
        );
        let cell = self.cell.read(cx);
        let selected = cell.buffer.selection();
        let theme = cx.theme();
        let mut selection = vec![];
        let clip = window.content_mask().bounds;
        let visible = visible_rows(
            bounds.top(),
            layout.line_height,
            layout.lines.len(),
            clip.top(),
            clip.bottom(),
        );
        for row in visible.clone() {
            let line = &layout.lines[row];
            let start = layout.starts[row];
            let end = start + line.len();
            let first = cell
                .search_matches
                .partition_point(|range| range.end <= start);
            for range in cell.search_matches[first..]
                .iter()
                .take_while(|range| range.start < end)
            {
                let left = line.x_for_index(range.start.saturating_sub(start));
                let right = line.x_for_index(range.end.min(end) - start);
                let active = cell.search_active.as_ref() == Some(range);
                selection.push(fill(
                    Bounds::new(
                        point(
                            bounds.left() + left,
                            bounds.top() + layout.line_height * row as f32,
                        ),
                        size(right - left, layout.line_height),
                    ),
                    if active {
                        theme.find_active
                    } else {
                        theme.find_match
                    },
                ));
            }
            if selected.start <= end && selected.end > start && !selected.is_empty() {
                let left = line.x_for_index(selected.start.saturating_sub(start).min(line.len()));
                let right = if selected.end > end {
                    (line.width + px(8.)).min(bounds.size.width)
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
        if cell.reveal_search
            && let Some(range) = &cell.search_active
        {
            let (row, col) = visual_position(&layout.starts, range.start);
            if let Some(line) = layout.lines.get(row) {
                window.request_autoscroll(Bounds::new(
                    point(
                        bounds.left() + line.x_for_index(col),
                        bounds.top() + layout.line_height * row as f32,
                    ),
                    size(gpui::px(2.), layout.line_height),
                ));
            }
        }
        let (row, col) = visual_position(&layout.starts, cell.buffer.head);
        let cursor =
            (cell.cursor_visible && selected.is_empty() && visible.contains(&row)).then(|| {
                let line = &layout.lines[row];
                // Match the baseline used by ShapedLine::paint, excluding line leading.
                // An empty line has no styled runs, so its shaped ascent and
                // descent are zero. Use the shaped reference glyph in that case.
                let baseline = if line.len() == 0 {
                    layout.empty_baseline
                } else {
                    (layout.line_height - line.ascent - line.descent) / 2. + line.ascent
                };
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
        Prepaint {
            selection,
            cursor,
            visible,
        }
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Rc<RefCell<Rc<Layout>>>,
        prepaint: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = state.borrow();
        if layout.gutter > px(0.) {
            let style = window.text_style();
            let font_size = style.font_size.to_pixels(window.rem_size());
            for row in prepaint.visible.clone() {
                let Some(number) = layout.numbers[row] else {
                    continue;
                };
                let text = number.to_string();
                let mut run = style.to_run(text.len());
                run.color = cx.theme().app.muted_fg;
                let number = window
                    .text_system()
                    .shape_line(text.into(), font_size, &[run], None);
                number
                    .paint(
                        point(
                            bounds.left() + layout.gutter - px(12.) - number.width,
                            bounds.top() + layout.line_height * row as f32,
                        ),
                        layout.line_height,
                        gpui::TextAlign::Left,
                        None,
                        window,
                        cx,
                    )
                    .expect("paint line number");
            }
        }
        let bounds = Bounds::new(
            point(bounds.left() + layout.gutter, bounds.top()),
            size(bounds.size.width - layout.gutter, bounds.size.height),
        );
        let focus = self.cell.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.cell.clone()),
            cx,
        );
        for quad in prepaint.selection.drain(..) {
            window.paint_quad(quad);
        }
        for row in prepaint.visible.clone() {
            let line = &layout.lines[row];
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
            cell.reveal_search = false;
            cell.lines = layout.lines.clone();
            cell.starts = layout.starts.clone();
            cell.bounds = Some(bounds);
            cell.line_height = layout.line_height;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn empty_search_input_uses_the_same_baseline_as_typed_text(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        let cell = cx.update(|window, cx| {
            cx.set_global(
                crate::theme::ThemeSpec::light().resolve(&crate::theme::Derivation::default()),
            );
            crate::assets::register_fonts(cx).unwrap();
            cx.new(|cx| CodeCell::new_search(window, cx))
        });
        cx.draw(point(px(0.), px(0.)), size(px(240.), px(30.)), |_, _| {
            cell.clone().into_any_element()
        });
        let empty_baseline = cx.update(|_, cx| {
            let cell = cell.read(cx);
            let cache = cell.text_layouts.borrow();
            let layout = &cache.entries.last().unwrap().1;
            assert_eq!(layout.lines[0].len(), 0);
            layout.empty_baseline
        });
        cx.update(|_, cx| cell.update(cx, |cell, cx| cell.set_text("Search".into(), cx)));
        cx.draw(point(px(0.), px(0.)), size(px(240.), px(30.)), |_, _| {
            cell.clone().into_any_element()
        });
        cx.update(|_, cx| {
            let cell = cell.read(cx);
            let cache = cell.text_layouts.borrow();
            let layout = &cache.entries.last().unwrap().1;
            let line = &layout.lines[0];
            let baseline = (layout.line_height - line.ascent - line.descent) / 2. + line.ascent;
            assert!((baseline - empty_baseline).abs() < px(0.01));
        });
    }

    #[gpui::test]
    fn find_reveals_a_match_deep_inside_a_virtualized_cell_without_editing_source(
        cx: &mut gpui::TestAppContext,
    ) {
        let cx = cx.add_empty_window();
        let source = format!("{}needle", "value = 1\n".repeat(200));
        let start = source.find("needle").unwrap();
        let cell = cx.update(|window, cx| {
            cx.set_global(
                crate::theme::ThemeSpec::light().resolve(&crate::theme::Derivation::default()),
            );
            crate::assets::register_fonts(cx).unwrap();
            cx.new(|cx| {
                let mut cell = CodeCell::new(window, cx);
                cell.set_text(source.clone(), cx);
                cell.set_search(
                    std::iter::once(start..start + 6).collect(),
                    Some(start..start + 6),
                    true,
                    cx,
                );
                cell
            })
        });
        let state = gpui::ListState::new(1, gpui::ListAlignment::Top, px(0.));
        struct TestView {
            cell: Entity<CodeCell>,
            state: gpui::ListState,
        }
        impl Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let cell = self.cell.clone();
                gpui::list(self.state.clone(), move |_, _, _| {
                    cell.clone().into_any_element()
                })
                .w_full()
                .h_full()
            }
        }
        cx.draw(point(px(0.), px(0.)), size(px(600.), px(300.)), |_, cx| {
            cx.new(|_| TestView {
                cell: cell.clone(),
                state: state.clone(),
            })
            .into_any_element()
        });
        assert!(state.logical_scroll_top().offset_in_item > px(1000.));
        cx.update(|_, cx| {
            cell.update(cx, |cell, cx| {
                assert_eq!(cell.text(), source);
                assert!(cell.undo.is_empty());
                cell.finish_find(&(start..start + 6), cx);
                cell.set_search(vec![], None, false, cx);
                assert_eq!(cell.selected_text(), "needle");
                assert!(cell.search_matches.is_empty());
                cell.set_kind(ganymede_core::notebook::CellKind::Markdown, cx);
                cell.preview_markdown(cx);
                assert!(cell.markdown_preview.is_some());
                cell.set_search(
                    std::iter::once(start..start + 6).collect(),
                    Some(start..start + 6),
                    true,
                    cx,
                );
                assert!(cell.markdown_preview.is_none());
                cell.set_search(vec![], None, false, cx);
                assert!(cell.markdown_preview.is_some());
                assert_eq!(cell.text(), source);
            });
        });
    }

    #[test]
    fn indentation_uses_selected_width_without_changing_source_characters() {
        for indentation in ["  ", "    ", "        "] {
            let text = "α = 1\nβ = 2".to_owned();
            let buffer = Buffer {
                anchor: 0,
                head: text.len(),
                text,
            };
            let result = buffer.indented(false, indentation);
            assert_eq!(
                result.text,
                format!("{indentation}α = 1\n{indentation}β = 2")
            );
            assert_eq!(result.indented(true, indentation), buffer);
        }
    }

    #[test]
    fn scroll_layout_cache_reuses_geometry_and_invalidates_visual_inputs() {
        let key = LayoutKey {
            source: Rc::from("print('hello')"),
            gutter: px(0.),
            style: gpui::TextStyle::default(),
            font_size: px(14.),
            line_height: px(20.),
            width: Some(px(500.)),
            marked: None,
            syntax: None,
        };
        let layout = Rc::new(Layout {
            lines: Rc::default(),
            starts: Rc::default(),
            numbers: vec![],
            size: size(px(500.), px(20.)),
            line_height: px(20.),
            cap_height: px(10.),
            empty_baseline: px(15.),
            gutter: px(0.),
        });
        let mut cache = TextLayouts::default();
        cache.insert(key.clone(), layout.clone());
        for _ in 0..120 {
            assert!(Rc::ptr_eq(&cache.get(&key).unwrap(), &layout));
        }
        let mut changed = key.clone();
        changed.width = Some(px(250.));
        assert!(cache.get(&changed).is_none());
        cache.insert(changed, layout.clone());
        assert!(
            cache.get(&key).is_some(),
            "intrinsic/constrained widths can coexist"
        );
        let mut changed = key.clone();
        changed.source = Rc::from("other");
        assert!(cache.get(&changed).is_none());
        let mut changed = key.clone();
        changed.marked = Some(0..5);
        assert!(cache.get(&changed).is_none());
        let mut changed = key.clone();
        changed.font_size = px(16.);
        assert!(cache.get(&changed).is_none());
        let mut changed = key.clone();
        changed.style.color = gpui::rgb(0xff0000).into();
        assert!(cache.get(&changed).is_none());
        let mut changed = key.clone();
        changed.syntax = Some([gpui::rgb(0x888888).into(); 8]);
        assert!(cache.get(&changed).is_none());
        cache.insert(changed, layout);
        assert_eq!(cache.entries.len(), 2);
    }

    #[test]
    fn scrolling_only_paints_rows_intersecting_the_viewport() {
        assert_eq!(
            visible_rows(px(0.), px(20.), 10_000, px(2000.), px(2600.)),
            100..130
        );
        assert_eq!(
            visible_rows(px(0.), px(20.), 10_000, px(2001.), px(2601.)),
            100..131
        );
        assert_eq!(visible_rows(px(800.), px(20.), 100, px(0.), px(600.)), 0..0);
        assert_eq!(
            visible_rows(px(-3000.), px(20.), 100, px(0.), px(600.)),
            100..100
        );
    }

    #[test]
    fn indentation_preserves_reversed_selection_and_excludes_next_line() {
        let buffer = Buffer {
            text: "é = 1\nprint(é)\nlast".into(),
            anchor: 17,
            head: 0,
        };
        // Use the exact byte start of the third line.
        let buffer = Buffer {
            anchor: buffer.text.find("last").unwrap(),
            ..buffer
        };
        let indented = buffer.indented(false, "    ");
        assert_eq!(indented.text, "    é = 1\n    print(é)\nlast");
        assert!(indented.anchor > indented.head);
        assert_eq!(indented.indented(true, "    "), buffer);
    }

    #[test]
    fn outdent_removes_one_tab_or_up_to_four_spaces() {
        let buffer = Buffer {
            text: "\tx\n  y\n    z".into(),
            anchor: 0,
            head: 13,
        };
        let buffer = Buffer {
            head: buffer.text.len(),
            ..buffer
        };
        let result = buffer.indented(true, "    ");
        assert_eq!(result.text, "x\ny\nz");
        assert_eq!(result.head, result.text.len());
        assert_eq!(result.anchor, 0);
        assert_eq!(result.indented(true, "    "), result);
    }

    #[test]
    fn visual_rows_map_soft_boundaries_and_newlines_without_changing_source_offsets() {
        // "abé" wraps before "😀z", followed by a newline and an empty row.
        let starts = [0, 4, 10];
        assert_eq!(visual_position(&starts, 2), (0, 2));
        assert_eq!(visual_position(&starts, 4), (1, 0));
        assert_eq!(visual_position(&starts, 8), (1, 4));
        assert_eq!(visual_position(&starts, 9), (1, 5));
        assert_eq!(visual_position(&starts, 10), (2, 0));
        assert_eq!(visual_position(&[0], 0), (0, 0));
    }

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
