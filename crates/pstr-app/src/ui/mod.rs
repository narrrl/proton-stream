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
pub mod shortcuts;
pub mod title;
pub mod toast;
pub mod transport;
pub mod widgets;

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

    /// A still for one episode: the provider's, else Proton's own thumbnail of
    /// the file. Only asked for by rows that are on screen.
    pub fn still(&mut self, title_key: &str, episode: &Episode) -> Option<egui::TextureHandle> {
        let url = self
            .episode(title_key, episode)
            .and_then(|found| found.still_url.clone());
        if let Some(url) = url {
            let key = format!("still:{}", url);
            let engine = self.engine;
            if let Some(texture) = self
                .posters
                .texture(&key, || engine.request_poster(key.clone(), url))
            {
                return Some(texture);
            }
        }
        let node = &episode.node;
        let engine = self.engine;
        self.thumbs
            .texture(&thumbnail_key(node), || engine.request_thumbnail(node))
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
            if let Some(texture) = self.posters.texture(&title.key, || {
                engine.request_poster(title.key.clone(), url.to_string())
            }) {
                return Some((texture, shape));
            }
        }

        let node = title.poster_node()?;
        let engine = self.engine;
        let texture = self
            .thumbs
            .texture(&thumbnail_key(node), || engine.request_thumbnail(node))?;
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

/// `1.25×`, or `1×`.
pub fn format_speed(speed: f64) -> String {
    let text = format!("{speed:.2}");
    format!("{}×", text.trim_end_matches('0').trim_end_matches('.'))
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

/// The search box in the top bar: a magnifier, the text, and a way to clear it.
///
/// Escape inside it clears it. `focus` puts the caret in it this frame — which
/// is what `Ctrl+F` and `/` ask for from anywhere in the library.
pub fn search_field(ui: &mut egui::Ui, search: &mut String, focus: bool) {
    let id = egui::Id::new("library-search");
    let width = 240.0;
    let height = ui.spacing().interact_size.y;
    let (outer, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let has_focus = ui.memory(|memory| memory.has_focus(id));
    let lit = ui
        .ctx()
        .animate_bool_with_time(id.with("lit"), has_focus, 0.12);

    let radius = CornerRadius::same((height / 2.0) as u8);
    ui.painter().rect_filled(outer, radius, theme::card());
    ui.painter().rect_stroke(
        outer,
        radius,
        Stroke::new(1.0, theme::card_hover().lerp_to_gamma(theme::accent(), lit)),
        egui::StrokeKind::Inside,
    );

    // A magnifier: a ring and a handle, so no font has to have the glyph.
    let ink = theme::muted().lerp_to_gamma(theme::text(), lit);
    let lens = egui::pos2(outer.left() + 16.0, outer.center().y - 1.0);
    ui.painter().circle_stroke(lens, 5.0, Stroke::new(1.5, ink));
    ui.painter().line_segment(
        [lens + Vec2::new(3.6, 3.6), lens + Vec2::new(7.0, 7.0)],
        Stroke::new(1.5, ink),
    );

    let clear_width = if search.is_empty() { 0.0 } else { 26.0 };
    let field_rect = Rect::from_min_max(
        egui::pos2(outer.left() + 28.0, outer.top()),
        egui::pos2(outer.right() - 8.0 - clear_width, outer.bottom()),
    );
    let response = ui.put(
        field_rect,
        egui::TextEdit::singleline(search)
            .id(id)
            .frame(egui::Frame::NONE)
            .hint_text(if has_focus {
                "Titles and filenames"
            } else {
                "Search   Ctrl+F"
            })
            .vertical_align(egui::Align::Center)
            .desired_width(field_rect.width()),
    );
    if focus {
        response.request_focus();
    }
    if response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Escape)) {
        search.clear();
        response.surrender_focus();
    }

    if !search.is_empty() {
        let clear = Rect::from_center_size(
            egui::pos2(outer.right() - 18.0, outer.center().y),
            Vec2::splat(18.0),
        );
        let response = ui.interact(clear, id.with("clear"), Sense::click());
        let ink = if response.hovered() {
            theme::text()
        } else {
            theme::muted()
        };
        let c = clear.center();
        let d = 3.5;
        ui.painter().line_segment(
            [c + Vec2::new(-d, -d), c + Vec2::new(d, d)],
            Stroke::new(1.5, ink),
        );
        ui.painter().line_segment(
            [c + Vec2::new(-d, d), c + Vec2::new(d, -d)],
            Stroke::new(1.5, ink),
        );
        if response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Clear (Esc)")
            .clicked()
        {
            search.clear();
        }
    }
}

/// A thin bar in the accent, for how far something has got.
///
/// egui's own progress bar is one flat colour with the percentage written over
/// it; this is the same bar the tiles and the seek bar use, so a download reads
/// as part of the same app.
pub fn progress_bar(ui: &mut egui::Ui, fraction: f32, width: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 6.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        let radius = CornerRadius::same(3);
        ui.painter().rect_filled(rect, radius, theme::card_hover());
        let mut done = rect;
        done.set_width(rect.width() * fraction.clamp(0.0, 1.0));
        if done.width() > 0.5 {
            theme::accent_fill(ui.painter(), done, radius, 0.0);
        }
    }
    response
}

/// A centred message for a page with nothing on it: what is missing, and what
/// to do about it.
pub fn empty_state(ui: &mut egui::Ui, heading: &str, body: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(96.0);
        ui.label(theme::Role::Title.rich(heading).strong());
        ui.add_space(6.0);
        ui.label(muted(body));
    });
}

/// The danger colour as a button: for the one choice in a dialog that cannot be
/// taken back.
pub fn danger_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(theme::on_accent()))
            .fill(theme::danger()),
    )
}

/// What a viewer answered a [`confirm`] dialog with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// Still open.
    Pending,
    Confirmed,
    /// Cancelled — by the button, Escape, or a click beside the dialog.
    Declined,
}

/// A question with two answers, over everything else.
///
/// For the actions that destroy something the viewer cannot get back from
/// inside the app — stored secrets, downloaded bytes. Everything else acts on
/// the click.
pub fn confirm(ctx: &egui::Context, id: &str, heading: &str, body: &str, verb: &str) -> Answer {
    let mut answer = Answer::Pending;
    let modal = egui::Modal::new(egui::Id::new(("confirm", id)))
        .frame(
            egui::Frame::new()
                .fill(theme::surface())
                .inner_margin(egui::Margin::same(18))
                .corner_radius(CornerRadius::same(10)),
        )
        .show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(
                theme::Role::Heading
                    .rich(heading)
                    .strong()
                    .color(theme::text()),
            );
            ui.add_space(6.0);
            ui.label(theme::Role::Body.rich(body).color(theme::muted()));
            ui.add_space(16.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if danger_button(ui, verb).clicked() {
                    answer = Answer::Confirmed;
                }
                if ui.button("Cancel").clicked() {
                    answer = Answer::Declined;
                }
            });
        });
    if answer == Answer::Pending && modal.should_close() {
        answer = Answer::Declined;
    }
    answer
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

/// Two lines of name plus one of subtitle. Fixed, because the tile is allocated
/// before the name is laid out — and because a grid whose rows are each a
/// different height by title length reads as broken.
const CARD_TEXT_HEIGHT: f32 = 54.0;

/// How tall [`card`] draws a tile of this width, so a grid can step over rows
/// it does not draw.
pub fn card_height(width: f32) -> f32 {
    (width * theme::CARD_ASPECT).round() + CARD_TEXT_HEIGHT
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
    let text_height = CARD_TEXT_HEIGHT;
    let radius = CornerRadius::same(8);

    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, image_height + text_height), Sense::click());
    // Reached with the arrow keys or Tab: bring it into view, before the
    // early return below would skip a tile that is just off screen.
    if response.gained_focus() {
        response.scroll_to_me(None);
    }
    if !ui.is_rect_visible(rect) {
        return response;
    }

    let image_rect = Rect::from_min_size(rect.min, Vec2::new(width, image_height));
    // Hover is a value, not a flag: the border, the lift and the shadow all
    // ride it, so they arrive together rather than each snapping on its own.
    // egui drives this from wall-clock time, so it is the same speed whatever
    // the frame rate — and it needs no state kept here.
    // Focus lights a tile the way the pointer does, so the keyboard can be
    // followed across the grid.
    let hover = ui.ctx().animate_bool_with_time(
        response.id.with("hover"),
        response.hovered() || response.has_focus(),
        0.12,
    );
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
        placeholder(painter, image_rect, card.name, 1.0 - art_in);
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
pub fn cover_uv(image: Vec2, target: Vec2) -> Rect {
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

/// What a tile with no picture shows: a gradient of its own, and its name.
///
/// Two grey letters on a grey card was the whole tile for any title nothing
/// knew a picture for — a wall of them read as a page that had failed to load.
/// The colours come from the name, so a title wears the same ones every time
/// and two titles side by side rarely match.
///
/// The hash is FNV-1a: stable across runs and platforms, which the standard
/// library's hasher deliberately is not.
pub(crate) fn placeholder(painter: &egui::Painter, rect: Rect, name: &str, opacity: f32) {
    let hash = name.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    let hue = (hash % 360) as f32 / 360.0;
    let light = theme::palette().light;
    let (saturation, value) = if light { (0.25, 0.92) } else { (0.45, 0.34) };
    let from: Color32 = egui::ecolor::Hsva::new(hue, saturation, value, 1.0).into();
    let to: Color32 =
        egui::ecolor::Hsva::new((hue + 0.12) % 1.0, saturation, value * 0.65, 1.0).into();
    theme::ramp_fill(
        painter,
        rect,
        CornerRadius::same(8),
        (from, to),
        theme::Direction::Vertical,
        opacity,
    );

    let ink = if light {
        Color32::from_black_alpha(170)
    } else {
        Color32::from_white_alpha(215)
    }
    .gamma_multiply(opacity);
    let mut job = egui::text::LayoutJob::simple(
        name.to_owned(),
        theme::Role::Heading.font(),
        ink,
        rect.width() - 28.0,
    );
    job.wrap.max_rows = 3;
    job.wrap.overflow_character = Some('…');
    job.halign = egui::Align::Center;
    let galley = painter.layout_job(job);
    painter.galley(
        egui::pos2(rect.center().x, rect.center().y - galley.size().y / 2.0),
        galley,
        ink,
    );
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
