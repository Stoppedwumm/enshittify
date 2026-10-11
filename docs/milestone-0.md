# Milestone 0 — Skeleton

**Status: complete** (see "Acceptance status" at the end).

## Scope

1. egui dock layout as specified (top bar, Media / Player / Properties /
   Timeline, nine layout modes), dark theme with accent.
2. Shortcut system: `Action` enum, `shortcuts.toml`, scopes, presets,
   rebinding with conflict detection, text fields swallow keys.
3. Create / open / save projects (`.cutroom` folder, Loro snapshot + oplog).
4. Import, probe and thumbnail media (content-addressed by BLAKE3).
5. Single-track playback with A/V sync (audio clock is master).

Beyond the strict M0 list, the skeleton already contains the pieces that the
rest of the plan depends on, so they can be tested early: multi-track data
model and commands, the WebSocket relay with journal + checkpoints, the sync
client and presence, and a minimal MP4 exporter (needed for acceptance test 1).

## Design

### Crates

| crate | role |
|---|---|
| `core` | `Ticks` (flicks), `Timebase`, drop-frame `Timecode`; ids; Loro document wrapper (`Document`); `Command`s; per-user undo; `normalize()`; `.cutroom` store; local SQLite index |
| `media` | BLAKE3 hashing; `MediaBackend` trait with **CLI** (`ffprobe`/`ffmpeg` executables) and **native** (`ffmpeg-next`, feature `ffmpeg`) backends; `FrameServer` (LRU cache + per-source decode workers with look-ahead and GOP-aware seek); thumbnails/filmstrips |
| `render` | `build_plan(snapshot, seq, tick) → RenderPlan` (pure); wgpu `Compositor` (WGSL, blend modes) and YUV→RGB converter; CPU reference rasterizer |
| `audio` | `AudioClock` (sample-counted), `TimelineAudio` mixer (gain, pan, fades), cpal output with null fallback, `AudioEngine` (feeder thread + lock-free ring) |
| `export` | presets (MP4 H.264/AAC 720p/1080p/4K × 24–60 fps), pipeline (same plan + mixer as preview → `ffmpeg`), job queue with progress/ETA/cancel |
| `sync` | postcard protocol (`Hello`/`Welcome`/`Update`/`Ack`/`Presence`/`Heartbeat`), presence throttle (≤ 20 Hz) and expiry (15 s), WebSocket client |
| `relay` | `cutroom-relay`: axum WebSocket rooms, in-memory LoroDoc per room, SQLite journal, checkpoints every 1000 updates / 60 s |
| `ui` | eframe app: session (doc + store + autosave + import), playback, preview, panels, dialogs, shortcuts, i18n (en/de), themes, layouts |
| `app` | `cutroom` binary |
| `tests` | acceptance tests: frame accuracy (golden), 3-peer convergence |

### Time
All edit positions are `i64` flicks (1/705 600 000 s). Every NTSC and integer
frame rate and every common sample rate is an integer number of flicks, so
splits, trims and sample positions are exact. `Timecode` supports drop-frame
for 29.97 and 59.94 (property-tested round trip over 24 h).

### Document
See [schema.md](schema.md). Every edit is a `Command` executed inside one Loro
transaction (one undo step, one network batch). Commands validate against the
pre-command snapshot before writing. Nested containers use Loro's
*mergeable* containers so concurrent creation merges instead of clobbering.
Moving a clip is an LWW write of `(track_id, start_tick)` plus an
application-level Lamport `pos_clock`; `normalize()` resolves overlaps
deterministically (higher clock wins, peer id breaks ties) and reports
conflicts, which the timeline draws as red stripes.

### Persistence
`.cutroom/doc.loro` (snapshot on Save) + `oplog/NNNNNNNNNN.update`
(incremental exports every ≤ 500 ms, written atomically on a background
thread) + `cache/` + `local/index.sqlite` (MediaId → path, never synced).
Open = import snapshot + every oplog file (idempotent).

### Playback
`FrameServer::request` never blocks: it returns the exact frame when cached,
else the last decoded frame of that source, and queues the request. Workers
decode forward when the target is ≤ 48 frames ahead, else seek (keyframe +
decode forward), and pre-decode 12 frames ahead. At 1× the playhead is the
audio clock (`origin + consumed_samples / rate`, integer math); shuttle speeds
use the wall clock. Edits during playback restart the audio pipeline.

### Rendering
Preview: wgpu compositor rendering into a texture registered with
egui-wgpu. If eframe cannot get a wgpu adapter it falls back to OpenGL
(glow) and the preview uses the CPU rasterizer. Export uses the same
`build_plan` and the GPU (or CPU) rasterizer.

## Risks

| risk | mitigation |
|---|---|
| FFmpeg dev libraries are painful on Windows | default build uses the CLI backend; native backend is an opt-in feature |
| CLI seeking is only frame-exact for CFR | VFR handling moves to the native backend (pts-based) in MVP; tests cover CFR at 23.976/25/29.97 |
| Concurrent container creation in Loro loses data | mergeable containers everywhere; dedicated test |
| Undo after remote edits | Loro `UndoManager` (local peer only); tested with interleaved peers |
| No GPU in CI | naga validates all WGSL; CPU rasterizer is the oracle; GPU tests skip without an adapter |
| cpal device quirks | null output fallback; clock advances only on consumed samples |
| egui texture limits | filmstrips sized ≤ 1920 px; oversized textures downscaled |

## Test plan

* Unit tests per crate (`cargo test`): time/timecode (incl. proptests),
  commands, undo isolation, convergence of concurrent edits, persistence and
  crash recovery, probe parsing, cache eviction, YUV matrices, CPU blending,
  plan geometry, clock drift, mixer, presets, protocol, presence throttle,
  shortcuts, i18n, layouts, settings, session.
* Media integration tests (need `ffmpeg` on PATH): frame-exact seeks at three
  rates on long-GOP H.264 with B-frames for **both** backends, audio decode,
  frame server, thumbnails.
* `tests/frame_accuracy.rs` — acceptance 1.
* `tests/convergence.rs` — acceptance 4 (600 ops by default,
  `CUTROOM_CONVERGENCE_OPS=10000` for the full run).
* `crates/relay/tests/relay.rs` — two live clients through a real relay,
  offline merge, journal replay after restart.
* `crates/ui/tests/ui_smoke.rs` — drives the whole app frame loop headlessly.
* `shortcuts::tests::ui_every_binding_dispatches` — acceptance 10.

## Acceptance status after M0

| # | criterion | status |
|---|---|---|
| 1 | frame accuracy (split @1000, export, reads 1000 at 29.97/25/23.976) | ✅ automated |
| 2 | A/V sync < 1 sample / 10 min | ✅ clock is integer sample count (test); device-latency compensation pending |
| 3 | realtime playback 4+4 tracks, UI ≥ 55 fps | ⏳ needs proxies (MVP); dropped-frame and UI-time metrics are in the status bar |
| 4 | 3-peer convergence, 10k commands, frame hashes | ✅ (`CUTROOM_CONVERGENCE_OPS=10000`, ~2 min release) |
| 5 | latency | ◐ relay round trip asserted < 500 ms on localhost; RTT-shaped test in MVP |
| 6 | presence ≥ 10 Hz without growing the doc | ✅ throttle test; presence never touches the doc |
| 7 | media sync / relink | ◐ relink rejects mismatched hashes; blob transfer in MVP |
| 8 | undo isolation | ✅ |
| 9 | crash safety < 2 s | ✅ 500 ms oplog flush; crash-recovery tests |
| 10 | shortcuts bound + UI test + conflicts | ✅ |
| 11 | export presets validated | ◐ H.264/AAC MP4 validated by probe in the frame-accuracy test; full preset matrix in MVP |
| 12 | FCP7 XML round trip | ⏳ v1 |

## Next (MVP)

Ripple engine (ripple delete, close gap, insert edit, magnetic timeline),
transitions, titles, waveforms (`.peaks`), proxies, media blob sync
(iroh-blobs / S3), relay auth (JWT + roles), soft locks UI polish, latency
tests at 100 ms RTT, full export preset validation.
