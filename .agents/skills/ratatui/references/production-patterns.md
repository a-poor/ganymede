# Production Patterns: Codex and Grok Build

Use this reference only when ordinary Ratatui patterns are insufficient. It summarizes production code reviewed on 2026-08-19 at pinned commits so the observations remain reproducible.

## OpenAI Codex CLI

Reviewed commit [`18937b226524164546e7328a2ed47c0d52536e0a`](https://github.com/openai/codex/tree/18937b226524164546e7328a2ed47c0d52536e0a). Its Rust TUI currently uses Ratatui 0.30.2 with defaults disabled and an aligned Crossterm 0.29 dependency.

### Durable ideas

- A typed TUI event stream combines terminal events with synthetic draw/resume events.
- A cloneable frame requester lets background activity request rendering without owning the terminal.
- Redraw requests are coalesced and rate-limited, so many state changes can produce one frame.
- The main loop keeps terminal/app ownership and multiplexes internal app events, active-thread events, terminal events, and server events.
- Input is handled before rendering; overlay input gets priority.
- Paste normalization occurs at the event boundary.
- Visible behavior is snapshot-tested. A VT100-based test backend is used where ANSI output, cursor, or scrollback behavior matters beyond Ratatui's normal test backend.
- Unicode-aware wrapping and semantic style helpers are repository conventions; hardcoded white is avoided in favor of terminal defaults.
- Streaming keeps canonical source separate from width-dependent rendered lines. Stable committed content and a mutable tail reduce churn, while a final authoritative completion can repair a missing last delta.
- Render caches include semantic revision, width, mode, and theme so resize or styling changes cannot reuse stale wrapping.

Relevant source:

- [terminal lifecycle and event types](https://github.com/openai/codex/blob/18937b226524164546e7328a2ed47c0d52536e0a/codex-rs/tui/src/tui.rs)
- [shared terminal event stream](https://github.com/openai/codex/blob/18937b226524164546e7328a2ed47c0d52536e0a/codex-rs/tui/src/tui/event_stream.rs)
- [coalescing frame requester](https://github.com/openai/codex/blob/18937b226524164546e7328a2ed47c0d52536e0a/codex-rs/tui/src/tui/frame_requester.rs)
- [top-level multiplexing loop](https://github.com/openai/codex/blob/18937b226524164546e7328a2ed47c0d52536e0a/codex-rs/tui/src/app/startup.rs)
- [VT100 test backend](https://github.com/openai/codex/blob/18937b226524164546e7328a2ed47c0d52536e0a/codex-rs/tui/src/test_backend.rs)
- [source-backed streaming controller](https://github.com/openai/codex/blob/18937b226524164546e7328a2ed47c0d52536e0a/codex-rs/tui/src/streaming/controller.rs)

### Copy only when justified

Codex has unusual needs: a shared process-global input broker, an inline terminal integrated with scrollback, multiple event producers, overlays, and high-rate agent output. A small application rarely needs a shared event broker, 120-FPS scheduler, custom terminal wrapper, or VT100 emulation. Start with typed events and a dirty flag; adopt a scheduler or emulator only when a test/performance requirement demands it.

## xAI Grok Build CLI

Reviewed commit [`d92c5b0b8582fda358de1f97446aa74af44a464f`](https://github.com/xai-org/grok-build/tree/d92c5b0b8582fda358de1f97446aa74af44a464f). Its pager stack currently pins Ratatui 0.29.0 and Crossterm 0.28.1, demonstrating why latest Ratatui examples must not be copied blindly.

### Durable ideas

- User intent is represented as `Action`.
- Synchronous dispatch mutates state, enforces invariants, and describes `Effect`s without terminal, network, or filesystem access.
- Async tasks perform effects and return typed task results to dispatch.
- Input handling reports whether state changed; unchanged input can skip redraw and preserve cursor behavior.
- The event loop is a thin coordinator over input, protocol messages, task results, timers, and configuration changes.
- Bursty terminal input and streaming output are processed in bounded batches, with redraw coalescing and input fairness.
- Resize is debounced briefly, and ticks are scheduled only when a feature actually needs one.
- Pure dispatch, direct buffer rendering, Unicode/input, and complete screens receive separate tests.
- Background notifications carry session identity; state for a hidden session can update without forcing a visible redraw.
- Key definitions and modal ownership are centralized so dispatch, help text, and escape behavior agree.

Relevant source:

- [application module and screen modes](https://github.com/xai-org/grok-build/blob/d92c5b0b8582fda358de1f97446aa74af44a464f/crates/codegen/xai-grok-pager/src/app/mod.rs)
- [actions and effects](https://github.com/xai-org/grok-build/blob/d92c5b0b8582fda358de1f97446aa74af44a464f/crates/codegen/xai-grok-pager/src/app/actions.rs)
- [synchronous dispatch contract](https://github.com/xai-org/grok-build/blob/d92c5b0b8582fda358de1f97446aa74af44a464f/crates/codegen/xai-grok-pager/src/app/dispatch/mod.rs)
- [state owner and input outcomes](https://github.com/xai-org/grok-build/blob/d92c5b0b8582fda358de1f97446aa74af44a464f/crates/codegen/xai-grok-pager/src/app/app_view.rs)
- [event loop, batching, and redraw policy](https://github.com/xai-org/grok-build/blob/d92c5b0b8582fda358de1f97446aa74af44a464f/crates/codegen/xai-grok-pager/src/app/event_loop.rs)
- [specialized rendering path](https://github.com/xai-org/grok-build/blob/d92c5b0b8582fda358de1f97446aa74af44a464f/crates/codegen/xai-grok-pager-render/src/render/draw.rs)
- [session-aware notification routing](https://github.com/xai-org/grok-build/blob/d92c5b0b8582fda358de1f97446aa74af44a464f/crates/codegen/xai-grok-pager/src/app/acp_handler/routing.rs)
- [centralized key ownership](https://github.com/xai-org/grok-build/blob/d92c5b0b8582fda358de1f97446aa74af44a464f/crates/codegen/xai-grok-pager/src/app/agent_view/key_owner.rs)

### Copy only when justified

Grok Build includes a custom inline terminal, a background terminal writer, synchronized updates, resize debounce, and detailed subprocess handoff to address streaming-agent and PTY constraints. Those are advanced solutions with additional shutdown and synchronization risks. Its dedicated blocking input thread is also tied to the cancellation behavior of its pinned Crossterm stack; a current project may be better served by `EventStream`.

## Ratatui Templates

Ratatui maintains [official templates](https://ratatui.rs/templates/) for hello-world, simple, async, event-driven, and component styles. Use the least complex template that matches the app, and compare its dependency branch/version with the project before borrowing code.

## Escalation Order

When an app is slow or visually unstable, escalate in this order:

1. Remove blocking work from draw/update.
2. Render only when visible state is dirty.
3. Coalesce redundant redraw/progress notifications.
4. Bound burst processing and preserve input fairness.
5. Cache measured hot derived data such as wrapping or layout.
6. Add a frame scheduler for genuine high-rate updates.
7. Add a terminal-emulator test backend for behavior normal buffer tests cannot cover.
8. Consider a custom writer/backend/terminal path only after profiling proves the standard path insufficient.

Keep the action/effect and testability lessons even if none of the advanced infrastructure is needed.
