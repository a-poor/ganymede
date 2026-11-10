# Linux builds and testing

Normal CI runs formatting, Clippy, and all non-ignored core and GUI tests on
Ubuntu 24.04, including GPUI's simulated-window tests. These do not require a
Mac or a desktop session. It also checks the `unfinished` feature. Tags and
manual CI runs with `full` enabled additionally run macOS checks and optimized
Linux/macOS packaging. Linux checks already run in the normal job, so the full
run does not repeat them on another Linux VM.

## Building packages

On Ubuntu/Debian, install the build tools once:

```sh
bash scripts/install-linux-deps.sh
just package-linux
```

The packager fetches bundled assets, builds with `--release --no-default-features`,
and creates a `.deb`, `.tar.gz`, and SHA-256 manifest in `target/dist`. It does not
install anything or publish a GitHub release. CI uploads these as workflow artifacts.
The package version comes from Cargo metadata. Native amd64 and arm64 packaging
is supported; CI currently builds amd64 only.

The `.deb` includes `/usr/bin/ganymede`, a desktop launcher, icon, and license
files. Linked-library dependencies are calculated with `dpkg-shlibdeps`; desktop
runtime dependencies are included explicitly. Install a downloaded package with:

```sh
sudo apt install ./ganymede_0.3.1_amd64.deb
```

The tarball contains a prefix-relative `bin/` and `share/` tree. Extract it and
run `bin/ganymede`, or copy those directories into `~/.local/` and ensure
`~/.local/bin` is on PATH. To remove a manual installation, remove only
`~/.local/bin/ganymede`, `~/.local/share/applications/com.austinpoor.ganymede.desktop`,
`~/.local/share/icons/hicolor/512x512@2/apps/com.austinpoor.ganymede.png`,
`~/.local/share/metainfo/com.austinpoor.ganymede.metainfo.xml`, and
`~/.local/share/doc/ganymede`. Application data is separate and retained.

These are dynamically linked native packages, not self-contained AppImages.
CI's Ubuntu 24.04 builds require compatible system libraries (including glibc
2.39 or newer). A package built on a newer local distribution can require a newer
glibc; inspect its dependencies with `dpkg-deb --field PACKAGE.deb Depends`.
Other distributions and older systems may need a source build.
The tarball needs the same libraries as the `.deb`, a Vulkan-capable driver,
X11 or Wayland, and a desktop portal backend for the user's desktop. A Secret
Service provider such as GNOME Keyring or KWallet is needed to remember tokens.
Python/Jupyter are needed only for local servers, not remote connections.

The desktop entry follows the [freedesktop specification](https://specifications.freedesktop.org/desktop-entry/latest-single/).
Dependency generation follows [Debian shared-library guidance](https://www.debian.org/doc/debian-policy/ch-sharedlibs.html).

## Software listings

Both Linux packages include AppStream metadata in `share/metainfo/`, describing
the application, developer, license, screenshot, and project links. The packager
validates it with `appstreamcli` (installed by `scripts/install-linux-deps.sh`).
Keep `packaging/linux/com.austinpoor.ganymede.metainfo.xml` current when changing
the application's description or screenshot. Software catalogs can use this
metadata when indexing the package; installing the file alone does not publish
an App Center listing.

Ubuntu App Center's local `.deb` installer currently does not use the AppStream
developer or release date for its title and information fields. It also always
shows a third-party package warning. Adding metadata or a detached signature
does not remove that warning. The package remains unsandboxed, so its
“Unrestricted” permissions label is expected.

## Signing release downloads

The packager writes a `.sha256` manifest covering the `.deb` and `.tar.gz`.
For public downloads, the release maintainer can sign that manifest with an
existing GPG signing key on their release machine. This is separate from macOS
code signing and notarization; neither packaging nor CI currently signs Linux
downloads automatically.

After downloading the final artifacts into one directory, first verify their
checksums, then sign the manifest. Replace `YOUR_SIGNING_KEY_FINGERPRINT` with
the full fingerprint of the intended signing key:

```sh
sha256sum --check ganymede-0.3.1-linux-amd64.sha256
gpg --armor --detach-sign --local-user YOUR_SIGNING_KEY_FINGERPRINT ganymede-0.3.1-linux-amd64.sha256
gpg --armor --export YOUR_SIGNING_KEY_FINGERPRINT > ganymede-release-key.asc
```

On macOS, use `shasum -a 256 --check` instead of `sha256sum --check`.
Upload the manifest, its `.asc` signature, and the public key alongside the exact
artifacts it covers. Publish the key fingerprint through a trusted project
channel so users can check it independently. Never upload the private key.

Users who have verified and imported the public key can verify the signature
and then the artifact checksums:

```sh
gpg --verify ganymede-0.3.1-linux-amd64.sha256.asc ganymede-0.3.1-linux-amd64.sha256
sha256sum --check ganymede-0.3.1-linux-amd64.sha256
```

A detached signature supports manual download verification; App Center does
not verify this sidecar file when opening a local `.deb`. Authenticated APT
updates would require a separately configured signed repository, which is not
provided yet.

## Desktop verification

Automated GUI tests validate input and view behavior using GPUI's test platform;
they do not exercise the real compositor, graphics driver, portal, or keyring.
Before releasing, test on both a Wayland and an X11 desktop:

- Launch from the desktop menu; verify the icon and window grouping.
- Drag, resize, maximize, tile, restore, minimize, and close the main window.
  Check connection, Settings, and About corners and controls too.
- Exercise menus and keyboard navigation at normal and fractional display scales.
- Open/save/export using native file dialogs; use Show file and the local project
  sidebar's Show in file manager action. Remote and SSH projects omit that action.
- Create a local environment, install packages, start Jupyter, and execute a cell.
- Try remembering and forgetting tokens with the keyring unlocked, locked, and
  unavailable; verify that manual entry without saving still works.
- Close with unsaved changes and relaunch to check recovery and XDG persistence.

A real desktop session is needed for this pass. Headless tests cannot establish
Wayland/X11 parity or the behavior of each desktop's portal backend.

## Tray integration

Linux uses `tray-icon` with its KSNI backend (StatusNotifierItem and D-BusMenu),
with no GTK/AppIndicator library dependency. A session D-Bus and a compatible
panel/extension are required to display the icon. GNOME users may need the
AppIndicator extension. The **Show tray icon** setting uses the same stored
preference as macOS's **Show menu bar icon** setting.

Normal Linux CI also exercises tray registration, menu updates, and cleanup on
an isolated D-Bus session:

```sh
dbus-run-session -- cargo test --locked -p ganymede-gui linux_tray_exports_updates_and_removes_its_dbus_menu -- --ignored
```

On a real desktop, toggle the tray off and on, open a recent connection, open
Settings, and inspect the running-server menu. Verify that **Stop…** asks for
confirmation and that canceling it leaves the server running. Close all windows
and use **Open Ganymede** to reopen one. Test **Quit Ganymede** with unsaved work.
The private-bus test does not establish panel rendering or activation behavior.
