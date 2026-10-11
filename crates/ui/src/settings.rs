//! Per-user settings (never stored in the shared project).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::i18n::Lang;
use crate::layout::LayoutMode;
use crate::shortcuts::Preset;
use crate::theme::Theme;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub user_name: String,
    pub lang: Lang,
    pub theme: Theme,
    pub layout: LayoutMode,
    pub shortcut_preset: Preset,
    /// 0 = full, 1 = half, 2 = quarter.
    pub preview_quality: u8,
    pub relay_url: String,
    pub recent_projects: Vec<PathBuf>,
    pub snapping: bool,
    pub magnetic: bool,
    pub safe_zones: bool,
    pub use_audio_device: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            user_name: std::env::var("USER")
                .or_else(|_| std::env::var("USERNAME"))
                .unwrap_or_else(|_| "Editor".into()),
            lang: Lang::from_env(),
            theme: Theme::Dark,
            layout: LayoutMode::Default,
            shortcut_preset: Preset::Default,
            preview_quality: 1,
            relay_url: "ws://127.0.0.1:7878/ws".into(),
            recent_projects: Vec::new(),
            snapping: true,
            magnetic: false,
            safe_zones: false,
            use_audio_device: true,
        }
    }
}

/// Where per-user files live. `CUTROOM_CONFIG_DIR` overrides (tests, demos
/// with two clients on one machine).
pub fn config_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CUTROOM_CONFIG_DIR") {
        return d.into();
    }
    directories::ProjectDirs::from("app", "Cutroom", "Cutroom")
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".cutroom-config"))
}

/// Default folder for new projects.
pub fn projects_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|u| u.document_dir().map(|d| d.join("Cutroom")))
        .unwrap_or_else(|| config_dir().join("projects"))
}

impl Settings {
    fn path(dir: &Path) -> PathBuf {
        dir.join("settings.toml")
    }

    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(Self::path(dir))
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let s = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(Self::path(dir), s)
    }

    pub fn push_recent(&mut self, p: &Path) {
        self.recent_projects.retain(|r| r != p);
        self.recent_projects.insert(0, p.to_path_buf());
        self.recent_projects.truncate(10);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let d = tempfile::tempdir().unwrap();
        let mut s = Settings {
            user_name: "Ana".into(),
            lang: Lang::De,
            ..Default::default()
        };
        s.push_recent(Path::new("/a"));
        s.push_recent(Path::new("/b"));
        s.push_recent(Path::new("/a"));
        s.save(d.path()).unwrap();
        let l = Settings::load(d.path());
        assert_eq!(l.user_name, "Ana");
        assert_eq!(l.lang, Lang::De);
        assert_eq!(
            l.recent_projects,
            vec![PathBuf::from("/a"), PathBuf::from("/b")]
        );
    }
}
