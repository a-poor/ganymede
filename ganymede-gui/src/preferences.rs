//! Small, app-wide preferences. Only explicit settings changes write to disk.
use crate::{autosave::AutosavePreference, theme::AppearancePreference};
use gpui::App;
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Preferences {
    version: u32,
    appearance: AppearancePreference,
    autosave: bool,
    editor: EditorPreferences,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            appearance: AppearancePreference::default(),
            autosave: true,
            editor: EditorPreferences::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Indentation {
    Two,
    #[default]
    Four,
    Eight,
}
impl Indentation {
    pub fn width(self) -> usize {
        match self {
            Self::Two => 2,
            Self::Four => 4,
            Self::Eight => 8,
        }
    }
    pub fn text(self) -> &'static str {
        match self {
            Self::Two => "  ",
            Self::Four => "    ",
            Self::Eight => "        ",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorPreferences {
    pub indentation: Indentation,
    pub line_numbers: bool,
    pub font_size: u8,
    pub console_enter: bool,
    pub auto_close_brackets: bool,
}
impl Default for EditorPreferences {
    fn default() -> Self {
        Self {
            indentation: Indentation::Four,
            line_numbers: false,
            font_size: 12,
            console_enter: false,
            auto_close_brackets: true,
        }
    }
}
impl gpui::Global for EditorPreferences {}
pub fn editor(cx: &App) -> EditorPreferences {
    cx.try_global::<EditorPreferences>()
        .cloned()
        .unwrap_or_default()
}

#[derive(Default)]
pub struct Persistence {
    pub error: Option<String>,
}
impl gpui::Global for Persistence {}

fn path() -> PathBuf {
    std::env::var_os("GANYMEDE_PREFERENCES_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::connection::history_path().with_extension("preferences.json"))
}

fn read(path: &Path) -> anyhow::Result<Preferences> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Preferences::default());
        }
        Err(error) => return Err(error.into()),
    };
    let preferences: Preferences = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(preferences.version == 1, "Unsupported preferences version");
    Ok(preferences)
}

fn write(path: &Path, preferences: &Preferences) -> anyhow::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> anyhow::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(preferences)?)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

pub fn init(cx: &mut App) {
    let (preferences, error) = match read(&path()) {
        Ok(preferences) => (preferences, None),
        Err(error) => (
            Preferences::default(),
            Some(format!("Could not load preferences: {error}")),
        ),
    };
    preferences.appearance.apply(cx.window_appearance(), cx);
    cx.set_global(AutosavePreference(preferences.autosave));
    let mut editor = preferences.editor;
    editor.font_size = editor.font_size.clamp(10, 24);
    cx.set_global(editor);
    cx.set_global(Persistence { error });
}

/// Called only by settings actions, never by system appearance notifications.
pub fn save(cx: &mut App) {
    let preferences = Preferences {
        appearance: *cx.global::<AppearancePreference>(),
        autosave: cx.global::<AutosavePreference>().0,
        editor: editor(cx),
        ..Preferences::default()
    };
    let error = write(&path(), &preferences)
        .err()
        .map(|error| format!("Could not save preferences: {error}"));
    cx.set_global(Persistence { error });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_partial_preferences_keep_defaults() {
        let path = std::env::temp_dir().join(format!("ganymede-missing-{}", uuid::Uuid::new_v4()));
        assert_eq!(read(&path).unwrap(), Preferences::default());
        let preferences: Preferences = serde_json::from_str(r#"{"autosave":false}"#).unwrap();
        assert!(!preferences.autosave);
        assert_eq!(preferences.appearance, AppearancePreference::Light);
    }

    #[test]
    fn settings_survive_replacement_and_invalid_files_are_preserved() {
        let directory =
            std::env::temp_dir().join(format!("ganymede-preferences-{}", uuid::Uuid::new_v4()));
        let path = directory.join("preferences.json");
        for appearance in [
            AppearancePreference::Dark,
            AppearancePreference::System,
            AppearancePreference::Light,
        ] {
            let expected = Preferences {
                appearance,
                autosave: false,
                editor: EditorPreferences {
                    font_size: 18,
                    line_numbers: true,
                    indentation: Indentation::Two,
                    console_enter: true,
                    auto_close_brackets: false,
                },
                ..Preferences::default()
            };
            write(&path, &expected).unwrap();
            assert_eq!(read(&path).unwrap(), expected);
        }
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        for invalid in ["not json", r#"{"version":99}"#] {
            std::fs::write(&path, invalid).unwrap();
            assert!(read(&path).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), invalid);
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn failed_write_reports_error_without_leaving_temporary_files() {
        let directory =
            std::env::temp_dir().join(format!("ganymede-preferences-{}", uuid::Uuid::new_v4()));
        let path = directory.join("preferences.json");
        std::fs::create_dir_all(&path).unwrap();
        assert!(write(&path, &Preferences::default()).is_err());
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        assert!(path.is_dir());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
