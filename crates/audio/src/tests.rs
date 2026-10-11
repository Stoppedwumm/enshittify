//! Timeline mixing against real media (skipped without FFmpeg).

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use cutroom_core::command::{AddClip, AddMedia, InitProject, NewClip, SetClipAudio};
use cutroom_core::model::{ClipSource, Meta};
use cutroom_core::{ClipId, Document, Ticks};
use cutroom_media::cli::CliBackend;
use cutroom_media::{AudioFormat, MediaBackend};

use crate::{AudioEngine, TimelineAudio};

fn tone(dir: &std::path::Path) -> Option<(Arc<dyn MediaBackend>, PathBuf)> {
    let be = CliBackend::locate().ok()?;
    let p = dir.join("tone.wav");
    let ok = Command::new(be.ffmpeg_path())
        .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i"])
        .arg("sine=frequency=440:sample_rate=48000:duration=2")
        .args(["-ac", "2"])
        .arg(&p)
        .status()
        .ok()?
        .success();
    ok.then(|| (Arc::new(be) as Arc<dyn MediaBackend>, p))
}

fn project(be: &dyn MediaBackend, path: &std::path::Path) -> (Document, InitProject) {
    let mut d = Document::with_peer(1);
    let init = InitProject::new(Meta::default());
    d.execute(&init).unwrap();
    let info = be.probe(path).unwrap();
    let id = cutroom_media::hash::hash_file(path).unwrap();
    d.execute(&AddMedia(info.to_entry(id, "tone"))).unwrap();
    // Clip on A1 starting at 1 s, using source 0..1 s.
    d.execute(&AddClip {
        seq: init.seq,
        clip: NewClip {
            id: ClipId::new(),
            name: "tone".into(),
            track: init.audio_track,
            start: Ticks::from_secs(1),
            in_tick: Ticks::ZERO,
            out_tick: Ticks::from_secs(1),
            source: ClipSource::Media(id),
        },
    })
    .unwrap();
    (d, init)
}

fn rms(b: &[f32]) -> f64 {
    (b.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / b.len().max(1) as f64).sqrt()
}

#[test]
fn timeline_places_audio_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let Some((be, path)) = tone(dir.path()) else {
        return;
    };
    let (mut d, init) = project(be.as_ref(), &path);
    let p2 = path.clone();
    let resolve = Arc::new(move |_| Some(p2.clone()));
    let mut tl = TimelineAudio::new(
        d.snapshot(),
        init.seq,
        AudioFormat::STEREO_48K,
        be.clone(),
        resolve.clone(),
        Ticks::ZERO,
    );
    let mut buf = vec![0f32; 48_000 * 2 * 3];
    for chunk in buf.chunks_mut(1000 * 2) {
        tl.fill(chunk);
    }
    // Silence before 1 s, tone in 1..2 s, silence after.
    let sec = |s: usize| &buf[s * 96_000..(s + 1) * 96_000];
    assert!(rms(sec(0)) < 1e-6);
    assert!(rms(sec(1)) > 0.05);
    assert!(rms(sec(2)) < 1e-6);
    assert_eq!(buf[48_000 * 2 - 2], 0.0);
    assert_ne!(buf[48_000 * 2 + 200], 0.0);

    // Gain −∞ silences the clip.
    let clip = *d.snapshot().main().unwrap().clips.keys().next().unwrap();
    d.execute(&SetClipAudio {
        seq: init.seq,
        clip,
        gain_db: Some(-96.0),
        pan: None,
    })
    .unwrap();
    let mut tl = TimelineAudio::new(
        d.snapshot(),
        init.seq,
        AudioFormat::STEREO_48K,
        be,
        resolve,
        Ticks::from_secs(1),
    );
    let mut b = vec![0f32; 9600];
    tl.fill(&mut b);
    assert!(rms(&b) < 1e-6);
}

#[test]
fn engine_clock_advances_in_realtime_with_null_output() {
    let dir = tempfile::tempdir().unwrap();
    let Some((be, path)) = tone(dir.path()) else {
        return;
    };
    let (mut d, init) = project(be.as_ref(), &path);
    let mut eng = AudioEngine::new(be, false);
    let p2 = path.clone();
    eng.play(
        d.snapshot(),
        init.seq,
        Ticks::from_secs(1),
        Arc::new(move |_| Some(p2.clone())),
    );
    std::thread::sleep(Duration::from_millis(400));
    let pos = eng.position().unwrap();
    assert!(pos > Ticks::from_secs(1), "{pos:?}");
    assert!(pos < Ticks::from_secs(2), "{pos:?}");
    eng.stop();
    assert!(eng.position().is_none());
}
