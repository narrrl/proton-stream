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
    pub appearance: Appearance,
}

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
            appearance_form(ui, prefs.appearance, actions);

            ui.add_space(28.0);
            playback_form(ui, prefs.autoplay, actions);

            ui.add_space(28.0);
            metadata_form(ui, settings, api_key, actions);
        });
}

/// The theme: a flavour, an accent, and whether the accent is a gradient.
///
/// Every control here repaints the whole window the moment it is clicked —
/// there is no "apply" — because the preview *is* the app. A swatch row that
/// only changed a small square would be asking someone to imagine the result.
fn appearance_form(ui: &mut egui::Ui, appearance: Appearance, actions: &mut Vec<Action>) {
    ui::section(ui, "Appearance");

    egui::Frame::new()
        .fill(theme::card())
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width().min(640.0));

            ui.label(ui::muted("Palette"));
            for flavor in Flavor::ALL {
                if ui
                    .radio(appearance.flavor == flavor, flavor.label())
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

            ui.add_space(12.0);
            ui.label(ui::muted("Accent"));
            ui.horizontal_wrapped(|ui| {
                for accent in Accent::ALL {
                    if swatch(ui, appearance, accent).clicked() && appearance.accent != accent {
                        actions.push(Action::SetAppearance(Appearance {
                            accent,
                            ..appearance
                        }));
                    }
                }
            });

            ui.add_space(12.0);
            let mut gradients = appearance.gradients;
            if ui
                .checkbox(&mut gradients, "Fade the accent into a second colour")
                .on_hover_text(
                    "Off draws it flat — better on a panel that bands a gradient into stripes",
                )
                .changed()
            {
                actions.push(Action::SetAppearance(Appearance {
                    gradients,
                    ..appearance
                }));
            }
        });
}

/// One accent, drawn in the colours it would actually produce.
///
/// Resolved against the *selected* flavour rather than the active palette, so
/// the row previews what clicking it would give rather than what is on screen.
fn swatch(ui: &mut egui::Ui, appearance: Appearance, accent: Accent) -> egui::Response {
    let selected = appearance.accent == accent;
    let size = egui::Vec2::new(52.0, 26.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let palette = theme::Palette::resolve(Appearance {
            accent,
            ..appearance
        });
        let radius = egui::CornerRadius::same(6);
        theme::swatch_fill(ui.painter(), rect, radius, &palette);
        if selected {
            // A ring rather than a tick: the tick would have to be drawn in an
            // ink that reads on eight different fills.
            ui.painter().rect_stroke(
                rect.expand(2.0),
                egui::CornerRadius::same(8),
                egui::Stroke::new(2.0, theme::text()),
                egui::StrokeKind::Outside,
            );
        }
    }
    response
        .on_hover_text(accent.label())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The handful of playback settings that are not per-file.
///
/// Volume, audio language and subtitle language are all set from the player
/// itself, where a viewer can hear the result — only autoplay has no natural
/// home there, because by the time it matters the episode is over.
fn playback_form(ui: &mut egui::Ui, autoplay: bool, actions: &mut Vec<Action>) {
    ui::section(ui, "Playback");

    egui::Frame::new()
        .fill(theme::card())
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width().min(640.0));

            let mut enabled = autoplay;
            if ui
                .checkbox(&mut enabled, "Play the next episode automatically")
                .on_hover_text("Only when an episode reaches its end, never after a failure")
                .changed()
            {
                actions.push(Action::SetAutoplay(enabled));
            }
            ui.label(ui::muted(
                "Volume, audio language and subtitles are set from the player, and remembered \
                 from there.",
            ));
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
    ui::section(ui, "Posters and descriptions");

    egui::Frame::new()
        .fill(theme::card())
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width().min(640.0));

            ui.label(ui::muted(
                "A share filled by the Proton Drive desktop client carries no thumbnails, so \
                 without this every tile is a pair of initials. Turning it on sends the titles \
                 in your library — not your files, and nothing about what you have watched — to \
                 the provider you choose, over HTTPS, each time a new one appears.",
            ));
            ui.add_space(10.0);

            let mut changed = false;
            let mut enabled = settings.enabled;
            if ui
                .checkbox(&mut enabled, "Look up posters and descriptions")
                .changed()
            {
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
        });
}
