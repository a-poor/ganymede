#!/usr/bin/env bash
# An unsigned preview bundle, not a signed/notarized public release.
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ "$(uname -s)" != Darwin ]]; then
    echo "macOS is required to package Ganymede." >&2
    exit 1
fi
export MACOSX_DEPLOYMENT_TARGET=15.0
cargo build --locked --release -p ganymede-gui --no-default-features
bundle="target/dist/Ganymede.app"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
cp target/release/ganymede-gui "$bundle/Contents/MacOS/Ganymede"
cp ganymede-gui/branding/AppIcon.icns "$bundle/Contents/Resources/AppIcon.icns"
cargo metadata --locked --no-deps --format-version 1 | python3 -c '
import json, plistlib, sys
from pathlib import Path
version = next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "ganymede-gui")
info = {
    "CFBundleIdentifier": "dev.ganymede.app",
    "CFBundleName": "Ganymede",
    "CFBundleDisplayName": "Ganymede",
    "CFBundleExecutable": "Ganymede",
    "CFBundleIconFile": "AppIcon.icns",
    "CFBundlePackageType": "APPL",
    "CFBundleShortVersionString": version,
    "CFBundleVersion": version,
    "NSHighResolutionCapable": True,
    "LSMinimumSystemVersion": "15.0",
}
with Path("target/dist/Ganymede.app/Contents/Info.plist").open("wb") as output:
    plistlib.dump(info, output)
'
plutil -lint "$bundle/Contents/Info.plist"
ditto -c -k --sequesterRsrc --keepParent "$bundle" target/dist/Ganymede-macOS.zip
echo "Unsigned preview: target/dist/Ganymede-macOS.zip"
