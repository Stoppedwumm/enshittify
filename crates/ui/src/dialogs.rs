//! Modal-ish windows: shortcut editor, export, collaborate, manage tracks,
//! preferences, about, marker edit, rename.

use std::path::PathBuf;

use cutroom_core::command::{AddTrack, EditMarker, RenameMedia, SetTrackProps, TrackPatch};
use cutroom_core::model::TrackKind;
use cutroom_core::{MarkerId, MediaId, Ticks, TrackId};
use cutroom_export::{ExportJob, JobState, Preset, SourceFile, mvp_presets};
use egui::{Color32, RichText};

use crate::app::{AppState, fmt_duration};
use crate::session::{ItemStatus, Session};
use crate::settings::{Settings, projects_dir};
use crate::shortcuts::{Action, Chord, Platform, Preset as KeyPreset, ShortcutMap, Trigger};
use crate::theme;

#[derive(Default)]
pub struct ShortcutEditor {
    search: String,
    recording: Option<Action>,
    conflict: Option<(Action, Chord, Vec<Action>)>,
}

pub struct ExportDialog {
    preset: usize,
    in_out: bool,
    output: String,
}

pub struct CollabDialog {
    url: String,
    room: String,
    name: String,
}

#[derive(Default)]
pub struct Dialogs {
    pub preferences: bool,
    pub about: bool,
    pub manage_tracks: bool,
    pub shortcuts: Option<ShortcutEditor>,
    pub export: Option<ExportDialog>,
    pub collab: Option<CollabDialog>,
    pub edit_marker: Option<(MarkerId, String, String)>,
    pub rename_media: Option<(MediaId, String)>,
    track_names: Vec<(TrackId, String)>,
}

impl Dialogs {
    pub fn open_shortcuts(&mut self) {
        self.shortcuts.get_or_insert_with(Default::default);
    }

    pub fn open_export(&mut self) {
        self.export.get_or_insert(ExportDialog {
            preset: usize::MAX,
            in_out: false,
            output: String::new(),
        });
    }

    pub fn open_collab(&mut self, settings: &Settings) {
        self.collab.get_or_insert_with(|| CollabDialog {
            url: settings.relay_url.clone(),
            room: String::new(),
            name: settings.user_name.clone(),
        });
    }

    /// While a shortcut is being recorded, normal dispatch is suspended.
    pub fn is_recording(&self) -> bool {
        self.shortcuts
            .as_ref()
            .is_some_and(|e| e.recording.is_some() || e.conflict.is_some())
    }
}

pub fn show(ctx: &egui::Context, s: &mut AppState) {
    shortcut_editor(ctx, s);
    export(ctx, s);
    collab(ctx, s);
    manage_tracks(ctx, s);
    preferences(ctx, s);
    about(ctx, s);
    marker(ctx, s);
    rename(ctx, s);
}

fn shortcut_editor(ctx: &egui::Context, s: &mut AppState) {
    let Some(mut ed) = s.dialogs.shortcuts.take() else {
        return;
    };
    let mut open = true;
    let platform = s.shortcuts.platform();
    // Capture a key for the action being recorded.
    if let Some(a) = ed.recording
        && ed.conflict.is_none()
    {
        let captured = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => Some(Chord::from_event(*key, *modifiers)),
                _ => None,
            })
        });
        if let Some(chord) = captured {
            if chord.trigger == Trigger::Key(egui::Key::Escape)
                && !chord.ctrl
                && !chord.alt
                && !chord.shift
                && !chord.cmd
            {
                ed.recording = None;
            } else {
                let others = s.shortcuts.conflicts_with(a, &chord);
                if others.is_empty() {
                    s.shortcuts.set(a, vec![chord]);
                    s.save_shortcuts();
                    ed.recording = None;
                } else {
                    ed.conflict = Some((a, chord, others));
                }
            }
        }
    }
    egui::Window::new(s.t("shortcuts-title"))
        .open(&mut open)
        .default_size([560.0, 520.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(s.t("shortcuts-preset"));
                let mut p = s.settings.shortcut_preset;
                egui::ComboBox::from_id_salt("kb-preset")
                    .selected_text(p.label())
                    .show_ui(ui, |ui| {
                        for x in KeyPreset::ALL {
                            ui.selectable_value(&mut p, x, x.label());
                        }
                    });
                if p != s.settings.shortcut_preset {
                    s.settings.shortcut_preset = p;
                    s.shortcuts = ShortcutMap::with_preset(Platform::current(), p);
                    s.save_shortcuts();
                    let _ = s.settings.save(&s.config_dir);
                }
                if ui.button(s.t("shortcuts-restore")).clicked() {
                    s.shortcuts =
                        ShortcutMap::with_preset(Platform::current(), s.settings.shortcut_preset);
                    s.save_shortcuts();
                }
                ui.add(
                    egui::TextEdit::singleline(&mut ed.search)
                        .hint_text(s.t("shortcuts-search"))
                        .desired_width(160.0),
                );
            });
            if let Some((a, chord, others)) = ed.conflict.clone() {
                egui::Frame::new()
                    .fill(theme::CONFLICT.gamma_multiply(0.3))
                    .inner_margin(6.0)
                    .corner_radius(4.0)
                    .show(ui, |ui| {
                        let names: Vec<&str> = others.iter().map(|o| o.label()).collect();
                        ui.label(s.i18n.ta(
                            "shortcuts-conflict",
                            &[
                                ("chord", &chord.display(platform)),
                                ("action", &names.join(", ")),
                            ],
                        ));
                        ui.horizontal(|ui| {
                            if ui.button(s.t("shortcuts-replace")).clicked() {
                                s.shortcuts.rebind_replacing(a, chord);
                                s.save_shortcuts();
                                ed.conflict = None;
                                ed.recording = None;
                            }
                            if ui.button(s.t("shortcuts-cancel")).clicked() {
                                ed.conflict = None;
                                ed.recording = None;
                            }
                        });
                    });
            }
            ui.separator();
            let q = ed.search.to_lowercase();
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("kb-grid")
                    .striped(true)
                    .num_columns(4)
                    .show(ui, |ui| {
                        for a in Action::ALL {
                            let chords = s.shortcuts.chords(*a).to_vec();
                            let text: Vec<String> =
                                chords.iter().map(|c| c.display(platform)).collect();
                            if !q.is_empty()
                                && !a.label().to_lowercase().contains(&q)
                                && !text.join(" ").to_lowercase().contains(&q)
                            {
                                continue;
                            }
                            ui.label(a.label());
                            ui.label(RichText::new(s.shortcuts.scope(*a).label()).small().weak());
                            let rec = ed.recording == Some(*a);
                            let label = if rec {
                                s.t("shortcuts-press")
                            } else if text.is_empty() {
                                "—".into()
                            } else {
                                text.join(", ")
                            };
                            if ui
                                .selectable_label(rec, RichText::new(label).monospace())
                                .clicked()
                            {
                                ed.recording = Some(*a);
                                ed.conflict = None;
                            }
                            if ui.small_button("✖").on_hover_text("Unbind").clicked() {
                                s.shortcuts.set(*a, vec![]);
                                s.save_shortcuts();
                            }
                            ui.end_row();
                        }
                    });
            });
        });
    if open {
        s.dialogs.shortcuts = Some(ed);
    }
}

fn default_preset_index(presets: &[Preset], s: &mut AppState) -> usize {
    let snap = s.session.snapshot();
    presets
        .iter()
        .position(|p| p.height == snap.meta.height && p.fps == snap.meta.timebase)
        .or_else(|| {
            presets
                .iter()
                .position(|p| p.height == 1080 && p.fps.num == 30)
        })
        .unwrap_or(0)
}

fn export(ctx: &egui::Context, s: &mut AppState) {
    let Some(mut d) = s.dialogs.export.take() else {
        return;
    };
    let presets = mvp_presets();
    if d.preset >= presets.len() {
        d.preset = default_preset_index(&presets, s);
    }
    if d.output.is_empty() {
        let name = s.session.name();
        d.output = projects_dir()
            .join(format!("{name}.mp4"))
            .display()
            .to_string();
    }
    let mut open = true;
    egui::Window::new(s.t("export-title"))
        .open(&mut open)
        .resizable(false)
        .show(ctx, |ui| {
            let running = s
                .export_job
                .and_then(|id| s.export_queue.as_ref().and_then(|q| q.state(id)));
            egui::Grid::new("exp")
                .num_columns(2)
                .spacing([10.0, 8.0])
                .show(ui, |ui| {
                    ui.label(s.t("export-preset"));
                    egui::ComboBox::from_id_salt("exp-preset")
                        .width(300.0)
                        .selected_text(&presets[d.preset].label)
                        .show_ui(ui, |ui| {
                            for (i, p) in presets.iter().enumerate() {
                                ui.selectable_value(&mut d.preset, i, &p.label);
                            }
                        });
                    ui.end_row();
                    ui.label(s.t("export-range"));
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut d.in_out, false, s.t("export-range-all"));
                        let has = s.mark_in.is_some() && s.mark_out.is_some();
                        ui.add_enabled_ui(has, |ui| {
                            ui.radio_value(&mut d.in_out, true, s.t("export-range-inout"))
                        });
                    });
                    ui.end_row();
                    ui.label(s.t("export-output"));
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut d.output).desired_width(260.0));
                        if ui.button("…").clicked()
                            && let Some(p) = rfd::FileDialog::new()
                                .add_filter("MP4", &["mp4"])
                                .set_file_name("export.mp4")
                                .save_file()
                        {
                            d.output = p.display().to_string();
                        }
                    });
                    ui.end_row();
                });
            ui.separator();
            match running {
                Some(JobState::Running(p)) => {
                    let pct = (p.fraction() * 100.0).round() as u32;
                    let eta = p.eta.map(fmt_duration).unwrap_or_else(|| "…".into());
                    ui.add(egui::ProgressBar::new(p.fraction()).text(s.i18n.ta(
                        "export-eta",
                        &[("percent", &pct.to_string()), ("eta", &eta)],
                    )));
                    if ui.button(s.t("export-cancel")).clicked()
                        && let (Some(q), Some(id)) = (&s.export_queue, s.export_job)
                    {
                        q.cancel(id);
                    }
                }
                Some(JobState::Queued) => {
                    ui.spinner();
                }
                other => {
                    match other {
                        Some(JobState::Done(p)) => {
                            ui.label(
                                RichText::new(format!(
                                    "✔ {} — {}",
                                    s.t("export-done"),
                                    p.display()
                                ))
                                .color(theme::ACCENT),
                            );
                        }
                        Some(JobState::Failed(e)) => {
                            ui.label(
                                RichText::new(s.i18n.ta("export-failed", &[("error", &e)]))
                                    .color(theme::CONFLICT),
                            );
                        }
                        _ => {}
                    }
                    let can = s.export_queue.is_some();
                    if !can {
                        ui.label(
                            RichText::new("FFmpeg not found — install it to export.")
                                .color(theme::CONFLICT),
                        );
                    }
                    let start =
                        egui::Button::new(RichText::new(s.t("export-start")).color(Color32::BLACK))
                            .fill(theme::ACCENT);
                    if ui.add_enabled(can, start).clicked() {
                        start_export(s, &presets[d.preset], d.in_out, PathBuf::from(&d.output));
                    }
                }
            }
        });
    if open {
        s.dialogs.export = Some(d);
    }
}

fn start_export(s: &mut AppState, preset: &Preset, in_out: bool, output: PathBuf) {
    let snap = s.session.snapshot();
    let Some(seq) = snap.main_sequence else {
        return;
    };
    let end = snap.main().map(|q| q.duration()).unwrap_or_default();
    let (start, end) = match (in_out, s.mark_in, s.mark_out) {
        (true, Some(i), Some(o)) if o > i => (i, o),
        _ => (Ticks::ZERO, end),
    };
    if end <= start {
        s.toast("Nothing to export");
        return;
    }
    let sources: Vec<SourceFile> = s
        .session
        .library
        .values()
        .filter(|it| it.status == ItemStatus::Ready)
        .filter_map(|it| {
            Some(SourceFile {
                id: it.id,
                path: it.path.clone()?,
                info: it.info.clone()?,
            })
        })
        .collect();
    if let Some(parent) = output.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let job = ExportJob {
        snapshot: snap,
        seq,
        start,
        end,
        preset: preset.clone(),
        output,
        sources,
        cpu_only: false,
    };
    if let Some(q) = &s.export_queue {
        s.export_job = Some(q.submit(job));
    }
}

fn collab(ctx: &egui::Context, s: &mut AppState) {
    let Some(mut d) = s.dialogs.collab.take() else {
        return;
    };
    let mut open = true;
    let mut close = false;
    egui::Window::new(s.t("collab-title"))
        .open(&mut open)
        .resizable(false)
        .show(ctx, |ui| {
            egui::Grid::new("collab")
                .num_columns(2)
                .spacing([10.0, 8.0])
                .show(ui, |ui| {
                    ui.label(s.t("collab-relay"));
                    ui.text_edit_singleline(&mut d.url);
                    ui.end_row();
                    ui.label(s.t("collab-room"));
                    ui.text_edit_singleline(&mut d.room);
                    ui.end_row();
                    ui.label(s.t("collab-name"));
                    ui.text_edit_singleline(&mut d.name);
                    ui.end_row();
                });
            let valid = !d.room.is_empty()
                && d.room.len() <= 64
                && d.room
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
            if !valid && !d.room.is_empty() {
                ui.label(
                    RichText::new("Room: letters, digits, - and _ only")
                        .small()
                        .color(theme::CONFLICT),
                );
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(valid, egui::Button::new(s.t("collab-connect")))
                    .on_hover_text("Share this project in the room")
                    .clicked()
                {
                    s.settings.user_name = d.name.clone();
                    s.connect(d.url.clone(), d.room.clone(), ctx);
                    close = true;
                }
                if ui
                    .add_enabled(valid, egui::Button::new(s.t("collab-join")))
                    .clicked()
                {
                    let path = projects_dir().join(format!("{}.cutroom", d.room));
                    let session = if path.join("doc.loro").exists() {
                        Session::open(&path, &s.backend)
                    } else {
                        Session::create_empty(&path)
                    };
                    match session {
                        Ok(sess) => {
                            s.session = sess;
                            s.selection.clear();
                            s.settings.user_name = d.name.clone();
                            s.connect(d.url.clone(), d.room.clone(), ctx);
                            close = true;
                        }
                        Err(e) => s.toast(format!("Cannot join: {e}")),
                    }
                }
                if s.sync.is_some() && ui.button(s.t("collab-disconnect")).clicked() {
                    s.sync = None;
                    s.presence = Default::default();
                }
            });
            if let Some(sync) = &s.sync {
                ui.separator();
                let status = if sync.connected {
                    s.t("collab-connected")
                } else {
                    s.t("collab-offline")
                };
                ui.label(format!("{status}: {} / {}", sync.url, sync.room));
                if let Some(e) = &sync.last_error {
                    ui.label(RichText::new(e).small().weak());
                }
            }
        });
    if open && !close {
        s.dialogs.collab = Some(d);
    }
}

fn manage_tracks(ctx: &egui::Context, s: &mut AppState) {
    if !s.dialogs.manage_tracks {
        return;
    }
    let snap = s.session.snapshot();
    let Some(seq) = snap.main().cloned() else {
        return;
    };
    let mut open = true;
    let mut patches: Vec<(TrackId, TrackPatch)> = vec![];
    let mut add: Option<TrackKind> = None;
    egui::Window::new(s.t("tools-manage-tracks"))
        .open(&mut open)
        .show(ctx, |ui| {
            egui::Grid::new("tracks")
                .num_columns(5)
                .striped(true)
                .show(ui, |ui| {
                    for t in &seq.tracks {
                        let entry =
                            match s.dialogs.track_names.iter().position(|(id, _)| *id == t.id) {
                                Some(i) => i,
                                None => {
                                    s.dialogs.track_names.push((t.id, t.name.clone()));
                                    s.dialogs.track_names.len() - 1
                                }
                            };
                        ui.label(format!("{:?}", t.kind));
                        let r = ui.text_edit_singleline(&mut s.dialogs.track_names[entry].1);
                        if r.lost_focus() && s.dialogs.track_names[entry].1 != t.name {
                            patches.push((
                                t.id,
                                TrackPatch {
                                    name: Some(s.dialogs.track_names[entry].1.clone()),
                                    ..Default::default()
                                },
                            ));
                        }
                        let mut locked = t.locked;
                        if ui.checkbox(&mut locked, "🔒").changed() {
                            patches.push((
                                t.id,
                                TrackPatch {
                                    locked: Some(locked),
                                    ..Default::default()
                                },
                            ));
                        }
                        let mut hidden = t.hidden;
                        if ui.checkbox(&mut hidden, "Hide").changed() {
                            patches.push((
                                t.id,
                                TrackPatch {
                                    hidden: Some(hidden),
                                    ..Default::default()
                                },
                            ));
                        }
                        let mut muted = t.muted;
                        if ui.checkbox(&mut muted, "Mute").changed() {
                            patches.push((
                                t.id,
                                TrackPatch {
                                    muted: Some(muted),
                                    ..Default::default()
                                },
                            ));
                        }
                        ui.end_row();
                    }
                });
            ui.horizontal(|ui| {
                if ui.button("+ Video track").clicked() {
                    add = Some(TrackKind::Video);
                }
                if ui.button("+ Audio track").clicked() {
                    add = Some(TrackKind::Audio);
                }
            });
        });
    for (track, patch) in patches {
        s.exec(&SetTrackProps {
            seq: seq.id,
            track,
            patch,
        });
    }
    if let Some(kind) = add {
        s.exec(&AddTrack {
            seq: seq.id,
            id: TrackId::new(),
            kind,
            name: None,
            index: None,
        });
    }
    s.dialogs.manage_tracks = open;
}

fn preferences(ctx: &egui::Context, s: &mut AppState) {
    if !s.dialogs.preferences {
        return;
    }
    let mut open = true;
    egui::Window::new(s.t("edit-preferences"))
        .open(&mut open)
        .resizable(false)
        .show(ctx, |ui| {
            egui::Grid::new("prefs")
                .num_columns(2)
                .spacing([10.0, 8.0])
                .show(ui, |ui| {
                    ui.label(s.t("collab-name"));
                    ui.text_edit_singleline(&mut s.settings.user_name);
                    ui.end_row();
                    ui.label(s.t("view-language"));
                    let mut l = s.settings.lang;
                    egui::ComboBox::from_id_salt("pref-lang")
                        .selected_text(l.label())
                        .show_ui(ui, |ui| {
                            for x in crate::i18n::Lang::ALL {
                                ui.selectable_value(&mut l, x, x.label());
                            }
                        });
                    if l != s.settings.lang {
                        s.settings.lang = l;
                        s.i18n = crate::i18n::I18n::new(l);
                    }
                    ui.end_row();
                    ui.label("Audio output");
                    ui.checkbox(
                        &mut s.settings.use_audio_device,
                        "Use audio device (restart to apply)",
                    );
                    ui.end_row();
                    ui.label("Audio device");
                    ui.label(RichText::new(s.playback.audio_device()).small());
                    ui.end_row();
                    ui.label("Media backend");
                    ui.label(RichText::new(s.backend.name()).small());
                    ui.end_row();
                });
        });
    if !open {
        let _ = s.settings.save(&s.config_dir);
    }
    s.dialogs.preferences = open;
}

fn about(ctx: &egui::Context, s: &mut AppState) {
    if !s.dialogs.about {
        return;
    }
    let mut open = true;
    egui::Window::new(s.t("help-about"))
        .open(&mut open)
        .resizable(false)
        .show(ctx, |ui| {
            ui.heading("Cutroom");
            ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
            ui.label("A collaborative, frame-accurate video editor.");
            ui.label(
                RichText::new("Loro CRDT · wgpu · FFmpeg · egui")
                    .small()
                    .weak(),
            );
        });
    s.dialogs.about = open;
}

fn marker(ctx: &egui::Context, s: &mut AppState) {
    let Some((id, mut name, mut comment)) = s.dialogs.edit_marker.take() else {
        return;
    };
    let mut open = true;
    let mut save = false;
    egui::Window::new("Marker")
        .open(&mut open)
        .resizable(false)
        .show(ctx, |ui| {
            ui.text_edit_singleline(&mut name);
            ui.text_edit_multiline(&mut comment);
            if ui.button("OK").clicked() {
                save = true;
            }
        });
    if save {
        if let Some(seq) = s.seq() {
            s.exec(&EditMarker {
                seq,
                id,
                name: Some(name),
                comment: Some(comment),
                color: None,
                resolved: None,
            });
        }
    } else if open {
        s.dialogs.edit_marker = Some((id, name, comment));
    }
}

fn rename(ctx: &egui::Context, s: &mut AppState) {
    let Some((id, mut name)) = s.dialogs.rename_media.take() else {
        return;
    };
    let mut open = true;
    let mut save = false;
    egui::Window::new("Rename")
        .open(&mut open)
        .resizable(false)
        .show(ctx, |ui| {
            let r = ui.text_edit_singleline(&mut name);
            r.request_focus();
            if (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                || ui.button("OK").clicked()
            {
                save = true;
            }
        });
    if save {
        s.exec(&RenameMedia { id, name });
    } else if open {
        s.dialogs.rename_media = Some((id, name));
    }
}
