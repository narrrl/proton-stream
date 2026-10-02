//! The controls for whatever is playing, in two densities.
//!
//! [`full`] is the overlay over the picture: everything, laid out to be aimed at
//! from a sofa — a wide seek bar with the file's chapters marked on it, a
//! centred transport cluster, and the pickers off to the right. [`mini`] is the
//! strip along the bottom of the library, where the picture is *not*: it keeps
//! only what someone browsing needs, plus the one control that page has to have
//! — the way back to the video.
//!
//! Both push the same [`Action`]s, so there is one behaviour and two layouts,
//! and neither mutates: this module draws.

use egui::{Color32, CornerRadius, Rect, Sense, Stroke, Vec2, pos2};
use pstr_player::{MAX_VOLUME, Track, TrackKind};

use crate::app::{Action, Adjacent, Page};
use crate::playback::{Command, Playback};
use crate::theme;
use crate::ui;

/// Height of the clickable seek bar. Comfortably larger than the few pixels it
/// draws, because a seek bar you have to aim at is a seek bar nobody uses.
const SEEK_HIT_HEIGHT: f32 = 18.0;

/// How wide the volume slider draws. Wide enough to land on a value, narrow
/// enough that it does not read as a second seek bar.
const VOLUME_WIDTH: f32 = 90.0;

/// Height of the transport row, and the reason it is a constant.
///
/// `Align::Center` in a horizontal layout centres each widget against the row
/// height *known when that widget is added*, and the row only grows as it goes.
/// The play button is the tallest thing in the cluster and sits in the middle,
/// so everything left of it — previous, −10s — is centred against a shorter row
/// and lands a few pixels high. Calling `set_min_height` with the tallest
/// widget's height before adding anything gives every one of them the same
/// height to centre against.
const ROW_HEIGHT_FULL: f32 = 44.0;

/// The same, for the library strip. See [`ROW_HEIGHT_FULL`].
const ROW_HEIGHT_MINI: f32 = 34.0;

/// Whether there is a file before and after this one in the same title.
#[derive(Debug, Clone, Copy, Default)]
pub struct Neighbours {
    pub previous: bool,
    pub next: bool,
}

/// The overlay's controls: the bottom half of the player page.
pub fn full(
    ui: &mut egui::Ui,
    playback: &Playback,
    neighbours: Neighbours,
    actions: &mut Vec<Action>,
) {
    seek_bar(ui, playback, actions, 6.0);
    times(ui, playback, true);
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ui.set_min_height(ROW_HEIGHT_FULL);

        // Left cluster: everything that moves the playhead, in the order a
        // hand reaches for it.
        step_button(
            ui,
            "⏮",
            "Previous (P)",
            neighbours.previous,
            actions,
            |a| {
                a.push(Action::PlayAdjacent(Adjacent::Previous));
            },
        );
        skip_button(ui, "−10s", "Back ten seconds (←)", -10.0, actions);
        play_pause(ui, playback, actions, 44.0);
        skip_button(ui, "+30s", "Forward thirty seconds (→)", 30.0, actions);
        step_button(ui, "⏭", "Next (N)", neighbours.next, actions, |a| {
            a.push(Action::PlayAdjacent(Adjacent::Next));
        });

        ui.add_space(12.0);
        if playback.seeking || playback.buffering || !playback.loaded {
            ui.add(egui::Spinner::new().size(16.0));
            ui.label(ui::muted(if playback.loaded && playback.seeking {
                "seeking"
            } else {
                "buffering"
            }));
        }

        // Right cluster: everything that is a choice rather than a move.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            track_menus(ui, playback, actions);
            ui.add_space(8.0);
            speed_menu(ui, playback, actions);
            ui.add_space(8.0);
            chapter_menu(ui, playback, actions);
            ui.add_space(8.0);
            volume(ui, playback, actions);
        });
    });
}

/// The strip under the library: what is playing, and back to it.
pub fn mini(
    ui: &mut egui::Ui,
    playback: &Playback,
    neighbours: Neighbours,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        ui.set_min_height(ROW_HEIGHT_MINI);

        // The way back to the picture. First, and a real button rather than a
        // label, because leaving the player page with Esc is otherwise a
        // one-way trip: playback carries on with nowhere to watch it.
        if ui
            .add(
                egui::Button::new(theme::Role::Label.rich("Back to video"))
                    .fill(theme::card_hover())
                    .corner_radius(CornerRadius::same(8)),
            )
            .on_hover_text("Show the picture again")
            .clicked()
        {
            actions.push(Action::Goto(Page::Player));
        }
        ui.add_space(10.0);

        ui.vertical(|ui| {
            // Back to what is playing: after ten minutes of browsing, the bar
            // is the only thing that still knows which title this came from.
            if ui
                .add(
                    egui::Label::new(theme::Role::Body.rich(&playback.target.title_name).strong())
                        .sense(Sense::click()),
                )
                .on_hover_text("Show this title")
                .clicked()
            {
                actions.push(Action::Goto(Page::Title(playback.target.title_key.clone())));
            }
            ui.label(ui::muted(playback.target.caption()));
        });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Stop").clicked() {
                actions.push(Action::Player(Command::Stop));
            }
            ui.add_space(6.0);
            step_button(ui, "⏭", "Next (N)", neighbours.next, actions, |a| {
                a.push(Action::PlayAdjacent(Adjacent::Next));
            });
            play_pause(ui, playback, actions, 34.0);
            step_button(
                ui,
                "⏮",
                "Previous (P)",
                neighbours.previous,
                actions,
                |a| {
                    a.push(Action::PlayAdjacent(Adjacent::Previous));
                },
            );

            ui.add_space(8.0);
            track_menus(ui, playback, actions);
            ui.add_space(6.0);
            volume(ui, playback, actions);

            if playback.seeking || playback.buffering || !playback.loaded {
                ui.add_space(6.0);
                ui.add(egui::Spinner::new().size(14.0));
            }
        });
    });

    ui.add_space(4.0);
    seek_bar(ui, playback, actions, 5.0);
    times(ui, playback, false);
}

/// The one button whose position should never move.
///
/// Painted rather than set as `▶`/`⏸` text. A glyph is centred by its advance
/// width, and `U+25B6`'s advance carries side bearings that are not symmetric —
/// in a round button that reads, correctly, as a triangle sitting off to one
/// side. It also comes from whichever fallback font has it, so the shape and its
/// weight vary by platform. Two polygons cost less than either problem.
fn play_pause(ui: &mut egui::Ui, playback: &Playback, actions: &mut Vec<Action>, size: f32) {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());

    let lift = if response.is_pointer_button_down_on() {
        -0.18
    } else if response.hovered() {
        0.12
    } else {
        0.0
    };
    let painter = ui.painter();
    // A rounded rectangle whose corner radius is its half-width *is* a circle,
    // and unlike `circle_filled` it can be painted with a gradient.
    theme::accent_fill(
        painter,
        rect,
        CornerRadius::same((size / 2.0).round() as u8),
        lift,
    );

    let centre = rect.center();
    if playback.paused {
        // A triangle's centroid is a third of the way from base to apex, not
        // half — so centring its bounding box puts the visual mass left of the
        // circle's centre, which is the usual reason a play button looks wrong.
        // Placed by centroid instead: base at `centre.x - width / 3`.
        let (width, height) = (size * 0.30, size * 0.34);
        let base = centre.x - width / 3.0;
        painter.add(egui::Shape::convex_polygon(
            vec![
                pos2(base, centre.y - height / 2.0),
                pos2(base, centre.y + height / 2.0),
                pos2(base + width, centre.y),
            ],
            theme::on_accent(),
            Stroke::NONE,
        ));
    } else {
        // Pause is symmetric, so it is centred the ordinary way.
        let bar = (size * 0.09).round();
        let height = (size * 0.32).round();
        let gap = (size * 0.10).round();
        let y = (centre.y - height / 2.0).round();

        for x in [
            (centre.x - gap / 2.0 - bar).round(),
            (centre.x + gap / 2.0).round(),
        ] {
            painter.rect_filled(
                Rect::from_min_size(pos2(x, y), Vec2::new(bar, height)),
                CornerRadius::same((bar / 2.0).round() as u8),
                theme::on_accent(),
            );
        }
    }

    if response
        .on_hover_text(if playback.paused {
            "Play (Space)"
        } else {
            "Pause (Space)"
        })
        .clicked()
    {
        actions.push(Action::Player(Command::TogglePause));
    }
}

fn skip_button(
    ui: &mut egui::Ui,
    label: &str,
    hover: &str,
    seconds: f64,
    actions: &mut Vec<Action>,
) {
    if ui
        .add(egui::Button::new(label).min_size(Vec2::new(52.0, 30.0)))
        .on_hover_text(hover)
        .clicked()
    {
        actions.push(Action::Player(Command::SeekBy(seconds)));
    }
}

/// Previous/next episode. Drawn even at the ends of a title, disabled: a
/// control that comes and goes is harder to find than one that is greyed.
fn step_button(
    ui: &mut egui::Ui,
    label: &str,
    hover: &str,
    enabled: bool,
    actions: &mut Vec<Action>,
    push: impl FnOnce(&mut Vec<Action>),
) {
    let response = ui.add_enabled(
        enabled,
        egui::Button::new(theme::Role::Subhead.rich(label)).min_size(Vec2::new(38.0, 30.0)),
    );
    if response.on_hover_text(hover).clicked() {
        push(actions);
    }
}

/// Position and duration, at the ends of the seek bar they belong to.
///
/// `over_picture` picks the ink. Muted grey is right on the library's own dark
/// surface, and unreadable over a bright frame — a pale grey at 12 px against
/// a sunlit shot is gone whatever the scrim does, so over the picture this goes
/// to near-white and a point larger.
fn times(ui: &mut egui::Ui, playback: &Playback, over_picture: bool) {
    let (size, ink, dim) = if over_picture {
        (13.0, Color32::WHITE, Color32::from_white_alpha(215))
    } else {
        (12.0, theme::muted(), theme::muted())
    };
    let time = |text: String| egui::RichText::new(text).size(size).color(ink);

    ui.horizontal(|ui| {
        ui.label(time(ui::format_time(playback.position)));
        if let Some(chapter) = playback.chapter().filter(|_| playback.chapters.len() > 1) {
            ui.label(
                egui::RichText::new(format!("· {}", chapter.label()))
                    .size(size)
                    .color(dim),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            match playback.duration {
                Some(duration) => ui.label(time(ui::format_time(duration))),
                None => ui.label(time("—:—".to_owned())),
            };
        });
    });
}

/// Mute, and how loud.
///
/// Drawn into a right-to-left layout, so the slider is added first and ends up
/// to the *right* of the speaker it belongs to.
fn volume(ui: &mut egui::Ui, playback: &Playback, actions: &mut Vec<Action>) {
    // A muted player shows an empty slider rather than the level it will come
    // back to: "muted" and "turned down" look the same on a bar, and only one
    // of them is undone by clicking the speaker.
    let mut value = if playback.muted { 0.0 } else { playback.volume };

    let slider = ui
        .scope(|ui| {
            ui.spacing_mut().slider_width = VOLUME_WIDTH;
            ui.add(
                egui::Slider::new(&mut value, 0.0..=MAX_VOLUME)
                    .show_value(false)
                    .trailing_fill(true),
            )
        })
        .inner;

    if slider.changed() {
        // Committed on release, not on every frame of the drag: this is a file
        // write, and a slider dragged across the bar would be one per frame.
        actions.push(Action::SetVolume {
            volume: value,
            commit: !slider.dragged(),
        });
    }
    if slider.drag_stopped() {
        actions.push(Action::SetVolume {
            volume: value,
            commit: true,
        });
    }

    let icon = if playback.muted || playback.volume <= 0.0 {
        "🔇"
    } else {
        "🔊"
    };
    if ui
        .button(icon)
        .on_hover_text(format!("Mute (M) — {:.0}%", playback.volume))
        .clicked()
    {
        actions.push(Action::ToggleMute);
    }
}

/// The audio and subtitle pickers.
///
/// Each is shown only when there is a choice to make: one audio track is not a
/// menu, and a file with no subtitles should not offer a subtitle button that
/// opens onto nothing. Added subtitles-first so that, in this right-to-left
/// layout, audio reads to the left of subtitles.
fn track_menus(ui: &mut egui::Ui, playback: &Playback, actions: &mut Vec<Action>) {
    let subtitles: Vec<&Track> = playback.tracks_of(TrackKind::Subtitle).collect();
    if !subtitles.is_empty() {
        let selected = playback.selected_track(TrackKind::Subtitle);
        let label = match selected {
            Some(track) => format!("Subs · {}", short(track)),
            None => "Subs · Off".to_string(),
        };
        ui.menu_button(label, |ui| {
            // "Off" first, and always: it is the entry most often wanted, and
            // it is the only one that is not in the file.
            if ui.selectable_label(selected.is_none(), "Off").clicked() {
                actions.push(Action::SelectTrack {
                    kind: TrackKind::Subtitle,
                    id: None,
                });
                ui.close();
            }
            ui.separator();
            track_options(ui, &subtitles, TrackKind::Subtitle, actions);
        })
        .response
        .on_hover_text("Subtitle track");
    }

    let audio: Vec<&Track> = playback.tracks_of(TrackKind::Audio).collect();
    if audio.len() > 1 {
        let label = match playback.selected_track(TrackKind::Audio) {
            Some(track) => format!("Audio · {}", short(track)),
            None => "Audio · Off".to_string(),
        };
        ui.menu_button(label, |ui| {
            track_options(ui, &audio, TrackKind::Audio, actions);
        })
        .response
        .on_hover_text("Audio track");
    }
}

fn track_options(ui: &mut egui::Ui, tracks: &[&Track], kind: TrackKind, actions: &mut Vec<Action>) {
    for track in tracks {
        if ui.selectable_label(track.selected, track.label()).clicked() {
            actions.push(Action::SelectTrack {
                kind,
                id: Some(track.id),
            });
            ui.close();
        }
    }
}

/// How fast it plays. Says so on the button only when it is not normal speed,
/// because that is the one case worth noticing at a glance.
fn speed_menu(ui: &mut egui::Ui, playback: &Playback, actions: &mut Vec<Action>) {
    let label = if (playback.speed - 1.0).abs() < 1e-6 {
        "Speed".to_owned()
    } else {
        format!("Speed · {}", ui::format_speed(playback.speed))
    };
    ui.menu_button(label, |ui| {
        for speed in [0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0] {
            if ui
                .selectable_label(
                    (playback.speed - speed).abs() < 1e-6,
                    ui::format_speed(speed),
                )
                .clicked()
            {
                actions.push(Action::SetSpeed(speed));
                ui.close();
            }
        }
    })
    .response
    .on_hover_text("Playback speed ([ and ])");
}

/// Jump to a chapter. Only drawn for a file that has more than one, which for
/// most releases means anime and not much else.
fn chapter_menu(ui: &mut egui::Ui, playback: &Playback, actions: &mut Vec<Action>) {
    if playback.chapters.len() < 2 {
        return;
    }
    let current = playback.chapter().map(|chapter| chapter.index);
    ui.menu_button("Chapters", |ui| {
        for chapter in &playback.chapters {
            let label = format!("{}   {}", ui::format_time(chapter.start), chapter.label());
            if ui
                .selectable_label(current == Some(chapter.index), label)
                .clicked()
            {
                actions.push(Action::Player(Command::SeekTo(chapter.start)));
                ui.close();
            }
        }
    })
    .response
    .on_hover_text("Chapters");
}

/// A track in as few words as fit on a button: what distinguishes it from the
/// others, which is nearly always its language.
fn short(track: &Track) -> String {
    track
        .language
        .as_deref()
        .map(pstr_player::language_name)
        .or_else(|| track.title.clone())
        .unwrap_or_else(|| format!("Track {}", track.id))
}

/// A bar that shows where playback is, marks the chapters, and seeks where it
/// is clicked.
fn seek_bar(ui: &mut egui::Ui, playback: &Playback, actions: &mut Vec<Action>, thickness: f32) {
    let width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, SEEK_HIT_HEIGHT), Sense::click_and_drag());

    let active = response.hovered() || response.dragged();
    // Grows rather than jumps when the pointer arrives.
    let grow = ui
        .ctx()
        .animate_bool_with_time(response.id.with("grow"), active, 0.12);
    let track = Rect::from_center_size(rect.center(), Vec2::new(width, thickness + 3.0 * grow));
    let radius = CornerRadius::same(3);
    let painter = ui.painter();
    // From the palette rather than a fixed grey: the same bar sits on the
    // video's dark scrim and on a light theme's transport strip.
    painter.rect_filled(track, radius, theme::muted().gamma_multiply(0.35));

    let duration = playback.duration.filter(|duration| *duration > 0.0);
    // What is already read ahead, a shade lighter than the empty track: on a
    // link this slow, whether a jump lands in it is the difference between an
    // instant seek and a fetch.
    if let (Some(duration), Some(end)) = (duration, playback.buffered)
        && end > playback.position
    {
        let from = (playback.position / duration).clamp(0.0, 1.0) as f32;
        let to = (end / duration).clamp(0.0, 1.0) as f32;
        let buffered = Rect::from_min_max(
            egui::pos2(track.left() + track.width() * from, track.top()),
            egui::pos2(track.left() + track.width() * to, track.bottom()),
        );
        painter.rect_filled(buffered, radius, theme::muted().gamma_multiply(0.30));
    }
    let pointer = response
        .interact_pointer_pos()
        .or_else(|| response.hover_pos())
        .map(|position| ((position.x - track.left()) / track.width()).clamp(0.0, 1.0));

    // Where the pointer would land, lighter than what has been played: the bar
    // shows the jump before it is made.
    if let (Some(fraction), true) = (pointer, active) {
        let mut ghost = track;
        ghost.set_width(track.width() * fraction);
        painter.rect_filled(ghost, radius, theme::muted().gamma_multiply(0.45));
    }

    // While dragging, the knob follows the pointer even though the seek waits
    // for the release — see below. A knob that stays put under a moving hand
    // reads as a bar that is not listening.
    let shown = match (response.dragged(), pointer) {
        (true, Some(fraction)) => Some(fraction),
        _ => playback.progress(),
    };
    if let Some(progress) = shown {
        let mut played = track;
        played.set_width(track.width() * progress);
        // The gradient is sized to the *played* part, not to the whole bar, so
        // the far end of it arrives at the playhead rather than at the end of
        // the file. A bar that only ever shows the first fifth of its own ramp
        // does not read as a gradient at all.
        theme::accent_fill(painter, played, radius, 0.0);
        painter.circle_filled(
            egui::pos2(played.right(), track.center().y),
            5.0 + 3.0 * grow,
            Color32::WHITE,
        );
    }

    // Seeking needs a duration: without one there is nothing for a fraction of
    // the bar to mean. Before `FileLoaded` mpv has no timeline anyway.
    let Some(duration) = duration else {
        return;
    };

    // Chapter marks, cut into the bar rather than drawn on it: an opening is
    // two minutes of a twenty-four minute file, and the point of the mark is to
    // be able to aim just past it.
    for chapter in playback.chapters.iter().skip(1) {
        let fraction = (chapter.start / duration).clamp(0.0, 1.0) as f32;
        let x = track.left() + track.width() * fraction;
        painter.rect_filled(
            Rect::from_min_size(
                egui::pos2(x - 1.5, track.top() - 1.0),
                Vec2::new(3.0, track.height() + 2.0),
            ),
            CornerRadius::ZERO,
            Color32::from_black_alpha(200),
        );
    }

    // On release, not on every frame of the drag: each seek cancels outstanding
    // read-ahead and refetches, so a dragged bar that seeks continuously spends
    // the whole drag throwing away blocks it just paid for.
    if let Some(fraction) = pointer
        && (response.clicked() || response.drag_stopped())
    {
        actions.push(Action::Player(Command::SeekTo(fraction as f64 * duration)));
    }

    if let (Some(fraction), true) = (pointer, active) {
        let at = f64::from(fraction) * duration;
        // The chapter under the pointer, when there is one: "42:10 · OP" tells
        // a viewer what they are about to land in, which is the whole reason to
        // aim at a particular part of the bar.
        let hint = match pstr_player::chapter_at(&playback.chapters, at) {
            Some(index) => format!(
                "{}  ·  {}",
                ui::format_time(at),
                playback.chapters[index].label()
            ),
            None => ui::format_time(at),
        };
        bubble(
            ui,
            egui::pos2(track.left() + track.width() * fraction, track.top() - 10.0),
            &hint,
        );
    }
}

/// A small label floating above a point, kept inside the window.
fn bubble(ui: &egui::Ui, anchor: egui::Pos2, text: &str) {
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new("seek-bubble"),
    ));
    let galley = painter.layout_no_wrap(text.to_owned(), theme::Role::Label.font(), Color32::WHITE);
    let size = galley.size() + Vec2::new(16.0, 8.0);
    let screen = ui.ctx().content_rect();
    let left = (anchor.x - size.x / 2.0).clamp(screen.left() + 4.0, screen.right() - size.x - 4.0);
    let frame = Rect::from_min_size(egui::pos2(left, anchor.y - size.y), size);
    painter.rect_filled(frame, CornerRadius::same(6), Color32::from_black_alpha(215));
    painter.galley(frame.min + Vec2::new(8.0, 4.0), galley, Color32::WHITE);
}
