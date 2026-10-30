//! Drawing. Every function here takes what it needs explicitly rather than an
//! `&mut App`, because the borrow checker is what keeps a click handler from
//! mutating the library it is iterating: pages collect [`Action`]s and the app
//! applies them after the frame is drawn.

pub mod account;
pub mod downloads;
pub mod history;
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
use pstr_core::metadata::{ArtShape, EpisodeGuide, EpisodeMetadata, MetadataRecord, TitleNames};

use crate::app::Action;
use crate::engine::{Engine, ImageCache, WIDE_MAX_EDGE, thumbnail_key};
use crate::theme;
use widgets::Named;

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
    /// Which name a title goes by.
    pub names: TitleNames,
}

pub use pstr_core::browse::display_name as title_name;

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

    /// The name to show for `title`. See [`title_name`].
    pub fn name<'t>(&'t self, title: &'t Title) -> &'t str {
        title_name(title, self.metadata, self.names)
    }

    /// A 2:3 cover for `title`, for the poster grid; whatever [`Art::of`]
    /// has when there is none.
    pub fn poster(&mut self, title: &Title) -> Option<(egui::TextureHandle, ArtShape)> {
        let url = self
            .metadata
            .get(&title.key)
            .and_then(|record| record.metadata.as_ref())
            .and_then(|metadata| metadata.poster_url.clone());
        if let Some(url) = url {
            let key = poster_key(&title.key);
            let engine = self.engine;
            if let Some(texture) = self
                .posters
                .texture(&key, || engine.request_poster(key.clone(), url))
            {
                return Some((texture, ArtShape::Portrait));
            }
        }
        self.of(title)
    }

    /// Art for a strip across the whole window, at full size: fanart, else
    /// AniList's banner, else the poster blurred into a wash of its colours —
    /// see [`crate::engine::Engine::request_ambient`] — else whatever
    /// [`Art::of`] has.
    ///
    /// Its own texture rather than the tile's: a 640 px tile picture
    /// stretched to a 1600 px hero is soft, and a 1920 px one in every tile
    /// would be most of the texture budget.
    pub fn wide(&mut self, title: &Title) -> Option<(egui::TextureHandle, ArtShape)> {
        let url = self
            .metadata
            .get(&title.key)
            .and_then(|record| record.metadata.as_ref())
            .and_then(|metadata| metadata.wide_art())
            .map(str::to_owned);
        if let Some(url) = url {
            let key = wide_key(&title.key);
            let engine = self.engine;
            if let Some(texture) = self
                .posters
                .texture(&key, || engine.request_art(key.clone(), url, WIDE_MAX_EDGE))
            {
                return Some((texture, ArtShape::Landscape));
            }
        } else if let Some(url) = self.poster_url(title) {
            let key = ambient_key(&title.key);
            let engine = self.engine;
            if let Some(texture) = self
                .posters
                .texture(&key, || engine.request_ambient(key.clone(), url))
            {
                return Some((texture, ArtShape::Landscape));
            }
        }
        self.of(title)
    }

    /// The colour of whatever [`Art::wide`] is drawing for `title`, once it
    /// has arrived — what that title's veils are tinted with. See
    /// [`theme::mood_veil`].
    pub fn mood(&self, title: &Title) -> Option<egui::Color32> {
        [wide_key, ambient_key, poster_key]
            .into_iter()
            .map(|key| key(&title.key))
            // The tile's own picture last: what `wide` falls back on.
            .chain(std::iter::once(title.key.clone()))
            .find_map(|key| self.posters.mood(&key))
    }

    /// Whether `title` has a picture made for a strip across the window, rather
    /// than [`Art::wide`] standing one in.
    pub fn has_wide_art(&self, title: &Title) -> bool {
        self.metadata
            .get(&title.key)
            .and_then(|record| record.metadata.as_ref())
            .is_some_and(|metadata| metadata.wide_art().is_some())
    }

    fn poster_url(&self, title: &Title) -> Option<String> {
        self.metadata
            .get(&title.key)
            .and_then(|record| record.metadata.as_ref())
            .and_then(|metadata| metadata.poster_url.clone())
    }

    /// The title's name as lettering, trimmed to it, when the provider has
    /// one and it has arrived. Drawn over wide art in place of the name.
    pub fn logo(&mut self, title: &Title) -> Option<egui::TextureHandle> {
        let url = self
            .metadata
            .get(&title.key)
            .and_then(|record| record.metadata.as_ref())
            .and_then(|metadata| metadata.logo_url.clone())?;
        let key = logo_key(&title.key);
        let engine = self.engine;
        self.posters
            .texture(&key, || engine.request_logo(key.clone(), url))
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

/// The central panel's margin around every page. Anything drawn edge to edge —
/// the library's banner, the title page's band — reaches past it.
pub const PAGE_MARGIN: egui::Vec2 = egui::vec2(18.0, 12.0);

/// The texture key of a title's cover, beside [`Art::of`]'s, which is the
/// title key itself.
pub fn poster_key(title_key: &str) -> String {
    format!("poster:{title_key}")
}

/// The texture key of a title's full-size wide art.
pub fn wide_key(title_key: &str) -> String {
    format!("wide:{title_key}")
}

/// The texture key of a title's poster blurred for a band.
pub fn ambient_key(title_key: &str) -> String {
    format!("ambient:{title_key}")
}

/// A title's logo in place of its name in Display type, when there is one to
/// draw; `None` leaves the caller to set the name as it does.
///
/// At most `max_height` tall and as wide as the space allows, up to 420 px —
/// a logo the width of the column shouts. A light palette keeps the name: a
/// clear logo is lettering made for dark art, white as often as not, and over
/// a near-white veil it is gone.
pub fn logo_heading(
    ui: &mut egui::Ui,
    art: &mut Art<'_>,
    title: &Title,
    max_height: f32,
) -> Option<egui::Response> {
    if theme::palette().light {
        return None;
    }
    let texture = art.logo(title);
    // From nothing rather than from the name it replaces: the name is gone the
    // frame the logo is ready, and a logo easing in reads as it arriving.
    let shown = ui.ctx().animate_bool_with_time(
        ui.id().with(("logo", &title.key)),
        texture.is_some(),
        theme::motion::ARRIVE,
    );
    let texture = texture?;
    let aspect = texture.aspect_ratio().max(0.1);
    let width = ui.available_width().min(420.0).min(max_height * aspect);
    let size = egui::vec2(width, width / aspect);
    Some(
        ui.add(
            egui::Image::new((texture.id(), size)).tint(egui::Color32::WHITE.gamma_multiply(shown)),
        )
        .on_hover_text(art.name(title)),
    )
}

/// The texture key of a title's logo.
pub fn logo_key(title_key: &str) -> String {
    format!("logo:{title_key}")
}

/// `24 min`, `2 h 4 min`, `2 h`: a provider's running time in minutes.
pub fn format_runtime(minutes: u32) -> String {
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes} min"),
        (hours, 0) => format!("{hours} h"),
        (hours, minutes) => format!("{hours} h {minutes} min"),
    }
}

/// "just now", "5 min ago", "3 h ago", "2 days ago": how long before `now`
/// the Unix time `at` was.
pub fn ago(at: i64, now: i64) -> String {
    let ago = (now - at).max(0);
    match ago {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", ago / 60),
        3600..86_400 => format!("{} h ago", ago / 3600),
        86_400..172_800 => "1 day ago".to_owned(),
        _ => format!("{} days ago", ago / 86_400),
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

/// `+0.3 s`, `−1.2 s` or `0 s`: a subtitle delay, signed so the direction
/// reads without knowing which way mpv counts.
pub fn format_delay(seconds: f64) -> String {
    if seconds.abs() < 0.05 {
        "0 s".to_owned()
    } else if seconds > 0.0 {
        format!("+{seconds:.1} s")
    } else {
        format!("−{:.1} s", -seconds)
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
    ui.add_space(theme::space::S);
    ui.label(
        theme::Role::Section
            .rich(text)
            .strong()
            .color(theme::text()),
    );
    ui.add_space(theme::space::XXS);
}

/// Small grey text, for everything that is context rather than content.
pub fn muted(text: impl Into<String>) -> egui::RichText {
    theme::Role::Caption.rich(text).color(theme::muted())
}

/// The one button style that means "this is the action".
pub fn accent_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    filled(ui, text, Fill::Accent, Vec2::new(0.0, 0.0))
}

/// The button beside the accent one: the same shape, in a film of the text
/// colour, so "Start over" next to "Resume" reads as the alternative rather
/// than as a second, unrelated kind of control.
pub fn tonal_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    filled(ui, text, Fill::Tonal, Vec2::new(0.0, 0.0))
}

/// My List's button: `+ My List` to put the title on it, `✓ My List` once it
/// is, which takes it off again.
pub fn list_button(ui: &mut egui::Ui, key: &str, on_list: bool, actions: &mut Vec<Action>) {
    let label = if on_list {
        format!("{}  My List", egui_phosphor::regular::CHECK)
    } else {
        format!("{}  My List", egui_phosphor::regular::PLUS)
    };
    let response = tonal_button(ui, &label).on_hover_text(if on_list {
        "On My List. Click to take it off"
    } else {
        "Put it on My List, to find it again on every device"
    });
    if response.clicked() {
        actions.push(Action::SetOnList {
            key: key.to_owned(),
            on: !on_list,
        });
    }
}

/// A button laid over the picture: dark glass and white ink, whatever the
/// theme, because what is under it is a video frame and not the page.
pub fn glass_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    filled(ui, text, Fill::Glass, Vec2::new(0.0, 0.0))
}

/// The search box in the top bar: a magnifier, the text, and a way to clear it.
///
/// Escape inside it clears it. `focus` puts the caret in it this frame — which
/// is what `Ctrl+F` and `/` ask for from anywhere in the library.
///
/// Empty and with the caret in it, it offers the `recent` searches under it.
/// Picking one fills the box; the return says the viewer asked to forget them.
pub fn search_field(
    ui: &mut egui::Ui,
    search: &mut String,
    focus: bool,
    recent: &[String],
) -> bool {
    let id = egui::Id::new("library-search");
    let width = 240.0;
    let height = ui.spacing().interact_size.y;
    let (outer, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let has_focus = ui.memory(|memory| memory.has_focus(id));
    let lit = ui
        .ctx()
        .animate_bool_with_time(id.with("lit"), has_focus, theme::motion::HOVER);

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
                "Titles, episodes, people"
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
            .hint("Clear (Esc)")
            .clicked()
        {
            search.clear();
        }
    }

    // Kept up while the pointer is over it as well as while the box has the
    // caret: pressing on it takes the caret away before the click lands.
    let popup = id.with("recent");
    let over = ui
        .data(|data| data.get_temp::<bool>(popup))
        .unwrap_or(false);
    let mut forget = false;
    if search.is_empty() && !recent.is_empty() && (response.has_focus() || over) {
        let mut picked = None;
        let shown = egui::Area::new(popup)
            .order(egui::Order::Foreground)
            .fixed_pos(outer.left_bottom() + Vec2::new(0.0, 6.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(width - 12.0);
                    ui.label(
                        theme::Role::Caption
                            .rich("Recent searches")
                            .color(theme::muted()),
                    );
                    for query in recent {
                        if ui
                            .add(
                                egui::Button::selectable(false, query.as_str())
                                    .frame_when_inactive(false),
                            )
                            .clicked()
                        {
                            picked = Some(query.clone());
                        }
                    }
                    ui.separator();
                    forget = ui
                        .add(
                            egui::Button::selectable(false, muted("Clear recent searches"))
                                .frame_when_inactive(false),
                        )
                        .clicked();
                });
            });
        let over = shown.response.contains_pointer();
        ui.data_mut(|data| data.insert_temp(popup, over && !forget && picked.is_none()));
        if let Some(query) = picked {
            *search = query;
        }
    } else {
        ui.data_mut(|data| data.remove::<bool>(popup));
    }
    forget
}

/// A thin bar in the accent, for how far something has got.
///
/// egui's own progress bar is one flat colour with the percentage written over
/// it; this is the same bar the tiles and the seek bar use, so a download reads
/// as part of the same app.
pub fn progress_bar(ui: &mut egui::Ui, fraction: f32, width: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 6.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        let radius = CornerRadius::same(theme::radius::BAR);
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
        ui.add_space(theme::space::S);
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
    confirm_with(ctx, id, heading, body, verb, true)
}

/// [`confirm`], for a question whose yes loses nothing — a large download —
/// and so is the accent rather than the danger colour.
pub fn confirm_with(
    ctx: &egui::Context,
    id: &str,
    heading: &str,
    body: &str,
    verb: &str,
    destroys: bool,
) -> Answer {
    let mut answer = Answer::Pending;
    let modal = egui::Modal::new(egui::Id::new(("confirm", id)))
        .frame(
            egui::Frame::new()
                .fill(theme::surface())
                .inner_margin(egui::Margin::same(18))
                .corner_radius(CornerRadius::same(theme::radius::LG)),
        )
        .show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(
                theme::Role::Heading
                    .rich(heading)
                    .strong()
                    .color(theme::text()),
            );
            ui.add_space(theme::space::S);
            ui.label(theme::Role::Body.rich(body).color(theme::muted()));
            ui.add_space(theme::space::XL);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let yes = if destroys {
                    danger_button(ui, verb)
                } else {
                    accent_button(ui, verb)
                };
                if yes.clicked() {
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
    /// A film of the text colour over whatever is behind it.
    Tonal,
    /// Dark glass over the picture.
    Glass,
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
        let left =
            ctx.animate_value_with_time(id.with("left"), target.left(), theme::motion::STATE);
        let right =
            ctx.animate_value_with_time(id.with("right"), target.right(), theme::motion::STATE);
        let rect = Rect::from_min_max(
            egui::pos2(left, target.top()),
            egui::pos2(right, target.bottom()),
        );
        ui.painter().set(
            pill,
            theme::accent_shape(ui.ctx(), rect, CornerRadius::same(theme::radius::MD)),
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
    let response = response.named(text);
    if !ui.is_rect_visible(rect) {
        return response;
    }

    let radius = CornerRadius::same(theme::radius::MD);
    // Eased rather than switched, so the veil arrives over a few frames instead
    // of appearing whole the moment the pointer crosses the edge. The press is
    // quicker than the hover: a click should feel like it registered, not like
    // it was considered.
    let ctx = ui.ctx().clone();
    let hover = ctx.animate_bool_with_time(
        response.id.with("hover"),
        response.hovered(),
        theme::motion::HOVER,
    );
    let press = ctx.animate_bool_with_time(
        response.id.with("press"),
        response.is_pointer_button_down_on(),
        theme::motion::PRESS,
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
        Fill::Tonal => {
            let film = if theme::palette().light {
                Color32::from_black_alpha(16)
            } else {
                Color32::from_white_alpha(22)
            };
            ui.painter().rect_filled(rect, radius, film);
            theme::veil_over(ui.painter(), rect, radius, lift);
            theme::text()
        }
        Fill::Glass => {
            ui.painter()
                .rect_filled(rect, radius, Color32::from_black_alpha(140));
            ui.painter().rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, Color32::from_white_alpha(18)),
                egui::StrokeKind::Inside,
            );
            theme::veil_over(ui.painter(), rect, radius, lift);
            Color32::WHITE
        }
    };
    // A disabled control is drawn at part strength: the ink is what says what
    // the button is, so that is what fades.
    let ink = if ui.is_enabled() {
        ink
    } else {
        ink.gamma_multiply(0.4)
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
    /// The picture's height over its width: [`theme::CARD_ASPECT`] for a
    /// still, [`theme::POSTER_ASPECT`] for a cover.
    pub aspect: f32,
}

/// Two lines of name plus one of subtitle. Fixed, because the tile is allocated
/// before the name is laid out — and because a grid whose rows are each a
/// different height by title length reads as broken.
const CARD_TEXT_HEIGHT: f32 = 54.0;

/// How tall [`card`] draws a tile of this width and shape, so a grid can step
/// over rows it does not draw.
pub fn card_height(width: f32, aspect: f32) -> f32 {
    (width * aspect).round() + CARD_TEXT_HEIGHT
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
    let image_height = (width * card.aspect).round();
    // A cover fitted into a still-shaped tile is drawn whole; into a tile its
    // own shape, it fills it like anything else.
    let fit = card.aspect < 1.0;
    let text_height = CARD_TEXT_HEIGHT;
    let radius = CornerRadius::same(theme::radius::MD);

    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, image_height + text_height), Sense::click());
    let response = if card.subtitle.is_empty() {
        response.named(card.name)
    } else {
        response.named(format!("{}, {}", card.name, card.subtitle))
    };
    // Reached with the arrow keys or Tab: bring it into view, before the
    // early return below would skip a tile that is just off screen.
    if response.gained_focus() {
        response.scroll_to_me(None);
    }
    if !ui.is_rect_visible(rect) {
        return response;
    }

    let resting = Rect::from_min_size(rect.min, Vec2::new(width, image_height));
    // Hover is a value, not a flag: the border, the lift and the shadow all
    // ride it, so they arrive together rather than each snapping on its own.
    // egui drives this from wall-clock time, so it is the same speed whatever
    // the frame rate — and it needs no state kept here.
    // Focus lifts a tile the way the pointer does, and adds a ring of its own,
    // so the keyboard can be followed across the grid.
    let hover = ui.ctx().animate_bool_with_time(
        response.id.with("hover"),
        response.hovered() || response.has_focus(),
        theme::motion::HOVER,
    );
    let focus = ui.ctx().animate_bool_with_time(
        response.id.with("focus"),
        response.has_focus(),
        theme::motion::HOVER,
    );
    // The picture grows towards the viewer into the gap around it, which is
    // what says "this one" across a shelf of near-identical rectangles. The
    // name under it stays put, so the row's text never wobbles.
    let image_rect = resting.expand2(resting.size() * lift_per_side(width) * hover);
    // The picture arrives whenever its download finishes, several frames after
    // the tile first drew. Fading it in over the placeholder turns a wall of
    // letters popping into stills into one settle.
    let art_in = ui.ctx().animate_bool_with_time(
        response.id.with("art"),
        card.art.is_some(),
        theme::motion::ARRIVE,
    );
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
        Some((texture, ArtShape::Portrait)) if fit => {
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
        Some((texture, ArtShape::Portrait)) => {
            painter.add(
                egui::epaint::RectShape::filled(image_rect, radius, ink).with_texture(
                    texture.id(),
                    cover_uv(texture.size_vec2(), image_rect.size()),
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
            CornerRadius::same(theme::radius::BADGE),
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
    if focus > 0.0 {
        painter.rect_stroke(
            image_rect.expand(3.0),
            CornerRadius::same(theme::radius::MD + 3),
            Stroke::new(2.0, theme::text().gamma_multiply(focus)),
            egui::StrokeKind::Outside,
        );
    }

    let text_rect = Rect::from_min_size(
        resting.left_bottom() + Vec2::new(0.0, 6.0),
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

/// Every sideways row in the app: the shelves on the home page and the
/// franchise on a title's page.
///
/// What runs past either edge fades into the page rather than being cut off
/// by it, so a row says there is more without a scroll bar saying it. Under
/// the pointer, a round button at each edge with more beyond it pages the row
/// by most of its width: a mouse has no sideways wheel, and the bar was the
/// only other way across.
///
/// The row is padded by half a gap all round inside its scroll area, and the
/// area reaches the same half gap past the column on either side, so a lifted
/// tile is not clipped by the area and the tiles still line up with the
/// headings above them.
pub fn shelf<R>(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let half = (theme::CARD_GAP / 2.0) as i8;
    let id = ui.id().with(("shelf", &id_salt));
    let page = id.with("page");
    let pending: Option<f32> = ui.ctx().data_mut(|data| data.remove_temp(page));
    let output = egui::Frame::new()
        .outer_margin(egui::Margin {
            left: -half,
            right: -half,
            top: -half,
            bottom: -half,
        })
        .show(ui, |ui| {
            egui::ScrollArea::horizontal()
                .id_salt(id)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    if let Some(delta) = pending {
                        ui.scroll_with_delta_animation(
                            Vec2::new(delta, 0.0),
                            egui::style::ScrollAnimation::duration(SHELF_PAGE_SECONDS),
                        );
                    }
                    egui::Frame::new()
                        .inner_margin(egui::Margin::same(half))
                        .show(ui, add)
                        .inner
                })
        })
        .inner;

    let rect = output.inner_rect;
    let offset = output.state.offset.x;
    let overflow = (output.content_size.x - rect.width()).max(0.0);
    let ends = [
        (offset > 1.0, false, egui_phosphor::regular::CARET_LEFT, 1.0),
        (
            offset < overflow - 1.0,
            true,
            egui_phosphor::regular::CARET_RIGHT,
            -1.0,
        ),
    ];
    let pointer_in = ui.rect_contains_pointer(rect);
    let painter = ui.painter_at(rect);
    for (more, right, icon, direction) in ends {
        if !more {
            continue;
        }
        let strip = if right {
            Rect::from_min_max(egui::pos2(rect.right() - SHELF_FADE, rect.top()), rect.max)
        } else {
            Rect::from_min_max(
                rect.min,
                egui::pos2(rect.left() + SHELF_FADE, rect.bottom()),
            )
        };
        painter.add(theme::fade_shape(
            ui.ctx(),
            strip,
            theme::background(),
            theme::Direction::Horizontal,
            right,
        ));

        let button_id = id.with(right);
        let shown = ui
            .ctx()
            .animate_bool_with_time(button_id, pointer_in, theme::motion::HOVER);
        if shown == 0.0 {
            continue;
        }
        let centre = egui::pos2(
            if right {
                rect.right() - SHELF_BUTTON / 2.0 - theme::space::M
            } else {
                rect.left() + SHELF_BUTTON / 2.0 + theme::space::M
            },
            rect.center().y,
        );
        let button = Rect::from_center_size(centre, Vec2::splat(SHELF_BUTTON));
        let response = ui
            .interact(button, button_id.with("button"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let response = response.named(if right { "Scroll right" } else { "Scroll left" });
        let lit = if response.hovered() { 1.0 } else { 0.0 };
        painter.circle_filled(
            centre,
            SHELF_BUTTON / 2.0,
            Color32::from_black_alpha((150.0 + 60.0 * lit) as u8).gamma_multiply(shown),
        );
        painter.text(
            centre,
            Align2::CENTER_CENTER,
            icon,
            egui::FontId::proportional(18.0),
            Color32::WHITE.gamma_multiply(shown),
        );
        if response.clicked() {
            let delta = direction * rect.width() * SHELF_PAGE;
            ui.ctx().data_mut(|data| data.insert_temp(page, delta));
            ui.ctx().request_repaint();
        }
    }
    output.inner
}

/// How wide the fade at a shelf's edge is.
const SHELF_FADE: f32 = 56.0;
/// The diameter of a shelf's paging buttons.
const SHELF_BUTTON: f32 = 40.0;
/// How much of a shelf's width one page moves: most of it, so the tile that
/// was cut by the edge is whole on the next page.
const SHELF_PAGE: f32 = 0.85;
const SHELF_PAGE_SECONDS: f32 = 0.35;

/// How far a lifted tile grows past each edge, as a fraction of its size:
/// [`theme::TILE_LIFT`], but never so far that it reaches into the gap's other
/// half, which belongs to the neighbour. A wide window makes wide tiles.
fn lift_per_side(width: f32) -> f32 {
    (theme::TILE_LIFT / 2.0).min((theme::CARD_GAP / 2.0 - 1.0) / width.max(1.0))
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
        CornerRadius::same(theme::radius::MD),
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
    columns_of(available, theme::CARD_WIDTH)
}

/// [`columns`], for cards at least `minimum` wide.
pub fn columns_of(available: f32, minimum: f32) -> Grid {
    let columns =
        (((available + theme::CARD_GAP) / (minimum + theme::CARD_GAP)).floor() as usize).max(1);
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
    #[test]
    fn a_subtitle_delay_reads_with_its_direction() {
        assert_eq!(super::format_delay(0.3), "+0.3 s");
        assert_eq!(super::format_delay(-1.2), "−1.2 s");
        assert_eq!(super::format_delay(0.0000001), "0 s");
    }

    use super::*;

    #[test]
    fn a_lifted_tile_never_reaches_its_neighbour_however_wide_the_window() {
        for available in [400.0, 1280.0, 1919.0, 2560.0, 3840.0] {
            for minimum in [theme::CARD_WIDTH, theme::POSTER_WIDTH] {
                let width = columns_of(available, minimum).width;
                assert!(width * lift_per_side(width) < theme::CARD_GAP / 2.0);
            }
        }
    }

    #[test]
    fn a_running_time_reads_in_hours_and_minutes() {
        assert_eq!(format_runtime(24), "24 min");
        assert_eq!(format_runtime(120), "2 h");
        assert_eq!(format_runtime(124), "2 h 4 min");
    }

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
