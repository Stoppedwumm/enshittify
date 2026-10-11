//! Decoded media buffers.

use std::sync::Arc;

/// A decoded picture in straight-alpha RGBA8 (sRGB-encoded, BT.709 primaries).
///
/// Decoders convert to RGBA on the decode thread; the GPU compositor also
/// accepts planar YUV (see `cutroom-render`), which backends may adopt later
/// to move the conversion onto the GPU.
#[derive(Clone)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    /// Frame index in the source's own frame rate.
    pub index: i64,
    /// Row-major RGBA8, `width * height * 4` bytes, no padding.
    pub rgba: Arc<[u8]>,
}

impl std::fmt::Debug for VideoFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "VideoFrame({}x{} #{})",
            self.width, self.height, self.index
        )
    }
}

impl VideoFrame {
    pub fn byte_len(&self) -> usize {
        self.rgba.len()
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }

    pub fn to_image(&self) -> image::RgbaImage {
        image::RgbaImage::from_raw(self.width, self.height, self.rgba.to_vec())
            .expect("frame size matches buffer")
    }
}

/// Output format of audio decoders: interleaved `f32`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

impl AudioFormat {
    pub const STEREO_48K: AudioFormat = AudioFormat {
        sample_rate: 48_000,
        channels: 2,
    };
}
