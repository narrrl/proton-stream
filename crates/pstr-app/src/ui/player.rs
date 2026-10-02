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
/// How wide the episode list is, at most. A narrow window gives it less.
const DRAWER_WIDTH: f32 = 380.0;
/// The stills in the episode list.
const DRAWER_STILL: Vec2 = Vec2::new(112.0, 63.0);

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
    /// The title's episodes, when it has more than one — what the list down
    /// the right of the picture shows.
    pub episodes: Option<Episodes<'a>>,
}

/// The playing title's episodes, for the list beside the picture.
pub struct Episodes<'a> {
    pub title: &'a pstr_core::library::Title,
    pub art: ui::Art<'a>,
    /// Whether the list is out.
    pub open: bool,
}

/// What the end-of-episode card says, when there is one.
///
/// The app decides *whether* to show it and counts it down; this is only what
/// to draw. See [`crate::app::UpNext`].
pub struct UpNextCard {
    /// Seconds until the next episode starts.
    pub seconds: f64,
    /// The same, as a fraction of the whole countdown, for the ring.
    pub left: f32,
    /// Which episode that is.
    pub caption: String,
    /// Its still, once there is one.
    pub still: Option<egui::TextureHandle>,
}

impl Overlay {
    /// How much of the bottom of the window the controls take, the last time
    /// they were drawn — so things floating over the picture can keep clear.
    pub fn covered(&self) -> f32 {
        self.chrome_height
    }

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
        mut episodes,
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
    } else if playback.buffering || playback.seeking {
        stalled(ui, rect);
    }

    // Click anywhere on the picture to pause, which is the one control that
    // should not require aiming at anything.
    if response.clicked() {
        actions.push(Action::Player(Command::TogglePause));
    }
    if response.double_clicked() {
        // The first half of a double click already paused, and a viewer who
        // double-clicked asked for fullscreen, not for a pause — so the second
        // half takes the pause back. Waiting out the double-click delay before
        // pausing would make every single click feel late instead.
        actions.push(Action::Player(Command::TogglePause));
        actions.push(Action::ToggleFullscreen);
    }

    let drawer_open = episodes.as_ref().is_some_and(|list| list.open);
    // The controls stay up while the list is out: it hangs off the button in
    // the title strip, and is read at leisure.
    let visible = overlay.visible(&ctx, playback) || drawer_open;
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

    // Last frame's height, like everything else that has to clear the controls.
    playback.lift_subtitles(
        if visible { overlay.chrome_height } else { 0.0 },
        rect.height(),
    );

    // Faded rather than switched, both ways: controls that blink out under a
    // reading eye are more distracting than the ones that were there.
    let fade = ctx.animate_bool_with_time(ui.id().with("chrome"), visible, theme::motion::FADE);
    if fade > 0.0 {
        overlay.chrome_height = ui
            .scope(|ui| {
                ui.multiply_opacity(fade);
                chrome(
                    ui,
                    playback,
                    neighbours,
                    episodes.as_ref().map(|list| list.open),
                    rect,
                    overlay.chrome_height,
                    actions,
                )
            })
            .inner;
    }
    if let Some(list) = episodes.as_mut().filter(|list| list.open) {
        episode_drawer(&ctx, list, playback, rect, overlay.chrome_height, actions);
    } else {
        ctx.data_mut(|data| data.remove::<String>(egui::Id::new(DRAWER_SCROLLED)));
    }
    if !visible {
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
        theme::Role::Title.font(),
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
        theme::Role::Label.font(),
        theme::muted(),
    );
}

/// A spinner over a picture held waiting for data. Without it, a frozen frame
/// with the controls hidden looks the same as a hang.
fn stalled(ui: &mut egui::Ui, rect: Rect) {
    let disc = Rect::from_center_size(rect.center(), Vec2::splat(64.0));
    ui.painter()
        .circle_filled(disc.center(), 32.0, Color32::from_black_alpha(140));
    ui.put(
        Rect::from_center_size(disc.center(), Vec2::splat(36.0)),
        egui::Spinner::new().size(36.0).color(Color32::WHITE),
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
        theme::Role::Body.font(),
        theme::muted(),
    );
    ui.painter().text(
        rect.center() + Vec2::new(0.0, 12.0),
        Align2::CENTER_CENTER,
        opening.unwrap_or("opening…"),
        theme::Role::Title.font(),
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
            ui.add_space(theme::space::M);
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
    let font = theme::Role::Body.font();
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
    const STILL: Vec2 = Vec2::new(128.0, 72.0);
    let size = Vec2::new(400.0, 104.0);

    floating(ui, rect, size, bottom_margin, |ui| {
        egui::Frame::new()
            .fill(Color32::from_black_alpha(215))
            .stroke(egui::Stroke::new(1.0, Color32::from_white_alpha(120)))
            .corner_radius(CornerRadius::same(theme::radius::MD))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(size.x - 24.0);
                ui.horizontal(|ui| {
                    // The still, with the countdown running round it: what is
                    // coming and when, read in one glance.
                    let (frame, response) = ui.allocate_exact_size(STILL, Sense::click());
                    let painter = ui.painter();
                    let radius = CornerRadius::same(theme::radius::SM);
                    match &card.still {
                        Some(texture) => {
                            painter.add(
                                egui::epaint::RectShape::filled(frame, radius, Color32::WHITE)
                                    .with_texture(
                                        texture.id(),
                                        ui::cover_uv(texture.size_vec2(), frame.size()),
                                    ),
                            );
                        }
                        None => {
                            painter.rect_filled(frame, radius, Color32::from_white_alpha(20));
                        }
                    }
                    painter.rect_filled(frame, radius, Color32::from_black_alpha(90));
                    countdown_ring(painter, frame.center(), 22.0, card.left);
                    // Rounded up, so a ring that says "1" is never followed by
                    // a second of nothing happening.
                    painter.text(
                        frame.center(),
                        Align2::CENTER_CENTER,
                        card.seconds.ceil() as u32,
                        theme::Role::Body.font(),
                        Color32::WHITE,
                    );
                    if response.on_hover_text("Play now").clicked() {
                        actions.push(Action::PlayAdjacent(Adjacent::Next));
                    }

                    ui.add_space(theme::space::L);
                    ui.vertical(|ui| {
                        ui.label(
                            theme::Role::Caption
                                .rich("Up next")
                                .strong()
                                .color(Color32::from_white_alpha(200)),
                        );
                        ui.add(
                            egui::Label::new(
                                theme::Role::Body
                                    .rich(&card.caption)
                                    .strong()
                                    .color(Color32::WHITE),
                            )
                            .truncate(),
                        );
                        ui.add_space(theme::space::S);
                        ui.horizontal(|ui| {
                            if ui::accent_button(ui, "Play now").clicked() {
                                actions.push(Action::PlayAdjacent(Adjacent::Next));
                            }
                            ui.add_space(theme::space::S);
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
            });
    });
}

/// A ring that empties clockwise from twelve o'clock as `left` runs to zero.
fn countdown_ring(painter: &egui::Painter, center: egui::Pos2, radius: f32, left: f32) {
    painter.circle_stroke(
        center,
        radius,
        egui::Stroke::new(3.0, Color32::from_white_alpha(60)),
    );
    let left = left.clamp(0.0, 1.0);
    if left <= 0.0 {
        return;
    }
    let steps = (64.0 * left).ceil().max(2.0) as usize;
    let sweep = std::f32::consts::TAU * left;
    let points: Vec<egui::Pos2> = (0..=steps)
        .map(|step| {
            let angle = -std::f32::consts::FRAC_PI_2 + sweep * step as f32 / steps as f32;
            center + radius * Vec2::new(angle.cos(), angle.sin())
        })
        .collect();
    painter.add(egui::Shape::line(
        points,
        egui::Stroke::new(3.0, theme::accent()),
    ));
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
    drawer: Option<bool>,
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
                    egui::Button::new(theme::Role::Label.rich("Library"))
                        .fill(Color32::from_black_alpha(160))
                        .corner_radius(CornerRadius::same(theme::radius::MD))
                        .min_size(Vec2::new(96.0, 32.0)),
                )
                .on_hover_text("Keep playing, and go back to the library (Esc)")
                .clicked()
            {
                actions.push(Action::LeavePlayer);
            }
            ui.add_space(theme::space::L);
            ui.vertical(|ui| {
                ui.label(
                    theme::Role::Heading
                        .rich(&playback.target.title_name)
                        .strong()
                        .color(Color32::WHITE),
                );
                ui.label(
                    theme::Role::Label
                        .rich(playback.target.caption())
                        .color(Color32::from_white_alpha(190)),
                );
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let fullscreen = ui.input(|input| input.viewport().fullscreen.unwrap_or(false));
                if ui
                    .add(
                        egui::Button::new(theme::Role::Subhead.rich(if fullscreen {
                            egui_phosphor::regular::CORNERS_IN
                        } else {
                            egui_phosphor::regular::CORNERS_OUT
                        }))
                        .fill(Color32::from_black_alpha(160))
                        .corner_radius(CornerRadius::same(theme::radius::MD))
                        .min_size(Vec2::new(38.0, 32.0)),
                    )
                    .on_hover_text(if fullscreen {
                        "Leave fullscreen (F)"
                    } else {
                        "Fullscreen (F)"
                    })
                    .clicked()
                {
                    actions.push(Action::ToggleFullscreen);
                }
                if let Some(open) = drawer {
                    ui.add_space(theme::space::S);
                    let fill = if open {
                        theme::accent()
                    } else {
                        Color32::from_black_alpha(160)
                    };
                    let ink = if open {
                        theme::on_accent()
                    } else {
                        Color32::WHITE
                    };
                    if ui
                        .add(
                            egui::Button::new(
                                theme::Role::Subhead
                                    .rich(egui_phosphor::regular::LIST_BULLETS)
                                    .color(ink),
                            )
                            .fill(fill)
                            .corner_radius(CornerRadius::same(theme::radius::MD))
                            .min_size(Vec2::new(38.0, 32.0)),
                        )
                        .on_hover_text("Episodes (E)")
                        .clicked()
                    {
                        actions.push(Action::ToggleEpisodes);
                    }
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

/// Which file the episode list last scrolled to, in egui's memory, so it
/// scrolls to the playing one once when it opens rather than every frame.
const DRAWER_SCROLLED: &str = "episode-drawer-scrolled";

/// The title's episodes, down the right of the picture between the title
/// strip and the controls. A season at a time, opened on the playing one.
fn episode_drawer(
    ctx: &egui::Context,
    list: &mut Episodes<'_>,
    playback: &Playback,
    rect: Rect,
    chrome_height: f32,
    actions: &mut Vec<Action>,
) {
    let width = DRAWER_WIDTH.min(rect.width() * 0.45);
    let area = Rect::from_min_max(
        egui::pos2(rect.right() - width - 16.0, rect.top() + TITLE_HEIGHT),
        egui::pos2(rect.right() - 16.0, rect.bottom() - chrome_height - 8.0),
    );
    if area.height() < 160.0 {
        return;
    }
    let title = list.title;
    let target = &playback.target;
    let is_playing = |episode: &pstr_core::library::Episode| {
        episode.node.share_id == target.share_id && episode.node.link_id == target.link_id
    };
    let playing_season = title
        .seasons
        .iter()
        .position(|season| season.episodes.iter().any(is_playing))
        .unwrap_or(0);
    let season_id = egui::Id::new(("episode-drawer-season", &title.key));
    let mut season = ctx
        .data(|data| data.get_temp::<usize>(season_id))
        .unwrap_or(playing_season)
        .min(title.seasons.len().saturating_sub(1));

    let margin = theme::space::L;
    egui::Area::new(egui::Id::new("episode-drawer"))
        .order(egui::Order::Foreground)
        .fixed_pos(area.min)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme::surface().gamma_multiply(0.96))
                .corner_radius(CornerRadius::same(theme::radius::LG))
                .inner_margin(egui::Margin::same(margin as i8))
                .shadow(theme::tile_shadow(1.0))
                .show(ui, |ui| {
                    ui.set_width(area.width() - 2.0 * margin);
                    ui.set_height(area.height() - 2.0 * margin);
                    ui.horizontal(|ui| {
                        ui.label(theme::Role::Heading.rich("Episodes").color(theme::text()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add(
                                    egui::Button::new(
                                        theme::Role::Subhead
                                            .rich(egui_phosphor::regular::X)
                                            .color(theme::muted()),
                                    )
                                    .frame(false),
                                )
                                .on_hover_text("Close (E)")
                                .clicked()
                            {
                                actions.push(Action::ToggleEpisodes);
                            }
                        });
                    });
                    if title.seasons.len() > 1 {
                        ui.add_space(theme::space::XS);
                        let labels: Vec<(usize, String)> = title
                            .seasons
                            .iter()
                            .enumerate()
                            .map(|(index, season)| (index, season.label()))
                            .collect();
                        let choices: Vec<(usize, &str)> = labels
                            .iter()
                            .map(|(index, label)| (*index, label.as_str()))
                            .collect();
                        egui::ScrollArea::horizontal()
                            .id_salt("drawer-seasons")
                            .show(ui, |ui| {
                                if let Some(picked) = ui::widgets::segmented(ui, season, &choices) {
                                    season = picked;
                                    ctx.data_mut(|data| data.insert_temp(season_id, picked));
                                }
                            });
                    }
                    ui.add_space(theme::space::S);

                    let Some(shown) = title.seasons.get(season) else {
                        return;
                    };
                    egui::ScrollArea::vertical()
                        .id_salt("drawer-episodes")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for episode in &shown.episodes {
                                let row = drawer_row(
                                    ui,
                                    &mut list.art,
                                    title,
                                    episode,
                                    is_playing(episode),
                                    actions,
                                );
                                if is_playing(episode) {
                                    let scrolled = ctx.data(|data| {
                                        data.get_temp::<String>(egui::Id::new(DRAWER_SCROLLED))
                                    });
                                    if scrolled.as_deref() != Some(target.link_id.as_str()) {
                                        row.scroll_to_me(Some(egui::Align::Center));
                                        ctx.data_mut(|data| {
                                            data.insert_temp(
                                                egui::Id::new(DRAWER_SCROLLED),
                                                target.link_id.clone(),
                                            )
                                        });
                                    }
                                }
                            }
                        });
                });
        });
}

/// One episode in the list: its still, which plays it, and what it is.
fn drawer_row(
    ui: &mut egui::Ui,
    art: &mut ui::Art<'_>,
    title: &pstr_core::library::Title,
    episode: &pstr_core::library::Episode,
    playing: bool,
    actions: &mut Vec<Action>,
) -> egui::Response {
    let frame = egui::Frame::new()
        .corner_radius(CornerRadius::same(theme::radius::MD))
        .inner_margin(egui::Margin::same(6))
        .fill(if playing {
            theme::accent_dim()
        } else {
            Color32::TRANSPARENT
        });
    let response = frame
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui::title::still(
                    ui,
                    art,
                    title,
                    episode,
                    episode.is_watched(),
                    DRAWER_STILL,
                    actions,
                );
                ui.add_space(theme::space::M);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = theme::space::XXS;
                    let name = art
                        .episode(&title.key, episode)
                        .and_then(|found| found.name.clone())
                        .unwrap_or_else(|| episode.detail().to_owned());
                    if let Some(numbering) = episode.numbering() {
                        ui.label(
                            egui::RichText::new(numbering)
                                .monospace()
                                .color(theme::muted()),
                        );
                    }
                    let mut job = egui::text::LayoutJob::simple(
                        name,
                        theme::Role::Label.font(),
                        if episode.is_watched() && !playing {
                            theme::muted()
                        } else {
                            theme::text()
                        },
                        ui.available_width(),
                    );
                    job.wrap.max_rows = 2;
                    job.wrap.overflow_character = Some('…');
                    ui.label(job);
                    let state = if playing {
                        Some("Playing".to_owned())
                    } else if episode.is_watched() {
                        Some("Watched".to_owned())
                    } else {
                        episode
                            .resume_at()
                            .map(|at| format!("Stopped at {}", ui::format_time(at)))
                    };
                    if let Some(state) = state {
                        ui.label(theme::Role::Caption.rich(state).color(if playing {
                            theme::accent()
                        } else {
                            theme::muted()
                        }));
                    }
                });
            });
        })
        .response;
    ui.add_space(theme::space::XXS);
    response
}

/// The gradient behind the title strip, fading downwards out of the top edge.
fn scrim_top(ui: &egui::Ui, rect: Rect) {
    // Squared, so the fade is dark where the text is and gone well before the
    // middle of the picture — a linear ramp over a strip this tall greys the
    // whole top third of the frame.
    fade(ui.painter(), rect, |t| {
        let fraction = 1.0 - t;
        fraction * fraction * 225.0
    });
}

/// The wash behind the controls: flat over the whole strip, fading in above it.
///
/// A gradient that starts fading only where the controls start puts the top of
/// them — the seek bar, the position and the chapter name — over what is very
/// nearly bare picture, and over a bright frame that text disappears. So the
/// strip itself is one flat wash, and the ramp happens in the empty picture
/// above it, where there is nothing to obscure.
fn scrim_bottom(ui: &egui::Ui, strip: Rect) {
    let painter = ui.painter();
    painter.rect_filled(
        strip,
        CornerRadius::ZERO,
        Color32::from_black_alpha(SCRIM_ALPHA as u8),
    );
    let above = Rect::from_min_max(
        egui::pos2(strip.left(), strip.top() - SCRIM_FADE),
        egui::pos2(strip.right(), strip.top()),
    );
    // Squared again: the wash should meet the strip at full strength and be
    // gone within a finger's width of picture above it.
    fade(painter, above, |t| t * t * SCRIM_ALPHA);
}

/// Black over `rect`, at the alpha `curve` gives for each fraction of the way
/// down it.
///
/// One mesh with a colour per row of vertices, which the GPU interpolates
/// between. It replaced a stack of flat bands, and the bands were what showed:
/// each overlapped the next by a pixel so no gap could open between them, and
/// every overlap was a line of double darkness across the picture.
fn fade(painter: &egui::Painter, rect: Rect, curve: impl Fn(f32) -> f32) {
    /// Enough rows that the curve between them reads as a curve.
    const ROWS: u32 = 16;
    let mut mesh = egui::Mesh::default();
    for row in 0..=ROWS {
        let t = row as f32 / ROWS as f32;
        let y = egui::lerp(rect.top()..=rect.bottom(), t);
        let colour = Color32::from_black_alpha(curve(t).clamp(0.0, 255.0) as u8);
        mesh.colored_vertex(egui::pos2(rect.left(), y), colour);
        mesh.colored_vertex(egui::pos2(rect.right(), y), colour);
        if row > 0 {
            let base = (row - 1) * 2;
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base + 1, base + 3, base + 2);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// What a key just did, in a pill near the top of the picture, for a moment.
///
/// Returns whether it is still showing.
pub fn osd(ctx: &egui::Context, text: &str, at: f64) -> bool {
    /// How long it stays, and how much of that is fading out.
    const SECONDS: f64 = 0.9;
    const FADE: f64 = 0.25;

    let age = ctx.input(|input| input.time) - at;
    if age >= SECONDS {
        return false;
    }
    let opacity = ((SECONDS - age) / FADE).clamp(0.0, 1.0) as f32;
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("osd"),
    ));
    let galley = painter.layout_no_wrap(text.to_owned(), theme::Role::Title.font(), Color32::WHITE);
    let size = galley.size() + Vec2::new(36.0, 18.0);
    let screen = ctx.content_rect();
    let frame = Rect::from_center_size(
        egui::pos2(screen.center().x, screen.top() + TITLE_HEIGHT + 40.0),
        size,
    );
    painter.rect_filled(
        frame,
        CornerRadius::same((size.y / 2.0) as u8),
        Color32::from_black_alpha((190.0 * opacity) as u8),
    );
    painter.galley_with_override_text_color(
        frame.min + Vec2::new(18.0, 9.0),
        galley,
        Color32::WHITE.gamma_multiply(opacity),
    );
    ctx.request_repaint();
    true
}
