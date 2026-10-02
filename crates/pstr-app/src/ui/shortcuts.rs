//! The list of keyboard shortcuts, on `?`.
//!
//! Written out here rather than generated from the handlers, because the
//! handlers are split between the player page and every other page, and what a
//! viewer needs is one list grouped by where they are.

use crate::theme;
use crate::ui;

const GROUPS: &[(&str, &[(&str, &str)])] = &[
    (
        "Anywhere",
        &[
            ("Ctrl F  or  /", "Search the library"),
            ("F5  or  Ctrl R", "Crawl every share again"),
            (
                "Ctrl 1 – 5",
                "Library, History, Shares, Downloads, Settings",
            ),
            ("Ctrl ,", "Settings"),
            ("Ctrl V", "Paste a share link to add it"),
            ("Tab  or  arrows", "Move between titles and episodes"),
            ("Enter", "Open or play what is highlighted"),
            ("Alt ←  or  mouse back", "Back"),
            ("?", "This list"),
        ],
    ),
    (
        "Playing",
        &[
            ("Space  or  K", "Pause and play"),
            ("←  /  →", "Back 10 s, forward 30 s"),
            ("↑  /  ↓", "Volume"),
            ("Ctrl S", "Save the frame to Pictures/proton-stream"),
            ("M", "Mute"),
            ("N  /  P", "Next and previous episode"),
            ("[  /  ]", "Slower and faster"),
            ("S", "Skip the opening or the credits"),
            ("E", "The list of episodes"),
            ("F  or  double-click", "Fullscreen"),
            ("Esc", "Leave fullscreen, then the player"),
        ],
    ),
];

/// Draw the list. Returns whether it asked to be closed.
pub fn show(ctx: &egui::Context) -> bool {
    let mut close = false;
    let modal = egui::Modal::new(egui::Id::new("shortcuts"))
        .frame(
            egui::Frame::new()
                .fill(theme::surface())
                .inner_margin(egui::Margin::same(20))
                .corner_radius(egui::CornerRadius::same(theme::radius::LG)),
        )
        .show(ctx, |ui| {
            ui.set_width(460.0);
            ui.horizontal(|ui| {
                ui.label(theme::Role::Heading.rich("Keyboard shortcuts").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
            for (heading, keys) in GROUPS {
                ui.add_space(theme::space::L);
                ui.label(ui::muted(heading.to_uppercase()));
                ui.add_space(theme::space::XS);
                egui::Grid::new(*heading)
                    .num_columns(2)
                    .spacing([18.0, 8.0])
                    .show(ui, |ui| {
                        for (key, what) in *keys {
                            ui.label(
                                theme::Role::Label
                                    .rich(*key)
                                    .monospace()
                                    .color(theme::accent()),
                            );
                            ui.label(theme::Role::Body.rich(*what));
                            ui.end_row();
                        }
                    });
            }
        });
    close || modal.should_close()
}
