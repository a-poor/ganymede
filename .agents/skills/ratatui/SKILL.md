---
name: ratatui
description: "Build, extend, debug, and review Rust terminal user interfaces with Ratatui. Use for Ratatui app architecture, terminal and event loops, widgets, layout, input and focus, async work, rendering performance, terminal cleanup, and TUI testing. Inspect the project's pinned Ratatui and backend versions before applying APIs or examples."
---

# Ratatui Development

Build a reliable terminal application without forcing a framework onto the repository. Ratatui is an immediate-mode rendering library: the application owns state, input, updates, effects, redraw policy, and shutdown.

## Start With Evidence

Before proposing or editing code:

1. Read repository instructions and the relevant `Cargo.toml` files.
2. Inspect `Cargo.lock` and existing imports to determine the exact Ratatui, backend, async-runtime, and snapshot-test versions. Note `default-features = false` and every enabled feature.
3. Check for multiple incompatible backend versions, especially Crossterm. Use `cargo tree -p crossterm` when available.
4. Trace the current state owner, event source, update path, render entry point, terminal setup/restore path, screen mode, and tests.
5. Determine whether the request is a local change, a bug fix, an architecture change, or a new app. Preserve established structure and styling unless the task calls for redesign.

Do not copy APIs from current online examples into an older project. Ratatui 0.30 made substantial API and workspace changes, docs.rs may show feature-gated APIs, and direct backend dependencies must align with the selected Ratatui release. Prefer local source for the pinned crate, then version-matched docs and release notes.

## Choose the Smallest Sufficient Architecture

- For a small synchronous app, use one state object, one blocking or polled input loop, one update function, and one complete draw per render pass.
- For a moderate interactive app, translate backend events into typed application events or actions. Keep mutation in an update/dispatch layer and rendering as a projection of state.
- For background I/O, streaming, subprocesses, or several components, use a unidirectional pipeline: `input/event -> action -> update -> effect -> completion event`. Keep terminal and mutable app ownership in the main loop; workers communicate through typed messages.
- Add component boundaries around independently stateful or reusable regions, not around every visual fragment.

Read [architecture.md](references/architecture.md) for decision criteria and implementation patterns. Do not introduce reducers, components, async, or a custom terminal layer merely for structural symmetry.

## Preserve Terminal Correctness

- On compatible Ratatui versions, prefer the provided `run` or `init`/`restore` helpers for the modes they actually manage. Keep app-owned modes in a small RAII guard when possible; do not replace the whole lifecycle solely to add bracketed paste or mouse capture.
- When setup is manual, make every enabled mode a balanced pair: raw mode, alternate screen, mouse capture, focus events, bracketed paste, keyboard enhancement flags, cursor visibility, and title changes.
- Restore on success, error, panic, cancellation, partial initialization, and suspend/external-program handoff. For a custom lifecycle, cleanup should attempt every inverse operation even if one fails. Audit older managed helpers before assuming they aggregate cleanup failures.
- Install custom panic/reporting hooks before Ratatui initialization so Ratatui can wrap them where supported.
- Use one consistent writer for setup, drawing, and teardown. Avoid ordinary stdout/stderr logging while the terminal UI owns that stream.
- Treat backend write failure as a potentially desynchronized session: restore and exit or deliberately reinitialize instead of blindly retrying.

Read [terminal-and-events.md](references/terminal-and-events.md) whenever changing initialization, input, async work, viewports, redraw scheduling, suspend/resume, or subprocess handoff.

## Keep Rendering Predictable

- Compose the entire visible UI in one draw call. Reconstructing widgets every frame is normal; Ratatui diffs terminal-cell buffers before writing.
- Keep draw closures fast, deterministic, and free of filesystem, network, subprocess, sleep, or lock-waiting work.
- Derive geometry from `Frame::area()` or the `Rect` passed to a widget. Never assume origin `(0, 0)` or a minimum terminal size.
- Store persistent selection, scrolling, editing, and focus state outside ephemeral widget values.
- Render overlays in z-order and clear their rectangle before drawing when underlying cells might remain visible.
- Set the terminal cursor only for the active editable control and compute its column using display width, not byte length or character count.
- Route input by priority: active modal/overlay, focused component, then global shortcuts. Treat paste as a semantic event rather than a burst of key presses.
- Render usable compact states. Guard zero-width and zero-height rectangles and use saturating/clamped coordinate arithmetic in custom widgets.
- Prefer terminal-default foreground/background and semantic styles. Do not communicate important state only through color, Unicode icons, or font-specific glyphs.

Read [ui-composition.md](references/ui-composition.md) for layouts, widgets, focus, Unicode, cursor behavior, popups, and custom-widget safety.

## Control Work and Redraws

- Async enables concurrent input and work; it does not make Ratatui drawing asynchronous.
- Let workers return typed messages. Do not let them mutate UI state or draw the terminal directly, and do not hold mutable state borrows across `.await`.
- Separate logical ticks from frames. Render when state becomes dirty; use a frame clock only for animation or another measured need.
- Coalesce redundant redraw requests and bursty progress/stream updates. Preserve input fairness and bound or intentionally coalesce high-rate channels.
- A resize event schedules a render; the area observed during that render is authoritative. Fixed viewports may require explicit resizing, depending on the pinned version.
- Profile before adding caches, background terminal writers, custom backends, manual buffer flushing, or partial-render schemes. Ratatui already performs buffer diffing.

## Verify Behavior, Not Just Compilation

Use the repository's own commands, then cover the changed behavior at the cheapest effective layer:

1. Pure update/dispatch tests for state transitions, invariants, effect descriptions, and error paths.
2. Direct widget/buffer tests for local rendering, exact styles, clipping, and edge geometry.
3. `Terminal<TestBackend>` tests at deterministic dimensions for complete frames, cursor state, selection, scrolling, and resize.
4. Snapshots for meaningful screens at compact and normal sizes. Review new snapshots; do not accept them mechanically.
5. PTY or terminal-emulator tests only for escape sequences, real terminal lifecycle, scrollback, suspend/resume, or backend behavior that `TestBackend` cannot represent.

Include narrow, short, empty-data, long-text, Unicode, paste, repeat/release key, and resize cases when relevant. Text-only snapshots do not prove color/style correctness; assert cells or buffers for style regressions.

Read [testing.md](references/testing.md) before adding visible behavior or fixing a regression.

## Use Production Patterns Selectively

Read [production-patterns.md](references/production-patterns.md) only when the app needs high-rate streaming, an inline viewport with scrollback, redraw arbitration, shared terminal input, terminal handoff, or a custom rendering path. It extracts durable lessons from OpenAI Codex and xAI Grok Build while marking their application-specific machinery.

## Finish the Task

- Format and test the smallest affected workspace scope; run broader checks in proportion to the risk.
- Explain version and feature assumptions, architecture choices, terminal-lifecycle implications, and verification performed.
- Call out remaining terminal-specific uncertainty rather than claiming behavior that was not exercised.
- Do not leave terminal restoration, resize behavior, or visible UI changes untested when the project has a suitable test seam.
