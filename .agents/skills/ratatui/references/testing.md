# Testing Ratatui Applications

Use this reference for any visible behavior change, layout bug, input regression, terminal-lifecycle change, or rendering optimization.

## Test Pyramid

### 1. Pure state/update tests

Test state transitions without a terminal or async runtime when possible:

- raw event to action mapping
- action plus model to new model
- focus and mode transitions
- selection/scroll invariants
- described effects and cancellation
- dirty/unchanged/quit outcomes
- stale worker result rejection

These should be the largest and fastest layer.

### 2. Widget and buffer tests

Render a component into `Buffer::empty(Rect)` and compare against `Buffer::with_lines`, expected cells, or a known buffer. This catches clipping, border, style, and coordinate mistakes without terminal lifecycle noise.

Use direct `Cell` or `Buffer` assertions for colors and modifiers. Text snapshots of a `TestBackend` generally do not encode style.

### 3. Full-frame tests

Create `Terminal<TestBackend>` at a fixed size, render the same top-level path as production, and inspect the completed buffer. Assert cursor position/visibility when the version's test backend supports it.

A typical structure, adapted to the project's pinned APIs, is:

```rust
let backend = TestBackend::new(40, 10);
let mut terminal = Terminal::new(backend)?;
let completed = terminal.draw(|frame| render_app(frame, &app))?;
assert_eq!(completed.buffer, &expected);
```

Use [`TestBackend`](https://docs.rs/ratatui/latest/ratatui/backend/struct.TestBackend.html) and [`CompletedFrame`](https://docs.rs/ratatui/latest/ratatui/struct.CompletedFrame.html) only as supported by the pinned release.

### 4. Snapshot tests

Snapshots are valuable for complete screens and regressions that are hard to read cell by cell. Follow Ratatui's [snapshot recipe](https://ratatui.rs/recipes/testing/snapshots/): deterministic dimensions and explicit review.

- Name snapshots by state and dimensions.
- Stabilize clocks, IDs, paths, random values, and platform-dependent text.
- Keep a small number of representative high-value screens.
- Review diffs as UI changes, not generated files to accept reflexively.
- Supplement text snapshots with style/cursor assertions.

### 5. PTY or emulator tests

Use a pseudoterminal or terminal parser only when testing behavior below `TestBackend`, such as:

- raw/alternate-screen escape sequencing
- scrolling regions and inline scrollback
- synchronized updates
- title/mouse/focus/bracketed-paste modes
- suspend/resume and child-process handoff
- backend writes, cursor queries, or cleanup after failure

These tests are slower and more platform-sensitive. Keep most product behavior above this layer.

## High-Value Test Matrix

Select cases relevant to the change:

| Dimension | Cases |
|---|---|
| Geometry | tiny, 80x24, wide/short, narrow/tall, nonzero-origin fixed viewport |
| Data | empty, one item, many items, long unbroken value, error/loading/completed |
| Text | ASCII, combining mark, CJK, emoji/ZWJ, mixed styled spans |
| Navigation | first/last item, wrap or clamp, page movement, focus traversal |
| Input | press, repeat, release, paste, CRLF/newline normalization, large paste |
| View | overlay open/closed, selection offscreen, resize, scrolling, cursor clipping |
| Async | out-of-order completion, cancellation, burst coalescing, channel closure |
| Lifecycle | normal exit, error, panic hook, partial init, suspend/resume if supported |

## Assertions That Matter

- The complete visible state is rendered after one draw.
- No stale cells remain behind overlays or shortened content.
- Persistent widget state remains valid after data shrinks.
- Cursor is visible only for the focused editor and lands on the expected display column.
- No buffer access panics on empty/tiny rectangles.
- Resize uses the new frame area and updates hit-test geometry.
- Unchanged events do not trigger expensive work or unnecessary frames if redraw suppression is a requirement.
- A burst of worker messages does not starve quit/input.
- Cleanup attempts all owned terminal-mode inverses.

## Verification Commands

Use repository-specific commands and scope first. A common Rust sequence is:

```text
cargo fmt --check
cargo test -p <affected-package>
cargo clippy -p <affected-package> --all-targets --all-features -- -D warnings
```

Do not enable `--all-features` blindly when it activates incompatible backends, unstable Ratatui APIs, platform-only code, or a larger policy-specific matrix. Follow workspace CI and contribution instructions.

If snapshots change, run the repository's established snapshot-review command and inspect each diff. Do not update unrelated snapshots.

## Debugging a Rendering Failure

1. Reduce to one deterministic model and terminal size.
2. Inspect the final `Buffer` and cursor state.
3. Identify the smallest component that introduces the wrong cells.
4. Check area origin, clipping, constraint math, render order, and persistent state.
5. For Unicode/cursor errors, log byte index, grapheme boundary, and display width separately.
6. Move to a PTY/emulator only if the expected buffer is correct but the physical terminal is wrong.
