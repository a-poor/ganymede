# List commands in justfile order.
default:
    @just --list --unsorted

# Launch the native GUI.
run-gui:
    cargo run -p ganymede-gui

# Download Inter, JetBrains Mono, Geist Pixel, and Phosphor assets.
fetch-assets:
    bash scripts/fetch-assets.sh

# Build, notarize, verify, and upload a draft release (requires signing env vars).
release-macos notes:
    python3 scripts/release-macos.py --notes {{quote(notes)}}

# Build Linux .deb and tar.gz packages without installing or publishing.
package-linux:
    python3 scripts/package-linux.py
