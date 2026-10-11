//! CPU reference rasterizer.
//!
//! Executes a [`RenderPlan`] with the same math as the WGSL compositor
//! (bilinear sampling, premultiplied-alpha blending in 8-bit gamma space).
//! Deterministic across platforms, which makes it the oracle for golden and
//! convergence tests, and the fallback when no GPU adapter exists.

use cutroom_core::model::BlendMode;

use crate::plan::{LayerSource, RenderPlan};

/// Renders to straight RGBA8 over opaque black.
pub fn render(plan: &RenderPlan) -> Vec<u8> {
    let (w, h) = (plan.width as usize, plan.height as usize);
    // Premultiplied float accumulator.
    let mut acc = vec![[0f32, 0.0, 0.0, 1.0]; w * h];
    for layer in &plan.layers {
        let Some(inv) = layer.xform.inverse() else {
            continue;
        };
        // Bounding box of the transformed quad.
        let corners =
            [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)].map(|(u, v)| layer.xform.apply(u, v));
        let minx = corners
            .iter()
            .map(|c| c.0)
            .fold(f64::INFINITY, f64::min)
            .floor()
            .max(0.0) as usize;
        let maxx = corners
            .iter()
            .map(|c| c.0)
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil()
            .min(w as f64) as usize;
        let miny = corners
            .iter()
            .map(|c| c.1)
            .fold(f64::INFINITY, f64::min)
            .floor()
            .max(0.0) as usize;
        let maxy = corners
            .iter()
            .map(|c| c.1)
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil()
            .min(h as f64) as usize;
        for y in miny..maxy {
            for x in minx..maxx {
                let (u, v) = inv.apply(x as f64 + 0.5, y as f64 + 0.5);
                if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                    continue;
                }
                let s = match &layer.source {
                    LayerSource::Solid(c) => *c,
                    LayerSource::Frame(f) => sample_bilinear(&f.rgba, f.width, f.height, u, v),
                };
                let a = s[3] * layer.opacity;
                let src = [s[0] * a, s[1] * a, s[2] * a, a];
                let d = &mut acc[y * w + x];
                *d = blend(layer.blend, src, *d);
            }
        }
    }
    let mut out = vec![0u8; w * h * 4];
    for (i, p) in acc.iter().enumerate() {
        let a = p[3].clamp(0.0, 1.0);
        for c in 0..3 {
            let v = if a > 0.0 { p[c] / a } else { 0.0 };
            out[i * 4 + c] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
        out[i * 4 + 3] = (a * 255.0).round() as u8;
    }
    out
}

/// Premultiplied blending, mirroring the GPU pipelines' fixed-function state.
pub(crate) fn blend(mode: BlendMode, s: [f32; 4], d: [f32; 4]) -> [f32; 4] {
    let mut o = [0f32; 4];
    for i in 0..4 {
        o[i] = match mode {
            BlendMode::Normal => s[i] + d[i] * (1.0 - s[3]),
            BlendMode::Add => s[i] + d[i],
            BlendMode::Multiply => {
                if i < 3 {
                    s[i] * d[i] + d[i] * (1.0 - s[3])
                } else {
                    s[3] + d[3] * (1.0 - s[3])
                }
            }
            BlendMode::Screen => s[i] + d[i] * (1.0 - s[i]),
        };
    }
    o.map(|v| v.clamp(0.0, 1.0))
}

/// Bilinear sample with clamp-to-edge, texel centres at +0.5.
fn sample_bilinear(rgba: &[u8], w: u32, h: u32, u: f64, v: f64) -> [f32; 4] {
    let fx = (u * w as f64 - 0.5).clamp(0.0, (w - 1) as f64);
    let fy = (v * h as f64 - 0.5).clamp(0.0, (h - 1) as f64);
    let (x0, y0) = (fx.floor() as u32, fy.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
    let px = |x: u32, y: u32| {
        let i = ((y * w + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]].map(|c| c as f32 / 255.0)
    };
    let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
    let mut o = [0f32; 4];
    for i in 0..4 {
        let top = a[i] + (b[i] - a[i]) * tx;
        let bot = c[i] + (d[i] - c[i]) * tx;
        o[i] = top + (bot - top) * ty;
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Affine, Layer};
    use cutroom_core::model::Transform;
    use cutroom_core::{ClipId, Ticks};
    use cutroom_media::VideoFrame;
    use std::sync::Arc;

    fn plan(layers: Vec<Layer>) -> RenderPlan {
        RenderPlan {
            width: 8,
            height: 4,
            tick: Ticks::ZERO,
            layers,
            missing: vec![],
        }
    }

    fn solid(c: [f32; 4], t: Transform, opacity: f32, blend: BlendMode) -> Layer {
        Layer {
            clip: ClipId::new(),
            source: LayerSource::Solid(c),
            source_key: None,
            xform: Affine::for_layer(8.0, 4.0, 8.0, 4.0, &t),
            opacity,
            blend,
        }
    }

    #[test]
    fn empty_is_black() {
        let px = render(&plan(vec![]));
        assert!(px.chunks(4).all(|p| p == [0, 0, 0, 255]));
    }

    #[test]
    fn opacity_and_blend() {
        let red = solid(
            [1.0, 0.0, 0.0, 1.0],
            Transform::default(),
            1.0,
            BlendMode::Normal,
        );
        let green_half = solid(
            [0.0, 1.0, 0.0, 1.0],
            Transform::default(),
            0.5,
            BlendMode::Normal,
        );
        let px = render(&plan(vec![red.clone(), green_half]));
        assert_eq!(&px[..4], &[128, 128, 0, 255]);
        let add = solid(
            [0.0, 0.0, 1.0, 1.0],
            Transform::default(),
            1.0,
            BlendMode::Add,
        );
        let px = render(&plan(vec![red, add]));
        assert_eq!(&px[..4], &[255, 0, 255, 255]);
    }

    #[test]
    fn half_scale_covers_centre_only() {
        let t = Transform {
            scale_x: 0.5,
            scale_y: 0.5,
            ..Transform::default()
        };
        let px = render(&plan(vec![solid([1.0; 4], t, 1.0, BlendMode::Normal)]));
        let at = |x: usize, y: usize| &px[(y * 8 + x) * 4..(y * 8 + x) * 4 + 4];
        assert_eq!(at(0, 0), [0, 0, 0, 255]);
        assert_eq!(at(3, 1), [255, 255, 255, 255]);
        assert_eq!(at(7, 3), [0, 0, 0, 255]);
    }

    #[test]
    fn frame_is_sampled_one_to_one() {
        let rgba: Vec<u8> = (0..8 * 4).flat_map(|i| [i as u8 * 7, 0, 0, 255]).collect();
        let f = VideoFrame {
            width: 8,
            height: 4,
            index: 0,
            rgba: Arc::from(rgba.clone()),
        };
        let l = Layer {
            clip: ClipId::new(),
            source: LayerSource::Frame(f),
            source_key: None,
            xform: Affine::for_layer(8.0, 4.0, 8.0, 4.0, &Transform::default()),
            opacity: 1.0,
            blend: BlendMode::Normal,
        };
        assert_eq!(render(&plan(vec![l])), rgba);
    }
}
