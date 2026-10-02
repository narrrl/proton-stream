//! The library page: what to watch, as a wall of stills.

use pstr_core::library::{Library, Title, TitleKind};

use crate::app::{Action, Filter, LibraryView, Page, Sort};
use crate::playback::PlaybackTarget;
use crate::theme;
use crate::ui::Art;
use crate::ui::{self, Card};

/// What the page lists, and what it was asked for.
pub struct Shelves<'a> {
    pub library: &'a Library,
    /// The search and the shelf, already worked out — see [`LibraryView`].
    pub view: &'a LibraryView,
    /// Whether the catalog has been read yet.
    pub loaded: bool,
    pub search: &'a str,
}

pub fn show(ui: &mut egui::Ui, art: &mut Art<'_>, shelves: Shelves<'_>, actions: &mut Vec<Action>) {
    let Shelves {
        library,
        view,
        loaded,
        search,
    } = shelves;
    if !loaded {
        return placeholder(ui);
    }
    if library.is_empty() {
        return empty_state(ui, actions);
    }

    let pick = |indices: &[usize]| -> Vec<&Title> {
        indices
            .iter()
            .filter_map(|&index| library.titles.get(index))
            .collect()
    };
    let matches = pick(&view.matches);
    let searching = !search.trim().is_empty();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if !searching {
                if let Some(title) = featured(library, view, art.metadata, today()) {
                    hero(ui, art, title, actions);
                    ui.add_space(theme::space::XL);
                }
                let resumable = pick(&view.resumable);
                if !resumable.is_empty() {
                    ui::section(ui, "Continue watching");
                    continue_row(ui, art, &resumable, actions);
                    ui.add_space(theme::space::XL);
                }
            }

            ui.horizontal(|ui| {
                ui::section(
                    ui,
                    &if searching {
                        format!(
                            "{} matching {:?}",
                            plural(matches.len(), "title"),
                            search.trim()
                        )
                    } else {
                        plural(matches.len(), "title")
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    shelf_controls(ui, view, actions);
                });
            });
            ui.add_space(theme::space::S);

            if matches.is_empty() {
                ui.add_space(theme::space::M);
                ui.label(ui::muted(if searching {
                    "Nothing here by that name."
                } else {
                    "Nothing in the library fits that filter."
                }));
                return;
            }

            grid(ui, art, &matches, actions);
        });
}

/// Days since the epoch: what the featured title turns over on.
fn today() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() / 86_400)
}

/// The title the banner shows: whatever was watched last, so the top of the
/// page is one click from carrying on. With nothing part-watched, one of the
/// titles a provider has a backdrop for, turning over once a day — the same
/// one every time the page is opened today, rather than a new one per frame.
fn featured<'a>(
    library: &'a Library,
    view: &LibraryView,
    metadata: &std::collections::HashMap<String, pstr_core::metadata::MetadataRecord>,
    day: u64,
) -> Option<&'a Title> {
    if let Some(title) = view
        .resumable
        .first()
        .and_then(|&index| library.titles.get(index))
    {
        return Some(title);
    }
    let pictured: Vec<&Title> = library
        .titles
        .iter()
        .filter(|title| {
            metadata
                .get(&title.key)
                .and_then(|record| record.metadata.as_ref())
                .is_some_and(|found| found.backdrop_url.is_some())
        })
        .collect();
    if pictured.is_empty() {
        return None;
    }
    pictured
        .get((day % pictured.len() as u64) as usize)
        .copied()
}

/// The banner across the top of the library: one title, large, with what it
/// is and a way to play it.
fn hero(ui: &mut egui::Ui, art: &mut Art<'_>, title: &Title, actions: &mut Vec<Action>) {
    let height = (ui.ctx().content_rect().height() * 0.42).clamp(220.0, 360.0);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    if !ui.is_rect_visible(rect) {
        return;
    }
    let radius = egui::CornerRadius::same(theme::radius::LG);
    let picture = art.of(title);
    let found = art
        .metadata
        .get(&title.key)
        .and_then(|record| record.metadata.clone());

    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, radius, theme::card());
    let art_in = ui.ctx().animate_bool_with_time(
        response.id.with("art"),
        picture.is_some(),
        theme::motion::ARRIVE,
    );
    if art_in < 1.0 {
        ui::placeholder(&painter, rect, "", 1.0 - art_in);
    }
    if let Some((texture, _)) = &picture {
        painter.add(
            egui::epaint::RectShape::filled(
                rect,
                radius,
                egui::Color32::WHITE.gamma_multiply(art_in),
            )
            .with_texture(texture.id(), ui::cover_uv(texture.size_vec2(), rect.size())),
        );
    }
    // The same two coats as the title page's band, for the same reason: a
    // still is arbitrary, and the text over it has to read whatever it is.
    if theme::ramps_on() {
        painter.rect_filled(rect, radius, theme::background().gamma_multiply(0.25));
        let text_side = egui::Rect::from_min_max(
            rect.min,
            egui::pos2(rect.left() + rect.width() * 0.7, rect.bottom()),
        );
        painter.add(theme::fade_shape(
            ui.ctx(),
            text_side,
            theme::background(),
            theme::Direction::Horizontal,
            false,
        ));
        let bottom =
            egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 90.0), rect.max);
        painter.add(theme::fade_shape(
            ui.ctx(),
            bottom,
            theme::background(),
            theme::Direction::Vertical,
            true,
        ));
    } else {
        painter.rect_filled(rect, radius, theme::background().gamma_multiply(0.82));
    }

    let inner = egui::Rect::from_min_max(
        rect.min + egui::vec2(32.0, 0.0),
        egui::pos2(
            rect.left() + (rect.width() * 0.55).max(360.0),
            rect.bottom() - 28.0,
        ),
    );
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::bottom_up(egui::Align::Min)),
        |ui| {
            ui.horizontal(|ui| {
                if let Some(episode) = title.next_up() {
                    let label = match (title.resume(), episode.numbering()) {
                        (Some(_), Some(numbering)) => {
                            format!("{}  Resume {numbering}", egui_phosphor::regular::PLAY)
                        }
                        (Some(_), None) => format!("{}  Resume", egui_phosphor::regular::PLAY),
                        (None, _) => format!("{}  Play", egui_phosphor::regular::PLAY),
                    };
                    if ui::accent_button(ui, &label).clicked() {
                        actions.push(Action::Play(PlaybackTarget::new(title, episode)));
                    }
                    ui.add_space(theme::space::S);
                }
                if ui.button("More info").clicked() {
                    actions.push(Action::Goto(Page::Title(title.key.clone())));
                }
            });
            ui.add_space(theme::space::L);
            if let Some(overview) = found.as_ref().and_then(|found| found.overview.as_deref()) {
                let mut job = egui::text::LayoutJob::simple(
                    overview.to_owned(),
                    theme::Role::Body.font(),
                    theme::text(),
                    ui.available_width(),
                );
                job.wrap.max_rows = 3;
                ui.label(job);
                ui.add_space(theme::space::M);
            }
            let mut facts = vec![subtitle(title)];
            if let Some(found) = &found {
                if let Some(rating) = found.rating {
                    facts.push(format!("{} {rating:.1}", egui_phosphor::regular::STAR));
                }
                facts.extend(found.genres.iter().take(3).cloned());
            }
            ui.label(ui::muted(facts.join("  ·  ")));
            ui.add_space(theme::space::XXS);
            ui.add(
                egui::Label::new(
                    theme::Role::Display
                        .rich(&title.name)
                        .strong()
                        .color(theme::text()),
                )
                .truncate(),
            );
        },
    );

    if response.clicked() {
        actions.push(Action::Goto(Page::Title(title.key.clone())));
    }
}

/// The filter pills and the order, at the right of the grid's heading.
fn shelf_controls(ui: &mut egui::Ui, view: &LibraryView, actions: &mut Vec<Action>) {
    // Right to left: the order first, so it ends up last.
    let sort = egui::ComboBox::from_id_salt("library-sort")
        .selected_text(match view.sort {
            Sort::Name => "A – Z",
            Sort::Recent => "Recently watched",
            Sort::Year => "Newest",
        })
        .width(150.0)
        .show_ui(ui, |ui| {
            let mut picked = None;
            for (sort, label) in [
                (Sort::Name, "A – Z"),
                (Sort::Recent, "Recently watched"),
                (Sort::Year, "Newest"),
            ] {
                if ui.selectable_label(view.sort == sort, label).clicked() {
                    picked = Some(sort);
                }
            }
            picked
        });
    if let Some(Some(sort)) = sort.inner {
        actions.push(Action::SetShelf(view.filter, sort));
    }
    ui.add_space(theme::space::L);
    if let Some(filter) = ui::widgets::segmented(
        ui,
        view.filter,
        &[
            (Filter::All, "All"),
            (Filter::Series, "Series"),
            (Filter::Films, "Films"),
            (Filter::Watching, "Watching"),
            (Filter::Unwatched, "Not started"),
        ],
    ) {
        actions.push(Action::SetShelf(filter, view.sort));
    }
}

/// The top shelf: one card per part-watched title, scrolling sideways.
fn continue_row(
    ui: &mut egui::Ui,
    art: &mut Art<'_>,
    titles: &[&Title],
    actions: &mut Vec<Action>,
) {
    egui::ScrollArea::horizontal()
        .id_salt("continue")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for title in titles {
                    let Some(episode) = title.resume() else {
                        continue;
                    };
                    let remaining = episode
                        .watch
                        .and_then(|watch| watch.duration_secs)
                        .map(|duration| {
                            format!(
                                "{} left",
                                ui::format_time(
                                    duration - episode.watch.map_or(0.0, |w| w.position_secs)
                                )
                            )
                        })
                        .unwrap_or_else(|| episode.label());

                    let response = ui::card(
                        ui,
                        Card {
                            art: art.of(title),
                            name: &title.name,
                            subtitle: remaining,
                            progress: episode.progress().map(|value| value as f32),
                            badge: episode.numbering(),
                            // A shelf that scrolls sideways has no right edge
                            // to reach, so nothing to flex to.
                            width: theme::CARD_WIDTH,
                        },
                    );
                    tile_menu(&response, title, true, actions);

                    if response.clicked() {
                        actions.push(Action::Goto(Page::Title(title.key.clone())));
                    }
                }
            });
        });
}

/// Every title, wrapped to the window.
fn grid(ui: &mut egui::Ui, art: &mut Art<'_>, titles: &[&Title], actions: &mut Vec<Action>) {
    let grid = ui::columns(ui.available_width());
    let row_height = ui::card_height(grid.width);
    for row in titles.chunks(grid.columns) {
        // A row scrolled out of view is only its height. Laying it out anyway
        // costs a subtitle and an artwork lookup per card per frame, and while
        // a film plays under the transport bar a frame is every frame.
        let slot = egui::Rect::from_min_size(
            ui.cursor().min,
            egui::vec2(ui.available_width(), row_height),
        );
        if !ui.is_rect_visible(slot) {
            // The spacing `horizontal` would have added after itself, too, or
            // the page grows and shrinks under the scroll bar as rows come and
            // go.
            ui.add_space(row_height + ui.spacing().item_spacing.y + theme::CARD_GAP);
            continue;
        }
        ui.horizontal(|ui| {
            // The gap the width was divided around, so what is drawn matches
            // what was measured. egui's default item spacing is narrower, and
            // the difference times the column count is a visible drift towards
            // the left edge.
            ui.spacing_mut().item_spacing.x = theme::CARD_GAP;
            for title in row {
                let response = ui::card(
                    ui,
                    Card {
                        art: art.of(title),
                        name: &title.name,
                        subtitle: subtitle(title),
                        progress: title.resume().and_then(|e| e.progress()).map(|v| v as f32),
                        // What a film is says itself in the subtitle below the
                        // card; a second `Film` over the poster is noise.
                        badge: None,
                        width: grid.width,
                    },
                );
                tile_menu(&response, title, false, actions);

                if response.clicked() {
                    actions.push(Action::Goto(Page::Title(title.key.clone())));
                }
            }
        });
        ui.add_space(theme::CARD_GAP);
    }
}

/// What a right-click on a tile offers: the things otherwise one page away.
///
/// `continuing` is the Continue watching shelf, where the tile can also be
/// taken off the shelf.
fn tile_menu(
    response: &egui::Response,
    title: &Title,
    continuing: bool,
    actions: &mut Vec<Action>,
) {
    response.context_menu(|ui| {
        if let Some(episode) = title.next_up() {
            let label = match (title.resume(), episode.numbering()) {
                (Some(_), Some(numbering)) => format!("Resume {numbering}"),
                (Some(_), None) => "Resume".to_owned(),
                (None, _) => "Play".to_owned(),
            };
            if ui.button(label).clicked() {
                actions.push(Action::Play(PlaybackTarget::new(title, episode)));
            }
        }
        if ui.button("Open").clicked() {
            actions.push(Action::Goto(Page::Title(title.key.clone())));
        }
        ui.separator();

        let all_watched = title.watched_count() == title.episode_count();
        if ui
            .button(if all_watched {
                "Mark unwatched"
            } else {
                "Mark watched"
            })
            .clicked()
        {
            actions.push(Action::SetTitleWatched {
                key: title.key.clone(),
                watched: !all_watched,
            });
        }
        if continuing
            && let Some(episode) = title.resume()
            && ui
                .button("Remove from Continue watching")
                .on_hover_text("Forget where you stopped in it")
                .clicked()
        {
            actions.push(Action::ForgetPosition {
                share_id: episode.node.share_id.clone(),
                link_id: episode.node.link_id.clone(),
            });
        }

        let count = title.episode_count();
        let download = if count == 1 {
            "Download".to_owned()
        } else {
            format!("Download all {count}")
        };
        if ui.button(download).clicked() {
            actions.push(Action::MakeOffline(
                title
                    .episodes()
                    .map(|episode| PlaybackTarget::new(title, episode))
                    .collect(),
            ));
        }
        ui.separator();
        if ui.button("Change match…").clicked() {
            actions.push(Action::OpenMatcher(title.key.clone()));
        }
    });
}

/// The grey line under a card: what it is, how much there is, and how much is
/// left.
///
/// A film says so, and says its year after it. The year alone was ambiguous in
/// the one place it mattered: `Ghost in the Shell` over `1995` reads as a
/// season, an episode count, anything — where every series card on the same
/// shelf is counting episodes.
fn subtitle(title: &Title) -> String {
    if title.kind == TitleKind::Film {
        return match title.year {
            Some(year) => format!("Film  ·  {year}"),
            None => "Film".into(),
        };
    }
    let total = title.episode_count();
    let watched = title.watched_count();
    if watched == 0 {
        plural(total, "episode")
    } else if watched >= total {
        format!("{} · watched", plural(total, "episode"))
    } else {
        format!("{watched} of {total} watched")
    }
}

/// The page before the catalog has been read: the shape of a grid, with
/// nothing in it yet.
///
/// Not the empty state. The catalog is on disk and is read in a fraction of a
/// second, and telling every launch "Nothing in the library yet" for that
/// fraction is a flash of something false.
fn placeholder(ui: &mut egui::Ui) {
    let grid = ui::columns(ui.available_width());
    let height = ui::card_height(grid.width);
    ui::section(ui, "Library");
    for _ in 0..2 {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = theme::CARD_GAP;
            for _ in 0..grid.columns {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(grid.width, height), egui::Sense::hover());
                let image = egui::Rect::from_min_size(
                    rect.min,
                    egui::vec2(grid.width, (grid.width * theme::CARD_ASPECT).round()),
                );
                ui.painter().rect_filled(
                    image,
                    egui::CornerRadius::same(theme::radius::MD),
                    theme::card(),
                );
            }
        });
        ui.add_space(theme::CARD_GAP);
    }
}

fn empty_state(ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    ui.vertical_centered(|ui| {
        ui.add_space(120.0);
        ui.label(
            theme::Role::Title
                .rich("Nothing in the library yet")
                .strong(),
        );
        ui.add_space(theme::space::S);
        ui.label(ui::muted(
            "Add a Proton Drive share link, and everything playable behind it shows up here.",
        ));
        ui.add_space(theme::space::XL);
        if ui::accent_button(ui, "Add a share").clicked() {
            actions.push(Action::Goto(Page::Shares));
        }
    });
}

pub fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn title(kind: TitleKind, year: Option<u32>) -> Title {
        Title {
            key: "k".into(),
            name: "Ghost in the Shell".into(),
            year,
            kind,
            seasons: Vec::new(),
            share_ids: Vec::new(),
        }
    }

    /// A film says it is one. The year on its own was read as anything but —
    /// the cards beside it are all counting episodes.
    #[test]
    fn a_film_is_labelled_a_film_and_dated_after_it() {
        assert_eq!(
            subtitle(&title(TitleKind::Film, Some(1995))),
            "Film  ·  1995"
        );
        assert_eq!(subtitle(&title(TitleKind::Film, None)), "Film");
        assert_eq!(
            subtitle(&title(TitleKind::Series, Some(1995))),
            "0 episodes"
        );
    }

    #[test]
    fn the_banner_features_the_title_watched_last_and_nothing_without_a_picture() {
        let mut second = title(TitleKind::Film, None);
        second.key = "second".into();
        let library = Library {
            titles: vec![title(TitleKind::Film, None), second],
        };
        let metadata = std::collections::HashMap::new();
        let mut view = LibraryView::default();
        assert!(featured(&library, &view, &metadata, 0).is_none());
        view.resumable = vec![1];
        assert_eq!(
            featured(&library, &view, &metadata, 0).map(|title| title.key.as_str()),
            Some("second")
        );
    }

    #[test]
    fn plurals_read_naturally() {
        assert_eq!(plural(1, "title"), "1 title");
        assert_eq!(plural(0, "title"), "0 titles");
        assert_eq!(plural(12, "episode"), "12 episodes");
    }
}
