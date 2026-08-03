//! The library page: what to watch, as a wall of stills.

use pstr_core::library::{Library, Title, TitleKind};

use crate::app::{Action, Page};
use crate::theme;
use crate::ui::Art;
use crate::ui::{self, Card};

pub fn show(
    ui: &mut egui::Ui,
    art: &mut Art<'_>,
    library: &Library,
    search: &str,
    actions: &mut Vec<Action>,
) {
    if library.is_empty() {
        return empty_state(ui, actions);
    }

    let matches = library.search(search);
    let searching = !search.trim().is_empty();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if !searching {
                let resumable = library.continue_watching();
                if !resumable.is_empty() {
                    ui::section(ui, "Continue watching");
                    continue_row(ui, art, &resumable, actions);
                    ui.add_space(16.0);
                }
            }

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

            if matches.is_empty() {
                ui.add_space(8.0);
                ui.label(ui::muted("Nothing here by that name."));
                return;
            }

            grid(ui, art, &matches, actions);
        });
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

                    let clicked = ui::card(
                        ui,
                        Card {
                            art: art.of(title),
                            name: &title.name,
                            subtitle: remaining,
                            progress: episode.progress().map(|value| value as f32),
                            badge: episode.numbering(),
                        },
                    )
                    .clicked();

                    if clicked {
                        actions.push(Action::Goto(Page::Title(title.key.clone())));
                    }
                }
            });
        });
}

/// Every title, wrapped to the window.
fn grid(ui: &mut egui::Ui, art: &mut Art<'_>, titles: &[&Title], actions: &mut Vec<Action>) {
    let columns = ui::columns(ui.available_width());
    for row in titles.chunks(columns) {
        ui.horizontal(|ui| {
            for title in row {
                let clicked = ui::card(
                    ui,
                    Card {
                        art: art.of(title),
                        name: &title.name,
                        subtitle: subtitle(title),
                        progress: title.resume().and_then(|e| e.progress()).map(|v| v as f32),
                        // What a film is says itself in the subtitle below the
                        // card; a second `Film` over the poster is noise.
                        badge: None,
                    },
                )
                .clicked();

                if clicked {
                    actions.push(Action::Goto(Page::Title(title.key.clone())));
                }
            }
        });
        ui.add_space(theme::CARD_GAP);
    }
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

fn empty_state(ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    ui.vertical_centered(|ui| {
        ui.add_space(120.0);
        ui.label(
            egui::RichText::new("Nothing in the library yet")
                .size(20.0)
                .strong(),
        );
        ui.add_space(6.0);
        ui.label(ui::muted(
            "Add a Proton Drive share link, and everything playable behind it shows up here.",
        ));
        ui.add_space(18.0);
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
    fn plurals_read_naturally() {
        assert_eq!(plural(1, "title"), "1 title");
        assert_eq!(plural(0, "title"), "0 titles");
        assert_eq!(plural(12, "episode"), "12 episodes");
    }
}
