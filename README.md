# ganymede

`ganymede` is a fast native Jupyter Server client written in Rust.

A Rust workspace containing:

- `ganymede-gui`: the application, with a dependency on `ganymede-core`.
- `ganymede-core`: the shared library.

Run the application with `cargo run` (or `cargo run -p ganymede-gui`).
Check all workspace members with `cargo check --workspace`.

## Jupyter clients

`ganymede_core::jupyter` exposes `JupyterRestClient`, `KernelClient`, and
`ServerConfig`. Supply the server root (including prefixes such as `/user/alice/`)
and an optional token separately. Both clients use token authorization headers.
Password/cookie login and JupyterHub OAuth are not implemented.

The REST client supports notebook reads/saves, sessions, kernelspec discovery,
and kernel start/interrupt/restart/shutdown. Notebook documents remain JSON to
preserve metadata. Deleting a REST session also shuts down its kernel.

The kernel client uses Jupyter's default WebSocket protocol, including binary
buffers. `execute()` returns a request ID once sent; receive output through
`KernelEvents` and correlate it using `KernelMessage::parent_id()`.
`ExecutionTracker` observes both the execution reply and matching idle status.
Late output is still delivered. Kernel exceptions are messages, not transport
errors. Stdin is disabled by default; enabling it requires answering input events.

The event queue is bounded. Overflow closes the connection and delivers a
terminal `EventOverflow` after queued messages are drained. Connections do not
automatically reconnect or replay requests. Dropping a submitted execution future
does not interrupt the kernel; use the REST interrupt operation explicitly.
Closing the WebSocket or dropping all handles does not shut down the kernel.

Run the example against your own server (it creates and cleans up a kernel):

```sh
export GANYMEDE_JUPYTER_URL=http://127.0.0.1:8888/
export GANYMEDE_JUPYTER_TOKEN=your-token
cargo run -p ganymede-core --example execute -- "print('hello'); 6 * 7"
```

Run unit and local mock-server tests with `cargo test --workspace`.
For a real Jupyter integration test, install `jupyter-server` and `ipykernel`
in a Python environment, then run:

```sh
python scripts/test_jupyter.py
```

This starts a disposable, token-protected server with a URL prefix and temporary
notebook storage, exercises execution, stdin, errors, interrupt, restart, and
cleanup, then stops the server and its kernels. The runner requires a Unix host.

## Application notebook API

`ganymede_core::notebook` builds on those clients and owns notebook state for an
application. It has no GUI dependency.

- `NotebookDocument` holds code, markdown, and raw cells with stable `CellId`s,
  sources, outputs, execution counts, metadata, and revision-based dirty state.
- `NotebookController` edits that document, saves it, associates it with a server
  session, executes cells, and applies kernel messages to the appropriate cells.
- `NotebookSnapshot` exposes the document, connection/session, kernel status,
  per-cell execution phases, pending stdin prompts, and save status.
- `NotebookEvent` reports document/cell changes, execution and connection changes,
  input requests, and protocol errors. Extension messages remain available through
  `UnhandledMessage`; widget rendering is not implemented.

Typical use (inside a Tokio runtime):

```rust,no_run
use ganymede_core::{
    jupyter::ServerConfig,
    notebook::{CellKind, NotebookController, NotebookDocument},
};

async fn example() -> Result<(), Box<dyn std::error::Error>> {
let config = ServerConfig::new("http://127.0.0.1:8888/")?.with_token("token")?;
let mut document = NotebookDocument::new();
let cell = document.insert_cell(0, CellKind::Code, "print('hello')")?;
let notebook = NotebookController::new(config, "example.ipynb", document)?;

let snapshots = notebook.watch();
let events = notebook.subscribe();
notebook.save().await?;
notebook.connect("python3").await?;
notebook.run_cell(&cell, false).await?; // returns after submission
// Render snapshots as they change; answer pending stdin when enabled.
// Call save() after execution completes to persist generated outputs.
Ok(())
}
```

Use `NotebookController::open(config, path)` to load an existing notebook without
connecting to its kernel. Offline editing supports inserting, moving, removing,
and changing cell source/metadata. Removing an executing cell requires finishing
or interrupting it first, so a pending stdin prompt cannot be orphaned.

`watch()` coalesces changes into the latest snapshot. `subscribe()` provides
ordered events through a bounded broadcast receiver; if it reports `Lagged`,
resynchronize from `snapshot()`. Pending input is also retained in snapshots.
Snapshots are immutable owned copies; the initial implementation clones the
document on publication, which may warrant optimization for large notebooks.

Outputs are correlated by request ID, including late output after completion.
Completion requires both the execution reply and matching IOPub idle message.
Rerunning a cell replaces its previous execution/output association; the submitted
source remains available separately from any edits made during execution.
Display IDs can update outputs in multiple cells, and `clear_output(wait=true)`
defers clearing until new output arrives. Display IDs are runtime state and are
not written into notebook files.

Saves serialize a captured revision. Edits/output arriving during a save remain
dirty, and failed saves retain the document and expose `SaveState::Failed`.
Concurrent saves are serialized. Once polled, async controller operations run in
owned tasks: cancelling their awaiting caller does not cancel the operation or
replay it. Use `interrupt()` to interrupt execution explicitly.

Connection lifecycle is explicit:

- `connect()` creates/reuses a notebook session or reconnects to the known one.
  It waits for a kernel-info handshake before accepting execution.
- `attach_session(id)` selects an existing session for the same notebook path.
- `disconnect()` closes the transport and retains the session and document.
- `detach_session()` additionally forgets the association, leaving any server
  session/kernel alive. Use it to recover from an externally deleted session.
- `restart_kernel()` restarts and reconnects without rerunning cells.
- `shutdown_session()` explicitly deletes the server session and its kernel.

Transport loss marks unfinished executions `Unknown`, preserving edits and
outputs. No automatic reconnect or request replay occurs. Connection state
describes the transport, not continuous kernel health: a server can leave a
WebSocket open after deleting its kernel. Dropping the controller disconnects
after any in-flight operations finish and never shuts down a server kernel.

Notebook JSON round trips retain unknown top-level, cell, and output fields,
attachments, metadata, and string/line-array representations. Importing older
notebooks without cell IDs assigns IDs, upgrades the notebook format to at least
4.5, and marks the document dirty. Unsupported major formats/cell types and
duplicate IDs are rejected. Jupyter may add its own trust metadata on reload;
the controller does not merge server-normalized fields into an edited document.

Run the complete application example with the same environment variables above:

```sh
cargo run -p ganymede-core --example notebook
```

It creates a uniquely named notebook, runs a cell, saves outputs, and shuts down
its session. Passing a path as the first argument selects the save destination.
The disposable integration runner tests both API layers and both examples.
