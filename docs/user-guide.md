# User guide

[Back to the README](../README.md)

## Create a local server

Choose **Create** on the welcome screen, select your project folder, and choose a
Python environment: Global Python, Virtual environment, or Custom Python.
That environment must have `jupyter-server` and `ipykernel` installed. Click
**Start server** to open the workspace.

Ganymede uses your existing Python installation; it does not bundle or install
Python for you. The selected folder becomes the server's file root.

## Connect to an existing server

Choose **Connect**, enter the Jupyter server URL and token if required, then click
**Connect**. URLs with a deployment prefix are supported, as are pasted Jupyter
Lab URLs containing a token.

**Remember token** saves a successfully validated token in macOS Keychain. Recent
connections let you reopen a server. Their overflow menu offers **Edit connection…**
to review or change the connection before opening it, and **Remove from recents**.
Use **Forget token** in the connection form to remove a saved credential.

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

Open a notebook from the Files sidebar or create one from the workspace launcher.
Choose a kernel using the notebook's kernel control.

- **Shift+Enter** runs the selected cell. Variables persist in its kernel between runs.
- Double-click a Markdown cell, or select it and press **Enter**, to edit it.
  **Shift+Enter** renders the Markdown again.
- **Command+S** saves. Autosave is on by default and can be changed in Settings.
- **Command+F** searches cell source. **Enter** and **Shift+Enter** move between
  matches; **Escape** closes search. Outputs are not searched.
- **Escape**, then **Command+Z**, undoes cell operations such as insertion,
  deletion, movement, and type changes. While typing, undo applies to text edits.

Use the Run and Kernel menus for execution, interrupt, restart, and shutdown.
Restart clears the kernel's variables. Closing a notebook does **not** shut down
its kernel; use the Kernels sidebar to inspect and explicitly shut down kernels.

Images, tables, Markdown, equations, and styled text outputs render natively.
Interactive Jupyter widgets and browser-executed HTML or JavaScript are not
supported. Unsupported output data is retained when saving the notebook.

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

## Settings and local data

Settings controls the theme, autosave, and supported editor preferences. Preferences
and recent connections persist across launches. Reopening a server restores its
saved tabs and sidebar layout; restoring a layout does not execute cells.

On macOS, the default storage directory is
`~/Library/Application Support/Ganymede/`. It contains preferences, recent
connections, workspace layouts, recovery copies, and local server records.
Recovery copies contain notebook or text-file contents, and local server records
include the credentials needed to manage those servers. Remembered remote tokens
are stored in Keychain. Treat local app data as private when sharing diagnostics.

## Report a problem

[Open a GitHub issue](https://github.com/a-poor/ganymede/issues) with your Ganymede
version, macOS version, steps to reproduce, and any relevant error message.
Remove tokens and private notebook contents from logs or screenshots.
