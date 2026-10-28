# Undo / redo

**Contents:** [Transaction model](#the-transaction-model) · [Grouping](#grouping) · [Selections in undo](#selections-in-undo) · [IME grouping](#ime-grouping) · [Two verbs for "replace everything"](#two-verbs-for-replace-everything) · [API shape](#api-shape)

## The transaction model

Undo units are **transactions**, not keystrokes. The simple implementation (adequate for single-user; both references' behavior distilled):

```rust
struct Transaction {
    inverse_edits: Vec<(Range<usize>, String)>,  // applying these reverts the transaction
    selections_before: Vec<Selection<usize>>,    // restore on undo
    selections_after: Vec<Selection<usize>>,     // restore on redo
    at: Instant,                                 // for grouping
}
struct History {
    undo_stack: Vec<Transaction>,
    redo_stack: Vec<Transaction>,
    transaction_depth: usize,        // nesting: begin/end are depth-counted
    group_interval: Duration,        // ~300 ms
}
```

Rules (all load-bearing):

- Every mutation records its inverse into the open transaction. Since all edits flow through `replace_text_in_range` (core-editing.md), that's **one** instrumentation point: the inverse of `replace(range, new)` is `replace(range.start..range.start + new.len(), old_text)`.
- `begin`/`end` are **depth-counted** so compound commands ("replace all", auto-indent after Enter) nest freely and land as one undo step. An empty transaction pops itself on end.
- **New edits clear the redo stack.**
- Undo pops → apply inverse edits (in reverse order) → restore `selections_before` → push onto redo. Redo mirrors.
- Cap the stack (count and/or bytes) — gpui-component bounds both retained transactions and change-list size.

Zed's implementation differs internally (undo is op-based: counters flip CRDT-fragment visibility, so concurrent collaborative undos converge — `crates/text/src/text.rs` `History`/`UndoMap`) but exposes the same transaction semantics. Copy the semantics, not the CRDT (selections-and-multicursor.md §Collaboration).

## Grouping

A burst of typing must be one undo step: when ending a transaction, if the gap since the previous entry `< group_interval` (~300 ms in Zed's release builds; 0 in tests) **and** the edits are adjacent (gpui-component checks adjacency too), merge into the previous entry. Explicit boundaries beat time sometimes: break coalescing on cursor movement, on Enter, and around commands like paste so "undo" doesn't swallow surprising amounts.

## Selections in undo

Undo should restore *where you were*, not just the text — hence `selections_before`/`selections_after` on each transaction. Record them in the selection mutation gate (`change_selections` history — selections-and-multicursor.md) or at transaction begin/end.

## IME grouping

A composition session (typing romaji → committed kanji) is several `replace_and_mark`/`replace_text_in_range` calls but must be **one undo step**: open (or remember) a transaction at composition start and group everything until commit/unmark into it. Zed tracks `ime_transaction: Option<TransactionId>` and calls `group_until_transaction(id)` after each IME event.

## Two verbs for "replace everything"

A distinction from gpui-component that prevents two classic bugs:

| Verb | History | Change events | Use for |
|---|---|---|---|
| `set_value(text)` | **clears** undo stack | suppressed | loading a different document |
| `replace_all(text)` | one normal transaction | emitted | a formatter/linter rewriting the buffer |

Without it you get either "undo un-loads the file" or "the formatter destroyed my undo history". Document which of your mutation APIs emit events — their silent `set_value` is a recurring footgun for consumers.

## API shape

Drive undo/redo through **actions** (`Undo`/`Redo` bound to cmd-z / cmd-shift-z, ctrl-z / ctrl-y) rather than public methods — gpui-component exposes no `undo()` method at all; programmatic callers dispatch the action (`window.dispatch_action(Box::new(Undo), cx)`). This keeps focus/context handling uniform and gives vim-mode-style layers one interception point. Read-only mode: ignore Undo/Redo like any other user edit.
