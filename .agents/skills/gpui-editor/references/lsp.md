# LSP integration: diagnostics, completion, hover, definitions

**Contents:** [Coordinate conversions](#the-coordinate-conversion-zoo) · [Diagnostics](#diagnostics-push-model) · [Provider traits](#async-features-pull-model-provider-traits) · [Async discipline](#async-discipline) · [Semantic tokens & colors](#semantic-tokens--document-colors) · [Applying server edits](#applying-server-edits)

How editor features talk to a language server (or anything shaped like one — a linter, an AI backend, your own analyzer). The architecture lesson from both references: **the editor never speaks JSON-RPC**. It exposes seams (a diagnostic set you push into; provider traits it pulls from), and the LSP client is an ordinary app component behind them.

## The coordinate conversion zoo

Three coordinate systems meet here; conversion errors are the classic LSP-integration bug:

| System | Used by | Unit |
|---|---|---|
| byte offset / `Point { row, byte_col }` | your editor internals | UTF-8 bytes |
| `lsp_types::Position { line, character }` | LSP wire (default) | **UTF-16** code units, 0-based |
| `OffsetUtf16` | IME (core-editing.md) | UTF-16 code units |

Put the conversions on your rope as an extension trait (`offset_to_position`, `position_to_offset`, and the row/col variants) and convert **at the seam boundary** — providers take byte offsets, the LSP client converts to/from `Position` when serializing. (LSP 3.17 lets you negotiate UTF-8 positions; don't count on servers supporting it.) Reuse `lsp_types` structs on your seams (`Hover`, `LocationLink`, `Diagnostic`, `Position`) instead of inventing parallel types — both references do.

## Diagnostics: push model

No provider trait — diagnostics arrive when they arrive (LSP publishes them unsolicited), so the editor just owns a queryable set the app pushes into:

```rust
pub struct DiagnosticSet { /* interval-queryable: gpui-component backs it with a sum tree */ }
impl DiagnosticSet {
    pub fn push(&mut self, d: impl Into<Diagnostic>);
    pub fn extend<I>(&mut self, diags: I);
    pub fn clear(&mut self);
    pub fn range(&self, r: Range<usize>) -> impl Iterator<Item = &DiagnosticEntry>;  // per visible range
    pub fn for_offset(&self, o: usize) -> Option<&DiagnosticEntry>;                  // hover lookup
}

pub struct Diagnostic { range, message, severity, code, source }   // From<lsp_types::Diagnostic>
```

Usage: on `textDocument/publishDiagnostics`, convert positions → byte ranges, `clear()` + `extend(..)`, notify. Rendering fans out to existing mechanisms: wavy underlines colored by severity → a decoration collection (decoration.md); gutter icons per row; hover card at the offset → overlays.md; scrollbar markers → the same lane as search matches (search.md). Store entry ranges as anchors (or refresh the whole set on every publish, which is what servers do anyway — versioned so stale publishes for an old document version are dropped).

## Async features: pull model (provider traits)

Request/response features are traits the app implements and hands to the editor — plain `Option<Rc<dyn …>>` fields work fine (gpui-component, `crates/base/src/input/editor/lsp/`):

```rust
pub trait CompletionProvider {
    fn completions(&self, text: &Rope, offset: usize, trigger: CompletionContext,
                   window: &mut Window, cx: &mut App) -> Task<Result<CompletionResponse>>;
    /// Called synchronously on every insertion — cheap check: should we open/refresh the menu?
    fn is_completion_trigger(&self, offset: usize, new_text: &str, cx: &mut App) -> bool;
    // optional: inline (ghost-text) completions with their own debounce; resolve for lazy detail
}
pub trait HoverProvider {
    fn hover(&self, text: &Rope, offset: usize, ..) -> Task<Result<Option<lsp_types::Hover>>>;
}
pub trait DefinitionProvider {   // cmd-hover underline, cmd-click, GoToDefinition action
    fn definitions(&self, text: &Rope, offset: usize, ..) -> Task<Result<Vec<lsp_types::LocationLink>>>;
}
pub trait CodeActionProvider {
    fn id(&self) -> SharedString;                                  // multiple providers can coexist
    fn code_actions(&self, editor: Entity<EditorState>, range: Range<usize>, ..)
        -> Task<Result<Vec<CodeAction>>>;
    fn perform_code_action(&self, editor: Entity<EditorState>, action: CodeAction, ..)
        -> Task<Result<()>>;
}
```

Shape notes: providers get `&Rope` + byte offset (read-only view, cheap), return GPUI `Task`s; the *editor* owns debounce, menu state, and rendering (overlays.md) — providers are data-in, not UI-in. Code actions get the editor entity because performing an action edits the buffer. Navigation targets outside this document (`LocationLink` to another file) resolve through an app-supplied open-document callback — the editor can't open files.

## Async discipline

- Provider calls run through `cx.spawn` orchestration; results apply via `this.update(..)` — standard GPUI async.
- **Cancel by drop**: keep the in-flight `Task` on the menu/popover state; a newer request or a dismiss drops it.
- **Debounce** user-driven triggers: completions ~100–300 ms after the triggering keystroke, hover ~300 ms of dwell, inline completions ~300 ms (gpui-component's default).
- **Staleness check**: record the document version (an edit counter) with each request; discard responses for old versions instead of applying edits at shifted offsets.
- Design invalidation explicitly: gpui-component's provider assignment only takes effect on the next text change unless `refresh()` is called — a documented wart. Prefer setters that notify/invalidate immediately.

## Semantic tokens & document colors

- **Semantic tokens** (server-computed highlighting: "this identifier is a mutable static") layer *over* tree-sitter syntax: a provider returns ranges + token-type names; resolve names → styles through the same theme resolver as syntax captures (highlighting.md), merged with higher priority than syntax, lower than decorations. Because resolution is by name at paint time, theme switches don't refetch.
- **Document colors**: provider returns ranges + colors; render a swatch chip *inline* before each range — an inline widget (decoration.md §Inline widgets).

## Applying server edits

Formatting, rename, and code actions return multi-edit payloads (`TextEdit[]` / `WorkspaceEdit`). Rules: convert all positions **against the document version the server saw** (staleness check), sort and apply back-to-front so earlier offsets stay valid, wrap the whole batch in **one undo transaction** (undo.md), and route through the normal mutation funnel so events/highlighting/decorations all update. `replace_all` semantics (undo-preserving), never `set_value`.
