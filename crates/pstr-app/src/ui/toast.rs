//! Short messages in the corner, for a few seconds.
//!
//! They replaced a single status line along the bottom of the window, which had
//! three problems a stack does not: a second message wiped the first before it
//! could be read, an error looked like any other line in a slightly different
//! grey, and the line took a strip of the page with it every time it appeared.

use egui::{Align2, Color32, CornerRadius, Sense, Stroke, Vec2};

use crate::theme;

/// How long a message stays up.
const SECONDS: f64 = 5.0;
/// How long an error stays up. Longer: it is more likely to be read twice.
const ERROR_SECONDS: f64 = 9.0;
/// How many fit at once. A burst beyond that drops the oldest.
const MOST: usize = 4;
/// How long one takes to arrive and to leave.
const FADE: f64 = 0.18;
const WIDTH: f32 = 340.0;

pub struct Toast {
    id: u64,
    text: String,
    error: bool,
    at: f64,
}

/// The messages on screen.
#[derive(Default)]
pub struct Toasts {
    items: Vec<Toast>,
    next: u64,
}

impl Toasts {
    pub fn push(&mut self, now: f64, text: impl Into<String>, error: bool) {
        let text = text.into();
        // The same message again — a retry that failed the same way — restarts
        // the one already up rather than stacking a copy of it.
        if let Some(existing) = self.items.iter_mut().find(|toast| toast.text == text) {
            existing.at = now;
            existing.error = error;
            return;
        }
        self.next += 1;
        self.items.push(Toast {
            id: self.next,
            text,
            error,
            at: now,
        });
        if self.items.len() > MOST {
            self.items.remove(0);
        }
    }

    /// Draw the stack above `bottom` points from the window's bottom edge, and
    /// drop whatever has expired or been closed.
    pub fn show(&mut self, ctx: &egui::Context, bottom: f32) {
        let now = ctx.input(|input| input.time);
        let pointer = ctx.input(|input| input.pointer.hover_pos());
        let mut closed = None;
        let mut hovered = false;
        let mut y = ctx.content_rect().bottom() - bottom - 16.0;
        let right = ctx.content_rect().right() - 16.0;

        // Newest at the bottom, nearest where the eye already is.
        for toast in self.items.iter().rev() {
            let life = lifetime(toast.error);
            let age = now - toast.at;
            let arriving = (age / FADE).clamp(0.0, 1.0) as f32;
            let leaving = ((life - age) / FADE).clamp(0.0, 1.0) as f32;
            let opacity = ease(arriving.min(leaving));

            let area = egui::Area::new(egui::Id::new(("toast", toast.id)))
                .order(egui::Order::Foreground)
                .pivot(Align2::RIGHT_BOTTOM)
                .fixed_pos(egui::pos2(right + (1.0 - opacity) * 24.0, y))
                .interactable(true)
                .show(ctx, |ui| {
                    ui.set_opacity(opacity);
                    card(ui, toast)
                });
            if area.inner {
                closed = Some(toast.id);
            }
            if pointer.is_some_and(|at| area.response.rect.contains(at)) {
                hovered = true;
            }
            y -= area.response.rect.height() + 8.0;
        }

        if let Some(id) = closed {
            self.items.retain(|toast| toast.id != id);
        }
        // Under the pointer, nothing leaves: someone is reading it.
        if hovered {
            for toast in &mut self.items {
                let life = lifetime(toast.error);
                toast.at = toast.at.max(now - life + 1.5);
            }
        }
        self.items
            .retain(|toast| now - toast.at < lifetime(toast.error));
        if !self.items.is_empty() {
            // Enough frames to animate the edges and to notice an expiry.
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}

fn lifetime(error: bool) -> f64 {
    if error { ERROR_SECONDS } else { SECONDS }
}

fn ease(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// One message. Returns whether its close button was pressed.
fn card(ui: &mut egui::Ui, toast: &Toast) -> bool {
    let mut close = false;
    let edge = if toast.error {
        theme::danger()
    } else {
        theme::accent()
    };
    egui::Frame::new()
        .fill(theme::surface())
        .stroke(Stroke::new(1.0, theme::card_hover()))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin {
            left: 16,
            right: 10,
            top: 10,
            bottom: 10,
        })
        .shadow(egui::Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: Color32::from_black_alpha(90),
        })
        .show(ui, |ui| {
            ui.set_width(WIDTH);
            let frame = ui.max_rect();
            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(theme::Role::Label.rich(&toast.text).color(theme::text()))
                        .wrap(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    let (rect, response) =
                        ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
                    let ink = if response.hovered() {
                        theme::text()
                    } else {
                        theme::muted()
                    };
                    let d = 4.0;
                    let c = rect.center();
                    ui.painter().line_segment(
                        [c + Vec2::new(-d, -d), c + Vec2::new(d, d)],
                        Stroke::new(1.5, ink),
                    );
                    ui.painter().line_segment(
                        [c + Vec2::new(-d, d), c + Vec2::new(d, -d)],
                        Stroke::new(1.5, ink),
                    );
                    close = response.on_hover_text("Dismiss").clicked();
                });
            });
            // A strip down the leading edge says which kind it is before a word
            // of it is read.
            let strip = egui::Rect::from_min_max(
                egui::pos2(frame.left() - 10.0, frame.top() - 4.0),
                egui::pos2(frame.left() - 7.0, frame.bottom() + 4.0),
            );
            ui.painter().rect_filled(strip, CornerRadius::same(2), edge);
        });
    close
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repeated_message_restarts_rather_than_stacks() {
        let mut toasts = Toasts::default();
        toasts.push(0.0, "could not open", true);
        toasts.push(3.0, "could not open", true);
        assert_eq!(toasts.items.len(), 1);
        assert_eq!(toasts.items[0].at, 3.0);
    }

    #[test]
    fn a_burst_keeps_only_the_newest() {
        let mut toasts = Toasts::default();
        for index in 0..10 {
            toasts.push(0.0, format!("message {index}"), false);
        }
        assert_eq!(toasts.items.len(), MOST);
        assert_eq!(toasts.items.last().unwrap().text, "message 9");
    }
}
