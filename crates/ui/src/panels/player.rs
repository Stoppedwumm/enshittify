//! Player panel (program monitor) and source viewer (Dual layout).

use std::sync::Arc;

use cutroom_core::model::MediaKind;
use cutroom_core::{Ticks, Timecode};
use cutroom_media::cache::Quality;
use egui::{Color32, RichText, Sense, Stroke, Vec2};

use crate::app::AppState;
use crate::shortcuts::Action;
use crate::theme;

#[derive(Default)]
pub struct PlayerUi {
    tc_edit: Option<String>,
}

fn fit(avail: egui::Rect, aspect: f32) -> egui::Rect {
    let (w, h) = (avail.width(), avail.height());
    let size = if w / h > aspect {
        Vec2::new(h * aspect, h)
    } else {
        Vec2::new(w, w / aspect)
    };
    egui::Rect::from_center_size(avail.center(), size)
}

fn transport_button(ui: &mut egui::Ui, s: &AppState, label: &str, a: Action, tip: &str) -> bool {
    let hint = s.shortcuts.hint(a);
    let tip = if hint.is_empty() {
        tip.to_string()
    } else {
        format!("{tip} ({hint})")
    };
    ui.add(egui::Button::new(RichText::new(label).size(15.0)).frame(false))
        .on_hover_text(tip)
        .clicked()
}

pub fn ui(ui: &mut egui::Ui, s: &mut AppState, source: bool) {
    if source {
        return source_ui(ui, s);
    }
    let ctx = ui.ctx().clone();
    let snap = s.session.snapshot();
    let Some(seq) = snap.main_sequence else {
        return;
    };
    let aspect = snap.meta.width as f32 / snap.meta.height.max(1) as f32;
    let controls_h = 58.0;
    let avail = ui.available_rect_before_wrap();
    let video_area =
        egui::Rect::from_min_max(avail.min, egui::pos2(avail.max.x, avail.max.y - controls_h));
    ui.painter()
        .rect_filled(video_area, 0.0, Color32::from_gray(12));
    let rect = fit(video_area.shrink(4.0), aspect);
    ui.painter().rect_filled(rect, 0.0, Color32::BLACK);

    // Live (uncommitted) property edits preview through a patched snapshot.
    let render_snap = match s.pending_transform {
        Some((clip, t)) => {
            let mut sn = (*snap).clone();
            if let Some(c) = sn
                .sequences
                .get_mut(&seq)
                .and_then(|q| q.clips.get_mut(&clip))
            {
                c.transform = t;
            }
            Arc::new(sn)
        }
        None => snap.clone(),
    };
    let playing = s.playback.is_playing();
    let display_scale =
        (rect.width() * ctx.pixels_per_point()) as f64 / snap.meta.width.max(1) as f64;
    let (q, scale) = if playing {
        let q = s.quality();
        let f = match q {
            Quality::Full => 1.0,
            Quality::Half => 0.5,
            _ => 0.25,
        };
        (q, display_scale.min(f))
    } else {
        (Quality::Full, display_scale.min(1.0))
    };
    if let Some((tex, _)) = s.preview.render(
        &ctx,
        &render_snap,
        seq,
        s.playback.playhead,
        scale.max(0.1),
        &s.frames,
        q,
        playing,
    ) {
        ui.painter().image(
            tex,
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    if s.settings.safe_zones {
        for (f, c) in [
            (0.9, Color32::from_white_alpha(90)),
            (0.8, Color32::from_rgba_unmultiplied(46, 211, 168, 110)),
        ] {
            let r = egui::Rect::from_center_size(rect.center(), rect.size() * f);
            ui.painter()
                .rect_stroke(r, 0.0, Stroke::new(1.0, c), egui::StrokeKind::Inside);
        }
        let c = rect.center();
        ui.painter().line_segment(
            [c - Vec2::new(8.0, 0.0), c + Vec2::new(8.0, 0.0)],
            Stroke::new(1.0, Color32::from_white_alpha(120)),
        );
        ui.painter().line_segment(
            [c - Vec2::new(0.0, 8.0), c + Vec2::new(0.0, 8.0)],
            Stroke::new(1.0, Color32::from_white_alpha(120)),
        );
    }
    ui.allocate_rect(video_area, Sense::hover());

    // Transport row.
    let tb = snap.meta.timebase;
    let total = snap.main().map(|q| q.duration()).unwrap_or_default();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let df = tb.supports_drop_frame();
        let cur = Timecode::from_ticks(s.playback.playhead, tb, df).to_string();
        let edit = s.player_ui.tc_edit.get_or_insert_with(|| cur.clone());
        let r = ui.add(
            egui::TextEdit::singleline(edit)
                .font(egui::TextStyle::Monospace)
                .desired_width(92.0),
        );
        if r.lost_focus() {
            if let Ok(tc) = Timecode::parse(edit, tb)
                && let Ok(f) = tc.to_frame(tb)
            {
                let t = tb.frames_to_ticks(f);
                s.player_ui.tc_edit = None;
                s.seek(t);
            } else {
                s.player_ui.tc_edit = None;
            }
        } else if !r.has_focus() {
            s.player_ui.tc_edit = None;
        }
        ui.label(
            RichText::new(format!("/ {}", Timecode::from_ticks(total, tb, df)))
                .monospace()
                .weak(),
        );
        ui.separator();
        let mut act = None;
        if transport_button(ui, s, "⏮", Action::PrevFrame, "Previous frame") {
            act = Some(Action::PrevFrame);
        }
        let pp = if s.playback.is_playing() {
            "⏸"
        } else {
            "▶"
        };
        if transport_button(ui, s, pp, Action::PlayPause, "Play / pause") {
            act = Some(Action::PlayPause);
        }
        if transport_button(ui, s, "⏭", Action::NextFrame, "Next frame") {
            act = Some(Action::NextFrame);
        }
        if transport_button(ui, s, "⏹", Action::Stop, "Stop") {
            act = Some(Action::Stop);
        }
        ui.separator();
        if transport_button(ui, s, "{", Action::MarkIn, "Mark in") {
            act = Some(Action::MarkIn);
        }
        if transport_button(ui, s, "}", Action::MarkOut, "Mark out") {
            act = Some(Action::MarkOut);
        }
        if s.playback.speed().abs() > 1 {
            ui.label(RichText::new(format!("{}×", s.playback.speed())).color(theme::ACCENT));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if transport_button(ui, s, "⛶", Action::FullscreenPreview, "Fullscreen") {
                act = Some(Action::FullscreenPreview);
            }
            if transport_button(ui, s, "📷", Action::Snapshot, "Snapshot") {
                act = Some(Action::Snapshot);
            }
            let mut sz = s.settings.safe_zones;
            if ui
                .toggle_value(&mut sz, "▣")
                .on_hover_text(s.t("view-safe-zones"))
                .changed()
            {
                s.settings.safe_zones = sz;
            }
            let qkeys = ["quality-full", "quality-half", "quality-quarter"];
            let mut qi = s.settings.preview_quality.min(2);
            egui::ComboBox::from_id_salt("preview-quality")
                .width(56.0)
                .selected_text(s.t(qkeys[qi as usize]))
                .show_ui(ui, |ui| {
                    for (i, k) in qkeys.iter().enumerate() {
                        ui.selectable_value(&mut qi, i as u8, s.t(k));
                    }
                })
                .response
                .on_hover_text(s.t("quality"));
            s.settings.preview_quality = qi;
        });
        if let Some(a) = act {
            s.run_action(a, &ctx);
        }
    });
}

/// Source viewer: shows the selected media item on its own playhead.
fn source_ui(ui: &mut egui::Ui, s: &mut AppState) {
    let Some(id) = s.selected_media else {
        ui.centered_and_justified(|ui| ui.label(RichText::new("Select a media item").weak()));
        return;
    };
    let snap = s.session.snapshot();
    let Some(e) = snap.media.get(&id).cloned() else {
        return;
    };
    let avail = ui.available_rect_before_wrap();
    let area = egui::Rect::from_min_max(avail.min, egui::pos2(avail.max.x, avail.max.y - 36.0));
    ui.painter().rect_filled(area, 0.0, Color32::from_gray(12));
    if e.kind != MediaKind::Audio {
        let idx = e.fps.map(|f| f.ticks_to_frame(s.source_tick)).unwrap_or(0);
        if let Some(frame) = s.frames.request(id, idx, Quality::Half) {
            let img = egui::ColorImage::from_rgba_unmultiplied(
                [frame.width as usize, frame.height as usize],
                &frame.rgba,
            );
            let tex = ui
                .ctx()
                .load_texture("source-viewer", img, egui::TextureOptions::LINEAR);
            let aspect = e.width as f32 / e.height.max(1) as f32;
            let r = fit(area.shrink(4.0), aspect);
            ui.painter().image(
                tex.id(),
                r,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    } else {
        ui.painter().text(
            area.center(),
            egui::Align2::CENTER_CENTER,
            "🎵",
            egui::FontId::proportional(40.0),
            Color32::GRAY,
        );
    }
    ui.allocate_rect(area, Sense::hover());
    ui.horizontal(|ui| {
        let mut secs = s.source_tick.as_secs_f64();
        let max = e.duration.as_secs_f64().max(0.001);
        ui.label(RichText::new(&e.name).small());
        if ui
            .add(egui::Slider::new(&mut secs, 0.0..=max).show_value(false))
            .changed()
        {
            s.source_tick = Ticks::from_secs_f64(secs);
        }
        if ui
            .button("Insert ⤓")
            .on_hover_text(s.shortcuts.hint(Action::InsertEdit))
            .clicked()
        {
            crate::panels::timeline::place_media_at_playhead(s, id, true);
        }
    });
}
