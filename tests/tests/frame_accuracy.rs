//! Acceptance 1 (frame accuracy): a burned-in counter video is split at frame
//! 1000; the right segment is moved to the start of the timeline and
//! exported. The first exported frame must read 1000. Run at 29.97, 25 and
//! 23.976 fps.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use cutroom_core::command::*;
use cutroom_core::model::{ClipSource, Meta};
use cutroom_core::{ClipId, Document, Ticks, Timebase};
use cutroom_export::{ExportJob, Preset, SourceFile, run_export};
use cutroom_media::{DecodeOptions, MediaBackend};
use cutroom_tests::*;

#[test]
fn split_at_1000_then_export_reads_1000() {
    let Some(cli) = ffmpeg() else { return };
    let backend: Arc<dyn MediaBackend> = Arc::new(cli.clone());
    let dir = tempfile::tempdir().unwrap();
    for (rate, tb) in [
        ("30000/1001", Timebase::FPS_29_97),
        ("25", Timebase::FPS_25),
        ("24000/1001", Timebase::FPS_23_976),
    ] {
        let src = counter_video(&cli, dir.path(), &format!("src{}.mp4", tb.num), rate, 1060);
        let info = backend.probe(&src).unwrap();
        let id = cutroom_media::hash::hash_file(&src).unwrap();

        let mut doc = Document::with_peer(1);
        let init = InitProject::new(Meta {
            timebase: tb,
            width: 320,
            height: 180,
            ..Meta::default()
        });
        doc.execute(&init).unwrap();
        doc.execute(&AddMedia(info.to_entry(id, "counter")))
            .unwrap();
        let clip = ClipId::new();
        doc.execute(&AddClip {
            seq: init.seq,
            clip: NewClip {
                id: clip,
                name: "counter".into(),
                track: init.video_track,
                start: Ticks::ZERO,
                in_tick: Ticks::ZERO,
                out_tick: info.duration,
                source: ClipSource::Media(id),
            },
        })
        .unwrap();
        let right = ClipId::new();
        doc.execute(&SplitClip {
            seq: init.seq,
            clip,
            at: tb.frames_to_ticks(1000),
            new_id: right,
        })
        .unwrap();
        doc.execute(&DeleteClips {
            seq: init.seq,
            clips: vec![clip],
        })
        .unwrap();
        doc.execute(&MoveClips {
            seq: init.seq,
            moves: vec![(right, init.video_track, Ticks::ZERO)],
        })
        .unwrap();

        let out = dir.path().join(format!("out{}.mp4", tb.num));
        let mut preset = Preset::h264(180, tb);
        preset.width = 320;
        preset.crf = 10;
        let job = ExportJob {
            snapshot: doc.snapshot(),
            seq: init.seq,
            start: Ticks::ZERO,
            end: tb.frames_to_ticks(10),
            preset,
            output: out.clone(),
            sources: vec![SourceFile {
                id,
                path: src.clone(),
                info: info.clone(),
            }],
            cpu_only: true,
        };
        let mut last = None;
        run_export(
            &job,
            backend.clone(),
            cli.ffmpeg_path(),
            &AtomicBool::new(false),
            |p| last = Some(p),
        )
        .unwrap();
        assert_eq!(last.unwrap().frame, 10);

        // Validate the file and read back the first frames.
        let oi = backend.probe(&out).unwrap();
        assert_eq!(oi.fps, Some(tb), "exported fps at {rate}");
        assert_eq!(oi.frames(), 10);
        assert!(oi.has_audio());
        let mut dec = backend.open_video(&out, &oi, DecodeOptions::FULL).unwrap();
        for k in 0..10 {
            let f = dec.frame_at(k).unwrap().unwrap();
            assert_eq!(read_counter(&f), 1000 + k, "{rate}: exported frame {k}");
        }
    }
}
