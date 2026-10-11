//! Media panel: asset tabs, folder tree, thumbnail grid with hover-scrub.

use std::collections::HashSet;

use cutroom_core::model::MediaKind;
use cutroom_core::{MediaId, Ticks};
use egui::{Color32, RichText, Sense, Vec2};

use crate::app::AppState;
use crate::session::ItemStatus;
use crate::shortcuts::Action;
use crate::theme;

/// Drag-and-drop payload from the media grid to the timeline.
#[derive(Clone, Copy, Debug)]
pub struct MediaDrag(pub MediaId);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AssetTab {
    #[default]
    Media,
    Stock,
    Audio,
    Titles,
    Transitions,
    Effects,
    Stickers,
    Templates,
}

impl AssetTab {
    const ALL: [(AssetTab, &'static str, &'static str); 8] = [
        (AssetTab::Media, "🎞", "tab-media"),
        (AssetTab::Stock, "🛒", "tab-stock"),
        (AssetTab::Audio, "🎵", "tab-audio"),
        (AssetTab::Titles, "🔤", "tab-titles"),
        (AssetTab::Transitions, "⧉", "tab-transitions"),
        (AssetTab::Effects, "✨", "tab-effects"),
        (AssetTab::Stickers, "⭐", "tab-stickers"),
        (AssetTab::Templates, "▦", "tab-templates"),
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    Video,
    Audio,
    Image,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Sort {
    #[default]
    Added,
    Name,
    Duration,
}

pub struct MediaUi {
    pub tab: AssetTab,
    pub search: String,
    pub filter: Filter,
    pub sort: Sort,
    pub tile: f32,
    failed_textures: HashSet<(MediaId, bool)>,
}

impl Default for MediaUi {
    fn default() -> Self {
        MediaUi {
            tab: AssetTab::Media,
            search: String::new(),
            filter: Filter::All,
            sort: Sort::Added,
            tile: 132.0,
            failed_textures: HashSet::new(),
        }
    }
}

/// Uploads freshly generated thumbnails/filmstrips as egui textures.
pub fn load_textures(ctx: &egui::Context, s: &mut AppState) {
    let mut todo = Vec::new();
    for it in s.session.library.values() {
        for (strip, p) in [(false, &it.thumb), (true, &it.strip)] {
            if let Some(p) = p
                && !s.textures.contains_key(&(it.id, strip))
                && !s.media_ui.failed_textures.contains(&(it.id, strip))
            {
                todo.push((it.id, strip, p.clone()));
            }
        }
    }
    let max_side = ctx.input(|i| i.max_texture_side) as u32;
    for (id, strip, p) in todo.into_iter().take(4) {
        match image::open(&p) {
            Ok(img) => {
                let mut img = img.to_rgba8();
                if img.width() > max_side || img.height() > max_side {
                    let f = max_side as f32 / img.width().max(img.height()) as f32;
                    img = image::imageops::resize(
                        &img,
                        ((img.width() as f32 * f) as u32).max(1),
                        ((img.height() as f32 * f) as u32).max(1),
                        image::imageops::FilterType::Triangle,
                    );
                }
                let ci = egui::ColorImage::from_rgba_unmultiplied(
                    [img.width() as usize, img.height() as usize],
                    img.as_raw(),
                );
                let tex = ctx.load_texture(
                    format!("thumb-{}-{strip}", id.short()),
                    ci,
                    egui::TextureOptions::LINEAR,
                );
                s.textures.insert((id, strip), tex);
            }
            Err(_) => {
                s.media_ui.failed_textures.insert((id, strip));
            }
        }
    }
}

fn fmt_dur(t: Ticks) -> String {
    let s = t.as_secs_f64().round() as i64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

pub fn ui(ui: &mut egui::Ui, s: &mut AppState) {
    // Asset tabs.
    ui.horizontal(|ui| {
        for (tab, icon, key) in AssetTab::ALL {
            let sel = s.media_ui.tab == tab;
            let r = ui
                .selectable_label(sel, RichText::new(icon).size(16.0))
                .on_hover_text(s.t(key));
            if r.clicked() {
                s.media_ui.tab = tab;
            }
        }
    });
    ui.separator();
    if s.media_ui.tab != AssetTab::Media {
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new(s.t("coming-soon")).weak());
        });
        return;
    }

    // Toolbar: Import, search, filter, sort.
    ui.horizontal(|ui| {
        if ui
            .button(format!("＋ {}", s.t("import-button")))
            .on_hover_text(s.shortcuts.hint(Action::ImportMedia))
            .clicked()
        {
            s.import_dialog();
        }
        let hint = s.t("search-hint");
        ui.add(
            egui::TextEdit::singleline(&mut s.media_ui.search)
                .hint_text(hint)
                .desired_width(140.0),
        );
        let f = &mut s.media_ui.filter;
        egui::ComboBox::from_id_salt("media-filter")
            .width(70.0)
            .selected_text(s.i18n.t(match f {
                Filter::All => "filter-all",
                Filter::Video => "filter-video",
                Filter::Audio => "filter-audio",
                Filter::Image => "filter-image",
            }))
            .show_ui(ui, |ui| {
                for (v, k) in [
                    (Filter::All, "filter-all"),
                    (Filter::Video, "filter-video"),
                    (Filter::Audio, "filter-audio"),
                    (Filter::Image, "filter-image"),
                ] {
                    ui.selectable_value(f, v, s.i18n.t(k));
                }
            });
        let so = &mut s.media_ui.sort;
        egui::ComboBox::from_id_salt("media-sort")
            .width(80.0)
            .selected_text(s.i18n.t(match so {
                Sort::Added => "sort-added",
                Sort::Name => "sort-name",
                Sort::Duration => "sort-duration",
            }))
            .show_ui(ui, |ui| {
                for (v, k) in [
                    (Sort::Added, "sort-added"),
                    (Sort::Name, "sort-name"),
                    (Sort::Duration, "sort-duration"),
                ] {
                    ui.selectable_value(so, v, s.i18n.t(k));
                }
            });
    });

    let avail = ui.available_size();
    ui.horizontal_top(|ui| {
        // Folder tree (bins arrive with the MVP; one root folder for now).
        ui.allocate_ui(Vec2::new(110.0, avail.y), |ui| {
            ui.vertical(|ui| {
                let _ = ui.selectable_label(true, format!("📁 {}", s.t("folder-project-media")));
            });
        });
        ui.separator();
        ui.vertical(|ui| grid(ui, s));
    });
}

fn grid(ui: &mut egui::Ui, s: &mut AppState) {
    let snap = s.session.snapshot();
    let q = s.media_ui.search.to_lowercase();
    let mut items: Vec<_> = s
        .session
        .library
        .values()
        .filter_map(|it| snap.media.get(&it.id).map(|e| (it.clone(), e.clone())))
        .filter(|(_, e)| q.is_empty() || e.name.to_lowercase().contains(&q))
        .filter(|(_, e)| match s.media_ui.filter {
            Filter::All => true,
            Filter::Video => e.kind == MediaKind::Video,
            Filter::Audio => e.kind == MediaKind::Audio,
            Filter::Image => e.kind == MediaKind::Image,
        })
        .collect();
    match s.media_ui.sort {
        Sort::Added => items.sort_by_key(|(it, _)| it.order),
        Sort::Name => items.sort_by_key(|(_, e)| e.name.to_lowercase()),
        Sort::Duration => items.sort_by_key(|(_, e)| std::cmp::Reverse(e.duration)),
    }

    if items.is_empty() && s.session.pending.is_empty() {
        ui.add_space(24.0);
        let hint = s.shortcuts.hint(Action::ImportMedia);
        ui.label(RichText::new(s.i18n.ta("media-empty", &[("shortcut", &hint)])).weak());
        return;
    }

    let tile = s.media_ui.tile;
    let cols = ((ui.available_width() / (tile + 8.0)).floor() as usize).max(1);
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            egui::Grid::new("media-grid")
                .spacing([8.0, 8.0])
                .show(ui, |ui| {
                    for (n, (it, e)) in items.iter().enumerate() {
                        tile_ui(ui, s, it, e, tile);
                        if (n + 1) % cols == 0 {
                            ui.end_row();
                        }
                    }
                    for p in s.session.pending.clone() {
                        ui.vertical(|ui| {
                            let (r, _) = ui.allocate_exact_size(
                                Vec2::new(tile, tile * 9.0 / 16.0),
                                Sense::hover(),
                            );
                            ui.painter()
                                .rect_filled(r, 4.0, ui.visuals().extreme_bg_color);
                            ui.put(r, egui::Spinner::new());
                            ui.label(
                                RichText::new(
                                    p.file_name()
                                        .map(|f| f.to_string_lossy().into_owned())
                                        .unwrap_or_default(),
                                )
                                .small(),
                            );
                        });
                    }
                });
        });
}

fn tile_ui(
    ui: &mut egui::Ui,
    s: &mut AppState,
    it: &crate::session::LibraryItem,
    e: &cutroom_core::model::MediaEntry,
    tile: f32,
) {
    let id = it.id;
    let size = Vec2::new(tile, tile * 9.0 / 16.0);
    ui.vertical(|ui| {
        ui.set_width(tile);
        let drag_id = egui::Id::new(("media-tile", id));
        let resp = ui
            .dnd_drag_source(drag_id, MediaDrag(id), |ui| {
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                let p = ui.painter_at(rect);
                p.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
                let hover = ui.rect_contains_pointer(rect);
                // Hover-scrub through the filmstrip, otherwise the poster frame.
                let strip = s.textures.get(&(id, true));
                if let (true, Some(strip)) = (hover, strip) {
                    let n = cutroom_media::thumbs::STRIP_FRAMES as f32;
                    let x = ui
                        .ctx()
                        .pointer_hover_pos()
                        .map(|p| ((p.x - rect.left()) / rect.width()).clamp(0.0, 0.999))
                        .unwrap_or(0.0);
                    let slot = (x * n).floor();
                    let uv = egui::Rect::from_min_max(
                        egui::pos2(slot / n, 0.0),
                        egui::pos2((slot + 1.0) / n, 1.0),
                    );
                    p.image(strip.id(), rect, uv, Color32::WHITE);
                    // Scrub position indicator.
                    let xx = rect.left() + x * rect.width();
                    p.line_segment(
                        [
                            egui::pos2(xx, rect.bottom() - 4.0),
                            egui::pos2(xx, rect.bottom()),
                        ],
                        egui::Stroke::new(2.0, theme::ACCENT),
                    );
                } else if let Some(t) = s.textures.get(&(id, false)) {
                    p.image(
                        t.id(),
                        rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                } else {
                    let icon = match e.kind {
                        MediaKind::Audio => "🎵",
                        MediaKind::Image => "🖼",
                        MediaKind::Video => "🎞",
                    };
                    p.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        icon,
                        egui::FontId::proportional(28.0),
                        Color32::GRAY,
                    );
                }
                // Duration badge.
                if e.kind != MediaKind::Image {
                    let txt = fmt_dur(e.duration);
                    let galley =
                        p.layout_no_wrap(txt, egui::FontId::monospace(10.0), Color32::WHITE);
                    let br = egui::Rect::from_min_size(
                        rect.right_bottom() - galley.size() - Vec2::new(6.0, 4.0),
                        galley.size() + Vec2::new(4.0, 2.0),
                    );
                    p.rect_filled(br, 2.0, Color32::from_black_alpha(170));
                    p.galley(br.min + Vec2::new(2.0, 1.0), galley, Color32::WHITE);
                }
                // Status badges.
                let badge = match &it.status {
                    ItemStatus::Offline => Some((s.t("media-offline"), theme::CONFLICT)),
                    ItemStatus::Importing => Some(("…".to_string(), Color32::GRAY)),
                    ItemStatus::Failed(_) => Some(("!".to_string(), theme::CONFLICT)),
                    ItemStatus::Ready => None,
                };
                if let Some((txt, c)) = badge {
                    let r = egui::Rect::from_min_size(
                        rect.min + Vec2::new(4.0, 4.0),
                        Vec2::new(58.0, 14.0),
                    );
                    p.rect_filled(r, 2.0, c);
                    p.text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        txt,
                        egui::FontId::proportional(10.0),
                        Color32::WHITE,
                    );
                }
                if s.selected_media == Some(id) {
                    p.rect_stroke(
                        rect,
                        4.0,
                        egui::Stroke::new(2.0, theme::ACCENT),
                        egui::StrokeKind::Inside,
                    );
                }
            })
            .response;
        let resp = resp.interact(Sense::click());
        if resp.clicked() || resp.drag_started() {
            s.selected_media = Some(id);
            s.focus = crate::shortcuts::Scope::Media;
        }
        if resp.double_clicked() {
            s.source_tick = Ticks::ZERO;
        }
        resp.context_menu(|ui| {
            if ui.button("Rename").clicked() {
                s.dialogs.rename_media = Some((id, e.name.clone()));
                ui.close();
            }
            if let Some(p) = &it.path
                && ui.button("Reveal in folder").clicked()
            {
                s.selected_media = Some(id);
                let _ = p;
                s.run_action(Action::RevealInFolder, ui.ctx());
                ui.close();
            }
            if ui.button("Relink…").clicked() {
                if let Some(f) = rfd::FileDialog::new().pick_file() {
                    s.session.relink(id, f, &s.backend);
                }
                ui.close();
            }
            if ui.button("Remove from project").clicked() {
                s.exec(&cutroom_core::command::RemoveMedia(id));
                ui.close();
            }
        });
        let name = egui::Label::new(RichText::new(&e.name).small()).truncate();
        ui.add(name).on_hover_text(match &it.status {
            ItemStatus::Failed(err) => err.clone(),
            _ => e.name.clone(),
        });
    });
}
