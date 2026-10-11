//! Probe results.

use cutroom_core::model::{MediaEntry, MediaKind};
use cutroom_core::{MediaId, Ticks, Timebase};

/// Everything Cutroom needs to know about a media file.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaInfo {
    pub kind: MediaKind,
    pub duration: Ticks,
    pub fps: Option<Timebase>,
    pub width: u32,
    pub height: u32,
    pub frame_count: Option<i64>,
    pub video_codec: Option<String>,
    pub pix_fmt: Option<String>,
    /// e.g. `bt709`, `bt470bg`, `bt2020nc`.
    pub color_space: Option<String>,
    /// `tv` (limited) or `pc` (full).
    pub color_range: Option<String>,
    /// Display rotation in degrees from container side data.
    pub rotation: i32,
    pub audio_codec: Option<String>,
    pub audio_channels: u32,
    pub audio_sample_rate: u32,
    pub size_bytes: u64,
}

impl MediaInfo {
    pub fn has_video(&self) -> bool {
        matches!(self.kind, MediaKind::Video | MediaKind::Image)
    }

    pub fn has_audio(&self) -> bool {
        self.audio_channels > 0
    }

    /// Converts to the shared-document entry (no paths).
    pub fn to_entry(&self, id: MediaId, name: &str) -> MediaEntry {
        MediaEntry {
            id,
            name: name.to_string(),
            kind: self.kind,
            duration: self.duration,
            fps: self.fps,
            width: self.width,
            height: self.height,
            audio_channels: self.audio_channels,
            audio_sample_rate: self.audio_sample_rate,
            size_bytes: self.size_bytes,
            proxy_hash: None,
        }
    }

    /// Number of whole frames, derived from the duration when the container
    /// does not report it.
    pub fn frames(&self) -> i64 {
        match (self.frame_count, self.fps) {
            (Some(n), _) if n > 0 => n,
            (_, Some(fps)) => fps.ticks_to_frame(self.duration).max(1),
            _ => 1,
        }
    }
}
