//! Picking a title's provider entry by hand.
//!
//! The matcher is deliberately unwilling to guess — see
//! [`pstr_meta::matching::MATCH_FLOOR`], and the reasoning that a wrong poster
//! does not look like a bug but like the library being wrong. The cost of that
//! is a handful of titles it will not decide about, and this dialog is where
//! they get decided: the provider's own answers, unscored and in its own order,
//! and a click that pins one of them.
//!
//! Nothing here scores anything. A person reading a list of eight entries with
//! their posters next to them is not guessing, which is exactly why the floor
//! that stops the scorer should not also stop them.

use pstr_core::metadata::{ProviderId, TitleMetadata};

use crate::app::{Action, Matcher};
use crate::theme;
use crate::ui::{self, Art};

/// How tall the result list may get before it scrolls. Enough for four rows,
/// which is what a page of provider answers usually needs.
const RESULTS_HEIGHT: f32 = 340.0;

/// A result row's poster, at the shape a cover image actually is.
const THUMB: egui::Vec2 = egui::vec2(46.0, 69.0);

pub fn show(
    ctx: &egui::Context,
    matcher: &mut Matcher,
    art: &mut Art<'_>,
    provider: ProviderId,
    actions: &mut Vec<Action>,
) {
    // What the title is matched to now, cloned out before `art` is borrowed for
    // the rows below.
    let current = art
        .metadata
        .get(&matcher.title_key)
        .map(|record| (record.metadata.clone(), record.manual));

    let modal = egui::Modal::new(egui::Id::new("matcher"))
        .frame(
            egui::Frame::new()
                .fill(theme::surface())
                .inner_margin(egui::Margin::same(18))
                .corner_radius(egui::CornerRadius::same(10)),
        )
        .show(ctx, |ui| {
            ui.set_width(620.0);

            ui.label(
                egui::RichText::new(format!("Match “{}”", matcher.title_name))
                    .size(18.0)
                    .strong()
                    .color(theme::text()),
            );
            ui.label(ui::muted(format!(
                "Everything {} answers with, in its own order — nothing is scored here.",
                provider.label()
            )));
            ui.add_space(10.0);

            search_row(ui, matcher, actions);
            ui.add_space(6.0);
            current_row(ui, matcher, current.as_ref(), actions);
            ui.add_space(10.0);

            if let Some(error) = &matcher.error {
                ui.label(egui::RichText::new(error).size(12.0).color(theme::danger()));
                ui.add_space(6.0);
            }

            results(ui, matcher, art, actions);

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        actions.push(Action::CloseMatcher);
                    }
                });
            });
        });

    // Escape, a click on the backdrop, or the close button above: all of them
    // mean the same thing, and the dialog changes nothing until a row is
    // clicked, so leaving it needs no confirmation.
    if modal.should_close() {
        actions.push(Action::CloseMatcher);
    }
}

/// The box and the button. Enter searches, because a search box that ignores it
/// is a search box people think is broken.
fn search_row(ui: &mut egui::Ui, matcher: &mut Matcher, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        let box_width = ui.available_width() - 90.0;
        let field = ui.add(
            egui::TextEdit::singleline(&mut matcher.query)
                .hint_text("Title to search for")
                .desired_width(box_width.max(120.0)),
        );
        if std::mem::take(&mut matcher.focus) {
            field.request_focus();
        }

        let entered = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if matcher.searching {
            ui.add(egui::Spinner::new().size(16.0));
        } else if (ui.button("Search").clicked() || entered) && !matcher.query.trim().is_empty() {
            actions.push(Action::SearchMatches);
        }
    });
}

/// What the title is matched to now, and the way back to no match at all.
fn current_row(
    ui: &mut egui::Ui,
    matcher: &Matcher,
    current: Option<&(Option<TitleMetadata>, bool)>,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        match current {
            Some((Some(found), manual)) => {
                ui.label(ui::muted(format!(
                    "Currently {}{}",
                    found.name,
                    if *manual { ", chosen by hand" } else { "" }
                )));
                // Only worth offering when there is something to undo: with no
                // row stored, "clear" would delete nothing.
                if ui
                    .button("Clear")
                    .on_hover_text("Forget this match and let the next match run decide again")
                    .clicked()
                {
                    actions.push(Action::ForgetMatch(matcher.title_key.clone()));
                }
            }
            Some((None, _)) => {
                ui.label(ui::muted("Currently unmatched — the provider had nothing."));
                if ui
                    .button("Clear")
                    .on_hover_text("Forget the stored miss so this title is asked about again")
                    .clicked()
                {
                    actions.push(Action::ForgetMatch(matcher.title_key.clone()));
                }
            }
            None => {
                ui.label(ui::muted("Not matched yet."));
            }
        };
    });
}

/// The provider's answers, one clickable row each.
fn results(ui: &mut egui::Ui, matcher: &Matcher, art: &mut Art<'_>, actions: &mut Vec<Action>) {
    if matcher.results.is_empty() {
        ui.label(ui::muted(if matcher.searching {
            "Searching…"
        } else if matcher.asked {
            // Both providers match on whole words, so a name the index has never
            // seen empties the page rather than returning something near it.
            "Nothing came back. Try a shorter name, or the title in another language."
        } else {
            "Search to see what the provider has."
        }));
        return;
    }

    egui::ScrollArea::vertical()
        .max_height(RESULTS_HEIGHT)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for found in &matcher.results {
                if row(ui, art, found).clicked() {
                    actions.push(Action::ChooseMatch(Box::new(found.clone())));
                }
                ui.add_space(4.0);
            }
        });
}

/// One answer: its cover, its names, and enough of its synopsis to tell two
/// entries in the same franchise apart — which is the whole job of this list.
fn row(ui: &mut egui::Ui, art: &mut Art<'_>, found: &TitleMetadata) -> egui::Response {
    let response = egui::Frame::new()
        .fill(theme::card())
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                let (rect, _) = ui.allocate_exact_size(THUMB, egui::Sense::hover());
                if let Some(texture) = poster(art, found) {
                    ui.painter().image(
                        texture.id(),
                        rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                } else {
                    ui.painter().rect_filled(
                        rect,
                        egui::CornerRadius::same(4),
                        theme::card_hover(),
                    );
                }

                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(&found.name)
                            .size(14.0)
                            .strong()
                            .color(theme::text()),
                    );
                    if let Some(original) = &found.original_name {
                        ui.label(ui::muted(original.clone()));
                    }
                    ui.label(ui::muted(facts(found)));
                    if let Some(overview) = &found.overview {
                        let mut job = egui::text::LayoutJob::simple(
                            overview.replace('\n', " "),
                            egui::FontId::proportional(11.5),
                            theme::muted(),
                            ui.available_width(),
                        );
                        job.wrap.max_rows = 2;
                        job.wrap.overflow_character = Some('…');
                        ui.label(job);
                    }
                });
            });
        })
        .response;

    let response = response.interact(egui::Sense::click());
    if response.hovered() {
        ui.painter().rect_stroke(
            response.rect,
            egui::CornerRadius::same(6),
            egui::Stroke::new(1.5, theme::accent()),
            egui::StrokeKind::Inside,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The cover for one answer, cached under the provider's id for it rather than
/// under a title key — these are not any title's artwork until one is picked.
fn poster(art: &mut Art<'_>, found: &TitleMetadata) -> Option<egui::TextureHandle> {
    let url = found
        .poster_url
        .clone()
        .or_else(|| found.backdrop_url.clone())?;
    let key = format!("option:{}:{}", found.provider.as_str(), found.remote_id);
    let engine = art.engine;
    let requested = key.clone();
    art.posters
        .texture(key, || engine.request_poster(requested, url))
}

/// `Film · 2017 · 26 episodes · ★ 8.2`, with whatever the provider left out
/// simply absent.
fn facts(found: &TitleMetadata) -> String {
    let mut parts: Vec<String> = vec![
        match found.kind {
            pstr_core::library::TitleKind::Series => "Series",
            pstr_core::library::TitleKind::Film => "Film",
        }
        .to_string(),
    ];
    if let Some(year) = found.year {
        parts.push(year.to_string());
    }
    if let Some(episodes) = found.episodes {
        parts.push(ui::library::plural(episodes as usize, "episode"));
    }
    if let Some(rating) = found.rating {
        parts.push(format!("★ {rating:.1}"));
    }
    parts.join("  ·  ")
}
