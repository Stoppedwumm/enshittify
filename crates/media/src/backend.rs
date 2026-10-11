//! Backend abstraction over FFmpeg flavours.

use std::path::Path;

use crate::frame::{AudioFormat, VideoFrame};
use crate::info::MediaInfo;

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error(
        "ffmpeg executables not found (looked for {0}); install FFmpeg or set CUTROOM_FFMPEG_DIR"
    )]
    NotFound(String),
    #[error("probe failed: {0}")]
    Probe(String),
    #[error("decode failed: {0}")]
    Decode(String),
    #[error("no {0} stream")]
    NoStream(&'static str),
    #[error("unsupported: {0}")]
    Unsupported(String),
}

/// Output size control for video decoders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct DecodeOptions {
    /// Bound the output to this box, preserving aspect ratio. `None` = native size.
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
}

impl DecodeOptions {
    pub const FULL: DecodeOptions = DecodeOptions {
        max_width: None,
        max_height: None,
    };

    pub fn bounded(w: u32, h: u32) -> Self {
        DecodeOptions {
            max_width: Some(w),
            max_height: Some(h),
        }
    }

    /// Output dimensions for a `w×h` source, rounded to even numbers
    /// (required by most YUV pipelines).
    pub fn output_size(&self, w: u32, h: u32) -> (u32, u32) {
        let (w, h) = (w.max(1), h.max(1));
        let sx = self.max_width.map(|m| m as f64 / w as f64).unwrap_or(1.0);
        let sy = self.max_height.map(|m| m as f64 / h as f64).unwrap_or(1.0);
        let s = sx.min(sy).min(1.0);
        let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
        if s >= 1.0 {
            (w, h)
        } else {
            (even(w as f64 * s), even(h as f64 * s))
        }
    }
}

/// Sequential video decoder with exact frame seeking.
pub trait VideoDecoder: Send {
    /// Output width/height after scaling.
    fn output_size(&self) -> (u32, u32);

    /// Positions the decoder so that the next [`decode_next`](Self::decode_next)
    /// returns frame `index` (seeking to the preceding keyframe internally).
    fn seek(&mut self, index: i64) -> Result<(), crate::MediaError>;

    /// Next frame in presentation order; `None` at end of stream.
    fn decode_next(&mut self) -> Result<Option<VideoFrame>, crate::MediaError>;

    /// Convenience: seek + decode one frame.
    fn frame_at(&mut self, index: i64) -> Result<Option<VideoFrame>, crate::MediaError> {
        self.seek(index)?;
        self.decode_next()
    }
}

/// Sequential audio decoder producing interleaved f32 in a fixed format.
pub trait AudioDecoder: Send {
    fn format(&self) -> AudioFormat;
    /// Positions at the given output sample frame.
    fn seek(&mut self, sample: i64) -> Result<(), crate::MediaError>;
    /// Fills `out` (interleaved) and returns the number of sample *frames*
    /// written; 0 at end of stream.
    fn read(&mut self, out: &mut [f32]) -> Result<usize, crate::MediaError>;
}

pub trait MediaBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn probe(&self, path: &Path) -> Result<MediaInfo, MediaError>;
    fn open_video(
        &self,
        path: &Path,
        info: &MediaInfo,
        opts: DecodeOptions,
    ) -> Result<Box<dyn VideoDecoder>, MediaError>;
    fn open_audio(
        &self,
        path: &Path,
        format: AudioFormat,
    ) -> Result<Box<dyn AudioDecoder>, MediaError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_size_preserves_aspect() {
        assert_eq!(DecodeOptions::FULL.output_size(1920, 1080), (1920, 1080));
        assert_eq!(
            DecodeOptions::bounded(320, 320).output_size(1920, 1080),
            (320, 180)
        );
        assert_eq!(
            DecodeOptions::bounded(960, 540).output_size(1280, 720),
            (960, 540)
        );
        assert_eq!(
            DecodeOptions::bounded(4000, 4000).output_size(640, 480),
            (640, 480)
        );
    }
}
