//! Custom-painted timeline.

use std::collections::BTreeSet;

use cutroom_core::command::{
    AddClip, AddTrack, MoveClips, NewClip, SetTrackProps, SplitClip, TrackPatch, TrimClip,
};
use cutroom_core::model::{
    Clip, ClipSource, DocSnapshot, Generator, MediaKind, Sequence, Track, TrackKind,
};
use cutroom_core::normalize::normalize_track;
use cutroom_core::{ClipId, MediaId, Ticks, Timebase, Timecode, TrackId};
use cutroom_sync::protocol::DragGhost;
use egui::{Align2, Color32, CursorIcon, FontId, Pos2, Rect, RichText, Sense, Stroke, pos2, vec2};

use crate::app::AppState;
use crate::panels::media::MediaDrag;
use crate::shortcuts::{Action, Scope};
use crate::theme;

const RULER_H: f32 = 26.0;
const HEADER_W: f32 = 156.0;
const EDGE_GRAB: f32 = 6.0;
const SNAP_PX: f32 = 8.0;
const SECTION_GAP: f32 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Select,
    Blade,
}

#[derive(Clone, Debug)]
enum Drag {
    Scrub,
    Move {
        anchor: Ticks,
        anchor_row: usize,
        clips: Vec<Clip>,
        delta: Ticks,
        row_delta: i32,
    },
    Trim {
        clip: Box<Clip>,
        left: bool,
        t: Ticks,
    },
    ResizeTrack {
        track: TrackId,
        start_h: f32,
        start_y: f32,
    },
    Marquee {
        start: Pos2,
    },
}

pub struct TimelineUi {
    pub px_per_sec: f32,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub tool: Tool,
    drag: Option<Drag>,
    view_w: f32,
    ghost: Option<DragGhost>,
    /// Pending track height while resizing (committed on release).
    pending_height: Option<(TrackId, f32)>,
}

impl Default for TimelineUi {
    fn default() -> Self {
        TimelineUi {
            px_per_sec: 60.0,
            scroll_x: 0.0,
            scroll_y: 0.0,
            tool: Tool::Select,
            drag: None,
            view_w: 800.0,
            ghost: None,
            pending_height: None,
        }
    }
}

impl TimelineUi {
    pub fn zoom(&mut self, f: f32) {
        self.px_per_sec = (self.px_per_sec * f).clamp(1.0, 4000.0);
    }

    pub fn fit(&mut self, end: Ticks) {
        let secs = end.as_secs_f64().max(1.0) as f32;
        self.px_per_sec = ((self.view_w - 40.0).max(100.0) / secs).clamp(1.0, 4000.0);
        self.scroll_x = 0.0;
    }

    /// For presence: what is being dragged right now.
    pub fn drag_ghost(&self) -> Option<DragGhost> {
        self.ghost.clone()
    }

    /// Visible time range in ticks (presence).
    pub fn viewport(&self) -> (i64, i64) {
        let a = Ticks::from_secs_f64((self.scroll_x / self.px_per_sec) as f64);
        let b = Ticks::from_secs_f64(((self.scroll_x + self.view_w) / self.px_per_sec) as f64);
        (a.0, b.0)
    }

    fn x(&self, t: Ticks, left: f32) -> f32 {
        left + t.as_secs_f64() as f32 * self.px_per_sec - self.scroll_x
    }

    fn t(&self, x: f32, left: f32) -> Ticks {
        Ticks::from_secs_f64(((x - left + self.scroll_x) / self.px_per_sec).max(0.0) as f64)
    }
}

struct Row {
    track: Track,
    top: f32,
    h: f32,
}

fn rows(seq: &Sequence, top: f32, pending: Option<(TrackId, f32)>) -> (Vec<Row>, f32) {
    let mut out = Vec::new();
    let mut y = top;
    let h_of = |t: &Track| match pending {
        Some((id, h)) if id == t.id => h,
        _ => t.height,
    };
    let visual: Vec<&Track> = seq.tracks.iter().filter(|t| t.kind.is_visual()).collect();
    for t in visual.iter().rev() {
        let h = h_of(t);
        out.push(Row {
            track: (*t).clone(),
            top: y,
            h,
        });
        y += h + 1.0;
    }
    y += SECTION_GAP;
    for t in seq.tracks.iter().filter(|t| t.kind == TrackKind::Audio) {
        let h = h_of(t);
        out.push(Row {
            track: t.clone(),
            top: y,
            h,
        });
        y += h + 1.0;
    }
    (out, y)
}

fn clip_color(snap: &DocSnapshot, c: &Clip, kind: TrackKind) -> Color32 {
    match &c.source {
        ClipSource::Generator(Generator::Title) => theme::CLIP_TITLE,
        ClipSource::Generator(_) => theme::CLIP_GENERATOR,
        ClipSource::Media(m) => match snap.media.get(m).map(|e| e.kind) {
            _ if kind == TrackKind::Audio => theme::CLIP_AUDIO,
            Some(MediaKind::Audio) => theme::CLIP_AUDIO,
            _ => theme::CLIP_VIDEO,
        },
    }
}

/// Snaps `t` to the nearest candidate within `thr`; returns (snapped, hit).
fn snap_to(t: Ticks, cands: &[Ticks], thr: Ticks) -> (Ticks, bool) {
    cands
        .iter()
        .map(|c| (*c, (c.0 - t.0).abs()))
        .filter(|(_, d)| *d <= thr.0)
        .min_by_key(|(_, d)| *d)
        .map(|(c, _)| (c, true))
        .unwrap_or((t, false))
}

fn ruler_step(pps: f32, tb: Timebase) -> (f64, f64) {
    let frame = 1.0 / tb.fps_f64();
    let steps = [
        frame,
        2.0 * frame,
        5.0 * frame,
        10.0 * frame,
        0.5,
        1.0,
        2.0,
        5.0,
        10.0,
        15.0,
        30.0,
        60.0,
        120.0,
        300.0,
        600.0,
        1800.0,
        3600.0,
    ];
    let major = steps
        .iter()
        .copied()
        .find(|s| *s * pps as f64 >= 90.0)
        .unwrap_or(3600.0);
    let minor = if major <= 10.0 * frame {
        frame
    } else {
        major / 5.0
    };
    (major, minor)
}

/// Places a media item on the timeline at `at` on a suitable track.
pub fn place_media(
    s: &mut AppState,
    media: MediaId,
    track_hint: Option<TrackId>,
    at: Ticks,
) -> Option<ClipId> {
    let snap = s.session.snapshot();
    let seq = snap.main()?;
    let e = snap.media.get(&media)?;
    let tb = snap.meta.timebase;
    let want_audio = e.kind == MediaKind::Audio;
    let fits = |t: &Track| {
        if want_audio {
            t.kind == TrackKind::Audio
        } else {
            t.kind == TrackKind::Video
        }
    };
    let track = track_hint
        .and_then(|id| seq.track(id))
        .filter(|t| fits(t) && !t.locked)
        .or_else(|| seq.tracks.iter().find(|t| fits(t) && !t.locked))?
        .id;
    let dur = match e.kind {
        MediaKind::Image => Ticks::from_secs(5),
        _ => e.duration,
    };
    if dur.0 <= 0 {
        return None;
    }
    let id = ClipId::new();
    let ok = s.exec(&AddClip {
        seq: seq.id,
        clip: NewClip {
            id,
            name: e.name.clone(),
            track,
            start: tb.snap(at),
            in_tick: Ticks::ZERO,
            out_tick: dur,
            source: ClipSource::Media(media),
        },
    });
    ok.then_some(id)
}

/// Insert/Overwrite/Apply: place at the playhead on the first suitable track.
pub fn place_media_at_playhead(s: &mut AppState, media: MediaId, _insert: bool) {
    // Ripple insert lands with the MVP's ripple engine; both place for now.
    let at = s.playback.playhead;
    if let Some(id) = place_media(s, media, None, at) {
        s.selection = BTreeSet::from([id]);
    }
}

fn toolbar(ui: &mut egui::Ui, s: &mut AppState) {
    let ctx = ui.ctx().clone();
    let mut act: Option<Action> = None;
    ui.horizontal(|ui| {
        let mut b = |ui: &mut egui::Ui, s: &AppState, icon: &str, a: Action, tip: &str| {
            let hint = s.shortcuts.hint(a);
            let tip = if hint.is_empty() {
                tip.to_string()
            } else {
                format!("{tip} ({hint})")
            };
            if ui
                .add(egui::Button::new(RichText::new(icon).size(14.0)).frame(false))
                .on_hover_text(tip)
                .clicked()
            {
                act = Some(a);
            }
        };
        b(ui, s, "↶", Action::Undo, &s.t("tl-undo"));
        b(ui, s, "↷", Action::Redo, &s.t("tl-redo"));
        b(ui, s, "🗑", Action::Delete, &s.t("tl-delete"));
        b(ui, s, "✂", Action::SplitAtPlayhead, &s.t("tl-split"));
        ui.separator();
        b(ui, s, "⛶", Action::CropAndZoom, "Crop");
        b(ui, s, "⏩", Action::SpeedDialog, "Speed");
        b(ui, s, "◐", Action::ChromaKey, "Mask");
        b(ui, s, "◆", Action::KeyframePanel, "Keyframe");
        b(ui, s, "🎨", Action::ColorCorrection, "Color");
        b(ui, s, "🔊", Action::DetachAudio, "Audio");
        b(ui, s, "T", Action::ApplySelectedAsset, "Text");
        ui.menu_button(RichText::new("⋯").size(14.0), |ui| {
            if ui.button(s.t("tools-manage-tracks")).clicked() {
                s.dialogs.manage_tracks = true;
                ui.close();
            }
            if ui.button("Add video track").clicked() {
                add_track(s, TrackKind::Video);
                ui.close();
            }
            if ui.button("Add audio track").clicked() {
                add_track(s, TrackKind::Audio);
                ui.close();
            }
            if ui.button("Add color clip").clicked() {
                add_color_clip(s);
                ui.close();
            }
        })
        .response
        .on_hover_text(s.t("tl-more"));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let end = s
                .session
                .snapshot()
                .main()
                .map(|q| q.duration())
                .unwrap_or_default();
            if ui
                .add(egui::Button::new("⇔").frame(false))
                .on_hover_text(format!(
                    "{} ({})",
                    s.t("tl-zoom-fit"),
                    s.shortcuts.hint(Action::ZoomToFit)
                ))
                .clicked()
            {
                s.timeline.fit(end);
            }
            let mut z = s.timeline.px_per_sec.ln();
            if ui
                .add(egui::Slider::new(&mut z, 0.0..=8.3).show_value(false))
                .changed()
            {
                s.timeline.px_per_sec = z.exp();
            }
            b(
                ui,
                s,
                "▶▣",
                Action::RenderPreview,
                &s.t("tl-render-preview"),
            );
            b(ui, s, "⚑", Action::AddMarker, &s.t("tl-add-marker"));
            ui.toggle_value(&mut s.settings.snapping, "🧲")
                .on_hover_text(format!(
                    "{} ({})",
                    s.t("tl-snapping"),
                    s.shortcuts.hint(Action::ToggleSnapping)
                ));
            ui.toggle_value(&mut s.settings.magnetic, "⧈")
                .on_hover_text(s.t("tl-magnetic"));
            ui.separator();
            if ui
                .selectable_label(s.timeline.tool == Tool::Blade, "✂")
                .on_hover_text(s.t("tl-blade"))
                .clicked()
            {
                s.timeline.tool = Tool::Blade;
            }
            if ui
                .selectable_label(s.timeline.tool == Tool::Select, "⬉")
                .on_hover_text(s.t("tl-select"))
                .clicked()
            {
                s.timeline.tool = Tool::Select;
            }
        });
    });
    if let Some(a) = act {
        s.run_action(a, &ctx);
    }
}

pub fn add_track(s: &mut AppState, kind: TrackKind) {
    if let Some(seq) = s.seq() {
        s.exec(&AddTrack {
            seq,
            id: TrackId::new(),
            kind,
            name: None,
            index: None,
        });
    }
}

fn add_color_clip(s: &mut AppState) {
    let snap = s.session.snapshot();
    let Some(seq) = snap.main() else { return };
    let Some(track) = seq.tracks.iter().find(|t| t.kind == TrackKind::Video) else {
        return;
    };
    let (sid, tid) = (seq.id, track.id);
    let at = s.playback.playhead;
    s.exec(&AddClip {
        seq: sid,
        clip: NewClip {
            id: ClipId::new(),
            name: "Color".into(),
            track: tid,
            start: at,
            in_tick: Ticks::ZERO,
            out_tick: Ticks::from_secs(5),
            source: ClipSource::Generator(Generator::Color([0.18, 0.83, 0.66, 1.0])),
        },
    });
}

pub fn ui(ui: &mut egui::Ui, s: &mut AppState) {
    toolbar(ui, s);
    ui.separator();
    let snap = s.session.snapshot();
    let Some(seq) = snap.main().cloned() else {
        return;
    };
    let tb = snap.meta.timebase;

    // Compound-clip tabs (only the main sequence until v1).
    ui.horizontal(|ui| {
        let _ = ui.selectable_label(true, RichText::new(format!("▤ {}", seq.name)).small());
    });

    let full = ui.available_rect_before_wrap();
    let header = Rect::from_min_max(full.min, pos2(full.min.x + HEADER_W, full.max.y));
    let content = Rect::from_min_max(pos2(header.max.x, full.min.y), full.max);
    let ruler = Rect::from_min_max(content.min, pos2(content.max.x, content.min.y + RULER_H));
    let tracks_area = Rect::from_min_max(pos2(content.min.x, ruler.max.y), content.max);
    s.timeline.view_w = content.width();
    let left = content.min.x;
    let (rows, rows_bottom) = rows(
        &seq,
        tracks_area.min.y - s.timeline.scroll_y,
        s.timeline.pending_height,
    );
    let rows_h = rows_bottom - (tracks_area.min.y - s.timeline.scroll_y);

    let id = ui.id().with("timeline-content");
    let resp = ui.interact(content, id, Sense::click_and_drag());
    let header_resp = ui.interact(
        header,
        ui.id().with("timeline-header"),
        Sense::click_and_drag(),
    );
    if resp.clicked() || resp.drag_started() || header_resp.clicked() {
        s.focus = Scope::Timeline;
    }

    // ------------------------------------------------------------ input
    let pointer = ui.ctx().pointer_interact_pos();
    let modifiers = ui.input(|i| i.modifiers);
    if ui.rect_contains_pointer(full) {
        let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
        if zoom != 1.0 || (modifiers.command && scroll.y != 0.0) {
            let f = if zoom != 1.0 {
                zoom
            } else {
                (scroll.y / 200.0).exp()
            };
            if let Some(p) = pointer {
                let t = s.timeline.t(p.x, left);
                s.timeline.zoom(f);
                s.timeline.scroll_x =
                    (t.as_secs_f64() as f32 * s.timeline.px_per_sec - (p.x - left)).max(0.0);
            } else {
                s.timeline.zoom(f);
            }
        } else if modifiers.shift || scroll.x != 0.0 {
            let d = if scroll.x != 0.0 { scroll.x } else { scroll.y };
            s.timeline.scroll_x = (s.timeline.scroll_x - d).max(0.0);
        } else if scroll.y != 0.0 {
            let max = (rows_h - tracks_area.height()).max(0.0);
            s.timeline.scroll_y = (s.timeline.scroll_y - scroll.y).clamp(0.0, max);
        }
    }
    // Auto-scroll to keep the playhead visible while playing.
    if s.playback.is_playing() {
        let px = s.timeline.x(s.playback.playhead, left);
        if px > content.max.x - 20.0 || px < content.min.x {
            s.timeline.scroll_x =
                (s.playback.playhead.as_secs_f64() as f32 * s.timeline.px_per_sec - 40.0).max(0.0);
        }
    }

    let row_at = |y: f32| rows.iter().position(|r| y >= r.top && y < r.top + r.h);
    let clip_rect = |c: &Clip, r: &Row, tl: &TimelineUi| {
        Rect::from_min_max(
            pos2(tl.x(c.start, left), r.top + 1.0),
            pos2(tl.x(c.end(), left), r.top + r.h - 1.0),
        )
    };
    let hit_clip = |p: Pos2, tl: &TimelineUi| -> Option<(Clip, Rect, usize)> {
        let ri = row_at(p.y)?;
        let r = &rows[ri];
        seq.clips_on(r.track.id)
            .map(|c| (c.clone(), clip_rect(c, r, tl), ri))
            .filter(|(_, rect, _)| rect.expand2(vec2(EDGE_GRAB, 0.0)).contains(p))
            .max_by_key(|(c, _, _)| (c.pos_clock, c.pos_peer))
    };

    // Snap candidates (playhead, edit points, markers, in/out), excluding moving clips.
    let mut fixed_cands = vec![s.playback.playhead, Ticks::ZERO];
    fixed_cands.extend(seq.markers.values().map(|m| m.tick));
    fixed_cands.extend(s.mark_in);
    fixed_cands.extend(s.mark_out);
    let snap_cands = |exclude: &BTreeSet<ClipId>| -> Vec<Ticks> {
        let mut v = fixed_cands.clone();
        v.extend(
            seq.clips
                .values()
                .filter(|c| !exclude.contains(&c.id))
                .flat_map(|c| [c.start, c.end()]),
        );
        v
    };
    let snap_thr = Ticks::from_secs_f64((SNAP_PX / s.timeline.px_per_sec) as f64);

    // Hover cursor feedback.
    if let Some(p) = ui.ctx().pointer_hover_pos()
        && tracks_area.contains(p)
        && s.timeline.drag.is_none()
    {
        if s.timeline.tool == Tool::Blade {
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        } else if let Some((_, rect, _)) = hit_clip(p, &s.timeline)
            && ((p.x - rect.left()).abs() < EDGE_GRAB || (p.x - rect.right()).abs() < EDGE_GRAB)
        {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }
    }

    if resp.drag_started()
        && let Some(p) = resp.interact_pointer_pos()
    {
        if ruler.contains(p) {
            s.timeline.drag = Some(Drag::Scrub);
        } else if s.timeline.tool == Tool::Select {
            match hit_clip(p, &s.timeline) {
                Some((c, rect, ri)) => {
                    let locked = seq.track(c.track_id).is_some_and(|t| t.locked);
                    let edge_l = (p.x - rect.left()).abs() < EDGE_GRAB;
                    let edge_r = (p.x - rect.right()).abs() < EDGE_GRAB;
                    if !s.selection.contains(&c.id) {
                        if !(modifiers.shift || modifiers.command) {
                            s.selection.clear();
                        }
                        s.selection.insert(c.id);
                    }
                    if locked {
                        s.timeline.drag = None;
                    } else if edge_l || edge_r {
                        let t = if edge_l { c.start } else { c.end() };
                        s.timeline.drag = Some(Drag::Trim {
                            clip: Box::new(c),
                            left: edge_l,
                            t,
                        });
                    } else {
                        let clips: Vec<Clip> = s
                            .selection
                            .iter()
                            .filter_map(|id| seq.clips.get(id).cloned())
                            .collect();
                        s.timeline.drag = Some(Drag::Move {
                            anchor: s.timeline.t(p.x, left),
                            anchor_row: ri,
                            clips,
                            delta: Ticks::ZERO,
                            row_delta: 0,
                        });
                    }
                }
                None => s.timeline.drag = Some(Drag::Marquee { start: p }),
            }
        }
    }
    if header_resp.drag_started()
        && let Some(p) = header_resp.interact_pointer_pos()
        && let Some(r) = rows.iter().find(|r| (p.y - (r.top + r.h)).abs() < 5.0)
    {
        s.timeline.drag = Some(Drag::ResizeTrack {
            track: r.track.id,
            start_h: r.track.height,
            start_y: p.y,
        });
    }

    let dragging = resp.dragged() || header_resp.dragged();
    if dragging && let Some(p) = pointer {
        let pt = s.timeline.t(p.x, left);
        let ph = s.playback.playhead;
        match &mut s.timeline.drag {
            Some(Drag::Scrub) => {
                let t = if s.settings.snapping {
                    snap_to(pt, &snap_cands(&BTreeSet::new()), snap_thr).0
                } else {
                    pt
                };
                s.playback.set_playhead(tb.snap_floor(t));
                s.playback.pause();
            }
            Some(Drag::Move {
                anchor,
                anchor_row,
                clips,
                delta,
                row_delta,
            }) => {
                let mut d = pt - *anchor;
                let first = clips.iter().map(|c| c.start).min().unwrap_or_default();
                if first + d < Ticks::ZERO {
                    d = -first;
                }
                if s.settings.snapping {
                    let ex: BTreeSet<ClipId> = clips.iter().map(|c| c.id).collect();
                    let cands = snap_cands(&ex);
                    let mut best: Option<(i64, Ticks)> = None;
                    for c in clips.iter() {
                        for edge in [c.start + d, c.end() + d] {
                            let (sn, hit) = snap_to(edge, &cands, snap_thr);
                            if hit {
                                let adj = sn - edge;
                                if best.is_none_or(|(b, _)| adj.0.abs() < b) {
                                    best = Some((adj.0.abs(), adj));
                                }
                            }
                        }
                    }
                    if let Some((_, adj)) = best {
                        d += adj;
                    }
                }
                *delta = tb.snap(first + d) - first;
                *row_delta = row_at(p.y)
                    .map(|r| r as i32 - *anchor_row as i32)
                    .unwrap_or(*row_delta);
                let _ = ph;
            }
            Some(Drag::Trim {
                clip,
                left: is_left,
                t,
            }) => {
                let mut nt = pt;
                if s.settings.snapping {
                    let ex = BTreeSet::from([clip.id]);
                    nt = snap_to(nt, &snap_cands(&ex), snap_thr).0;
                }
                let nt = tb.snap(nt);
                let frame = tb.frame_duration();
                let src_len = snap
                    .media
                    .get(&clip.media_id().unwrap_or(MediaId([0; 32])))
                    .filter(|m| m.kind != MediaKind::Image)
                    .map(|m| m.duration);
                *t = if *is_left {
                    // Can't extend before source start.
                    let min = clip.start - (clip.in_tick - Ticks::ZERO);
                    nt.clamp(min.max(Ticks::ZERO), clip.end() - frame)
                } else {
                    let max = src_len
                        .map(|d| clip.start + (d - clip.in_tick))
                        .unwrap_or(Ticks(i64::MAX / 4));
                    nt.clamp(clip.start + frame, max)
                };
            }
            Some(Drag::ResizeTrack {
                track,
                start_h,
                start_y,
            }) => {
                let h = (*start_h + p.y - *start_y).clamp(28.0, 200.0);
                s.timeline.pending_height = Some((*track, h));
            }
            _ => {}
        }
    }

    // Presence ghost for collaborators.
    s.timeline.ghost = match &s.timeline.drag {
        Some(Drag::Move { clips, delta, .. }) if !clips.is_empty() => Some(DragGhost {
            clip_ids: clips.iter().map(|c| c.id.key()).collect(),
            track_id: clips[0].track_id.key(),
            start_tick: (clips[0].start + *delta).0,
        }),
        _ => None,
    };

    if resp.drag_stopped() || header_resp.drag_stopped() {
        match s.timeline.drag.take() {
            Some(Drag::Move {
                clips,
                delta,
                row_delta,
                anchor_row,
                ..
            }) if delta != Ticks::ZERO || row_delta != 0 => {
                let mut moves = Vec::new();
                for c in &clips {
                    let cur_row = rows
                        .iter()
                        .position(|r| r.track.id == c.track_id)
                        .unwrap_or(anchor_row);
                    let target =
                        (cur_row as i32 + row_delta).clamp(0, rows.len() as i32 - 1) as usize;
                    let src_kind = rows[cur_row].track.kind;
                    let tk = &rows[target].track;
                    let track = if tk.kind.is_visual() == src_kind.is_visual() && !tk.locked {
                        tk.id
                    } else {
                        c.track_id
                    };
                    moves.push((c.id, track, c.start + delta));
                }
                s.exec(&MoveClips { seq: seq.id, moves });
            }
            Some(Drag::Trim {
                clip,
                left: is_left,
                t,
            }) => {
                let cmd = if is_left {
                    let d = t - clip.start;
                    TrimClip {
                        seq: seq.id,
                        clip: clip.id,
                        start: t,
                        in_tick: clip.in_tick + d,
                        out_tick: clip.out_tick,
                    }
                } else {
                    TrimClip {
                        seq: seq.id,
                        clip: clip.id,
                        start: clip.start,
                        in_tick: clip.in_tick,
                        out_tick: clip.in_tick + (t - clip.start),
                    }
                };
                if t != if is_left { clip.start } else { clip.end() } {
                    s.exec(&cmd);
                }
            }
            Some(Drag::ResizeTrack { track, .. }) => {
                if let Some((_, h)) = s.timeline.pending_height.take() {
                    s.exec(&SetTrackProps {
                        seq: seq.id,
                        track,
                        patch: TrackPatch {
                            height: Some(h),
                            ..Default::default()
                        },
                    });
                }
            }
            Some(Drag::Marquee { start }) => {
                if let Some(p) = pointer {
                    let r = Rect::from_two_pos(start, p);
                    if !(modifiers.shift || modifiers.command) {
                        s.selection.clear();
                    }
                    for row in &rows {
                        for c in seq.clips_on(row.track.id) {
                            if clip_rect(c, row, &s.timeline).intersects(r) {
                                s.selection.insert(c.id);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let t = s.timeline.t(p.x, left);
        if ruler.contains(p) {
            s.seek(tb.snap_floor(t));
        } else if let Some((c, _, _)) = hit_clip(p, &s.timeline) {
            match s.timeline.tool {
                Tool::Blade => {
                    let at = tb.snap(t);
                    if at > c.start && at < c.end() {
                        s.exec(&SplitClip {
                            seq: seq.id,
                            clip: c.id,
                            at,
                            new_id: ClipId::new(),
                        });
                    }
                }
                Tool::Select => {
                    if modifiers.shift || modifiers.command {
                        if !s.selection.remove(&c.id) {
                            s.selection.insert(c.id);
                        }
                    } else {
                        s.selection = BTreeSet::from([c.id]);
                    }
                }
            }
        } else {
            s.selection.clear();
            s.seek(tb.snap_floor(t));
        }
    }

    // Media dropped from the media panel.
    let drop_target = rows
        .iter()
        .zip(0..)
        .find(|(r, _)| pointer.is_some_and(|p| p.y >= r.top && p.y < r.top + r.h))
        .map(|(r, _)| r.track.id);
    if let Some(payload) = resp.dnd_release_payload::<MediaDrag>()
        && let Some(p) = pointer
    {
        let t = s.timeline.t(p.x, left);
        let t = if s.settings.snapping {
            snap_to(t, &snap_cands(&BTreeSet::new()), snap_thr).0
        } else {
            t
        };
        if let Some(id) = place_media(s, payload.0, drop_target, t) {
            s.selection = BTreeSet::from([id]);
        }
    }

    // ------------------------------------------------------------ paint
    let painter = ui.painter_at(full);
    let vis = ui.visuals().clone();
    painter.rect_filled(full, 0.0, vis.extreme_bg_color);
    let tracks_painter = ui.painter_at(tracks_area);

    // Rows background + clips.
    let presence_sel: Vec<(Color32, Vec<String>)> = s
        .presence
        .peers()
        .map(|p| {
            (
                Color32::from_rgb(p.color[0], p.color[1], p.color[2]),
                p.selected_clips.clone(),
            )
        })
        .collect();
    for r in &rows {
        let row_rect = Rect::from_min_max(
            pos2(tracks_area.min.x, r.top),
            pos2(tracks_area.max.x, r.top + r.h),
        );
        tracks_painter.rect_filled(row_rect, 0.0, vis.faint_bg_color);
        if r.track.locked {
            tracks_painter.rect_filled(row_rect, 0.0, Color32::from_black_alpha(60));
        }
        let view = normalize_track(&seq, r.track.id);
        for c in seq.clips_on(r.track.id) {
            let mut rect = clip_rect(c, r, &s.timeline);
            // Live drag preview.
            match &s.timeline.drag {
                Some(Drag::Move {
                    clips,
                    delta,
                    row_delta,
                    ..
                }) if clips.iter().any(|m| m.id == c.id) => {
                    let ri = rows
                        .iter()
                        .position(|x| x.track.id == r.track.id)
                        .unwrap_or(0);
                    let ti = (ri as i32 + row_delta).clamp(0, rows.len() as i32 - 1) as usize;
                    let dy = rows[ti].top - r.top;
                    let dx = delta.as_secs_f64() as f32 * s.timeline.px_per_sec;
                    painter_ghost(&tracks_painter, rect.translate(vec2(dx, dy)));
                }
                Some(Drag::Trim {
                    clip,
                    left: is_left,
                    t,
                }) if clip.id == c.id => {
                    let x = s.timeline.x(*t, left);
                    if *is_left {
                        rect.min.x = x;
                    } else {
                        rect.max.x = x;
                    }
                }
                _ => {}
            }
            if rect.right() < tracks_area.left() || rect.left() > tracks_area.right() {
                continue;
            }
            let base = clip_color(&snap, c, r.track.kind);
            let fill = if c.enabled {
                base
            } else {
                base.gamma_multiply(0.4)
            };
            tracks_painter.rect_filled(rect, 3.0, fill);
            // Filmstrip / thumbnails for video clips.
            if r.track.kind.is_visual()
                && let Some(m) = c.media_id()
            {
                draw_thumbs(&tracks_painter, s, m, c, rect, &snap);
            }
            if r.track.kind == TrackKind::Audio {
                draw_wave_placeholder(&tracks_painter, rect);
            }
            // Name band.
            let band = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.min.y + 14.0));
            tracks_painter.rect_filled(band, 3.0, Color32::from_black_alpha(110));
            let name_p = ui.painter_at(band.intersect(tracks_area));
            name_p.text(
                band.min + vec2(4.0, 1.0),
                Align2::LEFT_TOP,
                &c.name,
                FontId::proportional(10.0),
                Color32::WHITE,
            );
            if c.speed.num != c.speed.den {
                name_p.text(
                    band.right_top() + vec2(-4.0, 1.0),
                    Align2::RIGHT_TOP,
                    format!("{:.2}×", c.speed.num as f64 / c.speed.den as f64),
                    FontId::proportional(10.0),
                    theme::ACCENT,
                );
            }
            if c.media_id()
                .is_some_and(|m| s.session.local_path(m).is_none())
            {
                name_p.text(
                    band.right_top() + vec2(-4.0, 1.0),
                    Align2::RIGHT_TOP,
                    "⚠ offline",
                    FontId::proportional(10.0),
                    Color32::YELLOW,
                );
            }
            // Selection outlines: mine, then collaborators'.
            if s.selection.contains(&c.id) {
                tracks_painter.rect_stroke(
                    rect,
                    3.0,
                    Stroke::new(2.0, theme::ACCENT),
                    egui::StrokeKind::Inside,
                );
            }
            let key = c.id.key();
            for (i, (col, sel)) in presence_sel.iter().enumerate() {
                if sel.contains(&key) {
                    tracks_painter.rect_stroke(
                        rect.expand(1.0 + i as f32 * 2.0),
                        3.0,
                        Stroke::new(1.5, *col),
                        egui::StrokeKind::Outside,
                    );
                }
            }
        }
        // Conflict stripes where clips overlap.
        for cf in &view.conflicts {
            let x0 = s.timeline.x(cf.start, left);
            let x1 = s.timeline.x(cf.end, left);
            let stripe = Rect::from_min_max(pos2(x0, r.top + 1.0), pos2(x1, r.top + 4.0));
            tracks_painter.rect_filled(stripe, 0.0, theme::CONFLICT);
            if ui.rect_contains_pointer(stripe.expand(3.0)) {
                egui::Tooltip::always_open(
                    ui.ctx().clone(),
                    ui.layer_id(),
                    egui::Id::new("conflict-tip"),
                    egui::PopupAnchor::Pointer,
                )
                .show(|ui| ui.label(s.t("tl-conflict")));
            }
        }
    }

    // Remote drag ghosts.
    for p in s.presence.peers() {
        if let Some(g) = &p.drag {
            let col = Color32::from_rgb(p.color[0], p.color[1], p.color[2]);
            if let Some(r) = rows.iter().find(|r| r.track.id.key() == g.track_id) {
                let len = g
                    .clip_ids
                    .first()
                    .and_then(|k| k.parse().ok())
                    .and_then(|id: ClipId| seq.clips.get(&id))
                    .map(|c| c.duration())
                    .unwrap_or(Ticks::ONE_SECOND);
                let x = s.timeline.x(Ticks(g.start_tick), left);
                let rect = Rect::from_min_size(
                    pos2(x, r.top + 2.0),
                    vec2(len.as_secs_f64() as f32 * s.timeline.px_per_sec, r.h - 4.0),
                );
                tracks_painter.rect_stroke(
                    rect,
                    3.0,
                    Stroke::new(1.5, col),
                    egui::StrokeKind::Inside,
                );
                tracks_painter.text(
                    rect.left_bottom() + vec2(3.0, -2.0),
                    Align2::LEFT_BOTTOM,
                    &p.name,
                    FontId::proportional(10.0),
                    col,
                );
            }
        }
    }

    // Media drag ghost.
    if let (Some(payload), Some(p)) = (resp.dnd_hover_payload::<MediaDrag>(), pointer)
        && let Some(e) = snap.media.get(&payload.0)
        && let Some(r) = rows.iter().find(|r| p.y >= r.top && p.y < r.top + r.h)
    {
        let dur = if e.kind == MediaKind::Image {
            Ticks::from_secs(5)
        } else {
            e.duration
        };
        let x = p.x;
        let rect = Rect::from_min_size(
            pos2(x, r.top + 1.0),
            vec2(dur.as_secs_f64() as f32 * s.timeline.px_per_sec, r.h - 2.0),
        );
        painter_ghost(&tracks_painter, rect);
    }

    // Marquee.
    if let (Some(Drag::Marquee { start }), Some(p)) = (&s.timeline.drag, pointer) {
        let r = Rect::from_two_pos(*start, p);
        painter.rect_filled(r, 0.0, theme::ACCENT.gamma_multiply(0.12));
        painter.rect_stroke(
            r,
            0.0,
            Stroke::new(1.0, theme::ACCENT),
            egui::StrokeKind::Inside,
        );
    }

    // In/out shading across tracks.
    if let (Some(i), Some(o)) = (s.mark_in, s.mark_out)
        && o > i
    {
        let r = Rect::from_min_max(
            pos2(s.timeline.x(i, left), ruler.min.y),
            pos2(s.timeline.x(o, left), tracks_area.max.y),
        );
        painter.rect_filled(r.intersect(content), 0.0, Color32::from_white_alpha(10));
    }

    // Ruler.
    let rp = ui.painter_at(ruler);
    rp.rect_filled(ruler, 0.0, vis.panel_fill);
    let (major, minor) = ruler_step(s.timeline.px_per_sec, tb);
    let t0 = (s.timeline.scroll_x / s.timeline.px_per_sec) as f64;
    let t1 = t0 + (content.width() / s.timeline.px_per_sec) as f64;
    let mut m = (t0 / minor).floor() * minor;
    while m <= t1 {
        let x = left + (m as f32) * s.timeline.px_per_sec - s.timeline.scroll_x;
        let is_major = ((m / major).round() * major - m).abs() < minor * 0.01;
        let h = if is_major { 10.0 } else { 4.0 };
        rp.line_segment(
            [pos2(x, ruler.max.y - h), pos2(x, ruler.max.y)],
            Stroke::new(1.0, vis.weak_text_color()),
        );
        if is_major {
            let tc = Timecode::from_ticks(Ticks::from_secs_f64(m), tb, tb.supports_drop_frame());
            rp.text(
                pos2(x + 3.0, ruler.min.y + 2.0),
                Align2::LEFT_TOP,
                tc.to_string(),
                FontId::monospace(9.0),
                vis.weak_text_color(),
            );
        }
        m += minor;
    }
    if let (Some(i), Some(o)) = (s.mark_in, s.mark_out) {
        let r = Rect::from_min_max(
            pos2(s.timeline.x(i, left), ruler.max.y - 5.0),
            pos2(s.timeline.x(o, left), ruler.max.y),
        );
        rp.rect_filled(r, 0.0, theme::ACCENT.gamma_multiply(0.6));
    }
    for (t, label) in [(s.mark_in, "{"), (s.mark_out, "}")] {
        if let Some(t) = t {
            rp.text(
                pos2(s.timeline.x(t, left), ruler.max.y - 2.0),
                Align2::CENTER_BOTTOM,
                label,
                FontId::monospace(11.0),
                theme::ACCENT,
            );
        }
    }
    // Markers.
    for mk in seq.markers.values() {
        let x = s.timeline.x(mk.tick, left);
        let col = theme::MARKERS[mk.color.index()];
        let pts = vec![
            pos2(x - 5.0, ruler.min.y + 12.0),
            pos2(x + 5.0, ruler.min.y + 12.0),
            pos2(x + 5.0, ruler.min.y + 18.0),
            pos2(x, ruler.min.y + 23.0),
            pos2(x - 5.0, ruler.min.y + 18.0),
        ];
        rp.add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
        let hit = Rect::from_center_size(pos2(x, ruler.min.y + 17.0), vec2(12.0, 12.0));
        if ui.rect_contains_pointer(hit) {
            let text = if mk.name.is_empty() && mk.comment.is_empty() {
                format!("Marker · {}", mk.author)
            } else {
                format!("{}\n{}\n— {}", mk.name, mk.comment, mk.author)
            };
            egui::Tooltip::always_open(
                ui.ctx().clone(),
                ui.layer_id(),
                egui::Id::new(("marker-tip", mk.id)),
                egui::PopupAnchor::Pointer,
            )
            .show(|ui| ui.label(text));
            if ui.input(|i| {
                i.pointer
                    .button_double_clicked(egui::PointerButton::Primary)
            }) {
                s.dialogs.edit_marker = Some((mk.id, mk.name.clone(), mk.comment.clone()));
            }
        }
    }

    // Remote playheads.
    for p in s.presence.peers() {
        let col = Color32::from_rgb(p.color[0], p.color[1], p.color[2]);
        let x = s.timeline.x(Ticks(p.playhead_tick), left);
        if x < content.min.x || x > content.max.x {
            continue;
        }
        painter.line_segment(
            [pos2(x, ruler.min.y), pos2(x, tracks_area.max.y)],
            Stroke::new(1.0, col),
        );
        let flag = Rect::from_min_size(
            pos2(x, ruler.min.y),
            vec2(8.0 + p.name.len() as f32 * 6.0, 12.0),
        );
        painter.rect_filled(flag, 2.0, col);
        painter.text(
            flag.left_center() + vec2(3.0, 0.0),
            Align2::LEFT_CENTER,
            &p.name,
            FontId::proportional(9.0),
            Color32::BLACK,
        );
    }

    // Playhead.
    let px = s.timeline.x(s.playback.playhead, left);
    if px >= content.min.x && px <= content.max.x {
        painter.line_segment(
            [pos2(px, ruler.min.y), pos2(px, tracks_area.max.y)],
            Stroke::new(1.5, theme::PLAYHEAD),
        );
        let tri = vec![
            pos2(px - 6.0, ruler.min.y),
            pos2(px + 6.0, ruler.min.y),
            pos2(px, ruler.min.y + 8.0),
        ];
        painter.add(egui::Shape::convex_polygon(
            tri,
            theme::PLAYHEAD,
            Stroke::NONE,
        ));
    }

    // Track headers.
    headers(ui, s, &seq, &rows, header, ruler.height());
}

fn painter_ghost(p: &egui::Painter, rect: Rect) {
    p.rect_filled(rect, 3.0, theme::ACCENT.gamma_multiply(0.25));
    p.rect_stroke(
        rect,
        3.0,
        Stroke::new(1.0, theme::ACCENT),
        egui::StrokeKind::Inside,
    );
}

fn draw_thumbs(
    p: &egui::Painter,
    s: &AppState,
    m: MediaId,
    c: &Clip,
    rect: Rect,
    snap: &DocSnapshot,
) {
    let strip = s.textures.get(&(m, true));
    let thumb = s.textures.get(&(m, false));
    let Some(tex) = strip.or(thumb) else { return };
    let inner = Rect::from_min_max(pos2(rect.min.x, rect.min.y + 14.0), rect.max);
    if inner.height() < 8.0 {
        return;
    }
    let tw = inner.height() * 16.0 / 9.0;
    let dur = snap
        .media
        .get(&m)
        .map(|e| e.duration)
        .unwrap_or(Ticks::ONE_SECOND)
        .0
        .max(1) as f64;
    let n = cutroom_media::thumbs::STRIP_FRAMES as f32;
    let clip = p.clip_rect().intersect(inner);
    let p = p.with_clip_rect(clip);
    let mut x = inner.min.x;
    while x < inner.max.x {
        if x + tw >= clip.min.x && x <= clip.max.x {
            let r = Rect::from_min_size(pos2(x, inner.min.y), vec2(tw, inner.height()));
            let uv = if strip.is_some() {
                let frac = (x - rect.min.x) / rect.width().max(1.0);
                let src = c.in_tick.0 as f64 + frac as f64 * (c.out_tick - c.in_tick).0 as f64;
                let slot = ((src / dur) as f32 * n).floor().clamp(0.0, n - 1.0);
                Rect::from_min_max(pos2(slot / n, 0.0), pos2((slot + 1.0) / n, 1.0))
            } else {
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
            };
            p.image(tex.id(), r, uv, Color32::from_white_alpha(200));
        }
        x += tw;
    }
}

fn draw_wave_placeholder(p: &egui::Painter, rect: Rect) {
    // Real `.peaks` waveforms arrive with the MVP; draw a neutral baseline.
    let y = rect.center().y + 6.0;
    p.line_segment(
        [pos2(rect.min.x + 2.0, y), pos2(rect.max.x - 2.0, y)],
        Stroke::new(1.0, Color32::from_white_alpha(60)),
    );
}

fn headers(
    ui: &mut egui::Ui,
    s: &mut AppState,
    seq: &Sequence,
    rows: &[Row],
    header: Rect,
    ruler_h: f32,
) {
    let painter = ui.painter_at(header);
    let vis = ui.visuals().clone();
    painter.rect_filled(header, 0.0, vis.panel_fill);
    let area = Rect::from_min_max(pos2(header.min.x, header.min.y + ruler_h), header.max);
    let mut patch: Option<(TrackId, TrackPatch)> = None;
    let mut v = 0;
    let mut a = 0;
    let n_visual = rows.iter().filter(|r| r.track.kind.is_visual()).count();
    for r in rows {
        let rect = Rect::from_min_max(
            pos2(header.min.x, r.top),
            pos2(header.max.x - 1.0, r.top + r.h),
        );
        if !rect.intersects(area) {
            continue;
        }
        let rp = ui.painter_at(rect.intersect(area));
        rp.rect_filled(rect, 0.0, vis.faint_bg_color);
        let (icon, label) = match r.track.kind {
            TrackKind::Audio => {
                a += 1;
                ("🔊", format!("A{a}"))
            }
            TrackKind::Title => {
                v += 1;
                ("T", format!("V{}", n_visual - v + 1))
            }
            TrackKind::Video => {
                v += 1;
                ("🎞", format!("V{}", n_visual - v + 1))
            }
        };
        rp.text(
            rect.left_center() + vec2(6.0, 0.0),
            Align2::LEFT_CENTER,
            format!("{icon} {label}"),
            FontId::proportional(11.0),
            vis.text_color(),
        );
        rp.text(
            rect.left_center() + vec2(46.0, 0.0),
            Align2::LEFT_CENTER,
            &r.track.name,
            FontId::proportional(10.0),
            vis.weak_text_color(),
        );
        // Toggles.
        let mut x = rect.right() - 20.0;
        let lock_icon = if r.track.locked { "🔒" } else { "🔓" };
        let (vis_icon, vis_on) = match r.track.kind {
            TrackKind::Audio => (if r.track.muted { "🔇" } else { "🔈" }, !r.track.muted),
            _ => (if r.track.hidden { "◌" } else { "👁" }, !r.track.hidden),
        };
        for (i, ic) in [vis_icon, lock_icon].iter().enumerate() {
            let br = Rect::from_center_size(pos2(x, rect.center().y), vec2(18.0, 18.0));
            let resp = ui.interact(
                br.intersect(area),
                ui.id().with(("hdr", r.track.id, i)),
                Sense::click(),
            );
            let col = if resp.hovered() {
                theme::ACCENT
            } else {
                vis.text_color()
            };
            rp.text(
                br.center(),
                Align2::CENTER_CENTER,
                *ic,
                FontId::proportional(12.0),
                col,
            );
            if resp.clicked() {
                let mut p = TrackPatch::default();
                if i == 0 {
                    match r.track.kind {
                        TrackKind::Audio => p.muted = Some(vis_on),
                        _ => p.hidden = Some(vis_on),
                    }
                } else {
                    p.locked = Some(!r.track.locked);
                }
                patch = Some((r.track.id, p));
            }
            x -= 22.0;
        }
        // Resize handle at the bottom edge.
        let edge = Rect::from_min_max(
            pos2(rect.min.x, rect.max.y - 3.0),
            pos2(rect.max.x, rect.max.y + 2.0),
        );
        if ui.rect_contains_pointer(edge) {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
        }
    }
    if let Some((track, p)) = patch {
        s.exec(&SetTrackProps {
            seq: seq.id,
            track,
            patch: p,
        });
    }
}
