# Decision log

Decisions taken without blocking on the product owner. Newest last.

| # | date | decision | why |
|---|---|---|---|
| D1 | M0 | **Two FFmpeg backends.** Default: drive the `ffmpeg`/`ffprobe` executables (CLI backend). Opt-in feature `ffmpeg` links FFmpeg natively via `ffmpeg-next =9.0.0` (verified against FFmpeg 6.1; supports 6–8). Runtime picks native if compiled in, else CLI. | Must build and run on Windows, macOS and Linux with a plain `cargo build`; FFmpeg dev libs are hard to provide on Windows. Both backends pass the same frame-accuracy tests. |
| D2 | M0 | `rsmpeg` backend deferred. | Requires FFmpeg 8 headers at build time; adds nothing until the native backend needs features `ffmpeg-next` lacks. |
| D3 | M0 | Top-level schema entries are Loro root containers instead of one `root` LoroMap; nested containers use `ensure_mergeable_*`. | Root/mergeable containers have deterministic ids, so concurrent creation merges. A plain `insert_container` race silently drops one side. |
| D4 | M0 | Clip speed stored as an exact rational (`num/den`). | Keeps timeline durations exact in ticks. |
| D5 | M0 | Overlap winner uses an application-level Lamport clock (`pos_clock`, `pos_peer`) written with every position change. | Loro does not expose per-key Lamport timestamps; this is deterministic on every replica. |
| D6 | M0 | Commands validate against the pre-command snapshot before writing; if one still fails mid-write it is committed and reverted (`revert_to`). | Loro has no transaction rollback. |
| D7 | M0 | Autosave = incremental `export(updates since persisted)` every ≤ 500 ms, written atomically by a background thread; Save = snapshot + oplog compaction. | Acceptance 9 (< 2 s loss) without blocking the UI. |
| D8 | M0 | Per-machine data (`MediaId → path`, kv) lives in `.cutroom/local/index.sqlite` (rusqlite, bundled SQLite). Per-user settings/layouts/shortcut overrides live in the OS config dir. | Never in the shared doc; bundled SQLite avoids a system dependency. |
| D9 | M0 | Decoders output RGBA8 (swscale with tagged BT.601/709/2020 matrix and range). The WGSL YUV→RGB converter exists and is tested but is not yet in the decode path. | Keeps M0 simple; moving conversion to the GPU is a performance step for MVP. |
| D10 | M0 | Compositing in 8-bit gamma space (`Rgba8Unorm`), premultiplied alpha, fixed-function blends (normal/add/multiply/screen). | Matches typical consumer NLE look; linear-light / 16-bit float pipeline later with color management. |
| D11 | M0 | CPU reference rasterizer implements the same plan; it is the test oracle and the fallback when no GPU adapter exists. The app falls back to eframe's glow (OpenGL) renderer if wgpu is unavailable. | CI and many VMs have no GPU; preview must still work. |
| D12 | M0 | All WGSL is validated with naga in unit tests. | Shader errors are caught without a GPU. |
| D13 | M0 | Audio engine renders at the device's preferred rate; FFmpeg resamples during decode. `rubato` will be added when the in-engine mixer needs it (variable speed). A null output drives the clock when no device exists. | Fewer moving parts for M0; identical behaviour headless. |
| D14 | M0 | Audio clock = samples actually consumed by the device; underruns play silence and do not advance the clock. Output-latency compensation is pending. | Video can never run ahead of what was heard; zero drift by construction. |
| D15 | M0 | Export pipes raw RGBA frames into the `ffmpeg` executable; audio is pre-rendered to a temp `f32le` file. | Portable (no named pipes); native encoding comes with the native backend. |
| D16 | M0 | Relay has no authentication yet (rooms validated `[A-Za-z0-9_-]{1,64}`; peer ids in presence are server-assigned). JWT + roles land in MVP; TLS via reverse proxy or rustls then. | Scope; bind to localhost / trusted LAN for now. |
| D17 | M0 | Dock layouts are persisted with postcard, not JSON. | egui_dock rects contain infinities, which JSON cannot encode. |
| D18 | M0 | Shortcut table interpretation: Linux uses the Windows column; macOS "Delete" = Backspace key (forward-delete also bound); Rename has no macOS binding (table shows "—"); Alt+Click is a mouse chord; text fields swallow keys only when a `TextEdit` has focus. | Faithful to the table; platform conventions. |
| D19 | M0 | Shortcut presets: Premiere and Final Cut overrides remap a handful of signature keys and resolve resulting conflicts (e.g. FCP Detach = Ctrl+Shift+S moves Save As to Ctrl+Alt+Shift+S). Tests assert zero conflicts for every preset on both platforms. | Rebinding must never leave silent conflicts. |
| D20 | M0 | Filmstrips use 10 frames of 192×108 (1920 px). | egui panics above the GPU's max texture side (2048 on some devices) — found by the UI smoke test. |
| D21 | M0 | Still images get a 5 s default duration. Insert/Overwrite edit currently place without rippling (ripple engine is MVP). | Filmora-like defaults; avoids half-implemented ripple. |
| D22 | M0 | Joining a room (`--relay/--room` or "Join as new project") starts from an empty document. | A second `InitProject` would race the owner and create a competing main sequence. |
| D23 | M0 | Timecode is shown drop-frame for 29.97/59.94, non-drop otherwise. | Broadcast convention. |
| D24 | M0 | Edition 2024, MSRV 1.88 (let-chains). Exact `=x.y.z` pins in `[workspace.dependencies]`; `Cargo.lock` committed. | Reproducible builds. |
| D25 | M0 | No system packages are installed by the build; on Linux the GUI needs `libxkbcommon-x11` (X11) at runtime and `libasound2-dev` to build cpal. | User requirement: cross-platform without touching the system; documented in README. |
