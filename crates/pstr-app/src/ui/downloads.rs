//! Inspectable desktop download queue.

use crate::app::Action;
use crate::engine::{DownloadItem, DownloadState};
use crate::{theme, ui};

pub fn show(ui: &mut egui::Ui, downloads: &[DownloadItem], actions: &mut Vec<Action>) {
    if downloads.is_empty() {
        ui::empty_state(
            ui,
            "No downloads",
            "Download an episode, a season or a whole show from its page, and it plays \
             without a connection.",
        );
        return;
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_max_width(960.0);
            summary(ui, downloads);
            ui.add_space(12.0);

            let mut start = 0;
            while start < downloads.len() {
                let title_key = &downloads[start].target.title_key;
                let end = downloads[start..]
                    .iter()
                    .position(|item| item.target.title_key != *title_key)
                    .map_or(downloads.len(), |offset| start + offset);
                group(ui, &downloads[start..end], actions);
                ui.add_space(14.0);
                start = end;
            }
        });
}

/// The line under the heading: how much, how fast, how long.
fn summary(ui: &mut egui::Ui, downloads: &[DownloadItem]) {
    ui::section(ui, "Downloads");
    let running: Vec<_> = downloads
        .iter()
        .filter(|item| item.state == DownloadState::Running)
        .collect();
    let rate: f64 = running.iter().map(|item| item.rate).sum();
    let left: u64 = downloads
        .iter()
        .filter(|item| {
            matches!(
                item.state,
                DownloadState::Queued | DownloadState::Running | DownloadState::Paused
            )
        })
        .map(|item| item.total.saturating_sub(item.downloaded))
        .sum();
    let mut parts = vec![format!(
        "{} available offline",
        ui::library::plural(
            downloads
                .iter()
                .filter(|item| item.state == DownloadState::Completed)
                .count(),
            "file"
        )
    )];
    if !running.is_empty() {
        parts.push(format!("{}/s", size(rate as u64)));
    }
    if left > 0 {
        parts.push(format!("{} to go", size(left)));
        if rate > 0.0 {
            parts.push(format!("about {}", ui::format_time(left as f64 / rate)));
        }
    }
    ui.label(ui::muted(parts.join("  ·  ")));
    ui.label(ui::muted(
        "Pausing or cancelling keeps every block already downloaded.",
    ));
}

/// One title's downloads, under a bar for all of them together.
fn group(ui: &mut egui::Ui, group: &[DownloadItem], actions: &mut Vec<Action>) {
    let title = &group[0].target.title_name;
    let downloaded: u64 = group.iter().map(|item| item.downloaded).sum();
    let total: u64 = group.iter().map(|item| item.total).sum();
    let completed = group
        .iter()
        .filter(|item| item.state == DownloadState::Completed)
        .count();
    let totals_known = group.iter().all(|item| item.total > 0);

    ui.horizontal(|ui| {
        ui.label(theme::Role::Subhead.rich(title).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(ui::muted(if totals_known {
                format!(
                    "{completed} of {}  ·  {} of {}",
                    group.len(),
                    size(downloaded),
                    size(total)
                )
            } else {
                format!("{completed} of {}  ·  working out the size…", group.len())
            }));
        });
    });
    if totals_known && group.len() > 1 {
        ui::progress_bar(ui, fraction(downloaded, total), ui.available_width());
    }
    ui.add_space(6.0);
    for item in group {
        row(ui, item, actions);
    }
}

fn row(ui: &mut egui::Ui, item: &DownloadItem, actions: &mut Vec<Action>) {
    egui::Frame::new()
        .fill(theme::card())
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let caption = if item.target.subtitle.is_empty() {
                    item.target.name.as_str()
                } else {
                    item.target.subtitle.as_str()
                };
                ui.add_sized(
                    [72.0, 18.0],
                    egui::Label::new(egui::RichText::new(caption).monospace()).truncate(),
                )
                .on_hover_text(&item.target.name);

                let (status, colour) = status(item);
                ui.add_sized(
                    [220.0, 18.0],
                    egui::Label::new(theme::Role::Caption.rich(status).color(colour)).truncate(),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    buttons(ui, item, actions);
                    if item.total > 0 && item.state != DownloadState::Completed {
                        ui.add_space(8.0);
                        ui.label(ui::muted(format!(
                            "{} / {}",
                            size(item.downloaded),
                            size(item.total)
                        )));
                        let width = (ui.available_width() - 8.0).clamp(40.0, 320.0);
                        ui::progress_bar(ui, item.percent(), width);
                    }
                });
            });
        });
    ui.add_space(4.0);
}

/// What a row says about itself, and in what colour.
fn status(item: &DownloadItem) -> (String, egui::Color32) {
    match &item.state {
        DownloadState::Queued => ("Waiting".to_owned(), theme::muted()),
        DownloadState::Running => {
            let mut text = format!("{}/s", size(item.rate as u64));
            if let Some(eta) = item.eta() {
                text.push_str(&format!("  ·  {} left", ui::format_time(eta)));
            }
            (text, theme::text())
        }
        DownloadState::Paused => ("Paused".to_owned(), theme::muted()),
        DownloadState::Completed => ("Available offline".to_owned(), theme::muted()),
        DownloadState::Cancelled => ("Stopped  ·  progress kept".to_owned(), theme::muted()),
        DownloadState::Failed(error) => (format!("Failed: {error}"), theme::danger()),
    }
}

fn buttons(ui: &mut egui::Ui, item: &DownloadItem, actions: &mut Vec<Action>) {
    let key = || item.key.clone();
    match item.state {
        DownloadState::Queued | DownloadState::Running => {
            if ui
                .button("Cancel")
                .on_hover_text("Stop, and keep what has downloaded")
                .clicked()
            {
                actions.push(Action::CancelDownload(key()));
            }
            if ui.button("Pause").clicked() {
                actions.push(Action::PauseDownload(key()));
            }
        }
        DownloadState::Paused => {
            if ui
                .button("Cancel")
                .on_hover_text("Stop, and keep what has downloaded")
                .clicked()
            {
                actions.push(Action::CancelDownload(key()));
            }
            if ui.button("Resume").clicked() {
                actions.push(Action::ResumeDownload(key()));
            }
        }
        DownloadState::Cancelled | DownloadState::Failed(_) => {
            if ui
                .button("Delete")
                .on_hover_text("Discard what has downloaded so far")
                .clicked()
            {
                actions.push(Action::RemoveDownload(key(), true));
            }
            if ui
                .button(if matches!(item.state, DownloadState::Failed(_)) {
                    "Retry"
                } else {
                    "Resume"
                })
                .clicked()
            {
                actions.push(Action::ResumeDownload(key()));
            }
        }
        DownloadState::Completed => {
            if ui
                .button("Delete")
                .on_hover_text("Delete the offline copy; keep the online source")
                .clicked()
            {
                actions.push(Action::RemoveDownload(key(), false));
            }
        }
    }
}

fn fraction(downloaded: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (downloaded as f64 / total as f64).clamp(0.0, 1.0) as f32
    }
}

fn size(bytes: u64) -> String {
    ui::format_size(i64::try_from(bytes).unwrap_or(i64::MAX))
}
