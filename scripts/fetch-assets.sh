#!/usr/bin/env bash
set -euo pipefail

# Pin upstream releases so every checkout uses the same assets.
inter_version=4.1
jetbrains_version=2.304
phosphor_version=2.0.8
geist_version=1.7.0

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
assets="$repo_root/ganymede-gui/assets"
temp_dir="$(mktemp -d)"
trap 'rm -rf "$temp_dir"' EXIT

download() {
    curl --fail --location --silent --show-error --retry 3 "$1" --output "$2"
}

echo "Downloading Inter $inter_version, JetBrains Mono $jetbrains_version, and Phosphor ${phosphor_version}..."
download "https://github.com/rsms/inter/releases/download/v$inter_version/Inter-$inter_version.zip" "$temp_dir/inter.zip"
download "https://github.com/JetBrains/JetBrainsMono/releases/download/v$jetbrains_version/JetBrainsMono-$jetbrains_version.zip" "$temp_dir/jetbrains.zip"
download "https://codeload.github.com/phosphor-icons/core/tar.gz/refs/tags/v$phosphor_version" "$temp_dir/phosphor.tar.gz"

# Extract only desktop fonts, SVG icons, and their licenses before installing.
mkdir -p "$temp_dir/inter" "$temp_dir/jetbrains-mono" "$temp_dir/phosphor"
unzip -q -j "$temp_dir/inter.zip" 'extras/ttf/*.ttf' LICENSE.txt -d "$temp_dir/inter"
unzip -q -j "$temp_dir/jetbrains.zip" 'fonts/ttf/*.ttf' OFL.txt -d "$temp_dir/jetbrains-mono"
tar -xzf "$temp_dir/phosphor.tar.gz" -C "$temp_dir/phosphor" --strip-components=1 \
    "core-$phosphor_version/assets" "core-$phosphor_version/LICENSE"

mkdir -p "$assets/fonts/inter" "$assets/fonts/jetbrains-mono" "$assets/icons/phosphor"
cp -R "$temp_dir/inter/." "$assets/fonts/inter/"
cp -R "$temp_dir/jetbrains-mono/." "$assets/fonts/jetbrains-mono/"
cp -R "$temp_dir/phosphor/assets/." "$assets/icons/phosphor/"
cp "$temp_dir/phosphor/LICENSE" "$assets/icons/phosphor/LICENSE"
# These additions postdate the pinned release above.
phosphor_additions_revision=2b75f3ad12b420c9504ef05df8d2564a28f8500e
for icon in rows-plus-top rows-plus-bottom; do
    download "https://raw.githubusercontent.com/phosphor-icons/core/$phosphor_additions_revision/assets/regular/$icon.svg" "$assets/icons/phosphor/regular/$icon.svg"
done
mkdir -p "$assets/fonts/geist-pixel"
download "https://raw.githubusercontent.com/vercel/geist-font/geist%40$geist_version/fonts/GeistPixel/ttf/GeistPixel-Square.ttf" "$assets/fonts/geist-pixel/GeistPixel-Square.ttf"
download "https://raw.githubusercontent.com/vercel/geist-font/geist%40$geist_version/OFL.txt" "$assets/fonts/geist-pixel/OFL.txt"
echo "Assets ready in $assets"
