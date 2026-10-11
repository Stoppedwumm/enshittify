//! Deterministic render-order view over a sequence.
//!
//! Overlapping clips on one track are legal in the data (two peers can move
//! clips into the same spot concurrently). `normalize()` decides, identically
//! on every replica, which clip is visible where: the clip with the higher
//! position clock (Lamport) wins; the peer id breaks ties, then the clip id.

use crate::ids::{ClipId, TrackId};
use crate::model::{Clip, Sequence};
use crate::time::Ticks;

/// A visible stretch of a clip on its track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub clip: ClipId,
    pub start: Ticks,
    pub end: Ticks,
}

/// A time range where two or more clips on the same track overlap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub track: TrackId,
    pub start: Ticks,
    pub end: Ticks,
    /// All clips involved, winner first.
    pub clips: Vec<ClipId>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrackView {
    pub segments: Vec<Segment>,
    pub conflicts: Vec<Conflict>,
}

fn priority(c: &Clip) -> (i64, u64, ClipId) {
    (c.pos_clock, c.pos_peer, c.id)
}

/// Computes the visible segments and conflicts of one track.
pub fn normalize_track(seq: &Sequence, track: TrackId) -> TrackView {
    let clips: Vec<&Clip> = seq.clips_on(track).filter(|c| c.duration().0 > 0).collect();
    // Boundaries of every clip, sorted & deduplicated.
    let mut cuts: Vec<Ticks> = clips.iter().flat_map(|c| [c.start, c.end()]).collect();
    cuts.sort();
    cuts.dedup();
    let mut view = TrackView::default();
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let mut covering: Vec<&Clip> = clips.iter().copied().filter(|c| c.contains(a)).collect();
        if covering.is_empty() {
            continue;
        }
        covering.sort_by_key(|c| std::cmp::Reverse(priority(c)));
        let winner = covering[0].id;
        match view.segments.last_mut() {
            Some(s) if s.clip == winner && s.end == a => s.end = b,
            _ => view.segments.push(Segment {
                clip: winner,
                start: a,
                end: b,
            }),
        }
        if covering.len() > 1 {
            let ids: Vec<ClipId> = covering.iter().map(|c| c.id).collect();
            match view.conflicts.last_mut() {
                Some(c) if c.end == a && c.clips == ids => c.end = b,
                _ => view.conflicts.push(Conflict {
                    track,
                    start: a,
                    end: b,
                    clips: ids,
                }),
            }
        }
    }
    view
}

/// The clip visible at `t` on `track`, if any.
pub fn visible_clip_at(seq: &Sequence, track: TrackId, t: Ticks) -> Option<&Clip> {
    seq.clips_on(track)
        .filter(|c| c.enabled && c.contains(t))
        .max_by_key(|c| priority(c))
}
