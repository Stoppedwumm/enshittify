//! Timeline audio renderer: decodes and mixes every audible clip.
//!
//! Used by the realtime engine (on a feeder thread) and by export (offline),
//! so preview and export sound identical.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use cutroom_core::model::{Clip, DocSnapshot, MediaKind, Speed, TrackKind};
use cutroom_core::{ClipId, MediaId, SeqId, Ticks};
use cutroom_media::{AudioDecoder, AudioFormat, MediaBackend};

use crate::mixer::{db_to_gain, mix_stereo, pan_gains};

/// Resolves a content hash to a file on this machine.
pub type PathResolver = Arc<dyn Fn(MediaId) -> Option<PathBuf> + Send + Sync>;

struct ClipDecoder {
    dec: Box<dyn AudioDecoder>,
    /// Source sample the decoder will return next.
    next: i64,
}

pub struct TimelineAudio {
    snap: Arc<DocSnapshot>,
    seq: SeqId,
    format: AudioFormat,
    backend: Arc<dyn MediaBackend>,
    resolve: PathResolver,
    /// Output sample index on the timeline of the next `fill`.
    pos: i64,
    decoders: HashMap<ClipId, ClipDecoder>,
    failed: HashMap<ClipId, ()>,
    scratch: Vec<f32>,
}

impl TimelineAudio {
    pub fn new(
        snap: Arc<DocSnapshot>,
        seq: SeqId,
        format: AudioFormat,
        backend: Arc<dyn MediaBackend>,
        resolve: PathResolver,
        from: Ticks,
    ) -> Self {
        assert_eq!(format.channels, 2, "mixer bus is stereo");
        TimelineAudio {
            snap,
            seq,
            format,
            backend,
            resolve,
            pos: from.to_samples(format.sample_rate),
            decoders: HashMap::new(),
            failed: HashMap::new(),
            scratch: Vec::new(),
        }
    }

    pub fn format(&self) -> AudioFormat {
        self.format
    }

    pub fn position(&self) -> Ticks {
        Ticks::from_samples(self.pos, self.format.sample_rate)
    }

    fn audible(&self, c: &Clip) -> bool {
        let Some(seq) = self.snap.sequences.get(&self.seq) else {
            return false;
        };
        let Some(track) = seq.track(c.track_id) else {
            return false;
        };
        if !c.enabled || track.muted || c.speed != Speed::default() || c.reversed {
            // Non-unit speed audio (with pitch handling) lands in v1.
            return false;
        }
        let Some(m) = c.media_id().and_then(|id| self.snap.media.get(&id)) else {
            return false;
        };
        m.audio_channels > 0
            && m.kind != MediaKind::Image
            && matches!(track.kind, TrackKind::Audio | TrackKind::Video)
    }

    /// Renders the next `out.len() / 2` stereo frames.
    pub fn fill(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let rate = self.format.sample_rate;
        let frames = (out.len() / 2) as i64;
        let (b0, b1) = (self.pos, self.pos + frames);
        let clips: Vec<Clip> = self
            .snap
            .sequences
            .get(&self.seq)
            .map(|s| {
                s.clips
                    .values()
                    .filter(|c| c.start.to_samples(rate) < b1 && c.end().to_samples(rate) > b0)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let live: Vec<ClipId> = clips.iter().map(|c| c.id).collect();
        self.decoders.retain(|id, _| live.contains(id));
        let clips: Vec<Clip> = clips.into_iter().filter(|c| self.audible(c)).collect();

        for c in &clips {
            let cs = c.start.to_samples(rate);
            let ce = c.end().to_samples(rate);
            let (s0, s1) = (b0.max(cs), b1.min(ce));
            if s1 <= s0 {
                continue;
            }
            let src_start = c.in_tick.to_samples(rate) + (s0 - cs);
            let n = (s1 - s0) as usize;
            self.scratch.resize(n * 2, 0.0);
            if !self.read_clip(c, src_start, n) {
                continue;
            }
            let g = db_to_gain(c.audio.gain_db);
            let fade = |t: i64| -> f32 {
                let fi = c.audio.fade_in.to_samples(rate);
                let fo = c.audio.fade_out.to_samples(rate);
                let mut v = 1.0f32;
                if fi > 0 && t - cs < fi {
                    v *= (t - cs) as f32 / fi as f32;
                }
                if fo > 0 && ce - t < fo {
                    v *= (ce - t) as f32 / fo as f32;
                }
                v.clamp(0.0, 1.0)
            };
            let off = ((s0 - b0) * 2) as usize;
            mix_stereo(
                &mut out[off..off + n * 2],
                &self.scratch[..n * 2],
                g * fade(s0),
                g * fade(s1),
                pan_gains(c.audio.pan),
            );
        }
        self.pos = b1;
    }

    /// Reads `n` frames of `c` starting at source sample `src` into `scratch`.
    fn read_clip(&mut self, c: &Clip, src: i64, n: usize) -> bool {
        if self.failed.contains_key(&c.id) {
            return false;
        }
        if !self.decoders.contains_key(&c.id) {
            let Some(path) = c.media_id().and_then(|m| (self.resolve)(m)) else {
                return false;
            };
            match self.backend.open_audio(&path, self.format) {
                Ok(dec) => {
                    self.decoders.insert(c.id, ClipDecoder { dec, next: 0 });
                }
                Err(e) => {
                    tracing::warn!("audio open failed for {}: {e}", path.display());
                    self.failed.insert(c.id, ());
                    return false;
                }
            }
        }
        let cd = self.decoders.get_mut(&c.id).expect("inserted");
        if cd.next != src {
            if let Err(e) = cd.dec.seek(src) {
                tracing::warn!("audio seek failed: {e}");
                return false;
            }
            cd.next = src;
        }
        let mut filled = 0;
        while filled < n {
            match cd.dec.read(&mut self.scratch[filled * 2..n * 2]) {
                Ok(0) => break,
                Ok(k) => filled += k,
                Err(e) => {
                    tracing::warn!("audio decode failed: {e}");
                    break;
                }
            }
        }
        self.scratch[filled * 2..n * 2].fill(0.0);
        cd.next = src + n as i64;
        true
    }
}
