//! Frame-accurate time.
//!
//! All edit positions are stored as [`Ticks`] — `i64` counts of *flicks*
//! (1/705 600 000 s). Flicks divide evenly into every common film/video frame
//! rate (including the NTSC 1000/1001 family) and every common audio sample
//! rate, so mixed-rate media maps to exact integers. Floating point seconds are
//! only ever produced for display or for handing a seek hint to a decoder.

use std::fmt;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};

use serde::{Deserialize, Serialize};

/// Flicks per second. 705 600 000 = 2^8·3^2·5^5·7^2.
pub const FLICKS_PER_SECOND: i64 = 705_600_000;

/// A position or duration on the project time axis, in flicks.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Ticks(pub i64);

impl Ticks {
    pub const ZERO: Ticks = Ticks(0);
    pub const ONE_SECOND: Ticks = Ticks(FLICKS_PER_SECOND);

    pub const fn new(v: i64) -> Self {
        Ticks(v)
    }

    pub const fn get(self) -> i64 {
        self.0
    }

    /// Whole seconds → ticks.
    pub const fn from_secs(s: i64) -> Self {
        Ticks(s * FLICKS_PER_SECOND)
    }

    /// Lossy conversion for display and decoder seek hints only.
    pub fn as_secs_f64(self) -> f64 {
        self.0 as f64 / FLICKS_PER_SECOND as f64
    }

    /// Lossy conversion from seconds, rounded to the nearest flick. Use only
    /// for values coming *from* the outside world (e.g. container durations).
    pub fn from_secs_f64(s: f64) -> Self {
        Ticks((s * FLICKS_PER_SECOND as f64).round() as i64)
    }

    /// Exact conversion from a rational timestamp `pts * tb_num / tb_den`
    /// seconds (an FFmpeg `AVRational` time base), rounded to nearest.
    pub fn from_rational(pts: i64, tb_num: i64, tb_den: i64) -> Self {
        assert!(tb_den != 0, "zero time base denominator");
        let n = pts as i128 * tb_num as i128 * FLICKS_PER_SECOND as i128;
        Ticks(div_round(n, tb_den as i128) as i64)
    }

    /// Audio sample index → ticks (exact for all common rates).
    pub fn from_samples(samples: i64, sample_rate: u32) -> Self {
        Ticks::from_rational(samples, 1, sample_rate as i64)
    }

    /// Ticks → audio sample index, floored.
    pub fn to_samples(self, sample_rate: u32) -> i64 {
        div_floor(
            self.0 as i128 * sample_rate as i128,
            FLICKS_PER_SECOND as i128,
        ) as i64
    }

    pub fn max(self, o: Ticks) -> Ticks {
        Ticks(self.0.max(o.0))
    }

    pub fn min(self, o: Ticks) -> Ticks {
        Ticks(self.0.min(o.0))
    }

    pub fn clamp(self, lo: Ticks, hi: Ticks) -> Ticks {
        Ticks(self.0.clamp(lo.0, hi.0))
    }
}

impl Add for Ticks {
    type Output = Ticks;
    fn add(self, o: Ticks) -> Ticks {
        Ticks(self.0 + o.0)
    }
}
impl Sub for Ticks {
    type Output = Ticks;
    fn sub(self, o: Ticks) -> Ticks {
        Ticks(self.0 - o.0)
    }
}
impl AddAssign for Ticks {
    fn add_assign(&mut self, o: Ticks) {
        self.0 += o.0
    }
}
impl SubAssign for Ticks {
    fn sub_assign(&mut self, o: Ticks) {
        self.0 -= o.0
    }
}
impl Neg for Ticks {
    type Output = Ticks;
    fn neg(self) -> Ticks {
        Ticks(-self.0)
    }
}

fn div_floor(n: i128, d: i128) -> i128 {
    let q = n / d;
    if (n % d != 0) && ((n < 0) != (d < 0)) {
        q - 1
    } else {
        q
    }
}

fn div_round(n: i128, d: i128) -> i128 {
    // Round half away from zero.
    let (n, d) = if d < 0 { (-n, -d) } else { (n, d) };
    if n >= 0 {
        (n + d / 2) / d
    } else {
        -((-n + d / 2) / d)
    }
}

/// A rational frame rate, e.g. `30000/1001` for 29.97 fps.
///
/// `num/den` is *frames per second*.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Timebase {
    pub num: u32,
    pub den: u32,
}

impl Timebase {
    pub const FPS_23_976: Timebase = Timebase::new(24000, 1001);
    pub const FPS_24: Timebase = Timebase::new(24, 1);
    pub const FPS_25: Timebase = Timebase::new(25, 1);
    pub const FPS_29_97: Timebase = Timebase::new(30000, 1001);
    pub const FPS_30: Timebase = Timebase::new(30, 1);
    pub const FPS_50: Timebase = Timebase::new(50, 1);
    pub const FPS_59_94: Timebase = Timebase::new(60000, 1001);
    pub const FPS_60: Timebase = Timebase::new(60, 1);

    pub const fn new(num: u32, den: u32) -> Self {
        assert!(num > 0 && den > 0, "timebase must be positive");
        Timebase { num, den }
    }

    /// Builds a timebase from an arbitrary rational (e.g. from a container)
    /// and reduces it.
    pub fn reduced(num: u32, den: u32) -> Self {
        let g = gcd(num as u64, den as u64) as u32;
        Timebase::new(num / g, den / g)
    }

    /// Snaps a floating-point rate to the closest well-known rate if within
    /// 0.01 fps; otherwise approximates with a /1000 denominator.
    pub fn from_fps_f64(fps: f64) -> Self {
        const KNOWN: [Timebase; 8] = [
            Timebase::FPS_23_976,
            Timebase::FPS_24,
            Timebase::FPS_25,
            Timebase::FPS_29_97,
            Timebase::FPS_30,
            Timebase::FPS_50,
            Timebase::FPS_59_94,
            Timebase::FPS_60,
        ];
        for k in KNOWN {
            if (k.fps_f64() - fps).abs() < 0.01 {
                return k;
            }
        }
        Timebase::reduced((fps * 1000.0).round().max(1.0) as u32, 1000)
    }

    pub fn fps_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// Nominal integer frame rate used for timecode (30 for 29.97, 24 for 23.976).
    pub fn nominal_fps(self) -> u32 {
        (self.num as f64 / self.den as f64).round() as u32
    }

    /// Duration of `n` frames as ticks (start of frame `n`), rounded to nearest flick.
    /// Exact for every rate whose frame duration is an integer number of flicks
    /// (all rates in this module).
    pub fn frames_to_ticks(self, frames: i64) -> Ticks {
        Ticks::from_rational(frames, self.den as i64, self.num as i64)
    }

    /// The frame index containing `t` (floor).
    pub fn ticks_to_frame(self, t: Ticks) -> i64 {
        div_floor(
            t.0 as i128 * self.num as i128,
            FLICKS_PER_SECOND as i128 * self.den as i128,
        ) as i64
    }

    /// The nearest frame boundary to `t`, as a frame index.
    pub fn ticks_to_nearest_frame(self, t: Ticks) -> i64 {
        div_round(
            t.0 as i128 * self.num as i128,
            FLICKS_PER_SECOND as i128 * self.den as i128,
        ) as i64
    }

    /// Snaps `t` down to the start of its frame.
    pub fn snap_floor(self, t: Ticks) -> Ticks {
        self.frames_to_ticks(self.ticks_to_frame(t))
    }

    /// Snaps `t` to the nearest frame boundary.
    pub fn snap(self, t: Ticks) -> Ticks {
        self.frames_to_ticks(self.ticks_to_nearest_frame(t))
    }

    /// Duration of one frame.
    pub fn frame_duration(self) -> Ticks {
        self.frames_to_ticks(1)
    }

    /// Whether this rate is conventionally shown with drop-frame timecode.
    pub fn supports_drop_frame(self) -> bool {
        self == Timebase::FPS_29_97 || self == Timebase::FPS_59_94
    }
}

impl fmt::Display for Timebase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{:.3}", self.fps_f64())
        }
    }
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

/// SMPTE timecode, optionally drop-frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timecode {
    pub negative: bool,
    pub hours: u32,
    pub minutes: u32,
    pub seconds: u32,
    pub frames: u32,
    pub drop_frame: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TimecodeError {
    #[error("malformed timecode: {0}")]
    Malformed(String),
    #[error("field out of range in {0}")]
    OutOfRange(String),
    #[error("frame {0} does not exist in drop-frame timecode")]
    DroppedFrame(String),
    #[error("drop-frame timecode requires 29.97 or 59.94 fps")]
    DropFrameUnsupported,
}

impl Timecode {
    /// Frames dropped per minute (except every 10th) for a DF rate.
    fn drop_count(tb: Timebase) -> i64 {
        // 2 for 29.97, 4 for 59.94.
        (tb.nominal_fps() as i64 / 30) * 2
    }

    /// Frame number → timecode. Drop-frame is only honoured for 29.97/59.94.
    pub fn from_frame(frame: i64, tb: Timebase, drop_frame: bool) -> Timecode {
        let drop_frame = drop_frame && tb.supports_drop_frame();
        let negative = frame < 0;
        let mut f = frame.abs();
        let fps = tb.nominal_fps() as i64;
        if drop_frame {
            let d = Self::drop_count(tb);
            let frames_per_10min = fps * 600 - d * 9;
            let frames_per_min = fps * 60 - d;
            let tens = f / frames_per_10min;
            let rem = f % frames_per_10min;
            f += 9 * d * tens;
            if rem > d {
                f += d * ((rem - d) / frames_per_min);
            }
        }
        Timecode {
            negative,
            hours: (f / (fps * 3600)) as u32,
            minutes: ((f / (fps * 60)) % 60) as u32,
            seconds: ((f / fps) % 60) as u32,
            frames: (f % fps) as u32,
            drop_frame,
        }
    }

    pub fn from_ticks(t: Ticks, tb: Timebase, drop_frame: bool) -> Timecode {
        Self::from_frame(tb.ticks_to_frame(t), tb, drop_frame)
    }

    /// Timecode → frame number.
    pub fn to_frame(&self, tb: Timebase) -> Result<i64, TimecodeError> {
        let fps = tb.nominal_fps() as i64;
        if self.drop_frame && !tb.supports_drop_frame() {
            return Err(TimecodeError::DropFrameUnsupported);
        }
        if self.minutes >= 60 || self.seconds >= 60 || self.frames as i64 >= fps {
            return Err(TimecodeError::OutOfRange(self.to_string()));
        }
        let total_minutes = self.hours as i64 * 60 + self.minutes as i64;
        let mut f = (total_minutes * 60 + self.seconds as i64) * fps + self.frames as i64;
        if self.drop_frame {
            let d = Self::drop_count(tb);
            if self.seconds == 0 && (self.frames as i64) < d && !self.minutes.is_multiple_of(10) {
                return Err(TimecodeError::DroppedFrame(self.to_string()));
            }
            f -= d * (total_minutes - total_minutes / 10);
        }
        Ok(if self.negative { -f } else { f })
    }

    /// Parses `HH:MM:SS:FF`, `HH:MM:SS;FF` (drop-frame), shorter forms like
    /// `SS:FF` / `MM:SS:FF`, or a bare frame number.
    pub fn parse(s: &str, tb: Timebase) -> Result<Timecode, TimecodeError> {
        let s = s.trim();
        let (negative, body) = match s.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, s),
        };
        if body.is_empty() {
            return Err(TimecodeError::Malformed(s.into()));
        }
        if body.chars().all(|c| c.is_ascii_digit()) {
            let n: i64 = body
                .parse()
                .map_err(|_| TimecodeError::Malformed(s.into()))?;
            let tc = Timecode::from_frame(n, tb, tb.supports_drop_frame());
            return Ok(Timecode { negative, ..tc });
        }
        let drop_frame = body.contains(';');
        let parts: Vec<&str> = body.split([':', ';', '.']).collect();
        if parts.len() > 4 || parts.iter().any(|p| p.is_empty()) {
            return Err(TimecodeError::Malformed(s.into()));
        }
        let mut nums = [0u32; 4];
        let off = 4 - parts.len();
        for (i, p) in parts.iter().enumerate() {
            nums[off + i] = p
                .parse()
                .map_err(|_| TimecodeError::Malformed(s.into()))?;
        }
        let tc = Timecode {
            negative,
            hours: nums[0],
            minutes: nums[1],
            seconds: nums[2],
            frames: nums[3],
            drop_frame,
        };
        tc.to_frame(tb)?; // validate
        Ok(tc)
    }
}

impl fmt::Display for Timecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sep = if self.drop_frame { ';' } else { ':' };
        write!(
            f,
            "{}{:02}:{:02}:{:02}{}{:02}",
            if self.negative { "-" } else { "" },
            self.hours,
            self.minutes,
            self.seconds,
            sep,
            self.frames
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn frame_durations_are_exact() {
        for tb in [
            Timebase::FPS_23_976,
            Timebase::FPS_24,
            Timebase::FPS_25,
            Timebase::FPS_29_97,
            Timebase::FPS_30,
            Timebase::FPS_50,
            Timebase::FPS_59_94,
            Timebase::FPS_60,
        ] {
            let n = FLICKS_PER_SECOND as i128 * tb.den as i128;
            assert_eq!(n % tb.num as i128, 0, "{tb} not exact");
        }
        for sr in [8000, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 192000] {
            assert_eq!(FLICKS_PER_SECOND % sr, 0, "{sr} not exact");
        }
    }

    #[test]
    fn frame_1000_at_ntsc() {
        let tb = Timebase::FPS_29_97;
        let t = tb.frames_to_ticks(1000);
        assert_eq!(t, Ticks(1000 * 23_543_520));
        assert_eq!(tb.ticks_to_frame(t), 1000);
        assert_eq!(tb.ticks_to_frame(t - Ticks(1)), 999);
    }

    #[test]
    fn drop_frame_known_values() {
        let tb = Timebase::FPS_29_97;
        let tc = |f| Timecode::from_frame(f, tb, true).to_string();
        assert_eq!(tc(0), "00:00:00;00");
        assert_eq!(tc(1799), "00:00:59;29");
        assert_eq!(tc(1800), "00:01:00;02");
        assert_eq!(tc(17982), "00:10:00;00");
        assert_eq!(tc(107892), "01:00:00;00");
        let tb60 = Timebase::FPS_59_94;
        assert_eq!(
            Timecode::from_frame(3600, tb60, true).to_string(),
            "00:01:00;04"
        );
        assert!(matches!(
            Timecode::parse("00:01:00;01", tb),
            Err(TimecodeError::DroppedFrame(_))
        ));
        assert_eq!(
            Timecode::parse("00:10:00;00", tb).unwrap().to_frame(tb),
            Ok(17982)
        );
    }

    #[test]
    fn non_drop_and_short_forms() {
        let tb = Timebase::FPS_25;
        assert_eq!(Timecode::from_frame(1000, tb, false).to_string(), "00:00:40:00");
        assert_eq!(Timecode::parse("40:00", tb).unwrap().to_frame(tb), Ok(1000));
        assert_eq!(Timecode::parse("1000", tb).unwrap().to_frame(tb), Ok(1000));
        assert_eq!(Timecode::parse("-1:00", tb).unwrap().to_frame(tb), Ok(-25));
        assert!(Timecode::parse("00:00:00:25", tb).is_err());
        assert!(Timecode::parse("a:b", tb).is_err());
        // Drop-frame flag is ignored for rates that don't support it.
        assert!(!Timecode::from_frame(10, tb, true).drop_frame);
    }

    #[test]
    fn samples_round_trip() {
        let t = Ticks::from_samples(48_000 * 600, 48_000);
        assert_eq!(t, Ticks::from_secs(600));
        assert_eq!(t.to_samples(48_000), 48_000 * 600);
        assert_eq!(Ticks::from_rational(1001, 1, 30000), Timebase::FPS_29_97.frame_duration());
    }

    #[test]
    fn from_fps_snaps() {
        assert_eq!(Timebase::from_fps_f64(29.97002997), Timebase::FPS_29_97);
        assert_eq!(Timebase::from_fps_f64(23.976), Timebase::FPS_23_976);
        assert_eq!(Timebase::from_fps_f64(12.5), Timebase::new(25, 2));
    }

    proptest! {
        #[test]
        fn frame_tick_round_trip(f in -10_000_000i64..10_000_000, i in 0usize..8) {
            let tbs = [Timebase::FPS_23_976, Timebase::FPS_24, Timebase::FPS_25, Timebase::FPS_29_97,
                       Timebase::FPS_30, Timebase::FPS_50, Timebase::FPS_59_94, Timebase::FPS_60];
            let tb = tbs[i];
            let t = tb.frames_to_ticks(f);
            prop_assert_eq!(tb.ticks_to_frame(t), f);
            prop_assert_eq!(tb.ticks_to_frame(t + tb.frame_duration() - Ticks(1)), f);
            prop_assert_eq!(tb.snap(t + Ticks(3)), t);
        }

        #[test]
        fn drop_frame_round_trip(f in 0i64..(24 * 3600 * 60), sixty in any::<bool>()) {
            let tb = if sixty { Timebase::FPS_59_94 } else { Timebase::FPS_29_97 };
            let tc = Timecode::from_frame(f, tb, true);
            prop_assert_eq!(tc.to_frame(tb), Ok(f));
            let parsed = Timecode::parse(&tc.to_string(), tb).unwrap();
            prop_assert_eq!(parsed, tc);
        }
    }
}
