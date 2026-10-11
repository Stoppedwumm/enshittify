# Cutroom

A desktop non-linear video editor for Windows, macOS and Linux, with a
familiar consumer-NLE workflow and first-class real-time **multiplayer** editing.

* Frame-accurate: every position is an integer number of *flicks*; drop-frame timecode.
* The project **is** a CRDT (Loro). Every edit is a command, one transaction,
  one undo step (per user), one network batch.
* Rendering is a pure function of `(document snapshot, tick)`; preview and
  export share the compositor (wgpu/WGSL, with a CPU reference path).
* Media is content-addressed (BLAKE3); collaborators never relink by path.

## Status

**Milestone 0 (skeleton) is complete** — see [docs/milestone-0.md](docs/milestone-0.md)
and the [decision log](docs/decisions.md).

## Building

Requirements: Rust ≥ 1.88 and FFmpeg 6–8 **executables** (`ffmpeg`, `ffprobe`)
on `PATH` (or in `CUTROOM_FFMPEG_DIR`). No FFmpeg development libraries are
needed for the default build.

| OS | extra build deps | runtime |
|---|---|---|
| Windows | — | FFmpeg (e.g. `winget install ffmpeg`) |
| macOS | — | `brew install ffmpeg` |
| Linux | `libasound2-dev` (ALSA headers for audio) | FFmpeg; `libxkbcommon-x11` on X11 |

```sh
just build          # or: cargo build --workspace
just test           # all tests; media tests skip without ffmpeg
just run            # launch the editor
just demo           # generate a demo project and open it
just run-relay      # start the collaboration relay on 127.0.0.1:7878
just demo-two-clients
```

`just build-native` links FFmpeg natively via `ffmpeg-next` (needs FFmpeg dev
libraries + pkg-config, or `FFMPEG_DIR` on Windows).

## Workspace

```
crates/core     time, ids, Loro schema, commands, undo, persistence
crates/media    probe, hashing, decode backends, frame cache, thumbnails
crates/render   render plans, wgpu compositor, CPU rasterizer
crates/audio    master clock, mixer, device output
crates/export   presets, encode pipeline, job queue
crates/sync     wire protocol, presence, relay client
crates/relay    cutroom-relay server
crates/ui       egui application
crates/app      cutroom binary
tests/          acceptance tests (golden frames, convergence)
```

## Collaboration quick start

```sh
cutroom-relay --addr 0.0.0.0:7878            # on a trusted LAN (no auth in M0)
cutroom MyFilm.cutroom --relay ws://HOST:7878/ws --room myfilm --name Ana
cutroom --relay ws://HOST:7878/ws --room myfilm --name Ben   # joins
```

Or use **Share** in the top bar.
