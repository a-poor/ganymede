# Async & Background Tasks

**Contents:** [Overview](#overview) · [Quick Start](#quick-start) · [Core Patterns](#core-patterns) · [Common Pitfalls](#common-pitfalls)

## Overview

GPUI provides integrated async runtime for foreground UI updates and background computation.

**Key Concepts:**

- **Foreground tasks**: UI thread, can update entities (`cx.spawn`)
- **Background tasks**: Worker threads, CPU-intensive work (`cx.background_spawn`)
- All entity updates happen on foreground thread

## Quick Start

### Foreground Tasks (UI Updates)

When spawned from `Context<Self>`, the closure receives `(WeakEntity<Self>, &mut AsyncApp)`:

```rust
impl MyComponent {
    fn fetch_data(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            // Runs on UI thread, can await and update entities
            let data = fetch_from_api().await;

            this.update(cx, |state, cx| {
                state.data = Some(data);
                cx.notify();
            }).ok();
        }).detach();
    }
}
```

When spawned from `&mut App` (not inside an entity), the closure receives only `(cx: &mut AsyncApp)`:

```rust
cx.spawn(async move |cx: &mut AsyncApp| {
    // No entity reference
}).detach();
```

### Spawn with Window Context (spawn_in)

Use `spawn_in` when the task also needs window access (`update_in`):

```rust
impl MyComponent {
    fn animate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            // cx here is AsyncWindowContext
            this.update_in(cx, |state, window, cx| {
                // Can access window here
                state.frame += 1;
                cx.notify();
            }).ok();
        }).detach();
    }
}
```

### Background Tasks (Heavy Work)

`Task` has no combinators — to get a background result onto the UI thread, **await the background task
inside a foreground `cx.spawn`**:

```rust
impl MyComponent {
    fn process_file(&mut self, cx: &mut Context<Self>) {
        let background = cx.background_spawn(async move {
            // Runs on a background thread, CPU-intensive
            heavy_computation()
        });

        cx.spawn(async move |this, cx| {
            let result = background.await;    // Task<R> is itself a future
            this.update(cx, |state, cx| {
                state.result = result;
                cx.notify();
            }).ok();
        }).detach();
    }
}
```

### Task Management

```rust
struct MyView {
    _task: Task<()>,  // Prefix with _ if stored but not accessed
}

impl MyView {
    fn new(cx: &mut Context<Self>) -> Self {
        let _task = cx.spawn(async move |this, cx: &mut AsyncApp| {
            // Task automatically cancelled when dropped
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;

                this.update(cx, |state, cx| {
                    state.tick();
                    cx.notify();
                }).ok();
            }
        });

        Self { _task }
    }
}
```

## Core Patterns

### 1. Async Data Fetching (from Context<Self>)

```rust
cx.spawn(async move |this, cx: &mut AsyncApp| {
    let data = fetch_data().await?;
    this.update(cx, |state, cx| {
        state.data = Some(data);
        cx.notify();
    })?;
    Ok::<_, anyhow::Error>(())
}).detach();
```

### 2. Background Computation + UI Update

```rust
let background = cx.background_spawn(async move { heavy_work() });
cx.spawn(async move |this, cx| {
    let result = background.await;
    this.update(cx, |state, cx| {
        state.result = result;
        cx.notify();
    }).ok();
}).detach();
```

### 3. Periodic Tasks

```rust
cx.spawn(async move |this, cx: &mut AsyncApp| {
    loop {
        cx.background_executor().timer(Duration::from_secs(5)).await;

        this.update(cx, |state, cx| {
            state.tick();
            cx.notify();
        }).ok();
    }
}).detach();
```

### 4. Task Cancellation

Tasks are automatically cancelled when dropped. Store in struct to keep alive.

## Common Pitfalls

### ❌ Don't: Use `defer_in` and then update the same entity through its handle

`cx.defer_in(window, callback)` re-acquires the current entity's lock to run the callback — calling
`entity.update(cx, …)`/`entity.read(cx)` on that **same** entity inside it panics
(`cannot update … while it is already being updated`). Use the `&mut` reference the callback provides
instead. Full treatment with examples: [entity-best-practices.md](entity-best-practices.md).

### ❌ Don't: Update entities from background tasks

```rust
// ❌ Wrong: Can't update entities from background thread
cx.background_spawn(async move {
    entity.update(cx, |state, cx| { // Compile error!
        state.data = data;
    });
});
```

### ✅ Do: Await the background task from a foreground task

```rust
// ✅ Correct: the Task is a future — await it where entity updates are legal
let background = cx.background_spawn(async move { compute_data() });
cx.spawn(async move |this, cx| {
    let data = background.await;
    this.update(cx, |state, cx| {
        state.data = data;
        cx.notify();
    }).ok();
}).detach();
```

Background futures must be `Send + 'static` and cannot capture `cx` or entities. Note the asymmetry when
propagating errors: `WeakEntity::update` returns `Result` (the entity may be gone), so `.ok()`/`?`
accordingly. For `Task<Result<..>>` there is `TaskExt::detach_and_log_err(cx)`.

## Task replacement = debounce / cancellation

Dropping a `Task` cancels it, so **assigning a new task into the same field cancels the old one**:

```rust
struct SearchPage { pending: Option<Task<()>> }

fn on_edit(&mut self, cx: &mut Context<Self>) {
    self.pending = Some(cx.spawn(async move |this, cx| {
        cx.background_executor().timer(Duration::from_millis(250)).await; // debounce window
        // run the query, update state...
    }));
    // the previous pending task (still in its timer) was just dropped → cancelled
}
```

Use `Duration::ZERO` to bypass the debounce (e.g. on Enter). The same trick discards stale in-flight
loads: replacing the task guarantees at most one live request. Detached tasks can't be cancelled — prefer
stored tasks when the work can go stale, and race-guard detached updates by checking expected state inside
`this.update` before applying a late result.

## Tokio interop

GPUI's `BackgroundExecutor` polls any `Send` future, but it is **not a tokio reactor**: tokio I/O and
timers (async `reqwest`, `tokio::time`) panic at runtime with "no reactor running". Two facts make interop
easy:

- **`tokio::sync` primitives (`oneshot`, `mpsc`, `broadcast`, `watch`) and `JoinHandle` are
  executor-agnostic** — they can be awaited directly inside `cx.spawn` with no glue. Channels are the
  entire bridge.
- **Never use `#[tokio::main]`**: GPUI must own the real main thread (a macOS requirement) and
  `Application::run` blocks it. Build an explicit `tokio::runtime::Runtime` before launching GPUI and pass
  its `Handle` around (e.g. via a `Global`).

Standard request/response shape:

```rust
let (tx, rx) = tokio::sync::oneshot::channel();
tokio_handle.spawn(async move {           // tokio side: reactor available
    let _ = tx.send(do_database_query().await);
});
cx.spawn(async move |this, cx| {          // GPUI side: just awaiting a channel
    if let Ok(result) = rx.await {
        this.update(cx, |state, cx| { state.apply(result); cx.notify(); }).ok();
    }
}).detach();
```

The Zed repo's `gpui_tokio` crate packages exactly this (`Tokio::spawn(cx, fut)` returns a GPUI `Task`
whose drop **aborts** the tokio task — the cancellation propagation you'd get wrong hand-rolling).
Long-lived streams work the same way: a loop in `cx.spawn` awaiting a broadcast/mpsc channel fed by a
tokio task. Per-message `cx.notify()` is fine — notifies dedupe per entity per frame and GPUI coalesces
redraws, so no batching layer is needed for streaming updates.

