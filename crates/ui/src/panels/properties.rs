//! Property panel. Edits go through commands; while a value is being dragged
//! the preview uses an uncommitted override and a single command (one undo
//! step) is issued on release.

use cutroom_core::ClipId;
use cutroom_core::command::{ResetTransform, SetClipAudio, SetTransform, TransformField};
use cutroom_core::model::{BlendMode, Clip, MediaKind, Transform};
use egui::{Color32, RichText};

use crate::app::AppState;
use crate::shortcuts::Action;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PropTab {
    #[default]
    Video,
    Audio,
    Color,
    Speed,
}

#[derive(Default)]
pub struct PropsUi {
    pub tab: PropTab,
    pending_audio: Option<(ClipId, f64, f64)>,
}

/// Keyframe diamond next to a numeric property (keyframing lands in v1).
fn diamond(ui: &mut egui::Ui, s: &mut AppState) {
    let r = ui
        .add(egui::Button::new(RichText::new("◇").color(Color32::GRAY)).frame(false))
        .on_hover_text(format!(
            "Keyframe ({})",
            s.shortcuts.hint(Action::AddKeyframe)
        ));
    let alt_click = r.clicked()
        && s.shortcuts
            .click_matches(Action::AddKeyframe, ui.input(|i| i.modifiers));
    if r.clicked() || alt_click {
        s.not_available("Keyframes");
    }
}

fn module(ui: &mut egui::Ui, title: String, open: bool, add: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(RichText::new(title).strong())
        .default_open(open)
        .show(ui, add);
}

fn coming(ui: &mut egui::Ui, s: &AppState) {
    ui.label(RichText::new(s.t("coming-soon")).weak().small());
}

pub fn ui(ui: &mut egui::Ui, s: &mut AppState) {
    let Some(clip) = s.selected_clips().into_iter().next() else {
        ui.add_space(16.0);
        ui.vertical_centered(|ui| ui.label(RichText::new(s.t("props-none")).weak()));
        return;
    };
    let snap = s.session.snapshot();
    let media = clip.media_id().and_then(|m| snap.media.get(&m)).cloned();
    let has_video = media
        .as_ref()
        .map(|m| m.kind != MediaKind::Audio)
        .unwrap_or(true);
    let has_audio = media
        .as_ref()
        .map(|m| m.audio_channels > 0)
        .unwrap_or(false);

    // Soft lock badge: someone else has this clip selected.
    let key = clip.id.key();
    for p in s.presence.peers() {
        if p.selected_clips.contains(&key) {
            let c = Color32::from_rgb(p.color[0], p.color[1], p.color[2]);
            egui::Frame::new()
                .fill(c)
                .corner_radius(4.0)
                .inner_margin(4.0)
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(s.i18n.ta("props-editing-by", &[("name", &p.name)]))
                            .color(Color32::BLACK),
                    );
                });
        }
    }

    ui.label(RichText::new(&clip.name).strong());
    ui.horizontal(|ui| {
        let tabs = [
            (PropTab::Video, "props-video", has_video),
            (PropTab::Audio, "props-audio", has_audio),
            (PropTab::Color, "props-color", has_video),
            (PropTab::Speed, "props-speed", true),
        ];
        if !tabs.iter().any(|(t, _, on)| *t == s.props.tab && *on) {
            s.props.tab = tabs.iter().find(|t| t.2).map(|t| t.0).unwrap_or_default();
        }
        for (t, k, on) in tabs {
            if on && ui.selectable_label(s.props.tab == t, s.t(k)).clicked() {
                s.props.tab = t;
            }
        }
    });
    ui.separator();
    let footer_h = 34.0;
    let h = ui.available_height() - footer_h;
    egui::ScrollArea::vertical()
        .max_height(h)
        .auto_shrink(false)
        .show(ui, |ui| match s.props.tab {
            PropTab::Video => video_tab(ui, s, &clip),
            PropTab::Audio => audio_tab(ui, s, &clip),
            PropTab::Color => {
                for k in ["Basic", "LUT", "HSL", "Curves", "Wheels"] {
                    module(ui, k.to_string(), false, |ui| coming(ui, s));
                }
            }
            PropTab::Speed => {
                for k in ["Uniform", "Ramp", "Reverse", "Freeze"] {
                    module(ui, k.to_string(), false, |ui| coming(ui, s));
                }
            }
        });
    ui.separator();
    ui.horizontal(|ui| {
        ui.add_enabled(false, egui::Button::new(s.t("props-save-custom")));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(s.t("props-reset")).clicked()
                && let Some(seq) = s.seq()
            {
                s.pending_transform = None;
                s.exec(&ResetTransform { seq, clip: clip.id });
            }
        });
    });
}

fn video_tab(ui: &mut egui::Ui, s: &mut AppState, clip: &Clip) {
    let mut t: Transform = match s.pending_transform {
        Some((id, t)) if id == clip.id => t,
        _ => clip.transform,
    };
    let before = t;
    let mut commit = false;
    fn track(commit: &mut bool, r: egui::Response) {
        if r.drag_stopped() || (r.changed() && !r.dragged()) {
            *commit = true;
        }
    }
    module(ui, s.t("props-transform"), true, |ui| {
        egui::Grid::new("xf")
            .num_columns(3)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label(s.t("props-position"));
                ui.horizontal(|ui| {
                    track(
                        &mut commit,
                        ui.add(egui::DragValue::new(&mut t.x).prefix("X ").speed(1.0)),
                    );
                    track(
                        &mut commit,
                        ui.add(egui::DragValue::new(&mut t.y).prefix("Y ").speed(1.0)),
                    );
                });
                diamond(ui, s);
                ui.end_row();
                ui.label(s.t("props-scale"));
                let mut pct = t.scale_x * 100.0;
                track(
                    &mut commit,
                    ui.add(
                        egui::DragValue::new(&mut pct)
                            .suffix(" %")
                            .speed(0.5)
                            .range(1.0..=1000.0),
                    ),
                );
                t.scale_x = pct / 100.0;
                t.scale_y = pct / 100.0;
                diamond(ui, s);
                ui.end_row();
                ui.label(s.t("props-rotation"));
                track(
                    &mut commit,
                    ui.add(egui::DragValue::new(&mut t.rotation).suffix("°").speed(0.5)),
                );
                diamond(ui, s);
                ui.end_row();
                ui.label(s.t("props-flip"));
                ui.horizontal(|ui| {
                    if ui.selectable_label(t.flip_h, "⇋").clicked() {
                        t.flip_h = !t.flip_h;
                        commit = true;
                    }
                    if ui.selectable_label(t.flip_v, "⇵").clicked() {
                        t.flip_v = !t.flip_v;
                        commit = true;
                    }
                });
                ui.end_row();
            });
    });
    module(ui, s.t("props-compositing"), true, |ui| {
        egui::Grid::new("comp")
            .num_columns(3)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label(s.t("props-blend"));
                let mut b = t.blend;
                egui::ComboBox::from_id_salt("blend")
                    .selected_text(format!("{b:?}"))
                    .show_ui(ui, |ui| {
                        for m in [
                            BlendMode::Normal,
                            BlendMode::Add,
                            BlendMode::Multiply,
                            BlendMode::Screen,
                        ] {
                            ui.selectable_value(&mut b, m, format!("{m:?}"));
                        }
                    });
                if b != t.blend {
                    t.blend = b;
                    commit = true;
                }
                ui.label("");
                ui.end_row();
                ui.label(s.t("props-opacity"));
                let mut op = t.opacity * 100.0;
                track(
                    &mut commit,
                    ui.add(egui::Slider::new(&mut op, 0.0..=100.0).suffix(" %")),
                );
                t.opacity = op / 100.0;
                diamond(ui, s);
                ui.end_row();
            });
    });
    for k in [
        "props-crop",
        "props-stabilization",
        "props-chroma-key",
        "props-mask",
        "props-animation",
    ] {
        module(ui, s.t(k), false, |ui| coming(ui, s));
    }

    if t != before || s.pending_transform.is_some_and(|(id, _)| id == clip.id) {
        s.pending_transform = Some((clip.id, t));
    }
    if commit && let Some(seq) = s.seq() {
        s.pending_transform = None;
        if t != clip.transform {
            s.exec(&SetTransform {
                seq,
                clip: clip.id,
                fields: vec![
                    TransformField::X(t.x),
                    TransformField::Y(t.y),
                    TransformField::ScaleX(t.scale_x),
                    TransformField::ScaleY(t.scale_y),
                    TransformField::Rotation(t.rotation),
                    TransformField::Opacity(t.opacity),
                    TransformField::FlipH(t.flip_h),
                    TransformField::FlipV(t.flip_v),
                    TransformField::Blend(t.blend),
                ],
            });
        }
    }
}

fn audio_tab(ui: &mut egui::Ui, s: &mut AppState, clip: &Clip) {
    let (mut gain, mut pan) = match s.props.pending_audio {
        Some((id, g, p)) if id == clip.id => (g, p),
        _ => (clip.audio.gain_db, clip.audio.pan),
    };
    let mut commit = false;
    module(ui, s.t("props-volume"), true, |ui| {
        egui::Grid::new("aud")
            .num_columns(3)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label(s.t("props-volume"));
                let r = ui.add(egui::Slider::new(&mut gain, -60.0..=12.0).suffix(" dB"));
                commit |= r.drag_stopped() || (r.changed() && !r.dragged());
                diamond(ui, s);
                ui.end_row();
                ui.label(s.t("props-pan"));
                let r = ui.add(egui::Slider::new(&mut pan, -1.0..=1.0));
                commit |= r.drag_stopped() || (r.changed() && !r.dragged());
                ui.label("");
                ui.end_row();
            });
    });
    module(ui, s.t("props-fades"), false, |ui| {
        ui.label(
            RichText::new(format!(
                "In {:.2}s · Out {:.2}s",
                clip.audio.fade_in.as_secs_f64(),
                clip.audio.fade_out.as_secs_f64()
            ))
            .small(),
        );
        coming(ui, s);
    });
    for k in ["Pitch", "Denoise", "Ducking"] {
        module(ui, k.to_string(), false, |ui| coming(ui, s));
    }
    s.props.pending_audio =
        ((gain, pan) != (clip.audio.gain_db, clip.audio.pan)).then_some((clip.id, gain, pan));
    if commit && let Some(seq) = s.seq() {
        s.props.pending_audio = None;
        s.exec(&SetClipAudio {
            seq,
            clip: clip.id,
            gain_db: Some(gain),
            pan: Some(pan),
        });
    }
}
