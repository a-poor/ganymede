# Events & Subscriptions

**Contents:** [Overview](#overview) · [Quick Start](#quick-start) · [Common Patterns](#common-patterns) · [subscribe_in](#subscribe_in--subscription-with-window-access) · [observe_window_activation](#observe_window_activation) · [observe_global](#observe_global) · [Subscription Lifetime](#subscription-lifetime) · [Click Handlers & Propagation](#click-handlers--propagation) · [Best Practices](#best-practices)

## Overview

GPUI provides event system for component coordination:

**Event Mechanisms:**
- **Custom Events**: Define and emit type-safe events
- **Observations**: React to entity state changes
- **Subscriptions**: Listen to events from other entities
- **Global Events**: App-wide event handling

## Quick Start

### Define and Emit Events

```rust
#[derive(Clone)]
enum MyEvent {
    DataUpdated(String),
    ActionTriggered,
}

// Required: cx.emit(event) only compiles if the entity declares that it
// emits this event type. Without this impl you get a compile error.
impl EventEmitter<MyEvent> for MyComponent {}

impl MyComponent {
    fn update_data(&mut self, data: String, cx: &mut Context<Self>) {
        self.data = data.clone();

        // Emit event
        cx.emit(MyEvent::DataUpdated(data));
        cx.notify();
    }
}
```

**`cx.emit()` never triggers a re-render.** Only `cx.notify()` schedules one. Emitting without notifying is a common bug: subscribers run, but the emitting view's UI doesn't update. Call both when the emitting entity's own rendered state changed.

### Subscribe to Events

```rust
impl Listener {
    fn new(source: Entity<MyComponent>, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            // Subscribe to events
            cx.subscribe(&source, |this, emitter, event: &MyEvent, cx| {
                match event {
                    MyEvent::DataUpdated(data) => {
                        this.handle_update(data.clone(), cx);
                    }
                    MyEvent::ActionTriggered => {
                        this.handle_action(cx);
                    }
                }
            }).detach();

            Self { source }
        })
    }
}
```

### Observe Entity Changes

```rust
impl Observer {
    fn new(target: Entity<Target>, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            // Observe entity for any changes
            cx.observe(&target, |this, observed, cx| {
                // Called when observed.update() calls cx.notify()
                println!("Target changed");
                cx.notify();
            }).detach();

            Self { target }
        })
    }
}
```

## Common Patterns

### 1. Parent-Child Communication

```rust
// Parent emits events
impl Parent {
    fn notify_children(&mut self, cx: &mut Context<Self>) {
        cx.emit(ParentEvent::Updated);
        cx.notify();
    }
}

// Children subscribe
impl Child {
    fn new(parent: Entity<Parent>, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            cx.subscribe(&parent, |this, parent, event, cx| {
                this.handle_parent_event(event, cx);
            }).detach();

            Self { parent }
        })
    }
}
```

### 2. Global Event Broadcasting

`Entity<T>` / `WeakEntity<T>` require a sized `'static` type, so a `Vec<WeakEntity<dyn Listener>>` does not compile. Make the bus an `EventEmitter` and let listeners subscribe to it:

```rust
struct EventBus;

#[derive(Clone)]
enum GlobalEvent {
    ThemeChanged,
    UserLoggedOut,
}

impl EventEmitter<GlobalEvent> for EventBus {}

impl EventBus {
    fn broadcast(&mut self, event: GlobalEvent, cx: &mut Context<Self>) {
        cx.emit(event);
    }
}

// Any component holding the shared Entity<EventBus> subscribes:
// cx.subscribe(&bus, |this, _bus, event: &GlobalEvent, cx| { ... }).detach();
```

### 3. Observer Pattern

```rust
cx.observe(&entity, |this, observed, cx| {
    // React to any state change
    let state = observed.read(cx);
    this.sync_with_state(state, cx);
}).detach();
```

## subscribe_in — Subscription with Window Access

Use when the subscription callback needs `&mut Window`:

```rust
// Store subscriptions to keep them alive
struct MyComponent {
    _subscriptions: Vec<Subscription>,
}

impl MyComponent {
    fn new(input: &Entity<InputState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _subscriptions = vec![
            cx.subscribe_in(input, window, |this, state, event, window, cx| {
                match event {
                    InputEvent::PressEnter { .. } => this.on_submit(window, cx),
                    InputEvent::Change => {
                        let val = state.read(cx).value();
                        this.on_change(val, cx);
                    }
                    _ => {}
                }
            }),
        ];
        Self { _subscriptions }
    }
}
```

`subscribe` vs `subscribe_in`:
- `cx.subscribe(&entity, |this, source, event, cx|)` — no window
- `cx.subscribe_in(&entity, window, |this, source, event, window, cx|)` — window access

## observe_window_activation

```rust
let _sub = cx.observe_window_activation(window, |this, window, cx| {
    if window.is_window_active() {
        this.start_polling(cx);
    } else {
        this.stop_polling(cx);
    }
});
```

## observe_global

From `Context<Self>` the callback receives the entity and its context (app/context.rs:176). Keep the returned `Subscription` alive:

```rust
cx.observe_global::<MyGlobal>(|this, cx| {
    cx.notify(); // Re-render when the global changes
})
.detach();
```

## Subscription Lifetime

Subscriptions are cancelled when dropped. Two ways to keep alive:

```rust
// 1. .detach() — lives until entity is dropped
cx.subscribe(&entity, |this, _, event, cx| {
    // ...
}).detach();

// 2. Store in struct — cancelled when struct drops
struct MyView {
    _subscriptions: Vec<Subscription>,
}
// _subscriptions.push(cx.subscribe(...));
```

Use `.detach()` for permanent subscriptions; store in struct for subscriptions that should stop when the component unmounts.

## Click Handlers & Propagation

`ClickEvent` (interactive.rs:281) carries only input info (position, modifiers) — no custom payload. Per-row data flows by closure capture:

```rust
for item in &self.items {
    let id = item.id;
    row = row.on_click(cx.listener(move |this, _event: &ClickEvent, _window, cx| {
        this.select(id, cx);
    }));
}
```

A handler shared across N rows must be `Rc<dyn Fn(...)>` — a `Box<dyn Fn>` is moved into the first row's closure, so the second row fails to compile:

```rust
type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

let handler: ClickHandler = Rc::new(|event, window, cx| { /* ... */ });
// clone the Rc into each row's closure:
let h = handler.clone();
row = row.on_click(move |event, window, cx| h(event, window, cx));
```

Propagation:
- **Actions** stop propagating after the first handler by default — call `cx.propagate()` to let outer handlers also run.
- **Mouse events** bubble by default — call `cx.stop_propagation()` to stop them.

## Best Practices

### ✅ Detach Subscriptions

```rust
// ✅ Detach to keep alive
cx.subscribe(&entity, |this, source, event, cx| {
    // Handle event
}).detach();
```

### ✅ Clean Event Types

```rust
#[derive(Clone)]
enum AppEvent {
    DataChanged { id: usize, value: String },
    ActionPerformed(ActionType),
    Error(String),
}
```

### ❌ Avoid Event Loops

```rust
// ❌ Don't create mutual subscriptions
entity1.subscribe(entity2) → emits event
entity2.subscribe(entity1) → emits event → infinite loop!
```

