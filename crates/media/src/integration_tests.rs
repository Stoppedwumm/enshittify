//! Backend tests against real encoded media (skipped without FFmpeg).

use std::sync::Arc;
use std::time::Duration;

use cutroom_core::model::MediaKind;
use cutroom_core::{MediaId, Timebase};

use crate::backend::{DecodeOptions, MediaBackend};
use crate::cache::Quality;
use crate::frame::AudioFormat;
use crate::server::FrameServer;
use crate::testutil::*;

fn backends() -> Vec<Arc<dyn MediaBackend>> {
    let mut v: Vec<Arc<dyn MediaBackend>> = Vec::new();
    if let Some(b) = backend() {
        v.push(Arc::new(b));
    }
    #[cfg(feature = "ffmpeg")]
    v.push(Arc::new(
        crate::native::NativeBackend::new().expect("native ffmpeg"),
    ));
    v
}

#[test]
fn frame_accurate_seek_at_common_rates() {
    let Some(cli) = backend() else { return };
    let dir = tempfile::tempdir().unwrap();
    for (rate, tb) in [
        ("30000/1001", Timebase::FPS_29_97),
        ("25", Timebase::FPS_25),
        ("24000/1001", Timebase::FPS_23_976),
    ] {
        let name = format!("c{}.mp4", tb.num);
        let path = counter_video(&cli, dir.path(), &name, rate, 1100);
        for be in backends() {
            let info = be.probe(&path).unwrap();
            assert_eq!(info.kind, MediaKind::Video);
            assert_eq!(info.fps, Some(tb), "{} fps via {}", rate, be.name());
            assert_eq!(
                info.frames(),
                1100,
                "{} frame count via {}",
                rate,
                be.name()
            );
            assert!(info.has_audio());
            let mut dec = be.open_video(&path, &info, DecodeOptions::FULL).unwrap();
            for target in [1000, 0, 999, 1, 517, 1099] {
                let f = dec.frame_at(target).unwrap().expect("frame");
                assert_eq!(f.index, target);
                assert_eq!(
                    read_counter(&f),
                    target,
                    "{} frame {target} via {}",
                    rate,
                    be.name()
                );
            }
            // Sequential decode after a seek.
            dec.seek(1000).unwrap();
            for expect in 1000..1010 {
                let f = dec.decode_next().unwrap().unwrap();
                assert_eq!(read_counter(&f), expect);
            }
        }
    }
}

#[test]
fn audio_decode_and_seek() {
    let Some(cli) = backend() else { return };
    let dir = tempfile::tempdir().unwrap();
    let path = counter_video(&cli, dir.path(), "a.mp4", "25", 100);
    for be in backends() {
        let mut dec = be.open_audio(&path, AudioFormat::STEREO_48K).unwrap();
        let mut buf = vec![0f32; 4800 * 2];
        dec.seek(24_000).unwrap();
        let mut total = 0;
        let mut energy = 0.0f64;
        while total < 48_000 {
            let n = dec.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            energy += buf[..n * 2]
                .iter()
                .map(|s| (*s as f64).powi(2))
                .sum::<f64>();
            total += n;
        }
        assert!(total >= 48_000, "{} decoded {total}", be.name());
        let rms = (energy / (total * 2) as f64).sqrt();
        assert!(rms > 0.05, "{} rms {rms}", be.name());
    }
}

#[test]
fn frame_server_serves_exact_frames_and_prefetches() {
    let Some(cli) = backend() else { return };
    let dir = tempfile::tempdir().unwrap();
    let path = counter_video(&cli, dir.path(), "s.mp4", "25", 300);
    for be in backends() {
        let info = be.probe(&path).unwrap();
        let srv = FrameServer::new(be.clone(), 256 << 20);
        let id = MediaId::of_bytes(b"s");
        srv.register(id, path.clone(), info);
        let f = srv
            .get_blocking(id, 200, Quality::Full, Duration::from_secs(20))
            .expect("frame 200");
        assert_eq!(read_counter(&f), 200);
        // Look-ahead fills the following frames without further requests.
        let f = srv
            .get_blocking(id, 205, Quality::Full, Duration::from_secs(20))
            .unwrap();
        assert_eq!(read_counter(&f), 205);
        // Non-blocking request returns something immediately.
        assert!(srv.request(id, 10, Quality::Full).is_some());
        let half = srv
            .get_blocking(id, 50, Quality::Half, Duration::from_secs(20))
            .unwrap();
        assert_eq!((half.width, half.height), (160, 90));
        assert_eq!(read_counter(&half), 50);
        // Beyond the end clamps to the last frame.
        let f = srv
            .get_blocking(id, 10_000, Quality::Full, Duration::from_secs(20))
            .unwrap();
        assert_eq!(read_counter(&f), 299);
    }
}

#[test]
fn thumbnails_are_generated() {
    let Some(cli) = backend() else { return };
    let dir = tempfile::tempdir().unwrap();
    let path = counter_video(&cli, dir.path(), "t.mp4", "25", 50);
    let info = cli.probe(&path).unwrap();
    let id = crate::hash::hash_file(&path).unwrap();
    let cache = dir.path().join("cache");
    let (thumb, strip) = crate::thumbs::ensure_thumbs(&cli, &cache, id, &path, &info).unwrap();
    let t = image::open(&thumb).unwrap();
    assert_eq!((t.width(), t.height()), (320, 180));
    let s = image::open(strip.unwrap()).unwrap();
    assert_eq!(s.width(), 320 * crate::thumbs::STRIP_FRAMES);
}
