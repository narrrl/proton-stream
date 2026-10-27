//! Settings: how the app looks, how it plays, and what it asks the internet.
//!
//! Split out of the shares page, where these lived because that was the only
//! page with a form on it. Nobody looking for the theme looks under the page
//! that manages Proton Drive links, and a page that is half link management and
//! half preferences is two pages.

use pstr_core::appearance::{Accent, Appearance, Flavor, Tiles};
use pstr_core::metadata::{MetadataConfig, ProviderId, TitleNames};
use pstr_core::prefs::{Colour, PlaybackPrefs, SubtitleBackdrop, SubtitleStyle};
use pstr_player::LANGUAGES;

use crate::app::Action;
use crate::desktop_prefs::{CACHE_CHOICES, DesktopPrefs, TEXT_SIZE_CHOICES};
use crate::theme;
use crate::ui;
use crate::ui::widgets::Named;

/// The settings that are a single value rather than a form.
///
/// Bundled for the same reason [`crate::ui::Art`] is: they arrive together,
/// they are read-only, and passing them one by one is what pushed this
/// function past the argument count anybody can read.
#[derive(Debug, Clone, Copy)]
pub struct Prefs<'a> {
    pub autoplay: bool,
    pub auto_skip: bool,
    /// The languages, read from here.
    pub playback: &'a PlaybackPrefs,
    pub appearance: Appearance,
    pub desktop: DesktopPrefs,
    /// What the streaming cache holds, once the shares are open.
    pub cache_used: Option<u64>,
}

/// The widest the settings column gets. Rows that run the width of a 2K window
/// put a switch a whole screen away from what it switches.
const COLUMN: f32 = 760.0;

pub fn show(
    ui: &mut egui::Ui,
    settings: &mut MetadataConfig,
    prefs: Prefs<'_>,
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
                    ui.add_space(theme::space::M);
                    appearance_form(ui, prefs.appearance, prefs.desktop, actions);
                    playback_form(ui, prefs, actions);
                    subtitle_form(ui, &prefs.playback.subtitle_style, actions);
                    storage_form(ui, prefs, actions);
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
fn appearance_form(
    ui: &mut egui::Ui,
    appearance: Appearance,
    desktop: DesktopPrefs,
    actions: &mut Vec<Action>,
) {
    ui::widgets::settings_group(ui, "Appearance", None, |ui| {
        ui.add_space(theme::space::L);
        ui.label(theme::Role::Body.rich("Palette").color(theme::text()));
        ui.add_space(theme::space::M);
        ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
        ui.horizontal_wrapped(|ui| {
            for flavor in Flavor::ALL {
                if palette_card(ui, appearance, flavor)
                    .named(flavor.label())
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
        ui.add_space(theme::space::L);
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

        ui::widgets::settings_row(
            ui,
            "Text size",
            Some("Everything grows with it. Kept on this computer only."),
            |ui| {
                let choices: Vec<(u16, String)> = TEXT_SIZE_CHOICES
                    .iter()
                    .map(|&percent| (percent, format!("{percent}%")))
                    .collect();
                let labels: Vec<(u16, &str)> = choices
                    .iter()
                    .map(|(percent, label)| (*percent, label.as_str()))
                    .collect();
                if let Some(text_size) = ui::widgets::segmented(ui, desktop.text_size, &labels) {
                    actions.push(Action::SetDesktopPrefs(DesktopPrefs {
                        text_size,
                        ..desktop
                    }));
                }
            },
        );
        ui::widgets::settings_row(
            ui,
            "Library tiles",
            Some("Covers, or 16:9 stills. Covers are the art every matched title has."),
            |ui| {
                let choices: Vec<(Tiles, &str)> = Tiles::ALL
                    .iter()
                    .map(|tiles| (*tiles, tiles.label()))
                    .collect();
                if let Some(tiles) = ui::widgets::segmented(ui, appearance.tiles, &choices) {
                    actions.push(Action::SetAppearance(Appearance {
                        tiles,
                        ..appearance
                    }));
                }
            },
        );
        ui::widgets::settings_row(
            ui,
            "Title names",
            Some("The name a matched title goes by. Its other names still find it in search."),
            |ui| {
                let choices: Vec<(TitleNames, &str)> = TitleNames::ALL
                    .iter()
                    .map(|names| (*names, names.label()))
                    .collect();
                if let Some(names) = ui::widgets::segmented(ui, appearance.names, &choices) {
                    actions.push(Action::SetAppearance(Appearance {
                        names,
                        ..appearance
                    }));
                }
            },
        );

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
        let hover =
            ui.ctx()
                .animate_bool_with_time(response.id, response.hovered(), theme::motion::HOVER);
        let painter = ui.painter();
        let radius = egui::CornerRadius::same(theme::radius::MD);
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
            painter.rect_filled(
                tile(x),
                egui::CornerRadius::same(theme::radius::BAR),
                palette.card,
            );
        }
        let pill =
            egui::Rect::from_min_size(preview.min + egui::vec2(8.0, 3.0), egui::vec2(22.0, 6.0));
        theme::swatch_fill(
            painter,
            pill,
            egui::CornerRadius::same(theme::radius::BAR),
            &palette,
        );
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
        .hint(accent.label())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The playback settings that are not per-file.
///
/// Volume and speed are set from the player itself, where a viewer can hear
/// the result. The languages are here as well as there: what a first episode
/// opens with is decided before there is a player to choose from.
fn playback_form(ui: &mut egui::Ui, prefs: Prefs<'_>, actions: &mut Vec<Action>) {
    ui::widgets::settings_group(
        ui,
        "Playback",
        Some(
            "Volume and speed are set from the player. A language chosen in the player for one show is kept for that show.",
        ),
        |ui| {
            languages(ui, prefs.playback, actions);
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
            let mut desktop = prefs.desktop;
            ui::widgets::settings_row(
                ui,
                "Hardware decoding",
                Some(
                    "Decode on the graphics card. Turn off if video shows green frames or nothing at all; it costs CPU, not quality. From the next file.",
                ),
                |ui| {
                    if ui::widgets::toggle(ui, &mut desktop.hardware_decoding, "").changed() {
                        actions.push(Action::SetDesktopPrefs(desktop));
                    }
                },
            );
        },
    );
}

/// The audio and subtitle languages every file opens with, until a show has a
/// choice of its own.
fn languages(ui: &mut egui::Ui, playback: &PlaybackPrefs, actions: &mut Vec<Action>) {
    let name = |tag: &Option<String>| match tag {
        Some(tag) => pstr_player::language_name(tag),
        None => "The file's default".to_owned(),
    };
    let audio = playback.audio_language.clone();
    ui::widgets::settings_row(
        ui,
        "Audio language",
        Some("Played when a file has it."),
        |ui| {
            egui::ComboBox::from_id_salt("audio-language")
                .selected_text(name(&audio))
                .show_ui(ui, |ui| {
                    for choice in
                        std::iter::once(None).chain(LANGUAGES.map(|tag| Some(tag.to_owned())))
                    {
                        if ui
                            .selectable_label(audio == choice, name(&choice))
                            .clicked()
                            && audio != choice
                        {
                            actions.push(Action::SetLanguages {
                                audio: choice,
                                subtitles: playback.subtitles,
                                subtitle: playback.subtitle_language.clone(),
                            });
                        }
                    }
                });
        },
    );
    // Off, the file's default, or a language: "off" is a decision of its
    // own, which no language expresses.
    let subtitle = playback
        .subtitles
        .then(|| playback.subtitle_language.clone());
    let subtitle_name = |choice: &Option<Option<String>>| match choice {
        None => "Off".to_owned(),
        Some(tag) => name(tag),
    };
    ui::widgets::settings_row(
        ui,
        "Subtitles",
        Some("Shown when a file has them in this language."),
        |ui| {
            egui::ComboBox::from_id_salt("subtitle-language")
                .selected_text(subtitle_name(&subtitle))
                .show_ui(ui, |ui| {
                    let choices = [None, Some(None)]
                        .into_iter()
                        .chain(LANGUAGES.map(|tag| Some(Some(tag.to_owned()))));
                    for choice in choices {
                        if ui
                            .selectable_label(subtitle == choice, subtitle_name(&choice))
                            .clicked()
                            && subtitle != choice
                        {
                            actions.push(Action::SetLanguages {
                                audio: playback.audio_language.clone(),
                                subtitles: choice.is_some(),
                                subtitle: choice.flatten(),
                            });
                        }
                    }
                });
        },
    );
}

/// How subtitles look, with a sample line to judge it by.
///
/// Every change reaches a playing file as it is made, so the film itself is
/// the other preview. A styled track takes only the size and the position
/// unless it is asked to take the rest; see [`SubtitleStyle`].
fn subtitle_form(ui: &mut egui::Ui, style: &SubtitleStyle, actions: &mut Vec<Action>) {
    ui::widgets::settings_group(
        ui,
        "Subtitles",
        Some(
            "Changes show on the playing video straight away. Plain subtitles (SRT) take all of this; styled ones (ASS) only the size and the position, unless you restyle them below.",
        ),
        |ui| {
            let mut chosen = style.clone();
            ui.add_space(theme::space::L);
            subtitle_preview(ui, &chosen);
            ui.add_space(theme::space::L);

            ui::widgets::settings_row(ui, "Size", None, |ui| {
                percent_slider(
                    ui,
                    &mut chosen.scale,
                    SubtitleStyle::MIN_SCALE..=SubtitleStyle::MAX_SCALE,
                );
            });
            ui::widgets::settings_row(
                ui,
                "Position",
                Some("How far up the picture the line sits."),
                |ui| subtitle_position_slider(ui, &mut chosen.position),
            );
            ui::widgets::settings_row(
                ui,
                "Font",
                Some("Any font installed here. The sample above is drawn in the app's own font."),
                |ui| {
                    ui::widgets::toggle(ui, &mut chosen.italic, "Italic");
                    ui::widgets::toggle(ui, &mut chosen.bold, "Bold");
                    font_picker(ui, &mut chosen.font);
                },
            );
            ui::widgets::settings_row(ui, "Text colour", None, |ui| {
                colour_button(ui, &mut chosen.colour);
            });
            ui::widgets::settings_row(
                ui,
                "Background",
                Some("A box reads on bright scenes; an outline hides less of the picture."),
                |ui| {
                    let choices =
                        SubtitleBackdrop::ALL.map(|backdrop| (backdrop, backdrop.label()));
                    if let Some(backdrop) = ui::widgets::segmented(ui, chosen.backdrop, &choices) {
                        chosen.backdrop = backdrop;
                    }
                },
            );
            match chosen.backdrop {
                SubtitleBackdrop::Outline => {
                    ui::widgets::settings_row(ui, "Outline", None, |ui| {
                        colour_button(ui, &mut chosen.outline_colour);
                        ui.add(
                            egui::Slider::new(
                                &mut chosen.outline_size,
                                0.0..=SubtitleStyle::MAX_OUTLINE,
                            )
                            .step_by(0.05)
                            .fixed_decimals(1),
                        );
                    });
                }
                SubtitleBackdrop::Box => {
                    ui::widgets::settings_row(
                        ui,
                        "Box colour",
                        Some("Its opacity is how much of the picture shows through."),
                        |ui| colour_button(ui, &mut chosen.box_colour),
                    );
                }
            }
            ui::widgets::settings_row(ui, "Shadow", Some("How far it falls. 0 is none."), |ui| {
                colour_button(ui, &mut chosen.shadow_colour);
                ui.add(
                    egui::Slider::new(&mut chosen.shadow_offset, 0.0..=SubtitleStyle::MAX_SHADOW)
                        .step_by(0.1)
                        .fixed_decimals(1),
                );
            });
            ui::widgets::settings_row(
                ui,
                "Blur",
                Some("Softens the edges. Not shown in the sample."),
                |ui| {
                    ui.add(
                        egui::Slider::new(&mut chosen.blur, 0.0..=SubtitleStyle::MAX_BLUR)
                            .step_by(0.1)
                            .fixed_decimals(1),
                    );
                },
            );
            ui::widgets::settings_row(
                ui,
                "Restyle styled subtitles",
                Some(
                    "Repaint styled (ASS) dialogue in this style too. Signs, songs and karaoke can come out wrong.",
                ),
                |ui| {
                    ui::widgets::toggle(ui, &mut chosen.restyle_styled, "");
                },
            );
            ui::widgets::settings_row(ui, "Reset", Some("Back to mpv's own look."), |ui| {
                if ui
                    .add_enabled(
                        *style != SubtitleStyle::default(),
                        egui::Button::new("Reset"),
                    )
                    .clicked()
                {
                    chosen = SubtitleStyle::default();
                }
            });

            if chosen != *style {
                actions.push(Action::SetSubtitleStyle(chosen.sanitized()));
            }
        },
    );
}

/// A scale as a percentage slider, 100% being mpv's own size.
pub(crate) fn percent_slider(
    ui: &mut egui::Ui,
    scale: &mut f64,
    range: std::ops::RangeInclusive<f64>,
) -> egui::Response {
    let mut percent = *scale * 100.0;
    let response = ui.add(
        egui::Slider::new(&mut percent, range.start() * 100.0..=range.end() * 100.0)
            .step_by(5.0)
            .suffix("%")
            .fixed_decimals(0),
    );
    *scale = percent / 100.0;
    response
}

/// Where the line sits, 0 at the top of the picture and 100 at the bottom —
/// shown the other way up, as height above the bottom edge, which is how
/// anyone would describe moving subtitles up.
pub(crate) fn subtitle_position_slider(ui: &mut egui::Ui, position: &mut f64) -> egui::Response {
    let mut height = 100.0 - *position;
    let response = ui.add(
        egui::Slider::new(&mut height, 0.0..=90.0)
            .step_by(1.0)
            .suffix("% up")
            .fixed_decimals(0),
    );
    *position = 100.0 - height;
    response
}

/// A swatch that opens a colour wheel with an opacity slider.
fn colour_button(ui: &mut egui::Ui, colour: &mut Colour) {
    let [r, g, b, a] = colour.to_array();
    let mut picked = egui::Color32::from_rgba_unmultiplied(r, g, b, a);
    // The `Color32` form rather than `Hsva`, because egui remembers the hue
    // behind it: rebuilt from RGB each frame, black and grey have none, and
    // the wheel would snap back to red under the mouse.
    let response = egui::color_picker::color_edit_button_srgba(
        ui,
        &mut picked,
        egui::color_picker::Alpha::OnlyBlend,
    );
    // Only on a change: a round trip through premultiplied alpha can nudge
    // an untouched colour by a step.
    if response.changed() {
        *colour = Colour::from(picked.to_srgba_unmultiplied());
    }
}

/// The font: the installed families to pick from, and a box to type any name
/// into — the only way on a system whose families the app cannot list.
fn font_picker(ui: &mut egui::Ui, font: &mut Option<String>) {
    let filter_id = egui::Id::new("subtitle-font-filter");
    let mut filter: String = ui.data(|data| data.get_temp(filter_id)).unwrap_or_default();
    let families = theme::font_families(ui.ctx());
    egui::ComboBox::from_id_salt("subtitle-font")
        .selected_text(font.as_deref().unwrap_or("Default"))
        .width(200.0)
        .height(320.0)
        // Not on any click: the search box is clicked into.
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut filter)
                    .hint_text("Search or type a font name")
                    .desired_width(f32::INFINITY),
            )
            .request_focus();
            let typed = filter.trim();
            if ui.selectable_label(font.is_none(), "Default").clicked() {
                *font = None;
                ui.close();
            }
            let wanted = typed.to_lowercase();
            let mut exact = false;
            for family in families.unwrap_or_default() {
                if !family.to_lowercase().contains(&wanted) {
                    continue;
                }
                exact |= family.eq_ignore_ascii_case(typed);
                let label = egui::RichText::new(family.as_str());
                if ui
                    .selectable_label(font.as_deref() == Some(family.as_str()), label)
                    .clicked()
                {
                    *font = Some(family.clone());
                    ui.close();
                }
            }
            if families.is_none() {
                ui.label(ui::muted("Reading the installed fonts…"));
            }
            if !typed.is_empty() && !exact && ui.button(format!("Use “{typed}”")).clicked() {
                *font = Some(typed.to_owned());
                ui.close();
            }
        });
    ui.data_mut(|data| data.insert_temp(filter_id, filter));
}

/// A sample line, half on a bright sky and half on night, drawn in the
/// chosen style — so whether it reads on both is visible before a film is.
///
/// An approximation: the app's own font rather than the chosen one, egui's
/// slant for italic, and no blur. Proportions follow mpv's, where sizes are
/// in pixels of a 38-pixel font.
fn subtitle_preview(ui: &mut egui::Ui, style: &SubtitleStyle) {
    const HEIGHT: f32 = 132.0;
    /// mpv's `sub-font-size`, the unit every other size is relative to.
    const MPV_FONT: f32 = 38.0;
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, HEIGHT), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let radius = egui::CornerRadius::same(theme::radius::MD);

    // Sky above haze on the left, a night street on the right.
    let half = egui::Rect::from_min_max(rect.min, egui::pos2(rect.center().x, rect.max.y));
    let mut sky = egui::Mesh::default();
    sky.colored_vertex(half.left_top(), egui::Color32::from_rgb(0x8E, 0xC5, 0xF0));
    sky.colored_vertex(half.right_top(), egui::Color32::from_rgb(0xB8, 0xDC, 0xF8));
    sky.colored_vertex(
        half.right_bottom(),
        egui::Color32::from_rgb(0xFA, 0xF7, 0xEE),
    );
    sky.colored_vertex(
        half.left_bottom(),
        egui::Color32::from_rgb(0xF2, 0xF0, 0xE8),
    );
    sky.add_triangle(0, 1, 2);
    sky.add_triangle(0, 2, 3);
    painter.rect_filled(rect, radius, egui::Color32::from_rgb(0x14, 0x16, 0x22));
    painter.add(sky);

    let font_px = 22.0 * style.scale as f32;
    let unit = font_px / MPV_FONT;
    let family = if style.bold {
        theme::semibold()
    } else {
        egui::FontFamily::Proportional
    };
    let job = egui::text::LayoutJob::single_section(
        "Can you read this on both sides?".to_owned(),
        egui::TextFormat {
            font_id: egui::FontId::new(font_px, family),
            color: egui::Color32::PLACEHOLDER,
            italics: style.italic,
            ..Default::default()
        },
    );
    let galley = painter.layout_job(job);
    let size = galley.size();
    // Bottom of the line at `position` percent down, as mpv places it, kept
    // inside the strip.
    let margin = 8.0;
    let bottom = rect.top() + HEIGHT * style.position as f32 / 100.0 - margin;
    let top = bottom
        .min(rect.bottom() - margin)
        .max(rect.top() + margin + size.y)
        - size.y;
    let at = egui::pos2(rect.center().x - size.x / 2.0, top);
    let colour = |colour: Colour| {
        let [r, g, b, a] = colour.to_array();
        egui::Color32::from_rgba_unmultiplied(r, g, b, a)
    };

    let shadow = style.shadow_offset as f32 * unit;
    match style.backdrop {
        SubtitleBackdrop::Box => {
            let padding = 6.0 * unit;
            let line = egui::Rect::from_min_size(at, size).expand(padding);
            if shadow > 0.0 {
                painter.rect_filled(
                    line.translate(egui::vec2(shadow, shadow)),
                    0.0,
                    colour(style.shadow_colour),
                );
            }
            painter.rect_filled(line, 0.0, colour(style.box_colour));
        }
        SubtitleBackdrop::Outline => {
            let outline = style.outline_size as f32 * unit;
            let rings = |offset: egui::Vec2, ink: egui::Color32| {
                if outline <= 0.0 {
                    painter.galley_with_override_text_color(at + offset, galley.clone(), ink);
                    return;
                }
                for step in 0..8 {
                    let angle = step as f32 * std::f32::consts::TAU / 8.0;
                    let ring = egui::vec2(angle.cos(), angle.sin()) * outline;
                    painter.galley_with_override_text_color(
                        at + offset + ring,
                        galley.clone(),
                        ink,
                    );
                }
            };
            if shadow > 0.0 {
                rings(egui::vec2(shadow, shadow), colour(style.shadow_colour));
            }
            if outline > 0.0 {
                rings(egui::Vec2::ZERO, colour(style.outline_colour));
            }
        }
    }
    painter.galley_with_override_text_color(at, galley, colour(style.colour));
}

/// The streaming cache: how big it may get, and a way to empty it.
fn storage_form(ui: &mut egui::Ui, prefs: Prefs<'_>, actions: &mut Vec<Action>) {
    ui::widgets::settings_group(
        ui,
        "Storage",
        Some(
            "Video already fetched is kept on disk, so watching it again or seeking back costs nothing. Downloads are separate and never count towards this.",
        ),
        |ui| {
            let budget = ui::format_size(prefs.desktop.cache_bytes() as i64);
            let used = match prefs.cache_used {
                Some(used) => format!("{} of {budget} in use", ui::format_size(used as i64)),
                None => format!("Up to {budget}"),
            };
            ui::widgets::settings_row(ui, "Streaming cache", Some(&used), |ui| {
                let choices: Vec<(u32, String)> = CACHE_CHOICES
                    .iter()
                    .map(|&gib| (gib, format!("{gib} GiB")))
                    .collect();
                let labels: Vec<(u32, &str)> = choices
                    .iter()
                    .map(|(gib, label)| (*gib, label.as_str()))
                    .collect();
                if let Some(cache_gib) =
                    ui::widgets::segmented(ui, prefs.desktop.cache_gib, &labels)
                {
                    actions.push(Action::SetDesktopPrefs(DesktopPrefs {
                        cache_gib,
                        ..prefs.desktop
                    }));
                }
            });
            ui::widgets::settings_row(
                ui,
                "Clear the streaming cache",
                Some("Frees the space now. Whatever plays next is fetched again."),
                |ui| {
                    if ui.button("Clear").clicked() {
                        actions.push(Action::ClearStreamCache);
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
        ui.add_space(theme::space::L);
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.label(ui::muted(
            "A share filled by the Proton Drive desktop client carries no thumbnails, so \
                 without this every tile is a pair of initials. Turning it on sends the titles \
                 in your library — not your files, and nothing about what you have watched — to \
                 the provider you choose, over HTTPS, each time a new one appears. With AniList, \
                 the ids it matched — not the titles — then go to ani.zip for episode names and \
                 wide artwork.",
        ));
        ui.add_space(theme::space::L);

        let mut changed = false;
        let mut enabled = settings.enabled;
        if ui::widgets::toggle(ui, &mut enabled, "Look up posters and descriptions").changed() {
            settings.enabled = enabled;
            changed = true;
        }

        if settings.enabled {
            ui.add_space(theme::space::L);
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
                ui.add_space(theme::space::L);
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

            ui.add_space(theme::space::L);
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
            ui.add_space(theme::space::S);
            ui.label(ui::muted(
                "Off. Nothing is sent anywhere, and turning it off also deletes the answers \
                     already stored.",
            ));
        }

        if changed {
            actions.push(Action::SetMetadataConfig(settings.clone()));
        }
        ui.add_space(theme::space::L);
    });
}
