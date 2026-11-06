# Third-party acknowledgments

Ganymede's own code is dual-licensed under MIT OR Apache-2.0. Dependencies and
bundled third-party assets retain their upstream licenses.

## Fonts and icons

| Asset | Upstream | License | Downloaded license file |
| --- | --- | --- | --- |
| Inter 4.1 | https://github.com/rsms/inter | SIL OFL 1.1 | `ganymede-gui/assets/fonts/inter/LICENSE.txt` |
| JetBrains Mono 2.304 | https://github.com/JetBrains/JetBrainsMono | SIL OFL 1.1 | `ganymede-gui/assets/fonts/jetbrains-mono/OFL.txt` |
| Geist Pixel 1.7.0 | https://github.com/vercel/geist-font | SIL OFL 1.1 | `ganymede-gui/assets/fonts/geist-pixel/OFL.txt` |
| Phosphor 2.0.8, plus pinned additions | https://github.com/phosphor-icons/core | MIT | `ganymede-gui/assets/icons/phosphor/LICENSE` |

`scripts/fetch-assets.sh` downloads these assets and their license texts. The fonts,
selected icons, and corresponding license texts are embedded in the application.
Downloaded files are not tracked in this repository. The PlanetRise application
icon is maintained separately in `ganymede-gui/branding/`.

## Rust dependencies

Ganymede uses [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui)
and other Rust libraries. `Cargo.lock` records the exact dependency versions and
Git revisions. Their manifests and source distributions contain the applicable
license notices. This document is an asset acknowledgment, not an exhaustive
inventory of transitive dependency licenses.
