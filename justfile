# List commands in justfile order.
default:
    @just --list --unsorted

# Launch the native GUI.
run-gui:
    cargo run -p ganymede-gui

# Download Inter, JetBrains Mono, Geist Pixel, and Phosphor assets.
fetch-assets:
    bash scripts/fetch-assets.sh
