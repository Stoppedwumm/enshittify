//! Sample-counted master clock.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use cutroom_core::Ticks;

/// Timeline position = `origin + consumed_samples / sample_rate`, computed in
/// exact integer flicks.
#[derive(Debug)]
pub struct AudioClock {
    origin: Ticks,
    sample_rate: u32,
    consumed: AtomicU64,
    running: AtomicBool,
}

impl AudioClock {
    pub fn new(origin: Ticks, sample_rate: u32) -> Self {
        AudioClock {
            origin,
            sample_rate,
            consumed: AtomicU64::new(0),
            running: AtomicBool::new(true),
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn origin(&self) -> Ticks {
        self.origin
    }

    /// Called from the device callback with the number of sample frames that
    /// were actually played from the timeline (excludes underrun silence).
    pub fn advance(&self, frames: u64) {
        if self.running.load(Ordering::Relaxed) {
            self.consumed.fetch_add(frames, Ordering::Release);
        }
    }

    pub fn samples(&self) -> u64 {
        self.consumed.load(Ordering::Acquire)
    }

    pub fn now(&self) -> Ticks {
        self.origin + Ticks::from_samples(self.samples() as i64, self.sample_rate)
    }

    pub fn pause(&self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_drift_over_ten_minutes_with_odd_buffers() {
        // Acceptance 2: < 1 sample drift per 10 min @ 48 kHz.
        let c = AudioClock::new(Ticks::from_secs(5), 48_000);
        let total: u64 = 48_000 * 600;
        let sizes = [441u64, 512, 480, 1024, 17, 333];
        let mut done = 0u64;
        let mut i = 0;
        while done < total {
            let n = sizes[i % sizes.len()].min(total - done);
            c.advance(n);
            done += n;
            i += 1;
        }
        assert_eq!(c.now(), Ticks::from_secs(605));
        assert_eq!((c.now() - c.origin()).to_samples(48_000), total as i64);
    }
}
