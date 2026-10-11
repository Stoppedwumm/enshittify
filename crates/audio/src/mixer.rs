//! Mixing primitives.

/// dB → linear amplitude.
pub fn db_to_gain(db: f64) -> f32 {
    if db <= -96.0 {
        0.0
    } else {
        10f64.powf(db / 20.0) as f32
    }
}

/// Balance-style pan: centre keeps both channels at unity, hard left/right
/// silences the opposite side. `pan` in `[-1, 1]`.
pub fn pan_gains(pan: f64) -> (f32, f32) {
    let p = pan.clamp(-1.0, 1.0) as f32;
    ((1.0 - p).min(1.0), (1.0 + p).min(1.0))
}

/// Adds `src` (interleaved stereo) into `dst` with a per-frame gain ramp
/// from `g0` to `g1` and static pan gains.
pub fn mix_stereo(dst: &mut [f32], src: &[f32], g0: f32, g1: f32, pan: (f32, f32)) {
    let frames = dst.len().min(src.len()) / 2;
    if frames == 0 {
        return;
    }
    let step = (g1 - g0) / frames as f32;
    for i in 0..frames {
        let g = g0 + step * i as f32;
        dst[2 * i] += src[2 * i] * g * pan.0;
        dst[2 * i + 1] += src[2 * i + 1] * g * pan.1;
    }
}

/// Hard clip to [-1, 1] (the master bus has no limiter yet).
pub fn clip(buf: &mut [f32]) {
    for s in buf {
        *s = s.clamp(-1.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gains() {
        assert_eq!(db_to_gain(0.0), 1.0);
        assert!((db_to_gain(-6.0) - 0.501).abs() < 0.001);
        assert_eq!(db_to_gain(-120.0), 0.0);
        assert_eq!(pan_gains(0.0), (1.0, 1.0));
        assert_eq!(pan_gains(-1.0), (1.0, 0.0));
        assert_eq!(pan_gains(1.0), (0.0, 1.0));
    }

    #[test]
    fn ramp_mix() {
        let mut d = vec![0.0; 8];
        mix_stereo(&mut d, &[1.0; 8], 0.0, 1.0, (1.0, 0.5));
        assert_eq!(d[0], 0.0);
        assert_eq!(d[6], 0.75);
        assert_eq!(d[7], 0.375);
    }
}
