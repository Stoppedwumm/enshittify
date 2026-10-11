//! The eframe application: state, frame loop, top bar, action dispatch.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cutroom_core::command::*;
use cutroom_core::model::{Clip, MarkerColor, Meta, Transform};
use cutroom_core::{ClipId, MarkerId, MediaId, SeqId, Ticks, Timecode};
use cutroom_export::ExportQueue;
use cutroom_media::MediaBackend;
use cutroom_media::cache::Quality;
use cutroom_media::server::FrameServer;
use cutroom_sync::client::{ClientEvent, SyncClient};
use cutroom_sync::presence::{PresenceBoard, PresenceThrottle};
use cutroom_sync::protocol::Presence;
use egui::{Color32, RichText};
use egui_dock::{DockArea, DockState, Style as DockStyle};

use crate::dialogs::Dialogs;
use crate::i18n::I18n;
use crate::layout::{LayoutMode, Tab};
use crate::panels::{media::MediaUi, properties::PropsUi, timeline::TimelineUi};
use crate::playback::Playback;
use crate::preview::Preview;
use crate::session::{SaveState, Session};
use crate::settings::{Settings, config_dir, projects_dir};
use crate::shortcuts::{Action, Platform, Scope, ShortcutMap};
use crate::theme;

/// Command-line options.
#[derive(Clone, Debug, Default)]
pub struct AppOptions {
    pub project: Option<PathBuf>,
    pub relay: Option<String>,
    pub room: Option<String>,
    pub user_name: Option<String>,
    pub no_audio: bool,
}

pub struct SyncState {
    pub client: SyncClient,
    pub url: String,
    pub room: String,
    pub connected: bool,
    pub sent: loro::VersionVector,
    pub last_error: Option<String>,
}

pub struct Toast {
    pub text: String,
    pub at: Instant,
}

/// Everything except the dock tree (kept separate so the tab viewer can
/// borrow the state mutably).
pub struct AppState {
    pub i18n: I18n,
    pub settings: Settings,
    pub config_dir: PathBuf,
    pub shortcuts: ShortcutMap,
    pub backend: Arc<dyn MediaBackend>,
    pub frames: Arc<FrameServer>,
    pub session: Session,
    pub playback: Playback,
    pub preview: Preview,
    pub source_preview: Preview,
    pub selection: BTreeSet<ClipId>,
    pub selected_media: Option<MediaId>,
    /// Source viewer playhead (Dual layout), in source ticks.
    pub source_tick: Ticks,
    pub timeline: TimelineUi,
    pub media_ui: MediaUi,
    pub props: PropsUi,
    pub player_ui: crate::panels::player::PlayerUi,
    pub focus: Scope,
    pub mark_in: Option<Ticks>,
    pub mark_out: Option<Ticks>,
    pub clipboard: Vec<Clip>,
    pub toasts: Vec<Toast>,
    pub dialogs: Dialogs,
    pub textures: HashMap<(MediaId, bool), egui::TextureHandle>,
    pub sync: Option<SyncState>,
    pub presence: PresenceBoard,
    pub throttle: PresenceThrottle,
    pub follow: Option<u64>,
    pub export_queue: Option<ExportQueue>,
    pub export_job: Option<cutroom_export::JobId>,
    pub fullscreen: bool,
    /// Live value while a property is being dragged; committed on release.
    pub pending_transform: Option<(ClipId, Transform)>,
    pub frame_times: Vec<f32>,
    pub quit: bool,
    frame_no: u64,
}

pub struct CutroomApp {
    pub dock: DockState<Tab>,
    pub state: AppState,
}

impl CutroomApp {
    pub fn new(cc: &eframe::CreationContext<'_>, opts: AppOptions) -> anyhow::Result<Self> {
        let config_dir = config_dir();
        let mut settings = Settings::load(&config_dir);
        if let Some(n) = &opts.user_name {
            settings.user_name = n.clone();
        }
        if opts.no_audio {
            settings.use_audio_device = false;
        }
        theme::apply(&cc.egui_ctx, settings.theme);
        egui_extras_install(&cc.egui_ctx);

        let backend = cutroom_media::default_backend()?;
        let frames = Arc::new(FrameServer::new(backend.clone(), 768 << 20));
        let ctx = cc.egui_ctx.clone();
        frames.set_waker(move || ctx.request_repaint());

        let joining = opts.relay.is_some() && opts.room.is_some();
        let session = match &opts.project {
            Some(p) if p.join("doc.loro").exists() => Session::open(p, &backend)?,
            // Joining a room: start empty and receive the room's document, so
            // we never race the owner with a second InitProject.
            Some(p) if joining => Session::create_empty(p)?,
            Some(p) => Session::create(p, Meta::default())?,
            None if joining => {
                let room = opts.room.as_deref().unwrap_or("room");
                let p = projects_dir().join(format!("{room}.cutroom"));
                if p.join("doc.loro").exists() {
                    Session::open(&p, &backend)?
                } else {
                    Session::create_empty(&p)?
                }
            }
            None => match settings.recent_projects.first() {
                Some(p) if p.join("doc.loro").exists() => Session::open(p, &backend)?,
                _ => Session::create(&untitled_path(), Meta::default())?,
            },
        };
        let shortcuts = load_shortcuts(&config_dir, &settings);
        let dock = settings.layout.load(&config_dir);
        let rs = cc.wgpu_render_state.as_ref();
        let ffmpeg = cutroom_media::cli::CliBackend::locate()
            .ok()
            .map(|c| c.ffmpeg_path().to_path_buf());
        let ctx = cc.egui_ctx.clone();
        let export_queue =
            ffmpeg.map(|f| ExportQueue::new(backend.clone(), f, move || ctx.request_repaint()));
        let mut state = AppState {
            i18n: I18n::new(settings.lang),
            config_dir,
            shortcuts,
            playback: Playback::new(backend.clone(), settings.use_audio_device),
            preview: Preview::new(rs),
            source_preview: Preview::new(rs),
            backend,
            frames,
            session,
            selection: BTreeSet::new(),
            selected_media: None,
            source_tick: Ticks::ZERO,
            timeline: TimelineUi::default(),
            media_ui: MediaUi::default(),
            props: PropsUi::default(),
            player_ui: Default::default(),
            focus: Scope::Timeline,
            mark_in: None,
            mark_out: None,
            clipboard: Vec::new(),
            toasts: Vec::new(),
            dialogs: Dialogs::default(),
            textures: HashMap::new(),
            sync: None,
            presence: PresenceBoard::default(),
            throttle: PresenceThrottle::default(),
            follow: None,
            export_queue,
            export_job: None,
            fullscreen: false,
            pending_transform: None,
            frame_times: Vec::new(),
            quit: false,
            frame_no: 0,
            settings,
        };
        let root = state.session.store.root().to_path_buf();
        state.settings.push_recent(&root);
        if let (Some(url), Some(room)) = (opts.relay, opts.room) {
            state.connect(url, room, &cc.egui_ctx);
        }
        Ok(CutroomApp { dock, state })
    }
}

/// Image loaders are not needed; textures are uploaded manually.
fn egui_extras_install(_ctx: &egui::Context) {}

fn untitled_path() -> PathBuf {
    let dir = projects_dir();
    let _ = std::fs::create_dir_all(&dir);
    for n in 1.. {
        let p = dir.join(format!("Untitled {n}.cutroom"));
        if !p.exists() {
            return p;
        }
    }
    unreachable!()
}

pub fn load_shortcuts(dir: &Path, settings: &Settings) -> ShortcutMap {
    let mut m = ShortcutMap::with_preset(Platform::current(), settings.shortcut_preset);
    if let Ok(s) = std::fs::read_to_string(dir.join("shortcuts.user.toml"))
        && let Err(e) = m.apply_toml(&s)
    {
        tracing::warn!("ignoring invalid shortcuts.user.toml: {e}");
    }
    m
}

pub fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

impl AppState {
    pub fn t(&self, key: &str) -> String {
        self.i18n.t(key)
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.toasts.push(Toast {
            text: text.into(),
            at: Instant::now(),
        });
    }

    pub fn not_available(&mut self, feature: &str) {
        let msg = self.i18n.ta("not-available", &[("feature", feature)]);
        self.toast(msg);
    }

    /// Executes a command, reporting failures as a toast.
    pub fn exec(&mut self, cmd: &dyn Command) -> bool {
        match self.session.execute(cmd) {
            Ok(()) => true,
            Err(e) => {
                self.toast(e.to_string());
                false
            }
        }
    }

    pub fn seq(&mut self) -> Option<SeqId> {
        self.session.seq()
    }

    pub fn quality(&self) -> Quality {
        match self.settings.preview_quality {
            0 => Quality::Full,
            1 => Quality::Half,
            _ => Quality::Quarter,
        }
    }

    pub fn seek(&mut self, t: Ticks) {
        let snap = self.session.snapshot();
        let Some(seq) = snap.main_sequence else {
            return;
        };
        let tb = snap.meta.timebase;
        let resolve = self.session.path_resolver();
        let v = self.session.doc.frontiers();
        self.playback
            .seek(tb.snap_floor(t), &snap, seq, &resolve, v);
    }

    pub fn timecode(&mut self, t: Ticks) -> String {
        let tb = self.session.snapshot().meta.timebase;
        Timecode::from_ticks(t, tb, tb.supports_drop_frame()).to_string()
    }

    fn save_settings(&self) {
        if let Err(e) = self.settings.save(&self.config_dir) {
            tracing::warn!("cannot save settings: {e}");
        }
    }

    pub fn save_shortcuts(&self) {
        let base = ShortcutMap::with_preset(Platform::current(), self.settings.shortcut_preset);
        let diff = self.shortcuts.diff_toml(&base);
        let p = self.config_dir.join("shortcuts.user.toml");
        let _ = std::fs::create_dir_all(&self.config_dir);
        if let Err(e) = std::fs::write(p, diff) {
            tracing::warn!("cannot save shortcuts: {e}");
        }
    }

    // ---------------------------------------------------------------- project

    pub fn new_project(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("New Cutroom project")
            .set_directory(projects_dir())
            .set_file_name("My Project")
            .save_file()
        else {
            return;
        };
        match Session::create(&path, Meta::default()) {
            Ok(s) => self.replace_session(s),
            Err(e) => self.toast(format!("Cannot create project: {e}")),
        }
    }

    pub fn open_project_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .set_title("Open Cutroom project (.cutroom folder)")
            .set_directory(projects_dir())
            .pick_folder()
        {
            self.open_project(&p);
        }
    }

    pub fn open_project(&mut self, p: &Path) {
        match Session::open(p, &self.backend) {
            Ok(s) => self.replace_session(s),
            Err(e) => self.toast(format!("Cannot open project: {e}")),
        }
    }

    fn replace_session(&mut self, s: Session) {
        let _ = self.session.save();
        self.playback.pause();
        self.playback.set_playhead(Ticks::ZERO);
        self.selection.clear();
        self.selected_media = None;
        self.textures.clear();
        self.sync = None;
        self.presence = PresenceBoard::default();
        self.session = s;
        let root = self.session.store.root().to_path_buf();
        self.settings.push_recent(&root);
        self.save_settings();
    }

    pub fn save(&mut self) {
        match self.session.save() {
            Ok(()) => {}
            Err(e) => self.toast(format!("Save failed: {e}")),
        }
    }

    pub fn save_as(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .set_title("Save project as")
            .set_directory(projects_dir())
            .save_file()
        {
            if let Err(e) = self.session.save_as(&p) {
                self.toast(format!("Save As failed: {e}"));
            } else {
                let root = self.session.store.root().to_path_buf();
                self.settings.push_recent(&root);
                self.save_settings();
            }
        }
    }

    pub fn import_dialog(&mut self) {
        if let Some(files) = rfd::FileDialog::new()
            .set_title("Import media")
            .add_filter(
                "Media",
                &[
                    "mp4", "mov", "mkv", "webm", "avi", "m4v", "mxf", "mts", "wav", "mp3", "aac",
                    "m4a", "flac", "ogg", "opus", "png", "jpg", "jpeg", "webp", "bmp", "tif",
                    "tiff", "gif",
                ],
            )
            .add_filter("All files", &["*"])
            .pick_files()
        {
            self.session.import(files, &self.backend);
        }
    }

    // ---------------------------------------------------------------- sync

    pub fn connect(&mut self, url: String, room: String, ctx: &egui::Context) {
        let ctx = ctx.clone();
        let peer = self.session.doc.peer_id();
        let client = SyncClient::connect(
            url.clone(),
            room.clone(),
            peer,
            self.session.version().encode(),
            None,
            move || ctx.request_repaint(),
        );
        self.sync = Some(SyncState {
            client,
            url: url.clone(),
            room,
            connected: false,
            sent: self.session.version(),
            last_error: None,
        });
        self.settings.relay_url = url;
        self.save_settings();
    }

    fn pump_sync(&mut self) {
        let Some(sync) = self.sync.as_mut() else {
            return;
        };
        let now = Instant::now();
        while let Ok(ev) = sync.client.events().try_recv() {
            match ev {
                ClientEvent::Connected => {
                    sync.connected = true;
                    sync.last_error = None;
                }
                ClientEvent::Disconnected(e) => {
                    sync.connected = false;
                    sync.last_error = Some(e);
                }
                ClientEvent::Remote(bytes) => {
                    let had_unsent = self.session.version() != sync.sent;
                    if let Err(e) = self.session.doc.import(&bytes) {
                        tracing::warn!("bad remote update: {e}");
                    }
                    if !had_unsent {
                        sync.sent = self.session.version();
                    }
                }
                ClientEvent::PushSince(vv) => {
                    let vv = loro::VersionVector::decode(&vv).unwrap_or_default();
                    let bytes = self.session.doc.export_updates_since(&vv);
                    sync.client.send_update(bytes);
                    sync.sent = self.session.version();
                }
                ClientEvent::Presence(p) => self.presence.apply(p, now),
                ClientEvent::PresenceGone(peer) => self.presence.remove(peer),
                ClientEvent::Acked(_) => {}
            }
        }
        // One network batch per local transaction (or per frame at most).
        if sync.connected && self.session.version() != sync.sent {
            let bytes = self.session.doc.export_updates_since(&sync.sent);
            sync.client.send_update(bytes);
            sync.sent = self.session.version();
        }
        self.presence.expire(now);
        // Presence: playhead, selection, panel, viewport.
        let p = Presence {
            peer_id: self.session.doc.peer_id(),
            name: self.settings.user_name.clone(),
            color: {
                let c = theme::peer_color(self.session.doc.peer_id());
                [c.r(), c.g(), c.b()]
            },
            playhead_tick: self.playback.playhead.0,
            selected_clips: self.selection.iter().map(|c| c.key()).collect(),
            active_panel: self.focus.label().to_lowercase(),
            drag: self.timeline.drag_ghost(),
            viewport: self.timeline.viewport(),
        };
        self.throttle.update(p);
        if let Some(p) = self.throttle.poll(now)
            && sync.connected
        {
            sync.client.send_presence(p);
        }
        // Follow mode: track the collaborator's playhead.
        if let Some(f) = self.follow
            && let Some(p) = self.presence.peers().find(|p| p.peer_id == f)
        {
            self.playback.set_playhead(Ticks(p.playhead_tick));
        }
    }

    // ---------------------------------------------------------------- actions

    pub fn selected_clips(&mut self) -> Vec<Clip> {
        let snap = self.session.snapshot();
        let Some(seq) = snap.main() else {
            return vec![];
        };
        self.selection
            .iter()
            .filter_map(|id| seq.clips.get(id).cloned())
            .collect()
    }

    fn edit_points(&mut self) -> Vec<Ticks> {
        let snap = self.session.snapshot();
        let mut v: Vec<Ticks> = snap
            .main()
            .map(|s| s.clips.values().flat_map(|c| [c.start, c.end()]).collect())
            .unwrap_or_default();
        v.push(Ticks::ZERO);
        v.sort();
        v.dedup();
        v
    }

    fn marker_ticks(&mut self) -> Vec<Ticks> {
        let snap = self.session.snapshot();
        let mut v: Vec<Ticks> = snap
            .main()
            .map(|s| s.markers.values().map(|m| m.tick).collect())
            .unwrap_or_default();
        v.sort();
        v
    }

    fn jump(&mut self, points: Vec<Ticks>, forward: bool) {
        let ph = self.playback.playhead;
        let t = if forward {
            points.into_iter().find(|t| *t > ph)
        } else {
            points.into_iter().rev().find(|t| *t < ph)
        };
        if let Some(t) = t {
            self.seek(t);
        }
    }

    fn add_marker(&mut self, color: MarkerColor) {
        let Some(seq) = self.seq() else { return };
        let author = self.settings.user_name.clone();
        let tick = self.playback.playhead;
        self.exec(&AddMarker {
            seq,
            id: MarkerId::new(),
            tick,
            color,
            name: String::new(),
            author,
        });
    }

    fn split_at_playhead(&mut self) {
        let snap = self.session.snapshot();
        let Some(seq) = snap.main() else { return };
        let t = self.playback.playhead;
        // Selected clips under the playhead, else every clip under it.
        let mut targets: Vec<ClipId> = seq
            .clips
            .values()
            .filter(|c| self.selection.contains(&c.id) && c.start < t && c.end() > t)
            .map(|c| c.id)
            .collect();
        if targets.is_empty() {
            targets = seq
                .clips
                .values()
                .filter(|c| c.start < t && c.end() > t)
                .filter(|c| seq.track(c.track_id).is_some_and(|tr| !tr.locked))
                .map(|c| c.id)
                .collect();
        }
        let sid = seq.id;
        for c in targets {
            self.exec(&SplitClip {
                seq: sid,
                clip: c,
                at: t,
                new_id: ClipId::new(),
            });
        }
    }

    fn trim_to_playhead(&mut self, start: bool) {
        let t = self.playback.playhead;
        let Some(seq) = self.seq() else { return };
        for c in self.selected_clips() {
            if !(c.start < t && c.end() > t) {
                continue;
            }
            let src = c.source_tick_at(t);
            let cmd = if start {
                TrimClip {
                    seq,
                    clip: c.id,
                    start: t,
                    in_tick: src,
                    out_tick: c.out_tick,
                }
            } else {
                TrimClip {
                    seq,
                    clip: c.id,
                    start: c.start,
                    in_tick: c.in_tick,
                    out_tick: src,
                }
            };
            self.exec(&cmd);
        }
    }

    fn delete_selection(&mut self) {
        let Some(seq) = self.seq() else { return };
        if self.focus == Scope::Media {
            if let Some(m) = self.selected_media.take() {
                self.exec(&RemoveMedia(m));
            }
            return;
        }
        let clips: Vec<ClipId> = self.selection.iter().copied().collect();
        if !clips.is_empty() && self.exec(&DeleteClips { seq, clips }) {
            self.selection.clear();
        }
    }

    fn paste(&mut self) {
        let Some(seq) = self.seq() else { return };
        if self.clipboard.is_empty() {
            return;
        }
        let first = self
            .clipboard
            .iter()
            .map(|c| c.start)
            .min()
            .unwrap_or_default();
        let at = self.playback.playhead;
        let mut new_sel = BTreeSet::new();
        for c in self.clipboard.clone() {
            let id = ClipId::new();
            let ok = self.exec(&AddClip {
                seq,
                clip: NewClip {
                    id,
                    name: c.name.clone(),
                    track: c.track_id,
                    start: at + (c.start - first),
                    in_tick: c.in_tick,
                    out_tick: c.out_tick,
                    source: c.source.clone(),
                },
            });
            if ok {
                new_sel.insert(id);
            }
        }
        self.selection = new_sel;
    }

    fn nudge(&mut self, frames: i64, tracks: i32) {
        let snap = self.session.snapshot();
        let Some(seq) = snap.main() else { return };
        let tb = snap.meta.timebase;
        let order: Vec<_> = seq.tracks.iter().map(|t| t.id).collect();
        let moves: Vec<_> = self
            .selected_clips()
            .into_iter()
            .map(|c| {
                let idx = order.iter().position(|t| *t == c.track_id).unwrap_or(0) as i32;
                // Visual "up" is a higher video track.
                let ni = (idx + tracks).clamp(0, order.len() as i32 - 1) as usize;
                let track = if seq.track(order[ni]).map(|t| t.kind)
                    == seq.track(c.track_id).map(|t| t.kind)
                {
                    order[ni]
                } else {
                    c.track_id
                };
                (
                    c.id,
                    track,
                    (c.start + tb.frames_to_ticks(frames)).max(Ticks::ZERO),
                )
            })
            .collect();
        if !moves.is_empty() {
            let sid = seq.id;
            self.exec(&MoveClips { seq: sid, moves });
        }
    }

    fn rotate(&mut self, deg: f64) {
        let Some(seq) = self.seq() else { return };
        for c in self.selected_clips() {
            let r = (c.transform.rotation + deg).rem_euclid(360.0);
            self.exec(&SetTransform {
                seq,
                clip: c.id,
                fields: vec![TransformField::Rotation(r)],
            });
        }
    }

    pub fn snapshot_png(&mut self) {
        let snap = self.session.snapshot();
        let Some(seq) = snap.main_sequence else {
            return;
        };
        let (w, h, rgba) =
            self.preview
                .snapshot_rgba(&snap, seq, self.playback.playhead, &self.frames);
        let Some(img) = image::RgbaImage::from_raw(w, h, rgba) else {
            return;
        };
        let dir = self.session.store.root().join("snapshots");
        let _ = std::fs::create_dir_all(&dir);
        let name = format!(
            "Snapshot {}.png",
            self.timecode(self.playback.playhead)
                .replace([':', ';'], "-")
        );
        let p = dir.join(name);
        match img.save(&p) {
            Ok(()) => {
                self.session.import(vec![p], &self.backend);
            }
            Err(e) => self.toast(format!("Snapshot failed: {e}")),
        }
    }

    /// Executes an action from a shortcut, menu or toolbar.
    pub fn run_action(&mut self, a: Action, ctx: &egui::Context) {
        use Action::*;
        let tb = self.session.snapshot().meta.timebase;
        let end = self
            .session
            .snapshot()
            .main()
            .map(|s| s.duration())
            .unwrap_or_default();
        match a {
            NewProject => self.new_project(),
            OpenProject => self.open_project_dialog(),
            SaveProject => self.save(),
            SaveProjectAs => self.save_as(),
            ImportMedia => self.import_dialog(),
            Preferences => self.dialogs.preferences = true,
            ShortcutEditor => self.dialogs.open_shortcuts(),
            Export => self.dialogs.open_export(),
            Undo => {
                self.session.doc.undo();
            }
            Redo => {
                self.session.doc.redo();
            }
            Copy | Cut => {
                self.clipboard = self.selected_clips();
                if a == Cut {
                    self.delete_selection();
                }
            }
            Paste => self.paste(),
            Duplicate => {
                let clips = self.selected_clips();
                if let (Some(seq), false) = (self.seq(), clips.is_empty()) {
                    let end = clips.iter().map(|c| c.end()).max().unwrap_or_default();
                    let first = clips.iter().map(|c| c.start).min().unwrap_or_default();
                    for c in clips {
                        self.exec(&AddClip {
                            seq,
                            clip: NewClip {
                                id: ClipId::new(),
                                name: c.name.clone(),
                                track: c.track_id,
                                start: end + (c.start - first),
                                in_tick: c.in_tick,
                                out_tick: c.out_tick,
                                source: c.source.clone(),
                            },
                        });
                    }
                }
            }
            Delete => self.delete_selection(),
            SelectAll => {
                let snap = self.session.snapshot();
                self.selection = snap
                    .main()
                    .map(|s| s.clips.keys().copied().collect())
                    .unwrap_or_default();
            }
            SelectClipRange => {
                let clips = self.selected_clips();
                if let (Some(s), Some(e)) = (
                    clips.iter().map(|c| c.start).min(),
                    clips.iter().map(|c| c.end()).max(),
                ) {
                    self.mark_in = Some(s);
                    self.mark_out = Some(e);
                }
            }
            CancelClipRange | ClearInOut => {
                self.mark_in = None;
                self.mark_out = None;
            }
            NudgeLeft => self.nudge(-1, 0),
            NudgeRight => self.nudge(1, 0),
            NudgeUp => self.nudge(0, 1),
            NudgeDown => self.nudge(0, -1),
            SplitAtPlayhead => self.split_at_playhead(),
            TrimStartToPlayhead => self.trim_to_playhead(true),
            TrimEndToPlayhead => self.trim_to_playhead(false),
            EditProperties => self.focus = Scope::Timeline,
            RotateCw => self.rotate(90.0),
            RotateCcw => self.rotate(-90.0),
            SelectTool => self.timeline.tool = crate::panels::timeline::Tool::Select,
            BladeTool => self.timeline.tool = crate::panels::timeline::Tool::Blade,
            PlayPause => {
                let snap = self.session.snapshot();
                if let Some(seq) = snap.main_sequence {
                    let r = self.session.path_resolver();
                    let v = self.session.doc.frontiers();
                    self.playback.toggle(&snap, seq, &r, v);
                }
            }
            Stop => {
                self.playback.pause();
                self.playback.set_playhead(Ticks::ZERO);
            }
            ShuttleReverse | ShuttleForward => {
                let snap = self.session.snapshot();
                if let Some(seq) = snap.main_sequence {
                    let r = self.session.path_resolver();
                    let v = self.session.doc.frontiers();
                    let dir = if a == ShuttleForward { 1 } else { -1 };
                    self.playback.shuttle(dir, &snap, seq, &r, v);
                }
            }
            ShuttleStop => self.playback.pause(),
            FullscreenPreview => {
                self.fullscreen = !self.fullscreen;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
            }
            PrevFrame => self.seek(self.playback.playhead - tb.frame_duration()),
            NextFrame => self.seek(self.playback.playhead + tb.frame_duration()),
            BackOneSecond => self.seek(self.playback.playhead - Ticks::ONE_SECOND),
            ForwardOneSecond => self.seek(self.playback.playhead + Ticks::ONE_SECOND),
            PrevEditPoint | NextEditPoint => {
                let p = self.edit_points();
                self.jump(p, a == NextEditPoint);
            }
            PrevMarker | NextMarker => {
                let p = self.marker_ticks();
                self.jump(p, a == NextMarker);
            }
            ProjectStart => self.seek(Ticks::ZERO),
            ProjectEnd => self.seek(end),
            ClipStart | ClipEnd => {
                if let Some(c) = self.selected_clips().first() {
                    let t = if a == ClipStart {
                        c.start
                    } else {
                        c.end() - tb.frame_duration()
                    };
                    self.seek(t);
                }
            }
            PrevClip | NextClip => {
                let snap = self.session.snapshot();
                let starts: Vec<Ticks> = snap
                    .main()
                    .map(|s| {
                        let mut v: Vec<_> = s.clips.values().map(|c| c.start).collect();
                        v.sort();
                        v.dedup();
                        v
                    })
                    .unwrap_or_default();
                self.jump(starts, a == NextClip);
            }
            Snapshot => self.snapshot_png(),
            ZoomIn => self.timeline.zoom(1.25),
            ZoomOut => self.timeline.zoom(0.8),
            ZoomToFit => self.timeline.fit(end),
            MarkIn => self.mark_in = Some(self.playback.playhead),
            MarkOut => self.mark_out = Some(self.playback.playhead),
            AddMarker => self.add_marker(MarkerColor::Blue),
            EditMarker => {
                let ph = self.playback.playhead;
                let snap = self.session.snapshot();
                if let Some(m) = snap
                    .main()
                    .and_then(|s| s.markers.values().min_by_key(|m| (m.tick - ph).0.abs()))
                {
                    self.dialogs.edit_marker = Some((m.id, m.name.clone(), m.comment.clone()));
                }
            }
            ToggleSnapping => self.settings.snapping = !self.settings.snapping,
            FollowCollaborator => {
                self.follow = match self.follow {
                    Some(_) => None,
                    None => self.presence.peers().next().map(|p| p.peer_id),
                };
            }
            JumpToCollaborator => {
                let t = self.presence.peers().next().map(|p| p.playhead_tick);
                if let Some(t) = t {
                    self.seek(Ticks(t));
                }
            }
            Rename => {
                if let Some(m) = self.selected_media {
                    let name = self
                        .session
                        .snapshot()
                        .media
                        .get(&m)
                        .map(|e| e.name.clone())
                        .unwrap_or_default();
                    self.dialogs.rename_media = Some((m, name));
                }
            }
            RevealInFolder => {
                if let Some(p) = self.selected_media.and_then(|m| self.session.local_path(m)) {
                    reveal(&p);
                }
            }
            InsertEdit | OverwriteEdit | ApplySelectedAsset => {
                if let Some(m) = self.selected_media {
                    crate::panels::timeline::place_media_at_playhead(self, m, a == InsertEdit);
                }
            }
            _ => {
                if let Some(i) = a.marker_color_index() {
                    self.add_marker(MarkerColor::ALL[i]);
                } else {
                    let label = a.label();
                    self.not_available(label);
                }
            }
        }
    }

    fn file_drops(&mut self, ctx: &egui::Context) {
        let files: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if files.is_empty() {
            return;
        }
        let (projects, media): (Vec<_>, Vec<_>) =
            files.into_iter().partition(|p| p.join("doc.loro").exists());
        if let Some(p) = projects.first() {
            self.open_project(p);
        }
        if !media.is_empty() {
            self.session.import(media, &self.backend);
        }
    }
}

fn reveal(p: &Path) {
    let dir = p.parent().unwrap_or(p);
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer")
        .arg("/select,")
        .arg(p)
        .spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-R").arg(p).spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    let _ = dir;
}

struct Viewer<'a> {
    state: &'a mut AppState,
}

impl egui_dock::TabViewer for Viewer<'_> {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        self.state.t(tab.i18n_key()).into()
    }

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(("cutroom-tab", *tab))
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        // Clicking inside a panel makes it the shortcut context.
        if ui.rect_contains_pointer(ui.max_rect()) && ui.input(|i| i.pointer.any_pressed()) {
            self.state.focus = match tab {
                Tab::Media => Scope::Media,
                Tab::Timeline => Scope::Timeline,
                Tab::Player | Tab::Source => Scope::Player,
                Tab::Properties => self.state.focus,
            };
        }
        match tab {
            Tab::Media => crate::panels::media::ui(ui, self.state),
            Tab::Player => crate::panels::player::ui(ui, self.state, false),
            Tab::Source => crate::panels::player::ui(ui, self.state, true),
            Tab::Properties => crate::panels::properties::ui(ui, self.state),
            Tab::Timeline => crate::panels::timeline::ui(ui, self.state),
        }
    }

    fn is_closeable(&self, _tab: &Tab) -> bool {
        false
    }

    fn scroll_bars(&self, _tab: &Tab) -> [bool; 2] {
        [false, false]
    }
}

impl CutroomApp {
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.state;
        let ctx = ui.ctx().clone();
        let mut actions: Vec<Action> = Vec::new();
        let mut new_layout: Option<LayoutMode> = None;
        egui::MenuBar::new().ui(ui, |ui| {
            let item =
                |ui: &mut egui::Ui, s: &AppState, key: &str, a: Action, out: &mut Vec<Action>| {
                    let btn = egui::Button::new(s.t(key)).shortcut_text(s.shortcuts.hint(a));
                    if ui.add(btn).clicked() {
                        out.push(a);
                    }
                };
            ui.menu_button(s.t("menu-file"), |ui| {
                item(ui, s, "file-new", Action::NewProject, &mut actions);
                item(ui, s, "file-open", Action::OpenProject, &mut actions);
                ui.menu_button("Recent", |ui| {
                    for p in s.settings.recent_projects.clone() {
                        if ui.button(p.display().to_string()).clicked() {
                            s.open_project(&p);
                        }
                    }
                });
                ui.separator();
                item(ui, s, "file-save", Action::SaveProject, &mut actions);
                item(ui, s, "file-save-as", Action::SaveProjectAs, &mut actions);
                item(ui, s, "file-archive", Action::ArchiveProject, &mut actions);
                ui.separator();
                item(ui, s, "file-import", Action::ImportMedia, &mut actions);
                item(ui, s, "file-export", Action::Export, &mut actions);
                ui.separator();
                if ui.button(s.t("file-quit")).clicked() {
                    s.quit = true;
                }
            });
            ui.menu_button(s.t("menu-edit"), |ui| {
                item(ui, s, "edit-undo", Action::Undo, &mut actions);
                item(ui, s, "edit-redo", Action::Redo, &mut actions);
                ui.separator();
                item(ui, s, "edit-cut", Action::Cut, &mut actions);
                item(ui, s, "edit-copy", Action::Copy, &mut actions);
                item(ui, s, "edit-paste", Action::Paste, &mut actions);
                item(ui, s, "edit-duplicate", Action::Duplicate, &mut actions);
                item(ui, s, "edit-delete", Action::Delete, &mut actions);
                item(ui, s, "edit-select-all", Action::SelectAll, &mut actions);
                ui.separator();
                item(ui, s, "edit-preferences", Action::Preferences, &mut actions);
            });
            ui.menu_button(s.t("menu-tools"), |ui| {
                item(
                    ui,
                    s,
                    "tools-shortcuts",
                    Action::ShortcutEditor,
                    &mut actions,
                );
                if ui.button(s.t("tools-manage-tracks")).clicked() {
                    s.dialogs.manage_tracks = true;
                }
                if ui.button(s.t("tools-collaborate")).clicked() {
                    s.dialogs.open_collab(&s.settings);
                }
            });
            ui.menu_button(s.t("menu-view"), |ui| {
                ui.menu_button(s.t("view-layout"), |ui| {
                    for m in LayoutMode::ALL {
                        if ui
                            .radio(s.settings.layout == m, s.t(m.i18n_key()))
                            .clicked()
                        {
                            new_layout = Some(m);
                        }
                    }
                    ui.separator();
                    if ui.button("Reset current layout").clicked() {
                        s.settings.layout.reset(&s.config_dir);
                        new_layout = Some(s.settings.layout);
                    }
                });
                ui.menu_button(s.t("view-theme"), |ui| {
                    for (t, k) in [
                        (theme::Theme::Dark, "view-theme-dark"),
                        (theme::Theme::Light, "view-theme-light"),
                    ] {
                        if ui.radio(s.settings.theme == t, s.t(k)).clicked() {
                            s.settings.theme = t;
                            theme::apply(&ctx, t);
                            s.save_settings();
                        }
                    }
                });
                ui.menu_button(s.t("view-language"), |ui| {
                    for l in crate::i18n::Lang::ALL {
                        if ui.radio(s.i18n.lang() == l, l.label()).clicked() {
                            s.settings.lang = l;
                            s.i18n = I18n::new(l);
                            s.save_settings();
                        }
                    }
                });
                let label = s.t("view-safe-zones");
                ui.checkbox(&mut s.settings.safe_zones, label);
            });
            ui.menu_button(s.t("menu-help"), |ui| {
                if ui.button(s.t("help-shortcuts")).clicked() {
                    actions.push(Action::ShortcutEditor);
                }
                if ui.button(s.t("help-about")).clicked() {
                    s.dialogs.about = true;
                }
            });

            ui.separator();
            // Project name + save state.
            let name = s.session.name();
            let state = match s.session.save_state() {
                SaveState::Saved => s.t("state-saved"),
                SaveState::Autosaved => s.t("state-autosaved"),
                SaveState::Unsaved => s.t("state-unsaved"),
            };
            ui.label(RichText::new(name).strong());
            ui.label(RichText::new(state).small().weak());

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let export = egui::Button::new(
                    RichText::new(format!("⬆ {}", s.t("export-button")))
                        .color(Color32::BLACK)
                        .strong(),
                )
                .fill(theme::ACCENT);
                if ui
                    .add(export)
                    .on_hover_text(s.shortcuts.hint(Action::Export))
                    .clicked()
                {
                    actions.push(Action::Export);
                }
                if ui.button(format!("👥 {}", s.t("share-button"))).clicked() {
                    s.dialogs.open_collab(&s.settings);
                }
                // Avatars: collaborators, then me.
                let me = s.session.doc.peer_id();
                for p in s.presence.peers() {
                    avatar(
                        ui,
                        &p.name,
                        Color32::from_rgb(p.color[0], p.color[1], p.color[2]),
                        s.follow == Some(p.peer_id),
                    );
                }
                avatar(ui, &s.settings.user_name, theme::peer_color(me), false);
                if let Some(sync) = &s.sync {
                    let (txt, c) = if sync.connected {
                        (s.t("collab-connected"), theme::ACCENT)
                    } else {
                        (s.t("collab-offline"), Color32::GRAY)
                    };
                    ui.label(RichText::new(format!("● {txt}")).color(c).small())
                        .on_hover_text(format!("{} · {}", sync.url, sync.room));
                }
                ui.separator();
                let mut mode = s.settings.layout;
                egui::ComboBox::from_id_salt("layout-mode")
                    .selected_text(s.t(mode.i18n_key()))
                    .show_ui(ui, |ui| {
                        for m in LayoutMode::ALL {
                            ui.selectable_value(&mut mode, m, s.t(m.i18n_key()));
                        }
                    });
                if mode != s.settings.layout {
                    new_layout = Some(mode);
                }
            });
        });
        if let Some(m) = new_layout {
            let _ = self
                .state
                .settings
                .layout
                .save(&self.state.config_dir, &self.dock);
            self.state.settings.layout = m;
            self.dock = m.load(&self.state.config_dir);
            self.state.save_settings();
        }
        for a in actions {
            self.state.run_action(a, &ctx);
        }
    }
}

fn avatar(ui: &mut egui::Ui, name: &str, color: Color32, ring: bool) {
    let initials: String = name
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
    let p = ui.painter();
    p.circle_filled(rect.center(), 11.0, color);
    if ring {
        p.circle_stroke(rect.center(), 12.0, egui::Stroke::new(2.0, Color32::WHITE));
    }
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        if initials.is_empty() {
            "?".into()
        } else {
            initials
        },
        egui::FontId::proportional(10.0),
        Color32::BLACK,
    );
    resp.on_hover_text(name);
}

impl eframe::App for CutroomApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let t0 = Instant::now();
        let s = &mut self.state;

        // Background work → state.
        let ready = s.session.poll_imports(&s.backend, &s.frames);
        if !ready.is_empty() && s.selected_media.is_none() {
            s.selected_media = ready.first().copied();
        }
        s.pump_sync();
        s.session.autosave_tick();
        crate::panels::media::load_textures(&ctx, s);
        s.file_drops(&ctx);

        // Playback clock.
        let snap = s.session.snapshot();
        if let Some(seq) = snap.main_sequence {
            let resolve = s.session.path_resolver();
            let v = s.session.doc.frontiers();
            let end = snap.main().map(|q| q.duration()).unwrap_or_default();
            s.playback.update(&snap, seq, &resolve, v, end);
        }
        if s.playback.is_playing() || !s.session.pending.is_empty() || s.sync.is_some() {
            ctx.request_repaint();
        }

        // Shortcuts (text fields swallow keys inside `collect`).
        let actions = if s.dialogs.is_recording() {
            Vec::new()
        } else {
            s.shortcuts.collect(&ctx, s.focus)
        };
        for a in actions {
            s.run_action(a, &ctx);
        }

        egui::Panel::top("top-bar").show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status-bar").show(ui, |ui| status_bar(ui, &mut self.state));
        let mut style = DockStyle::from_egui(ui.style());
        style.tab_bar.fill_tab_bar = false;
        let dock = &mut self.dock;
        let state = &mut self.state;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                DockArea::new(dock)
                    .style(style)
                    .show_close_buttons(false)
                    .show_add_buttons(false)
                    .show_leaf_collapse_buttons(false)
                    .show_inside(ui, &mut Viewer { state });
            });

        crate::dialogs::show(&ctx, &mut self.state);
        toasts(&ctx, &mut self.state);

        let dt = t0.elapsed().as_secs_f32();
        let ft = &mut self.state.frame_times;
        ft.push(dt);
        if ft.len() > 120 {
            ft.remove(0);
        }
        if self.state.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        self.screenshot_hook(&ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let _ = self.state.session.save();
        let _ = self
            .state
            .settings
            .layout
            .save(&self.state.config_dir, &self.dock);
        self.state.save_settings();
    }
}

impl CutroomApp {
    /// `CUTROOM_SCREENSHOT=out.png`: render a few frames, save a screenshot
    /// and exit (headless smoke tests, docs).
    fn screenshot_hook(&mut self, ctx: &egui::Context) {
        let Some(path) = std::env::var_os("CUTROOM_SCREENSHOT") else {
            return;
        };
        self.state.frame_no += 1;
        let settle: u64 = std::env::var("CUTROOM_SCREENSHOT_FRAMES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(90);
        if self.state.frame_no == settle {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        ctx.request_repaint();
        let shot = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            let [w, h] = img.size;
            let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, bytes) {
                let _ = buf.save(&path);
            }
            self.state.quit = true;
        }
    }
}

fn status_bar(ui: &mut egui::Ui, s: &mut AppState) {
    ui.horizontal(|ui| {
        let snap = s.session.snapshot();
        ui.label(
            RichText::new(format!(
                "{}×{} · {} fps · {}",
                snap.meta.width,
                snap.meta.height,
                snap.meta.timebase,
                s.backend.name()
            ))
            .small()
            .weak(),
        );
        ui.separator();
        ui.label(
            RichText::new(format!(
                "Preview: {} · dropped {:.1}%",
                s.preview.backend_name(),
                s.preview.drop_ratio() * 100.0
            ))
            .small()
            .weak(),
        );
        if !s.session.pending.is_empty() {
            ui.separator();
            ui.spinner();
            ui.label(RichText::new(s.t("media-importing")).small());
        }
        if let (Some(q), Some(id)) = (&s.export_queue, s.export_job)
            && let Some(cutroom_export::JobState::Running(p)) = q.state(id)
        {
            ui.separator();
            ui.add(
                egui::ProgressBar::new(p.fraction())
                    .desired_width(140.0)
                    .show_percentage(),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let ft = &s.frame_times;
            if !ft.is_empty() {
                let avg = ft.iter().sum::<f32>() / ft.len() as f32;
                ui.label(
                    RichText::new(format!("UI {:.1} ms", avg * 1000.0))
                        .small()
                        .weak(),
                );
            }
        });
    });
}

fn toasts(ctx: &egui::Context, s: &mut AppState) {
    s.toasts.retain(|t| t.at.elapsed() < Duration::from_secs(4));
    if s.toasts.is_empty() {
        return;
    }
    egui::Area::new(egui::Id::new("toasts"))
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -32.0))
        .show(ctx, |ui| {
            for t in &s.toasts {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(&t.text);
                });
            }
        });
    ctx.request_repaint_after(Duration::from_millis(500));
}

/// Launches the desktop app. Prefers the wgpu renderer (Vulkan / Metal /
/// DX12 / GL); falls back to OpenGL via glow if no wgpu adapter exists, in
/// which case the preview uses the CPU rasterizer. `CUTROOM_RENDERER=gl`
/// forces the fallback.
pub fn run(opts: AppOptions) -> eframe::Result<()> {
    let forced_gl = std::env::var("CUTROOM_RENDERER").is_ok_and(|v| v == "gl");
    if !forced_gl {
        match launch(opts.clone(), eframe::Renderer::Wgpu) {
            Err(e) => tracing::warn!("wgpu renderer unavailable ({e}); falling back to OpenGL"),
            ok => return ok,
        }
    }
    launch(opts, eframe::Renderer::Glow)
}

fn launch(opts: AppOptions, renderer: eframe::Renderer) -> eframe::Result<()> {
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Cutroom")
            .with_inner_size([1600.0, 940.0])
            .with_min_inner_size([960.0, 600.0])
            .with_drag_and_drop(true),
        renderer,
        ..Default::default()
    };
    eframe::run_native(
        "Cutroom",
        native,
        Box::new(move |cc| {
            CutroomApp::new(cc, opts)
                .map(|a| Box::new(a) as Box<dyn eframe::App>)
                .map_err(|e| e.into_boxed_dyn_error())
        }),
    )
}
