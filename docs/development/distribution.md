# macOS distribution

[Back to the README](../../README.md)

Run commands from the repository root.

## Checks and preview builds

GitHub Actions runs formatting, Clippy, workspace tests, and a check of the
`unfinished` feature on pushes and pull requests. Live Jupyter tests remain a
separate opt-in check via `scripts/test_jupyter.py`. Rust is pinned in
`rust-toolchain.toml`.

After checks pass, CI builds an optimized macOS app with unfinished features
disabled and uploads unsigned, architecture-specific ZIP and DMG artifacts. This is
a testing artifact, not a signed or notarized public release. It requires macOS
15 or later. To build the same bundle locally, fetch assets, then run:

```sh
bash scripts/package-macos.sh
```

The results are `target/dist/Ganymede-macOS.zip` and `target/dist/Ganymede-macOS.dmg`,
including the PlanetRise app icon. The DMG contains the app and an Applications
shortcut for drag-and-drop installation.
The preview bundle is ad-hoc signed for integrity; it is not Developer ID signed
or notarized and is not ready for public distribution. Automatic updates are
not included yet.

To arrange the DMG icons in a compact Finder window on a logged-in Mac, use
`GANYMEDE_DMG_LAYOUT=1 bash scripts/create-dmg.sh`. This repackages the existing
bundle without rebuilding it. Finder automation permission may be requested.
CI uses the default Finder layout and does not require a desktop session.

For public distribution, install a **Developer ID Application** certificate and
its private key in Keychain, and store notarization credentials with
`xcrun notarytool store-credentials`. Then run:

```sh
GANYMEDE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)" \
GANYMEDE_NOTARY_PROFILE="ganymede-notary" \
bash scripts/notarize-macos.sh
```

This signs the built app with the hardened runtime and a timestamp, submits it
to Apple, staples and validates the ticket, checks Gatekeeper acceptance, and
creates `target/dist/Ganymede-macOS-notarized.zip`. It also packages the stapled
app in `target/dist/Ganymede-macOS-notarized.dmg`, signs and notarizes that image,
and staples and verifies its ticket. Set `GANYMEDE_DMG_LAYOUT=1` on this command
to include the arranged Finder layout. It fails on any unsuccessful
step. Credentials are read from Keychain; never commit certificates, private
keys, or passwords. CI continues producing preview artifacts without credentials.
See [Apple’s notarization workflow](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution).
