# Architecture and State Flow

Use this reference when designing a new app, untangling a large event loop, adding background work, or deciding whether to introduce components.

## Immediate-Mode Model

Ratatui is not a retained widget tree. Each render pass starts from an empty current buffer, composes the whole frame, diffs it with the previous buffer, and emits changed cells. Therefore:

- Application state is the source of truth.
- Widget values are usually short-lived view descriptions.
- One draw closure composes one complete frame.
- Manual partial rendering is usually incorrect or unnecessary.
- Render order is z-order.

See Ratatui's [rendering model](https://ratatui.rs/concepts/rendering/) and [under-the-hood buffer flow](https://ratatui.rs/concepts/rendering/under-the-hood/).

## Architecture Ladder

Choose the lowest rung that satisfies the app.

| Shape | Use when | Suggested flow |
|---|---|---|
| Small loop | A few fields, synchronous input, no background work | backend event -> `App::handle` -> `draw` |
| Typed update | Several modes or controls, testable behavior matters | event -> action/message -> `update(&mut Model, action)` -> draw |
| Action/effect | Network, process, file, streaming, or cancellable work | event -> action -> pure/sync update + effect -> task result -> action |
| Components | Several independently stateful regions or reusable panels | top-level router -> component action/update/render |

Official alternatives include the [Elm architecture](https://ratatui.rs/concepts/application-patterns/the-elm-architecture/) and [component architecture](https://ratatui.rs/concepts/application-patterns/component-architecture/). Treat them as patterns, not requirements.

## State Ownership

Prefer a single clear owner for mutable application and terminal state. A robust model separates:

- **Domain state:** records, jobs, connection state, results.
- **View state:** focus, selection, scroll offsets, open modal, editor cursor.
- **Ephemeral events:** input, paste, resize, timer, worker completion.
- **Effects:** descriptions of work to start, cancel, persist, or send.
- **Derived view data:** filtered rows, wrapped lines, status labels.

Compute cheap derived data while rendering. Cache expensive derivations in state and invalidate them explicitly. Do not hide authoritative state in widget constructors or global variables.

Stateful widget types such as `ListState`, `TableState`, and `ScrollbarState` belong in model/component state because selection and offsets must survive widget reconstruction.

## Typed Update Pattern

Keep terminal-library details at the boundary:

```rust
enum AppEvent {
    Key(KeyEvent),
    Paste(String),
    Resize,
    Tick,
    WorkFinished(Result<Output, AppError>),
}

enum Action {
    Quit,
    MoveSelection(i32),
    Submit,
    WorkFinished(Result<Output, AppError>),
}

enum Effect {
    StartWork(Request),
    CancelWork(JobId),
}

struct Update {
    dirty: bool,
    effects: Vec<Effect>,
}
```

Use a boundary function to turn `AppEvent` into zero or more `Action`s. Let update/dispatch synchronously mutate the model and return `Update`. Execute effects outside update and feed completions back through the same action path.

This yields deterministic tests: given model plus action, assert the new model, dirty flag, and effect descriptions without a runtime or terminal.

Do not require every app to define all three enums. A small app can combine event and action; a complex app benefits from separating raw input, user intent, and asynchronous work.

## Components

Create a component when at least one is true:

- It owns persistent interaction state.
- It has a coherent event/update/render lifecycle.
- It is reused in different screens.
- Isolating it materially improves tests or ownership.

A component need not spawn a task or own a channel. Keep shared navigation, overlays, and global shortcuts in a top-level router. Give active overlays first refusal on input, then the focused component, then global bindings.

Avoid component trees that obscure data flow. Passing a `Rect` and borrowed view model to a render helper is enough for static regions.

## Effects and Concurrency

- Run blocking work through the runtime's blocking facility or a worker thread, never in the draw closure.
- Workers send immutable results/messages; the main loop applies them.
- Give jobs identities so stale completions can be ignored after cancellation or replacement.
- Define shutdown order: stop accepting work, cancel tasks, await/join readers and workers, drain only what is safe, then restore the terminal.
- Choose channel bounds deliberately. A capacity-one redraw signal can collapse redundant frames; semantic actions usually must not be dropped.

## Redraw Policy

Track whether an update changes visible state. A useful result distinguishes:

- `Changed`: request a render.
- `Unchanged`: preserve the current terminal frame and cursor behavior.
- `EffectOnly`: start work without drawing unless status changed.
- `Quit`: leave the loop and restore.

For bursts, mark the app dirty and let one pending redraw cover many changes. For animation, schedule ticks independently of redraws and cap the frame rate. Keep input and cancellation responsive under heavy worker traffic.

## Refactoring Safely

When changing an existing app:

1. Characterize behavior with update and frame tests.
2. Introduce typed boundaries without changing key bindings or layout.
3. Move side effects out of update/render one kind at a time.
4. Keep terminal initialization and cleanup stable until event flow is covered.
5. Re-profile before adopting production-grade redraw scheduling or custom rendering.
