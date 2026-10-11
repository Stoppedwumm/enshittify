//! Visual theme. Dark is the default; the accent is a Cutroom mint.

use egui::{Color32, CornerRadius, Stroke, Visuals};
use serde::{Deserialize, Serialize};

pub const ACCENT: Color32 = Color32::from_rgb(0x2E, 0xD3, 0xA8);
pub const ACCENT_DARK: Color32 = Color32::from_rgb(0x1F, 0x9E, 0x7E);
pub const PLAYHEAD: Color32 = Color32::from_rgb(0xFF, 0x4D, 0x4D);
pub const CONFLICT: Color32 = Color32::from_rgb(0xE5, 0x3E, 0x3E);
pub const CLIP_VIDEO: Color32 = Color32::from_rgb(0x2D, 0x5B, 0x8C);
pub const CLIP_AUDIO: Color32 = Color32::from_rgb(0x2B, 0x7A, 0x55);
pub const CLIP_TITLE: Color32 = Color32::from_rgb(0x7A, 0x4B, 0xA8);
pub const CLIP_GENERATOR: Color32 = Color32::from_rgb(0x8A, 0x6D, 0x2B);

/// Marker palette, indexed like `MarkerColor`.
pub const MARKERS: [Color32; 8] = [
    Color32::from_rgb(0x3D, 0x8B, 0xFD),
    Color32::from_rgb(0xF0, 0x4A, 0x4A),
    Color32::from_rgb(0xF5, 0x8D, 0x2B),
    Color32::from_rgb(0xF2, 0xC9, 0x2E),
    Color32::from_rgb(0x46, 0xC3, 0x5E),
    Color32::from_rgb(0x2E, 0xC4, 0xD6),
    Color32::from_rgb(0x9B, 0x5D, 0xE5),
    Color32::from_rgb(0xE5, 0x5D, 0xB4),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

pub fn apply(ctx: &egui::Context, theme: Theme) {
    let mut v = match theme {
        Theme::Dark => {
            let mut v = Visuals::dark();
            v.panel_fill = Color32::from_rgb(0x1B, 0x1D, 0x22);
            v.window_fill = Color32::from_rgb(0x22, 0x25, 0x2B);
            v.extreme_bg_color = Color32::from_rgb(0x12, 0x13, 0x16);
            v.faint_bg_color = Color32::from_rgb(0x24, 0x27, 0x2E);
            v
        }
        Theme::Light => Visuals::light(),
    };
    v.selection.bg_fill = ACCENT_DARK;
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(4);
    }
    v.window_corner_radius = CornerRadius::same(8);
    let et = match theme {
        Theme::Dark => egui::Theme::Dark,
        Theme::Light => egui::Theme::Light,
    };
    ctx.set_theme(et);
    ctx.set_visuals_of(et, v);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(8.0, 3.0);
    });
}

/// A deterministic colour per collaborator.
pub fn peer_color(peer: u64) -> Color32 {
    const PALETTE: [Color32; 8] = [
        Color32::from_rgb(0xFF, 0x8A, 0x3D),
        Color32::from_rgb(0x5B, 0x9C, 0xFF),
        Color32::from_rgb(0xE0, 0x5B, 0xFF),
        Color32::from_rgb(0xFF, 0xD1, 0x3D),
        Color32::from_rgb(0x3D, 0xE0, 0xFF),
        Color32::from_rgb(0xFF, 0x5B, 0x8C),
        Color32::from_rgb(0x9C, 0xFF, 0x5B),
        Color32::from_rgb(0xB4, 0x8C, 0xFF),
    ];
    PALETTE[(peer.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 61) as usize]
}
