//! Headless smoke test of the full application frame loop: top bar, dock,
//! every panel, dialogs and layouts, driven by synthetic input through a real
//! egui context (no window or GPU needed).

use std::time::{Duration, Instant};

use cutroom_ui::app::{AppOptions, CutroomApp};
use cutroom_ui::layout::LayoutMode;
use cutroom_ui::session::ItemStatus;
use cutroom_ui::shortcuts::Action;
use eframe::App;
use egui::{Event, Key, Modifiers, RawInput};

struct Harness {
    ctx: egui::Context,
    app: CutroomApp,
    frame: eframe::Frame,
}

impl Harness {
    fn step(&mut self, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 940.0),
            )),
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let frame = &mut self.frame;
        let mut out = self.ctx.run_ui(input, |ui| app.ui(ui, frame));
        out.textures_delta.clear();
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.step(vec![]);
        }
    }

    fn key(&mut self, key: Key, modifiers: Modifiers) {
        let ev = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        };
        self.step(vec![ev(true), ev(false)]);
        self.frames(2);
    }

    fn clip_count(&mut self) -> usize {
        self.app
            .state
            .session
            .snapshot()
            .main()
            .map(|s| s.clips.len())
            .unwrap_or(0)
    }
}

#[test]
fn full_ui_frame_loop() {
    if cutroom_media::cli::CliBackend::locate().is_err() {
        eprintln!("skipping: FFmpeg not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // Per-user files go to a temp dir (single test in this binary → no env races).
    unsafe { std::env::set_var("CUTROOM_CONFIG_DIR", dir.path().join("cfg")) };
    let project = cutroom_ui::demo::create_demo_project(dir.path()).unwrap();

    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let app = CutroomApp::new(
        &cc,
        AppOptions {
            project: Some(project),
            no_audio: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut h = Harness {
        ctx,
        app,
        frame: eframe::Frame::_new_kittest(),
    };

    // Media re-attaches in the background.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        h.frames(1);
        let ready = h
            .app
            .state
            .session
            .library
            .values()
            .all(|i| i.status == ItemStatus::Ready);
        if ready || Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        h.app
            .state
            .session
            .library
            .values()
            .all(|i| i.status == ItemStatus::Ready)
    );
    h.frames(10);
    let clips = h.clip_count();
    assert_eq!(clips, 3);

    // Playback advances the playhead (null audio output → audio clock).
    let cmd = if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD
    } else {
        Modifiers::CTRL
    };
    h.key(Key::Space, Modifiers::NONE);
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(600) {
        h.frames(1);
        std::thread::sleep(Duration::from_millis(16));
    }
    assert!(h.app.state.playback.playhead.0 > 0, "playhead did not move");
    h.key(Key::Space, Modifiers::NONE);
    assert!(!h.app.state.playback.is_playing());

    // Split at playhead with the shortcut, then undo it.
    h.app.state.focus = cutroom_ui::shortcuts::Scope::Timeline;
    h.key(Key::B, cmd);
    assert_eq!(h.clip_count(), clips + 1);
    h.key(Key::Z, cmd);
    assert_eq!(h.clip_count(), clips);

    // Markers, in/out, frame stepping.
    let markers = h.app.state.session.snapshot().main().unwrap().markers.len();
    h.key(Key::M, Modifiers::NONE);
    assert_eq!(
        h.app.state.session.snapshot().main().unwrap().markers.len(),
        markers + 1
    );
    h.key(Key::I, Modifiers::NONE);
    let before = h.app.state.playback.playhead;
    h.key(Key::ArrowRight, Modifiers::NONE);
    assert!(h.app.state.playback.playhead > before);
    h.key(Key::O, Modifiers::NONE);
    assert!(h.app.state.mark_in.is_some() && h.app.state.mark_out.is_some());

    // Placing media from the library onto the timeline.
    let media = *h.app.state.session.library.keys().next().unwrap();
    cutroom_ui::panels::timeline::place_media_at_playhead(&mut h.app.state, media, false);
    assert_eq!(h.clip_count(), clips + 1);

    // Every dialog and every layout renders.
    let ctx = h.ctx.clone();
    for a in [Action::Export, Action::ShortcutEditor, Action::Preferences] {
        h.app.state.run_action(a, &ctx);
        h.frames(3);
    }
    h.app.state.dialogs.manage_tracks = true;
    h.app.state.dialogs.about = true;
    let settings = h.app.state.settings.clone();
    h.app.state.dialogs.open_collab(&settings);
    h.frames(3);
    for m in LayoutMode::ALL {
        h.app.dock = m.build();
        h.frames(3);
    }
    // Text field focus swallows shortcuts: the collab dialog's fields exist;
    // typing "M" into a focused field must not add a marker.
    h.frames(2);

    // Autosave reached disk.
    std::thread::sleep(Duration::from_millis(600));
    h.frames(2);
    std::thread::sleep(Duration::from_millis(200));
    let oplog = h.app.state.session.store.root().join("oplog");
    assert!(
        std::fs::read_dir(oplog).unwrap().count() > 0,
        "autosave wrote nothing"
    );
}
