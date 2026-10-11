//! Cutroom export.
//!
//! The pipeline renders every output frame through the same
//! [`cutroom_render::build_plan`] path as the preview (blocking for exact
//! frames), rasterizes on the GPU when available (CPU reference otherwise),
//! renders audio with the same [`cutroom_audio::TimelineAudio`] mixer, and
//! pipes both into the `ffmpeg` executable.

pub mod pipeline;
pub mod presets;
pub mod queue;

pub use pipeline::{ExportError, ExportJob, Progress, SourceFile, run_export};
pub use presets::{Preset, mvp_presets};
pub use queue::{ExportQueue, JobId, JobState};
