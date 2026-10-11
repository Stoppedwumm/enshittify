//! Cutroom media layer.
//!
//! * [`hash`] — BLAKE3 content addressing ([`cutroom_core::MediaId`]).
//! * [`backend`] — the [`MediaBackend`] trait plus two implementations:
//!   [`cli::CliBackend`] (spawns the `ffprobe`/`ffmpeg` executables; works on
//!   any machine with FFmpeg on `PATH`, no build-time dependency) and, behind
//!   the `ffmpeg` feature, `native::NativeBackend` (ffmpeg-next, dynamically
//!   linked).
//! * [`cache`] / [`server`] — LRU frame cache and per-source background
//!   decode workers with look-ahead and GOP-aware seeking, so the UI thread
//!   never blocks on decoding.
//! * [`thumbs`] — thumbnails and filmstrips, generated on the rayon pool.

pub mod backend;
pub mod cache;
pub mod cli;
pub mod frame;
pub mod hash;
pub mod info;
#[cfg(feature = "ffmpeg")]
pub mod native;
pub mod server;
pub mod thumbs;

pub use backend::{AudioDecoder, DecodeOptions, MediaBackend, MediaError, VideoDecoder};
pub use frame::{AudioFormat, VideoFrame};
pub use info::MediaInfo;

use std::sync::Arc;

/// Picks the best available backend: native FFmpeg when compiled in and
/// initialisable, otherwise the CLI backend.
pub fn default_backend() -> Result<Arc<dyn MediaBackend>, MediaError> {
    #[cfg(feature = "ffmpeg")]
    {
        match native::NativeBackend::new() {
            Ok(b) => return Ok(Arc::new(b)),
            Err(e) => tracing::warn!("native FFmpeg unavailable ({e}); falling back to CLI"),
        }
    }
    Ok(Arc::new(cli::CliBackend::locate()?))
}

#[cfg(test)]
pub(crate) mod testutil;

#[cfg(test)]
mod integration_tests;
