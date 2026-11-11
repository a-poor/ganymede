# Repository Guidelines

## Project Structure & Module Organization

Ganymede is a Rust 2024 workspace with two crates:

- `ganymede-core/src/jupyter/`: REST clients, WebSocket transport, and protocol models.
- `ganymede-core/src/notebook/`: editable documents, execution state, and the application-facing controller. Keep this layer independent of GPUI.
- `ganymede-gui/src/`: GPUI startup and views; `theme.rs` defines semantic design tokens.
- `ganymede-core/tests/`: mock-server and live Jupyter integration tests. Unit tests also live beside their implementations.
- `ganymede-core/examples/`: executable client and notebook examples.
- `scripts/test_jupyter.py`: disposable Jupyter test runner.

Downloaded fonts and icons live in `ganymede-gui/assets/`, whose contents are gitignored. Run `just fetch-assets` to populate them. GPUI dependencies are pinned to a Zed Git revision; update both GPUI dependencies together.

## Build, Test, and Development Commands

Run commands from the repository root:

- `cargo run -p ganymede-gui`: launch the native window; `cargo run` defaults to this crate.
- `cargo build --workspace`: build both crates.
- `cargo check --workspace`: check compilation without producing executables.
- `cargo fmt --all -- --check`: verify formatting; use `cargo fmt --all` to apply it.
- `cargo clippy --workspace --all-targets -- -D warnings`: lint application code, tests, and examples.
- `cargo test --workspace`: run unit and mock-server tests.
- `python scripts/test_jupyter.py`: run live integration tests and examples on Unix; requires `jupyter-server` and `ipykernel` in that Python environment.

## Coding Style & Naming Conventions

Use rustfmt defaults, four-space indentation, `snake_case` functions/modules, and `PascalCase` types. Prefer typed errors and explicit lifecycle operations. Preserve unknown notebook JSON fields and revision-based dirty state.

In GPUI render code, obtain colors, fonts, and radii from `cx.theme()`. Consult relevant `.agents/skills/gpui*` instructions and verify APIs against the pinned source.

When work reveals an error, omission, or reusable improvement in a user-maintained skill, verify it against the pinned source or a reproduction and check the upstream skill and existing issues. Create an issue when authorized, or suggest one with concrete evidence and the proposed correction. The `gpui`, `gpui-editor`, and `gpui-style` skills come from `a-poor/skills` (https://github.com/a-poor/skills); use `skills-lock.json` to identify the source of other skills. Include the affected skill/reference, relevant version or revision, observed behavior, and validation in reports. Avoid duplicate issues and keep credentials and private application data out of examples.

Prefer compact, information-dense UI: small icons and text, restrained padding, and tight row spacing. Avoid oversized cards and controls. Settings selectors should fit on a single line where practical.

## Testing Guidelines

Use `#[test]` and `#[tokio::test]`; name tests after observable behavior. Add regression coverage for protocol, persistence, and concurrency changes. Live tests are ignored by default and exercised by the Python runner. No numeric coverage threshold is configured. Visually verify GUI changes and retain theme contrast checks.

## Commit & Pull Request Guidelines

Use concise, action-oriented commit subjects, such as `Add application-facing notebook documents and controller`. Keep unrelated edits separate. PR descriptions should explain behavior, relevant issues, and validation; include screenshots for visible GUI changes.

Prefer rebasing feature branches onto `main` when possible to bring them up to date. When integrating a multi-commit feature branch into `main`, prefer squash merge.

## Security & Configuration

Examples use `GANYMEDE_JUPYTER_URL` and `GANYMEDE_JUPYTER_TOKEN`. Keep tokens out of committed files, URLs, and logs. Never automatically replay execution requests after disconnects or treat closing a window as authorization to delete a server kernel.
