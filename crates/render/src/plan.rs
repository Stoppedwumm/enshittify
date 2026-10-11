//! Render plans: the pure, rasterizer-independent half of rendering.

use cutroom_core::model::{BlendMode, ClipSource, DocSnapshot, Generator, MediaKind, Transform};
use cutroom_core::normalize::visible_clip_at;
use cutroom_core::{ClipId, MediaId, SeqId, Ticks};
use cutroom_media::VideoFrame;

/// Supplies decoded frames. Preview uses a non-blocking implementation
/// (returns the nearest available frame); export blocks for exact frames.
pub trait FrameProvider {
    fn frame(&self, media: MediaId, index: i64) -> Option<VideoFrame>;
}

/// 2×3 affine map from layer UV (`[0,1]²`, origin top-left) to output pixels:
/// `x = m[0]·u + m[1]·v + m[2]`, `y = m[3]·u + m[4]·v + m[5]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine(pub [f64; 6]);

impl Affine {
    pub fn apply(&self, u: f64, v: f64) -> (f64, f64) {
        let m = &self.0;
        (m[0] * u + m[1] * v + m[2], m[3] * u + m[4] * v + m[5])
    }

    /// Inverse map (output pixel → UV); `None` if degenerate.
    pub fn inverse(&self) -> Option<Affine> {
        let [a, b, c, d, e, f] = self.0;
        let det = a * e - b * d;
        if det.abs() < 1e-12 {
            return None;
        }
        let (ia, ib, id, ie) = (e / det, -b / det, -d / det, a / det);
        Some(Affine([
            ia,
            ib,
            -(ia * c + ib * f),
            id,
            ie,
            -(id * c + ie * f),
        ]))
    }

    /// Layer geometry: a `w×h` source fitted ("contain") into `out_w×out_h`,
    /// then the clip transform applied about its anchor.
    pub fn for_layer(src_w: f64, src_h: f64, out_w: f64, out_h: f64, t: &Transform) -> Affine {
        let fit = (out_w / src_w).min(out_h / src_h);
        let sx = t.scale_x * fit * if t.flip_h { -1.0 } else { 1.0 };
        let sy = t.scale_y * fit * if t.flip_v { -1.0 } else { 1.0 };
        let (s, c) = t.rotation.to_radians().sin_cos();
        // p = T(centre + xy) · R · S(sx, sy) · T(-anchor·size) · S(w, h) · uv
        let (ax, ay) = (t.anchor_x * src_w, t.anchor_y * src_h);
        let (tx, ty) = (out_w / 2.0 + t.x, out_h / 2.0 + t.y);
        // Columns of R·S: (c·sx, s·sx), (-s·sy, c·sy).
        let (m00, m10, m01, m11) = (c * sx, s * sx, -s * sy, c * sy);
        Affine([
            m00 * src_w,
            m01 * src_h,
            tx - (m00 * ax + m01 * ay),
            m10 * src_w,
            m11 * src_h,
            ty - (m10 * ax + m11 * ay),
        ])
    }
}

#[derive(Clone, Debug)]
pub enum LayerSource {
    Frame(VideoFrame),
    /// Straight-alpha RGBA.
    Solid([f32; 4]),
}

#[derive(Clone, Debug)]
pub struct Layer {
    pub clip: ClipId,
    pub source: LayerSource,
    /// Identifies the source texture for GPU caching.
    pub source_key: Option<(MediaId, i64)>,
    pub xform: Affine,
    pub opacity: f32,
    pub blend: BlendMode,
}

#[derive(Clone, Debug)]
pub struct RenderPlan {
    pub width: u32,
    pub height: u32,
    pub tick: Ticks,
    /// Bottom (V1) first.
    pub layers: Vec<Layer>,
    /// Layers whose source frame was not available (e.g. still downloading).
    pub missing: Vec<ClipId>,
}

/// Builds the plan for `seq` at `tick`. Deterministic given the same
/// snapshot and frames.
pub fn build_plan(
    snap: &DocSnapshot,
    seq: SeqId,
    tick: Ticks,
    frames: &dyn FrameProvider,
) -> RenderPlan {
    let (w, h) = (snap.meta.width, snap.meta.height);
    let mut plan = RenderPlan {
        width: w,
        height: h,
        tick,
        layers: Vec::new(),
        missing: Vec::new(),
    };
    let Some(seq) = snap.sequences.get(&seq) else {
        return plan;
    };
    for track in seq.visual_tracks().filter(|t| !t.hidden) {
        let Some(clip) = visible_clip_at(seq, track.id, tick) else {
            continue;
        };
        let t = &clip.transform;
        let (source, key, sw, sh) = match &clip.source {
            ClipSource::Media(mid) => {
                let Some(m) = snap.media.get(mid) else {
                    plan.missing.push(clip.id);
                    continue;
                };
                let index = match (m.kind, m.fps) {
                    (MediaKind::Image, _) | (_, None) => 0,
                    (_, Some(fps)) => fps.ticks_to_frame(clip.source_tick_at(tick)),
                };
                match frames.frame(*mid, index) {
                    // Geometry uses the media's logical size so preview
                    // quality never changes layout.
                    Some(f) => (
                        LayerSource::Frame(f),
                        Some((*mid, index)),
                        m.width.max(1) as f64,
                        m.height.max(1) as f64,
                    ),
                    None => {
                        plan.missing.push(clip.id);
                        continue;
                    }
                }
            }
            ClipSource::Generator(Generator::Color(c)) => {
                (LayerSource::Solid(*c), None, w as f64, h as f64)
            }
            // Titles and adjustment layers render from the MVP onwards.
            ClipSource::Generator(_) => continue,
        };
        plan.layers.push(Layer {
            clip: clip.id,
            source,
            source_key: key,
            xform: Affine::for_layer(sw, sh, w as f64, h as f64, t),
            opacity: t.opacity.clamp(0.0, 1.0) as f32,
            blend: t.blend,
        });
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
    }

    #[test]
    fn identity_fit_fills_frame() {
        let a = Affine::for_layer(1920.0, 1080.0, 1920.0, 1080.0, &Transform::default());
        assert!(close(a.apply(0.0, 0.0), (0.0, 0.0)));
        assert!(close(a.apply(1.0, 1.0), (1920.0, 1080.0)));
        // 4:3 source is pillarboxed.
        let a = Affine::for_layer(640.0, 480.0, 1920.0, 1080.0, &Transform::default());
        assert!(close(a.apply(0.0, 0.0), (240.0, 0.0)));
        assert!(close(a.apply(1.0, 1.0), (1680.0, 1080.0)));
    }

    #[test]
    fn rotation_and_inverse() {
        let t = Transform {
            rotation: 90.0,
            x: 10.0,
            ..Transform::default()
        };
        let a = Affine::for_layer(100.0, 100.0, 100.0, 100.0, &t);
        // Top-left corner rotates clockwise to the top-right.
        assert!(close(a.apply(0.0, 0.0), (110.0, 0.0)));
        let inv = a.inverse().unwrap();
        let (x, y) = a.apply(0.3, 0.7);
        assert!(close(inv.apply(x, y), (0.3, 0.7)));
    }
}
