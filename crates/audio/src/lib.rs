//! Cutroom audio.
//!
//! * [`clock::AudioClock`] — the master clock. Playback position is derived
//!   from the integer count of samples the device has consumed, so it cannot
//!   drift against the timeline (video frames are chosen to match it).
//! * [`timeline::TimelineAudio`] — pulls clip audio through the decoders and
//!   mixes tracks (gain, pan, fades). The same code renders preview and export.
//! * [`output`] — cpal device output with a null fallback for headless use.
//! * [`engine::AudioEngine`] — glues them together with a feeder thread and a
//!   lock-free ring buffer; the device callback never blocks or allocates.

pub mod clock;
pub mod engine;
pub mod mixer;
pub mod output;
pub mod timeline;

pub use clock::AudioClock;
pub use engine::AudioEngine;
pub use timeline::TimelineAudio;

#[cfg(test)]
mod tests;
