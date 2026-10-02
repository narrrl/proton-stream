//! Settings: how the app looks, how it plays, and what it asks the internet.
//!
//! Split out of the shares page, where these lived because that was the only
//! page with a form on it. Nobody looking for the theme looks under the page
//! that manages Proton Drive links, and a page that is half link management and
//! half preferences is two pages.

use pstr_core::appearance::{Accent, Appearance, Flavor};
use pstr_core::metadata::{MetadataConfig, ProviderId};

use crate::app::Action;
use crate::theme;
use crate::ui;

/// The settings that are a single value rather than a form.
///
/// Bundled for the same reason [`crate::ui::Art`] is: they arrive together,
/// they are read-only, and passing them one by one is what pushed this
/// function past the argument count anybody can read.
#[derive(Debug, Clone, Copy)]
pub struct Prefs {
    pub autoplay: bool,
    pub auto_skip: bool,
    pub appearance: Appearance,
}

/// The widest the settings column gets. Rows that run the width of a 2K window
/// put a switch a whole screen away from what it switches.
const COLUMN: f32 = 760.0;

pub fn show(
    ui: &mut egui::Ui,
    settings: &mut MetadataConfig,
    prefs: Prefs,
    api_key: &mut String,
    actions: &mut Vec<Action>,
) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Centred, so a wide window frames the column rather than leaving it
            // against one edge with the rest of the page empty.
            let width = ui.available_width().min(COLUMN);
            let margin = ((ui.available_width() - width) / 2.0).max(0.0);
            ui.horizontal(|ui| {
                ui.add_space(margin);
                ui.vertical(|ui| {
                    ui.set_width(width);
                    ui.add_space(8.0);
                    appearance_form(ui, prefs.appearance, actions);
                    playback_form(ui, prefs, actions);
                    metadata_form(ui, settings, api_key, actions);
                    about(ui);
                });
            });
        });
}

/// The theme: a flavour, an accent, and whether the accent is a gradient.
///
/// Every control here repaints the whole window the moment it is clicked —
/// there is no "apply" — because the preview *is* the app. A swatch row that
/// only changed a small square would be asking someone to imagine the result.
fn appearance_form(ui: &mut egui::Ui, appearance: Appearance, actions: &mut Vec<Action>) {
    ui::widgets::settings_group(ui, "Appearance", None, |ui| {
        ui.add_space(12.0);
        ui.label(theme::Role::Body.rich("Palette").color(theme::text()));
        ui.add_space(8.0);
        ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
        ui.horizontal_wrapped(|ui| {
            for flavor in Flavor::ALL {
                if palette_card(ui, appearance, flavor)
                    .on_hover_text(flavor.description())
                    .clicked()
                    && appearance.flavor != flavor
                {
                    actions.push(Action::SetAppearance(Appearance {
                        flavor,
                        ..appearance
                    }));
                }
            }
        });
        ui.add_space(14.0);
        ui.spacing_mut().item_spacing.y = 0.0;

        ui::widgets::settings_row(ui, "Accent", None, |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            // Right to left, so reversed to read in the order they are listed.
            for accent in Accent::ALL.into_iter().rev() {
                if swatch(ui, appearance, accent).clicked() && appearance.accent != accent {
                    actions.push(Action::SetAppearance(Appearance {
                        accent,
                        ..appearance
                    }));
                }
            }
        });

        let mut gradients = appearance.gradients;
        ui::widgets::settings_row(
            ui,
            "Gradient accent",
            Some(
                "Fade the accent into a second colour. Off draws it flat, which suits a panel that bands gradients into stripes.",
            ),
            |ui| {
                if ui::widgets::toggle(ui, &mut gradients, "").changed() {
                    actions.push(Action::SetAppearance(Appearance {
                        gradients,
                        ..appearance
                    }));
                }
            },
        );
    });
}

/// One palette, as a miniature of the app wearing it: page, a card on it, a
/// line of text and the accent.
fn palette_card(ui: &mut egui::Ui, appearance: Appearance, flavor: Flavor) -> egui::Response {
    let size = egui::vec2(132.0, 92.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let palette = theme::Palette::resolve(Appearance {
            flavor,
            ..appearance
        });
        let selected = appearance.flavor == flavor;
        let hover = ui
            .ctx()
            .animate_bool_with_time(response.id, response.hovered(), 0.10);
        let painter = ui.painter();
        let radius = egui::CornerRadius::same(8);
        let preview = egui::Rect::from_min_size(rect.min, egui::vec2(size.x, 64.0));

        painter.rect_filled(preview, radius, palette.background);
        let bar = egui::Rect::from_min_size(preview.min, egui::vec2(size.x, 12.0));
        painter.rect_filled(
            bar,
            egui::CornerRadius {
                nw: 8,
                ne: 8,
                sw: 0,
                se: 0,
            },
            palette.surface,
        );
        let tile = |x: f32| {
            egui::Rect::from_min_size(preview.min + egui::vec2(x, 20.0), egui::vec2(34.0, 22.0))
        };
        for x in [8.0, 49.0, 90.0] {
            painter.rect_filled(tile(x), egui::CornerRadius::same(3), palette.card);
        }
        let pill =
            egui::Rect::from_min_size(preview.min + egui::vec2(8.0, 3.0), egui::vec2(22.0, 6.0));
        theme::swatch_fill(painter, pill, egui::CornerRadius::same(3), &palette);
        painter.hline(
            (preview.left() + 8.0)..=(preview.left() + 60.0),
            preview.top() + 50.0,
            egui::Stroke::new(2.0, palette.text),
        );
        painter.hline(
            (preview.left() + 8.0)..=(preview.left() + 40.0),
            preview.top() + 56.0,
            egui::Stroke::new(2.0, palette.muted),
        );

        let ring = if selected {
            egui::Stroke::new(2.0, theme::accent())
        } else {
            egui::Stroke::new(
                1.0,
                theme::card_hover().lerp_to_gamma(theme::muted(), hover),
            )
        };
        painter.rect_stroke(preview, radius, ring, egui::StrokeKind::Outside);
        painter.text(
            egui::pos2(rect.left() + 2.0, preview.bottom() + 8.0),
            egui::Align2::LEFT_TOP,
            flavor.label(),
            theme::Role::Label.font(),
            if selected {
                theme::text()
            } else {
                theme::muted()
            },
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// One accent, drawn in the colours it would actually produce.
///
/// Resolved against the *selected* flavour rather than the active palette, so
/// the row previews what clicking it would give rather than what is on screen.
fn swatch(ui: &mut egui::Ui, appearance: Appearance, accent: Accent) -> egui::Response {
    let selected = appearance.accent == accent;
    let size = egui::Vec2::splat(26.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let palette = theme::Palette::resolve(Appearance {
            accent,
            ..appearance
        });
        let radius = egui::CornerRadius::same(13);
        theme::swatch_fill(ui.painter(), rect, radius, &palette);
        if selected {
            // A ring rather than a tick: the tick would have to be drawn in an
            // ink that reads on eight different fills.
            ui.painter().circle_stroke(
                rect.center(),
                rect.width() / 2.0 + 3.0,
                egui::Stroke::new(2.0, theme::text()),
            );
        }
    }
    response
        .on_hover_text(accent.label())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The playback settings that are not per-file.
///
/// Volume, audio language, subtitle language and speed are all set from the
/// player itself, where a viewer can hear the result.
fn playback_form(ui: &mut egui::Ui, prefs: Prefs, actions: &mut Vec<Action>) {
    ui::widgets::settings_group(
        ui,
        "Playback",
        Some(
            "Volume, audio, subtitles and speed are set from the player, and remembered from there.",
        ),
        |ui| {
            let mut autoplay = prefs.autoplay;
            ui::widgets::settings_row(
                ui,
                "Play the next episode",
                Some("When an episode ends, start the next one. Never after a file that failed."),
                |ui| {
                    if ui::widgets::toggle(ui, &mut autoplay, "").changed() {
                        actions.push(Action::SetAutoplay(autoplay));
                    }
                },
            );
            let mut auto_skip = prefs.auto_skip;
            ui::widgets::settings_row(
                ui,
                "Skip openings and credits",
                Some(
                    "Jump past chapters named as an opening, ending or preview, rather than offering a button. Chapter names are the release's guess, so this can skip a scene.",
                ),
                |ui| {
                    if ui::widgets::toggle(ui, &mut auto_skip, "").changed() {
                        actions.push(Action::SetAutoSkip(auto_skip));
                    }
                },
            );
        },
    );
}

/// What this is, and which version.
fn about(ui: &mut egui::Ui) {
    ui::widgets::settings_group(ui, "About", None, |ui| {
        ui::widgets::settings_row(
            ui,
            "proton-stream",
            Some("Streams Proton Drive share links without an account or a download step."),
            |ui| {
                ui.label(ui::muted(concat!("version ", env!("CARGO_PKG_VERSION"))));
            },
        );
        ui::widgets::settings_row(ui, "Keyboard shortcuts", None, |ui| {
            ui.label(ui::muted("press ?"));
        });
    });
}

/// Enrichment: the switch, the provider, and the cost of turning it on.
///
/// The privacy note is not decoration and it is not in a tooltip. Turning this
/// on sends the titles in someone's library to a third party, which is a thing
/// they can only agree to if they are told — so it is stated where the switch
/// is, before the switch, in the same size as everything else.
fn metadata_form(
    ui: &mut egui::Ui,
    settings: &mut MetadataConfig,
    api_key: &mut String,
    actions: &mut Vec<Action>,
) {
    ui::widgets::settings_group(ui, "Posters and descriptions", None, |ui| {
        ui.add_space(12.0);
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.label(ui::muted(
            "A share filled by the Proton Drive desktop client carries no thumbnails, so \
                 without this every tile is a pair of initials. Turning it on sends the titles \
                 in your library — not your files, and nothing about what you have watched — to \
                 the provider you choose, over HTTPS, each time a new one appears.",
        ));
        ui.add_space(10.0);

        let mut changed = false;
        let mut enabled = settings.enabled;
        if ui::widgets::toggle(ui, &mut enabled, "Look up posters and descriptions").changed() {
            settings.enabled = enabled;
            changed = true;
        }

        if settings.enabled {
            ui.add_space(10.0);
            ui.label(ui::muted("Provider"));
            for provider in ProviderId::ALL {
                if ui
                    .radio(settings.provider == provider, provider.label())
                    .on_hover_text(provider.description())
                    .clicked()
                    && settings.provider != provider
                {
                    settings.provider = provider;
                    changed = true;
                }
            }

            if settings.provider.needs_api_key() {
                ui.add_space(10.0);
                ui.label(ui::muted(format!("{} API key", settings.provider.label())));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(api_key)
                            .password(true)
                            .hint_text("v3 API key")
                            .desired_width(300.0),
                    );
                    if ui.button("Save key").clicked() {
                        actions.push(Action::SetApiKey {
                            provider: settings.provider,
                            key: std::mem::take(api_key),
                        });
                    }
                });
                ui.label(ui::muted(
                    "Stored in your system keyring, never in a config file. Leave the box \
                         empty and save to forget it.",
                ));
            }

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui::accent_button(ui, "Match the library").clicked() {
                    actions.push(Action::MatchTitles { force: false });
                }
                if ui
                    .button("Match everything again")
                    .on_hover_text("Ask about every title, including ones already matched")
                    .clicked()
                {
                    actions.push(Action::MatchTitles { force: true });
                }
            });
        } else {
            ui.add_space(6.0);
            ui.label(ui::muted(
                "Off. Nothing is sent anywhere, and turning it off also deletes the answers \
                     already stored.",
            ));
        }

        if changed {
            actions.push(Action::SetMetadataConfig(settings.clone()));
        }
        ui.add_space(12.0);
    });
}
