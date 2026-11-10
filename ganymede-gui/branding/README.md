# Application icon

User-provided PlanetRise icon set. These are committed branding assets, separate
from the downloaded fonts and UI icons in `assets/`.

- `AppIcon.icns`: macOS app icon, including multiple resolutions. Copied into the
  app bundle by `scripts/package-macos.sh`.
- `icon-macos.png`: 1024px macOS artwork with rounded corners and transparent padding.
- `icon-source.png`: original 1024px square artwork, installed as the Linux desktop icon
  in `hicolor/512x512@2/apps` (512 logical pixels at scale 2).
- `icon.ico`: Windows icon, retained for future Windows packaging.
