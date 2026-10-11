//! Read-side model: plain Rust structs materialised from the Loro document.
//!
//! The UI, renderer and exporter only ever *read* these snapshots; all
//! writes go through [`crate::command::Command`]s. A [`DocSnapshot`] is an
//! immutable value, which is what lets `render(doc_snapshot, tick)` be a pure
//! function.

use std::collections::BTreeMap;

use loro::LoroValue;
use serde::{Deserialize, Serialize};

use crate::ids::{ClipId, MarkerId, MediaId, SeqId, TrackId};
use crate::time::{Ticks, Timebase};

/// Current document schema version, stored in `meta.schema_version`.
pub const SCHEMA_VERSION: i64 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ColorSpace {
    #[default]
    Rec709,
    Rec601,
    Rec2020,
}

impl ColorSpace {
    pub fn as_str(self) -> &'static str {
        match self {
            ColorSpace::Rec709 => "bt709",
            ColorSpace::Rec601 => "bt601",
            ColorSpace::Rec2020 => "bt2020",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "bt601" => ColorSpace::Rec601,
            "bt2020" => ColorSpace::Rec2020,
            _ => ColorSpace::Rec709,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Meta {
    pub name: String,
    pub timebase: Timebase,
    pub width: u32,
    pub height: u32,
    pub sample_rate: u32,
    pub color_space: ColorSpace,
    pub schema_version: i64,
}

impl Default for Meta {
    fn default() -> Self {
        Meta {
            name: "Untitled".into(),
            timebase: Timebase::FPS_30,
            width: 1920,
            height: 1080,
            sample_rate: 48_000,
            color_space: ColorSpace::Rec709,
            schema_version: SCHEMA_VERSION,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MediaKind {
    Video,
    Audio,
    Image,
}

impl MediaKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MediaKind::Video => "video",
            MediaKind::Audio => "audio",
            MediaKind::Image => "image",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "video" => MediaKind::Video,
            "audio" => MediaKind::Audio,
            "image" => MediaKind::Image,
            _ => return None,
        })
    }
}

/// A media entry in the shared document. Contains no paths.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaEntry {
    pub id: MediaId,
    pub name: String,
    pub kind: MediaKind,
    pub duration: Ticks,
    pub fps: Option<Timebase>,
    pub width: u32,
    pub height: u32,
    pub audio_channels: u32,
    pub audio_sample_rate: u32,
    pub size_bytes: u64,
    pub proxy_hash: Option<MediaId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TrackKind {
    Video,
    Audio,
    Title,
}

impl TrackKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TrackKind::Video => "video",
            TrackKind::Audio => "audio",
            TrackKind::Title => "title",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "video" => TrackKind::Video,
            "audio" => TrackKind::Audio,
            "title" => TrackKind::Title,
            _ => return None,
        })
    }
    /// Whether clips on this track produce pictures.
    pub fn is_visual(self) -> bool {
        matches!(self, TrackKind::Video | TrackKind::Title)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub id: TrackId,
    pub kind: TrackKind,
    pub name: String,
    pub locked: bool,
    pub hidden: bool,
    pub muted: bool,
    pub height: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Generator {
    Title,
    /// Solid colour, straight RGBA.
    Color([f32; 4]),
    Adjustment,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClipSource {
    Media(MediaId),
    Generator(Generator),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BlendMode {
    #[default]
    Normal,
    Add,
    Multiply,
    Screen,
}

impl BlendMode {
    pub fn as_str(self) -> &'static str {
        match self {
            BlendMode::Normal => "normal",
            BlendMode::Add => "add",
            BlendMode::Multiply => "multiply",
            BlendMode::Screen => "screen",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "add" => BlendMode::Add,
            "multiply" => BlendMode::Multiply,
            "screen" => BlendMode::Screen,
            _ => BlendMode::Normal,
        }
    }
}

/// Clip transform. Position is in project pixels relative to frame centre;
/// rotation in degrees clockwise; anchor in normalised clip coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub x: f64,
    pub y: f64,
    pub scale_x: f64,
    pub scale_y: f64,
    pub rotation: f64,
    pub anchor_x: f64,
    pub anchor_y: f64,
    pub opacity: f64,
    pub blend: BlendMode,
    pub flip_h: bool,
    pub flip_v: bool,
}

impl Default for Transform {
    fn default() -> Self {
        Transform {
            x: 0.0,
            y: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
            rotation: 0.0,
            anchor_x: 0.5,
            anchor_y: 0.5,
            opacity: 1.0,
            blend: BlendMode::Normal,
            flip_h: false,
            flip_v: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipAudio {
    pub gain_db: f64,
    pub pan: f64,
    pub fade_in: Ticks,
    pub fade_out: Ticks,
}

impl Default for ClipAudio {
    fn default() -> Self {
        ClipAudio {
            gain_db: 0.0,
            pan: 0.0,
            fade_in: Ticks::ZERO,
            fade_out: Ticks::ZERO,
        }
    }
}

/// Playback speed as an exact rational (`num/den` × realtime).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Speed {
    pub num: i64,
    pub den: i64,
}

impl Default for Speed {
    fn default() -> Self {
        Speed { num: 1, den: 1 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub id: ClipId,
    pub name: String,
    pub track_id: TrackId,
    /// Timeline position of the first frame.
    pub start: Ticks,
    /// Source in-point (inclusive), in source ticks.
    pub in_tick: Ticks,
    /// Source out-point (exclusive), in source ticks.
    pub out_tick: Ticks,
    pub source: ClipSource,
    pub speed: Speed,
    pub reversed: bool,
    pub enabled: bool,
    pub transform: Transform,
    pub audio: ClipAudio,
    /// Application-level Lamport clock of the last position write; used by
    /// `normalize()` to decide which clip wins an overlap.
    pub pos_clock: i64,
    /// Peer that performed the last position write (tie breaker).
    pub pos_peer: u64,
    pub group_id: Option<String>,
    pub link_id: Option<String>,
}

impl Clip {
    /// Duration on the timeline.
    pub fn duration(&self) -> Ticks {
        let src = (self.out_tick - self.in_tick).0 as i128;
        Ticks((src * self.speed.den as i128 / self.speed.num.max(1) as i128) as i64)
    }

    pub fn end(&self) -> Ticks {
        self.start + self.duration()
    }

    /// Whether the clip covers timeline tick `t` (half-open).
    pub fn contains(&self, t: Ticks) -> bool {
        t >= self.start && t < self.end()
    }

    /// Maps a timeline tick to a source tick.
    pub fn source_tick_at(&self, t: Ticks) -> Ticks {
        let rel = (t - self.start).0 as i128 * self.speed.num as i128 / self.speed.den as i128;
        if self.reversed {
            self.out_tick - Ticks(1) - Ticks(rel as i64)
        } else {
            self.in_tick + Ticks(rel as i64)
        }
    }

    pub fn media_id(&self) -> Option<MediaId> {
        match &self.source {
            ClipSource::Media(m) => Some(*m),
            ClipSource::Generator(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MarkerColor {
    #[default]
    Blue,
    Red,
    Orange,
    Yellow,
    Green,
    Cyan,
    Purple,
    Pink,
}

impl MarkerColor {
    pub const ALL: [MarkerColor; 8] = [
        MarkerColor::Blue,
        MarkerColor::Red,
        MarkerColor::Orange,
        MarkerColor::Yellow,
        MarkerColor::Green,
        MarkerColor::Cyan,
        MarkerColor::Purple,
        MarkerColor::Pink,
    ];
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }
    pub fn from_index(i: i64) -> Self {
        Self::ALL.get(i as usize).copied().unwrap_or_default()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    pub id: MarkerId,
    pub tick: Ticks,
    pub color: MarkerColor,
    pub name: String,
    pub comment: String,
    pub author: String,
    pub resolved: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sequence {
    pub id: SeqId,
    pub name: String,
    /// Tracks in document order.
    pub tracks: Vec<Track>,
    pub clips: BTreeMap<ClipId, Clip>,
    pub markers: BTreeMap<MarkerId, Marker>,
}

impl Sequence {
    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    /// Video/title tracks from bottom (V1) to top.
    pub fn visual_tracks(&self) -> impl Iterator<Item = &Track> {
        self.tracks.iter().filter(|t| t.kind.is_visual())
    }

    pub fn audio_tracks(&self) -> impl Iterator<Item = &Track> {
        self.tracks.iter().filter(|t| t.kind == TrackKind::Audio)
    }

    pub fn clips_on(&self, track: TrackId) -> impl Iterator<Item = &Clip> {
        self.clips.values().filter(move |c| c.track_id == track)
    }

    /// End of the last clip.
    pub fn duration(&self) -> Ticks {
        self.clips
            .values()
            .map(|c| c.end())
            .max()
            .unwrap_or(Ticks::ZERO)
    }
}

/// Immutable materialised view of the whole document.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct DocSnapshot {
    pub meta: Meta,
    pub media: BTreeMap<MediaId, MediaEntry>,
    pub sequences: BTreeMap<SeqId, Sequence>,
    /// The sequence opened by default (first created).
    pub main_sequence: Option<SeqId>,
}

impl DocSnapshot {
    pub fn main(&self) -> Option<&Sequence> {
        self.main_sequence.and_then(|id| self.sequences.get(&id))
    }
}

// ---------------------------------------------------------------------------
// LoroValue → model parsing
// ---------------------------------------------------------------------------

pub(crate) type VMap = loro::LoroMapValue;

pub(crate) fn as_map(v: Option<&LoroValue>) -> Option<&VMap> {
    match v {
        Some(LoroValue::Map(m)) => Some(m),
        _ => None,
    }
}

pub(crate) fn get_str<'a>(m: &'a VMap, k: &str) -> Option<&'a str> {
    match m.get(k) {
        Some(LoroValue::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

pub(crate) fn get_i64(m: &VMap, k: &str) -> Option<i64> {
    match m.get(k) {
        Some(LoroValue::I64(v)) => Some(*v),
        Some(LoroValue::Double(v)) => Some(*v as i64),
        _ => None,
    }
}

pub(crate) fn get_f64(m: &VMap, k: &str) -> Option<f64> {
    match m.get(k) {
        Some(LoroValue::Double(v)) => Some(*v),
        Some(LoroValue::I64(v)) => Some(*v as f64),
        _ => None,
    }
}

pub(crate) fn get_bool(m: &VMap, k: &str) -> Option<bool> {
    match m.get(k) {
        Some(LoroValue::Bool(v)) => Some(*v),
        _ => None,
    }
}

pub(crate) fn parse_meta(m: &VMap) -> Meta {
    let d = Meta::default();
    let num = get_i64(m, "fps_num")
        .unwrap_or(d.timebase.num as i64)
        .max(1) as u32;
    let den = get_i64(m, "fps_den")
        .unwrap_or(d.timebase.den as i64)
        .max(1) as u32;
    Meta {
        name: get_str(m, "name").unwrap_or(&d.name).to_string(),
        timebase: Timebase::new(num, den),
        width: get_i64(m, "width").unwrap_or(d.width as i64) as u32,
        height: get_i64(m, "height").unwrap_or(d.height as i64) as u32,
        sample_rate: get_i64(m, "sample_rate").unwrap_or(d.sample_rate as i64) as u32,
        color_space: ColorSpace::parse(get_str(m, "color_space").unwrap_or("bt709")),
        schema_version: get_i64(m, "schema_version").unwrap_or(SCHEMA_VERSION),
    }
}

pub(crate) fn parse_media(key: &str, m: &VMap) -> Option<MediaEntry> {
    let id: MediaId = key.parse().ok()?;
    let fps = match (get_i64(m, "fps_num"), get_i64(m, "fps_den")) {
        (Some(n), Some(d)) if n > 0 && d > 0 => Some(Timebase::new(n as u32, d as u32)),
        _ => None,
    };
    Some(MediaEntry {
        id,
        name: get_str(m, "name").unwrap_or("").to_string(),
        kind: MediaKind::parse(get_str(m, "kind").unwrap_or(""))?,
        duration: Ticks(get_i64(m, "duration_ticks").unwrap_or(0)),
        fps,
        width: get_i64(m, "w").unwrap_or(0) as u32,
        height: get_i64(m, "h").unwrap_or(0) as u32,
        audio_channels: get_i64(m, "audio_ch").unwrap_or(0) as u32,
        audio_sample_rate: get_i64(m, "audio_rate").unwrap_or(0) as u32,
        size_bytes: get_i64(m, "size_bytes").unwrap_or(0) as u64,
        proxy_hash: get_str(m, "proxy_hash").and_then(|s| s.parse().ok()),
    })
}

fn parse_track(id: TrackId, m: &VMap) -> Option<Track> {
    Some(Track {
        id,
        kind: TrackKind::parse(get_str(m, "kind")?)?,
        name: get_str(m, "name").unwrap_or("").to_string(),
        locked: get_bool(m, "locked").unwrap_or(false),
        hidden: get_bool(m, "hidden").unwrap_or(false),
        muted: get_bool(m, "muted").unwrap_or(false),
        height: get_f64(m, "height").unwrap_or(56.0) as f32,
    })
}

fn parse_transform(m: Option<&VMap>) -> Transform {
    let d = Transform::default();
    let Some(m) = m else { return d };
    Transform {
        x: get_f64(m, "x").unwrap_or(d.x),
        y: get_f64(m, "y").unwrap_or(d.y),
        scale_x: get_f64(m, "scale_x").unwrap_or(d.scale_x),
        scale_y: get_f64(m, "scale_y").unwrap_or(d.scale_y),
        rotation: get_f64(m, "rotation").unwrap_or(d.rotation),
        anchor_x: get_f64(m, "anchor_x").unwrap_or(d.anchor_x),
        anchor_y: get_f64(m, "anchor_y").unwrap_or(d.anchor_y),
        opacity: get_f64(m, "opacity").unwrap_or(d.opacity),
        blend: BlendMode::parse(get_str(m, "blend").unwrap_or("normal")),
        flip_h: get_bool(m, "flip_h").unwrap_or(false),
        flip_v: get_bool(m, "flip_v").unwrap_or(false),
    }
}

fn parse_audio(m: Option<&VMap>) -> ClipAudio {
    let d = ClipAudio::default();
    let Some(m) = m else { return d };
    ClipAudio {
        gain_db: get_f64(m, "gain_db").unwrap_or(d.gain_db),
        pan: get_f64(m, "pan").unwrap_or(d.pan),
        fade_in: Ticks(get_i64(m, "fade_in").unwrap_or(0)),
        fade_out: Ticks(get_i64(m, "fade_out").unwrap_or(0)),
    }
}

fn parse_source(m: &VMap) -> Option<ClipSource> {
    match get_str(m, "source_kind")? {
        "media" => Some(ClipSource::Media(get_str(m, "media_id")?.parse().ok()?)),
        "title" => Some(ClipSource::Generator(Generator::Title)),
        "adjustment" => Some(ClipSource::Generator(Generator::Adjustment)),
        "color" => {
            let c = get_i64(m, "color_rgba").unwrap_or(0xFF) as u32;
            let [r, g, b, a] = c.to_be_bytes();
            Some(ClipSource::Generator(Generator::Color([
                r as f32 / 255.0,
                g as f32 / 255.0,
                b as f32 / 255.0,
                a as f32 / 255.0,
            ])))
        }
        _ => None,
    }
}

pub(crate) fn parse_clip(key: &str, m: &VMap) -> Option<Clip> {
    let id: ClipId = key.parse().ok()?;
    let speed = as_map(m.get("speed"));
    let speed = Speed {
        num: speed.and_then(|s| get_i64(s, "num")).unwrap_or(1).max(1),
        den: speed.and_then(|s| get_i64(s, "den")).unwrap_or(1).max(1),
    };
    Some(Clip {
        id,
        name: get_str(m, "name").unwrap_or("").to_string(),
        track_id: get_str(m, "track_id")?.parse().ok()?,
        start: Ticks(get_i64(m, "start_tick")?),
        in_tick: Ticks(get_i64(m, "in_tick")?),
        out_tick: Ticks(get_i64(m, "out_tick")?),
        source: parse_source(m)?,
        speed,
        reversed: get_bool(m, "reversed").unwrap_or(false),
        enabled: get_bool(m, "enabled").unwrap_or(true),
        transform: parse_transform(as_map(m.get("transform"))),
        audio: parse_audio(as_map(m.get("audio"))),
        pos_clock: get_i64(m, "pos_clock").unwrap_or(0),
        pos_peer: get_str(m, "pos_peer")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
        group_id: get_str(m, "group_id").map(str::to_string),
        link_id: get_str(m, "link_id").map(str::to_string),
    })
}

fn parse_marker(key: &str, m: &VMap) -> Option<Marker> {
    Some(Marker {
        id: key.parse().ok()?,
        tick: Ticks(get_i64(m, "tick")?),
        color: MarkerColor::from_index(get_i64(m, "color").unwrap_or(0)),
        name: get_str(m, "name").unwrap_or("").to_string(),
        comment: get_str(m, "comment").unwrap_or("").to_string(),
        author: get_str(m, "author").unwrap_or("").to_string(),
        resolved: get_bool(m, "resolved").unwrap_or(false),
    })
}

pub(crate) fn parse_sequence(key: &str, m: &VMap) -> Option<Sequence> {
    let id: SeqId = key.parse().ok()?;
    let props = as_map(m.get("track_props"));
    let mut tracks = Vec::new();
    if let Some(LoroValue::List(order)) = m.get("tracks") {
        for v in order.iter() {
            let LoroValue::String(tid) = v else { continue };
            let Ok(track_id) = tid.parse::<TrackId>() else {
                continue;
            };
            // A concurrently-duplicated track id in the order list is
            // shown once, at its first position.
            if tracks.iter().any(|t: &Track| t.id == track_id) {
                continue;
            }
            if let Some(t) = props
                .and_then(|p| as_map(p.get(tid.as_str())))
                .and_then(|tm| parse_track(track_id, tm))
            {
                tracks.push(t);
            }
        }
    }
    let mut clips = BTreeMap::new();
    if let Some(cm) = as_map(m.get("clips")) {
        for (k, v) in cm.iter() {
            if let Some(c) = as_map(Some(v)).and_then(|cv| parse_clip(k, cv))
                && tracks.iter().any(|t| t.id == c.track_id)
            {
                clips.insert(c.id, c);
            }
        }
    }
    let mut markers = BTreeMap::new();
    if let Some(mm) = as_map(m.get("markers")) {
        for (k, v) in mm.iter() {
            if let Some(mk) = as_map(Some(v)).and_then(|mv| parse_marker(k, mv)) {
                markers.insert(mk.id, mk);
            }
        }
    }
    Some(Sequence {
        id,
        name: get_str(m, "name").unwrap_or("Sequence").to_string(),
        tracks,
        clips,
        markers,
    })
}
