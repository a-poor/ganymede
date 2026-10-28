# Terminal Lifecycle and Events

Use this reference for setup, teardown, backends, input, async loops, viewports, redraw cadence, suspend/resume, or launching external programs.

## Version and Backend Audit

Before editing:

- Read the exact Ratatui dependency and feature set in `Cargo.toml`.
- Confirm the resolved version in `Cargo.lock`.
- Identify the backend and its resolved version. For Crossterm, inspect `cargo tree -p crossterm`; incompatible copies can maintain independent event queues and raw-mode bookkeeping.
- Inspect the selected version's lifecycle-helper source to see exactly which modes it enables/restores and whether cleanup continues after an error. Do not infer those guarantees from a newer release.
- Check whether the app renders to stdout or stderr and whether stdout is a data interface.
- Identify full-screen, inline, or fixed viewport behavior and target platforms.

The current Ratatui backend comparison documents the [Crossterm version warning](https://ratatui.rs/concepts/backends/comparison/). Online latest docs are not a substitute for version-matched APIs.

## Prefer Managed Lifecycle When It Fits

On Ratatui 0.30.x, `ratatui::run` is the ordinary Crossterm path: it initializes, executes a closure, and restores. `init`/`restore` and their fallible variants support a longer-lived terminal. `init_with_options` supports custom viewports and does not automatically enter the alternate screen. Confirm these APIs against the selected version.

Constructing `Terminal` directly does not itself enable raw mode, enter the alternate screen, or install Ratatui's cleanup panic hook. Manual construction requires manual lifecycle ownership.

Managed helpers cover only the modes documented by that release. If the app adds bracketed paste, mouse/focus capture, keyboard flags, or similar modes, prefer a narrow RAII guard for those additions inside the managed terminal lifetime. Replace the entire managed lifecycle only when suspend/resume, custom writers/viewports, partial-init tracking, or stronger cleanup semantics genuinely require it. Some older restore helpers return after the first cleanup error rather than attempting every inverse; inspect local source and choose the smallest guard that meets the product's failure requirements.

Order nested cleanup deliberately: create the extra-mode guard after managed initialization and drop it before calling managed restore on normal/error paths. Rust drops guards only after the panic hook runs, so RAII alone cannot disable app-owned modes before panic reporting. If that ordering matters, make the pre-Ratatui application panic hook idempotently disable those extra modes before it prints/delegates; then let Ratatui wrap that hook as supported by the selected version.

Install application error/panic reporting before Ratatui initialization where the selected version wraps the existing hook. Do not print a panic report into an active alternate/raw screen before restoration.

Sources: [`run`](https://docs.rs/ratatui/latest/ratatui/fn.run.html), [initialization module](https://docs.rs/ratatui/latest/ratatui/init/index.html), and [raw-mode concepts](https://ratatui.rs/concepts/backends/raw-mode/).

## Treat Modes as a Transaction

List every mutation in setup order and its inverse. Typical pairs include:

| Setup | Teardown |
|---|---|
| enable raw mode | disable raw mode |
| enter alternate screen | leave alternate screen |
| enable bracketed paste | disable bracketed paste |
| enable mouse/focus capture | disable mouse/focus capture |
| enable keyboard flags | pop/disable those flags |
| hide cursor | show cursor |
| set title | restore title if the app owns it |

When the application owns a custom lifecycle, cleanup should be idempotent where feasible and should attempt every inverse even after an earlier cleanup failure. Preserve the primary application error and attach cleanup errors rather than losing the original cause. Do not reimplement an otherwise adequate managed lifecycle merely to aggregate unlikely teardown errors in a small app.

If initialization fails halfway, undo only the modes known to have succeeded. An RAII guard or explicit setup-state record is appropriate for complex manual initialization.

Terminal/backend write failure may leave the physical screen and Ratatui buffers out of sync. Prefer restoring and ending the session. If the product must recover, deliberately clear/reinitialize and force a full redraw.

## Event Boundary

Ratatui does not read input; the backend does. Centralize backend consumption and translate it into app-level events:

- key press/repeat/release
- mouse
- paste
- focus gained/lost
- resize
- logical tick
- redraw request
- worker completion
- shutdown/cancellation

Prefer one owner of the backend event stream. Multiple readers can race over process-global input facilities.

Filter key kinds intentionally. Most commands should handle `Press`; text navigation may also handle `Repeat`; `Release` should normally be ignored. Do not accidentally execute a command twice on platforms that emit multiple kinds.

Treat paste separately from keystrokes. Normalize newlines only when the app's text model requires it, apply length/security limits appropriate to the input, and dispatch the paste as one semantic update where possible.

See Ratatui's [event-handling patterns](https://ratatui.rs/concepts/event-handling/).

## Synchronous Loop

Use blocking `event::read` when nothing must progress without input. Use `event::poll(timeout)` when the same thread must service ticks or work. Avoid zero-timeout busy loops.

The conceptual loop is:

```text
draw initial state
while running:
    wait for an event or actual deadline
    translate event to intent
    update state
    run/schedule described effects
    if visible state changed or resize occurred:
        draw the complete frame once
restore terminal
```

## Async Loop

Use async only when the application needs concurrent I/O, timers, cancellation, or workers. Ratatui drawing stays synchronous.

With Tokio and a compatible Crossterm release, `EventStream` can be selected with worker messages and timers. Some older production stacks instead dedicate a blocking thread to Crossterm input because their exact dependency versions have cancellation or global-reader constraints. Verify the pinned backend rather than copying either approach.

Maintain these invariants:

- The main loop owns mutable model and terminal state.
- No mutable model borrow is held across `.await`.
- Background tasks never draw or change terminal modes.
- Worker results include enough identity to reject stale completions.
- Cancellation and task joining happen before final terminal teardown.
- A flood of progress messages cannot starve input or shutdown.

## Redraw Scheduling

Separate these concepts:

- **Event:** something happened.
- **Tick:** logical time advanced.
- **Dirty:** visible state differs from the last frame.
- **Frame deadline:** the earliest useful next paint.

Static apps should draw on invalidation. Animation needs a bounded frame cadence. Coalesce redraw signals because one draw reflects all current state. Preserve semantic actions and terminal input; do not drop them merely because redraws are droppable.

Under bursty streaming, process a bounded batch, yield to input/cancellation, then paint at most once. Add sophisticated fairness or rate limiting only after observing starvation or excessive writes.

## Resize and Viewports

- Treat the area observed during drawing as authoritative; resize events may be coalesced or stale.
- Schedule a redraw after resize.
- In current Ratatui, full-screen and inline viewports auto-resize during draw; fixed viewports require explicit `Terminal::resize`. Verify the pinned release.
- Inline viewports are useful when normal scrollback must remain above the app. `Terminal::insert_before` supports inserting output above an inline UI on compatible versions/features.
- Never assume viewport origin is `(0, 0)`; fixed viewports can have nonzero coordinates.

See [`Viewport`](https://docs.rs/ratatui/latest/ratatui/enum.Viewport.html) and [`Terminal`](https://docs.rs/ratatui/latest/ratatui/struct.Terminal.html).

## Suspend, Resume, and External Programs

If the app launches an editor/shell or responds to job-control suspend:

1. Stop or park the event reader so it cannot consume the child's input.
2. Stop scheduling frames and drain/stop any terminal writer.
3. Restore every terminal mode owned by the UI.
4. Run or suspend to the external program.
5. Reinitialize modes in a known order.
6. Clear or force a full redraw and refresh size.

Late terminal writes after restoration can corrupt the shell, so writer/task shutdown ordering matters. Sanitize untrusted text used in terminal control sequences such as window titles; never embed raw control characters.

## Diagnostics

- Route logs to a file, tracing collector, or a pane rendered by the app; do not interleave ordinary prints with terminal drawing.
- Record event kind, action, state transition, effect, and render request at useful trace levels without logging sensitive pasted text.
- For terminal-only failures, reproduce under the target terminal and OS; emulator behavior, Unicode width, and keyboard protocols differ.
