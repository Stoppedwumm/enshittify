use crate::command::*;
use crate::model::*;
use crate::normalize::normalize_track;
use crate::store::ProjectStore;
use crate::*;

pub(crate) fn media(seed: &[u8], secs: i64) -> MediaEntry {
    MediaEntry {
        id: MediaId::of_bytes(seed),
        name: "clip.mp4".into(),
        kind: MediaKind::Video,
        duration: Ticks::from_secs(secs),
        fps: Some(Timebase::FPS_29_97),
        width: 1920,
        height: 1080,
        audio_channels: 2,
        audio_sample_rate: 48_000,
        size_bytes: 1234,
        proxy_hash: None,
    }
}

fn new_project(peer: u64) -> (Document, InitProject) {
    let mut d = Document::with_peer(peer);
    let init = InitProject::new(Meta {
        timebase: Timebase::FPS_29_97,
        ..Meta::default()
    });
    d.execute(&init).unwrap();
    (d, init)
}

fn add_clip(d: &mut Document, init: &InitProject, m: &MediaEntry, start: Ticks) -> ClipId {
    let id = ClipId::new();
    d.execute(&AddClip {
        seq: init.seq,
        clip: NewClip {
            id,
            name: m.name.clone(),
            track: init.video_track,
            start,
            in_tick: Ticks::ZERO,
            out_tick: m.duration,
            source: ClipSource::Media(m.id),
        },
    })
    .unwrap();
    id
}

#[test]
fn init_creates_main_sequence_with_tracks() {
    let (mut d, init) = new_project(1);
    let s = d.snapshot();
    let seq = s.main().unwrap();
    assert_eq!(seq.id, init.seq);
    assert_eq!(seq.tracks.len(), 2);
    assert_eq!(seq.tracks[0].kind, TrackKind::Video);
    assert_eq!(s.meta.timebase, Timebase::FPS_29_97);
    assert!(d.execute(&init).is_err(), "double init rejected");
}

#[test]
fn split_is_frame_accurate() {
    let (mut d, init) = new_project(1);
    let m = media(b"a", 60);
    d.execute(&AddMedia(m.clone())).unwrap();
    let c = add_clip(&mut d, &init, &m, Ticks::ZERO);
    let tb = Timebase::FPS_29_97;
    let at = tb.frames_to_ticks(1000);
    let right = ClipId::new();
    d.execute(&SplitClip {
        seq: init.seq,
        clip: c,
        at,
        new_id: right,
    })
    .unwrap();
    let s = d.snapshot();
    let seq = s.main().unwrap();
    let r = &seq.clips[&right];
    assert_eq!(tb.ticks_to_frame(r.in_tick), 1000);
    assert_eq!(tb.ticks_to_frame(r.source_tick_at(r.start)), 1000);
    assert_eq!(seq.clips[&c].end(), r.start);
    assert_eq!(seq.clips[&c].duration() + r.duration(), m.duration);
}

#[test]
fn invalid_commands_leave_doc_untouched() {
    let (mut d, init) = new_project(1);
    let before = d.state_json();
    let m = media(b"x", 1);
    // Media not added → rejected.
    let r = d.execute(&AddClip {
        seq: init.seq,
        clip: NewClip {
            id: ClipId::new(),
            name: "x".into(),
            track: init.video_track,
            start: Ticks::ZERO,
            in_tick: Ticks::ZERO,
            out_tick: m.duration,
            source: ClipSource::Media(m.id),
        },
    });
    assert!(r.is_err());
    assert_eq!(before, d.state_json());
}

#[test]
fn undo_is_per_user() {
    let (mut a, init) = new_project(1);
    let mut b = Document::with_peer(2);
    b.import(&a.export_snapshot()).unwrap();
    let m = media(b"m", 10);
    a.execute(&AddMedia(m.clone())).unwrap();
    b.import(&a.export_updates_since(&b.version())).unwrap();

    let ca = add_clip(&mut a, &init, &m, Ticks::ZERO);
    let cb = add_clip(&mut b, &init, &m, Ticks::from_secs(20));
    a.import(&b.export_updates_since(&a.version())).unwrap();
    b.import(&a.export_updates_since(&b.version())).unwrap();
    assert_eq!(a.state_json(), b.state_json());

    // A undoes: only A's clip disappears.
    assert!(a.undo());
    let s = a.snapshot();
    let seq = s.main().unwrap();
    assert!(!seq.clips.contains_key(&ca));
    assert!(seq.clips.contains_key(&cb));
    assert!(a.redo());
    assert!(a.snapshot().main().unwrap().clips.contains_key(&ca));
}

#[test]
fn concurrent_moves_converge_and_conflicts_are_detected() {
    let (mut a, init) = new_project(1);
    let m = media(b"m", 10);
    a.execute(&AddMedia(m.clone())).unwrap();
    let c1 = add_clip(&mut a, &init, &m, Ticks::ZERO);
    let c2 = add_clip(&mut a, &init, &m, Ticks::from_secs(10));
    let mut b = Document::with_peer(2);
    b.import(&a.export_snapshot()).unwrap();

    // Offline: A moves c1 right, B moves c2 left — they overlap afterwards.
    a.execute(&MoveClips {
        seq: init.seq,
        moves: vec![(c1, init.video_track, Ticks::from_secs(3))],
    })
    .unwrap();
    b.execute(&MoveClips {
        seq: init.seq,
        moves: vec![(c2, init.video_track, Ticks::from_secs(8))],
    })
    .unwrap();
    // Both also tweak the same clip's opacity.
    a.execute(&SetTransform {
        seq: init.seq,
        clip: c1,
        fields: vec![TransformField::Opacity(0.5)],
    })
    .unwrap();
    b.execute(&SetTransform {
        seq: init.seq,
        clip: c1,
        fields: vec![TransformField::Opacity(0.25)],
    })
    .unwrap();

    let ua = a.export_updates_since(&b.version());
    let ub = b.export_updates_since(&a.version());
    a.import(&ub).unwrap();
    b.import(&ua).unwrap();
    b.import(&ua).unwrap(); // duplicate delivery is harmless
    assert_eq!(a.state_json(), b.state_json());

    let sa = a.snapshot();
    let seq = sa.main().unwrap();
    let v = normalize_track(seq, init.video_track);
    assert_eq!(v.conflicts.len(), 1);
    assert_eq!(v.conflicts[0].start, Ticks::from_secs(8));
    assert_eq!(v.conflicts[0].end, Ticks::from_secs(13));
    assert_eq!(normalize_track(b.snapshot().main().unwrap(), init.video_track), v);
}

#[test]
fn locked_track_rejects_edits() {
    let (mut d, init) = new_project(1);
    let m = media(b"m", 10);
    d.execute(&AddMedia(m.clone())).unwrap();
    let c = add_clip(&mut d, &init, &m, Ticks::ZERO);
    d.execute(&SetTrackProps {
        seq: init.seq,
        track: init.video_track,
        patch: TrackPatch {
            locked: Some(true),
            ..Default::default()
        },
    })
    .unwrap();
    assert!(matches!(
        d.execute(&DeleteClips {
            seq: init.seq,
            clips: vec![c]
        }),
        Err(CommandError::TrackLocked(_))
    ));
}

#[test]
fn store_round_trip_and_crash_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Film");
    let (mut d, init) = new_project(1);
    let mut store = ProjectStore::create(&path, &d).unwrap();
    assert!(store.root().ends_with("Film.cutroom"));

    let m = media(b"m", 10);
    d.execute(&AddMedia(m.clone())).unwrap();
    assert!(store.flush_incremental(&d).unwrap());
    let c = add_clip(&mut d, &init, &m, Ticks::ZERO);
    assert!(store.flush_incremental(&d).unwrap());
    assert!(!store.flush_incremental(&d).unwrap());
    // "kill -9": drop without saving, reopen from snapshot + oplog.
    let expected = d.state_json();
    drop(store);
    let (mut store2, mut d2) = ProjectStore::open(&dir.path().join("Film.cutroom")).unwrap();
    assert_eq!(d2.state_json(), expected);
    assert!(d2.snapshot().main().unwrap().clips.contains_key(&c));
    // Opening must not leave something to undo.
    assert!(!d2.can_undo());
    store2.save(&d2).unwrap();
    assert_eq!(
        std::fs::read_dir(store2.root().join("oplog")).unwrap().count(),
        0
    );
    let (_, d3) = ProjectStore::open(store2.root()).unwrap();
    assert_eq!(d3.state_json(), expected);
}

#[test]
fn local_index_round_trip() {
    let idx = crate::local_index::LocalIndex::in_memory().unwrap();
    let lm = crate::local_index::LocalMedia {
        id: MediaId::of_bytes(b"q"),
        path: "/tmp/q.mp4".into(),
        size: 3,
        mtime: 7,
    };
    idx.set_path(&lm).unwrap();
    assert_eq!(idx.get(lm.id).unwrap(), Some(lm.clone()));
    assert_eq!(idx.all().unwrap().len(), 1);
    idx.kv_set("a", "b").unwrap();
    assert_eq!(idx.kv_get("a").unwrap().as_deref(), Some("b"));
}

#[test]
fn concurrent_nested_container_creation_merges() {
    // Two peers concurrently create the same sequence id: mergeable child
    // containers must merge rather than one replacing the other.
    let mut a = Document::with_peer(1);
    let mut b = Document::with_peer(2);
    let seq = SeqId::new();
    let (ta, tb) = (TrackId::new(), TrackId::new());
    a.execute(&CreateSequence {
        id: seq,
        name: "S".into(),
    })
    .unwrap();
    b.execute(&CreateSequence {
        id: seq,
        name: "S".into(),
    })
    .unwrap();
    a.execute(&AddTrack {
        seq,
        id: ta,
        kind: TrackKind::Video,
        name: None,
        index: None,
    })
    .unwrap();
    b.execute(&AddTrack {
        seq,
        id: tb,
        kind: TrackKind::Audio,
        name: None,
        index: None,
    })
    .unwrap();
    let ua = a.export_updates_since(&Default::default());
    let ub = b.export_updates_since(&Default::default());
    a.import(&ub).unwrap();
    b.import(&ua).unwrap();
    assert_eq!(a.state_json(), b.state_json());
    let s = a.snapshot();
    assert_eq!(s.sequences[&seq].tracks.len(), 2);
}
