//! Test media generation via the ffmpeg executable.
//!
//! Videos encode their frame number as 16 black/white columns (LSB left),
//! which survives lossy compression far better than burned-in digits and
//! needs no OCR.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::CliBackend;
use crate::frame::VideoFrame;

pub const BITS: u32 = 16;

pub fn backend() -> Option<CliBackend> {
    match CliBackend::locate() {
        Ok(b) => Some(b),
        Err(e) => {
            eprintln!("skipping: {e}");
            None
        }
    }
}

/// Encodes a counter video (H.264, long GOP with B-frames) plus a 1 kHz tone.
pub fn counter_video(b: &CliBackend, dir: &Path, name: &str, rate: &str, frames: u32) -> PathBuf {
    let out = dir.join(name);
    let geq = format!(
        "lum='if(lt(Y,H*0.8),255*mod(floor(N/pow(2,floor(X*{BITS}/W))),2),128)':cb=128:cr=128"
    );
    let status = Command::new(b.ffmpeg_path())
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
            "-c:a",
            "aac",
            "-b:a",
            "96k",
        ])
        .arg(&out)
        .status()
        .expect("run ffmpeg");
    assert!(status.success(), "ffmpeg failed to generate {name}");
    out
}

/// Reads the frame number back from a decoded counter frame.
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
