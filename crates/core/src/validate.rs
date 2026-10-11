//! Validation shared by commands and by the import path.

use crate::command::{CommandError, NewClip};
use crate::model::{ClipSource, DocSnapshot, Generator, MediaKind, Sequence, TrackKind};
use crate::time::Ticks;

/// Whether a track of `kind` can hold a clip with `source`.
pub fn track_accepts(
    snap: &DocSnapshot,
    kind: TrackKind,
    source: &ClipSource,
) -> Result<(), CommandError> {
    let ok = match (kind, source) {
        (TrackKind::Title, ClipSource::Generator(Generator::Title)) => true,
        (TrackKind::Video, ClipSource::Generator(_)) => true,
        (TrackKind::Video, ClipSource::Media(id)) => {
            let m = snap.media.get(id).ok_or(CommandError::NoMedia(*id))?;
            matches!(m.kind, MediaKind::Video | MediaKind::Image)
        }
        (TrackKind::Audio, ClipSource::Media(id)) => {
            let m = snap.media.get(id).ok_or(CommandError::NoMedia(*id))?;
            m.audio_channels > 0
        }
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(CommandError::Invalid(format!(
            "{} track cannot hold this clip",
            kind.as_str()
        )))
    }
}

/// Checks `[in, out)` against the source's extent. Stills and generators have
/// unbounded extent.
pub fn source_range(
    snap: &DocSnapshot,
    source: &ClipSource,
    in_tick: Ticks,
    out_tick: Ticks,
) -> Result<(), CommandError> {
    if in_tick.0 < 0 || out_tick <= in_tick {
        return Err(CommandError::Invalid(format!(
            "bad source range {}..{}",
            in_tick.0, out_tick.0
        )));
    }
    if let ClipSource::Media(id) = source {
        let m = snap.media.get(id).ok_or(CommandError::NoMedia(*id))?;
        if m.kind != MediaKind::Image && m.duration.0 > 0 && out_tick > m.duration {
            return Err(CommandError::Invalid("out point beyond media end".into()));
        }
    }
    Ok(())
}

pub fn new_clip(snap: &DocSnapshot, seq: &Sequence, c: &NewClip) -> Result<(), CommandError> {
    let track = seq.track(c.track).ok_or(CommandError::NoTrack(c.track))?;
    track_accepts(snap, track.kind, &c.source)?;
    source_range(snap, &c.source, c.in_tick, c.out_tick)?;
    if c.start.0 < 0 {
        return Err(CommandError::Invalid("negative start".into()));
    }
    Ok(())
}
