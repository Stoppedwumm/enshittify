//! Realtime playback engine.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use cutroom_core::model::DocSnapshot;
use cutroom_core::{SeqId, Ticks};
use cutroom_media::{AudioFormat, MediaBackend};

use crate::clock::AudioClock;
use crate::output::{self, OutputHandle};
use crate::timeline::{PathResolver, TimelineAudio};

/// Ring buffer length in seconds: enough to ride out decoder hiccups,
/// short enough that "stop" feels immediate.
const RING_SECONDS: f64 = 0.25;
const BLOCK_FRAMES: usize = 1024;

struct Playing {
    clock: Arc<AudioClock>,
    stop: Arc<AtomicBool>,
    feeder: Option<JoinHandle<()>>,
    _out: OutputHandle,
}

impl Drop for Playing {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.clock.pause();
        if let Some(f) = self.feeder.take() {
            let _ = f.join();
        }
    }
}

pub struct AudioEngine {
    format: AudioFormat,
    backend: Arc<dyn MediaBackend>,
    use_device: bool,
    playing: Option<Playing>,
    device_name: String,
}

impl AudioEngine {
    /// `use_device = false` forces the null output (tests, headless).
    pub fn new(backend: Arc<dyn MediaBackend>, use_device: bool) -> Self {
        let format = if use_device {
            output::preferred_format()
        } else {
            AudioFormat::STEREO_48K
        };
        AudioEngine {
            format,
            backend,
            use_device,
            playing: None,
            device_name: String::new(),
        }
    }

    pub fn format(&self) -> AudioFormat {
        self.format
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Starts playback of `seq` at `from`. Any current playback stops.
    pub fn play(&mut self, snap: Arc<DocSnapshot>, seq: SeqId, from: Ticks, resolve: PathResolver) {
        self.stop();
        let format = self.format;
        let cap = ((format.sample_rate as f64 * RING_SECONDS) as usize).max(BLOCK_FRAMES) * 2;
        let (mut prod, cons) = rtrb::RingBuffer::<f32>::new(cap);
        let clock = Arc::new(AudioClock::new(from, format.sample_rate));
        let stop = Arc::new(AtomicBool::new(false));
        let mut tl = TimelineAudio::new(snap, seq, format, self.backend.clone(), resolve, from);
        // Pre-roll one block so the device starts with real audio.
        let mut block = vec![0f32; BLOCK_FRAMES * 2];
        tl.fill(&mut block);
        if let Ok(mut ch) = prod.write_chunk_uninit(block.len()) {
            let (a, b) = ch.as_mut_slices();
            let (x, y) = block.split_at(a.len());
            for (d, s) in a.iter_mut().zip(x) {
                d.write(*s);
            }
            for (d, s) in b.iter_mut().zip(y) {
                d.write(*s);
            }
            // SAFETY: every slot of both slices was initialised above.
            unsafe { ch.commit_all() };
        }
        let stop2 = stop.clone();
        let feeder = std::thread::Builder::new()
            .name("audio-feeder".into())
            .spawn(move || {
                while !stop2.load(Ordering::Relaxed) {
                    if prod.slots() < block.len() {
                        std::thread::sleep(Duration::from_millis(3));
                        continue;
                    }
                    tl.fill(&mut block);
                    for s in &block {
                        // Space was checked above; push cannot fail.
                        let _ = prod.push(*s);
                    }
                }
            })
            .expect("spawn feeder");
        let out = output::start(format, cons, clock.clone(), self.use_device);
        self.device_name = out.device_name.clone();
        self.playing = Some(Playing {
            clock,
            stop,
            feeder: Some(feeder),
            _out: out,
        });
    }

    pub fn stop(&mut self) {
        self.playing = None;
    }

    pub fn is_playing(&self) -> bool {
        self.playing.is_some()
    }

    /// Master clock position while playing.
    pub fn position(&self) -> Option<Ticks> {
        self.playing.as_ref().map(|p| p.clock.now())
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.stop();
    }
}
