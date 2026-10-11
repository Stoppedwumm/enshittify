//! Export presets.

use cutroom_core::Timebase;

#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub id: String,
    pub label: String,
    pub width: u32,
    pub height: u32,
    pub fps: Timebase,
    /// x264 constant rate factor.
    pub crf: u8,
    pub audio_kbps: u32,
    pub extension: &'static str,
}

impl Preset {
    pub fn h264(height: u32, fps: Timebase) -> Self {
        let width = height * 16 / 9;
        let res = match height {
            2160 => "4K".to_string(),
            h => format!("{h}p"),
        };
        Preset {
            id: format!("mp4-h264-{height}p{}", fps),
            label: format!("MP4 · H.264/AAC · {res} · {fps} fps"),
            width,
            height,
            fps,
            crf: 18,
            audio_kbps: 192,
            extension: "mp4",
        }
    }

    /// ffmpeg output arguments for this preset (after the inputs).
    pub fn ffmpeg_output_args(&self) -> Vec<String> {
        let (w, h) = (self.width, self.height);
        let vf = format!(
            "scale={w}:{h}:force_original_aspect_ratio=decrease:out_color_matrix=bt709:out_range=tv,\
             pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,format=yuv420p"
        );
        [
            "-vf",
            &vf,
            "-c:v",
            "libx264",
            "-preset",
            "medium",
            "-crf",
            &self.crf.to_string(),
            "-profile:v",
            "high",
            "-pix_fmt",
            "yuv420p",
            "-colorspace",
            "bt709",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-color_range",
            "tv",
            "-c:a",
            "aac",
            "-b:a",
            &format!("{}k", self.audio_kbps),
            "-ar",
            "48000",
            "-movflags",
            "+faststart",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }
}

/// MVP presets: MP4 H.264/AAC at 720p/1080p/4K × 24/25/30/50/60 fps.
pub fn mvp_presets() -> Vec<Preset> {
    let mut v = Vec::new();
    for h in [720, 1080, 2160] {
        for fps in [
            Timebase::FPS_24,
            Timebase::FPS_25,
            Timebase::FPS_30,
            Timebase::FPS_50,
            Timebase::FPS_60,
        ] {
            v.push(Preset::h264(h, fps));
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_matrix() {
        let p = mvp_presets();
        assert_eq!(p.len(), 15);
        assert!(p.iter().any(|p| p.width == 3840 && p.height == 2160));
        assert_eq!(p[0].width, 1280);
        let ids: std::collections::HashSet<_> = p.iter().map(|p| &p.id).collect();
        assert_eq!(ids.len(), 15);
    }
}
