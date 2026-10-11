//! Dock layouts. Persisted per user and per layout mode; never in the doc.

use std::path::{Path, PathBuf};

use egui_dock::{DockState, NodeIndex};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tab {
    Media,
    Player,
    /// Source viewer (Dual layout).
    Source,
    Properties,
    Timeline,
}

impl Tab {
    pub fn i18n_key(self) -> &'static str {
        match self {
            Tab::Media => "panel-media",
            Tab::Player => "panel-player",
            Tab::Source => "panel-source",
            Tab::Properties => "panel-properties",
            Tab::Timeline => "panel-timeline",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum LayoutMode {
    #[default]
    Default,
    Organize,
    Edit,
    ShortVideo,
    Classic,
    Dual,
    Timeline,
    ColorGrading,
    AudioMixing,
}

impl LayoutMode {
    pub const ALL: [LayoutMode; 9] = [
        LayoutMode::Default,
        LayoutMode::Organize,
        LayoutMode::Edit,
        LayoutMode::ShortVideo,
        LayoutMode::Classic,
        LayoutMode::Dual,
        LayoutMode::Timeline,
        LayoutMode::ColorGrading,
        LayoutMode::AudioMixing,
    ];

    pub fn i18n_key(self) -> &'static str {
        match self {
            LayoutMode::Default => "layout-default",
            LayoutMode::Organize => "layout-organize",
            LayoutMode::Edit => "layout-edit",
            LayoutMode::ShortVideo => "layout-short",
            LayoutMode::Classic => "layout-classic",
            LayoutMode::Dual => "layout-dual",
            LayoutMode::Timeline => "layout-timeline",
            LayoutMode::ColorGrading => "layout-color",
            LayoutMode::AudioMixing => "layout-audio",
        }
    }

    fn file(self) -> String {
        format!("layout-{self:?}.bin").to_lowercase()
    }

    /// The factory arrangement for this mode.
    pub fn build(self) -> DockState<Tab> {
        use Tab::*;
        let root = NodeIndex::root();
        match self {
            LayoutMode::Default | LayoutMode::AudioMixing | LayoutMode::ShortVideo => {
                let (top_frac, left, mid) = match self {
                    LayoutMode::AudioMixing => (0.45, 0.35, 0.6),
                    LayoutMode::ShortVideo => (0.62, 0.38, 0.45),
                    _ => (0.6, 0.35, 0.615),
                };
                let mut d = DockState::new(vec![Media]);
                let s = d.main_surface_mut();
                let [top, _] = s.split_below(root, top_frac, vec![Timeline]);
                let [_, player] = s.split_right(top, left, vec![Player]);
                s.split_right(player, mid, vec![Properties]);
                d
            }
            LayoutMode::Organize => {
                let mut d = DockState::new(vec![Media]);
                let s = d.main_surface_mut();
                let [top, _] = s.split_below(root, 0.72, vec![Timeline]);
                let [_, right] = s.split_right(top, 0.6, vec![Player]);
                s.split_below(right, 0.55, vec![Properties]);
                d
            }
            LayoutMode::Edit => {
                let mut d = DockState::new(vec![Media]);
                let s = d.main_surface_mut();
                let [top, _] = s.split_below(root, 0.45, vec![Timeline]);
                let [_, player] = s.split_right(top, 0.3, vec![Player]);
                s.split_right(player, 0.65, vec![Properties]);
                d
            }
            LayoutMode::Classic => {
                let mut d = DockState::new(vec![Media, Properties]);
                let s = d.main_surface_mut();
                let [top, _] = s.split_below(root, 0.58, vec![Timeline]);
                s.split_right(top, 0.45, vec![Player]);
                d
            }
            LayoutMode::Dual => {
                let mut d = DockState::new(vec![Media]);
                let s = d.main_surface_mut();
                let [top, _] = s.split_below(root, 0.6, vec![Timeline]);
                let [_, src] = s.split_right(top, 0.25, vec![Source]);
                let [_, prog] = s.split_right(src, 0.36, vec![Player]);
                s.split_right(prog, 0.6, vec![Properties]);
                d
            }
            LayoutMode::Timeline => {
                let mut d = DockState::new(vec![Player]);
                let s = d.main_surface_mut();
                let [top, _] = s.split_below(root, 0.42, vec![Timeline]);
                s.split_right(top, 0.6, vec![Properties, Media]);
                d
            }
            LayoutMode::ColorGrading => {
                let mut d = DockState::new(vec![Player]);
                let s = d.main_surface_mut();
                let [top, _] = s.split_below(root, 0.68, vec![Timeline]);
                s.split_right(top, 0.68, vec![Properties, Media]);
                d
            }
        }
    }

    fn path(self, dir: &Path) -> PathBuf {
        dir.join(self.file())
    }

    /// The user's saved arrangement for this mode, or the factory one.
    pub fn load(self, dir: &Path) -> DockState<Tab> {
        // Binary, because dock rects contain infinities JSON can't encode.
        std::fs::read(self.path(dir))
            .ok()
            .and_then(|b| postcard::from_bytes::<DockState<Tab>>(&b).ok())
            .filter(|d| d.iter_all_tabs().any(|(_, t)| *t == Tab::Timeline))
            .unwrap_or_else(|| self.build())
    }

    pub fn save(self, dir: &Path, dock: &DockState<Tab>) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let b = postcard::to_stdvec(dock).map_err(std::io::Error::other)?;
        std::fs::write(self.path(dir), b)
    }

    pub fn reset(self, dir: &Path) {
        let _ = std::fs::remove_file(self.path(dir));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_has_core_panels() {
        for m in LayoutMode::ALL {
            let d = m.build();
            let tabs: Vec<Tab> = d.iter_all_tabs().map(|(_, t)| *t).collect();
            for t in [Tab::Media, Tab::Player, Tab::Properties, Tab::Timeline] {
                assert!(tabs.contains(&t), "{m:?} lacks {t:?}");
            }
        }
        assert!(
            LayoutMode::Dual
                .build()
                .iter_all_tabs()
                .any(|(_, t)| *t == Tab::Source)
        );
    }

    #[test]
    fn layouts_persist() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = LayoutMode::Edit.build();
        d.main_surface_mut().push_to_first_leaf(Tab::Source);
        LayoutMode::Edit.save(dir.path(), &d).unwrap();
        let l = LayoutMode::Edit.load(dir.path());
        assert!(l.iter_all_tabs().any(|(_, t)| *t == Tab::Source));
        LayoutMode::Edit.reset(dir.path());
        assert!(
            !LayoutMode::Edit
                .load(dir.path())
                .iter_all_tabs()
                .any(|(_, t)| *t == Tab::Source)
        );
    }
}
