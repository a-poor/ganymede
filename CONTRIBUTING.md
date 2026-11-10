# Contributing

Ganymede is an early native Jupyter client. Bug reports and focused contributions
are welcome. Please discuss larger features in an issue before implementing them.

## Development setup

The supported development and release target is macOS 15 or later. Install Xcode
and its command-line tools, Rust through rustup, and the Metal compiler toolchain.
The repository pins Rust in `rust-toolchain.toml`; rustup selects it automatically.
CI in `.github/workflows/ci.yml` is the reference build configuration.

```sh
git clone https://github.com/a-poor/ganymede.git
cd ganymede
bash scripts/fetch-assets.sh
cargo run -p ganymede-gui
```

Python is not bundled. For local servers and live integration tests, install
`jupyter-server` and `ipykernel` in a Python environment. See the
[user guide](docs/user-guide.md) for connections and the
[application behavior reference](docs/development/application-behavior.md) for
development configuration and implementation details.

## Workspace

- `ganymede-core`: Jupyter transport, notebook documents, and controllers; no GUI dependency.
- `ganymede-gui`: native GPUI views, platform integration, and semantic theme tokens.
- `scripts`: asset downloads, integration tests, and macOS packaging.
- `docs/user-guide.md`: instructions for app users.
- `docs/development/`: contributor API, behavior, and distribution references.

GPUI dependencies are pinned to one Zed revision; update them together.
See [AGENTS.md](AGENTS.md) for coding and review conventions. Commands below run
from the repository root. `just` is optional; `just` lists the available shortcuts.

## Checks

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo check --locked -p ganymede-gui --features unfinished
```

For live Jupyter checks, run `python scripts/test_jupyter.py` using the Python
environment with Jupyter installed. The runner creates and cleans up a disposable
server. These tests are separate from the default unit and mock-server suite.

Unfinished features are visible in debug builds and hidden in ordinary release
builds. Use `--features unfinished` for an optimized development build that includes
them. Never enable that feature in a distributed release.

## Changes and bug reports

Keep changes focused, explain the behavior and validation in pull requests, and
include screenshots for visible changes. Add regression tests for protocol,
persistence, and concurrency fixes. Never automatically replay execution after a
disconnect or treat closing a window as permission to shut down a kernel.

For bugs, include the app version, operating system and desktop environment, reproduction steps, and relevant
errors. Remove tokens, credentials, private notebook contents, and sensitive server
addresses before attaching logs or screenshots.

## Bundled assets

Run `bash scripts/fetch-assets.sh` (or `just fetch-assets`) to download the GUI assets (requires Bash, curl, unzip,
and tar):

- [Inter 4.1](https://github.com/rsms/inter/releases/tag/v4.1) desktop fonts → `ganymede-gui/assets/fonts/inter/`
- [JetBrains Mono 2.304](https://github.com/JetBrains/JetBrainsMono/releases/tag/v2.304) desktop fonts → `ganymede-gui/assets/fonts/jetbrains-mono/`
- Geist Pixel 1.7.0 Square font → `ganymede-gui/assets/fonts/geist-pixel/`
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

## Licensing

Contributions are provided under the project's dual MIT OR Apache-2.0 license.
Third-party fonts, icons, and dependencies retain their own licenses.
See [THIRD_PARTY.md](THIRD_PARTY.md).

For Linux build dependencies, packages, and desktop verification, see
[Linux builds and testing](docs/development/linux.md).
