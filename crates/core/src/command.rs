//! Commands: the only way to mutate the project document.
//!
//! A command validates against the pre-command [`DocSnapshot`] (available as
//! [`Tx::snapshot`]) and then emits Loro ops through [`Tx`]. The
//! [`crate::Document`] wraps each command in exactly one Loro transaction.
//!
//! **Rule:** perform every check before the first write. A command that fails
//! after writing is reverted, but that costs an extra history entry.

use std::fmt::Debug;

use loro::{LoroDoc, LoroMap, LoroMovableList, LoroValue};

use crate::document::{ROOT_MEDIA, ROOT_META, ROOT_SEQUENCES};
use crate::ids::{ClipId, MarkerId, MediaId, SeqId, TrackId};
use crate::model::{
    ClipAudio, ClipSource, DocSnapshot, Generator, MarkerColor, MediaEntry, Meta, SCHEMA_VERSION,
    Sequence, Speed, TrackKind, Transform,
};
use crate::time::Ticks;
use crate::validate;

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("sequence {0} not found")]
    NoSequence(SeqId),
    #[error("track {0} not found")]
    NoTrack(TrackId),
    #[error("clip {0} not found")]
    NoClip(ClipId),
    #[error("marker {0} not found")]
    NoMarker(MarkerId),
    #[error("media {0:?} not in project")]
    NoMedia(MediaId),
    #[error("track {0} is locked")]
    TrackLocked(TrackId),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error(transparent)]
    Loro(#[from] loro::LoroError),
}

pub type CmdResult = Result<(), CommandError>;

/// A user-level edit.
pub trait Command: Debug + Send + Sync {
    /// Short name, used as the Loro commit message (history view).
    fn name(&self) -> &'static str;
    fn apply(&self, tx: &mut Tx<'_>) -> CmdResult;
}

/// Write access to the document for the duration of one command.
pub struct Tx<'a> {
    doc: &'a LoroDoc,
    snap: &'a DocSnapshot,
}

impl<'a> Tx<'a> {
    pub(crate) fn new(doc: &'a LoroDoc, snap: &'a DocSnapshot) -> Self {
        Tx { doc, snap }
    }

    /// State before this command.
    pub fn snapshot(&self) -> &DocSnapshot {
        self.snap
    }

    pub fn peer(&self) -> u64 {
        self.doc.peer_id()
    }

    pub fn meta(&self) -> LoroMap {
        self.doc.get_map(ROOT_META)
    }

    pub fn media(&self) -> LoroMap {
        self.doc.get_map(ROOT_MEDIA)
    }

    pub fn sequences(&self) -> LoroMap {
        self.doc.get_map(ROOT_SEQUENCES)
    }

    pub fn seq_read(&self, id: SeqId) -> Result<&'a Sequence, CommandError> {
        self.snap
            .sequences
            .get(&id)
            .ok_or(CommandError::NoSequence(id))
    }

    pub fn seq_map(&self, id: SeqId) -> Result<LoroMap, CommandError> {
        Ok(self.sequences().ensure_mergeable_map(&id.key())?)
    }

    pub fn clips_map(&self, seq: SeqId) -> Result<LoroMap, CommandError> {
        Ok(self.seq_map(seq)?.ensure_mergeable_map("clips")?)
    }

    pub fn clip_map(&self, seq: SeqId, clip: ClipId) -> Result<LoroMap, CommandError> {
        Ok(self.clips_map(seq)?.ensure_mergeable_map(&clip.key())?)
    }

    pub fn track_order(&self, seq: SeqId) -> Result<LoroMovableList, CommandError> {
        Ok(self.seq_map(seq)?.ensure_mergeable_movable_list("tracks")?)
    }

    pub fn track_props(&self, seq: SeqId, track: TrackId) -> Result<LoroMap, CommandError> {
        Ok(self
            .seq_map(seq)?
            .ensure_mergeable_map("track_props")?
            .ensure_mergeable_map(&track.key())?)
    }

    pub fn marker_map(&self, seq: SeqId, m: MarkerId) -> Result<LoroMap, CommandError> {
        Ok(self
            .seq_map(seq)?
            .ensure_mergeable_map("markers")?
            .ensure_mergeable_map(&m.key())?)
    }

    /// Next application-level Lamport value for a position write in `seq`.
    pub fn next_pos_clock(&self, seq: SeqId) -> i64 {
        self.snap
            .sequences
            .get(&seq)
            .map(|s| s.clips.values().map(|c| c.pos_clock).max().unwrap_or(0))
            .unwrap_or(0)
            + 1
    }

    /// Writes `(track_id, start_tick)` as one LWW update with a fresh
    /// position clock.
    pub fn write_position(
        &self,
        seq: SeqId,
        clip: ClipId,
        track: TrackId,
        start: Ticks,
        clock: i64,
    ) -> CmdResult {
        let m = self.clip_map(seq, clip)?;
        m.insert("track_id", track.key())?;
        m.insert("start_tick", start.0)?;
        m.insert("pos_clock", clock)?;
        m.insert("pos_peer", self.peer().to_string())?;
        Ok(())
    }

    fn check_track_unlocked(&self, seq: &Sequence, track: TrackId) -> CmdResult {
        let t = seq.track(track).ok_or(CommandError::NoTrack(track))?;
        if t.locked {
            return Err(CommandError::TrackLocked(track));
        }
        Ok(())
    }

    fn require_clip<'s>(
        &self,
        seq: &'s Sequence,
        clip: ClipId,
    ) -> Result<&'s crate::model::Clip, CommandError> {
        seq.clips.get(&clip).ok_or(CommandError::NoClip(clip))
    }
}

fn write_meta(m: &LoroMap, meta: &Meta) -> CmdResult {
    m.insert("name", meta.name.as_str())?;
    m.insert("fps_num", meta.timebase.num as i64)?;
    m.insert("fps_den", meta.timebase.den as i64)?;
    m.insert("width", meta.width as i64)?;
    m.insert("height", meta.height as i64)?;
    m.insert("sample_rate", meta.sample_rate as i64)?;
    m.insert("color_space", meta.color_space.as_str())?;
    m.insert("schema_version", meta.schema_version)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Project / media
// ---------------------------------------------------------------------------

/// Initialises a new project: meta plus one sequence with V1 and A1.
#[derive(Debug, Clone)]
pub struct InitProject {
    pub meta: Meta,
    pub seq: SeqId,
    pub video_track: TrackId,
    pub audio_track: TrackId,
}

impl InitProject {
    pub fn new(meta: Meta) -> Self {
        InitProject {
            meta,
            seq: SeqId::new(),
            video_track: TrackId::new(),
            audio_track: TrackId::new(),
        }
    }
}

impl Command for InitProject {
    fn name(&self) -> &'static str {
        "init_project"
    }
    fn apply(&self, tx: &mut Tx<'_>) -> CmdResult {
        if tx.snapshot().main_sequence.is_some() {
            return Err(CommandError::Invalid("project already initialised".into()));
        }
        let meta = Meta {
            schema_version: SCHEMA_VERSION,
            ..self.meta.clone()
        };
        let m = tx.meta();
        write_meta(&m, &meta)?;
        m.insert("main_seq", self.seq.key())?;
        CreateSequence {
            id: self.seq,
            name: "Main".into(),
        }
        .apply(tx)?;
        for (id, kind, name) in [
            (self.video_track, TrackKind::Video, "Video 1"),
            (self.audio_track, TrackKind::Audio, "Audio 1"),
        ] {
            write_track(tx, self.seq, id, kind, name, None)?;
        }
        Ok(())
    }
}

/// Updates project settings (name, timebase, resolution, ...).
#[derive(Debug, Clone)]
pub struct SetMeta(pub Meta);

impl Command for SetMeta {
    fn name(&self) -> &'static str {
        "set_meta"
    }
    fn apply(&self, tx: &mut Tx<'_>) -> CmdResult {
        if self.0.width == 0 || self.0.height == 0 || self.0.sample_rate == 0 {
            return Err(CommandError::Invalid(
                "zero dimension or sample rate".into(),
            ));
        }
        write_meta(&tx.meta(), &self.0)
    }
}

/// Renames the project.
#[derive(Debug, Clone)]
pub struct RenameProject(pub String);

impl Command for RenameProject {
    fn name(&self) -> &'static str {
        "rename_project"
    }
    fn apply(&self, tx: &mut Tx<'_>) -> CmdResult {
        tx.meta().insert("name", self.0.as_str())?;
        Ok(())
    }
}

/// Adds (or refreshes) a media entry. Idempotent per hash.
#[derive(Debug, Clone)]
pub struct AddMedia(pub MediaEntry);

impl Command for AddMedia {
    fn name(&self) -> &'static str {
        "add_media"
    }
    fn apply(&self, tx: &mut Tx<'_>) -> CmdResult {
        let e = &self.0;
        let m = tx.media().ensure_mergeable_map(&e.id.to_hex())?;
        m.insert("name", e.name.as_str())?;
        m.insert("kind", e.kind.as_str())?;
        m.insert("duration_ticks", e.duration.0)?;
        if let Some(fps) = e.fps {
            m.insert("fps_num", fps.num as i64)?;
            m.insert("fps_den", fps.den as i64)?;
        }
        m.insert("w", e.width as i64)?;
        m.insert("h", e.height as i64)?;
        m.insert("audio_ch", e.audio_channels as i64)?;
        m.insert("audio_rate", e.audio_sample_rate as i64)?;
        m.insert("size_bytes", e.size_bytes as i64)?;
        if let Some(p) = e.proxy_hash {
            m.insert("proxy_hash", p.to_hex())?;
        }
        Ok(())
    }
}

/// Renames a media entry (display name only; content is addressed by hash).
#[derive(Debug, Clone)]
pub struct RenameMedia {
    pub id: MediaId,
    pub name: String,
}

impl Command for RenameMedia {
    fn name(&self) -> &'static str {
        "rename_media"
    }
    fn apply(&self, tx: &mut Tx<'_>) -> CmdResult {
        if !tx.snapshot().media.contains_key(&self.id) {
            return Err(CommandError::NoMedia(self.id));
        }
        tx.media()
            .ensure_mergeable_map(&self.id.to_hex())?
            .insert("name", self.name.as_str())?;
        Ok(())
    }
}

/// Removes a media entry. Refuses while clips still reference it.
#[derive(Debug, Clone)]
pub struct RemoveMedia(pub MediaId);

impl Command for RemoveMedia {
    fn name(&self) -> &'static str {
        "remove_media"
    }
    fn apply(&self, tx: &mut Tx<'_>) -> CmdResult {
        let snap = tx.snapshot();
        if !snap.media.contains_key(&self.0) {
            return Err(CommandError::NoMedia(self.0));
        }
        if snap
            .sequences
            .values()
            .flat_map(|s| s.clips.values())
            .any(|c| c.media_id() == Some(self.0))
        {
            return Err(CommandError::Invalid("media is used on a timeline".into()));
        }
        tx.media().delete(&self.0.to_hex())?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Sequences & tracks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CreateSequence {
    pub id: SeqId,
    pub name: String,
}

impl Command for CreateSequence {
    fn name(&self) -> &'static str {
        "create_sequence"
    }
    fn apply(&self, tx: &mut Tx<'_>) -> CmdResult {
        let s = tx.seq_map(self.id)?;
        s.insert("name", self.name.as_str())?;
        s.ensure_mergeable_movable_list("tracks")?;
        s.ensure_mergeable_map("track_props")?;
        s.ensure_mergeable_map("clips")?;
        s.ensure_mergeable_map("transitions")?;
        s.ensure_mergeable_map("markers")?;
        Ok(())
    }
}

fn write_track(
    tx: &Tx<'_>,
    seq: SeqId,
    id: TrackId,
    kind: TrackKind,
    name: &str,
    index: Option<usize>,
) -> CmdResult {
    let p = tx.track_props(seq, id)?;
    p.insert("kind", kind.as_str())?;
    p.insert("name", name)?;
    p.insert("locked", false)?;
    p.insert("hidden", false)?;
    p.insert("muted", false)?;
    p.insert("height", 56.0)?;
    let order = tx.track_order(seq)?;
    let idx = index.unwrap_or(order.len()).min(order.len());
    order.insert(idx, id.key())?;
    Ok(())
}

/// Adds a track. Video tracks are inserted above the topmost video track,
/// audio tracks after the last audio track, unless `index` is given.
#[derive(Debug, Clone)]
pub struct AddTrack {
    pub seq: SeqId,
    pub id: TrackId,
    pub kind: TrackKind,
    pub name: Option<String>,
    pub index: Option<usize>,
}

impl Command for AddTrack {
    fn name(&self) -> &'static str {
        "add_track"
    }
    fn apply(&self, tx: &mut Tx<'_>) -> CmdResult {
        let seq = tx.seq_read(self.seq)?;
        if seq.track(self.id).is_some() {
            return Err(CommandError::Invalid("track exists".into()));
        }
        let same: Vec<_> = seq
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.kind.is_visual() == self.kind.is_visual())
            .collect();
        let n = same.len() + 1;
        let default_name = match self.kind {
            TrackKind::Video => format!("Video {n}"),
            TrackKind::Audio => format!("Audio {n}"),
            TrackKind::Title => format!("Title {n}"),
        };
        let index = self
            .index
            .or_else(|| same.last().map(|(i, _)| i + 1))
            .unwrap_or(if self.kind.is_visual() {
                0
            } else {
                seq.tracks.len()
            });
        write_track(
            tx,
            self.seq,
            self.id,
            self.kind,
            self.name.as_deref().unwrap_or(&default_name),
            Some(index),
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct TrackPatch {
    pub name: Option<String>,
    pub locked: Option<bool>,
    pub hidden: Option<bool>,
    pub muted: Option<bool>,
    pub height: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct SetTrackProps {
    pub seq: SeqId,
    pub track: TrackId,
    pub patch: TrackPatch,
}

impl Command for SetTrackProps {
    fn name(&self) -> &'static str {
        "set_track_props"
    }
    fn apply(&self, tx: &mut Tx<'_>) -> CmdResult {
        let seq = tx.seq_read(self.seq)?;
        seq.track(self.track)
            .ok_or(CommandError::NoTrack(self.track))?;
        let p = tx.track_props(self.seq, self.track)?;
        let pa = &self.patch;
        if let Some(v) = &pa.name {
            p.insert("name", v.as_str())?;
        }
        if let Some(v) = pa.locked {
            p.insert("locked", v)?;
        }
        if let Some(v) = pa.hidden {
            p.insert("hidden", v)?;
        }
        if let Some(v) = pa.muted {
            p.insert("muted", v)?;
        }
        if let Some(v) = pa.height {
            p.insert("height", v.clamp(24.0, 240.0) as f64)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Clips
// ---------------------------------------------------------------------------

/// Everything needed to place a new clip.
#[derive(Debug, Clone)]
pub struct NewClip {
    pub id: ClipId,
    pub name: String,
    pub track: TrackId,
    pub start: Ticks,
    pub in_tick: Ticks,
    pub out_tick: Ticks,
    pub source: ClipSource,
}

#[derive(Debug, Clone)]
pub struct AddClip {
    pub seq: SeqId,
    pub clip: NewClip,
}

fn write_new_clip(tx: &Tx<'_>, seq: SeqId, c: &NewClip, clock: i64) -> CmdResult {
    let m = tx.clip_map(seq, c.id)?;
    m.insert("name", c.name.as_str())?;
    m.insert("in_tick", c.in_tick.0)?;
    m.insert("out_tick", c.out_tick.0)?;
    match &c.source {
        ClipSource::Media(id) => {
            m.insert("source_kind", "media")?;
            m.insert("media_id", id.to_hex())?;
        }
        ClipSource::Generator(Generator::Title) => m.insert("source_kind", "title")?,
        ClipSource::Generator(Generator::Adjustment) => m.insert("source_kind", "adjustment")?,
        ClipSource::Generator(Generator::Color(rgba)) => {
            m.insert("source_kind", "color")?;
            let b = rgba.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
            m.insert("color_rgba", u32::from_be_bytes(b) as i64)?;
        }
    }
    m.insert("reversed", false)?;
    m.insert("enabled", true)?;
    let sp = m.ensure_mergeable_map("speed")?;
    sp.insert("mode", "uniform")?;
    sp.insert("num", 1i64)?;
    sp.insert("den", 1i64)?;
    write_transform(&m.ensure_mergeable_map("transform")?, &Transform::default())?;
    write_audio(&m.ensure_mergeable_map("audio")?, &ClipAudio::default())?;
    m.ensure_mergeable_movable_list("effects")?;
    m.ensure_mergeable_map("keyframes")?;
    tx.write_position(seq, c.id, c.track, c.start, clock)
}

fn write_transform(m: &LoroMap, t: &Transform) -> CmdResult {
    m.insert("x", t.x)?;
    m.insert("y", t.y)?;
    m.insert("scale_x", t.scale_x)?;
    m.insert("scale_y", t.scale_y)?;
    m.insert("rotation", t.rotation)?;
    m.insert("anchor_x", t.anchor_x)?;
    m.insert("anchor_y", t.anchor_y)?;
    m.insert("opacity", t.opacity)?;
    m.insert("blend", t.blend.as_str())?;
    m.insert("flip_h", t.flip_h)?;
    m.insert("flip_v", t.flip_v)?;
    Ok(())
}

fn write_audio(m: &LoroMap, a: &ClipAudio) -> CmdResult {
    m.insert("gain_db", a.gain_db)?;
    m.insert("pan", a.pan)?;
    m.insert("fade_in", a.fade_in.0)?;
    m.insert("fade_out", a.fade_out.0)?;
    m.insert("duck", false)?;
    Ok(())
}

impl Command for AddClip {
    fn name(&self) -> &'static str {
        "add_clip"
    }
    fn apply(&self, tx: &mut Tx<'_>) -> CmdResult {
        let seq = tx.seq_read(self.seq)?;
        tx.check_track_unlocked(seq, self.clip.track)?;
        if seq.clips.contains_key(&self.clip.id) {
            return Err(CommandError::Invalid("clip id exists".into()));
        }
        validate::new_clip(tx.snapshot(), seq, &self.clip)?;
        let clock = tx.next_pos_clock(self.seq);
        write_new_clip(tx, self.seq, &self.clip, clock)
    }
}

/// Moves one or more clips (LWW write of `(track_id, start_tick)` per clip).
#[derive(Debug, Clone)]
pub struct MoveClips {
    pub seq: SeqId,
    pub moves: Vec<(ClipId, TrackId, Ticks)>,
}

impl Command for MoveClips {
    fn name(&self) -> &'static str {
        "move_clips"
    }
    fn apply(&self, tx: &mut Tx<'_>) -> CmdResult {
        let seq = tx.seq_read(self.seq)?;
        for (clip, track, start) in &self.moves {
            let c = tx.require_clip(seq, *clip)?;
            tx.check_track_unlocked(seq, c.track_id)?;
            tx.check_track_unlocked(seq, *track)?;
            let tk = seq.track(*track).ok_or(CommandError::NoTrack(*track))?;
            validate::track_accepts(tx.snapshot(), tk.kind, &c.source)?;
            if start.0 < 0 {
                return Err(CommandError::Invalid("negative start".into()));
            }
        }
        let clock = tx.next_pos_clock(self.seq);
        for (clip, track, start) in &self.moves {
            tx.write_position(self.seq, *clip, *track, *start, clock)?;
        }
        Ok(())
    }
}

/// Sets a clip's in/out points and start (trim from either edge).
#[derive(Debug, Clone)]
pub struct TrimClip {
    pub seq: SeqId,
    pub clip: ClipId,
    pub start: Ticks,
    pub in_tick: Ticks,
    pub out_tick: Ticks,
}

impl Command for TrimClip {
    fn name(&self) -> &'static str {
        "trim_clip"
    }
    fn apply(&self, tx: &mut Tx<'_>) -> CmdResult {
        let seq = tx.seq_read(self.seq)?;
        let c = tx.require_clip(seq, self.clip)?;
        tx.check_track_unlocked(seq, c.track_id)?;
        validate::source_range(tx.snapshot(), &c.source, self.in_tick, self.out_tick)?;
        if self.start.0 < 0 {
            return Err(CommandError::Invalid("negative start".into()));
        }
        let m = tx.clip_map(self.seq, self.clip)?;
        m.insert("in_tick", self.in_tick.0)?;
        m.insert("out_tick", self.out_tick.0)?;
        if self.start != c.start {
            let clock = tx.next_pos_clock(self.seq);
            tx.write_position(self.seq, self.clip, c.track_id, self.start, clock)?;
        }
        Ok(())
    }
}

/// Splits a clip at a timeline tick. The right half gets `new_id`.
#[derive(Debug, Clone)]
pub struct SplitClip {
    pub seq: SeqId,
    pub clip: ClipId,
    pub at: Ticks,
    pub new_id: ClipId,
}

impl Command for SplitClip {
    fn name(&self) -> &'static str {
        "split_clip"
    }
    fn apply(&self, tx: &mut Tx<'_>) -> CmdResult {
        let seq = tx.seq_read(self.seq)?;
        let c = tx.require_clip(seq, self.clip)?.clone();
        tx.check_track_unlocked(seq, c.track_id)?;
        if self.at <= c.start || self.at >= c.end() {
            return Err(CommandError::Invalid("split point outside clip".into()));
        }
        if c.reversed {
            return Err(CommandError::Invalid(
                "splitting reversed clips lands in v1".into(),
            ));
        }
        let src_split = c.source_tick_at(self.at);
        let m = tx.clip_map(self.seq, self.clip)?;
        m.insert("out_tick", src_split.0)?;
        let right = NewClip {
            id: self.new_id,
            name: c.name.clone(),
            track: c.track_id,
            start: self.at,
            in_tick: src_split,
            out_tick: c.out_tick,
            source: c.source.clone(),
        };
        let clock = tx.next_pos_clock(self.seq);
        write_new_clip(tx, self.seq, &right, clock)?;
        // Carry over the left clip's look and sound.
        let r = tx.clip_map(self.seq, self.new_id)?;
        write_transform(&r.ensure_mergeable_map("transform")?, &c.transform)?;
        write_audio(&r.ensure_mergeable_map("audio")?, &c.audio)?;
        let sp = r.ensure_mergeable_map("speed")?;
        let Speed { num, den } = c.speed;
        sp.insert("num", num)?;
        sp.insert("den", den)?;
        r.insert("enabled", c.enabled)?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct DeleteClips {
    pub seq: SeqId,
    pub clips: Vec<ClipId>,
}

impl Command for DeleteClips {
    fn name(&self) -> &'static str {
        "delete_clips"
    }
    fn apply(&self, tx: &mut Tx<'_>) -> CmdResult {
        let seq = tx.seq_read(self.seq)?;
        for id in &self.clips {
            let c = tx.require_clip(seq, *id)?;
            tx.check_track_unlocked(seq, c.track_id)?;
        }
        let m = tx.clips_map(self.seq)?;
        for id in &self.clips {
            m.delete(&id.key())?;
        }
        Ok(())
    }
}

/// A single transform field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransformField {
    X(f64),
    Y(f64),
    ScaleX(f64),
    ScaleY(f64),
    Rotation(f64),
    Opacity(f64),
    FlipH(bool),
    FlipV(bool),
    Blend(crate::model::BlendMode),
}

#[derive(Debug, Clone)]
pub struct SetTransform {
    pub seq: SeqId,
    pub clip: ClipId,
    pub fields: Vec<TransformField>,
}

impl Command for SetTransform {
    fn name(&self) -> &'static str {
        "set_transform"
    }
    fn apply(&self, tx: &mut Tx<'_>) -> CmdResult {
        let seq = tx.seq_read(self.seq)?;
        let c = tx.require_clip(seq, self.clip)?;
        tx.check_track_unlocked(seq, c.track_id)?;
        for f in &self.fields {
            let finite = match f {
                TransformField::X(v)
                | TransformField::Y(v)
                | TransformField::ScaleX(v)
                | TransformField::ScaleY(v)
                | TransformField::Rotation(v)
                | TransformField::Opacity(v) => v.is_finite(),
                _ => true,
            };
            if !finite {
                return Err(CommandError::Invalid("non-finite transform".into()));
            }
        }
        let m = tx
            .clip_map(self.seq, self.clip)?
            .ensure_mergeable_map("transform")?;
        for f in &self.fields {
            let (k, v): (&str, LoroValue) = match *f {
                TransformField::X(v) => ("x", v.into()),
                TransformField::Y(v) => ("y", v.into()),
                TransformField::ScaleX(v) => ("scale_x", v.into()),
                TransformField::ScaleY(v) => ("scale_y", v.into()),
                TransformField::Rotation(v) => ("rotation", v.into()),
                TransformField::Opacity(v) => ("opacity", v.clamp(0.0, 1.0).into()),
                TransformField::FlipH(v) => ("flip_h", v.into()),
                TransformField::FlipV(v) => ("flip_v", v.into()),
                TransformField::Blend(b) => ("blend", b.as_str().into()),
            };
            m.insert(k, v)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ResetTransform {
    pub seq: SeqId,
    pub clip: ClipId,
}

impl Command for ResetTransform {
    fn name(&self) -> &'static str {
        "reset_transform"
    }
    fn apply(&self, tx: &mut Tx<'_>) -> CmdResult {
        let seq = tx.seq_read(self.seq)?;
        let c = tx.require_clip(seq, self.clip)?;
        tx.check_track_unlocked(seq, c.track_id)?;
        write_transform(
            &tx.clip_map(self.seq, self.clip)?
                .ensure_mergeable_map("transform")?,
            &Transform::default(),
        )
    }
}

#[derive(Debug, Clone)]
pub struct SetClipAudio {
    pub seq: SeqId,
    pub clip: ClipId,
    pub gain_db: Option<f64>,
    pub pan: Option<f64>,
}

impl Command for SetClipAudio {
    fn name(&self) -> &'static str {
        "set_clip_audio"
    }
    fn apply(&self, tx: &mut Tx<'_>) -> CmdResult {
        let seq = tx.seq_read(self.seq)?;
        let c = tx.require_clip(seq, self.clip)?;
        tx.check_track_unlocked(seq, c.track_id)?;
        let m = tx
            .clip_map(self.seq, self.clip)?
            .ensure_mergeable_map("audio")?;
        if let Some(g) = self.gain_db {
            m.insert("gain_db", g.clamp(-96.0, 24.0))?;
        }
        if let Some(p) = self.pan {
            m.insert("pan", p.clamp(-1.0, 1.0))?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Markers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AddMarker {
    pub seq: SeqId,
    pub id: MarkerId,
    pub tick: Ticks,
    pub color: MarkerColor,
    pub name: String,
    pub author: String,
}

impl Command for AddMarker {
    fn name(&self) -> &'static str {
        "add_marker"
    }
    fn apply(&self, tx: &mut Tx<'_>) -> CmdResult {
        tx.seq_read(self.seq)?;
        let m = tx.marker_map(self.seq, self.id)?;
        m.insert("tick", self.tick.0)?;
        m.insert("color", self.color.index() as i64)?;
        m.insert("name", self.name.as_str())?;
        m.insert("comment", "")?;
        m.insert("author", self.author.as_str())?;
        m.insert("resolved", false)?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct EditMarker {
    pub seq: SeqId,
    pub id: MarkerId,
    pub name: Option<String>,
    pub comment: Option<String>,
    pub color: Option<MarkerColor>,
    pub resolved: Option<bool>,
}

impl Command for EditMarker {
    fn name(&self) -> &'static str {
        "edit_marker"
    }
    fn apply(&self, tx: &mut Tx<'_>) -> CmdResult {
        let seq = tx.seq_read(self.seq)?;
        if !seq.markers.contains_key(&self.id) {
            return Err(CommandError::NoMarker(self.id));
        }
        let m = tx.marker_map(self.seq, self.id)?;
        if let Some(v) = &self.name {
            m.insert("name", v.as_str())?;
        }
        if let Some(v) = &self.comment {
            m.insert("comment", v.as_str())?;
        }
        if let Some(v) = self.color {
            m.insert("color", v.index() as i64)?;
        }
        if let Some(v) = self.resolved {
            m.insert("resolved", v)?;
        }
        Ok(())
    }
}
