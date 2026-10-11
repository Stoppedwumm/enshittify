//! YUV → RGB conversion matrices (BT.601 / BT.709 / BT.2020, limited & full).

/// Luma coefficients (Kr, Kb) per standard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YuvMatrix {
    Bt601,
    Bt709,
    Bt2020,
}

impl YuvMatrix {
    fn kr_kb(self) -> (f64, f64) {
        match self {
            YuvMatrix::Bt601 => (0.299, 0.114),
            YuvMatrix::Bt709 => (0.2126, 0.0722),
            YuvMatrix::Bt2020 => (0.2627, 0.0593),
        }
    }

    /// From an FFmpeg `color_space` name; untagged → BT.709.
    pub fn from_ffmpeg(name: Option<&str>) -> Self {
        match name.unwrap_or("") {
            "bt470bg" | "smpte170m" | "bt601" => YuvMatrix::Bt601,
            "bt2020nc" | "bt2020c" | "bt2020" => YuvMatrix::Bt2020,
            _ => YuvMatrix::Bt709,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YuvRange {
    /// 16–235 luma, 16–240 chroma ("tv").
    Limited,
    /// 0–255 ("pc").
    Full,
}

/// Rows of an affine map: `rgb[i] = dot(m[i].xyz, (y, u, v)) + m[i].w`,
/// with `y, u, v` normalised to `[0, 1]` (byte / 255).
pub fn yuv_to_rgb_matrix(m: YuvMatrix, r: YuvRange) -> [[f32; 4]; 3] {
    let (kr, kb) = m.kr_kb();
    let kg = 1.0 - kr - kb;
    // Scale/offset from normalised code values to Y' in [0,1], Cb/Cr in [-0.5,0.5].
    let (ys, yo, cs, co) = match r {
        YuvRange::Limited => (255.0 / 219.0, -16.0 / 219.0, 255.0 / 224.0, -128.0 / 224.0),
        YuvRange::Full => (1.0, 0.0, 1.0, -128.0 / 255.0),
    };
    let rv = 2.0 * (1.0 - kr);
    let gu = -2.0 * kb * (1.0 - kb) / kg;
    let gv = -2.0 * kr * (1.0 - kr) / kg;
    let bu = 2.0 * (1.0 - kb);
    let row = |cu: f64, cv: f64| -> [f32; 4] {
        [
            ys as f32,
            (cu * cs) as f32,
            (cv * cs) as f32,
            (yo + (cu + cv) * co) as f32,
        ]
    };
    [row(0.0, rv), row(gu, gv), row(bu, 0.0)]
}

/// CPU reference conversion of one 8-bit sample.
pub fn yuv_to_rgb(m: &[[f32; 4]; 3], y: u8, u: u8, v: u8) -> [u8; 3] {
    let (y, u, v) = (y as f32 / 255.0, u as f32 / 255.0, v as f32 / 255.0);
    let f = |r: &[f32; 4]| {
        ((r[0] * y + r[1] * u + r[2] * v + r[3]).clamp(0.0, 1.0) * 255.0).round() as u8
    };
    [f(&m[0]), f(&m[1]), f(&m[2])]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_points() {
        for mx in [YuvMatrix::Bt601, YuvMatrix::Bt709, YuvMatrix::Bt2020] {
            let lim = yuv_to_rgb_matrix(mx, YuvRange::Limited);
            assert_eq!(yuv_to_rgb(&lim, 16, 128, 128), [0, 0, 0]);
            assert_eq!(yuv_to_rgb(&lim, 235, 128, 128), [255, 255, 255]);
            let full = yuv_to_rgb_matrix(mx, YuvRange::Full);
            assert_eq!(yuv_to_rgb(&full, 0, 128, 128), [0, 0, 0]);
            assert_eq!(yuv_to_rgb(&full, 255, 128, 128), [255, 255, 255]);
        }
        // BT.709 limited 75% red bar: Y=51 Cb=109 Cr=212 (ITU-R BT.2111 values ≈ 191,0,0).
        let m = yuv_to_rgb_matrix(YuvMatrix::Bt709, YuvRange::Limited);
        let [r, g, b] = yuv_to_rgb(&m, 51, 109, 212);
        assert!(
            (r as i32 - 191).abs() <= 2 && g <= 2 && b <= 2,
            "{r} {g} {b}"
        );
        // BT.601 limited pure red: Y=81 Cb=90 Cr=240.
        let m = yuv_to_rgb_matrix(YuvMatrix::Bt601, YuvRange::Limited);
        let [r, g, b] = yuv_to_rgb(&m, 81, 90, 240);
        assert!(r >= 253 && g <= 2 && b <= 2, "{r} {g} {b}");
    }
}
