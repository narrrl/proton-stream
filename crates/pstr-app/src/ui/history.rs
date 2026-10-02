//! Everything that has been played, newest first, under a heading per day.
//!
//! Continue watching answers "what was I in the middle of"; this answers
//! "what was that episode on Tuesday". It is read straight off the watch
//! state, so there is nothing to keep in step: an episode is here because it
//! has a position or is marked watched, and leaves when it has neither.

use chrono::{DateTime, Local, NaiveDate, TimeZone};
use pstr_core::library::{Episode, Library, Title, TitleKind};

use crate::app::{Action, Page};
use crate::theme;
use crate::ui::{self, Art};

const STILL: egui::Vec2 = egui::vec2(128.0, 72.0);
/// How far back the page goes. Further than anyone scrolls; the cap is there
/// so a library with years of history does not lay out every row of it.
const MOST: usize = 400;

/// Where one entry lives in [`Library::titles`]: title, season, episode.
pub type Entry = (usize, usize, usize);

/// The entries of `library.history()`, as positions, so they can be kept
/// between frames and only rebuilt when the library changes.
pub fn entries(library: &Library) -> Vec<Entry> {
    let mut position = std::collections::HashMap::new();
    for (t, title) in library.titles.iter().enumerate() {
        for (s, season) in title.seasons.iter().enumerate() {
            for (e, episode) in season.episodes.iter().enumerate() {
                position.insert(std::ptr::from_ref(episode), (t, s, e));
            }
        }
    }
    library
        .history()
        .into_iter()
        .filter_map(|(_, episode)| position.get(&std::ptr::from_ref(episode)).copied())
        .collect()
}

pub fn show(
    ui: &mut egui::Ui,
    art: &mut Art<'_>,
    library: &Library,
    entries: &[Entry],
    actions: &mut Vec<Action>,
) {
    if entries.is_empty() {
        ui::empty_state(
            ui,
            "Nothing watched yet",
            "Episodes and films you play show up here, newest first, so the one from \
             last week is easy to find again.",
        );
        return;
    }

    let today = Local::now().date_naive();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_max_width(960.0);
            let mut day = None;
            for &(t, s, e) in entries.iter().take(MOST) {
                let Some(title) = library.titles.get(t) else {
                    continue;
                };
                let Some(episode) = title
                    .seasons
                    .get(s)
                    .and_then(|season| season.episodes.get(e))
                else {
                    continue;
                };
                let Some(at) = Local.timestamp_opt(episode.last_played(), 0).single() else {
                    continue;
                };
                if day != Some(at.date_naive()) {
                    if day.is_some() {
                        ui.add_space(theme::space::L);
                    }
                    day = Some(at.date_naive());
                    ui::section(ui, &day_label(at.date_naive(), today));
                }
                row(ui, art, title, episode, at, actions);
            }
        });
}

fn row(
    ui: &mut egui::Ui,
    art: &mut Art<'_>,
    title: &Title,
    episode: &Episode,
    at: DateTime<Local>,
    actions: &mut Vec<Action>,
) {
    let row_id = ui.id().with(("history", &episode.node.link_id));
    let frame = egui::Frame::new()
        .corner_radius(egui::CornerRadius::same(theme::radius::LG))
        .inner_margin(egui::Margin::same(8));
    let mut prepared = frame.begin(ui);
    {
        let ui = &mut prepared.content_ui;
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            ui::title::still(
                ui,
                art,
                title,
                episode,
                episode.is_watched(),
                STILL,
                actions,
            );
            ui.add_space(theme::space::M);

            let remove = 40.0;
            let text_width = (ui.available_width() - remove).max(120.0);
            ui.vertical(|ui| {
                ui.set_width(text_width);
                ui.spacing_mut().item_spacing.y = theme::space::XXS;
                ui.add_space(theme::space::XS);
                let name = ui
                    .add(
                        egui::Label::new(theme::Role::Body.rich(&title.name).color(theme::text()))
                            .sense(egui::Sense::click())
                            .truncate(),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Show this title");
                if name.clicked() {
                    actions.push(Action::Goto(Page::Title(title.key.clone())));
                }
                // A film's only "episode" is its file, and its filename says
                // nothing the title above it does not.
                if title.kind != TitleKind::Film {
                    let episode_name = art
                        .episode(&title.key, episode)
                        .and_then(|found| found.name.clone())
                        .unwrap_or_else(|| episode.detail().to_owned());
                    let line = match episode.numbering() {
                        Some(numbering) => format!("{numbering}  ·  {episode_name}"),
                        None => episode_name,
                    };
                    ui.add(egui::Label::new(ui::muted(line)).truncate())
                        .on_hover_text(&episode.node.name);
                }
                ui.label(ui::muted(status(episode, at)));
            });

            let cluster = egui::vec2(ui.available_width(), STILL.y);
            let layout = egui::Layout::right_to_left(egui::Align::Center);
            ui.allocate_ui_with_layout(cluster, layout, |ui| {
                let remove = ui
                    .add(
                        egui::Button::new(
                            theme::Role::Subhead
                                .rich(egui_phosphor::regular::X)
                                .color(theme::muted()),
                        )
                        .frame(false),
                    )
                    .on_hover_text(if episode.is_watched() {
                        "Remove from history — marks it unwatched"
                    } else {
                        "Remove from history — forgets where you stopped"
                    });
                if remove.clicked() {
                    actions.push(Action::RemoveFromHistory {
                        share_id: episode.node.share_id.clone(),
                        link_id: episode.node.link_id.clone(),
                    });
                }
            });
        });
    }
    let response = prepared.allocate_space(ui);
    let lit =
        ui.ctx()
            .animate_bool_with_time(row_id, response.contains_pointer(), theme::motion::HOVER);
    prepared.frame.fill = theme::card().gamma_multiply(0.35 + 0.65 * lit);
    prepared.paint(ui);
    ui.add_space(theme::space::XXS);
}

/// The grey line under the names: how far it got, and when.
fn status(episode: &Episode, at: DateTime<Local>) -> String {
    let when = at.format("%H:%M");
    if episode.is_watched() {
        return format!("Watched  ·  {when}");
    }
    match (
        episode.resume_at(),
        episode.watch.and_then(|watch| watch.duration_secs),
    ) {
        (Some(position), Some(duration)) => format!(
            "Stopped at {} of {}  ·  {when}",
            ui::format_time(position),
            ui::format_time(duration)
        ),
        (Some(position), None) => format!("Stopped at {}  ·  {when}", ui::format_time(position)),
        _ => format!("Played  ·  {when}"),
    }
}

/// The heading over a day's entries: "Today", "Yesterday", a weekday within
/// the last week, and a date before that — with the year only when it is not
/// this one.
fn day_label(day: NaiveDate, today: NaiveDate) -> String {
    use chrono::Datelike;
    match (today - day).num_days() {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => day.format("%A").to_string(),
        _ if day.year() == today.year() => day.format("%-d %B").to_string(),
        _ => day.format("%-d %B %Y").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    #[test]
    fn a_day_is_named_relative_to_today_for_the_last_week_and_dated_before_that() {
        let today = date(2026, 10, 2);
        assert_eq!(day_label(today, today), "Today");
        assert_eq!(day_label(date(2026, 10, 1), today), "Yesterday");
        assert_eq!(day_label(date(2026, 9, 28), today), "Monday");
        assert_eq!(day_label(date(2026, 9, 25), today), "25 September");
        assert_eq!(day_label(date(2025, 12, 24), today), "24 December 2025");
    }
}
