# User guide

[Back to the README](../README.md)

## Application menus

On Linux, the main window shows File, Edit, View, Run, Kernel, Window, and Help
menus above the active tab's content, including the launcher. Settings is in File;
About and Check for Updates are in Help. The connection and Settings windows use
a hamburger menu beside the close button with commands relevant to those windows.
macOS continues to use the system menu bar.

Press **F10** on Linux to open the menu. Use Left/Right to switch menus, Up/Down to
select a command, and Enter to run it. Escape, Tab, or clicking outside dismisses
the menu. Unavailable commands are dimmed. Text-editing commands operate on the
editor selection that was active before opening the menu.

## Create a local server

Choose **Create** on the welcome screen, select your project folder, and choose a
Python environment. Ganymede checks the project's `.venv` and `venv`, Python
installations on `PATH`, common macOS install locations, and installed Python
versions reported by uv. Detected entries show their version and path. Readiness and missing packages appear
below the picker after selection.
A ready project environment is preferred automatically until you make a choice.
The **Python runtimes** section includes `python3 (PATH)` and detected interpreters.
Below the divider, choose **Virtual environment** or **Custom Python path** to enter
your own location.
For a virtual-environment folder that does not exist, **Create environment…**
creates it with `python3` from PATH and installs `jupyter-server` and `ipykernel`
after confirmation. The default folder is `.venv` inside your project. To use a
different Python or environment setup, create it yourself and select its folder.
Existing folders are never overwritten. If setup fails or is cancelled, any
partially created folder is kept; you can finish setup yourself or select a new path.
That environment must have `jupyter-server` and `ipykernel` installed. Click
**Start server** to open the workspace.

Ganymede uses your existing Python installation; it does not bundle or install
Python for you. Discovery never installs packages or creates environments.
Use **Install…** to install missing `jupyter-server` or `ipykernel` packages after
confirming the selected interpreter. Ganymede uses uv when available, otherwise
that interpreter's pip. OS-managed environments may reject installation; choose a
virtual environment in that case. Installation does not edit `pyproject.toml` or
`uv.lock`. **Refresh** checks again after changes made outside the app.

These choices launch Python directly, including uv-managed interpreters. They do
not run `uvx` or create an isolated tool environment. Relative virtual-environment
paths are resolved from the project folder. The selected folder becomes the server's
file root. Kernels are listed by the connected server after it starts; discovered
Python installations are not automatically registered as kernels.

## Connect to an existing server

Choose **Connect**, enter the Jupyter server URL and token if required, then click
**Connect**. URLs with a deployment prefix are supported, as are pasted Jupyter
Lab URLs containing a token.

**Remember token** saves a successfully validated token in the system keyring
(macOS Keychain on macOS). Turn it off to connect without saving a token. Linux
credential storage requires an available, unlocked system keyring. If saving
fails, unlock it and retry, or turn off Remember token and connect again. If a
saved token cannot be loaded, the form turns remembering off and lets you enter
the token manually. Turning remembering off does not delete an existing saved
token; use **Forget token** for that.

Use the eye icon beside the token field to show or hide it while editing. Tokens
start hidden; opening another connection hides them again.

Recent connections let you reopen a server. Their overflow menu offers **Edit connection…**
to review or change the connection before opening it, and **Remove from recents**.
Choose **Save and connect** to validate and save your edits. Changing the URL or
SSH host replaces the original Recent entry after a successful connection. Storage
failures keep the form open so you can retry. With Remember token off, an edited
token is used for reconnects during this app session; an older saved token is not
deleted or replaced. Use **Forget token** to remove that saved credential.

### Connect through SSH

Expand the advanced connection options and enter an SSH config alias or
`user@host`. Enter the Jupyter URL as seen from that host—for example,
`http://127.0.0.1:8888/` if Jupyter runs on the remote machine's loopback interface.

Ganymede uses OpenSSH and your existing keys, agent, and `~/.ssh/config`. Configure
custom ports and jump hosts there. Connect in Terminal first to verify the host
key, and unlock encrypted keys with `ssh-add` if needed. Interactive SSH password
prompts are not supported.

SSH connections reach an already-running Jupyter server. **Open server in browser**
is unavailable for them because your browser does not share Ganymede's SSH tunnel.

## Work with notebooks

Drag a notebook, file, or Launcher tab to rearrange it within the window. The
insertion line marks the drop position; hold near either end of an overflowing
tab strip to scroll. Press **Escape** or drop outside the strip to cancel. Moving
a tab keeps the active document and unsaved edits intact.

Open a notebook from the Files sidebar or create one from the workspace launcher.
Choose a kernel using the notebook's kernel control.

Drag a cell’s margin, outside its input and output content, to reorder it.
Dragging a selected cell moves the entire selected group. The insertion line marks the destination;
hold near the top or bottom of the notebook to scroll. Press **Escape** or drop
outside the notebook content to cancel. Each completed move is one undoable
operation and preserves cell content, outputs, and running work.

- **Shift+Enter** runs the selected cell. Variables persist in its kernel between runs.
  In Settings, **Run cell shortcut** can swap Enter and Shift+Enter while editing;
  the other shortcut inserts an indented newline. Cell navigation mode keeps
  Enter to edit and Shift+Enter to run.
- Double-click a Markdown cell, or select it and press **Enter**, to edit it.
  **Shift+Enter** renders the Markdown again.
- Press **Escape** to select a cell, then **M** for Markdown, **Y** for code, or
  **R** for raw. These commands change all selected cells as one undoable action
  and keep cell selection focused; press **Enter** to resume editing. The keys
  remain ordinary text in editors, text fields, and kernel input prompts.
- **Command+S** saves. Autosave is on by default and can be changed in Settings.
- **Command+F** searches cell source. **Enter** and **Shift+Enter** move between
  matches; **Escape** closes search. Outputs are not searched.
- **Escape**, then **Command+Z**, undoes cell operations such as insertion,
  deletion, movement, and type changes. While typing, undo applies to text edits.

In notebook source and text files, **Option+Left/Right** moves by word; add
**Shift** to extend the selection. Double-click selects a word, and triple-click
selects a full line. Drag after either gesture to select whole words or lines.

**Auto close brackets** in Settings pairs `()`, `[]`, `{}`, and quotes while typing
in notebook source and text files. Type an opening character to wrap selected text;
type the existing closing character to move past it. Backspace between an empty
pair removes both characters. Paste and IME composition remain literal.

Use the Run and Kernel menus for execution, interrupt, restart, and shutdown.
Restart clears the kernel's variables. Closing a notebook does **not** shut down
its kernel; use the Kernels sidebar to inspect and explicitly shut down kernels.

Images, tables, Markdown, equations, and styled text outputs render natively.
Interactive Jupyter widgets and browser-executed HTML or JavaScript are not
supported. Unsupported output data is retained when saving the notebook.

Drag across output text to select it, then use **Cmd+C** (**Ctrl+C** on Linux)
or Edit → Copy. **Cmd+A / Ctrl+A** selects the text in that cell's outputs;
**Escape** clears the selection. Shift-click extends it. Selection works across
wrapped lines and rendered paragraphs, code blocks, and tables; copied tables
use tabs between columns and newlines between rows. Scroll with the wheel or
trackpad while dragging to reach more of a long output. Output text is read-only.

Appending streamed text preserves the selection. Replacing or clearing existing
output text resets it. Selection stays within one cell's outputs. Use
**Cmd-click / Ctrl-click** to open links in rendered output without selecting text.
Images and rendered equations offer **Copy text representation** when their MIME
bundle contains plain text or LaTeX; otherwise a label indicates that no text
representation is available.

## Manage files

The Files sidebar browses the connected server's files. Open text files directly,
or right-click files and folders for creation, rename, Save As, and delete actions.
Use the sidebar menu to upload files through the macOS file picker.

**Command-click** selects multiple items; **Shift-click** selects a range. Drag
items onto a folder to move them. Deletion asks for confirmation. Uploads and moves
refuse conflicting destination names.

These operations affect the server's files. For remote connections, they do not
operate on files on your Mac. Dragging files from Finder into the sidebar is not
yet supported.

## Save and recover work

Autosave saves after a short pause in editing. If saving fails, Ganymede keeps your
edits, shows an error, and pauses autosave until you save manually or toggle it
back on. Closing with unsaved changes offers **Cancel**, **Discard**, or **Save**.

If the server's copy changed, choose whether to reload it, save a separate copy,
or overwrite it. The conflict check cannot prevent every simultaneous write by
another editor; real-time collaborative editing is not supported.

Ganymede also keeps local recovery copies of unsaved edits. After a crash, reopening
a document can offer **Restore**, **Discard**, or **Later**. Restored edits remain
unsaved, with autosave paused until you explicitly save. Recovery is best-effort;
it does not replace saving or backups.

## Handle a disconnected server

The server icon shows connection status. Open its card and choose **Reconnect**
when the server is available again. Open documents and edits remain in memory;
cached files may remain visible while the server is unavailable.

Ganymede never automatically reruns cells after a disconnect. Check the kernel's
state before deciding whether to run interrupted work again.

For servers created by Ganymede, the server card also offers **Stop Server** and
**Restart Server**. Stopping shuts down that server's kernels, including work in
other clients. Closing a window or quitting Ganymede leaves servers and kernels
running. SSH tunnels close when their workspace closes.

## macOS menu bar

The Ganymede menu-bar icon provides quick access to a new connection, recent
connections, running local servers, Settings, and Quit. Recent connections use the
same saved settings and credentials as the launcher. Stopping a local server
requires confirmation and ends its kernels. Hide the icon with **Show menu bar
icon** in Settings.

## Settings and local data

Settings controls the theme, autosave, and supported editor preferences. Preferences
and recent connections persist across launches. Reopening a server restores its
saved tabs and sidebar layout; restoring a layout does not execute cells.

On macOS, the default storage directory is
`~/Library/Application Support/Ganymede/`. It contains preferences, recent
connections, workspace layouts, recovery copies, and local server records.

On Linux, Ganymede follows XDG directory settings:

| Contents | Location | Default base directory |
| --- | --- | --- |
| Preferences | `$XDG_CONFIG_HOME/ganymede/preferences.json` | `~/.config` |
| Recent connections | `$XDG_STATE_HOME/ganymede/connections.json` | `~/.local/state` |
| Layouts and recovery copies | `$XDG_STATE_HOME/ganymede/connections.workspaces/` | `~/.local/state` |
| Local server records | `$XDG_STATE_HOME/ganymede/connections.servers/` | `~/.local/state` |
| Server runtime files | `$XDG_RUNTIME_DIR/ganymede/server-<uuid>/` | Private temporary directory if unavailable |

Empty or relative config/state directory settings use the defaults. Runtime
directories must be owned by the current user with permissions `0700`.
Ganymede currently has no separate persistent cache or application-data files
requiring `XDG_CACHE_HOME` or `XDG_DATA_HOME`.

At startup, Linux builds copy legacy data from `~/.config/ganymede/` to the new
locations without overwriting existing destination files. Original files remain
available for recovery, including any conflicting backups. Completion markers
prevent migration from restoring old state after you delete its new copy.
If migration fails, startup stops and reports the error; the originals remain.
Close older Ganymede versions before migrating so they cannot keep writing to
the legacy locations.

For development and tests, `GANYMEDE_CONNECTIONS_FILE` still overrides connection
storage and the derived workspace, server, and preference paths, bypassing legacy
migration. `GANYMEDE_PREFERENCES_FILE` overrides preferences independently. These
development overrides take precedence over XDG settings; normal Linux use should
use XDG settings instead.

Recovery copies contain notebook or text-file contents, and local server records
include the credentials needed to manage those servers. Remembered remote tokens
are stored in the system keyring (Keychain on macOS). Treat local app data as
private when sharing diagnostics.

## Check for updates

Choose **Ganymede → Check for Updates…**. A toast reports whether a newer stable
release is available. **Open release** opens its GitHub page so you can download
and install it. If the check fails, use **Retry** or try again later.

Checks happen only when requested and do not send Jupyter credentials. Updates
are not downloaded or installed automatically.

## Report a problem

[Open a GitHub issue](https://github.com/a-poor/ganymede/issues) with your Ganymede
version, macOS version, steps to reproduce, and any relevant error message.
Remove tokens and private notebook contents from logs or screenshots.

## Selecting multiple cells

Shift-click another cell to select a range. For keyboard selection, press Escape
to leave text editing, then use Shift+Up/Down. Up/Down selects a single cell and
Enter edits it. The toolbar shows the number of selected cells.

Run, copy, cut, paste, duplicate, delete, move, change type, and clear output apply
to the selected group. Shift+Enter runs the group and advances. In cell mode,
Command+C/X/V (Control on other platforms) copies/cuts/pastes cells and Delete or
Backspace deletes them. Undo restores a whole group operation, including its
selection. Copy/paste preserves cell types and saved outputs.

## Export a notebook

Choose **File → Export Notebook → HTML, Markdown, or PDF**. Ganymede sends a
snapshot of the current notebook to the connected Jupyter server’s `nbconvert`
endpoint, then opens a native dialog to save the result on your Mac. Unsaved
edits and existing outputs are included; exporting does not save the notebook,
change its dirty state, or request cell execution. Edits made after export begins
are not included. Only one export runs at a time in each workspace.

Install `nbconvert` in the **server’s Python environment** (including for SSH or
remote connections). PDF uses nbconvert’s Chromium-based WebPDF layout, avoiding
a LaTeX installation. Set it up in that environment with:

```sh
python -m pip install 'nbconvert[webpdf]'
python -m playwright install chromium
```

Ganymede does not install these dependencies or download browsers automatically.
Use the default non-executing nbconvert configuration; custom server-side
preprocessors are controlled by the server administrator. Export needs a live
connection and permission to use the server’s nbconvert endpoint. Conversion has
a two-minute timeout and a 128 MB download limit. Failures appear in a toast;
server logs contain detailed conversion errors.

Markdown without extracted images saves as `.md`. With images, Jupyter returns
`.zip`; extract it and keep the Markdown document beside its image files. HTML
and PDF use nbconvert’s layouts. Notebook attachments and saved image outputs are
supported. Notebook-relative external files are not resolved by the snapshot
endpoint: use attachments or embedded outputs instead. Interactive widgets and
JavaScript outputs may not survive static conversion, and HTML equations or
widgets may need network access to load their rendering libraries. PDF rendering
also depends on those resources being available to Chromium.

For managed local projects, right-click a file or folder and choose **Show in Finder**
on macOS or **Show in file manager** on Linux
to reveal it on your desktop. This action is omitted for remote and SSH connections.
After exporting a notebook, **Show file** reveals the saved export.

On Linux, **Show tray icon** in Settings enables a StatusNotifierItem tray menu
with the same recent connections and local-server commands. The desktop must
provide a compatible tray host; GNOME may require an AppIndicator extension.
The tray is optional, and its commands remain accessible in the app. Closing a
window does not stop local servers; use the tray's **Stop…** command to confirm
stopping a server, or **Quit Ganymede** to exit the application.
