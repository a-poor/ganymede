#!/usr/bin/env bash
# Sign and notarize the already-built release bundle. Credentials stay in Keychain.
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ "$(uname -s)" != Darwin ]]; then
    echo "macOS is required for signing and notarization." >&2
    exit 1
fi
: "${GANYMEDE_SIGNING_IDENTITY:?Set this to your Developer ID Application identity}"
: "${GANYMEDE_NOTARY_PROFILE:?Set this to your notarytool Keychain profile name}"
bundle="target/dist/Ganymede.app"
archive="target/dist/Ganymede-macOS-notarized.zip"
submission_dir="$(mktemp -d)"
trap 'rm -rf "$submission_dir"' EXIT
submission="$submission_dir/Ganymede.zip"
[[ -f "$bundle/Contents/MacOS/Ganymede" ]] || {
    echo "Build the release first with bash scripts/package-macos.sh" >&2
    exit 1
}
codesign --force --options runtime --timestamp --sign "$GANYMEDE_SIGNING_IDENTITY" "$bundle"
codesign --verify --deep --strict --verbose=2 "$bundle"
ditto -c -k --sequesterRsrc --keepParent "$bundle" "$submission"
xcrun notarytool submit "$submission" --keychain-profile "$GANYMEDE_NOTARY_PROFILE" --wait
xcrun stapler staple "$bundle"
xcrun stapler validate "$bundle"
spctl --assess --type execute --verbose=2 "$bundle"
# Include the stapled ticket in the distributed archive.
ditto -c -k --sequesterRsrc --keepParent "$bundle" "$archive"
echo "Notarized release: $archive"
# Package the stapled app, then sign/notarize the disk image itself.
dmg="target/dist/Ganymede-macOS-notarized.dmg"
bash scripts/create-dmg.sh "$dmg"
codesign --force --timestamp --sign "$GANYMEDE_SIGNING_IDENTITY" "$dmg"
codesign --verify --strict --verbose=2 "$dmg"
xcrun notarytool submit "$dmg" --keychain-profile "$GANYMEDE_NOTARY_PROFILE" --wait
xcrun stapler staple "$dmg"
xcrun stapler validate "$dmg"
spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
echo "Notarized release: $dmg"
