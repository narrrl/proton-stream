//! The look: one dark palette, applied once at startup.
//!
//! Deliberately dark and low-contrast around the edges — this is a window full
//! of video stills, and every bit of chrome brightness competes with them. The
//! accent is Proton's purple, which is also the only strong colour in the app,
//! so anything wearing it is something to click.

use egui::{Color32, CornerRadius, Stroke, Visuals};

pub const BACKGROUND: Color32 = Color32::from_rgb(0x0e, 0x0e, 0x12);
pub const SURFACE: Color32 = Color32::from_rgb(0x17, 0x17, 0x1d);
pub const CARD: Color32 = Color32::from_rgb(0x1e, 0x1e, 0x26);
pub const CARD_HOVER: Color32 = Color32::from_rgb(0x2a, 0x2a, 0x35);
pub const TEXT: Color32 = Color32::from_rgb(0xea, 0xea, 0xf0);
pub const MUTED: Color32 = Color32::from_rgb(0x8e, 0x8e, 0x9c);
pub const ACCENT: Color32 = Color32::from_rgb(0x7d, 0x4d, 0xff);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(0x53, 0x33, 0xad);
pub const DANGER: Color32 = Color32::from_rgb(0xe0, 0x55, 0x61);

/// Card geometry. The grid is built from these, so a change here moves
/// everything together.
pub const CARD_WIDTH: f32 = 232.0;
/// Video stills are 16:9. A poster shape would letterbox every one of them.
pub const CARD_ASPECT: f32 = 9.0 / 16.0;
pub const CARD_GAP: f32 = 14.0;

pub fn apply(ctx: &egui::Context) {
    let mut visuals = Visuals::dark();

    visuals.panel_fill = BACKGROUND;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = Color32::from_rgb(0x0a, 0x0a, 0x0d);
    visuals.faint_bg_color = SURFACE;
    visuals.override_text_color = Some(TEXT);
    visuals.selection.bg_fill = ACCENT_DIM;
    visuals.selection.stroke = Stroke::new(1.0, TEXT);
    visuals.hyperlink_color = ACCENT;

    let radius = CornerRadius::same(8);
    visuals.widgets.noninteractive.bg_fill = SURFACE;
    visuals.widgets.noninteractive.corner_radius = radius;
    visuals.widgets.noninteractive.bg_stroke =
        Stroke::new(1.0, Color32::from_rgb(0x26, 0x26, 0x30));

    visuals.widgets.inactive.bg_fill = CARD;
    visuals.widgets.inactive.weak_bg_fill = CARD;
    visuals.widgets.inactive.corner_radius = radius;
    visuals.widgets.inactive.bg_stroke = Stroke::NONE;

    visuals.widgets.hovered.bg_fill = CARD_HOVER;
    visuals.widgets.hovered.weak_bg_fill = CARD_HOVER;
    visuals.widgets.hovered.corner_radius = radius;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);

    visuals.widgets.active.bg_fill = ACCENT_DIM;
    visuals.widgets.active.weak_bg_fill = ACCENT_DIM;
    visuals.widgets.active.corner_radius = radius;

    ctx.set_visuals(visuals);

    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(10.0, 10.0);
        style.spacing.button_padding = egui::vec2(12.0, 7.0);
        style.spacing.interact_size.y = 30.0;
        style.spacing.scroll.bar_width = 10.0;

        use egui::FontFamily::Proportional;
        use egui::TextStyle::{Body, Button, Heading, Monospace, Small};
        style.text_styles = [
            (Heading, egui::FontId::new(26.0, Proportional)),
            (Body, egui::FontId::new(14.0, Proportional)),
            (Button, egui::FontId::new(14.0, Proportional)),
            (Small, egui::FontId::new(11.5, Proportional)),
            (
                Monospace,
                egui::FontId::new(12.5, egui::FontFamily::Monospace),
            ),
        ]
        .into();
    });
}
