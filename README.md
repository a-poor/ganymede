# Ganymede

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
