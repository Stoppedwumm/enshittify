//! Keyboard shortcuts: actions, chords, contexts, presets, conflict detection.
//!
//! Defaults live in `assets/shortcuts.toml` (one table per [`Action`], with
//! separate Windows/Linux and macOS bindings). Presets are override files
//! layered on top. User rebinding is stored per user (settings dir), never in
//! the shared project document.

use std::collections::BTreeMap;
use std::fmt;

use egui::{Key, Modifiers};
use serde::{Deserialize, Serialize};

macro_rules! actions {
    ($($variant:ident => $id:literal, $label:literal;)*) => {
        /// Every bindable command.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum Action { $($variant,)* }

        impl Action {
            pub const ALL: &'static [Action] = &[$(Action::$variant,)*];

            /// Stable identifier used in TOML files.
            pub fn id(self) -> &'static str {
                match self { $(Action::$variant => $id,)* }
            }

            /// English label (fallback when no translation exists).
            pub fn label(self) -> &'static str {
                match self { $(Action::$variant => $label,)* }
            }

            pub fn from_id(id: &str) -> Option<Action> {
                match id { $($id => Some(Action::$variant),)* _ => None }
            }
        }
    };
}

actions! {
    NewProject => "new_project", "New project";
    OpenProject => "open_project", "Open project";
    SaveProject => "save_project", "Save project";
    SaveProjectAs => "save_project_as", "Save project as";
    ArchiveProject => "archive_project", "Archive project";
    ImportMedia => "import_media", "Import media";
    RecordVoiceOver => "record_voice_over", "Record voice-over";
    NewMediaFolder => "new_media_folder", "New media folder";
    Preferences => "preferences", "Preferences";
    ShortcutEditor => "shortcut_editor", "Keyboard shortcuts";
    Export => "export", "Export";
    Undo => "undo", "Undo";
    Redo => "redo", "Redo";
    Cut => "cut", "Cut";
    Copy => "copy", "Copy";
    Paste => "paste", "Paste";
    Duplicate => "duplicate", "Duplicate";
    Delete => "delete", "Delete";
    RippleDelete => "ripple_delete", "Ripple delete";
    CloseGap => "close_gap", "Close gap";
    SelectAll => "select_all", "Select all";
    CopyEffects => "copy_effects", "Copy effects";
    PasteEffects => "paste_effects", "Paste effects";
    SelectClipRange => "select_clip_range", "Select clip range";
    CancelClipRange => "cancel_clip_range", "Cancel clip range";
    NudgeLeft => "nudge_left", "Nudge left";
    NudgeRight => "nudge_right", "Nudge right";
    NudgeUp => "nudge_up", "Nudge up";
    NudgeDown => "nudge_down", "Nudge down";
    SplitAtPlayhead => "split_at_playhead", "Split at playhead";
    TrimStartToPlayhead => "trim_start_to_playhead", "Trim start to playhead";
    TrimEndToPlayhead => "trim_end_to_playhead", "Trim end to playhead";
    EditProperties => "edit_properties", "Edit properties";
    CropAndZoom => "crop_and_zoom", "Crop & zoom";
    CropToFit => "crop_to_fit", "Crop to fit";
    RotateCw => "rotate_cw", "Rotate 90° clockwise";
    RotateCcw => "rotate_ccw", "Rotate 90° counter-clockwise";
    Group => "group", "Group";
    Ungroup => "ungroup", "Ungroup";
    SpeedDialog => "speed_dialog", "Speed";
    FreezeFrame => "freeze_frame", "Freeze frame";
    DetachAudio => "detach_audio", "Detach audio";
    MuteClip => "mute_clip", "Mute clip";
    Stabilization => "stabilization", "Stabilization";
    ColorCorrection => "color_correction", "Color correction";
    ChromaKey => "chroma_key", "Chroma key";
    MotionTracking => "motion_tracking", "Motion tracking";
    ColorMatch => "color_match", "Color match";
    KeyframePanel => "keyframe_panel", "Keyframe panel";
    AddKeyframe => "add_keyframe", "Add keyframe";
    PrevKeyframe => "prev_keyframe", "Previous keyframe";
    NextKeyframe => "next_keyframe", "Next keyframe";
    InsertEdit => "insert_edit", "Insert edit";
    OverwriteEdit => "overwrite_edit", "Overwrite edit";
    SelectTool => "select_tool", "Select tool";
    BladeTool => "blade_tool", "Blade tool";
    Rename => "rename", "Rename";
    RevealInFolder => "reveal_in_folder", "Reveal in folder";
    ApplySelectedAsset => "apply_selected_asset", "Apply selected asset";
    RenderPreview => "render_preview", "Render preview";
    PlayPause => "play_pause", "Play / pause";
    Stop => "stop", "Stop";
    FullscreenPreview => "fullscreen_preview", "Fullscreen preview";
    PrevFrame => "prev_frame", "Previous frame";
    NextFrame => "next_frame", "Next frame";
    PrevEditPoint => "prev_edit_point", "Previous edit point";
    NextEditPoint => "next_edit_point", "Next edit point";
    BackOneSecond => "back_one_second", "Back 1 second";
    ForwardOneSecond => "forward_one_second", "Forward 1 second";
    PrevMarker => "prev_marker", "Previous marker";
    NextMarker => "next_marker", "Next marker";
    ProjectStart => "project_start", "Go to project start";
    ProjectEnd => "project_end", "Go to project end";
    ClipStart => "clip_start", "Go to clip start";
    ClipEnd => "clip_end", "Go to clip end";
    PrevClip => "prev_clip", "Previous clip";
    NextClip => "next_clip", "Next clip";
    Snapshot => "snapshot", "Snapshot";
    ZoomIn => "zoom_in", "Zoom in";
    ZoomOut => "zoom_out", "Zoom out";
    ZoomToFit => "zoom_to_fit", "Zoom to fit";
    MarkIn => "mark_in", "Mark in";
    MarkOut => "mark_out", "Mark out";
    ClearInOut => "clear_in_out", "Clear in & out";
    AddMarker => "add_marker", "Add marker";
    EditMarker => "edit_marker", "Edit marker";
    MarkerColor1 => "marker_color_1", "Marker color 1";
    MarkerColor2 => "marker_color_2", "Marker color 2";
    MarkerColor3 => "marker_color_3", "Marker color 3";
    MarkerColor4 => "marker_color_4", "Marker color 4";
    MarkerColor5 => "marker_color_5", "Marker color 5";
    MarkerColor6 => "marker_color_6", "Marker color 6";
    MarkerColor7 => "marker_color_7", "Marker color 7";
    MarkerColor8 => "marker_color_8", "Marker color 8";
    ShuttleReverse => "shuttle_reverse", "Shuttle reverse (J)";
    ShuttleStop => "shuttle_stop", "Shuttle stop (K)";
    ShuttleForward => "shuttle_forward", "Shuttle forward (L)";
    ToggleSnapping => "toggle_snapping", "Toggle snapping";
    FollowCollaborator => "follow_collaborator", "Follow collaborator";
    JumpToCollaborator => "jump_to_collaborator", "Jump to collaborator";
}

impl Action {
    /// Actions that keep firing while a key is held (auto-repeat).
    pub fn repeats(self) -> bool {
        matches!(
            self,
            Action::PrevFrame
                | Action::NextFrame
                | Action::BackOneSecond
                | Action::ForwardOneSecond
                | Action::NudgeLeft
                | Action::NudgeRight
                | Action::NudgeUp
                | Action::NudgeDown
                | Action::ZoomIn
                | Action::ZoomOut
        )
    }

    pub fn marker_color_index(self) -> Option<usize> {
        use Action::*;
        [
            MarkerColor1,
            MarkerColor2,
            MarkerColor3,
            MarkerColor4,
            MarkerColor5,
            MarkerColor6,
            MarkerColor7,
            MarkerColor8,
        ]
        .iter()
        .position(|a| *a == self)
    }
}

/// Where a binding is active.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Global,
    Timeline,
    Media,
    Player,
}

impl Scope {
    /// Whether bindings in `self` and `other` can fire in the same situation.
    pub fn overlaps(self, other: Scope) -> bool {
        self == other || self == Scope::Global || other == Scope::Global
    }

    pub fn label(self) -> &'static str {
        match self {
            Scope::Global => "Global",
            Scope::Timeline => "Timeline",
            Scope::Media => "Media",
            Scope::Player => "Player",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// Windows and Linux share the "windows" column.
    Windows,
    MacOs,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Windows
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Trigger {
    Key(Key),
    /// Primary mouse click (e.g. Alt+Click on a property to add a keyframe).
    Click,
}

/// A key (or click) plus exact modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Chord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// macOS Command key.
    pub cmd: bool,
    pub trigger: Trigger,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("cannot parse shortcut {0:?}")]
pub struct ChordParseError(pub String);

impl Chord {
    pub fn key(key: Key) -> Self {
        Chord {
            ctrl: false,
            alt: false,
            shift: false,
            cmd: false,
            trigger: Trigger::Key(key),
        }
    }

    pub fn parse(s: &str) -> Result<Chord, ChordParseError> {
        let err = || ChordParseError(s.to_string());
        let s = s.trim();
        // Split on '+' but allow the key itself to be '+'… not used; keep simple.
        let mut parts: Vec<&str> = s.split('+').collect();
        if s.ends_with("++") {
            parts.pop();
            *parts.last_mut().ok_or_else(err)? = "+";
        }
        let key = parts.pop().ok_or_else(err)?.trim();
        let mut c = Chord {
            ctrl: false,
            alt: false,
            shift: false,
            cmd: false,
            trigger: Trigger::Click,
        };
        for m in parts {
            match m.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" => c.ctrl = true,
                "alt" | "option" | "opt" => c.alt = true,
                "shift" => c.shift = true,
                "cmd" | "command" | "super" => c.cmd = true,
                _ => return Err(err()),
            }
        }
        c.trigger = match key {
            "Click" => Trigger::Click,
            "Del" => Trigger::Key(Key::Delete),
            "Return" => Trigger::Key(Key::Enter),
            "Esc" => Trigger::Key(Key::Escape),
            "\\" => Trigger::Key(Key::Backslash),
            k => Trigger::Key(Key::from_name(k).ok_or_else(err)?),
        };
        Ok(c)
    }

    fn key_name(&self, p: Platform) -> String {
        match self.trigger {
            Trigger::Click => "Click".into(),
            Trigger::Key(k) => match (k, p) {
                (Key::ArrowLeft, _) => "←".into(),
                (Key::ArrowRight, _) => "→".into(),
                (Key::ArrowUp, _) => "↑".into(),
                (Key::ArrowDown, _) => "↓".into(),
                (Key::Backspace, Platform::MacOs) => "Delete".into(),
                (Key::Delete, Platform::MacOs) => "Fwd Delete".into(),
                (Key::Delete, _) => "Del".into(),
                (Key::Enter, Platform::MacOs) => "Return".into(),
                _ => k.symbol_or_name().to_string(),
            },
        }
    }

    /// Human-readable form for the given platform.
    pub fn display(&self, p: Platform) -> String {
        let mut s = String::new();
        match p {
            Platform::MacOs => {
                if self.ctrl {
                    s.push('⌃');
                }
                if self.alt {
                    s.push('⌥');
                }
                if self.shift {
                    s.push('⇧');
                }
                if self.cmd {
                    s.push('⌘');
                }
                s.push_str(&self.key_name(p));
            }
            Platform::Windows => {
                for (on, name) in [
                    (self.ctrl, "Ctrl+"),
                    (self.alt, "Alt+"),
                    (self.shift, "Shift+"),
                ] {
                    if on {
                        s.push_str(name);
                    }
                }
                s.push_str(&self.key_name(p));
            }
        }
        s
    }

    /// Canonical TOML form.
    pub fn to_config_string(&self) -> String {
        let mut s = String::new();
        for (on, name) in [
            (self.ctrl, "Ctrl+"),
            (self.alt, "Alt+"),
            (self.shift, "Shift+"),
            (self.cmd, "Cmd+"),
        ] {
            if on {
                s.push_str(name);
            }
        }
        s.push_str(&match self.trigger {
            Trigger::Click => "Click".to_string(),
            Trigger::Key(k) => k.name().to_string(),
        });
        s
    }

    /// Whether an input event with these modifiers matches exactly.
    pub fn matches(&self, trigger: Trigger, m: Modifiers) -> bool {
        self.trigger == trigger
            && self.ctrl == m.ctrl
            && self.alt == m.alt
            && self.shift == m.shift
            && self.cmd == m.mac_cmd
    }

    /// The chord described by a key event.
    pub fn from_event(key: Key, m: Modifiers) -> Chord {
        Chord {
            ctrl: m.ctrl,
            alt: m.alt,
            shift: m.shift,
            cmd: m.mac_cmd,
            trigger: Trigger::Key(key),
        }
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display(Platform::current()))
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct Entry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<Scope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    windows: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    macos: Option<Vec<String>>,
}

#[derive(Debug, thiserror::Error)]
pub enum ShortcutError {
    #[error("toml: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("unknown action {0:?}")]
    UnknownAction(String),
    #[error(transparent)]
    Chord(#[from] ChordParseError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Preset {
    #[default]
    Default,
    Premiere,
    FinalCut,
}

impl Preset {
    pub const ALL: [Preset; 3] = [Preset::Default, Preset::Premiere, Preset::FinalCut];

    pub fn label(self) -> &'static str {
        match self {
            Preset::Default => "Cutroom (default)",
            Preset::Premiere => "Premiere Pro",
            Preset::FinalCut => "Final Cut Pro",
        }
    }

    fn overrides(self) -> &'static str {
        match self {
            Preset::Default => "",
            Preset::Premiere => include_str!("../assets/shortcuts.premiere.toml"),
            Preset::FinalCut => include_str!("../assets/shortcuts.finalcut.toml"),
        }
    }
}

pub const DEFAULTS_TOML: &str = include_str!("../assets/shortcuts.toml");

/// Two actions bound to the same chord in overlapping scopes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub chord: Chord,
    pub a: Action,
    pub b: Action,
}

/// The active key map for one platform.
#[derive(Clone, Debug)]
pub struct ShortcutMap {
    platform: Platform,
    scopes: BTreeMap<Action, Scope>,
    bindings: BTreeMap<Action, Vec<Chord>>,
}

fn parse_table(src: &str) -> Result<BTreeMap<String, Entry>, ShortcutError> {
    Ok(toml::from_str(src)?)
}

impl ShortcutMap {
    /// Built-in defaults for `platform`.
    pub fn defaults(platform: Platform) -> Self {
        let mut m = ShortcutMap {
            platform,
            scopes: BTreeMap::new(),
            bindings: BTreeMap::new(),
        };
        m.apply_toml(DEFAULTS_TOML)
            .expect("built-in shortcuts.toml is valid");
        m
    }

    pub fn with_preset(platform: Platform, preset: Preset) -> Self {
        let mut m = Self::defaults(platform);
        m.apply_toml(preset.overrides())
            .expect("built-in preset is valid");
        m
    }

    pub fn platform(&self) -> Platform {
        self.platform
    }

    /// Layers a TOML table of overrides on top of the current map.
    pub fn apply_toml(&mut self, src: &str) -> Result<(), ShortcutError> {
        for (id, e) in parse_table(src)? {
            let action =
                Action::from_id(&id).ok_or_else(|| ShortcutError::UnknownAction(id.clone()))?;
            if let Some(c) = e.context {
                self.scopes.insert(action, c);
            }
            let list = match self.platform {
                Platform::Windows => e.windows,
                Platform::MacOs => e.macos,
            };
            if let Some(list) = list {
                let chords = list
                    .iter()
                    .map(|s| Chord::parse(s))
                    .collect::<Result<Vec<_>, _>>()?;
                self.bindings.insert(action, chords);
            }
        }
        Ok(())
    }

    /// Serialises the differences from `base` (the user's overrides file).
    pub fn diff_toml(&self, base: &ShortcutMap) -> String {
        let mut out: BTreeMap<String, Entry> = BTreeMap::new();
        for a in Action::ALL {
            let mine = self.chords(*a);
            if mine != base.chords(*a) {
                let list: Vec<String> = mine.iter().map(|c| c.to_config_string()).collect();
                let e = out.entry(a.id().to_string()).or_default();
                match self.platform {
                    Platform::Windows => e.windows = Some(list),
                    Platform::MacOs => e.macos = Some(list),
                }
            }
        }
        toml::to_string(&out).unwrap_or_default()
    }

    pub fn scope(&self, a: Action) -> Scope {
        self.scopes.get(&a).copied().unwrap_or(Scope::Global)
    }

    pub fn chords(&self, a: Action) -> &[Chord] {
        self.bindings.get(&a).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Display string of the first binding (menus, tooltips).
    pub fn hint(&self, a: Action) -> String {
        self.chords(a)
            .first()
            .map(|c| c.display(self.platform))
            .unwrap_or_default()
    }

    pub fn set(&mut self, a: Action, chords: Vec<Chord>) {
        self.bindings.insert(a, chords);
    }

    /// Actions that would conflict if `chord` were bound to `a`.
    pub fn conflicts_with(&self, a: Action, chord: &Chord) -> Vec<Action> {
        let scope = self.scope(a);
        Action::ALL
            .iter()
            .copied()
            .filter(|b| *b != a && self.scope(*b).overlaps(scope))
            .filter(|b| self.chords(*b).contains(chord))
            .collect()
    }

    /// Binds `chord` to `a`, removing it from any conflicting actions.
    pub fn rebind_replacing(&mut self, a: Action, chord: Chord) -> Vec<Action> {
        let losers = self.conflicts_with(a, &chord);
        for l in &losers {
            if let Some(v) = self.bindings.get_mut(l) {
                v.retain(|c| *c != chord);
            }
        }
        self.bindings.insert(a, vec![chord]);
        losers
    }

    /// All conflicts in the map.
    pub fn conflicts(&self) -> Vec<Conflict> {
        let mut out = Vec::new();
        for (i, a) in Action::ALL.iter().enumerate() {
            for b in &Action::ALL[i + 1..] {
                if !self.scope(*a).overlaps(self.scope(*b)) {
                    continue;
                }
                for c in self.chords(*a) {
                    if self.chords(*b).contains(c) {
                        out.push(Conflict {
                            chord: *c,
                            a: *a,
                            b: *b,
                        });
                    }
                }
            }
        }
        out
    }

    /// Resolves a key press in `active` panel scope. Panel-scoped bindings win
    /// over global ones with the same chord.
    pub fn lookup(
        &self,
        trigger: Trigger,
        m: Modifiers,
        active: Scope,
        repeat: bool,
    ) -> Option<Action> {
        let mut global = None;
        for (a, chords) in &self.bindings {
            if repeat && !a.repeats() {
                continue;
            }
            if !chords.iter().any(|c| c.matches(trigger, m)) {
                continue;
            }
            match self.scope(*a) {
                Scope::Global => global = global.or(Some(*a)),
                s if s == active => return Some(*a),
                _ => {}
            }
        }
        global
    }

    /// Collects actions triggered by this frame's key events. Returns nothing
    /// while a text field has keyboard focus (text fields swallow keys).
    pub fn collect(&self, ctx: &egui::Context, active: Scope) -> Vec<Action> {
        if ctx.text_edit_focused() {
            return Vec::new();
        }
        ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        repeat,
                        modifiers,
                        ..
                    } => self.lookup(Trigger::Key(*key), *modifiers, active, *repeat),
                    // egui turns Ctrl/Cmd+C/X/V into clipboard events.
                    egui::Event::Copy => self.lookup(
                        Trigger::Key(Key::C),
                        clipboard_mods(self.platform),
                        active,
                        false,
                    ),
                    egui::Event::Cut => self.lookup(
                        Trigger::Key(Key::X),
                        clipboard_mods(self.platform),
                        active,
                        false,
                    ),
                    egui::Event::Paste(_) => self.lookup(
                        Trigger::Key(Key::V),
                        clipboard_mods(self.platform),
                        active,
                        false,
                    ),
                    _ => None,
                })
                .collect()
        })
    }

    /// Whether a click with `m` is bound to `a` (e.g. Alt+Click = add keyframe).
    pub fn click_matches(&self, a: Action, m: Modifiers) -> bool {
        self.chords(a).iter().any(|c| c.matches(Trigger::Click, m))
    }
}

fn clipboard_mods(p: Platform) -> Modifiers {
    match p {
        Platform::MacOs => Modifiers {
            mac_cmd: true,
            command: true,
            ..Default::default()
        },
        Platform::Windows => Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        },
    }
}

/// The modifiers egui reports for a chord on its platform (for tests and
/// for synthesising events).
pub fn modifiers_for(c: &Chord, p: Platform) -> Modifiers {
    Modifiers {
        alt: c.alt,
        ctrl: c.ctrl,
        shift: c.shift,
        mac_cmd: c.cmd,
        command: match p {
            Platform::MacOs => c.cmd,
            Platform::Windows => c.ctrl,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_parse_and_cover_every_action() {
        for p in [Platform::Windows, Platform::MacOs] {
            let m = ShortcutMap::defaults(p);
            for a in Action::ALL {
                // The spec table leaves Rename unbound on macOS ("—").
                if p == Platform::MacOs && *a == Action::Rename {
                    assert!(m.chords(*a).is_empty());
                    continue;
                }
                assert!(!m.chords(*a).is_empty(), "{a:?} unbound on {p:?}");
            }
        }
    }

    #[test]
    fn defaults_and_presets_have_no_conflicts() {
        for p in [Platform::Windows, Platform::MacOs] {
            for preset in Preset::ALL {
                let m = ShortcutMap::with_preset(p, preset);
                assert_eq!(m.conflicts(), vec![], "{preset:?} on {p:?}");
            }
        }
    }

    #[test]
    fn chord_parsing_and_display() {
        let c = Chord::parse("Ctrl+Shift+S").unwrap();
        assert!(c.ctrl && c.shift && !c.alt && c.trigger == Trigger::Key(Key::S));
        assert_eq!(c.display(Platform::Windows), "Ctrl+Shift+S");
        let m = Chord::parse("Shift+Cmd+Z").unwrap();
        assert_eq!(m.display(Platform::MacOs), "⇧⌘Z");
        assert_eq!(
            Chord::parse("Alt+[").unwrap().trigger,
            Trigger::Key(Key::OpenBracket)
        );
        assert_eq!(
            Chord::parse("Ctrl+=").unwrap().trigger,
            Trigger::Key(Key::Equals)
        );
        assert_eq!(
            Chord::parse("Ctrl+-").unwrap().trigger,
            Trigger::Key(Key::Minus)
        );
        assert_eq!(Chord::parse("Alt+Click").unwrap().trigger, Trigger::Click);
        assert!(Chord::parse("Hyper+Q").is_err());
        assert!(Chord::parse("Ctrl+Nope").is_err());
        let rt = Chord::parse(&c.to_config_string()).unwrap();
        assert_eq!(rt, c);
    }

    #[test]
    fn rebinding_detects_and_resolves_conflicts() {
        let mut m = ShortcutMap::defaults(Platform::Windows);
        let ctrl_s = Chord::parse("Ctrl+S").unwrap();
        assert_eq!(
            m.conflicts_with(Action::Export, &ctrl_s),
            vec![Action::SaveProject]
        );
        let losers = m.rebind_replacing(Action::Export, ctrl_s);
        assert_eq!(losers, vec![Action::SaveProject]);
        assert!(m.chords(Action::SaveProject).is_empty());
        assert!(m.conflicts().is_empty());
        // Scoped bindings don't conflict across disjoint panels.
        let f = Chord::parse("Ctrl+Shift+R").unwrap();
        assert!(m.conflicts_with(Action::RevealInFolder, &f).is_empty());
        // Overrides round-trip through the user file format.
        let base = ShortcutMap::defaults(Platform::Windows);
        let diff = m.diff_toml(&base);
        let mut again = ShortcutMap::defaults(Platform::Windows);
        again.apply_toml(&diff).unwrap();
        assert_eq!(again.chords(Action::Export), m.chords(Action::Export));
        assert!(again.chords(Action::SaveProject).is_empty());
    }

    #[test]
    fn panel_scope_wins_and_repeat_filter() {
        let m = ShortcutMap::defaults(Platform::Windows);
        let none = Modifiers::NONE;
        assert_eq!(
            m.lookup(Trigger::Key(Key::C), none, Scope::Timeline, false),
            Some(Action::BladeTool)
        );
        assert_eq!(
            m.lookup(Trigger::Key(Key::C), none, Scope::Media, false),
            None
        );
        assert_eq!(
            m.lookup(Trigger::Key(Key::ArrowRight), none, Scope::Media, true),
            Some(Action::NextFrame)
        );
        assert_eq!(
            m.lookup(Trigger::Key(Key::M), none, Scope::Media, true),
            None
        );
    }

    /// Runs one egui frame with `events` and returns what the map collects.
    fn dispatch(
        m: &ShortcutMap,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        scope: Scope,
        text_field: bool,
    ) -> Vec<Action> {
        let mut got = Vec::new();
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            if text_field {
                let mut s = String::new();
                let r = ui.text_edit_singleline(&mut s);
                r.request_focus();
            }
            got = m.collect(ui.ctx(), scope);
        });
        out.textures_delta.clear();
        got
    }

    /// Acceptance 10: every binding in the table, on both platforms, fires its
    /// action when the key event arrives through a real egui context.
    #[test]
    fn ui_every_binding_dispatches() {
        for p in [Platform::Windows, Platform::MacOs] {
            let m = ShortcutMap::defaults(p);
            let ctx = egui::Context::default();
            let mut checked = 0;
            for a in Action::ALL {
                for c in m.chords(*a) {
                    let Trigger::Key(key) = c.trigger else {
                        assert!(m.click_matches(*a, modifiers_for(c, p)));
                        continue;
                    };
                    let ev = egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: modifiers_for(c, p),
                    };
                    // Press and release, so the next press is not a repeat.
                    let release = egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: false,
                        repeat: false,
                        modifiers: modifiers_for(c, p),
                    };
                    let got = dispatch(&m, &ctx, vec![ev, release], m.scope(*a), false);
                    assert_eq!(got, vec![*a], "{} on {p:?}", c.display(p));
                    checked += 1;
                }
            }
            assert!(checked >= 95, "{checked}");
        }
    }

    #[test]
    fn ui_text_fields_swallow_keys() {
        let m = ShortcutMap::defaults(Platform::Windows);
        let ctx = egui::Context::default();
        // Frame 1 gives the text field focus; frame 2 types into it.
        dispatch(&m, &ctx, vec![], Scope::Global, true);
        let ev = egui::Event::Key {
            key: Key::M,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        assert!(dispatch(&m, &ctx, vec![ev.clone()], Scope::Global, true).is_empty());
        // Without a focused text field the same key adds a marker.
        let ctx = egui::Context::default();
        assert_eq!(
            dispatch(&m, &ctx, vec![ev], Scope::Global, false),
            vec![Action::AddMarker]
        );
    }
}
