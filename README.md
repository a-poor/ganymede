# ganymede

`ganymede` is a fast native Jupyter Server client written in Rust.

A Rust workspace containing:

- `ganymede-gui`: the application, with a dependency on `ganymede-core`.
- `ganymede-core`: the shared library.

Run the application with `cargo run` (or `cargo run -p ganymede-gui`).
Check all workspace members with `cargo check --workspace`.

With [just](https://github.com/casey/just) installed, run `just` to list commands
in definition order, or `just run-gui` to launch the application.

Run `just fetch-assets` to download the GUI assets (requires Bash, curl, unzip,
and tar):

- [Inter 4.1](https://github.com/rsms/inter/releases/tag/v4.1) desktop fonts → `ganymede-gui/assets/fonts/inter/`
- [JetBrains Mono 2.304](https://github.com/JetBrains/JetBrainsMono/releases/tag/v2.304) desktop fonts → `ganymede-gui/assets/fonts/jetbrains-mono/`
- [Phosphor 2.0.8](https://github.com/phosphor-icons/core/releases/tag/v2.0.8) SVG icons, in all six weights → `ganymede-gui/assets/icons/phosphor/`

Downloaded assets and their licenses are ignored by Git. Versions are pinned in
`scripts/fetch-assets.sh`; rerunning the command refreshes the downloaded files.
Fetch assets before building the GUI on a fresh checkout. Selected fonts and
icons are embedded with `rust-embed` in debug and release builds, so the binary
does not need the asset directory at runtime. Startup registers Inter and
JetBrains Mono with GPUI; the theme uses these for UI and code text.

The `define_icons!` declaration in `ganymede-gui/src/icon.rs` is the icon
allowlist. For example, adding `CircleCheck => "regular/check-circle.svg",`
generates `Icon::CircleCheck`, its path, and its embed entry together. Render it
with `Icon::CircleCheck.svg().size(...).text_color(...)`, using theme values.
Only declared icons are embedded, alongside their license. Font weights are
selected explicitly in `ganymede-gui/src/assets.rs`.

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


## GUI notebooks on a local Jupyter server

Copy `.env.example` to `.env` in the repository root and set
`GANYMEDE_JUPYTER_URL` to the server root (for example,
`http://127.0.0.1:8888/`, without `/lab` or a token query). Set
`GANYMEDE_JUPYTER_TOKEN` separately and optionally
`GANYMEDE_JUPYTER_KERNEL` (defaults to `python3`). The local file is ignored
by Git; process environment variables override its values. The development
build reads this file from the repository root even when launched elsewhere.
Restart the app after changing configuration.

Run `cargo run -p ganymede-gui`. The demo starts with a new server notebook, using
Jupyter’s unused `Untitled` name. The file tree starts at the server root; folders
load on expansion. Its toolbar opens the launcher, creates a server-named folder
at the root, and toggles a filename filter input. Filtering is case-insensitive and keeps
ancestor folders visible for matching files, including files in collapsed folders.
Clear/close with the × button or Escape to restore the normal tree. Background
search is bounded to 1,024 directories and 32 levels; directory requests are limited
to four at a time. The overflow menu offers Upload
(native multi-file picker, targeting the root) and Refresh. Existing upload names
are rejected; uploads never intentionally replace an existing file. The tree
refreshes quietly every 10 seconds, retaining expanded folders and scroll position. Click a notebook to
open it; an already-open path reuses its tab. Opening a notebook connects automatically,
without executing code. Run or Shift+Enter executes the selected cell. Subsequent
cells share its kernel state. Run All executes code cells in order and stops
on an error; Markdown and Raw cells are skipped. Interrupt clears queued work
and interrupts the active kernel. Restart clears queued work and restarts the
kernel without rerunning code. Failed/disconnected executions are never
automatically replayed; a new explicit Run can reconnect.

This first integration displays streaming stdout/stderr, plain-text results,
and tracebacks. Rich outputs use their plain-text fallback where available.
Interactive stdin is disabled. Save with the toolbar or ⌘S (Ctrl+S elsewhere).
Saving preserves notebook/cell metadata, attachments, unknown fields, cell order,
cell types, and full output payloads, including rich outputs not yet rendered.
The unsaved dot tracks document revisions; edits or output received during a save
remain dirty. Save failures stay visible and retain changes. Closing a dirty
notebook asks before discarding it; reopening a closed notebook loads the server
copy. Closing tabs or the app does not delete server sessions or kernels.

Autosave is enabled by default and saves after about two seconds without document
changes (including execution outputs). Toggle it in Settings → Notebooks; this
preference currently applies for the app session, like appearance settings.
Saves are silent; failures show an error toast and pause autosave until a manual
save or toggling autosave off/on. ⌘S / Ctrl+S always remains available.

Closing a tab or the main window, or using ⌘Q / Quit, checks for unsaved work.
The confirmation offers Cancel, Discard changes, and Save and close/quit.
Saving must succeed, with no newer unsaved edits, before closing. Autosave pauses
while the dialog is open. Escape cancels; Tab/arrows choose an action and Enter
confirms. Text-file drafts cannot yet save, so they offer Cancel or Discard only.
OS termination outside the app's Quit action (including force quit) cannot show
this confirmation with the current GPUI platform hooks.

Text-file editing and terminal/console launcher items remain demos. Save As,
rename, and external-change conflict detection are not implemented yet.

To exercise the GUI execution adapter against the configured server:

```sh
cargo test -p ganymede-gui execution::tests::live_execution -- --ignored
```

This test creates its own unique scratch session, verifies streaming output,
shared state, errors, interrupt, and restart, and deletes that test session
afterward.
