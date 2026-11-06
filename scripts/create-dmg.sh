#!/usr/bin/env bash
# Package an existing bundle without changing its signature.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ "$(uname -s)" == Darwin ]] || { echo "macOS is required." >&2; exit 1; }
output="${1:-target/dist/Ganymede-macOS.dmg}"
bundle="target/dist/Ganymede.app"
[[ -f "$bundle/Contents/MacOS/Ganymede" ]] || {
    echo "Build the app first with bash scripts/package-macos.sh" >&2
    exit 1
}
work="$(mktemp -d)"
mounted=false
cleanup() {
    if [[ "$mounted" == true ]]; then
        hdiutil detach "$work/mount" -quiet || hdiutil detach "$work/mount" -force -quiet || true
    fi
    rm -rf "$work"
}
trap cleanup EXIT
mkdir -p "$work/staging" "$(dirname "$output")"
ditto "$bundle" "$work/staging/Ganymede.app"
ln -s /Applications "$work/staging/Applications"
hdiutil create -quiet -volname Ganymede -srcfolder "$work/staging" \
    -fs HFS+ -format UDRW "$work/writable.dmg"
# Finder layout requires a logged-in desktop. CI still produces a usable DMG.
if [[ "${GANYMEDE_DMG_LAYOUT:-0}" == 1 ]]; then
    mkdir "$work/mount"
    hdiutil attach "$work/writable.dmg" -mountpoint "$work/mount" -nobrowse -quiet
    mounted=true
    osascript - "$work/mount" <<'APPLESCRIPT'
on run argv
    set imageFolder to POSIX file (item 1 of argv) as alias
    tell application "Finder"
        set imageDisk to disk (name of imageFolder)
        open imageDisk
        set current view of container window of imageDisk to icon view
        set toolbar visible of container window of imageDisk to false
        set statusbar visible of container window of imageDisk to false
        set bounds of container window of imageDisk to {200, 200, 800, 560}
        set viewOptions to icon view options of container window of imageDisk
        set arrangement of viewOptions to not arranged
        set icon size of viewOptions to 112
        set text size of viewOptions to 14
        set position of item "Ganymede.app" of imageDisk to {155, 150}
        set position of item "Applications" of imageDisk to {445, 150}
        update imageDisk without registering applications
        delay 2
        close container window of imageDisk
        delay 2
    end tell
end run
APPLESCRIPT
    sync
    hdiutil detach "$work/mount" -quiet
    mounted=false
fi
hdiutil convert "$work/writable.dmg" -quiet -format UDZO -o "$work/final.dmg"
hdiutil verify "$work/final.dmg"
mv "$work/final.dmg" "$output"
echo "Disk image: $output"
