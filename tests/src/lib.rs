//! Shared helpers for acceptance tests.

use std::path::{Path, PathBuf};
use std::process::Command;

use cutroom_media::VideoFrame;
use cutroom_media::cli::CliBackend;

pub const BITS: u32 = 16;

/// The CLI backend, or `None` (test skipped) when FFmpeg is not installed.
pub fn ffmpeg() -> Option<CliBackend> {
    match CliBackend::locate() {
        Ok(b) => Some(b),
        Err(e) => {
            eprintln!("skipping: {e}");
            None
        }
    }
}

/// H.264 + AAC test video whose frames encode their own index as 16 binary
/// columns (the "burned-in frame counter").
pub fn counter_video(b: &CliBackend, dir: &Path, name: &str, rate: &str, frames: u32) -> PathBuf {
    let out = dir.join(name);
    let geq = format!(
        "lum='if(lt(Y,H*0.8),255*mod(floor(N/pow(2,floor(X*{BITS}/W))),2),128)':cb=128:cr=128"
    );
    let ok = Command::new(b.ffmpeg_path())
        .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!("color=c=black:s=320x180:r={rate}"))
        .args(["-f", "lavfi", "-i", "sine=frequency=1000:sample_rate=48000"])
        .arg("-vf")
        .arg(format!("format=yuv444p,geq={geq},format=yuv420p"))
        .args(["-frames:v", &frames.to_string(), "-shortest"])
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "12",
            "-g",
            "120",
            "-bf",
            "2",
        ])
        .args(["-c:a", "aac", "-b:a", "96k"])
        .arg(&out)
        .status()
        .expect("run ffmpeg")
        .success();
    assert!(ok, "ffmpeg failed to generate {name}");
    out
}

/// Reads the frame index back. Works at any output resolution.
pub fn read_counter(f: &VideoFrame) -> i64 {
    let y = f.height * 2 / 5;
    let mut v = 0i64;
    for bit in 0..BITS {
        let x = ((2 * bit + 1) * f.width) / (2 * BITS);
        if f.pixel(x, y)[0] > 128 {
            v |= 1 << bit;
        }
    }
    v
}
