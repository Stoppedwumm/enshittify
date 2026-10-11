//! Encode pipeline.

use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cutroom_audio::TimelineAudio;
use cutroom_core::model::DocSnapshot;
use cutroom_core::{MediaId, SeqId, Ticks};
use cutroom_media::cache::Quality;
use cutroom_media::server::FrameServer;
use cutroom_media::{AudioFormat, MediaBackend, MediaInfo, VideoFrame};
use cutroom_render::gpu::Compositor;
use cutroom_render::{FrameProvider, build_plan};

use crate::presets::Preset;

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("ffmpeg executable not found")]
    NoFfmpeg,
    #[error("encoder failed: {0}")]
    Encoder(String),
    #[error("media {0:?} is not available on this machine")]
    MissingMedia(MediaId),
    #[error("cancelled")]
    Cancelled,
    #[error("empty range")]
    EmptyRange,
}

/// A local file backing a media id.
#[derive(Clone, Debug)]
pub struct SourceFile {
    pub id: MediaId,
    pub path: PathBuf,
    pub info: MediaInfo,
}

#[derive(Clone)]
pub struct ExportJob {
    pub snapshot: Arc<DocSnapshot>,
    pub seq: SeqId,
    /// Timeline range `[start, end)`.
    pub start: Ticks,
    pub end: Ticks,
    pub preset: Preset,
    pub output: PathBuf,
    pub sources: Vec<SourceFile>,
    /// Force the CPU rasterizer (tests, GPU-less machines).
    pub cpu_only: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Progress {
    pub frame: u64,
    pub total: u64,
    pub elapsed: Duration,
    pub eta: Option<Duration>,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            1.0
        } else {
            self.frame as f32 / self.total as f32
        }
    }
}

struct BlockingFrames<'a>(&'a FrameServer);

impl FrameProvider for BlockingFrames<'_> {
    fn frame(&self, media: MediaId, index: i64) -> Option<VideoFrame> {
        self.0
            .get_blocking(media, index, Quality::Full, Duration::from_secs(30))
    }
}

/// Runs an export synchronously. Call from a background thread.
pub fn run_export(
    job: &ExportJob,
    backend: Arc<dyn MediaBackend>,
    ffmpeg: &std::path::Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(Progress),
) -> Result<(), ExportError> {
    let tb = job.preset.fps;
    let first = tb.ticks_to_frame(job.start + tb.frame_duration() - Ticks(1));
    let last = tb.ticks_to_frame(job.end - Ticks(1));
    if last < first {
        return Err(ExportError::EmptyRange);
    }
    let total = (last - first + 1) as u64;

    // Media must be local (proxies are never exported).
    let seq = job.snapshot.sequences.get(&job.seq);
    for c in seq.into_iter().flat_map(|s| s.clips.values()) {
        if let Some(m) = c.media_id()
            && !job.sources.iter().any(|s| s.id == m)
        {
            return Err(ExportError::MissingMedia(m));
        }
    }

    // 1. Audio → temp file (same mixer as preview).
    let tmp = std::env::temp_dir().join(format!(
        "cutroom-export-{}-{}.f32",
        std::process::id(),
        job.seq
    ));
    let paths: Vec<(MediaId, PathBuf)> =
        job.sources.iter().map(|s| (s.id, s.path.clone())).collect();
    let resolve =
        Arc::new(move |id: MediaId| paths.iter().find(|(m, _)| *m == id).map(|(_, p)| p.clone()));
    {
        let fmt = AudioFormat::STEREO_48K;
        let start_t = tb.frames_to_ticks(first);
        let end_t = tb.frames_to_ticks(last + 1);
        let mut samples = (end_t - start_t).to_samples(fmt.sample_rate);
        let mut tl = TimelineAudio::new(
            job.snapshot.clone(),
            job.seq,
            fmt,
            backend.clone(),
            resolve,
            start_t,
        );
        let mut w = BufWriter::new(std::fs::File::create(&tmp)?);
        let mut buf = vec![0f32; 4800 * 2];
        while samples > 0 {
            if cancel.load(Ordering::Relaxed) {
                let _ = std::fs::remove_file(&tmp);
                return Err(ExportError::Cancelled);
            }
            let n = (samples as usize).min(4800);
            tl.fill(&mut buf[..n * 2]);
            for s in &buf[..n * 2] {
                w.write_all(&s.to_le_bytes())?;
            }
            samples -= n as i64;
        }
        w.flush()?;
    }
    let _cleanup = TempFile(tmp.clone());

    // 2. Video frames → ffmpeg stdin.
    let (pw, ph) = (job.snapshot.meta.width, job.snapshot.meta.height);
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "rawvideo", "-pix_fmt", "rgba"])
        .arg("-s")
        .arg(format!("{pw}x{ph}"))
        .arg("-r")
        .arg(format!("{}/{}", tb.num, tb.den))
        .args(["-i", "-"])
        .args(["-f", "f32le", "-ar", "48000", "-ac", "2", "-i"])
        .arg(&tmp)
        .args(["-map", "0:v:0", "-map", "1:a:0"])
        .args(job.preset.ffmpeg_output_args())
        .arg("-shortest")
        .arg(&job.output)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|_| ExportError::NoFfmpeg)?;
    let mut stdin = BufWriter::with_capacity(1 << 22, child.stdin.take().expect("piped"));

    let server = FrameServer::new(backend, 512 << 20);
    for s in &job.sources {
        server.register(s.id, s.path.clone(), s.info.clone());
    }
    let mut gpu = if job.cpu_only {
        None
    } else {
        Compositor::new_headless().ok()
    };
    let provider = BlockingFrames(&server);
    let started = Instant::now();
    for (n, f) in (first..=last).enumerate() {
        if cancel.load(Ordering::Relaxed) {
            drop(stdin);
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&job.output);
            return Err(ExportError::Cancelled);
        }
        let plan = build_plan(&job.snapshot, job.seq, tb.frames_to_ticks(f), &provider);
        let rgba = match gpu.as_mut() {
            Some(c) => {
                c.render(&plan);
                c.read_output().expect("rendered")
            }
            None => cutroom_render::cpu::render(&plan),
        };
        if let Err(e) = stdin.write_all(&rgba) {
            let out = child.wait_with_output()?;
            return Err(ExportError::Encoder(format!(
                "{e}: {}",
                String::from_utf8_lossy(&out.stderr)
            )));
        }
        let done = n as u64 + 1;
        let elapsed = started.elapsed();
        let eta = (done > 2).then(|| elapsed.mul_f64((total - done) as f64 / done as f64));
        on_progress(Progress {
            frame: done,
            total,
            elapsed,
            eta,
        });
    }
    drop(stdin);
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(ExportError::Encoder(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
