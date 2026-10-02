//! One title: the still, what it is, and every episode under it.

use pstr_core::library::{Episode, Library, Season, Title, TitleKind};

use pstr_core::metadata::TitleMetadata;

use crate::app::{Action, Page};
use crate::engine::{DownloadItem, DownloadKey, DownloadState};
use crate::playback::PlaybackTarget;
use crate::theme;
use crate::ui::{self, Art, Card};

pub struct OfflineView<'a> {
    pub downloads: &'a [DownloadItem],
    pub files: &'a std::collections::HashSet<DownloadKey>,
}

pub fn show(
    ui: &mut egui::Ui,
    art: &mut Art<'_>,
    library: &Library,
    key: &str,
    offline: OfflineView<'_>,
    actions: &mut Vec<Action>,
) {
    let Some(title) = library.get(key) else {
        // The catalog was replaced under the page — a recrawl that dropped this
        // title. Nothing to show, so go back rather than draw an empty shell.
        actions.push(Action::Goto(Page::Library));
        return;
    };

    if back_link(ui).clicked() {
        actions.push(Action::Goto(Page::Library));
    }
    ui.add_space(6.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            header(ui, art, title, &offline, actions);
            ui.add_space(18.0);

            // One season at a time, picked from a row of pills, rather than
            // every season stacked as a collapsing header: a four-season show
            // was a page of headers to scroll past to reach the one being
            // watched. The one opened first is the one with the next episode.
            let picked_id = ui.id().with(("season", &title.key));
            let default = title
                .next_up()
                .and_then(|next| {
                    title.seasons.iter().position(|season| {
                        season
                            .episodes
                            .iter()
                            .any(|episode| episode.node.link_id == next.node.link_id)
                    })
                })
                .unwrap_or(0);
            let mut picked = ui
                .data(|data| data.get_temp::<usize>(picked_id))
                .unwrap_or(default)
                .min(title.seasons.len().saturating_sub(1));

            let Some(season) = title.seasons.get(picked) else {
                return;
            };
            ui.horizontal(|ui| {
                if title.seasons.len() > 1 {
                    let labels: Vec<String> =
                        title.seasons.iter().map(|season| season.label()).collect();
                    let choices: Vec<(usize, &str)> = labels
                        .iter()
                        .enumerate()
                        .map(|(index, label)| (index, label.as_str()))
                        .collect();
                    if let Some(index) = ui::widgets::segmented(ui, picked, &choices) {
                        picked = index;
                        ui.data_mut(|data| data.insert_temp(picked_id, index));
                    }
                    ui.add_space(10.0);
                }
                ui.label(ui::muted(ui::library::plural(
                    season.episodes.len(),
                    "episode",
                )));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    season_download(ui, title, season, &offline, actions);
                });
            });
            ui.add_space(10.0);

            let Some(season) = title.seasons.get(picked) else {
                return;
            };
            for episode in &season.episodes {
                episode_row(ui, art, title, season, episode, &offline, actions);
            }
            ui.add_space(12.0);
        });
}

/// A title's poster, at the poster's own shape, with how far into it the
/// viewer is along its foot.
fn poster(ui: &mut egui::Ui, texture: &egui::TextureHandle, progress: Option<f64>) {
    let size = egui::vec2(180.0, 270.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let radius = egui::CornerRadius::same(10);
    ui.painter()
        .add(theme::tile_shadow(1.0).as_shape(rect, radius));
    ui.painter().add(
        egui::epaint::RectShape::filled(rect, radius, egui::Color32::WHITE)
            .with_texture(texture.id(), ui::cover_uv(texture.size_vec2(), size)),
    );
    if let Some(progress) = progress {
        let track = egui::Rect::from_min_max(
            egui::pos2(rect.left(), rect.bottom() - 4.0),
            rect.right_bottom(),
        );
        let painter = ui.painter().with_clip_rect(track);
        painter.rect_filled(rect, radius, egui::Color32::from_black_alpha(150));
        let mut played = rect;
        played.set_width(rect.width() * progress.clamp(0.0, 1.0) as f32);
        theme::accent_fill(&painter, played, radius, 0.0);
    }
}

/// "← Library", as a link rather than a button: it is navigation, and a
/// filled button beside the page's real actions competed with them.
fn back_link(ui: &mut egui::Ui) -> egui::Response {
    let text = theme::Role::Label.rich(format!("{}  Library", egui_phosphor::regular::ARROW_LEFT));
    let response = ui.add(egui::Label::new(text.color(theme::muted())).sense(egui::Sense::click()));
    if response.hovered() {
        ui.painter().hline(
            response.rect.x_range(),
            response.rect.bottom(),
            egui::Stroke::new(1.0, theme::muted()),
        );
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Back (Esc, Alt ←)")
}

/// Download, or delete, a whole season.
fn season_download(
    ui: &mut egui::Ui,
    title: &Title,
    season: &Season,
    offline: &OfflineView<'_>,
    actions: &mut Vec<Action>,
) {
    let season_keys: Vec<_> = season.episodes.iter().map(key_of).collect();
    let all_offline = season_keys.iter().all(|key| offline.files.contains(key));
    if all_offline {
        if ui
            .button("Delete season")
            .on_hover_text("Delete the offline copies; keep the online source")
            .clicked()
        {
            for key in season_keys {
                actions.push(Action::RemoveDownload(key, false));
            }
        }
    } else if ui
        .button("Download season")
        .on_hover_text("Download every episode in this season")
        .clicked()
    {
        actions.push(Action::MakeOffline(
            season
                .episodes
                .iter()
                .map(|episode| PlaybackTarget::new(title, episode))
                .collect(),
        ));
    }
}

/// The still, the name and the one button that matters.
fn header(
    ui: &mut egui::Ui,
    art: &mut Art<'_>,
    title: &Title,
    offline: &OfflineView<'_>,
    actions: &mut Vec<Action>,
) {
    // Cloned out of the borrow: `art` is held mutably for the card below, and
    // the description is drawn beside it.
    let record = art.metadata.get(&title.key);
    let found = record.and_then(|record| record.metadata.clone());
    let by_hand = record.is_some_and(|record| record.manual);
    // Asked for once and used twice: the same picture is the poster beside the
    // text and the banner behind all of it.
    let picture = art.of(title);

    // Reserved before the header is laid out, because the band is as tall as
    // whatever the header comes to — a fixed height would clip a long overview
    // on one title and leave a gap under a short one on the next.
    let backdrop = picture
        .as_ref()
        .map(|(texture, _)| (texture.clone(), ui.painter().add(egui::Shape::Noop)));

    ui.add_space(6.0);
    ui.horizontal_top(|ui| {
        match &picture {
            // A poster is drawn as a poster. Fitted into the 16:9 tile the grid
            // uses, it was a narrow strip of picture between two bars.
            Some((texture, pstr_core::metadata::ArtShape::Portrait)) => {
                poster(ui, texture, title.resume().and_then(|e| e.progress()));
            }
            _ => {
                ui::card(
                    ui,
                    Card {
                        art: picture,
                        name: &title.name,
                        subtitle: String::new(),
                        progress: title.resume().and_then(|e| e.progress()).map(|v| v as f32),
                        badge: None,
                        width: theme::CARD_WIDTH,
                    },
                );
            }
        }

        ui.add_space(22.0);
        ui.vertical(|ui| {
            // Text in a column a person can read, with the picture to the
            // right of it rather than under it.
            ui.set_max_width(ui.available_width().min(820.0));
            ui.label(theme::Role::Display.rich(&title.name).strong());
            // The provider's name for it, when it is not the one the files use.
            // Worth showing rather than replacing the filename's: a viewer
            // should be able to tell what the match actually matched.
            if let Some(found) = &found
                && found.name != title.name
            {
                ui.label(ui::muted(format!(
                    "also known as {}{}",
                    found.name,
                    if by_hand { " (chosen by hand)" } else { "" }
                )));
            }
            ui.add_space(4.0);
            ui.label(ui::muted(meta_line(title, found.as_ref())));
            ui.add_space(14.0);

            ui.horizontal(|ui| {
                if let Some(episode) = title.next_up() {
                    let resuming = episode.resume_at().is_some();
                    // Where it resumes, in the button that resumes it. As a
                    // label of its own it ended up three controls further
                    // along, after the download buttons, reading as a fact
                    // about those.
                    let at = episode
                        .resume_at()
                        .map(|at| format!("  ·  {}", ui::format_time(at)))
                        .unwrap_or_default();
                    let label = match (resuming, episode.numbering()) {
                        (true, Some(numbering)) => format!("Resume {numbering}{at}"),
                        (true, None) => format!("Resume{at}"),
                        (false, Some(numbering)) if title.kind == TitleKind::Series => {
                            format!("Play {numbering}")
                        }
                        _ => "Play".to_string(),
                    };
                    if ui::accent_button(ui, &label).clicked() {
                        actions.push(Action::Play(PlaybackTarget::new(title, episode)));
                    }
                    // Beside the button it is the alternative to.
                    if resuming
                        && ui
                            .button("Start over")
                            .on_hover_text("Play from the beginning")
                            .clicked()
                    {
                        actions.push(Action::Play(PlaybackTarget::from_node(
                            title,
                            &episode.node,
                            None,
                        )));
                    }
                    let title_keys: Vec<_> = title.episodes().map(key_of).collect();
                    let all_offline = title_keys.iter().all(|key| offline.files.contains(key));
                    if all_offline {
                        if ui
                            .button("Delete show")
                            .on_hover_text("Delete the offline copies; keep the online source")
                            .clicked()
                        {
                            for key in title_keys {
                                actions.push(Action::RemoveDownload(key, false));
                            }
                        }
                    } else if ui
                        .button("Download show")
                        .on_hover_text("Download every episode for disconnected playback")
                        .clicked()
                    {
                        actions.push(Action::MakeOffline(
                            title
                                .episodes()
                                .map(|episode| PlaybackTarget::new(title, episode))
                                .collect(),
                        ));
                    }
                    let active = offline
                        .downloads
                        .iter()
                        .filter(|item| {
                            item.target.title_key == title.key
                                && matches!(
                                    item.state,
                                    DownloadState::Queued
                                        | DownloadState::Running
                                        | DownloadState::Paused
                                )
                        })
                        .count();
                    if active > 0 {
                        ui.label(ui::muted(format!("{active} downloading")));
                    } else if all_offline {
                        ui.label(ui::muted("Available offline"));
                    }
                }

                // The way out of a match the scorer would not make, or made
                // wrongly. Offered whether or not anything matched: the two
                // cases it exists for are a title with no poster at all and a
                // title wearing someone else's.
                if ui
                    .button(match &found {
                        Some(_) => "Change match",
                        None => "Match…",
                    })
                    .on_hover_text("Search the metadata provider and pick the entry yourself")
                    .clicked()
                {
                    actions.push(Action::OpenMatcher(title.key.clone()));
                }
            });

            if let Some(found) = &found {
                ui.add_space(14.0);
                description(ui, found);
            }
        });
    });

    if let Some((texture, index)) = backdrop {
        ui.add_space(10.0);
        band(ui, &texture, index);
    }
}

/// The picture again, across the whole width, behind the header.
///
/// The one piece of this page that had to wait for artwork to exist at all: the
/// providers already hand back a picture and its shape, so a backdrop costs no
/// new request — only somewhere to put it.
///
/// Three shapes over one another, in this order: the picture, a flat veil of the
/// page colour so text laid over it stays readable whatever the still happens to
/// be, and a fade into the page along the bottom so the band ends rather than
/// stops.
fn band(ui: &mut egui::Ui, texture: &egui::TextureHandle, index: egui::layers::ShapeIdx) {
    // Full width of the window, not of the content: the panel's own margin is
    // what the band has to reach past, or it reads as a picture in a box.
    const BLEED: f32 = 18.0;
    let content = ui.min_rect();
    let rect = egui::Rect::from_min_max(
        egui::pos2(ui.max_rect().left() - BLEED, content.top()),
        egui::pos2(ui.max_rect().right() + BLEED, content.bottom()),
    );

    // Past the panel margin, or the bleed is trimmed off at exactly the edge it
    // exists to cross.
    let painter = ui.painter().with_clip_rect(rect);
    // Every layer of the band goes into the one slot reserved before the header
    // was laid out. Painting the veils here, after the fact, would put them over
    // the name and the overview as well as the still — which is the picture
    // being readable at the cost of the text it exists behind.
    let mut layers: Vec<egui::Shape> = vec![
        egui::epaint::RectShape::filled(rect, egui::CornerRadius::ZERO, egui::Color32::WHITE)
            .with_texture(
                texture.id(),
                super::cover_uv(texture.size_vec2(), rect.size()),
            )
            .into(),
    ];
    // Heavier than it looks: a still is arbitrary, and the one it lands on may
    // be a white frame. This is the difference between a hero and an unreadable
    // page.
    //
    // Two coats rather than one heavy one. The first is flat and near-opaque —
    // enough on its own that body text over the busiest frame still reads. The
    // second is the picture's own contrast being knocked back further behind the
    // left half, where the poster, the name and the overview actually sit; the
    // right half keeps more of the still, so the band is still a picture.
    //
    // Heavier still when there is no fade coming, since then these are the only
    // things standing between the text and the picture.
    let ramps = theme::ramps_on();
    layers.push(
        egui::epaint::RectShape::filled(
            rect,
            egui::CornerRadius::ZERO,
            // Lighter with the ramps on: the horizontal one below is solid
            // where the text is, so this coat only has to calm the picture's
            // right side, not make it disappear.
            theme::background().gamma_multiply(if ramps { 0.72 } else { 0.94 }),
        )
        .into(),
    );
    if ramps {
        let text_side = egui::Rect::from_min_max(
            rect.min,
            egui::pos2(rect.left() + rect.width() * 0.72, rect.bottom()),
        );
        layers.push(theme::fade_shape(
            ui.ctx(),
            text_side,
            theme::background(),
            theme::Direction::Horizontal,
            // Solid at the left, where the poster and the text are.
            false,
        ));
    }

    // A viewer with ramps off is on a panel that bands them; a 120-point ramp
    // of one hue is exactly the case that shows it. The band then ends at its
    // own edge, which is the honest version of the same thing.
    if ramps {
        let fade =
            egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 120.0), rect.max);
        layers.push(theme::fade_shape(
            ui.ctx(),
            fade,
            theme::background(),
            theme::Direction::Vertical,
            // Solid at the bottom, where the page it hands over to is.
            true,
        ));
    }

    painter.set(index, egui::Shape::Vec(layers));
}

/// What the provider had to say. Only ever drawn under a match.
fn description(ui: &mut egui::Ui, found: &TitleMetadata) {
    if !found.genres.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for genre in &found.genres {
                ui::widgets::chip(ui, genre);
            }
        });
        ui.add_space(8.0);
    }
    if let Some(overview) = &found.overview {
        // Four lines, then "More". A provider's synopsis runs to three
        // paragraphs on a long show, and all of them pushed the episodes —
        // what the page is for — below the fold.
        const LINES: usize = 4;
        let id = ui.id().with(("overview", &found.remote_id));
        let open = ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(false);
        let mut job = egui::text::LayoutJob::simple(
            overview.clone(),
            theme::Role::Label.font(),
            theme::text(),
            ui.available_width(),
        );
        if !open {
            job.wrap.max_rows = LINES;
            job.wrap.overflow_character = Some('…');
        }
        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
        let clipped = galley.elided;
        ui.label(galley);
        if clipped || open {
            let toggle = ui.add(
                egui::Label::new(
                    theme::Role::Caption
                        .rich(if open { "Less" } else { "More" })
                        .color(theme::accent()),
                )
                .sense(egui::Sense::click()),
            );
            if toggle
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                ui.data_mut(|data| data.insert_temp(id, !open));
            }
        }
    }
    if let Some(url) = &found.url {
        ui.add_space(8.0);
        ui.hyperlink_to(
            ui::muted(format!("More on {}", found.provider.label())),
            url,
        );
    }
}

/// The grey line under the name: what the library knows, plus what the provider
/// added. The library's own counts come first — they describe the files that are
/// actually there, which is what a viewer is deciding about.
fn meta_line(title: &Title, found: Option<&TitleMetadata>) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(
        match title.kind {
            TitleKind::Series => "Series",
            TitleKind::Film => "Film",
        }
        .to_string(),
    );
    if let Some(year) = title.year {
        parts.push(year.to_string());
    }
    if title.kind == TitleKind::Series {
        if title.seasons.len() > 1 {
            parts.push(ui::library::plural(title.seasons.len(), "season"));
        }
        parts.push(ui::library::plural(title.episode_count(), "episode"));
    }
    let watched = title.watched_count();
    if watched > 0 {
        parts.push(format!("{watched} watched"));
    }

    if let Some(found) = found {
        if let Some(rating) = found.rating {
            parts.push(format!("{} {rating:.1}", egui_phosphor::regular::STAR));
        }
        // Only when it disagrees with what is on disk: "25 episodes · 25
        // episodes" tells nobody anything, but "12 episodes · 25 on AniList"
        // says the share is missing half a season.
        if let Some(total) = found.episodes
            && title.kind == TitleKind::Series
            && total as usize != title.episode_count()
        {
            parts.push(format!("{total} on {}", found.provider.label()));
        }
    }
    parts.join("  ·  ")
}

/// How big an episode's still is drawn.
const STILL: egui::Vec2 = egui::vec2(160.0, 90.0);

/// One row per file: its still, its name and what it is about, and the two
/// things that can be done to it besides playing it.
fn episode_row(
    ui: &mut egui::Ui,
    art: &mut Art<'_>,
    title: &Title,
    season: &Season,
    episode: &Episode,
    offline: &OfflineView<'_>,
    actions: &mut Vec<Action>,
) {
    // Cloned out before `ui` borrows: the row draws while `art` is held.
    let found = art.episode(&title.key, episode).map(|found| {
        (
            found.name.clone(),
            found.overview.clone(),
            found.air_date.clone(),
        )
    });
    let watched = episode.is_watched();
    let row_id = ui.id().with(("episode", &episode.node.link_id));

    let frame = egui::Frame::new()
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::same(8));
    let mut prepared = frame.begin(ui);
    {
        let ui = &mut prepared.content_ui;
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            still(ui, art, title, episode, watched, actions);
            ui.add_space(6.0);

            let controls = 96.0;
            let text_width = (ui.available_width() - controls).max(120.0);
            ui.vertical(|ui| {
                ui.set_width(text_width);
                ui.spacing_mut().item_spacing.y = 3.0;

                // The provider's name for the episode, when there is one:
                // "The Immortal Legion" reads as an episode, and
                // "[Reaktor] … E57 v2 [1080p][x265].mkv" reads as a filename.
                let numbering = episode
                    .numbering()
                    .or_else(|| season.number.map(|number| format!("S{number:02}")))
                    .unwrap_or_default();
                let name = found
                    .as_ref()
                    .and_then(|(name, _, _)| name.clone())
                    .unwrap_or_else(|| episode.detail().to_string());
                let mut job = egui::text::LayoutJob::default();
                if !numbering.is_empty() {
                    job.append(
                        &format!("{numbering}   "),
                        0.0,
                        egui::TextFormat::simple(egui::FontId::monospace(13.0), theme::muted()),
                    );
                }
                job.append(
                    &name,
                    0.0,
                    egui::TextFormat::simple(
                        theme::Role::Body.font(),
                        if watched {
                            theme::muted()
                        } else {
                            theme::text()
                        },
                    ),
                );
                job.wrap.max_rows = 1;
                job.wrap.overflow_character = Some('…');
                ui.add(egui::Label::new(job).truncate())
                    .on_hover_text(&episode.node.name);

                ui.label(ui::muted(details(
                    episode,
                    found.as_ref().and_then(|f| f.2.as_deref()),
                )));

                if let Some((_, Some(overview), _)) = &found {
                    let mut job = egui::text::LayoutJob::simple(
                        overview.clone(),
                        theme::Role::Caption.font(),
                        theme::muted(),
                        text_width,
                    );
                    job.wrap.max_rows = 2;
                    job.wrap.overflow_character = Some('…');
                    ui.add(egui::Label::new(job)).on_hover_text(overview);
                }
            });

            // A fixed height, the still's. `with_layout` would hand the
            // buttons all the height left in the scroll area's viewport and
            // centre them in it, so whichever row sat highest on screen
            // stretched down to the bottom of the window.
            let cluster = egui::vec2(ui.available_width(), STILL.y);
            let layout = egui::Layout::right_to_left(egui::Align::Center);
            ui.allocate_ui_with_layout(cluster, layout, |ui| {
                if ui::widgets::watched_mark(ui, watched)
                    .on_hover_text(if watched {
                        "Mark unwatched"
                    } else {
                        "Mark watched"
                    })
                    .clicked()
                {
                    actions.push(Action::SetWatched {
                        share_id: episode.node.share_id.clone(),
                        link_id: episode.node.link_id.clone(),
                        watched: !watched,
                        duration: episode.watch.and_then(|watch| watch.duration_secs),
                    });
                }
                ui.add_space(6.0);
                download(ui, title, episode, offline, actions);
            });
        });
    }
    // The row lights up under the pointer, so it is clear which file the
    // buttons on the right belong to in a list of thirty.
    let response = prepared.allocate_space(ui);
    let lit = ui.ctx().animate_bool_with_time(
        row_id,
        response.hovered() || response.contains_pointer(),
        0.10,
    );
    prepared.frame.fill = theme::card().gamma_multiply(0.35 + 0.65 * lit);
    prepared.paint(ui);
    ui.add_space(2.0);
}

/// The grey line under an episode's name: when it aired, how big it is, and
/// where it would resume.
fn details(episode: &Episode, air_date: Option<&str>) -> String {
    let mut parts = Vec::new();
    if let Some(at) = episode.resume_at() {
        let left = episode
            .watch
            .and_then(|watch| watch.duration_secs)
            .map(|duration| format!(" · {} left", ui::format_time(duration - at)))
            .unwrap_or_default();
        parts.push(format!("Resume at {}{left}", ui::format_time(at)));
    } else if episode.is_watched() {
        parts.push("Watched".into());
    }
    if let Some(date) = air_date {
        parts.push(date.to_owned());
    }
    match episode.node.size {
        // A file the share says is empty is one that will not play — an upload
        // that never finished, usually. Worth saying on the row rather than
        // only when a click on it comes back with "has no content".
        Some(0) => parts.push("empty — this file cannot be played".into()),
        Some(size) => parts.push(ui::format_size(size)),
        None => {}
    }
    parts.join("  ·  ")
}

/// The still, which is also the play button.
///
/// An explicit target rather than a clickable row: the row also carries the
/// watched mark and the download button, and a click that meant either of
/// those must never be one that starts a 1.4 GiB stream instead.
fn still(
    ui: &mut egui::Ui,
    art: &mut Art<'_>,
    title: &Title,
    episode: &Episode,
    watched: bool,
    actions: &mut Vec<Action>,
) {
    let (rect, response) = ui.allocate_exact_size(STILL, egui::Sense::click());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let radius = egui::CornerRadius::same(8);
    let painter = ui.painter();
    painter.rect_filled(rect, radius, theme::card_hover());
    // Asked for only once the row is on screen: a season of twenty-five is
    // twenty-five requests, and most of them are never scrolled to.
    if let Some(texture) = art.still(&title.key, episode) {
        let tint = if watched {
            egui::Color32::from_gray(150)
        } else {
            egui::Color32::WHITE
        };
        painter.add(
            egui::epaint::RectShape::filled(rect, radius, tint)
                .with_texture(texture.id(), ui::cover_uv(texture.size_vec2(), rect.size())),
        );
    }

    if response.gained_focus() {
        response.scroll_to_me(None);
    }
    let hover = ui.ctx().animate_bool_with_time(
        response.id,
        response.hovered() || response.has_focus(),
        0.12,
    );
    if hover > 0.0 {
        painter.rect_filled(
            rect,
            radius,
            egui::Color32::from_black_alpha((90.0 * hover) as u8),
        );
    }
    if response.has_focus() {
        painter.rect_stroke(
            rect,
            radius,
            egui::Stroke::new(2.0, theme::accent()),
            egui::StrokeKind::Inside,
        );
    }
    // A play mark, always faintly there and full under the pointer.
    let centre = rect.center();
    painter.circle_filled(
        centre,
        18.0,
        egui::Color32::from_black_alpha((110.0 + 80.0 * hover) as u8),
    );
    let ink = egui::Color32::WHITE.gamma_multiply(0.75 + 0.25 * hover);
    let r = 7.0;
    painter.add(egui::Shape::convex_polygon(
        vec![
            centre + egui::vec2(-r * 0.6, -r),
            centre + egui::vec2(r, 0.0),
            centre + egui::vec2(-r * 0.6, r),
        ],
        ink,
        egui::Stroke::NONE,
    ));

    if let Some(progress) = episode.progress() {
        let track = egui::Rect::from_min_max(
            egui::pos2(rect.left(), rect.bottom() - 4.0),
            rect.right_bottom(),
        );
        let painter = painter.with_clip_rect(track);
        painter.rect_filled(rect, radius, egui::Color32::from_black_alpha(150));
        let mut played = rect;
        played.set_width(rect.width() * progress.clamp(0.0, 1.0) as f32);
        theme::accent_fill(&painter, played, radius, 0.0);
    }

    if response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if episode.resume_at().is_some() {
            "Resume"
        } else {
            "Play"
        })
        .clicked()
    {
        actions.push(Action::Play(PlaybackTarget::new(title, episode)));
    }
}

/// One button that is the file's whole offline story: download it, see it
/// coming, pause it, or delete the copy.
fn download(
    ui: &mut egui::Ui,
    title: &Title,
    episode: &Episode,
    offline: &OfflineView<'_>,
    actions: &mut Vec<Action>,
) {
    use ui::widgets::{DownloadGlyph, download_button};

    let key = key_of(episode);
    let current = offline.downloads.iter().find(|item| item.key == key);
    if offline.files.contains(&key) {
        if download_button(ui, DownloadGlyph::Done)
            .on_hover_text("Available offline. Click to delete the copy; the share keeps the file.")
            .clicked()
        {
            actions.push(Action::RemoveDownload(key, false));
        }
        return;
    }
    match current.map(|item| (&item.state, item.percent())) {
        Some((DownloadState::Running | DownloadState::Queued, fraction)) => {
            if download_button(ui, DownloadGlyph::Progress(fraction))
                .on_hover_text(format!(
                    "Downloading, {:.0}%. Click to pause.",
                    fraction * 100.0
                ))
                .clicked()
            {
                actions.push(Action::PauseDownload(key));
            }
        }
        Some((
            DownloadState::Paused | DownloadState::Cancelled | DownloadState::Failed(_),
            fraction,
        )) => {
            if download_button(ui, DownloadGlyph::Paused(fraction))
                .on_hover_text(format!(
                    "Stopped at {:.0}%. Click to carry on.",
                    fraction * 100.0
                ))
                .clicked()
            {
                actions.push(Action::ResumeDownload(key));
            }
        }
        Some((DownloadState::Completed, _)) => {
            download_button(ui, DownloadGlyph::Done).on_hover_text("Available offline");
        }
        None => {
            if download_button(ui, DownloadGlyph::Download)
                .on_hover_text("Download, to play without a connection")
                .clicked()
            {
                actions.push(Action::MakeOffline(vec![PlaybackTarget::new(
                    title, episode,
                )]));
            }
        }
    }
}

fn key_of(episode: &Episode) -> DownloadKey {
    DownloadKey {
        share_id: episode.node.share_id.clone(),
        link_id: episode.node.link_id.clone(),
    }
}
