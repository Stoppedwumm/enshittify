//! Audio device output.
//!
//! The device callback pops interleaved stereo `f32` from an `rtrb` ring
//! buffer (wait-free, no allocation, no locks), maps it to the device's
//! channel layout and sample format, and advances the [`AudioClock`] by the
//! number of frames that came from the timeline. On underrun it plays silence
//! and the clock does not move, so video stays locked to what was heard.
//!
//! When no device is available (CI, headless servers) a null output consumes
//! the ring at real-time pace so playback still works.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use cutroom_media::AudioFormat;

use crate::clock::AudioClock;

/// Queries the default device's preferred rate. The engine renders at this
/// rate so the device never resamples.
pub fn preferred_format() -> AudioFormat {
    let rate = cpal::default_host()
        .default_output_device()
        .and_then(|d| d.default_output_config().ok())
        .map(|c| c.sample_rate())
        .filter(|r| (8_000..=384_000).contains(r))
        .unwrap_or(48_000);
    AudioFormat {
        sample_rate: rate,
        channels: 2,
    }
}

/// A running output; dropping it stops playback.
pub struct OutputHandle {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub device_name: String,
}

impl Drop for OutputHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

/// Starts output. Tries the default cpal device, falls back to a null sink.
pub fn start(
    format: AudioFormat,
    consumer: rtrb::Consumer<f32>,
    clock: Arc<AudioClock>,
    allow_device: bool,
) -> OutputHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<String, rtrb::Consumer<f32>>>(1);
    let stop2 = stop.clone();
    let clock2 = clock.clone();
    // cpal streams are !Send on some platforms: own the stream on a thread.
    let thread = std::thread::Builder::new()
        .name("audio-out".into())
        .spawn(move || {
            let mut consumer = Some(consumer);
            if allow_device {
                match open_device(format, &mut consumer, clock2.clone()) {
                    Ok((stream, name)) => {
                        let _ = ready_tx.send(Ok(name));
                        while !stop2.load(Ordering::Relaxed) {
                            std::thread::park_timeout(Duration::from_millis(50));
                        }
                        drop(stream);
                        return;
                    }
                    Err(e) => tracing::info!("no audio device ({e}); using null output"),
                }
            }
            let _ = ready_tx.send(Ok("null".into()));
            run_null(format, consumer.take().expect("consumer"), clock2, &stop2);
        })
        .expect("spawn audio thread");
    let device_name = match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(n)) => n,
        _ => "unknown".into(),
    };
    OutputHandle {
        stop,
        thread: Some(thread),
        device_name,
    }
}

fn run_null(
    format: AudioFormat,
    mut c: rtrb::Consumer<f32>,
    clock: Arc<AudioClock>,
    stop: &AtomicBool,
) {
    let start = Instant::now();
    let mut played: u64 = 0;
    while !stop.load(Ordering::Relaxed) {
        let due = (start.elapsed().as_secs_f64() * format.sample_rate as f64) as u64;
        while played < due {
            let avail = c.slots() / 2;
            if avail == 0 {
                // Underrun: wall time passes, timeline does not.
                played = due;
                break;
            }
            let n = (avail as u64).min(due - played) as usize;
            if let Ok(chunk) = c.read_chunk(n * 2) {
                chunk.commit_all();
            }
            played += n as u64;
            clock.advance(n as u64);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn open_device(
    format: AudioFormat,
    consumer: &mut Option<rtrb::Consumer<f32>>,
    clock: Arc<AudioClock>,
) -> Result<(cpal::Stream, String), String> {
    let host = cpal::default_host();
    let dev = host
        .default_output_device()
        .ok_or_else(|| "no default output device".to_string())?;
    let name = dev
        .description()
        .map(|d| format!("{d:?}"))
        .unwrap_or_else(|_| "default".into());
    let sup = dev.default_output_config().map_err(|e| e.to_string())?;
    let mut cfg = sup.config();
    cfg.sample_rate = format.sample_rate;
    let c = consumer.take().ok_or("consumer taken")?;
    let res = match sup.sample_format() {
        SampleFormat::F32 => build::<f32>(&dev, cfg, c, clock),
        SampleFormat::I16 => build::<i16>(&dev, cfg, c, clock),
        SampleFormat::U16 => build::<u16>(&dev, cfg, c, clock),
        SampleFormat::I32 => build::<i32>(&dev, cfg, c, clock),
        other => Err((format!("unsupported sample format {other:?}"), c)),
    };
    match res {
        Ok(s) => {
            s.play().map_err(|e| e.to_string())?;
            Ok((s, name))
        }
        Err((e, c)) => {
            *consumer = Some(c);
            Err(e)
        }
    }
}

type BuildErr = (String, rtrb::Consumer<f32>);

fn build<T>(
    dev: &cpal::Device,
    cfg: cpal::StreamConfig,
    consumer: rtrb::Consumer<f32>,
    clock: Arc<AudioClock>,
) -> Result<cpal::Stream, BuildErr>
where
    T: SizedSample + FromSample<f32>,
{
    let ch = cfg.channels as usize;
    // The callback needs the consumer by value; keep a shared slot so we can
    // hand it back if stream creation fails.
    let slot = Arc::new(parking_lot::Mutex::new(Some(consumer)));
    let cb_slot = slot.clone();
    let mut local: Option<rtrb::Consumer<f32>> = None;
    let stream = dev.build_output_stream::<T, _, _>(
        cfg,
        move |data: &mut [T], _| {
            if local.is_none() {
                // First callback only; uncontended.
                local = cb_slot.lock().take();
            }
            let Some(c) = local.as_mut() else {
                data.fill(T::EQUILIBRIUM);
                return;
            };
            let frames = data.len() / ch;
            let avail = (c.slots() / 2).min(frames);
            let mut taken = 0;
            if let Ok(chunk) = c.read_chunk(avail * 2) {
                let (a, b) = chunk.as_slices();
                let mut it = a.iter().chain(b.iter());
                for f in 0..avail {
                    let l = *it.next().unwrap_or(&0.0);
                    let r = *it.next().unwrap_or(&0.0);
                    let out = &mut data[f * ch..(f + 1) * ch];
                    match ch {
                        1 => out[0] = T::from_sample(0.5 * (l + r)),
                        _ => {
                            out[0] = T::from_sample(l);
                            out[1] = T::from_sample(r);
                            for o in &mut out[2..] {
                                *o = T::EQUILIBRIUM;
                            }
                        }
                    }
                }
                chunk.commit_all();
                taken = avail;
            }
            for s in &mut data[taken * ch..] {
                *s = T::EQUILIBRIUM;
            }
            clock.advance(taken as u64);
        },
        |e| tracing::warn!("audio stream error: {e}"),
        None,
    );
    match stream {
        Ok(s) => Ok(s),
        Err(e) => {
            let c = slot.lock().take().expect("consumer not yet taken");
            Err((e.to_string(), c))
        }
    }
}
