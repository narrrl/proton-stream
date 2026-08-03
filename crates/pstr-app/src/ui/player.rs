//! The player page: the picture, filling the window, with controls over it.
//!
//! The video is one `painter.image` call — everything that made it a texture is
//! in [`crate::video`]. What is left here is the part a viewer notices: the
//! controls fade out of the way while something is playing and come back on the
//! first movement of the mouse, which is the behaviour every player has and the
//! only reason a full-bleed picture is usable at all.
//!
//! Two things are deliberately *not* subject to that fade. The skip-opening
//! button, which is only up for the ninety seconds it applies to and is useless
//! if you have to wake the controls to reach it; and the whole page while
//! nothing is playing — between two episodes there is no picture to keep clear
//! of, only a black screen that has to say what it is doing.
//!
//! Nothing in this module mutates. Like every other page it collects
//! [`Action`]s, so a click on "back" can change the page it was drawn on.

use egui::{Align2, Color32, CornerRadius, Rect, Sense, Vec2};

use crate::app::{Action, Adjacent};
use crate::playback::{Command, Playback};
use crate::theme;
use crate::ui::{self, transport::Neighbours};

/// How long the controls stay up after the pointer stops moving.
///
/// Long enough to reach for them again after glancing away, short enough that
/// they are gone by the time anyone is annoyed by them.
const IDLE_SECONDS: f64 = 2.5;

/// The pointer has to move at least this far, in points, to count as movement.
/// A trackpad reports jitter of a pixel or so while nobody is touching it, and
/// without a floor the controls never hide.
const MOVEMENT: f32 = 1.5;

/// Padding around the strip of controls along the bottom.
///
/// The height is *not* a constant. The controls are two rows of buttons whose
/// size follows the theme and the platform's text scaling, and a fixed strip
/// that guesses low puts the buttons past the bottom edge of the window — which
/// is what a 132 px constant did here.
const CHROME_PAD_X: f32 = 26.0;
const CHROME_PAD_BOTTOM: f32 = 18.0;
const CHROME_PAD_TOP: f32 = 16.0;

/// What to assume the controls are worth before they have been drawn once.
/// Only ever wrong for a single frame, and only on the first one.
const CHROME_HEIGHT_GUESS: f32 = 116.0;

/// Height of the title strip along the top.
const TITLE_HEIGHT: f32 = 68.0;

/// Gap between the controls and whatever floats above them.
const FLOAT_GAP: f32 = 12.0;

/// The skip button: how tall, and how much room the label gets either side.
/// The height doubles as its corner radius, which is what makes it a pill.
const SKIP_HEIGHT: f32 = 40.0;
const SKIP_PAD_X: f32 = 22.0;

/// How dark the scrim gets behind the controls, and how far above them it takes
/// to get there.
///
/// The fade lives in the empty picture *above* the controls rather than across
/// them, so every row of them — the seek bar and the timecodes included — sits
/// on the full wash. See [`scrim_bottom`].
const SCRIM_ALPHA: f32 = 170.0;
const SCRIM_FADE: f32 = 88.0;

/// State the page keeps between frames.
///
/// Held by the app and passed in by reference, because one thing here *is*
/// written while drawing: how tall the controls turned out. Everything the
/// viewer can see still comes from the app's state.
#[derive(Debug, Clone, Copy)]
pub struct Overlay {
    /// When the pointer last moved, on egui's clock. `None` until the page has
    /// seen a frame, which is what keeps the controls up on arrival — a viewer
    /// who has just clicked "play" should see what the controls are before they
    /// fade.
    last_movement: Option<f64>,
    /// Where the pointer was, to tell movement from jitter.
    last_position: Option<egui::Pos2>,
    /// How tall the controls were the last time they were drawn, padding
    /// included. Measured rather than assumed — see [`CHROME_PAD_X`].
    chrome_height: f32,
}

impl Default for Overlay {
    fn default() -> Self {
        Self {
            last_movement: None,
            last_position: None,
            chrome_height: CHROME_HEIGHT_GUESS + CHROME_PAD_TOP + CHROME_PAD_BOTTOM,
        }
    }
}

/// Everything the page draws *over* the picture this frame.
///
/// Bundled because they arrive together and mean one thing between them — what
/// the controls are doing — and because passing them one at a time made the
/// page's entry point eight arguments long.
pub struct Chrome<'a> {
    /// Whether the controls are up, and how tall they were last time. The one
    /// piece of state drawing is allowed to write to.
    pub overlay: &'a mut Overlay,
    /// The end-of-episode card, when the app has decided there is one.
    pub up_next: Option<UpNextCard>,
    /// Whether stepping to another episode is possible in either direction.
    pub neighbours: Neighbours,
}

/// What the end-of-episode card says, when there is one.
///
/// The app decides *whether* to show it and counts it down; this is only what
/// to draw. See [`crate::app::UpNext`].
pub struct UpNextCard {
    /// Seconds until the next episode starts.
    pub seconds: f64,
    /// Which episode that is.
    pub caption: String,
}

impl Overlay {
    /// Fold this frame's pointer into the overlay's timer.
    pub fn observe(&mut self, ctx: &egui::Context) {
        let (now, pointer) = ctx.input(|input| (input.time, input.pointer.latest_pos()));
        let last_movement = *self.last_movement.get_or_insert(now);

        let moved = match (pointer, self.last_position) {
            (Some(current), Some(previous)) => current.distance(previous) > MOVEMENT,
            (Some(_), None) => true,
            (None, _) => false,
        };
        self.last_movement = Some(if moved { now } else { last_movement });
        if let Some(current) = pointer {
            self.last_position = Some(current);
        }
    }

    /// Whether the controls should be on screen.
    ///
    /// Always, while paused or still opening: a still picture with no controls
    /// looks like a crash, and there is nothing to be distracted from.
    fn visible(&self, ctx: &egui::Context, playback: &Playback) -> bool {
        if playback.paused || !playback.loaded {
            return true;
        }
        // A track or chapter menu is open. Hiding the controls would take the
        // button the menu hangs off the screen with it, which closes the menu —
        // so the viewer who stops to read a list of chapters loses the list.
        if egui::Popup::is_any_open(ctx) {
            return true;
        }
        let Some(last_movement) = self.last_movement else {
            return true;
        };
        ctx.input(|input| input.time) - last_movement < IDLE_SECONDS
    }
}

/// Draw the picture and its controls.
///
/// `playback` is `None` in the gap between one episode ending and the next
/// having opened — the page stays, because that is where the viewer means to
/// be, and `opening` is what it says while they wait.
pub fn show(
    ui: &mut egui::Ui,
    frame: &mut eframe::Frame,
    playback: Option<&mut Playback>,
    opening: Option<&str>,
    chrome_state: Chrome<'_>,
    actions: &mut Vec<Action>,
) {
    let Chrome {
        overlay,
        up_next,
        neighbours,
    } = chrome_state;
    let ctx = ui.ctx().clone();
    let rect = ui.available_rect_before_wrap();
    if rect.width() < 1.0 || rect.height() < 1.0 {
        return;
    }

    // Black rather than the app background: this is a cinema, and the letterbox
    // bars mpv paints inside the texture have to be the same colour as what
    // surrounds them.
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, Color32::BLACK);

    let Some(playback) = playback else {
        between(ui, opening, rect, actions);
        return;
    };

    let response = ui.allocate_rect(rect, Sense::click());
    let picture = paint_video(ui, frame, playback, rect);

    if !picture {
        waiting(ui, playback, rect);
    }

    // Click anywhere on the picture to pause, which is the one control that
    // should not require aiming at anything.
    if response.clicked() {
        actions.push(Action::Player(Command::TogglePause));
    }
    if response.double_clicked() {
        actions.push(Action::ToggleFullscreen);
    }

    let visible = overlay.visible(&ctx, playback);
    // Above the chrome when it is up, at the bottom corner when it is not —
    // and clear of it either way, which is what the measured height buys.
    let float_above = if visible {
        overlay.chrome_height + FLOAT_GAP
    } else {
        28.0
    };
    match up_next {
        // The card supersedes the skip button: both live in the same corner,
        // and both offer a way out of the same ninety seconds.
        Some(card) => up_next_card(ui, &card, rect, float_above, actions),
        None => skip(ui, playback, rect, float_above, actions),
    }

    if visible {
        overlay.chrome_height = chrome(
            ui,
            playback,
            neighbours,
            rect,
            overlay.chrome_height,
            actions,
        );
    } else {
        ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        // The controls have to disappear on their own, and nothing else is
        // going to cause a frame while a film plays without the mouse moving.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));
    }
}

/// Render this frame's picture into the rectangle. Reports whether there was
/// one — a player whose first frame has not arrived has nothing to draw.
fn paint_video(
    ui: &mut egui::Ui,
    frame: &mut eframe::Frame,
    playback: &mut Playback,
    rect: Rect,
) -> bool {
    let pixels_per_point = ui.ctx().pixels_per_point();
    let Some(video) = playback.video.as_mut() else {
        // mpv has a window of its own; there is nothing to composite here.
        return false;
    };

    let size = [
        (rect.width() * pixels_per_point).round() as i32,
        (rect.height() * pixels_per_point).round() as i32,
    ];
    let Some(texture) = video.texture(frame, size) else {
        return false;
    };
    if !video.has_picture() {
        return false;
    }

    ui.painter().image(
        texture,
        rect,
        // The whole texture: mpv has already scaled the picture into it and
        // letterboxed the remainder, so there is nothing to crop.
        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    true
}

/// What fills the screen before the first frame.
fn waiting(ui: &mut egui::Ui, playback: &Playback, rect: Rect) {
    let painter = ui.painter();
    painter.text(
        rect.center() - Vec2::new(0.0, 18.0),
        Align2::CENTER_CENTER,
        &playback.target.title_name,
        egui::FontId::proportional(20.0),
        theme::text(),
    );
    painter.text(
        rect.center() + Vec2::new(0.0, 10.0),
        Align2::CENTER_CENTER,
        if playback.is_embedded() {
            "opening…"
        } else {
            "playing in mpv's own window"
        },
        egui::FontId::proportional(13.0),
        theme::muted(),
    );
}

/// The screen between two episodes: one has ended and the next is opening.
///
/// It keeps a way out, because opening a file over a public link can take
/// seconds and a black screen with no controls is indistinguishable from a
/// hang.
fn between(ui: &mut egui::Ui, opening: Option<&str>, rect: Rect, actions: &mut Vec<Action>) {
    ui.painter().text(
        rect.center() - Vec2::new(0.0, 16.0),
        Align2::CENTER_CENTER,
        "Up next",
        egui::FontId::proportional(14.0),
        theme::muted(),
    );
    ui.painter().text(
        rect.center() + Vec2::new(0.0, 12.0),
        Align2::CENTER_CENTER,
        opening.unwrap_or("opening…"),
        egui::FontId::proportional(20.0),
        theme::text(),
    );

    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_size(
                rect.min + Vec2::new(16.0, 14.0),
                Vec2::new(240.0, 34.0),
            ))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            if ui.button("Back").clicked() {
                actions.push(Action::LeavePlayer);
            }
            ui.add_space(8.0);
            ui.add(egui::Spinner::new().size(14.0));
        },
    );
}

/// "Skip opening", when the playhead is inside a chapter that says it is one.
///
/// Bottom right, the corner every service puts it in, and drawn whether or not
/// the rest of the controls are up: an opening lasts ninety seconds, and a
/// button you have to wiggle the mouse to find is one you skip by hand instead.
fn skip(
    ui: &mut egui::Ui,
    playback: &Playback,
    rect: Rect,
    bottom_margin: f32,
    actions: &mut Vec<Action>,
) {
    let Some((label, to)) = playback.skippable() else {
        return;
    };

    // Measured from the label rather than fixed: "Skip opening" and "Skip
    // recap" are different lengths, and one constant leaves the shorter of them
    // adrift in a box of its own whitespace, which is what made this read as a
    // rectangle with some text in it instead of a button.
    let font = egui::FontId::proportional(14.0);
    let text_width = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), Color32::WHITE)
        .size()
        .x;
    let size = Vec2::new((text_width + SKIP_PAD_X * 2.0).round(), SKIP_HEIGHT);

    floating(ui, rect, size, bottom_margin, |ui| {
        // Buttons given an explicit fill keep it in every state, so hover has
        // to be answered by hand — and a control that does not answer the
        // pointer at all is the other half of looking dead.
        let hovered = ui.rect_contains_pointer(ui.max_rect());
        let (fill, stroke) = if hovered {
            (Color32::WHITE, Color32::WHITE)
        } else {
            (
                Color32::from_black_alpha(170),
                Color32::from_white_alpha(110),
            )
        };
        let text = if hovered {
            Color32::from_rgb(0x14, 0x14, 0x18)
        } else {
            Color32::WHITE
        };

        // Centred *and* justified. A button padded out by `min_size` puts its
        // label wherever the surrounding layout aligns to, and `floating` aligns
        // right — which left the label against the right edge with the box
        // reaching past it. This fills the area and centres the label in it, so
        // the padding lands evenly on both sides.
        let clicked = ui
            .centered_and_justified(|ui| {
                ui.add(
                    egui::Button::new(egui::RichText::new(label).font(font).strong().color(text))
                        .fill(fill)
                        .stroke(egui::Stroke::new(1.0, stroke))
                        // A pill, like every other floating control here. The
                        // sharp 6 px corner was the one square thing on the
                        // picture.
                        .corner_radius(CornerRadius::same((SKIP_HEIGHT / 2.0) as u8)),
                )
                .on_hover_text(format!("Jump to {}", ui::format_time(to)))
                .clicked()
            })
            .inner;
        if clicked {
            actions.push(Action::Player(Command::SeekTo(to)));
        }
    });
}

/// "Up next", counting down, with the two ways out of it.
///
/// The countdown itself belongs to the app — see [`crate::app::UpNext`] — so
/// this only draws what it was told and reports what was clicked. Both buttons
/// are real buttons rather than one button and a timer: a viewer who wants the
/// next episode *now* should not have to wait out a countdown that exists to
/// give them time to say no.
fn up_next_card(
    ui: &mut egui::Ui,
    card: &UpNextCard,
    rect: Rect,
    bottom_margin: f32,
    actions: &mut Vec<Action>,
) {
    let size = Vec2::new(330.0, 104.0);

    floating(ui, rect, size, bottom_margin, |ui| {
        egui::Frame::new()
            .fill(Color32::from_black_alpha(215))
            .stroke(egui::Stroke::new(1.0, Color32::from_white_alpha(120)))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.set_width(size.x - 28.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Up next")
                            .size(12.0)
                            .strong()
                            .color(Color32::from_white_alpha(200)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Rounded up, so a card that says "1s" is never
                        // followed by a second of nothing happening.
                        ui.label(
                            egui::RichText::new(format!("in {}s", card.seconds.ceil() as u32))
                                .size(12.0)
                                .color(theme::accent()),
                        );
                    });
                });
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&card.caption)
                            .size(14.0)
                            .strong()
                            .color(Color32::WHITE),
                    )
                    .truncate(),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui::accent_button(ui, "Play now").clicked() {
                        actions.push(Action::PlayAdjacent(Adjacent::Next));
                    }
                    ui.add_space(6.0);
                    if ui
                        .button("Watch till the end")
                        .on_hover_text("Play this one out — nothing will be skipped")
                        .clicked()
                    {
                        actions.push(Action::WatchToEnd);
                    }
                });
            });
    });
}

/// Put something in the bottom-right corner, `bottom_margin` above the edge.
///
/// The inset matches the controls' own padding rather than being its own
/// number, so a floating button lines up with the volume slider under it
/// instead of sitting four pixels off it.
fn floating(
    ui: &mut egui::Ui,
    rect: Rect,
    size: Vec2,
    bottom_margin: f32,
    add: impl FnOnce(&mut egui::Ui),
) {
    let area = Rect::from_min_size(
        egui::pos2(
            rect.right() - CHROME_PAD_X - size.x,
            rect.bottom() - bottom_margin - size.y,
        ),
        size,
    );
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(area)
            .layout(egui::Layout::top_down(egui::Align::Max)),
        add,
    );
}

/// The controls: a title strip at the top, the transport at the bottom.
///
/// Returns how tall the bottom strip came out, padding included, so the next
/// frame can put its scrim behind it and float the skip button clear of it. It
/// is measured because it is not knowable: the transport is two rows of themed
/// buttons, and on a display that scales text they are taller than they are
/// here. Guessing low is what put the buttons through the bottom of the window.
fn chrome(
    ui: &mut egui::Ui,
    playback: &Playback,
    neighbours: Neighbours,
    rect: Rect,
    previous_height: f32,
    actions: &mut Vec<Action>,
) -> f32 {
    let top = Rect::from_min_size(rect.min, Vec2::new(rect.width(), TITLE_HEIGHT));
    scrim_top(ui, top);
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(top.shrink2(Vec2::new(18.0, 14.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            if ui
                .add(
                    egui::Button::new(egui::RichText::new("Library").size(13.0))
                        .fill(Color32::from_black_alpha(160))
                        .corner_radius(CornerRadius::same(8))
                        .min_size(Vec2::new(96.0, 32.0)),
                )
                .on_hover_text("Keep playing, and go back to the library (Esc)")
                .clicked()
            {
                actions.push(Action::LeavePlayer);
            }
            ui.add_space(14.0);
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(&playback.target.title_name)
                        .size(18.0)
                        .strong()
                        .color(Color32::WHITE),
                );
                ui.label(
                    egui::RichText::new(playback.target.caption())
                        .size(13.0)
                        .color(Color32::from_white_alpha(190)),
                );
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new("⛶").size(15.0))
                            .fill(Color32::from_black_alpha(160))
                            .corner_radius(CornerRadius::same(8))
                            .min_size(Vec2::new(38.0, 32.0)),
                    )
                    .on_hover_text("Fullscreen (F)")
                    .clicked()
                {
                    actions.push(Action::ToggleFullscreen);
                }
            });
        },
    );

    // Last frame's measurement places the scrim and the first row of controls;
    // this frame's is returned for the next one. That is one frame of lag, and
    // only while the window is being resized.
    let strip = Rect::from_min_size(
        egui::pos2(rect.left(), rect.bottom() - previous_height),
        Vec2::new(rect.width(), previous_height),
    );
    scrim_bottom(ui, strip);

    // Anchored to the *bottom* of the window rather than laid out from the top
    // of the strip: whatever the controls turn out to be worth, the last row of
    // them ends a fixed distance above the edge instead of running off it.
    let content = Rect::from_min_max(
        egui::pos2(rect.left() + CHROME_PAD_X, strip.top() + CHROME_PAD_TOP),
        egui::pos2(
            rect.right() - CHROME_PAD_X,
            rect.bottom() - CHROME_PAD_BOTTOM,
        ),
    );
    let measured = ui
        .scope_builder(
            egui::UiBuilder::new()
                .max_rect(content)
                .layout(egui::Layout::top_down(egui::Align::Min)),
            |ui| {
                ui::transport::full(ui, playback, neighbours, actions);
                ui.min_rect().height()
            },
        )
        .inner;

    measured + CHROME_PAD_TOP + CHROME_PAD_BOTTOM
}

/// The gradient behind the title strip, fading downwards out of the top edge.
///
/// Drawn as a few bands rather than a real gradient — egui has no mesh gradient
/// primitive, and at this size the banding is invisible.
fn scrim_top(ui: &egui::Ui, rect: Rect) {
    const BANDS: usize = 10;
    let painter = ui.painter();
    let band = rect.height() / BANDS as f32;
    for index in 0..BANDS {
        let fraction = 1.0 - index as f32 / (BANDS - 1) as f32;
        let strip = Rect::from_min_size(
            egui::pos2(rect.left(), rect.top() + band * index as f32),
            Vec2::new(rect.width(), band + 1.0),
        );
        painter.rect_filled(
            strip,
            CornerRadius::ZERO,
            // Squared, so the fade is dark where the text is and gone well
            // before the middle of the picture — a linear ramp over a strip
            // this tall greys the whole top third of the frame.
            Color32::from_black_alpha((fraction * fraction * 225.0) as u8),
        );
    }
}

/// The wash behind the controls: flat over the whole strip, fading in above it.
///
/// A gradient that starts fading only where the controls start puts the top of
/// them — the seek bar, the position and the chapter name — over what is very
/// nearly bare picture, and over a bright frame that text disappears. So the
/// strip itself is one flat wash, and the ramp happens in the empty picture
/// above it, where there is nothing to obscure.
fn scrim_bottom(ui: &egui::Ui, strip: Rect) {
    const BANDS: usize = 12;
    let painter = ui.painter();
    painter.rect_filled(
        strip,
        CornerRadius::ZERO,
        Color32::from_black_alpha(SCRIM_ALPHA as u8),
    );

    let band = SCRIM_FADE / BANDS as f32;
    for index in 0..BANDS {
        let fraction = (index + 1) as f32 / BANDS as f32;
        let rect = Rect::from_min_size(
            egui::pos2(strip.left(), strip.top() - SCRIM_FADE + band * index as f32),
            Vec2::new(strip.width(), band + 1.0),
        );
        painter.rect_filled(
            rect,
            CornerRadius::ZERO,
            // Squared again: the wash should meet the strip at full strength
            // and be gone within a finger's width of picture above it.
            Color32::from_black_alpha((fraction * fraction * SCRIM_ALPHA) as u8),
        );
    }
}
