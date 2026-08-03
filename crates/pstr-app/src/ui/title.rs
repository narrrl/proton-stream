//! One title: the still, what it is, and every episode under it.

use pstr_core::library::{Episode, Library, Season, Title, TitleKind};

use pstr_core::metadata::TitleMetadata;

use crate::app::{Action, Page};
use crate::playback::PlaybackTarget;
use crate::theme;
use crate::ui::{self, Art, Card};

pub fn show(
    ui: &mut egui::Ui,
    art: &mut Art<'_>,
    library: &Library,
    key: &str,
    actions: &mut Vec<Action>,
) {
    let Some(title) = library.get(key) else {
        // The catalog was replaced under the page — a recrawl that dropped this
        // title. Nothing to show, so go back rather than draw an empty shell.
        actions.push(Action::Goto(Page::Library));
        return;
    };

    if ui.button("Library").clicked() {
        actions.push(Action::Goto(Page::Library));
    }
    ui.add_space(10.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            header(ui, art, title, actions);
            ui.add_space(18.0);

            for (index, season) in title.seasons.iter().enumerate() {
                let open = index == 0 || title.seasons.len() == 1;
                egui::CollapsingHeader::new(
                    egui::RichText::new(format!(
                        "{}  ·  {}",
                        season.label(),
                        ui::library::plural(season.episodes.len(), "episode")
                    ))
                    .size(15.0)
                    .strong(),
                )
                .id_salt(("season", index))
                .default_open(open)
                .show(ui, |ui| {
                    for episode in &season.episodes {
                        episode_row(ui, art, title, season, episode, actions);
                    }
                });
                ui.add_space(6.0);
            }
        });
}

/// The still, the name and the one button that matters.
fn header(ui: &mut egui::Ui, art: &mut Art<'_>, title: &Title, actions: &mut Vec<Action>) {
    // Cloned out of the borrow: `art` is held mutably for the card below, and
    // the description is drawn beside it.
    let record = art.metadata.get(&title.key);
    let found = record.and_then(|record| record.metadata.clone());
    let by_hand = record.is_some_and(|record| record.manual);

    ui.horizontal_top(|ui| {
        ui::card(
            ui,
            Card {
                art: art.of(title),
                name: &title.name,
                subtitle: String::new(),
                progress: title.resume().and_then(|e| e.progress()).map(|v| v as f32),
                badge: None,
            },
        );

        ui.add_space(18.0);
        ui.vertical(|ui| {
            ui.label(egui::RichText::new(&title.name).size(26.0).strong());
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
                    let label = match (resuming, episode.numbering()) {
                        (true, Some(numbering)) => format!("Resume {numbering}"),
                        (true, None) => "Resume".to_string(),
                        (false, Some(numbering)) if title.kind == TitleKind::Series => {
                            format!("Play {numbering}")
                        }
                        _ => "Play".to_string(),
                    };
                    if ui::accent_button(ui, &label).clicked() {
                        actions.push(Action::Play(PlaybackTarget::new(title, episode)));
                    }
                    if resuming && let Some(at) = episode.resume_at() {
                        ui.label(ui::muted(format!("at {}", ui::format_time(at))));
                        if ui
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
}

/// What the provider had to say. Only ever drawn under a match.
fn description(ui: &mut egui::Ui, found: &TitleMetadata) {
    if !found.genres.is_empty() {
        ui.label(ui::muted(found.genres.join(" · ")));
        ui.add_space(6.0);
    }
    if let Some(overview) = &found.overview {
        ui.add(
            egui::Label::new(
                egui::RichText::new(overview)
                    .size(13.0)
                    .color(theme::text()),
            )
            .wrap(),
        );
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
            parts.push(format!("★ {rating:.1}"));
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

/// One line per file: click anywhere on it to play.
fn episode_row(
    ui: &mut egui::Ui,
    art: &mut Art<'_>,
    title: &Title,
    season: &Season,
    episode: &Episode,
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
    egui::Frame::new()
        .fill(if watched {
            theme::background()
        } else {
            theme::card()
        })
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                // An explicit target rather than a clickable row: the row also
                // carries a checkbox, and a click that means "seen" must never
                // be one that starts a 1.4 GiB stream instead.
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new("▶").size(13.0))
                            .fill(theme::card_hover()),
                    )
                    .on_hover_text("Play")
                    .clicked()
                {
                    actions.push(Action::Play(PlaybackTarget::new(title, episode)));
                }

                let numbering = episode
                    .numbering()
                    .or_else(|| season.number.map(|number| format!("S{number:02}")))
                    .unwrap_or_default();
                ui.add_sized(
                    [66.0, 18.0],
                    egui::Label::new(
                        egui::RichText::new(numbering)
                            .monospace()
                            .color(theme::muted()),
                    ),
                );

                // The provider's name for the episode, when there is one:
                // "The Immortal Legion" reads as an episode, and
                // "[Reaktor] … E57 v2 [1080p][x265].mkv" reads as a filename.
                let name = found
                    .as_ref()
                    .and_then(|(name, _, _)| name.clone())
                    .unwrap_or_else(|| episode.detail().to_string());
                let label = egui::RichText::new(name).color(if watched {
                    theme::muted()
                } else {
                    theme::text()
                });
                let hover = match &found {
                    // The synopsis, where the provider has one — AniList does
                    // not, and the filename is worth showing either way.
                    Some((_, Some(overview), _)) => {
                        format!("{}\n\n{overview}", episode.node.name)
                    }
                    _ => episode.node.name.clone(),
                };
                ui.add(egui::Label::new(label).truncate())
                    .on_hover_text(hover);

                if let Some((_, _, Some(air_date))) = &found {
                    ui.label(ui::muted(air_date.clone()));
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut checked = watched;
                    if ui
                        .checkbox(&mut checked, "")
                        .on_hover_text(if watched {
                            "Mark unwatched"
                        } else {
                            "Mark watched"
                        })
                        .changed()
                    {
                        actions.push(Action::SetWatched {
                            share_id: episode.node.share_id.clone(),
                            link_id: episode.node.link_id.clone(),
                            watched: checked,
                            duration: episode.watch.and_then(|watch| watch.duration_secs),
                        });
                    }

                    match episode.node.size {
                        // A file the share says is empty is one that will not
                        // play — an upload that never finished, usually. Worth
                        // saying so on the row rather than only when a click on
                        // it comes back with "has no content".
                        Some(0) => {
                            ui.label(
                                egui::RichText::new("empty")
                                    .size(12.0)
                                    .color(theme::danger()),
                            )
                            .on_hover_text(
                                "The share reports no content for this file; it cannot be played.",
                            );
                        }
                        Some(size) => {
                            ui.label(ui::muted(ui::format_size(size)));
                        }
                        None => {}
                    }
                    if let Some(at) = episode.resume_at() {
                        ui.label(
                            egui::RichText::new(format!("resume {}", ui::format_time(at)))
                                .size(12.0)
                                .color(theme::accent()),
                        );
                    }
                });
            });
        });
    ui.add_space(4.0);
}
