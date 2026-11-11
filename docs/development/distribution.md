# macOS distribution

[Back to the README](../../README.md)

Run commands from the repository root.

## Prepare a public release

Use the local release command on an Apple Silicon Mac. Signing stays in macOS
Keychain; GitHub Actions continues to build unsigned preview artifacts.

One-time setup:

- Install the Developer ID Application certificate and matching private key in Keychain.
- Store notarization credentials using `xcrun notarytool store-credentials ganymede-notary`.
- Authenticate `gh` with access to create releases in `a-poor/ganymede`.
- Install the development prerequisites described in [CONTRIBUTING.md](../../CONTRIBUTING.md).

For each release:

1. Set the stable `X.Y.Z` version in the workspace `Cargo.toml` and refresh
   `Cargo.lock` with `cargo check --workspace`.
2. Move `CHANGELOG.md`'s `Unreleased` entries into a dated `X.Y.Z — YYYY-MM-DD`
   section, leaving an empty `Unreleased` section for future changes. Group concise,
   user-visible changes under Added, Changed, Fixed, or Removed; omit empty groups.
   Include Downloads with platform requirements and known limitations, and a full
   comparison link from the previous tag. Verify entries against the commit range.
3. Commit and push the release changes to `main`. The command requires a clean
   checkout matching remote `main`. It does not commit, push, or bump versions.
4. Extract that version's section verbatim as the release notes, excluding its
   version heading. This keeps the changelog and GitHub notes consistent. For example:

```sh
awk '/^## 0[.]4[.]0 — / { found=1; next } found && /^## / { exit } found { print }' CHANGELOG.md > /tmp/ganymede-release-notes.md
```

5. Run:

```sh
export GANYMEDE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export GANYMEDE_NOTARY_PROFILE="ganymede-notary"
python3 scripts/release-macos.py --notes /path/to/release-notes.md --check
python3 scripts/release-macos.py --notes /path/to/release-notes.md
```

`just release-macos /path/to/release-notes.md` is an equivalent shortcut for the
second command. Set `GANYMEDE_DMG_LAYOUT=1` for the arranged Finder layout on a
logged-in desktop. Do not run other packaging commands in the same checkout
while a release is building.

The command fetches pinned assets, runs formatting, Clippy, workspace tests, and
an `unfinished` feature compile check, then builds with unfinished features disabled.
It signs and notarizes both app and DMG, validates the bundle version and identifier,
checks architecture and Gatekeeper acceptance, and verifies an extracted ZIP.
Artifacts are staged in `target/dist/vX.Y.Z/` with versioned filenames and
`SHA256SUMS.txt`. It uploads them to a **draft** release targeting the exact source
commit and checks uploaded asset sizes and SHA-256 digests.

The command never publishes. Review the draft's notes and downloads, perform a
launch/upgrade smoke test, and publish through GitHub when ready. Full interactive
upgrade testing and live Jupyter integration tests remain separate checks.

## Failure recovery

A failed check, build, or notarization stops before upload. A failed upload leaves
an unpublished draft; do not publish it until all three artifacts verify.

Fix local credentials or transient network problems and rerun the same command.
It rebuilds and reverifies artifacts, then replaces assets only in a draft targeting
that exact commit. It refuses published versions and mismatched release tags or
draft commits. If source changes are needed after creating a draft, deliberately
remove the abandoned draft (and any conflicting tag after inspection), or choose a
new version. The script never deletes releases or moves existing tags.

Use a fresh version for changes to a published release. Never replace existing
public binaries in place. No certificates or passwords are written into the
repository or uploaded to GitHub by this workflow.

Run the release automation regression checks with:

```sh
python3 -m unittest discover -s scripts/tests -v
```

## Individual packaging commands

The commands below remain available for previews and troubleshooting; the release
command above coordinates them for public distribution.

## Checks and preview builds

On pushes and pull requests, GitHub Actions runs formatting, release automation
tests, and full workspace Clippy and tests (including the GUI) on Linux. Tag pushes additionally
run full workspace Clippy and tests and check the `unfinished` feature on macOS.
To run those macOS jobs before tagging, dispatch CI manually with **full** enabled.
Tags and full manual runs also build [Linux packages](linux.md).
Live Jupyter tests remain a separate opt-in check via `scripts/test_jupyter.py`.
Rust is pinned in `rust-toolchain.toml`.

After the full macOS checks pass, CI builds an optimized macOS app with unfinished features
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
