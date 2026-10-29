//! Assets are embedded in both debug and release binaries.

use std::borrow::Cow;

use anyhow::{Context as _, Result};
use gpui::{App, AssetSource, SharedString};

use crate::icon::IconAssets;

// Keep this explicit: extra downloaded weights and Inter Display are not shipped.
#[derive(rust_embed::RustEmbed)]
#[folder = "assets/fonts/"]
#[prefix = "fonts/"]
#[include = "inter/Inter-Regular.ttf"]
#[include = "inter/Inter-Italic.ttf"]
#[include = "inter/Inter-SemiBold.ttf"]
#[include = "inter/Inter-Bold.ttf"]
#[include = "inter/LICENSE.txt"]
#[include = "jetbrains-mono/JetBrainsMono-Regular.ttf"]
#[include = "jetbrains-mono/JetBrainsMono-Italic.ttf"]
#[include = "jetbrains-mono/JetBrainsMono-Bold.ttf"]
#[include = "jetbrains-mono/JetBrainsMono-BoldItalic.ttf"]
#[include = "jetbrains-mono/OFL.txt"]
struct FontAssets;

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(FontAssets::get(path)
            .or_else(|| IconAssets::get(path))
            .map(|file| file.data))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let directory = path.trim_end_matches('/');
        let prefix = format!("{directory}/");
        Ok(FontAssets::iter()
            .chain(IconAssets::iter())
            .filter(|name| directory.is_empty() || name.starts_with(&prefix))
            .map(|name| SharedString::from(name.into_owned()))
            .collect())
    }
}

pub fn register_fonts(cx: &App) -> Result<()> {
    let fonts = FontAssets::iter()
        .filter(|path| path.ends_with(".ttf"))
        .map(|path| {
            FontAssets::get(&path)
                .map(|file| file.data)
                .with_context(|| format!("missing embedded font: {path}"))
        })
        .collect::<Result<Vec<_>>>()?;
    anyhow::ensure!(
        fonts.len() == 8,
        "missing fonts; run `just fetch-assets` and rebuild"
    );
    cx.text_system().add_fonts(fonts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icon::Icon;

    #[test]
    fn bundle_contains_only_declared_icons_and_preserves_licenses() {
        let icons = Assets.list("icons/phosphor").unwrap();
        assert_eq!(icons.len(), Icon::ALL.len() + 1);
        for icon in Icon::ALL {
            let bytes = Assets.load(icon.path()).unwrap().unwrap();
            assert!(matches!(bytes, Cow::Borrowed(_)));
            assert!(std::str::from_utf8(&bytes).unwrap().contains("<svg"));
        }
        assert!(
            Assets
                .load("icons/phosphor/regular/play.svg")
                .unwrap()
                .is_none()
        );
        assert!(Assets.list("icon").unwrap().is_empty());
        for path in [
            "icons/phosphor/LICENSE",
            "fonts/inter/LICENSE.txt",
            "fonts/jetbrains-mono/OFL.txt",
        ] {
            assert!(!Assets.load(path).unwrap().unwrap().is_empty());
        }
    }

    #[test]
    fn desktop_fonts_are_embedded_without_unused_families() {
        let fonts: Vec<_> = Assets
            .list("fonts/")
            .unwrap()
            .into_iter()
            .filter(|path| path.ends_with(".ttf"))
            .collect();
        assert_eq!(fonts.len(), 8);
        for path in fonts {
            let bytes = Assets.load(&path).unwrap().unwrap();
            assert!(matches!(bytes, Cow::Borrowed(_)));
            assert_eq!(&bytes[..4], &[0, 1, 0, 0]);
        }
        assert!(
            Assets
                .load("fonts/inter/InterDisplay-Regular.ttf")
                .unwrap()
                .is_none()
        );
    }
}
