# Changelog

User-visible changes are recorded here, newest first. GitHub release notes use
the corresponding version section below. Earlier release notes are available in
[GitHub Releases](https://github.com/a-poor/ganymede/releases).

## Unreleased

## 0.4.0 — 2026-11-11

### Added

- Reorder notebook cells by dragging their margins or non-editing areas, including groups of selected cells, with automatic scrolling near the notebook edges. ([#33](https://github.com/a-poor/ganymede/issues/33))
- Reorder workspace tabs by dragging them, with automatic scrolling along the tab strip. ([#31](https://github.com/a-poor/ganymede/issues/31))
- Select and copy notebook output text, including text across multiple outputs in a cell. ([#32](https://github.com/a-poor/ganymede/issues/32))
- Right-click a cell's output area and choose **Copy output** to copy its output text. Rich outputs use an available text fallback. ([#35](https://github.com/a-poor/ganymede/issues/35))
- Change cell types with Jupyter-style command-mode keyboard shortcuts. ([#34](https://github.com/a-poor/ganymede/issues/34))
- Reveal or hide connection tokens while editing connection settings.

### Fixed

- Keep tab interactions from dragging the macOS window, while preserving window dragging from the titlebar. ([#30](https://github.com/a-poor/ganymede/issues/30))
- Correct backward word deletion keyboard shortcuts in editors. ([#29](https://github.com/a-poor/ganymede/issues/29))
- Save edits to recent connections reliably.
- Include AppStream metadata in Linux packages for software-center discovery.

### Downloads

- **macOS 15+ / Apple Silicon:** signed and notarized `.dmg` or `.zip`.
- **Linux x86-64:** `.deb` for Ubuntu 24.04 or compatible systems, or `.tar.gz`. Packages require glibc 2.39+, a Vulkan-capable driver, and a desktop portal backend. See [Linux setup notes](https://github.com/a-poor/ganymede/blob/v0.4.0/docs/development/linux.md).

Linux support remains new; a complete Wayland/X11 desktop compatibility pass remains outstanding. Python and Jupyter are not bundled and are required only for local servers. Automatic updates are not included yet.

**Full changelog:** [v0.3.1...v0.4.0](https://github.com/a-poor/ganymede/compare/v0.3.1...v0.4.0)
