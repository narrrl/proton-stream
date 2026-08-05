//! Inspectable desktop download queue.

use crate::app::Action;
use crate::engine::{DownloadItem, DownloadState};
use crate::{theme, ui};

pub fn show(ui: &mut egui::Ui, downloads: &[DownloadItem], actions: &mut Vec<Action>) {
    ui.heading("Downloads");
    ui.label(ui::muted(
        "Pause and Cancel keep complete downloaded blocks. Delete partial discards them.",
    ));
    ui.add_space(12.0);

    if downloads.is_empty() {
        ui.label("No offline downloads.");
        return;
    }

    let mut start = 0;
    while start < downloads.len() {
        let title_key = &downloads[start].target.title_key;
        let title = &downloads[start].target.title_name;
        let end = downloads[start..]
            .iter()
            .position(|item| item.target.title_key != *title_key)
            .map_or(downloads.len(), |offset| start + offset);
        let group = &downloads[start..end];
        let downloaded: u64 = group.iter().map(|item| item.downloaded).sum();
        let total: u64 = group.iter().map(|item| item.total).sum();
        let completed = group
            .iter()
            .filter(|item| item.state == DownloadState::Completed)
            .count();

        ui.label(egui::RichText::new(title).size(17.0).strong());
        let totals_known = group.iter().all(|item| item.total > 0);
        ui.horizontal(|ui| {
            if totals_known {
                ui.add(
                    egui::ProgressBar::new(fraction(downloaded, total))
                        .desired_width(260.0)
                        .show_percentage(),
                );
            } else {
                ui.add(egui::Spinner::new().size(16.0));
                ui.label(ui::muted("Calculating size…"));
            }
            ui.label(ui::muted(format!(
                "{} / {} · {completed}/{} files",
                bytes(downloaded),
                if !totals_known {
                    "calculating…".to_owned()
                } else {
                    bytes(total)
                },
                group.len()
            )));
        });
        for item in group {
            row(ui, item, actions);
        }
        ui.add_space(12.0);
        start = end;
    }
}

fn row(ui: &mut egui::Ui, item: &DownloadItem, actions: &mut Vec<Action>) {
    egui::Frame::new()
        .fill(theme::card())
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let caption = if item.target.subtitle.is_empty() {
                    item.target.name.as_str()
                } else {
                    item.target.subtitle.as_str()
                };
                ui.add_sized(
                    [180.0, 18.0],
                    egui::Label::new(egui::RichText::new(caption).strong()).truncate(),
                )
                .on_hover_text(&item.target.name);

                let status = match &item.state {
                    DownloadState::Queued => "Queued".to_owned(),
                    DownloadState::Running => "Downloading".to_owned(),
                    DownloadState::Paused => "Paused".to_owned(),
                    DownloadState::Completed => "Available offline".to_owned(),
                    DownloadState::Cancelled => "Cancelled · partial kept".to_owned(),
                    DownloadState::Failed(error) => format!("Failed: {error}"),
                };
                ui.add_sized(
                    [240.0, 18.0],
                    egui::Label::new(ui::muted(status)).truncate(),
                );

                if item.total > 0 {
                    ui.add(
                        egui::ProgressBar::new(item.percent())
                            .desired_width(140.0)
                            .show_percentage(),
                    );
                    ui.label(ui::muted(format!(
                        "{} / {}",
                        bytes(item.downloaded),
                        bytes(item.total)
                    )));
                }

                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| match item.state {
                        DownloadState::Queued | DownloadState::Running => {
                            if ui
                                .button("Cancel")
                                .on_hover_text("Stop and keep partial")
                                .clicked()
                            {
                                actions.push(Action::CancelDownload(item.key.clone()));
                            }
                            if ui.button("Pause").clicked() {
                                actions.push(Action::PauseDownload(item.key.clone()));
                            }
                        }
                        DownloadState::Paused => {
                            if ui
                                .button("Cancel")
                                .on_hover_text("Stop and keep partial")
                                .clicked()
                            {
                                actions.push(Action::CancelDownload(item.key.clone()));
                            }
                            if ui.button("Resume").clicked() {
                                actions.push(Action::ResumeDownload(item.key.clone()));
                            }
                        }
                        DownloadState::Cancelled | DownloadState::Failed(_) => {
                            if ui.button("Delete partial…").clicked() {
                                actions.push(Action::RemoveDownload(item.key.clone(), true));
                            }
                            if ui.button("Resume / Retry").clicked() {
                                actions.push(Action::ResumeDownload(item.key.clone()));
                            }
                        }
                        DownloadState::Completed => {
                            if ui.button("Make online-only").clicked() {
                                actions.push(Action::RemoveDownload(item.key.clone(), false));
                            }
                        }
                    },
                );
            });
        });
}

fn fraction(downloaded: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (downloaded as f64 / total as f64).clamp(0.0, 1.0) as f32
    }
}

fn bytes(value: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * KIB;
    const GIB: f64 = 1024.0 * MIB;
    let value = value as f64;
    if value >= GIB {
        format!("{:.1} GiB", value / GIB)
    } else if value >= MIB {
        format!("{:.1} MiB", value / MIB)
    } else if value >= KIB {
        format!("{:.1} KiB", value / KIB)
    } else {
        format!("{value:.0} B")
    }
}
