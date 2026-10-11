//! Demo project generator (examples, smoke tests, `just demo`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::session::Session;
use cutroom_core::command::*;
use cutroom_core::model::{ClipSource, Generator, MarkerColor, Meta, TrackKind};
use cutroom_core::{ClipId, MarkerId, Ticks, Timebase, TrackId};
use cutroom_media::MediaBackend;
use cutroom_media::server::FrameServer;

/// Generates a 20 s 29.97 fps test clip and a project that uses it (split
/// clip, overlay on V2, markers). Returns the project folder.
pub fn create_demo_project(out: &Path) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(out)?;
    let cli = cutroom_media::cli::CliBackend::locate()?;
    let media = out.join("demo-testsrc.mp4");
    if !media.exists() {
        let ok = Command::new(cli.ffmpeg_path())
            .args(["-nostdin", "-loglevel", "error", "-y"])
            .args([
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=1280x720:rate=30000/1001:duration=20",
            ])
            .args([
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=330:sample_rate=48000:duration=20",
            ])
            .args([
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&media)
            .status()?
            .success();
        anyhow::ensure!(ok, "ffmpeg failed");
    }
    let project = out.join("Demo.cutroom");
    if project.exists() {
        std::fs::remove_dir_all(&project)?;
    }
    let backend: Arc<dyn MediaBackend> = Arc::new(cli);
    let frames = FrameServer::new(backend.clone(), 64 << 20);
    let mut s = Session::create(
        &project,
        Meta {
            timebase: Timebase::FPS_29_97,
            width: 1280,
            height: 720,
            ..Meta::default()
        },
    )?;
    s.import(vec![media.clone()], &backend);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut ready = vec![];
    while ready.is_empty() && Instant::now() < deadline {
        ready = s.poll_imports(&backend, &frames);
        std::thread::sleep(Duration::from_millis(20));
    }
    let id = *ready
        .first()
        .ok_or_else(|| anyhow::anyhow!("import timed out"))?;
    // Let thumbnails finish.
    let deadline = Instant::now() + Duration::from_secs(30);
    while s.library[&id].strip.is_none() && Instant::now() < deadline {
        s.poll_imports(&backend, &frames);
        std::thread::sleep(Duration::from_millis(20));
    }
    let snap = s.snapshot();
    let seq = snap.main().unwrap();
    let (sid, v1) = (seq.id, seq.tracks[0].id);
    let entry = snap.media[&id].clone();
    let tb = snap.meta.timebase;
    let a = ClipId::new();
    s.execute(&AddClip {
        seq: sid,
        clip: NewClip {
            id: a,
            name: entry.name.clone(),
            track: v1,
            start: Ticks::ZERO,
            in_tick: Ticks::ZERO,
            out_tick: entry.duration,
            source: ClipSource::Media(id),
        },
    })?;
    s.execute(&SplitClip {
        seq: sid,
        clip: a,
        at: tb.frames_to_ticks(300),
        new_id: ClipId::new(),
    })?;
    let v2 = TrackId::new();
    s.execute(&AddTrack {
        seq: sid,
        id: v2,
        kind: TrackKind::Video,
        name: None,
        index: None,
    })?;
    let title = ClipId::new();
    s.execute(&AddClip {
        seq: sid,
        clip: NewClip {
            id: title,
            name: "Color overlay".into(),
            track: v2,
            start: Ticks::from_secs(4),
            in_tick: Ticks::ZERO,
            out_tick: Ticks::from_secs(4),
            source: ClipSource::Generator(Generator::Color([0.18, 0.83, 0.66, 1.0])),
        },
    })?;
    s.execute(&SetTransform {
        seq: sid,
        clip: title,
        fields: vec![
            TransformField::ScaleX(0.3),
            TransformField::ScaleY(0.3),
            TransformField::X(380.0),
            TransformField::Y(-200.0),
            TransformField::Opacity(0.8),
        ],
    })?;
    for (t, c, n) in [
        (2, MarkerColor::Blue, "Intro"),
        (10, MarkerColor::Orange, "Cut here"),
    ] {
        s.execute(&AddMarker {
            seq: sid,
            id: MarkerId::new(),
            tick: Ticks::from_secs(t),
            color: c,
            name: n.into(),
            author: "demo".into(),
        })?;
    }
    s.rename("Demo")?;
    s.save()?;
    Ok(s.store.root().to_path_buf())
}
