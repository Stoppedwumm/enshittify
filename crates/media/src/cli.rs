//! Backend that drives the `ffprobe` / `ffmpeg` executables.
//!
//! Needs nothing at build time, so a plain `cargo build` works on Windows,
//! macOS and Linux; at runtime FFmpeg must be on `PATH` or in
//! `CUTROOM_FFMPEG_DIR`. Each decoder owns one child process that streams raw
//! frames over a pipe; seeking restarts the process with an input-side `-ss`,
//! which FFmpeg resolves by seeking to the preceding keyframe and decoding
//! forward (frame-accurate for constant-frame-rate sources).

use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::Arc;

use cutroom_core::model::MediaKind;
use cutroom_core::{Ticks, Timebase};
use serde_json::Value;

use crate::backend::{AudioDecoder, DecodeOptions, MediaBackend, MediaError, VideoDecoder};
use crate::frame::{AudioFormat, VideoFrame};
use crate::info::MediaInfo;

#[derive(Clone, Debug)]
pub struct CliBackend {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn find_in(dir: &Path, name: &str) -> Option<PathBuf> {
    let p = dir.join(exe(name));
    p.is_file().then_some(p)
}

/// A `Command` that never pops up a console window on Windows.
fn command(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c.stdin(Stdio::null());
    c
}

/// Formats seconds for FFmpeg's `-ss` without float noise.
fn secs_arg(t: Ticks) -> String {
    let t = t.max(Ticks::ZERO).0 as i128;
    let f = cutroom_core::FLICKS_PER_SECOND as i128;
    // Nanosecond precision is far below a flick-level frame boundary.
    let ns = t * 1_000_000_000 / f;
    format!("{}.{:09}", ns / 1_000_000_000, ns % 1_000_000_000)
}

fn parse_ratio(s: &str) -> Option<(i64, i64)> {
    let (a, b) = s.split_once('/')?;
    let (a, b) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    (a > 0 && b > 0).then_some((a, b))
}

fn as_i64(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

fn as_f64(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

impl CliBackend {
    /// Finds the executables in `CUTROOM_FFMPEG_DIR`, then on `PATH`.
    pub fn locate() -> Result<Self, MediaError> {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Some(d) = std::env::var_os("CUTROOM_FFMPEG_DIR") {
            dirs.push(d.into());
        }
        if let Some(p) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&p));
        }
        if cfg!(target_os = "macos") {
            dirs.push("/opt/homebrew/bin".into());
            dirs.push("/usr/local/bin".into());
        }
        for d in &dirs {
            if let (Some(ffmpeg), Some(ffprobe)) = (find_in(d, "ffmpeg"), find_in(d, "ffprobe")) {
                return Ok(CliBackend { ffmpeg, ffprobe });
            }
        }
        Err(MediaError::NotFound(format!(
            "{} and {}",
            exe("ffmpeg"),
            exe("ffprobe")
        )))
    }

    pub fn ffmpeg_path(&self) -> &Path {
        &self.ffmpeg
    }

    /// Parses `ffprobe -print_format json -show_format -show_streams` output.
    pub fn parse_probe(json: &str, size_bytes: u64) -> Result<MediaInfo, MediaError> {
        let v: Value = serde_json::from_str(json).map_err(|e| MediaError::Probe(e.to_string()))?;
        let streams = v["streams"].as_array().cloned().unwrap_or_default();
        let format = &v["format"];
        let format_name = format["format_name"].as_str().unwrap_or("");
        let video = streams.iter().find(|s| {
            s["codec_type"] == "video" && s["disposition"]["attached_pic"].as_i64() != Some(1)
        });
        let audio = streams.iter().find(|s| s["codec_type"] == "audio");
        if video.is_none() && audio.is_none() {
            return Err(MediaError::Probe("no audio or video streams".into()));
        }

        let stream_duration = |s: &Value| -> Option<Ticks> {
            let (n, d) = parse_ratio(s["time_base"].as_str()?)?;
            let dts = as_i64(&s["duration_ts"])?;
            (dts > 0).then(|| Ticks::from_rational(dts, n, d))
        };
        let fmt_duration = as_f64(&format["duration"])
            .filter(|d| *d > 0.0)
            .map(Ticks::from_secs_f64);

        let is_image = video.is_some_and(|s| {
            let codec = s["codec_name"].as_str().unwrap_or("");
            let still_codec = matches!(
                codec,
                "png" | "mjpeg" | "bmp" | "tiff" | "webp" | "gif" | "jpegxl" | "qoi"
            );
            let nb = as_i64(&s["nb_frames"]).unwrap_or(0);
            (format_name.contains("image2") || format_name.ends_with("_pipe"))
                || (still_codec && nb <= 1 && audio.is_none() && fmt_duration.is_none())
        });

        let mut info = MediaInfo {
            kind: if is_image {
                MediaKind::Image
            } else if video.is_some() {
                MediaKind::Video
            } else {
                MediaKind::Audio
            },
            duration: Ticks::ZERO,
            fps: None,
            width: 0,
            height: 0,
            frame_count: None,
            video_codec: None,
            pix_fmt: None,
            color_space: None,
            color_range: None,
            rotation: 0,
            audio_codec: None,
            audio_channels: 0,
            audio_sample_rate: 0,
            size_bytes,
        };

        if let Some(s) = video {
            info.width = as_i64(&s["width"]).unwrap_or(0) as u32;
            info.height = as_i64(&s["height"]).unwrap_or(0) as u32;
            info.video_codec = s["codec_name"].as_str().map(str::to_string);
            info.pix_fmt = s["pix_fmt"].as_str().map(str::to_string);
            info.color_space = s["color_space"].as_str().map(str::to_string);
            info.color_range = s["color_range"].as_str().map(str::to_string);
            if !is_image {
                let rate = s["avg_frame_rate"]
                    .as_str()
                    .and_then(parse_ratio)
                    .or_else(|| s["r_frame_rate"].as_str().and_then(parse_ratio));
                info.fps = rate.map(|(n, d)| {
                    let tb = Timebase::from_fps_f64(n as f64 / d as f64);
                    // Keep exact container rationals that are already small.
                    if (tb.fps_f64() - n as f64 / d as f64).abs() < 1e-6 {
                        tb
                    } else {
                        Timebase::reduced(n as u32, d as u32)
                    }
                });
                info.frame_count = as_i64(&s["nb_frames"]).filter(|n| *n > 0);
                info.duration = stream_duration(s).or(fmt_duration).unwrap_or_default();
            }
            let rot = s["side_data_list"]
                .as_array()
                .and_then(|l| l.iter().find_map(|sd| as_i64(&sd["rotation"])))
                .or_else(|| as_i64(&s["tags"]["rotate"]));
            info.rotation = rot.unwrap_or(0) as i32;
        }
        if let Some(s) = audio {
            info.audio_codec = s["codec_name"].as_str().map(str::to_string);
            info.audio_channels = as_i64(&s["channels"]).unwrap_or(0) as u32;
            info.audio_sample_rate = as_i64(&s["sample_rate"]).unwrap_or(0) as u32;
            if info.kind == MediaKind::Audio {
                info.duration = stream_duration(s).or(fmt_duration).unwrap_or_default();
            }
        }
        if info.duration == Ticks::ZERO
            && let Some(d) = fmt_duration
            && !is_image
        {
            info.duration = d;
        }
        Ok(info)
    }
}

impl MediaBackend for CliBackend {
    fn name(&self) -> &'static str {
        "ffmpeg-cli"
    }

    fn probe(&self, path: &Path) -> Result<MediaInfo, MediaError> {
        let size = std::fs::metadata(path)?.len();
        let out = command(&self.ffprobe)
            .args([
                "-v",
                "error",
                "-print_format",
                "json",
                "-show_format",
                "-show_streams",
            ])
            .arg(path)
            .stderr(Stdio::piped())
            .output()?;
        if !out.status.success() {
            return Err(MediaError::Probe(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
        Self::parse_probe(&String::from_utf8_lossy(&out.stdout), size)
    }

    fn open_video(
        &self,
        path: &Path,
        info: &MediaInfo,
        opts: DecodeOptions,
    ) -> Result<Box<dyn VideoDecoder>, MediaError> {
        if !info.has_video() {
            return Err(MediaError::NoStream("video"));
        }
        let (w, h) = opts.output_size(info.width, info.height);
        Ok(Box::new(CliVideoDecoder {
            ffmpeg: self.ffmpeg.clone(),
            path: path.to_path_buf(),
            fps: info.fps,
            still: info.kind == MediaKind::Image,
            w,
            h,
            child: None,
            next_index: 0,
            buf: vec![0; (w * h * 4) as usize],
        }))
    }

    fn open_audio(
        &self,
        path: &Path,
        format: AudioFormat,
    ) -> Result<Box<dyn AudioDecoder>, MediaError> {
        Ok(Box::new(CliAudioDecoder {
            ffmpeg: self.ffmpeg.clone(),
            path: path.to_path_buf(),
            format,
            child: None,
            pending: Vec::new(),
        }))
    }
}

struct Proc {
    child: Child,
    out: BufReader<ChildStdout>,
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Reads exactly `buf.len()` bytes or reports EOF (`Ok(false)`).
fn read_full(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<bool> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) => return Ok(false),
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

struct CliVideoDecoder {
    ffmpeg: PathBuf,
    path: PathBuf,
    fps: Option<Timebase>,
    still: bool,
    w: u32,
    h: u32,
    child: Option<Proc>,
    next_index: i64,
    buf: Vec<u8>,
}

impl CliVideoDecoder {
    fn spawn(&mut self, index: i64) -> Result<(), MediaError> {
        self.child = None;
        let mut c = command(&self.ffmpeg);
        c.args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-noautorotate",
        ]);
        if !self.still
            && let Some(fps) = self.fps
            && index > 0
        {
            // Aim half a frame early so timestamp rounding can never skip the
            // requested frame; FFmpeg drops every frame before this point.
            let t = fps.frames_to_ticks(index) - Ticks(fps.frame_duration().0 / 2);
            c.arg("-ss").arg(secs_arg(t));
        }
        c.arg("-i").arg(&self.path);
        c.args([
            "-map",
            "0:v:0",
            "-an",
            "-sn",
            "-dn",
            "-fps_mode",
            "passthrough",
        ]);
        if self.still {
            c.args(["-frames:v", "1"]);
        }
        c.arg("-vf").arg(format!(
            "scale={}:{}:flags=bicubic:in_color_matrix=auto:in_range=auto:out_range=full,format=rgba",
            self.w, self.h
        ));
        c.args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"]);
        c.stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = c.spawn()?;
        let out = BufReader::with_capacity(
            self.buf.len().max(1 << 16),
            child.stdout.take().expect("piped stdout"),
        );
        self.child = Some(Proc { child, out });
        self.next_index = index;
        Ok(())
    }
}

impl VideoDecoder for CliVideoDecoder {
    fn output_size(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    fn seek(&mut self, index: i64) -> Result<(), MediaError> {
        let index = index.max(0);
        if self.child.is_some() && index == self.next_index {
            return Ok(());
        }
        self.spawn(index)
    }

    fn decode_next(&mut self) -> Result<Option<VideoFrame>, MediaError> {
        if self.child.is_none() {
            self.spawn(self.next_index)?;
        }
        let proc = self.child.as_mut().expect("spawned");
        if !read_full(&mut proc.out, &mut self.buf)? {
            self.child = None;
            return Ok(None);
        }
        let index = self.next_index;
        self.next_index += 1;
        Ok(Some(VideoFrame {
            width: self.w,
            height: self.h,
            index,
            rgba: Arc::from(self.buf.as_slice()),
        }))
    }
}

struct CliAudioDecoder {
    ffmpeg: PathBuf,
    path: PathBuf,
    format: AudioFormat,
    child: Option<Proc>,
    pending: Vec<u8>,
}

impl CliAudioDecoder {
    fn spawn(&mut self, sample: i64) -> Result<(), MediaError> {
        self.child = None;
        self.pending.clear();
        let mut c = command(&self.ffmpeg);
        c.args(["-nostdin", "-hide_banner", "-loglevel", "error"]);
        if sample > 0 {
            let t = Ticks::from_samples(sample, self.format.sample_rate);
            c.arg("-ss").arg(secs_arg(t));
        }
        c.arg("-i").arg(&self.path);
        c.args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-f", "f32le"]);
        c.arg("-ac").arg(self.format.channels.to_string());
        c.arg("-ar").arg(self.format.sample_rate.to_string());
        c.arg("-");
        c.stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = c.spawn()?;
        let out = BufReader::with_capacity(1 << 16, child.stdout.take().expect("piped stdout"));
        self.child = Some(Proc { child, out });
        Ok(())
    }
}

impl AudioDecoder for CliAudioDecoder {
    fn format(&self) -> AudioFormat {
        self.format
    }

    fn seek(&mut self, sample: i64) -> Result<(), MediaError> {
        self.spawn(sample.max(0))
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, MediaError> {
        if self.child.is_none() {
            self.spawn(0)?;
        }
        let ch = self.format.channels as usize;
        let frame_bytes = 4 * ch;
        let want = out.len() / ch * frame_bytes;
        let proc = self.child.as_mut().expect("spawned");
        let mut bytes = std::mem::take(&mut self.pending);
        let start = bytes.len();
        bytes.resize(want.max(start), 0);
        let mut filled = start;
        while filled < want {
            match proc.out.read(&mut bytes[filled..want]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
        let frames = filled / frame_bytes;
        let used = frames * frame_bytes;
        for (i, chunk) in bytes[..used].chunks_exact(4).enumerate() {
            out[i] = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        self.pending = bytes[used..filled].to_vec();
        Ok(frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secs_arg_formats_exactly() {
        assert_eq!(secs_arg(Ticks::from_secs(2)), "2.000000000");
        assert_eq!(
            secs_arg(Timebase::FPS_29_97.frames_to_ticks(1)),
            "0.033366666"
        );
        assert_eq!(secs_arg(Ticks(-5)), "0.000000000");
    }

    #[test]
    fn parse_probe_video_with_audio() {
        let json = r#"{
          "streams": [
            {"codec_type":"video","codec_name":"h264","width":1920,"height":1080,
             "pix_fmt":"yuv420p","color_space":"bt709","color_range":"tv",
             "avg_frame_rate":"30000/1001","r_frame_rate":"30000/1001",
             "time_base":"1/30000","duration_ts":300300,"nb_frames":"300",
             "disposition":{"attached_pic":0}},
            {"codec_type":"audio","codec_name":"aac","channels":2,"sample_rate":"48000",
             "time_base":"1/48000","duration_ts":480480}
          ],
          "format": {"format_name":"mov,mp4,m4a,3gp,3g2,mj2","duration":"10.010000"}
        }"#;
        let i = CliBackend::parse_probe(json, 99).unwrap();
        assert_eq!(i.kind, MediaKind::Video);
        assert_eq!(i.fps, Some(Timebase::FPS_29_97));
        assert_eq!(i.duration, Timebase::FPS_29_97.frames_to_ticks(300));
        assert_eq!(i.frames(), 300);
        assert_eq!((i.width, i.height), (1920, 1080));
        assert_eq!((i.audio_channels, i.audio_sample_rate), (2, 48_000));
        assert_eq!(i.color_range.as_deref(), Some("tv"));
    }

    #[test]
    fn parse_probe_image_and_audio_only() {
        let img = r#"{"streams":[{"codec_type":"video","codec_name":"png","width":640,"height":480}],
                      "format":{"format_name":"png_pipe"}}"#;
        let i = CliBackend::parse_probe(img, 1).unwrap();
        assert_eq!(i.kind, MediaKind::Image);
        assert_eq!(i.fps, None);

        let wav = r#"{"streams":[{"codec_type":"audio","codec_name":"pcm_s16le","channels":1,
                      "sample_rate":"44100","time_base":"1/44100","duration_ts":44100}],
                      "format":{"format_name":"wav","duration":"1.0"}}"#;
        let a = CliBackend::parse_probe(wav, 1).unwrap();
        assert_eq!(a.kind, MediaKind::Audio);
        assert_eq!(a.duration, Ticks::from_secs(1));
        assert!(CliBackend::parse_probe(r#"{"streams":[],"format":{}}"#, 0).is_err());
    }
}
