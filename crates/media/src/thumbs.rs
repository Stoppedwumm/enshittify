//! Thumbnails and filmstrips (run on the rayon pool by the app).

use std::path::{Path, PathBuf};

use cutroom_core::MediaId;
use cutroom_core::model::MediaKind;
use image::RgbaImage;

use crate::backend::{DecodeOptions, MediaBackend, MediaError};
use crate::info::MediaInfo;

pub const THUMB_W: u32 = 320;
pub const THUMB_H: u32 = 180;
/// Frames in a hover-scrub filmstrip.
pub const STRIP_FRAMES: u32 = 10;
/// Size of each filmstrip frame; the whole strip (1920 px) stays under the
/// 2048 px texture limit of low-end GPUs.
pub const STRIP_W: u32 = 192;
pub const STRIP_H: u32 = 108;

/// Where thumbnails for `id` live under a project's `cache/`.
pub fn thumb_path(cache_dir: &Path, id: MediaId) -> PathBuf {
    cache_dir
        .join("thumbs")
        .join(format!("{}.png", id.to_hex()))
}

pub fn strip_path(cache_dir: &Path, id: MediaId) -> PathBuf {
    cache_dir
        .join("thumbs")
        .join(format!("{}.strip.png", id.to_hex()))
}

fn frame_indices(info: &MediaInfo, n: u32) -> Vec<i64> {
    if info.kind == MediaKind::Image {
        return vec![0];
    }
    let total = info.frames().max(1);
    (0..n as i64)
        .map(|i| ((2 * i + 1) * total / (2 * n as i64)).min(total - 1))
        .collect()
}

/// A single poster frame at ~10% into the clip.
pub fn thumbnail(
    backend: &dyn MediaBackend,
    path: &Path,
    info: &MediaInfo,
) -> Result<RgbaImage, MediaError> {
    let mut dec = backend.open_video(path, info, DecodeOptions::bounded(THUMB_W, THUMB_H))?;
    let idx = if info.kind == MediaKind::Image {
        0
    } else {
        info.frames() / 10
    };
    let f = dec
        .frame_at(idx)?
        .or(dec.frame_at(0)?)
        .ok_or_else(|| MediaError::Decode("no frames".into()))?;
    Ok(f.to_image())
}

/// `n` evenly spaced frames laid out horizontally in one image.
pub fn filmstrip(
    backend: &dyn MediaBackend,
    path: &Path,
    info: &MediaInfo,
    n: u32,
) -> Result<RgbaImage, MediaError> {
    let mut dec = backend.open_video(path, info, DecodeOptions::bounded(STRIP_W, STRIP_H))?;
    let (w, h) = dec.output_size();
    let idx = frame_indices(info, n);
    let mut strip = RgbaImage::new(w * idx.len() as u32, h);
    for (slot, i) in idx.iter().enumerate() {
        if let Some(f) = dec.frame_at(*i)? {
            image::imageops::replace(&mut strip, &f.to_image(), (slot as u32 * w) as i64, 0);
        }
    }
    Ok(strip)
}

/// Generates (if missing) and returns the thumbnail and filmstrip paths.
pub fn ensure_thumbs(
    backend: &dyn MediaBackend,
    cache_dir: &Path,
    id: MediaId,
    path: &Path,
    info: &MediaInfo,
) -> Result<(PathBuf, Option<PathBuf>), MediaError> {
    let tp = thumb_path(cache_dir, id);
    let sp = strip_path(cache_dir, id);
    std::fs::create_dir_all(tp.parent().expect("has parent"))?;
    if !info.has_video() {
        return Err(MediaError::NoStream("video"));
    }
    if !tp.exists() {
        let img = thumbnail(backend, path, info)?;
        img.save(&tp)
            .map_err(|e| MediaError::Decode(e.to_string()))?;
    }
    if info.kind == MediaKind::Video && !sp.exists() {
        let img = filmstrip(backend, path, info, STRIP_FRAMES)?;
        img.save(&sp)
            .map_err(|e| MediaError::Decode(e.to_string()))?;
    }
    let strip = sp.exists().then_some(sp);
    Ok((tp, strip))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutroom_core::{Ticks, Timebase};

    #[test]
    fn indices_spread_evenly() {
        let info = MediaInfo {
            kind: MediaKind::Video,
            duration: Ticks::from_secs(10),
            fps: Some(Timebase::FPS_25),
            width: 1,
            height: 1,
            frame_count: Some(250),
            video_codec: None,
            pix_fmt: None,
            color_space: None,
            color_range: None,
            rotation: 0,
            audio_codec: None,
            audio_channels: 0,
            audio_sample_rate: 0,
            size_bytes: 0,
        };
        let idx = frame_indices(&info, 10);
        assert_eq!(idx.len(), 10);
        assert_eq!(idx[0], 12);
        assert_eq!(idx[9], 237);
    }
}
