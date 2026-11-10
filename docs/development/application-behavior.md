# Application behavior reference

[Back to the README](../../README.md)

This reference describes implementation behavior and limits for contributors. For
everyday use, see the [user guide](../user-guide.md). Development commands run
from the repository root.

## Connections and notebooks

Run `cargo run -p ganymede-gui` to open the connection picker. Start a local
server by choosing a project folder and an environment: Global Python (`python3`),
a virtual environment folder, or a custom Python executable.
Python environments need `jupyter-server` and `ipykernel`. Relative environment paths
resolve from the project folder. You can also connect to
an existing HTTP(S) server with its URL and optional token. Deployment prefixes
and pasted `/lab?token=…` URLs are supported.

For an existing server reached over SSH, enter an **SSH host** (an SSH config alias
or `user@host`) and the Jupyter URL as seen from that host, for example
`http://127.0.0.1:8888/`. Ganymede uses OpenSSH with your existing config, keys,
and agent. Custom SSH ports and jump hosts belong in `~/.ssh/config`. Password
prompts are not supported; unlock encrypted keys with `ssh-add`, and verify new
host keys in Terminal first. Unknown or changed host keys are rejected.

The app owns a loopback-only SOCKS tunnel for each connected workspace. REST and
kernel WebSocket traffic use remote DNS through that tunnel; HTTPS retains its
original hostname and certificate checks. There is no direct fallback. Recent
connections, Keychain entries, and workspace layouts are scoped to both the SSH
host and server URL. The server card shows the SSH host; **Reconnect** reopens a
lost tunnel without replaying cells. Closing the workspace or quitting closes the
tunnel and leaves the remote server and kernels running. Click the server address
in the server card to copy it. Opening a browser is unavailable for SSH connections,
since external browsers do not share the app's proxy. This connects to existing
servers only.

Click the notebook's kernel status to choose an installed kernel. Switching asks
for confirmation before replacing the kernel and clearing its variables. The
Kernels sidebar lists running kernels and associated notebook paths, refreshes
periodically, and provides an explicit shutdown action with confirmation.
Closing a notebook leaves its kernel running. After shutdown, use Reconnect or
run a cell to start a new kernel using the notebook's selected kernelspec.

Each workspace connects to one server. **File → New Window** (`Cmd+Shift+N` on
macOS) opens another picker. Recent connections are remembered without tokens;
on macOS, **Remember token** stores a successfully validated token in Keychain.
Recent connections then reconnect with one click after restarting the app. Keys
are scoped to the full normalized server URL, including deployment prefixes.
Use **Edit connection…** in a recent connection’s overflow menu to replace its token or choose
**Forget token**. Forgetting removes the Keychain item and cached Recent credential;
already-open workspaces remain connected. Without opt-in, remote credentials must
be entered again after restarting the app. Keychain access failures leave the
connection form open for retry or manual entry. Locally
started servers remain running when their window closes and can be reconnected
from Recent connections. Closing or quitting checks every workspace for unsaved
changes and does not shut down server kernels.

The titlebar server menu shows connection status and provides **Reconnect**,
**Open server in browser**, and **New connection**. Reachability is checked
every ten seconds. An outage keeps open documents and unsaved edits in memory
and cancels queued notebook runs. When the server returns, choose Reconnect to
reattach surviving sessions without rerunning cells or reloading documents.
Sessions that no longer exist remain disconnected; use the notebook's kernel
controls or explicitly run a cell to start a replacement. Browser links omit
credentials, so your browser may ask you to sign in.

Servers launched by Ganymede offer **Stop Server** and, when stopped,
**Restart Server** in the server card. Stop asks for confirmation and shuts down
all kernels, including work in other clients. Open documents and unsaved edits
remain in their windows. Restart uses the same folder, environment, address, and
token; it does not create kernels or run cells. A stopped entry in Recent
connections can be clicked to start it and reopen its workspace.

Local launch records survive app restarts in `connections.servers/<id>.json`,
alongside the recent-connections file (or the configured override). These separate
records contain the environment, project folder, protected runtime-file path,
launch timestamp, and local server token. On Unix their directory is owner-only
(`0700`) and files are `0600`; writes are atomic. Recent connections contain only
the record ID and connection metadata, never credentials.

Ganymede verifies the private runtime metadata and the authenticated server's
launch timestamp before offering Stop, and checks again before shutdown. A reused
PID or port alone never establishes ownership. A reachable server whose identity
cannot be verified can still be connected to normally, but has no management
actions. Invalid, incompatible, or insecure saved records are ignored. File locks
serialize management operations across app instances. Older local connections
without launch records remain ordinary connections; they are not adopted.
If another process takes a stopped server's port, Ganymede will not replace it.
Startup reports missing Python, jupyter-server, and ipykernel separately.

For a development shortcut, copy `.env.example` to `.env` in the repository root and set
`GANYMEDE_JUPYTER_URL` to the server root (for example,
`http://127.0.0.1:8888/`, without `/lab` or a token query). Set
`GANYMEDE_JUPYTER_TOKEN` separately and optionally
`GANYMEDE_JUPYTER_KERNEL` (defaults to `python3` for this shortcut). The local file is ignored
by Git; process environment variables override its values. The development
build reads this file from the repository root even when launched elsewhere.
Restart the app after changing configuration. The connection appears under
Recent connections using its server address.

Reopening a server restores notebook and text-file tabs in order, the active tab,
sidebar visibility and section, expanded folders, and document scroll positions.
Layouts are stored locally per server URL (including its base path), alongside the
recent-connections file. Layouts contain neither credentials nor document contents.
Restored notebooks attach only to existing
sessions at the same path. They do not start kernels or execute cells. Files that
cannot be reopened remain in the saved layout for a later attempt.

Unsaved notebook and text-file edits are backed up separately in the server's
`recovery/` directory. Changed revisions are queued once per second and written
atomically on a background worker; a crash can lose edits made since the last
completed backup. Recovery copies include document contents and the original
server version, but no connection credentials. On Unix the files are owner-only.
Each document instance has its own copy, so windows do not overwrite each other's
backups. Reopening offers **Restore**, **Discard**, or
**Later**. Restored edits stay dirty with autosave paused until an explicit save;
the original server version still controls conflict detection. Successful saves,
explicit discards, and confirmed deletion of an open file clear its recovery copy.
Unavailable files and copies deferred with **Later** are retained for reopening.

A new connection without a saved layout starts on the launcher without creating
a server notebook. Creating a notebook explicitly uses Jupyter’s unused `Untitled`
name. The file tree starts at the server root; folders
load on expansion. Within each directory, folders sort before files using
case-sensitive filename order. Its toolbar opens the launcher, prompts for a folder name in the selected directory,
and toggles a filename filter input. Filtering is case-insensitive and keeps
ancestor folders visible for matching files, including files in collapsed folders.
Clear/close with the × button or Escape to restore the normal tree. Background
search is bounded to 1,024 directories and 32 levels; directory requests are limited
to four at a time. The overflow menu offers Upload
(native multi-file picker, targeting the selected directory or root) and Refresh. Existing upload names
are rejected; uploads never intentionally replace an existing file. The tree
refreshes quietly every 10 seconds, retaining expanded folders and scroll position. Click a notebook to
open it; an already-open path reuses its tab. Opening a notebook connects automatically,
without executing code. Run or Shift+Enter executes the selected cell. Subsequent
cells share its kernel state. The fast-forward button confirms a kernel restart,
then runs all nonempty Code cells in order, stopping on an error; Markdown and Raw
cells are skipped. Interrupt Kernel clears queued work and interrupts the active
kernel. Restart Kernel confirms that variables will be lost and restarts without
rerunning code. Neither restart action deletes saved outputs. Kernel operation
failures appear as error toasts. A reconnect button appears when disconnected;
it reconnects to the existing session without executing code or resetting variables.
Failed/disconnected executions are never automatically replayed, and queued work
is cancelled on disconnect. A new explicit Run can also reconnect.

Find within a notebook with ⌘F (Ctrl+F elsewhere). The compact find bar searches
Code, Markdown, and Raw cell source using case-insensitive literal matching;
outputs are excluded. Enter/Shift+Enter navigate with wraparound, and Escape closes
the bar and returns focus to the current cell. Matches are highlighted across soft
wraps, including in offscreen cells. Markdown temporarily shows source for the
active match and returns to preview afterward. Query changes are debounced and
matched in the background; edits update results without jumping the scroll position.

Notebook scrolling uses a variable-height virtual list: only viewport cells, a
small overscan region, and an offscreen focused cell are rendered. Cell entities
and their edit/execution state stay alive. Edits and streaming outputs invalidate
individual height measurements while preserving the logical scroll anchor;
insertions/removals preserve unchanged measurements. Text layouts are cached and
only visible source rows are painted. One very large visible output still lays
out as a whole; output-internal virtualization is not implemented.

Notebook cell editors soft-wrap and grow vertically with their source. Code cells
use Python Tree-sitter syntax highlighting, cached until their source changes;
cells over 256 KiB fall back to plain text. Markdown/Raw notebook cells remain
unhighlighted. Standalone Python, Markdown, TOML, JSON, YAML, and SQL files use cached
Tree-sitter highlighting with the same size limit and theme palette. Language
detection follows file extensions, including after rename or Save As. Markdown
includes inline syntax and Python, TOML, JSON, YAML, and SQL fenced code. Tab inserts spaces using the configured indentation width, or
indents selected lines; Shift+Tab outdents. Enter preserves leading whitespace
and, in Code cells, indents Python block headers and bracket continuations.
Backspace in leading spaces removes one indentation step. Block indentation is
one undoable edit and retains the selection direction.

Notebook outputs render PNG/JPEG/GIF/WebP images (including plots), inert HTML
and DataFrame tables, Markdown, LaTeX equations, and ANSI-styled streams/tracebacks.
MIME selection prefers raster images, then HTML, LaTeX, Markdown, and plain text.
Unsupported representations retain their text fallback; malformed images fall back
where possible. Images are bounded to 32 MiB encoded content and 32 million pixels.
HTML preview is bounded to 2 MiB, 2,048 blocks, and tables to 500 rows/64 columns;
limited table/block previews show a notice. Original notebook MIME data is preserved
when saving, including unsupported representations.

Markdown cells open in preview. Double-click or select and press Enter to edit;
Run or Shift+Enter renders the source again. Headings, emphasis, links, nested lists,
fenced code, tables, embedded raster attachments, and LaTeX formulas are supported.
PNG/JPEG/GIF/WebP image URLs load asynchronously over HTTP(S); relative image paths
resolve from the notebook directory through Jupyter’s authenticated Contents API.
Leading `/` paths start at the server root. External requests never receive the
Jupyter token. Previews cache images per cell, limit downloads to 32 MiB and
32 million pixels, and load at most 32 distinct image sources per cell. Failed
loads show an inline placeholder. SVG and data URLs are not supported.
Formulas currently occupy separate lines, including inline math; unsupported LaTeX
shows its source. HTML is parsed into native controls; scripts, CSS, and widgets are not executed. Links open only on an explicit click
and only for HTTP(S)/mailto destinations. This is a native subset rather than a
browser-compatible HTML renderer.

Interactive `input()` and password requests appear beneath the executing cell.
Enter submits the value; repeated prompts reuse that area. Passwords are masked,
excluded from copying, and kept out of notebook content and output history by the
UI (kernel code can still print any value it receives). Replies are sent once for
the matching request; stale requests after interruption/restart are rejected.
Prompts disappear on completion, interruption, restart, or disconnect. Escape
returns focus to the cell without answering; use Interrupt Kernel to cancel.
Save with the toolbar or ⌘S (Ctrl+S elsewhere).
Saving preserves notebook/cell metadata, attachments, unknown fields, cell order,
cell types, and full output payloads, including rich outputs not yet rendered.
The unsaved dot tracks document revisions; edits or output received during a save
remain dirty. Save failures stay visible and retain changes. Closing a dirty
notebook asks before discarding it; reopening a closed notebook loads the server
copy. Closing tabs or the app does not delete server sessions or kernels.

Autosave is enabled by default and saves after about two seconds without document
changes (including execution outputs). Toggle it in Settings; this
preference and the selected theme persist across app launches.
Saves are silent; failures show an error toast and pause autosave until a manual
save or toggling autosave off/on. ⌘S / Ctrl+S always remains available.

Closing a tab or the main window, or using ⌘Q / Quit, checks for unsaved work.
The confirmation offers Cancel, Discard, and Save.
Saving must succeed, with no newer unsaved edits, before closing. Autosave pauses
while the dialog is open. Escape cancels; Tab/arrows choose an action and Enter
confirms. The same save and close protection applies to text files.
OS termination outside the app's Quit action (including force quit) cannot show
this confirmation with the current GPUI platform hooks.

Before each manual or automatic save, Ganymede checks the server's `last_modified`
against the version returned by the last open, reload, or successful save. External
changes (including deletion) pause autosave and open the same confirmation dialog:
Cancel, Reload, Save Copy, or Overwrite. Cancel keeps local edits; Reload discards
them and loads the server version; Overwrite explicitly replaces the server file.
Save Copy uses a server-allocated unused notebook name in the same directory and
shows its path in a toast. The original remains open and dirty, with autosave paused,
until you resolve its conflict or discard it. Toggling autosave does not dismiss a
conflict. Reload is refused during execution or if edits arrive while fetching.

This is a client-side version check, not an atomic conditional write: a different
editor can still save between the check and PUT, and changes that preserve the
server's modification timestamp cannot be detected. Real-time collaboration is
not implemented.

Text files open from the file tree with the plain-text editor, including Markdown,
Python, TOML, lock files, and extensionless files. The launcher creates real Text,
Markdown, and Python files using server-allocated names. Save with
⌘S / Ctrl+S; dirty indicators, autosave, external-change dialogs, and close protection
work for text files too. Detection uses the returned content rather than an extension
allowlist: UTF-8 text (including base64-encoded UTF-8 responses) is accepted; binary
content and unsupported encodings produce an error toast. UTF-8 BOM and uniform CRLF
line endings returned by the server are preserved on save. Specialized Markdown/Python
behavior beyond syntax highlighting is not implemented yet.

Right-click a file or folder for its context menu. Files offer Open, Rename, Save As,
and Delete; folders offer New notebook, New text file, New folder, Upload here,
Refresh, Rename, and Delete. Right-click the tree background for root-level creation
and upload actions. Rename and Save As use animated input dialogs; Save As accepts
a new server-relative path and refuses existing destinations. For an open document,
it saves current local edits and continues editing the new file, leaving the original
on the server. For a closed file, it copies server contents and selects the new file.

⌘-click (Ctrl-click on other platforms) toggles individual tree items. Shift-click
selects a range in the current visible tree order; ⌘/Ctrl+Shift-click adds that range.
Modifier clicks select without opening files or expanding folders. Right-clicking a
selected item preserves the selection. Multiple items offer Delete selected; Rename
and Save As remain single-item actions. Batch moves and deletes process each selected
folder only once, including its descendants, and report individual failures while
keeping successful changes.

Drag files or folders onto another folder to move them; dropping on a file targets
its parent, and dropping on the tree background moves to root. Self/descendant moves
are ignored and name collisions are rejected. Open tabs and their save paths follow
renames and moves, including descendants of moved folders. Pending saves finish before
path changes; unsaved edits remain dirty. Associated open notebook sessions are updated
without restarting or shutting down their kernels. Delete asks for confirmation and
closes affected tabs only after server deletion succeeds. Non-empty-folder deletion
follows the Jupyter server's policy; rejection appears in an error toast.

All mutations use the Jupyter Contents API. Uploads currently use the OS picker;
Finder-to-sidebar drops are not implemented. Git integration and Terminal/Console launcher items remain prototypes. They are
visible in development builds (`cargo run`) and hidden in ordinary release builds
(`cargo run --release`). Saved Git sidebar selections fall back to Files when
prototypes are disabled. To include prototypes in an optimized development build,
use `cargo run --release --features unfinished`. Do not enable that feature
for distributed releases (including through `--all-features`).

To exercise the GUI execution adapter against the configured server:

```sh
cargo test -p ganymede-gui execution::tests::live_execution -- --ignored
```

This test creates its own unique scratch session, verifies streaming output,
shared state, errors, interrupt, and restart, and deletes that test session
afterward.

## Notebook undo and redo

Cell insertion, deletion/cut, duplication, movement, and type changes have a
separate undo history per open notebook (up to 100 structural edits). Press
Escape to select the cell, then Cmd+Z to undo or Cmd+Shift+Z to redo (Ctrl on
other platforms). While editing text or answering an input prompt, these
shortcuts continue to use that input's own undo history.

Restoring a deleted cell retains its source, outputs, ID, metadata, and
attachments. Moving cells does not roll back their text or execution results.
Undo never executes code; changes that would remove or replace a running cell
are blocked until execution finishes or is interrupted. New structural edits clear
redo. History lasts until the notebook closes or is reloaded from the server;
undo/redo changes participate in normal saving and autosave.

## Preference storage

Theme and Autosave preferences are stored in
`~/Library/Application Support/Ganymede/connections.preferences.json` on macOS
(`~/.config/ganymede/connections.preferences.json` elsewhere). Changing a setting
writes the small file atomically; write failures appear in Settings. Missing
files use the existing Light-theme/Autosave-on defaults. Invalid or unsupported
files are left untouched at startup, with a message in Settings.
`GANYMEDE_PREFERENCES_FILE` overrides the path. Otherwise it follows the
`GANYMEDE_CONNECTIONS_FILE` location with a `.preferences.json` extension, keeping
isolated previews separate from normal preferences.

Notebook cell selection uses a contiguous range with one active cell. Shift-click
another cell or press Escape followed by Shift+Up/Down to extend the range.
Up/Down in cell mode selects one neighboring cell; Enter edits the active cell.
Shift-click inside the focused text editor retains normal text selection.
The selected group has a subtle background and a toolbar count; the active cell
retains its marker. Copy/cut/paste, duplicate, delete, move, cell-type changes,
clear output, and run apply to the range. Clipboard metadata preserves cell kinds,
metadata, attachments, and saved outputs, while plain-text consumers receive source
text. Pasted cells receive fresh IDs. Structural undo/redo restores the entire
operation and selection. A group containing a running cell cannot be cut or deleted;
queued and running cells retain their identities when moved. Shift+Enter runs the
range in document order and advances to the following cell.
