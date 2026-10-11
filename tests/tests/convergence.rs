//! Acceptance 4 (convergence): 3 peers apply random commands with offline
//! periods, reordered and duplicated delivery. Final snapshots must be
//! identical and rendered frames at 20 ticks must hash equally.
//!
//! The default run uses 600 commands per case so `cargo test` stays fast;
//! set `CUTROOM_CONVERGENCE_OPS=10000` for the full acceptance run.

use cutroom_core::command::*;
use cutroom_core::model::{ClipSource, Generator, MarkerColor, Meta, TrackKind};
use cutroom_core::{ClipId, Document, MarkerId, Ticks, TrackId};
use cutroom_media::VideoFrame;
use cutroom_render::{FrameProvider, build_plan, frame_hash};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};

struct NoFrames;
impl FrameProvider for NoFrames {
    fn frame(&self, _: cutroom_core::MediaId, _: i64) -> Option<VideoFrame> {
        None
    }
}

#[derive(Clone, Debug)]
enum Op {
    /// Peer edits.
    Edit {
        peer: usize,
        kind: u8,
        a: u64,
        b: u64,
        c: u64,
    },
    /// Deliver everything `from` has to `to` (possibly twice).
    Sync { from: usize, to: usize, dup: bool },
    /// Toggle a peer offline/online.
    Toggle { peer: usize },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        6 => (0..3usize, 0..8u8, any::<u64>(), any::<u64>(), any::<u64>())
            .prop_map(|(peer, kind, a, b, c)| Op::Edit { peer, kind, a, b, c }),
        3 => (0..3usize, 0..3usize, any::<bool>()).prop_map(|(from, to, dup)| Op::Sync { from, to, dup }),
        1 => (0..3usize).prop_map(|peer| Op::Toggle { peer }),
    ]
}

fn apply_edit(d: &mut Document, kind: u8, a: u64, b: u64, c: u64) {
    let snap = d.snapshot();
    let Some(seq) = snap.main() else { return };
    let tracks: Vec<TrackId> = seq
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Video)
        .map(|t| t.id)
        .collect();
    let clips: Vec<ClipId> = seq.clips.keys().copied().collect();
    let pick_clip = |x: u64| clips.get((x as usize) % clips.len().max(1)).copied();
    let sec = |x: u64| Ticks::from_secs((x % 60) as i64);
    let tb = snap.meta.timebase;
    let colour = |x: u64| {
        let v = x.to_le_bytes();
        [
            v[0] as f32 / 255.0,
            v[1] as f32 / 255.0,
            v[2] as f32 / 255.0,
            1.0,
        ]
    };
    let r = match kind {
        0 | 1 => d.execute(&AddClip {
            seq: seq.id,
            clip: NewClip {
                id: ClipId::new(),
                name: "c".into(),
                track: tracks[(a as usize) % tracks.len()],
                start: tb.snap(sec(b)),
                in_tick: Ticks::ZERO,
                out_tick: Ticks::from_secs(1 + (c % 5) as i64),
                source: ClipSource::Generator(Generator::Color(colour(c))),
            },
        }),
        2 => match pick_clip(a) {
            Some(cl) => d.execute(&MoveClips {
                seq: seq.id,
                moves: vec![(cl, tracks[(b as usize) % tracks.len()], tb.snap(sec(c)))],
            }),
            None => Ok(()),
        },
        3 => match pick_clip(a) {
            Some(cl) => {
                let clip = &seq.clips[&cl];
                let at = clip.start + Ticks(((clip.duration().0 as u64) / 2).max(1) as i64);
                d.execute(&SplitClip {
                    seq: seq.id,
                    clip: cl,
                    at: tb.snap(at),
                    new_id: ClipId::new(),
                })
            }
            None => Ok(()),
        },
        4 => match pick_clip(a) {
            Some(cl) => d.execute(&DeleteClips {
                seq: seq.id,
                clips: vec![cl],
            }),
            None => Ok(()),
        },
        5 => match pick_clip(a) {
            Some(cl) => d.execute(&SetTransform {
                seq: seq.id,
                clip: cl,
                fields: vec![
                    TransformField::Opacity((b % 100) as f64 / 100.0),
                    TransformField::ScaleX(0.25 + (c % 8) as f64 / 8.0),
                ],
            }),
            None => Ok(()),
        },
        6 => d.execute(&AddMarker {
            seq: seq.id,
            id: MarkerId::new(),
            tick: sec(a),
            color: MarkerColor::from_index((b % 8) as i64),
            name: "m".into(),
            author: "p".into(),
        }),
        _ => {
            // Undo is per user and must also converge.
            d.undo();
            Ok(())
        }
    };
    // Rejections (e.g. split point no longer inside the clip) are fine.
    let _ = r;
}

fn run_case(ops: Vec<Op>) -> Result<(), TestCaseError> {
    let mut peers: Vec<Document> = (1..=3).map(Document::with_peer).collect();
    let init = InitProject::new(Meta {
        width: 64,
        height: 36,
        ..Meta::default()
    });
    peers[0].execute(&init).unwrap();
    peers[0]
        .execute(&AddTrack {
            seq: init.seq,
            id: TrackId::new(),
            kind: TrackKind::Video,
            name: None,
            index: None,
        })
        .unwrap();
    let base = peers[0].export_snapshot();
    for p in &mut peers[1..] {
        p.import(&base).unwrap();
    }
    let mut online = [true; 3];
    for op in ops {
        match op {
            Op::Edit {
                peer,
                kind,
                a,
                b,
                c,
            } => apply_edit(&mut peers[peer], kind, a, b, c),
            Op::Sync { from, to, dup } => {
                if from == to || !online[from] || !online[to] {
                    continue;
                }
                let bytes = peers[from].export_updates_since(&peers[to].version());
                peers[to].import(&bytes).unwrap();
                if dup {
                    peers[to].import(&bytes).unwrap();
                }
            }
            Op::Toggle { peer } => online[peer] = !online[peer],
        }
    }
    // Everyone reconnects; deliver in a scrambled order.
    for (from, to) in [(2, 0), (0, 1), (1, 2), (2, 1), (1, 0), (0, 2)] {
        let bytes = peers[from].export_updates_since(&peers[to].version());
        peers[to].import(&bytes).unwrap();
    }
    let s0 = peers[0].state_json();
    for p in &peers[1..] {
        prop_assert_eq!(&p.state_json(), &s0);
    }
    let snaps: Vec<_> = peers.iter_mut().map(|p| p.snapshot()).collect();
    let seq = init.seq;
    for i in 0..20 {
        let t = Ticks::from_secs(i * 3);
        let hashes: Vec<_> = snaps
            .iter()
            .map(|s| {
                frame_hash(&cutroom_render::cpu::render(&build_plan(
                    s, seq, t, &NoFrames,
                )))
            })
            .collect();
        prop_assert!(
            hashes.iter().all(|h| *h == hashes[0]),
            "frame hash mismatch at {}",
            i
        );
    }
    Ok(())
}

#[test]
fn three_peers_converge() {
    let n: usize = std::env::var("CUTROOM_CONVERGENCE_OPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(600);
    let cases = if n > 2000 { 1 } else { 6 };
    let mut runner = TestRunner::new(Config {
        cases,
        ..Config::default()
    });
    runner
        .run(&proptest::collection::vec(op(), n..=n), run_case)
        .unwrap();
}
