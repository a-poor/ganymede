#!/usr/bin/env bash
# Build dependencies for Ubuntu 24.04 CI and Debian/Ubuntu development machines.
set -euo pipefail
sudo apt-get update
sudo apt-get install --no-install-recommends -y \
    build-essential clang cmake pkg-config libfontconfig1-dev libfreetype-dev \
    libwayland-dev libx11-xcb-dev libxkbcommon-x11-dev libssl-dev \
    libvulkan1 libegl1-mesa-dev desktop-file-utils dpkg-dev
