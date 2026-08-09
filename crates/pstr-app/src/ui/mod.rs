//! Drawing. Every function here takes what it needs explicitly rather than an
//! `&mut App`, because the borrow checker is what keeps a click handler from
//! mutating the library it is iterating: pages collect [`Action`]s and the app
//! applies them after the frame is drawn.

pub mod downloads;
pub mod library;
pub mod matcher;
pub mod player;
pub mod settings;
pub mod shares;
pub mod title;
pub mod transport;

use std::collections::HashMap;

use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, Vec2};
use pstr_core::library::{Episode, Title};
use pstr_core::metadata::{ArtShape, EpisodeGuide, EpisodeMetadata, MetadataRecord};

use crate::engine::{Engine, ImageCache, thumbnail_key};
use crate::theme;

/// Everything the pages need to draw one title's picture.
///
/// Bundled because there are now three places a tile's art can come from and
/// four call sites that need all three; passing them separately made every
/// signature in this module four arguments longer.
pub struct Art<'a> {
    pub engine: &'a Engine,
    /// Proton's own per-file thumbnails.
    pub thumbs: &'a mut ImageCache,
    /// Artwork from a metadata provider, keyed by title key.
    pub posters: &'a mut ImageCache,
    /// What the providers have said, keyed by title key.
    pub metadata: &'a HashMap<String, MetadataRecord>,
    /// What they said about the episodes under those titles.
    pub episodes: &'a HashMap<String, EpisodeGuide>,
}

impl Art<'_> {
    /// What the provider says about one file of a title, matched on the
    /// numbering its name states.
    pub fn episode(&self, title_key: &str, episode: &Episode) -> Option<&EpisodeMetadata> {
        let number = episode.node.parsed.episode?;
        self.episodes
            .get(title_key)?
            .get(episode.node.parsed.season, number)
    }

    /// The picture for a title, and how to fit it.
    ///
    /// Provider artwork first, then Proton's still, then nothing. That order is
    /// deliberate: a provider's poster is *of the title*, where a Proton
    /// thumbnail is whatever frame happened to be at the start of the first
    /// episode — often a black frame or a studio logo. Both beat initials.
    pub fn of(&mut self, title: &Title) -> Option<(egui::TextureHandle, ArtShape)> {
        if let Some((url, shape)) = self
            .metadata
            .get(&title.key)
            .and_then(|record| record.metadata.as_ref())
            .and_then(|metadata| metadata.tile_art())
        {
            let engine = self.engine;
            let key = title.key.clone();
            let url = url.to_string();
            if let Some(texture) = self
                .posters
                .texture(title.key.clone(), || engine.request_poster(key, url))
            {
                return Some((texture, shape));
            }
        }

        let node = title.poster_node()?;
        let engine = self.engine;
        let texture = self
            .thumbs
            .texture(thumbnail_key(node), || engine.request_thumbnail(node))?;
        Some((texture, ArtShape::Landscape))
    }
}

/// `1:03:47`, or `4:12` for anything under an hour.
pub fn format_time(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "--:--".into();
    }
    let total = seconds.round() as u64;
    let (hours, minutes, secs) = (total / 3600, (total / 60) % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes}:{secs:02}")
    }
}

/// `1.4 GiB`, for what a file costs to watch.
pub fn format_size(bytes: i64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes.max(0) as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// A section title with a rule under it.
pub fn section(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(
        theme::Role::Section
            .rich(text)
            .strong()
            .color(theme::text()),
    );
    ui.add_space(2.0);
}

/// Small grey text, for everything that is context rather than content.
pub fn muted(text: impl Into<String>) -> egui::RichText {
    theme::Role::Caption.rich(text).color(theme::muted())
}

/// The one button style that means "this is the action".
pub fn accent_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    filled(ui, text, Fill::Accent, Vec2::new(0.0, 0.0))
}

/// What [`filled`] paints behind its label.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fill {
    /// The accent, gradient and all.
    Accent,
    /// Nothing — something else already painted the accent here, and the label
    /// only has to be legible on it.
    Ink,
    /// Nothing at rest, the hover colour under the pointer.
    None,
}

/// A row of tabs with one accent pill that slides between them.
///
/// Drawn as a row rather than a tab at a time because the pill has to be
/// painted *behind* labels whose positions are not known until they have been
/// laid out, and because a pill that moves has to be one pill: a per-tab fill
/// can only ever cross-fade, which reads as two things blinking rather than one
/// thing travelling.
///
/// Returns the index that was clicked, if any.
pub fn tabs(ui: &mut egui::Ui, id: egui::Id, items: &[(&str, bool)]) -> Option<usize> {
    // Reserved now, filled in below: everything drawn after this lands on top
    // of it whatever it turns out to be.
    let pill = ui.painter().add(egui::Shape::Noop);

    let mut clicked = None;
    let mut selected = None;
    for (index, (text, is_selected)) in items.iter().enumerate() {
        let response = tab(ui, *is_selected, text);
        if *is_selected {
            selected = Some(response.rect);
        }
        if response.clicked() {
            clicked = Some(index);
        }
    }

    if let Some(target) = selected {
        // Interpolating the edges rather than the centre and the width keeps
        // the pill from overshooting when it moves between tabs of different
        // lengths — both edges arrive at the same time.
        let ctx = ui.ctx();
        let left = ctx.animate_value_with_time(id.with("left"), target.left(), 0.16);
        let right = ctx.animate_value_with_time(id.with("right"), target.right(), 0.16);
        let rect = Rect::from_min_max(
            egui::pos2(left, target.top()),
            egui::pos2(right, target.bottom()),
        );
        ui.painter().set(
            pill,
            theme::accent_shape(ui.ctx(), rect, CornerRadius::same(8)),
        );
    }

    clicked
}

/// One tab. The selected one is ink on the pill [`tabs`] draws; the others are
/// text until the pointer is over them.
fn tab(ui: &mut egui::Ui, selected: bool, text: &str) -> egui::Response {
    filled(
        ui,
        text,
        if selected { Fill::Ink } else { Fill::None },
        Vec2::new(4.0, 0.0),
    )
}

/// A button painted by hand, so that "filled with the accent" can mean a
/// gradient.
///
/// `egui::Button` takes a single `Color32` and there is no way in to give it
/// anything else, so the fill, the label and the hover states are all drawn
/// here. Everything else — sizing, padding, the click — is still egui's.
fn filled(ui: &mut egui::Ui, text: &str, fill: Fill, extra: Vec2) -> egui::Response {
    let padding = ui.spacing().button_padding + extra;
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::TextStyle::Button.resolve(ui.style()),
        Color32::PLACEHOLDER,
    );
    let size = Vec2::new(
        galley.size().x + padding.x * 2.0,
        (galley.size().y + padding.y * 2.0).max(ui.spacing().interact_size.y),
    );

    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }

    let radius = CornerRadius::same(8);
    // Eased rather than switched, so the veil arrives over a few frames instead
    // of appearing whole the moment the pointer crosses the edge. The press is
    // quicker than the hover: a click should feel like it registered, not like
    // it was considered.
    let ctx = ui.ctx().clone();
    let hover = ctx.animate_bool_with_time(response.id.with("hover"), response.hovered(), 0.10);
    let press = ctx.animate_bool_with_time(
        response.id.with("press"),
        response.is_pointer_button_down_on(),
        0.05,
    );
    let lift = 0.10 * hover * (1.0 - press) - 0.14 * press;

    let ink = match fill {
        Fill::Accent => {
            theme::accent_fill(ui.painter(), rect, radius, lift);
            theme::on_accent()
        }
        // The pill is already under this one; only the veil is missing.
        Fill::Ink => {
            theme::veil_over(ui.painter(), rect, radius, lift);
            theme::on_accent()
        }
        Fill::None => {
            if hover > 0.0 {
                ui.painter()
                    .rect_filled(rect, radius, theme::card_hover().gamma_multiply(hover));
            }
            theme::muted().lerp_to_gamma(theme::text(), hover)
        }
    };

    let position = rect.center() - galley.size() / 2.0;
    ui.painter().galley(position, galley, ink);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// What a tile shows.
pub struct Card<'a> {
    /// The picture and how to fit it. `None` draws the placeholder.
    pub art: Option<(egui::TextureHandle, ArtShape)>,
    pub name: &'a str,
    pub subtitle: String,
    /// Fraction watched, drawn as a bar across the bottom of the still.
    pub progress: Option<f32>,
    /// A short label in the corner — `S01E04`.
    pub badge: Option<String>,
    /// How wide to draw it.
    ///
    /// A grid flexes this so its rows reach the right edge of the window; a
    /// sideways-scrolling shelf has no edge to reach and passes
    /// [`theme::CARD_WIDTH`].
    pub width: f32,
}

/// Draw one tile and report whether it was clicked.
///
/// A landscape picture is cropped to fill: a grid of differently-shaped black
/// bars reads as broken, and a video still is not a composition anyone framed,
/// so nothing is lost by trimming it. A *poster* is fitted instead, because
/// cropping a 2:3 poster to 16:9 removes most of what makes it recognisable —
/// the title, usually. The gap it leaves is filled with the card colour, which
/// reads as deliberate in a way a stretched poster does not.
pub fn card(ui: &mut egui::Ui, card: Card<'_>) -> egui::Response {
    let width = card.width;
    let image_height = (width * theme::CARD_ASPECT).round();
    // Two lines of name plus one of subtitle. Fixed, because the tile is
    // allocated before the name is laid out — and because a grid whose rows are
    // each a different height by title length reads as broken.
    let text_height = 54.0;
    let radius = CornerRadius::same(8);

    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, image_height + text_height), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }

    let image_rect = Rect::from_min_size(rect.min, Vec2::new(width, image_height));
    // Hover is a value, not a flag: the border, the lift and the shadow all
    // ride it, so they arrive together rather than each snapping on its own.
    // egui drives this from wall-clock time, so it is the same speed whatever
    // the frame rate — and it needs no state kept here.
    let hover =
        ui.ctx()
            .animate_bool_with_time(response.id.with("hover"), response.hovered(), 0.12);
    // The picture arrives whenever its download finishes, several frames after
    // the tile first drew. Fading it in over the placeholder turns a wall of
    // letters popping into stills into one settle.
    let art_in = ui
        .ctx()
        .animate_bool_with_time(response.id.with("art"), card.art.is_some(), 0.22);
    let painter = ui.painter();

    if hover > 0.0 {
        painter.add(theme::tile_shadow(hover).as_shape(image_rect, radius));
    }
    painter.rect_filled(image_rect, radius, theme::card());

    // Under the picture, not instead of it: it is what shows through while the
    // art fades in, and it is the whole tile when there is no art to come.
    if art_in < 1.0 {
        painter.text(
            image_rect.center(),
            Align2::CENTER_CENTER,
            initials(card.name),
            egui::FontId::proportional(28.0),
            theme::muted().gamma_multiply(1.0 - art_in),
        );
    }

    // White because a textured rect *multiplies* its fill by the texture; the
    // alpha on it is what fades the picture in.
    let ink = Color32::WHITE.gamma_multiply(art_in);
    match &card.art {
        Some((texture, ArtShape::Landscape)) => {
            let size = texture.size_vec2();
            painter.add(
                egui::epaint::RectShape::filled(image_rect, radius, ink)
                    .with_texture(texture.id(), cover_uv(size, image_rect.size())),
            );
        }
        Some((texture, ArtShape::Portrait)) => {
            painter.add(
                egui::epaint::RectShape::filled(
                    contain_rect(texture.size_vec2(), image_rect),
                    radius,
                    ink,
                )
                .with_texture(
                    texture.id(),
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                ),
            );
        }
        None => {}
    }

    if let Some(badge) = &card.badge {
        let anchor = image_rect.right_top() + Vec2::new(-8.0, 8.0);
        let galley =
            painter.layout_no_wrap(badge.clone(), theme::Role::Micro.font(), Color32::WHITE);
        let background = Rect::from_min_size(
            anchor - Vec2::new(galley.size().x + 10.0, 0.0),
            galley.size() + Vec2::new(10.0, 4.0),
        );
        painter.rect_filled(
            background,
            CornerRadius::same(4),
            Color32::from_black_alpha(180),
        );
        painter.galley(background.min + Vec2::new(5.0, 2.0), galley, Color32::WHITE);
    }

    if let Some(progress) = card.progress {
        let clip_rect = Rect::from_min_size(
            image_rect.left_bottom() - Vec2::new(0.0, 4.0),
            Vec2::new(width, 4.0),
        );
        let clipped_painter = painter.with_clip_rect(clip_rect);

        let track = Rect::from_min_size(
            image_rect.left_bottom() - Vec2::new(0.0, 16.0),
            Vec2::new(width, 16.0),
        );
        let track_radius = CornerRadius {
            nw: 0,
            ne: 0,
            sw: 8,
            se: 8,
        };
        clipped_painter.rect_filled(track, track_radius, Color32::from_black_alpha(150));

        let mut played = track;
        played.set_width(track.width() * progress.clamp(0.0, 1.0));

        let played_radius = CornerRadius {
            nw: 0,
            ne: 0,
            sw: 8,
            se: if played.max.x >= track.max.x - 0.1 {
                8
            } else {
                0
            },
        };
        theme::accent_fill(&clipped_painter, played, played_radius, 0.0);
    }

    if hover > 0.0 {
        painter.rect_stroke(
            image_rect,
            radius,
            Stroke::new(2.0, theme::accent().gamma_multiply(hover)),
            egui::StrokeKind::Inside,
        );
    }

    let text_rect = Rect::from_min_size(
        image_rect.left_bottom() + Vec2::new(0.0, 6.0),
        Vec2::new(width, text_height - 6.0),
    );
    let painter = painter.with_clip_rect(text_rect);
    // Long titles are the normal case in a release-named library, so the name
    // is capped at two lines with an ellipsis rather than allowed to grow into
    // the line below it.
    let mut job = egui::text::LayoutJob::simple(
        card.name.to_string(),
        theme::Role::Body.font(),
        // Towards white as the pointer arrives, so the name lifts with the
        // border rather than staying put while everything around it moves.
        theme::text().lerp_to_gamma(Color32::WHITE, hover),
        width,
    );
    job.wrap.max_rows = 2;
    job.wrap.overflow_character = Some('…');
    let name = painter.layout_job(job);
    let name_height = name.size().y;
    painter.galley(text_rect.min, name, theme::text());
    painter.text(
        text_rect.min + Vec2::new(0.0, name_height + 2.0),
        Align2::LEFT_TOP,
        card.subtitle,
        theme::Role::Caption.font(),
        theme::muted(),
    );

    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The largest centred rectangle inside `target` with `image`'s aspect ratio.
///
/// The other half of [`cover_uv`]: where that one keeps the whole rectangle and
/// trims the picture, this keeps the whole picture and gives back part of the
/// rectangle.
fn contain_rect(image: Vec2, target: Rect) -> Rect {
    if image.x <= 0.0 || image.y <= 0.0 {
        return target;
    }
    let scale = (target.width() / image.x).min(target.height() / image.y);
    Rect::from_center_size(target.center(), image * scale)
}

/// UV rectangle that crops `image` to fill `target` without distorting it.
fn cover_uv(image: Vec2, target: Vec2) -> Rect {
    if image.x <= 0.0 || image.y <= 0.0 || target.x <= 0.0 || target.y <= 0.0 {
        return Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    }
    let image_aspect = image.x / image.y;
    let target_aspect = target.x / target.y;
    let (width, height) = if image_aspect > target_aspect {
        // Too wide: keep the full height, trim the sides.
        (target_aspect / image_aspect, 1.0)
    } else {
        (1.0, image_aspect / target_aspect)
    };
    Rect::from_center_size(egui::pos2(0.5, 0.5), Vec2::new(width, height))
}

/// Up to two letters, for a tile with no picture.
fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|word| word.chars().next())
        .filter(|character| character.is_alphanumeric())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect()
}

/// How a grid divides the width it was given.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub columns: usize,
    /// What each card should be drawn at, so the row reaches both edges.
    pub width: f32,
}

/// Fit as many cards as will go, then widen them to use the rest.
///
/// [`theme::CARD_WIDTH`] is a *minimum*, not the width. Dividing by it and
/// flooring — which is what this used to do — leaves up to one whole card of
/// dead space at the right of every row, and on a wide window that reads as a
/// layout that failed to line up rather than as a margin.
pub fn columns(available: f32) -> Grid {
    let columns = (((available + theme::CARD_GAP) / (theme::CARD_WIDTH + theme::CARD_GAP)).floor()
        as usize)
        .max(1);
    // The gaps come out of the width before it is split. Floored, because a
    // fraction of a point times four columns is enough to push the last card
    // onto a row of its own. A window too narrow for even one full card gets a
    // card narrower than the minimum, which is better than one clipped by the
    // edge.
    let width = ((available - theme::CARD_GAP * (columns - 1) as f32) / columns as f32)
        .floor()
        .max(1.0);
    Grid { columns, width }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_formatted_by_length() {
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(format_time(72.4), "1:12");
        assert_eq!(format_time(3805.0), "1:03:25");
        assert_eq!(format_time(f64::NAN), "--:--");
    }

    #[test]
    fn sizes_use_binary_units() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024 * 1024), "1.0 MiB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024 / 2), "1.5 GiB");
    }

    #[test]
    fn a_wide_image_is_cropped_at_the_sides() {
        let uv = cover_uv(Vec2::new(200.0, 50.0), Vec2::new(100.0, 50.0));
        assert!(uv.width() < 1.0);
        assert!((uv.height() - 1.0).abs() < f32::EPSILON);
        assert!((uv.center().x - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn a_tall_image_is_cropped_top_and_bottom() {
        let uv = cover_uv(Vec2::new(50.0, 200.0), Vec2::new(100.0, 50.0));
        assert!((uv.width() - 1.0).abs() < f32::EPSILON);
        assert!(uv.height() < 1.0);
    }

    #[test]
    fn a_degenerate_image_falls_back_to_the_whole_texture() {
        let uv = cover_uv(Vec2::ZERO, Vec2::new(100.0, 50.0));
        assert_eq!(
            uv,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))
        );
    }

    #[test]
    fn initials_take_two_letters() {
        assert_eq!(initials("Cowboy Bebop"), "CB");
        assert_eq!(initials("Akira"), "A");
        assert_eq!(initials(""), "");
    }

    #[test]
    fn column_count_never_reaches_zero() {
        assert_eq!(columns(0.0).columns, 1);
        assert!(columns(2000.0).columns > 1);
    }

    #[test]
    fn a_row_of_cards_fills_the_width_it_was_given() {
        // The defect this replaced: floor(available / CARD_WIDTH) cards at a
        // fixed width leave up to one card's worth of the window empty.
        for available in [400.0, 813.0, 1280.0, 1920.5, 3440.0] {
            let grid = columns(available);
            let used =
                grid.width * grid.columns as f32 + theme::CARD_GAP * (grid.columns - 1) as f32;
            assert!(used <= available, "{available}: {used} overflows the row");
            assert!(
                available - used < grid.columns as f32 + 1.0,
                "{available}: {} left over",
                available - used,
            );
        }
    }

    #[test]
    fn cards_never_go_below_the_minimum_while_they_still_fit() {
        for available in [500.0, 900.0, 2400.0] {
            assert!(columns(available).width >= theme::CARD_WIDTH);
        }
    }
}
