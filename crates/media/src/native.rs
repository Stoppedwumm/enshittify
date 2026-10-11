//! Native FFmpeg backend (`ffmpeg-next`, dynamically linked). Enabled with
//! the `ffmpeg` feature.
//!
//! The only `unsafe` here configures swscale's YUV→RGB matrix (BT.601 /
//! 709 / 2020, limited/full range), which ffmpeg-next does not wrap.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cutroom_core::model::MediaKind;
use cutroom_core::{Ticks, Timebase};
use ff::format::Pixel;
use ff::media::Type;
use ff::software::{resampling, scaling};
use ff::util::color;
use ffmpeg_next as ff;

use crate::backend::{AudioDecoder, DecodeOptions, MediaBackend, MediaError, VideoDecoder};
use crate::frame::{AudioFormat, VideoFrame};
use crate::info::MediaInfo;

fn err(e: ff::Error) -> MediaError {
    MediaError::Decode(e.to_string())
}

pub struct NativeBackend;

impl NativeBackend {
    pub fn new() -> Result<Self, MediaError> {
        ff::init().map_err(|e| MediaError::Unsupported(e.to_string()))?;
        ff::util::log::set_level(ff::util::log::Level::Error);
        Ok(NativeBackend)
    }
}

fn rational_fps(r: ff::Rational) -> Option<Timebase> {
    let (n, d) = (r.numerator(), r.denominator());
    if n <= 0 || d <= 0 {
        return None;
    }
    let snapped = Timebase::from_fps_f64(n as f64 / d as f64);
    Some(if (snapped.fps_f64() - n as f64 / d as f64).abs() < 1e-6 {
        snapped
    } else {
        Timebase::reduced(n as u32, d as u32)
    })
}

impl MediaBackend for NativeBackend {
    fn name(&self) -> &'static str {
        "ffmpeg-native"
    }

    fn probe(&self, path: &Path) -> Result<MediaInfo, MediaError> {
        let size = std::fs::metadata(path)?.len();
        let ictx = ff::format::input(path).map_err(|e| MediaError::Probe(e.to_string()))?;
        let fmt_name = ictx.format().name().to_string();
        let video = ictx
            .streams()
            .filter(|s| s.parameters().medium() == Type::Video)
            .find(|s| {
                !s.disposition()
                    .contains(ff::format::stream::Disposition::ATTACHED_PIC)
            });
        let audio = ictx.streams().best(Type::Audio);
        if video.is_none() && audio.is_none() {
            return Err(MediaError::Probe("no audio or video streams".into()));
        }
        let fmt_duration = (ictx.duration() > 0)
            .then(|| Ticks::from_rational(ictx.duration(), 1, ff::ffi::AV_TIME_BASE as i64));
        let stream_duration = |s: &ff::format::stream::Stream| {
            let tb = s.time_base();
            (s.duration() > 0 && tb.denominator() > 0).then(|| {
                Ticks::from_rational(s.duration(), tb.numerator() as i64, tb.denominator() as i64)
            })
        };
        let mut info = MediaInfo {
            kind: MediaKind::Audio,
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
            size_bytes: size,
        };
        if let Some(s) = &video {
            let ctx = ff::codec::context::Context::from_parameters(s.parameters()).map_err(err)?;
            let dec = ctx.decoder().video().map_err(err)?;
            let codec = dec.id().name().to_string();
            let still_codec = matches!(
                codec.as_str(),
                "png" | "mjpeg" | "bmp" | "tiff" | "webp" | "gif" | "jpegxl" | "qoi"
            );
            let is_image = fmt_name.contains("image2")
                || fmt_name.ends_with("_pipe")
                || (still_codec && s.frames() <= 1 && audio.is_none() && fmt_duration.is_none());
            info.kind = if is_image {
                MediaKind::Image
            } else {
                MediaKind::Video
            };
            info.width = dec.width();
            info.height = dec.height();
            info.video_codec = Some(codec);
            info.pix_fmt = Some(format!("{:?}", dec.format()).to_lowercase());
            info.color_space = Some(format!("{:?}", dec.color_space()).to_lowercase());
            info.color_range = Some(
                match dec.color_range() {
                    color::Range::JPEG => "pc",
                    _ => "tv",
                }
                .into(),
            );
            if !is_image {
                info.fps = rational_fps(s.avg_frame_rate()).or_else(|| rational_fps(s.rate()));
                info.frame_count = (s.frames() > 0).then_some(s.frames());
                info.duration = stream_duration(s).or(fmt_duration).unwrap_or_default();
            }
        }
        if let Some(s) = &audio {
            let ctx = ff::codec::context::Context::from_parameters(s.parameters()).map_err(err)?;
            let dec = ctx.decoder().audio().map_err(err)?;
            info.audio_codec = Some(dec.id().name().to_string());
            info.audio_channels = dec.channels() as u32;
            info.audio_sample_rate = dec.rate();
            if info.kind == MediaKind::Audio {
                info.duration = stream_duration(s).or(fmt_duration).unwrap_or_default();
            }
        }
        if info.duration == Ticks::ZERO
            && info.kind != MediaKind::Image
            && let Some(d) = fmt_duration
        {
            info.duration = d;
        }
        Ok(info)
    }

    fn open_video(
        &self,
        path: &Path,
        info: &MediaInfo,
        opts: DecodeOptions,
    ) -> Result<Box<dyn VideoDecoder>, MediaError> {
        Ok(Box::new(NativeVideo::open(path, info, opts)?))
    }

    fn open_audio(
        &self,
        path: &Path,
        format: AudioFormat,
    ) -> Result<Box<dyn AudioDecoder>, MediaError> {
        Ok(Box::new(NativeAudio::open(path, format)?))
    }
}

/// Configures the scaler's input matrix/range from the frame's tags.
// The SWS_CS_* constants' integer type differs between FFmpeg versions.
#[allow(clippy::unnecessary_cast)]
fn set_colorspace(sws: &mut scaling::Context, space: color::Space, range: color::Range) {
    let cs = match space {
        color::Space::BT709 => ff::ffi::SWS_CS_ITU709,
        color::Space::BT2020NCL | color::Space::BT2020CL => ff::ffi::SWS_CS_BT2020,
        color::Space::BT470BG | color::Space::SMPTE170M => ff::ffi::SWS_CS_ITU601,
        // Untagged HD content is almost always BT.709.
        _ => ff::ffi::SWS_CS_ITU709,
    } as i32;
    let src_full = i32::from(range == color::Range::JPEG);
    // SAFETY: `sws` owns a valid, initialised SwsContext for its lifetime;
    // `sws_getCoefficients` returns pointers to static tables; the
    // brightness/contrast/saturation values are FFmpeg's documented
    // neutral defaults (0, 1<<16, 1<<16). The call only mutates the context.
    unsafe {
        let ptr = sws.as_mut_ptr();
        let src = ff::ffi::sws_getCoefficients(cs);
        let dst = ff::ffi::sws_getCoefficients(ff::ffi::SWS_CS_DEFAULT as i32);
        ff::ffi::sws_setColorspaceDetails(ptr, src, src_full, dst, 1, 0, 1 << 16, 1 << 16);
    }
}

struct NativeVideo {
    path: PathBuf,
    ictx: ff::format::context::Input,
    stream: usize,
    dec: ff::decoder::Video,
    tb: ff::Rational,
    start_pts: i64,
    fps: Option<Timebase>,
    still: bool,
    out_w: u32,
    out_h: u32,
    sws: Option<(scaling::Context, (Pixel, u32, u32))>,
    /// Frames before this index are discarded after a seek.
    skip_until: i64,
    next_index: i64,
    eof_sent: bool,
}

// SAFETY-adjacent note: ffmpeg-next contexts are Send but not Sync; the
// decoder is owned by exactly one worker thread.
unsafe impl Send for NativeVideo {}

impl NativeVideo {
    fn open(path: &Path, info: &MediaInfo, opts: DecodeOptions) -> Result<Self, MediaError> {
        let ictx = ff::format::input(path).map_err(err)?;
        let s = ictx
            .streams()
            .best(Type::Video)
            .ok_or(MediaError::NoStream("video"))?;
        let stream = s.index();
        let tb = s.time_base();
        let start_pts = if s.start_time() == ff::ffi::AV_NOPTS_VALUE {
            0
        } else {
            s.start_time()
        };
        let mut ctx = ff::codec::context::Context::from_parameters(s.parameters()).map_err(err)?;
        ctx.set_threading(ff::codec::threading::Config::kind(
            ff::codec::threading::Type::Frame,
        ));
        let dec = ctx.decoder().video().map_err(err)?;
        let (out_w, out_h) = opts.output_size(info.width, info.height);
        Ok(NativeVideo {
            path: path.to_path_buf(),
            ictx,
            stream,
            dec,
            tb,
            start_pts,
            fps: info.fps,
            still: info.kind == MediaKind::Image,
            out_w,
            out_h,
            sws: None,
            skip_until: 0,
            next_index: 0,
            eof_sent: false,
        })
    }

    fn index_of(&self, pts: i64) -> i64 {
        match self.fps {
            Some(fps) if !self.still => {
                let t = Ticks::from_rational(
                    pts - self.start_pts,
                    self.tb.numerator() as i64,
                    self.tb.denominator() as i64,
                );
                fps.ticks_to_nearest_frame(t)
            }
            _ => self.next_index,
        }
    }

    fn convert(&mut self, src: &ff::frame::Video, index: i64) -> Result<VideoFrame, MediaError> {
        let key = (src.format(), src.width(), src.height());
        if self.sws.as_ref().map(|(_, k)| *k) != Some(key) {
            let mut ctx = scaling::Context::get(
                key.0,
                key.1,
                key.2,
                Pixel::RGBA,
                self.out_w,
                self.out_h,
                scaling::Flags::BICUBIC,
            )
            .map_err(err)?;
            set_colorspace(&mut ctx, src.color_space(), src.color_range());
            self.sws = Some((ctx, key));
        }
        let mut dst = ff::frame::Video::empty();
        self.sws
            .as_mut()
            .expect("set")
            .0
            .run(src, &mut dst)
            .map_err(err)?;
        let (w, h) = (self.out_w as usize, self.out_h as usize);
        let stride = dst.stride(0);
        let data = dst.data(0);
        let mut rgba = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            rgba.extend_from_slice(&data[y * stride..y * stride + w * 4]);
        }
        Ok(VideoFrame {
            width: self.out_w,
            height: self.out_h,
            index,
            rgba: Arc::from(rgba),
        })
    }
}

impl VideoDecoder for NativeVideo {
    fn output_size(&self) -> (u32, u32) {
        (self.out_w, self.out_h)
    }

    fn seek(&mut self, index: i64) -> Result<(), MediaError> {
        let index = index.max(0);
        if index == self.next_index && self.skip_until <= index {
            return Ok(());
        }
        let ts_ticks = self
            .fps
            .filter(|_| !self.still)
            .map(|f| f.frames_to_ticks(index))
            .unwrap_or_default();
        // Seek (backwards) to the keyframe at or before the target, in
        // stream time base, then decode forward discarding earlier frames.
        let target_pts = self.start_pts
            + (ts_ticks.0 as i128 * self.tb.denominator() as i128
                / (cutroom_core::FLICKS_PER_SECOND as i128 * self.tb.numerator() as i128))
                as i64;
        let av_ts = (target_pts - self.start_pts) as i128
            * self.tb.numerator() as i128
            * ff::ffi::AV_TIME_BASE as i128
            / self.tb.denominator() as i128;
        let av_ts = av_ts as i64
            + if self.start_pts > 0 {
                self.start_pts * self.tb.numerator() as i64 * ff::ffi::AV_TIME_BASE as i64
                    / self.tb.denominator() as i64
            } else {
                0
            };
        if self.ictx.seek(av_ts, ..av_ts).is_err() {
            // Some formats refuse; reopen from the start.
            self.ictx = ff::format::input(&self.path).map_err(err)?;
        }
        self.dec.flush();
        self.eof_sent = false;
        self.skip_until = index;
        self.next_index = index;
        Ok(())
    }

    fn decode_next(&mut self) -> Result<Option<VideoFrame>, MediaError> {
        let mut frame = ff::frame::Video::empty();
        loop {
            if self.dec.receive_frame(&mut frame).is_ok() {
                let pts = frame.timestamp().unwrap_or(0);
                let index = self.index_of(pts);
                if index < self.skip_until {
                    continue;
                }
                self.skip_until = 0;
                self.next_index = index + 1;
                return self.convert(&frame, index).map(Some);
            }
            if self.eof_sent {
                return Ok(None);
            }
            let mut packet = ff::Packet::empty();
            match packet.read(&mut self.ictx) {
                Ok(()) => {
                    if packet.stream() == self.stream {
                        // Corrupt packets are skipped rather than fatal.
                        let _ = self.dec.send_packet(&packet);
                    }
                }
                Err(ff::Error::Eof) => {
                    let _ = self.dec.send_eof();
                    self.eof_sent = true;
                }
                Err(e) => return Err(err(e)),
            }
        }
    }
}

struct NativeAudio {
    path: PathBuf,
    ictx: ff::format::context::Input,
    stream: usize,
    dec: ff::decoder::Audio,
    tb: ff::Rational,
    start_pts: i64,
    format: AudioFormat,
    swr: Option<resampling::Context>,
    /// Interleaved samples ready to hand out.
    pending: Vec<f32>,
    /// Output samples (per channel) to drop after a seek.
    skip: i64,
    seek_target: Option<i64>,
    eof_sent: bool,
    flushed: bool,
}

unsafe impl Send for NativeAudio {}

impl NativeAudio {
    fn open(path: &Path, format: AudioFormat) -> Result<Self, MediaError> {
        let ictx = ff::format::input(path).map_err(err)?;
        let s = ictx
            .streams()
            .best(Type::Audio)
            .ok_or(MediaError::NoStream("audio"))?;
        let stream = s.index();
        let tb = s.time_base();
        let start_pts = if s.start_time() == ff::ffi::AV_NOPTS_VALUE {
            0
        } else {
            s.start_time()
        };
        let ctx = ff::codec::context::Context::from_parameters(s.parameters()).map_err(err)?;
        let dec = ctx.decoder().audio().map_err(err)?;
        Ok(NativeAudio {
            path: path.to_path_buf(),
            ictx,
            stream,
            dec,
            tb,
            start_pts,
            format,
            swr: None,
            pending: Vec::new(),
            skip: 0,
            seek_target: None,
            eof_sent: false,
            flushed: false,
        })
    }

    fn out_layout(&self) -> ff::ChannelLayout {
        ff::ChannelLayout::default(self.format.channels as i32)
    }

    fn push_converted(&mut self, out: &ff::frame::Audio) {
        let n = out.samples() * self.format.channels as usize;
        if n == 0 {
            return;
        }
        let bytes = &out.data(0)[..n * 4];
        self.pending.extend(
            bytes
                .chunks_exact(4)
                .map(|c| f32::from_ne_bytes([c[0], c[1], c[2], c[3]])),
        );
    }

    /// Decodes one more chunk into `pending`. Returns false at end of stream.
    fn fill(&mut self) -> Result<bool, MediaError> {
        let mut frame = ff::frame::Audio::empty();
        loop {
            if self.dec.receive_frame(&mut frame).is_ok() {
                if frame.channel_layout().is_empty() {
                    frame.set_channel_layout(ff::ChannelLayout::default(frame.channels() as i32));
                }
                if self.swr.is_none() {
                    self.swr = Some(
                        resampling::Context::get(
                            frame.format(),
                            frame.channel_layout(),
                            frame.rate(),
                            ff::format::Sample::F32(ff::format::sample::Type::Packed),
                            self.out_layout(),
                            self.format.sample_rate,
                        )
                        .map_err(err)?,
                    );
                }
                // First frame after a seek: compute how many output samples
                // precede the target.
                if let Some(target) = self.seek_target.take() {
                    let pts = frame.timestamp().unwrap_or(self.start_pts);
                    let t = Ticks::from_rational(
                        pts - self.start_pts,
                        self.tb.numerator() as i64,
                        self.tb.denominator() as i64,
                    );
                    let at = t.to_samples(self.format.sample_rate);
                    self.skip = (target - at).max(0);
                }
                let mut out = ff::frame::Audio::empty();
                self.swr
                    .as_mut()
                    .expect("set")
                    .run(&frame, &mut out)
                    .map_err(err)?;
                self.push_converted(&out);
                return Ok(true);
            }
            if self.eof_sent {
                if !self.flushed {
                    self.flushed = true;
                    if let Some(swr) = self.swr.as_mut() {
                        let mut out = ff::frame::Audio::empty();
                        if swr.flush(&mut out).is_ok() {
                            self.push_converted(&out);
                            return Ok(true);
                        }
                    }
                }
                return Ok(false);
            }
            let mut packet = ff::Packet::empty();
            match packet.read(&mut self.ictx) {
                Ok(()) => {
                    if packet.stream() == self.stream {
                        let _ = self.dec.send_packet(&packet);
                    }
                }
                Err(ff::Error::Eof) => {
                    let _ = self.dec.send_eof();
                    self.eof_sent = true;
                }
                Err(e) => return Err(err(e)),
            }
        }
    }
}

impl AudioDecoder for NativeAudio {
    fn format(&self) -> AudioFormat {
        self.format
    }

    fn seek(&mut self, sample: i64) -> Result<(), MediaError> {
        let sample = sample.max(0);
        let t = Ticks::from_samples(sample, self.format.sample_rate);
        let av_ts = (t.0 as i128 * ff::ffi::AV_TIME_BASE as i128
            / cutroom_core::FLICKS_PER_SECOND as i128) as i64;
        if self.ictx.seek(av_ts, ..av_ts).is_err() {
            self.ictx = ff::format::input(&self.path).map_err(err)?;
        }
        self.dec.flush();
        self.swr = None;
        self.pending.clear();
        self.seek_target = Some(sample);
        self.skip = 0;
        self.eof_sent = false;
        self.flushed = false;
        Ok(())
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, MediaError> {
        let ch = self.format.channels as usize;
        let want = out.len() / ch * ch;
        loop {
            if self.skip > 0 && !self.pending.is_empty() {
                let drop = (self.skip as usize * ch).min(self.pending.len());
                self.pending.drain(..drop);
                self.skip -= (drop / ch) as i64;
            }
            if self.skip == 0 && self.pending.len() >= want {
                break;
            }
            if !self.fill()? {
                break;
            }
        }
        if self.skip > 0 {
            return Ok(0);
        }
        let n = self.pending.len().min(want);
        out[..n].copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
        Ok(n / ch)
    }
}
