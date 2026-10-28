# Core editing loop (plain text)

**Contents:** [Architecture](#the-architecture-in-one-diagram) · [State](#1-the-state-entity) · [Actions](#2-actions--key-bindings) · [EntityInputHandler](#3-entityinputhandler--the-ime-contract) · [Custom Element](#4-the-custom-element) · [Mouse](#5-mouse-handling) · [Render wiring](#6-render-focus--key_context--actions) · [Cursor blink](#7-cursor-blink) · [Checklist](#minimal-editor-checklist) · [Upstream defects](#known-defects-in-the-upstream-example)

The spine every editor level shares — single-line field through code editor. Runnable ground truth in the Zed repo:

- `crates/gpui/examples/input.rs` (~780 lines) — single-line input with selection, IME, clipboard, mouse. `cargo run -p gpui --example input`
- `crates/gpui/examples/view_example/example_editor.rs` (~550 lines) — multi-line editor with blink, focus subscriptions, tests. `cargo run -p gpui --example view_example`

For multi-line layout and wrapping see multiline-and-wrap.md; for styled/highlighted text see highlighting.md.

## The architecture in one diagram

```
                keystroke
                    │
      ┌─────────────┴───────────────┐
      │ named key? (keymap match)   │ printable char / IME?
      ▼                             ▼
  Action (Backspace, Left, …)   platform text input
      │ .on_action handlers         │ EntityInputHandler (UTF-16 boundary)
      └─────────────┬───────────────┘
                    ▼
        replace_text_in_range / move_to / select_to     ← ALL mutation funnels here
                    ▼  cx.notify()
              Render → custom Element
        request_layout: size only
        prepaint:  shape visible lines (shape_line + TextRuns) → cursor/selection quads
        paint:     handle_input(registration!) → selection → text → cursor
                   → stash last_layout/last_bounds on the entity  ← mouse + IME geometry read these next frame
```

Two input channels, one `FocusHandle`. Deletion = "select the range, then replace selection with `""`" — exactly one code path edits text.

## 1. The state entity

```rust
struct TextInput {
    focus_handle: FocusHandle,            // cx.focus_handle()
    content: SharedString,                // the document (String/Rope for documents — see large-text.md)
    placeholder: SharedString,
    selected_range: Range<usize>,         // BYTE offsets, always normalized start <= end
    selection_reversed: bool,             // which end is the moving head
    marked_range: Option<Range<usize>>,   // IME pre-edit (composition) region
    last_layout: Option<ShapedLine>,      // last frame's shaping — hit testing
    last_bounds: Option<Bounds<Pixels>>,  //   "        element bounds
    is_selecting: bool,                   // drag in progress
}

impl TextInput {
    fn cursor_offset(&self) -> usize {
        if self.selection_reversed { self.selected_range.start } else { self.selected_range.end }
    }
    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        cx.notify()
    }
    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed { self.selected_range.start = offset }
        else { self.selected_range.end = offset };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }
}
```

A collapsed selection *is* the caret. (For the multi-selection generalization see selections-and-multicursor.md.) Cursor movement steps over **grapheme clusters**, not chars/bytes (`unicode-segmentation` crate):

```rust
fn previous_boundary(&self, offset: usize) -> usize {
    self.content.grapheme_indices(true).rev()
        .find_map(|(idx, _)| (idx < offset).then_some(idx)).unwrap_or(0)
}
fn next_boundary(&self, offset: usize) -> usize {
    self.content.grapheme_indices(true)
        .find_map(|(idx, _)| (idx > offset).then_some(idx)).unwrap_or(self.content.len())
}
```

(These scan the whole string — fine for a field; use a rope + cached line starts for documents.)

## 2. Actions + key bindings

```rust
actions!(text_input, [Backspace, Delete, Left, Right, SelectLeft, SelectRight,
                      SelectAll, Home, End, ShowCharacterPalette, Paste, Cut, Copy]);

cx.bind_keys([
    KeyBinding::new("backspace",       Backspace,            Some("TextInput")),
    KeyBinding::new("delete",          Delete,               Some("TextInput")),
    KeyBinding::new("left",            Left,                 Some("TextInput")),
    KeyBinding::new("right",           Right,                Some("TextInput")),
    KeyBinding::new("shift-left",      SelectLeft,           Some("TextInput")),
    KeyBinding::new("shift-right",     SelectRight,          Some("TextInput")),
    KeyBinding::new("cmd-a",           SelectAll,            Some("TextInput")),
    KeyBinding::new("cmd-v",           Paste,                Some("TextInput")),
    KeyBinding::new("cmd-c",           Copy,                 Some("TextInput")),
    KeyBinding::new("cmd-x",           Cut,                  Some("TextInput")),
    KeyBinding::new("home",            Home,                 Some("TextInput")),
    KeyBinding::new("end",             End,                  Some("TextInput")),
    KeyBinding::new("ctrl-cmd-space",  ShowCharacterPalette, Some("TextInput")),
]);
```

**Printable characters are not actions** — they arrive via the input-handler channel. Handlers are `fn(&mut Self, &Action, &mut Window, &mut Context<Self>)`:

```rust
fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
    if self.selected_range.is_empty() {
        let prev = self.previous_boundary(self.cursor_offset());
        if self.cursor_offset() == prev { window.play_system_bell(); return; }
        self.select_to(prev, cx);
    }
    self.replace_text_in_range(None, "", window, cx)   // the one mutation funnel
}
fn copy(&mut self, _: &Copy, _w: &mut Window, cx: &mut Context<Self>) {
    if !self.selected_range.is_empty() {
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.content[self.selected_range.clone()].to_string()));
    }
}
fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
        self.replace_text_in_range(None, &text.replace("\n", " "), window, cx)  // single-line: strip \n
    }
}
```

`Delete` mirrors backspace with `next_boundary`; `Cut` = copy + replace-with-empty; `Left` on a non-empty selection collapses to its start without moving. A full multi-line action vocabulary (word deletes, line moves, page moves, indent/outdent) proven in production — gpui-component's set, registered under one `"Input"` context:

> Backspace, Delete, DeleteToBeginningOfLine, DeleteToEndOfLine, DeleteToPreviousWordStart, DeleteToNextWordEnd, Indent, Outdent, MoveUp/Down/Left/Right, MoveHome/End, MovePageUp/PageDown, MoveToStartOfLine/EndOfLine/Start/End, MoveToPreviousWord/NextWord, SelectAll, SelectToStartOfLine/EndOfLine/Start/End, SelectToPreviousWordStart/NextWordEnd, ShowCharacterPalette, Copy, Cut, Paste, Undo, Redo, Escape, Search, Replace, Enter { secondary, shift }

Zed's split rule is worth internalizing: a key becomes an *action* when the command needs editor context (Backspace is an action because of autoclose pairs and linked edits); anything unmatched falls through to the text-input system.

## 3. `EntityInputHandler` — the IME contract

This trait is how CJK input, dead keys, accent popups, and emoji palettes work. **All `_utf16` parameters and returns are UTF-16 code-unit offsets** (the `NSTextInputClient`/TSF vocabulary). Convert at the boundary; keep bytes inside:

```rust
fn offset_from_utf16(&self, offset: usize) -> usize {
    let (mut utf8, mut utf16) = (0, 0);
    for ch in self.content.chars() {
        if utf16 >= offset { break; }
        utf16 += ch.len_utf16();
        utf8  += ch.len_utf8();
    }
    utf8
}
// + mirror offset_to_utf16, and range_{from,to}_utf16 mapping both ends
```

The methods:

| Method | Purpose |
|---|---|
| `text_for_range(range_utf16, adjusted_range, window, cx) -> Option<String>` | OS asks "what text is here" (candidate windows, accessibility). Clamp; report what you used via `adjusted_range`. |
| `selected_text_range(ignore_disabled, ..) -> Option<UTF16Selection>` | Caret/selection as `UTF16Selection { range, reversed }`. Return `None` when input disabled — that's what suppresses the macOS press-and-hold accent menu. |
| `marked_text_range(&self, ..) -> Option<Range<usize>>` | Current composition region, or `None`. |
| `unmark_text(..)` | Abandon composition: `self.marked_range = None`. |
| `replace_text_in_range(range_utf16: Option<..>, new_text, ..)` | **Commit text. The funnel.** |
| `replace_and_mark_text_in_range(range_utf16, new_text, new_selected_range_utf16, ..)` | Composition *update* (typing romaji→kana). |
| `bounds_for_range(range_utf16, element_bounds, ..) -> Option<Bounds<Pixels>>` | Where the OS puts the IME candidate window (also reusable for your own popover anchoring — overlays.md). |
| `character_index_for_point(point, ..) -> Option<usize>` | Reverse hit-test for accessibility. |
| `accepts_text_input(..) -> bool` (has default) | Return false for read-only/disabled. |

The two core impls:

```rust
fn replace_text_in_range(&mut self, range_utf16: Option<Range<usize>>, new_text: &str,
                         _window: &mut Window, cx: &mut Context<Self>) {
    // priority: explicit range > marked (composition) range > current selection
    let range = range_utf16.as_ref().map(|r| self.range_from_utf16(r))
        .or(self.marked_range.clone())
        .unwrap_or(self.selected_range.clone());
    self.content = (self.content[0..range.start].to_owned() + new_text
                    + &self.content[range.end..]).into();
    self.selected_range = range.start + new_text.len()..range.start + new_text.len();
    self.marked_range.take();
    cx.notify();
}

fn replace_and_mark_text_in_range(&mut self, range_utf16: Option<Range<usize>>, new_text: &str,
                                  new_selected_range_utf16: Option<Range<usize>>,
                                  _window: &mut Window, cx: &mut Context<Self>) {
    let range = range_utf16.as_ref().map(|r| self.range_from_utf16(r))
        .or(self.marked_range.clone())
        .unwrap_or(self.selected_range.clone());
    self.content = (self.content[0..range.start].to_owned() + new_text
                    + &self.content[range.end..]).into();
    self.marked_range = (!new_text.is_empty())
        .then(|| range.start..range.start + new_text.len());
    // new_selected_range_utf16 is RELATIVE to the inserted text — rebase by range.start
    self.selected_range = new_selected_range_utf16.as_ref()
        .map(|r| self.range_from_utf16(r))
        .map(|r| range.start + r.start..range.start + r.end)
        .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
    cx.notify();
}

fn bounds_for_range(&mut self, range_utf16: Range<usize>, bounds: Bounds<Pixels>, ..) -> Option<Bounds<Pixels>> {
    let line = self.last_layout.as_ref()?;
    let range = self.range_from_utf16(&range_utf16);
    Some(Bounds::from_corners(
        point(bounds.left() + line.x_for_index(range.start), bounds.top()),
        point(bounds.left() + line.x_for_index(range.end),   bounds.bottom())))
}
```

The composition (marked) range renders with an underline — see highlighting.md §Styled runs. IME edits should group into one undo step — see undo.md.

**Minimum-viable ladder** (what `example_editor.rs` does): implement `text_for_range`, `selected_text_range`, `replace_text_in_range`; make `marked_text_range` return `None`, `unmark_text` a no-op, `replace_and_mark_text_in_range` delegate to `replace_text_in_range`, geometry methods return `None`. Typing works; add real IME later.

**Multi-cursor + IME** (Zed, `crates/editor/src/input.rs`): the OS reports a single-cursor view. Zed re-interprets incoming ranges — marked ranges win; an empty range means "at cursor"; a non-empty range with no marked text is fanned out to every cursor relative to the newest one.

## 4. The custom Element

```rust
struct TextElement { input: Entity<TextInput> }

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self { self }
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;
    fn id(&self) -> Option<ElementId> { None }             // stateless element; state lives on the entity
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> { None }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>,
                      window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();   // single line tall
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>,
                bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) -> PrepaintState {
        let input = self.input.read(cx);
        let style = window.text_style();
        let (display_text, color) = if input.content.is_empty() {
            (input.placeholder.clone(), hsla(0., 0., 0., 0.2))
        } else {
            (input.content.clone(), style.color)
        };

        // Build TextRuns (see highlighting.md for styled/multi-run text), then shape:
        let run = TextRun { len: display_text.len(), font: style.font(), color,
                            background_color: None, underline: None, strikethrough: None };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window.text_system().shape_line(display_text, font_size, &[run], None);
        // shape_line handles ONE line: no '\n', no wrapping (multiline-and-wrap.md).

        let cursor_x = line.x_for_index(input.cursor_offset());
        let (selection, cursor) = if input.selected_range.is_empty() {
            (None, Some(fill(
                Bounds::new(point(bounds.left() + cursor_x, bounds.top()),
                            size(px(2.), bounds.bottom() - bounds.top())),
                gpui::blue())))
        } else {
            (Some(fill(
                Bounds::from_corners(
                    point(bounds.left() + line.x_for_index(input.selected_range.start), bounds.top()),
                    point(bounds.left() + line.x_for_index(input.selected_range.end),   bounds.bottom())),
                rgba(0x3311ff30))), None)
        };
        PrepaintState { line: Some(line), cursor, selection }
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>,
             bounds: Bounds<Pixels>, _: &mut (), prepaint: &mut PrepaintState,
             window: &mut Window, cx: &mut App) {
        let focus_handle = self.input.read(cx).focus_handle.clone();

        // ★ THE registration that makes typing work at all — every frame, in paint:
        window.handle_input(&focus_handle, ElementInputHandler::new(bounds, self.input.clone()), cx);

        if let Some(selection) = prepaint.selection.take() { window.paint_quad(selection); }
        let line = prepaint.line.take().unwrap();
        line.paint(bounds.origin, window.line_height(), TextAlign::Left, None, window, cx).unwrap();
        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take() { window.paint_quad(cursor); }

        // stash for next frame's mouse + IME geometry
        self.input.update(cx, |input, _| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}
```

Paint order matters: **selection quad → text → cursor quad**. Cursor only when focused.

`ShapedLine` surface: `.text`, `.width`, `.len`, `x_for_index(byte) -> Pixels`, `index_for_x(x) -> Option<usize>` (None past end), `closest_index_for_x(x) -> usize` (clamps — use for clicks), `.paint(origin, line_height, align, content_width, window, cx)`.

**Horizontal scroll** (absent upstream): add `scroll_offset: Pixels` to the entity; in prepaint clamp it so the cursor stays inside `bounds` (and to `(line.width - bounds.size.width).max(px(0.))`); subtract it from every painted x and the `paint` origin; add it back before `closest_index_for_x` in hit tests; `.overflow_hidden()` on the wrapper div.

## 5. Mouse handling

```rust
fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
    if self.content.is_empty() { return 0; }
    let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else { return 0; };
    if position.y < bounds.top()    { return 0; }                    // drag above → start
    if position.y > bounds.bottom() { return self.content.len(); }   // drag below → end
    line.closest_index_for_x(position.x - bounds.left())             // position is WINDOW-space
}

fn on_mouse_down(&mut self, ev: &MouseDownEvent, _w: &mut Window, cx: &mut Context<Self>) {
    self.is_selecting = true;
    let ix = self.index_for_mouse_position(ev.position);
    if ev.modifiers.shift { self.select_to(ix, cx); } else { self.move_to(ix, cx); }
}
fn on_mouse_up(&mut self, _: &MouseUpEvent, _w: &mut Window, _cx: &mut Context<Self>) {
    self.is_selecting = false;
}
fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _w: &mut Window, cx: &mut Context<Self>) {
    if self.is_selecting { self.select_to(self.index_for_mouse_position(ev.position), cx); }
}
```

Wire with `.on_mouse_down(MouseButton::Left, ..)`, `.on_mouse_up(..)`, **`.on_mouse_up_out(..)`** (forgetting it leaves `is_selecting` stuck after a drag ends outside), `.on_mouse_move(..)`.

Double/triple click: `MouseDownEvent.click_count` — `2` → word, `3` → line, `4` → all (see selections-and-multicursor.md §Select modes for keeping shift-drag snapped to words after a double-click).

## 6. Render: focus + key_context + actions

```rust
impl Render for TextInput {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .key_context("TextInput")               // names the keymap context
            .track_focus(&self.focus_handle(cx))    // makes the subtree a focus target
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            // ... one per action ...
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .line_height(px(30.))                   // inherited: window.line_height() in the element
            .text_size(px(24.))                     // inherited: window.text_style() in the element
            .child(div().h(px(38.)).w_full().p(px(4.)).bg(white())
                   .child(TextElement { input: cx.entity() }))
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle { self.focus_handle.clone() }
}
```

Text style flows down by inheritance — `.line_height()`/`.text_size()` on the div are what the element reads via `window.line_height()`/`window.text_style()`. Embed with `.child(entity.clone())` (an `Entity<T: Render>` is an element). Focus at startup must be **deferred** — the window doesn't exist inside the build closure: `window.defer(cx, move |window, cx| focus_handle.focus(window, cx))` (or `window.focus(&handle, cx)` after the view exists).

## 7. Cursor blink

```rust
// fields: cursor_visible: bool, _blink_task: Task<()>
fn spawn_blink_task(cx: &mut Context<Self>) -> Task<()> {
    cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor().timer(Duration::from_millis(500)).await;
            if this.update(cx, |e, cx| { e.cursor_visible = !e.cursor_visible; cx.notify(); }).is_err() {
                break;   // entity dropped
            }
        }
    })
}
fn reset_blink(&mut self, cx: &mut Context<Self>) {   // call after EVERY edit/move
    self.cursor_visible = true;
    self._blink_task = Self::spawn_blink_task(cx);    // dropping the old Task cancels it
}
```

Start on focus, stop on blur: `cx.on_focus(&handle, window, |this, _, cx| this.reset_blink(cx))` / `cx.on_blur(..)`. Zed's production version (`crates/editor/src/blink_manager.rs`) uses an epoch counter so stale timers no-op, plus a brief "pause blinking while typing" window — copy that if the simple version flickers.

If the text lives in a shared `Entity<String>` others can mutate, `cx.observe(&value, ..)` and clamp the cursor back to a `char_boundary` before the next IME round-trip slices the string.

## Read-only, disabled, and events

Three-state model (from gpui-component, worth adopting): **read-only** keeps normal appearance, focus, selection, and copy — it only rejects user edits; **disabled** rejects interaction entirely; programmatic edits bypass both. Gate user paths via `accepts_text_input()` and a check at the top of `replace_text_in_range`.

Outward events — keep them coarse (subscribers who need the text read the state):

```rust
pub enum InputEvent { Change, PressEnter { secondary: bool, shift: bool }, Focus, Blur }
impl EventEmitter<InputEvent> for TextInput {}
```

`PressEnter` carrying modifiers lets a chat box do "enter sends, shift-enter newlines" in one match arm.

## Minimal editor checklist

- [ ] State entity: content, normalized `selected_range` + `reversed`, `marked_range`, `last_layout`/`last_bounds`, `FocusHandle`, `impl Focusable`
- [ ] Grapheme boundary helpers + UTF-16⇄byte converters
- [ ] `actions!` + handlers + `cx.bind_keys` with a key context; clipboard via `cx.write_to_clipboard`/`read_from_clipboard`
- [ ] `impl EntityInputHandler` (minimum: `text_for_range`, `selected_text_range`, `replace_text_in_range`; stubs for the rest, real IME later)
- [ ] Custom `Element`: layout size → prepaint shapes + quads → paint registers `ElementInputHandler`, paints selection/text/cursor, stashes layout
- [ ] Mouse down/move/up/**up_out**, click_count for word/line select
- [ ] `Render`: `key_context` + `track_focus` + `on_action`s + mouse handlers + text style
- [ ] Blink task; horizontal scroll clamp
- [ ] Then: multiline-and-wrap.md → undo.md → the features you need
- [ ] Tests: `#[gpui::test]`, `cx.add_window_view`, `cx.simulate_input("hello")`, `cx.simulate_keystrokes("left left")` (see `examples/view_example/example_tests.rs`; needs `--features test-support`)

Deps: `unicode-segmentation` for graphemes/words; `gpui_platform::application()` for the entry point.

## Known defects in the upstream example

Don't copy these from `input.rs` verbatim:

- `character_index_for_point` computes `point.x - line_point.x` where `line_point` is already the *localized* point — it should pass `line_point.x` to `index_for_x`. It also `assert_eq!(last_layout.text, self.content)`, which **panics while the placeholder is shown** — guard on `content.is_empty()` first.
- `replace_and_mark_text_in_range`'s selection rebase adds `range.start` to the start but `range.end` to the end (asymmetric; wrong when the replaced range is non-empty). Use the symmetric rebase in §3.
- Key bindings registered with context `None` despite the element's `key_context("TextInput")` — pass `Some("TextInput")`.
- No undo — see undo.md.
