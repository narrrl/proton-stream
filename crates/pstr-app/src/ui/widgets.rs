//! Controls egui does not have, or has in a shape that does not fit.
//!
//! Each is painted by hand for the same reason [`super::accent_button`] is: an
//! egui widget takes one flat colour, and what makes these readable is either a
//! gradient or a state drawn as a shape rather than as a fill.

use egui::{Color32, CornerRadius, Sense, Stroke, Vec2};

use crate::theme;

/// An on/off switch with its label beside it, for settings.
///
/// Replaces egui's checkbox, whose unchecked box is drawn in the card colour —
/// on a card, which is where every setting lives, an unchecked box was not
/// there at all. A switch says "off" with a shape, not with a fill that has to
/// stand out from what is behind it.
pub fn toggle(ui: &mut egui::Ui, on: &mut bool, text: &str) -> egui::Response {
    const TRACK: Vec2 = Vec2::new(34.0, 20.0);

    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        theme::Role::Body.font(),
        Color32::PLACEHOLDER,
    );
    let gap = if text.is_empty() { 0.0 } else { 10.0 };
    let size = Vec2::new(
        TRACK.x + gap + galley.size().x,
        TRACK
            .y
            .max(galley.size().y)
            .max(ui.spacing().interact_size.y),
    );
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, *on, text));

    if ui.is_rect_visible(rect) {
        let ctx = ui.ctx();
        let t = ctx.animate_bool_with_time(response.id, *on, theme::motion::STATE);
        let hover = ctx.animate_bool_with_time(
            response.id.with("hover"),
            response.hovered(),
            theme::motion::HOVER,
        );
        let track = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.center().y - TRACK.y / 2.0),
            TRACK,
        );
        let radius = CornerRadius::same((TRACK.y / 2.0) as u8);
        let painter = ui.painter();

        // Off is an outlined track; on is the accent filling it. Both are drawn
        // through the switch's travel, so the fill arrives with the knob.
        painter.rect_filled(track, radius, theme::card_hover());
        painter.rect_stroke(
            track,
            radius,
            Stroke::new(1.0, theme::muted().gamma_multiply(0.6 + 0.4 * hover)),
            egui::StrokeKind::Inside,
        );
        if t > 0.0 {
            let mut lit = track;
            lit.set_width(TRACK.y + (TRACK.x - TRACK.y) * t);
            theme::accent_fill(painter, lit, radius, 0.0);
        }

        let knob = egui::lerp(
            (track.left() + TRACK.y / 2.0)..=(track.right() - TRACK.y / 2.0),
            t,
        );
        let ink = theme::muted().lerp_to_gamma(Color32::WHITE, t);
        painter.circle_filled(egui::pos2(knob, track.center().y), TRACK.y / 2.0 - 3.0, ink);

        if response.has_focus() {
            painter.rect_stroke(
                track.expand(2.0),
                CornerRadius::same((TRACK.y / 2.0 + 2.0) as u8),
                Stroke::new(1.5, theme::accent()),
                egui::StrokeKind::Outside,
            );
        }

        let position = egui::pos2(track.right() + gap, rect.center().y - galley.size().y / 2.0);
        painter.galley(position, galley, theme::text());
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A round mark that is either a filled tick or an empty ring: "watched".
///
/// The checkbox it replaced had the same invisible-when-off problem as the one
/// [`toggle`] replaced, and a list of episodes is the one place where the
/// unwatched state is the one that most needs to be seen.
pub fn watched_mark(ui: &mut egui::Ui, watched: bool) -> egui::Response {
    const SIZE: f32 = 20.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(SIZE), Sense::click());
    if ui.is_rect_visible(rect) {
        let ctx = ui.ctx();
        let t = ctx.animate_bool_with_time(response.id, watched, theme::motion::STATE);
        let hover = ctx.animate_bool_with_time(
            response.id.with("hover"),
            response.hovered(),
            theme::motion::HOVER,
        );
        let painter = ui.painter();
        let center = rect.center();
        let radius = SIZE / 2.0 - 1.0;

        if t > 0.0 {
            painter.circle_filled(center, radius, theme::accent().gamma_multiply(t));
        }
        painter.circle_stroke(
            center,
            radius,
            Stroke::new(
                1.5,
                theme::muted()
                    .lerp_to_gamma(theme::text(), hover)
                    .gamma_multiply(1.0 - t),
            ),
        );
        // The tick is drawn in both states: faintly, under the pointer, while
        // unwatched — so the mark says what clicking it will do.
        let ink = if watched {
            theme::on_accent()
        } else {
            theme::muted().gamma_multiply(hover)
        };
        if watched || hover > 0.0 {
            let points = [
                center + Vec2::new(-4.5, 0.2),
                center + Vec2::new(-1.4, 3.3),
                center + Vec2::new(4.6, -3.2),
            ];
            painter.line(points.to_vec(), Stroke::new(1.8, ink));
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A group of settings: a heading, an optional line under it, and a card that
/// holds the rows.
///
/// The libadwaita preferences pattern, which is what `proton-drive-linux`
/// settles on: one card per subject, one row per setting, the setting's name
/// and what it does on the left and the control on the right — so the eye runs
/// down one edge for names and the other for state.
pub fn settings_group<R>(
    ui: &mut egui::Ui,
    heading: &str,
    description: Option<&str>,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.add_space(theme::space::XS);
    ui.label(
        theme::Role::Subhead
            .rich(heading)
            .strong()
            .color(theme::text()),
    );
    if let Some(description) = description {
        ui.add_space(theme::space::XXS);
        ui.label(crate::ui::muted(description));
    }
    ui.add_space(theme::space::M);
    let inner = egui::Frame::new()
        .fill(theme::card())
        .corner_radius(CornerRadius::same(theme::radius::LG))
        .inner_margin(egui::Margin::symmetric(16, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui)
        })
        .inner;
    ui.add_space(theme::space::XXL);
    inner
}

/// One row of a [`settings_group`]: what it is on the left, the control on the
/// right. Rows after the first get a hairline above them.
pub fn settings_row<R>(
    ui: &mut egui::Ui,
    title: &str,
    subtitle: Option<&str>,
    control: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let top = ui.cursor().top();
    // The first row of a card starts at its top margin; anything below that
    // has a row above it to be divided from.
    if top > ui.max_rect().top() + 1.0 {
        let y = top;
        ui.painter().hline(
            ui.max_rect().x_range(),
            y,
            Stroke::new(1.0, theme::card_hover()),
        );
    }
    ui.add_space(theme::space::L);
    let inner = ui
        .horizontal(|ui| {
            ui.set_min_height(28.0);
            let text_width = (ui.available_width() * 0.62).max(160.0);
            ui.vertical(|ui| {
                ui.set_max_width(text_width);
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(theme::Role::Body.rich(title).color(theme::text()));
                if let Some(subtitle) = subtitle {
                    ui.add(egui::Label::new(crate::ui::muted(subtitle)).wrap());
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), control)
                .inner
        })
        .inner;
    ui.add_space(theme::space::L);
    inner
}

/// A row of choices where exactly one is on: pills on a shared track.
///
/// Returns the one clicked, if it was not already the chosen one.
pub fn segmented<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    current: T,
    choices: &[(T, &str)],
) -> Option<T> {
    let mut picked = None;
    egui::Frame::new()
        .fill(theme::card_hover())
        .corner_radius(CornerRadius::same(theme::radius::MD))
        .inner_margin(egui::Margin::same(3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for (value, label) in choices {
                    let selected = *value == current;
                    let text = theme::Role::Label.rich(*label).color(if selected {
                        theme::on_accent()
                    } else {
                        theme::text()
                    });
                    let button = egui::Button::new(text)
                        .corner_radius(CornerRadius::same(theme::radius::SM))
                        .fill(if selected {
                            theme::accent()
                        } else {
                            Color32::TRANSPARENT
                        })
                        .stroke(Stroke::NONE);
                    if ui.add(button).clicked() && !selected {
                        picked = Some(*value);
                    }
                }
            });
        });
    picked
}

/// What a [`download_button`] shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DownloadGlyph {
    /// Not downloaded: an arrow into a tray.
    Download,
    /// Coming down: a ring filling with how far it has got.
    Progress(f32),
    /// Stopped part way: the ring, still, with a pause mark.
    Paused(f32),
    /// Here: a tick.
    Done,
}

/// A round icon button for one file's download.
pub fn download_button(ui: &mut egui::Ui, glyph: DownloadGlyph) -> egui::Response {
    const SIZE: f32 = 30.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(SIZE), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let hover =
        ui.ctx()
            .animate_bool_with_time(response.id, response.hovered(), theme::motion::HOVER);
    let painter = ui.painter();
    let c = rect.center();
    if hover > 0.0 {
        painter.circle_filled(c, SIZE / 2.0, theme::card_hover().gamma_multiply(hover));
    }
    let ink = theme::muted().lerp_to_gamma(theme::text(), hover);
    let stroke = Stroke::new(1.6, ink);
    let ring = |painter: &egui::Painter, fraction: f32| {
        let radius = 9.0;
        painter.circle_stroke(c, radius, Stroke::new(2.0, theme::card_hover()));
        let steps = 40;
        let points: Vec<egui::Pos2> = (0..=((steps as f32 * fraction.clamp(0.0, 1.0)) as usize))
            .map(|step| {
                let angle = -std::f32::consts::FRAC_PI_2
                    + std::f32::consts::TAU * step as f32 / steps as f32;
                c + Vec2::angled(angle) * radius
            })
            .collect();
        if points.len() > 1 {
            painter.add(egui::Shape::line(points, Stroke::new(2.0, theme::accent())));
        }
    };
    match glyph {
        DownloadGlyph::Download => {
            painter.line_segment([c + Vec2::new(0.0, -6.0), c + Vec2::new(0.0, 3.0)], stroke);
            painter.line(
                vec![
                    c + Vec2::new(-4.0, -1.0),
                    c + Vec2::new(0.0, 3.0),
                    c + Vec2::new(4.0, -1.0),
                ],
                stroke,
            );
            painter.line_segment([c + Vec2::new(-6.0, 7.0), c + Vec2::new(6.0, 7.0)], stroke);
        }
        DownloadGlyph::Progress(fraction) => {
            ring(painter, fraction);
            // Keeps a frame coming while the ring has somewhere to go.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
            if hover > 0.0 {
                for x in [-2.5, 2.5] {
                    painter.line_segment(
                        [c + Vec2::new(x, -3.5), c + Vec2::new(x, 3.5)],
                        Stroke::new(1.8, ink.gamma_multiply(hover)),
                    );
                }
            }
        }
        DownloadGlyph::Paused(fraction) => {
            ring(painter, fraction);
            for x in [-2.5, 2.5] {
                painter.line_segment(
                    [c + Vec2::new(x, -3.5), c + Vec2::new(x, 3.5)],
                    Stroke::new(1.8, ink),
                );
            }
        }
        DownloadGlyph::Done => {
            painter.line(
                vec![
                    c + Vec2::new(-5.0, 0.0),
                    c + Vec2::new(-1.5, 3.5),
                    c + Vec2::new(5.0, -3.5),
                ],
                Stroke::new(2.0, theme::accent().lerp_to_gamma(ink, hover)),
            );
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A small rounded label: a genre, a year, a fact.
pub fn chip(ui: &mut egui::Ui, text: &str) -> egui::Response {
    egui::Frame::new()
        .fill(theme::card_hover().gamma_multiply(0.8))
        .corner_radius(CornerRadius::same(theme::radius::LG))
        .inner_margin(egui::Margin::symmetric(9, 3))
        .show(ui, |ui| {
            ui.label(theme::Role::Caption.rich(text).color(theme::text()));
        })
        .response
}
