//! How often the window is allowed to draw — and why it does not simply let
//! vsync decide on Wayland.
//!
//! ## The bug this exists for
//!
//! A Wayland compositor throttles a client by *withholding* the `wl_surface`
//! frame callback, and it withholds it for as long as the surface is not being
//! shown — a window on another workspace, or behind a fullscreen one. Mesa's
//! EGL implements a swap interval of 1 by waiting on exactly that callback, so
//! `eglSwapBuffers` on a hidden surface does not return late, it does not
//! return at all:
//!
//! ```text
//!   eframe run_ui_and_paint ─▶ glutin swap_buffers ─▶ eglSwapBuffers
//!                                                          │
//!                              wl_display_dispatch_queue ◀─┘   ppoll, forever
//! ```
//!
//! That is the whole of the UI thread, and the same thread is the one that
//! answers `xdg_wm_base.ping`. Hyprland pings a client it has not heard from
//! and puts up an "application is not responding" dialog when the pong does not
//! come, which is what a viewer sees a few seconds after switching away from a
//! playing window. Nothing is actually wrong — coming back releases the frame
//! callback and the app carries on — but a dialog offering to kill the player
//! is not an acceptable way to say "this window is not visible".
//!
//! winit cannot rescue this for us: `WindowEvent::Occluded` is X11, macOS and
//! web only — its Wayland implementation was deliberately removed in 0.29 — so
//! eframe never learns the window is hidden and keeps painting it.
//!
//! ## What is done instead
//!
//! On Wayland the swap interval is set to 0 ([`vsync`]), which makes
//! `eglSwapBuffers` return immediately whether the surface is being shown or
//! not, and the frame rate is capped here instead ([`Pacer`]). Nothing is lost
//! by dropping vsync there: a Wayland compositor composites from the last
//! buffer a client committed, at its own refresh, so a client that commits too
//! often wastes work but cannot tear. X11 and Windows keep vsync, where a
//! client really does drive the scanout and hidden windows do not block.
//!
//! The cap is a ceiling and not a rate: frames are drawn when something asks
//! for one — a new mpv frame, an animation, a keystroke — so an idle window
//! still draws nothing, and a 24 fps film still draws 24 times a second.

use std::time::{Duration, Instant};

/// The default ceiling, in frames a second.
///
/// Above the rate of any video this plays and of every animation in the app.
/// It is deliberately not the display's refresh rate, which neither eframe nor
/// egui gives us — a window on a 144 Hz panel therefore animates at 60 while
/// vsync is off. [`CAP_VAR`] is the way out for someone who minds.
const DEFAULT_CAP: f32 = 60.0;

/// Overrides [`DEFAULT_CAP`]. `0` removes the ceiling altogether.
const CAP_VAR: &str = "PSTR_FRAME_CAP";

/// Whether this process is talking to a Wayland compositor, decided the way
/// winit decides it: either variable, non-empty, Wayland first.
pub fn on_wayland() -> bool {
    ["WAYLAND_DISPLAY", "WAYLAND_SOCKET"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

/// Whether to ask for a swap interval of 1. False on Wayland — see the module
/// documentation.
pub fn vsync() -> bool {
    !on_wayland()
}

/// The ceiling to pace to, or `None` when vsync is doing the pacing.
fn cap() -> Option<f32> {
    if vsync() {
        return None;
    }
    let Ok(value) = std::env::var(CAP_VAR) else {
        return Some(DEFAULT_CAP);
    };
    // Not a number, or a negative one, is a typo rather than a request to
    // free-run — free-running is what `0` says, explicitly.
    let parsed = value
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|fps| fps.is_finite() && *fps >= 0.0);
    match parsed {
        Some(fps) if fps > 0.0 => Some(fps),
        Some(_) => None,
        None => {
            tracing::warn!("{CAP_VAR}={value:?} is not a frame rate; using {DEFAULT_CAP}");
            Some(DEFAULT_CAP)
        }
    }
}

/// Holds the draw loop to a maximum rate.
///
/// Sleeping is the only thing that can do this: a frame is under way by the
/// time the app sees it, and every other source of repaints — mpv, egui's own
/// animations — asks for the *next* one immediately. The sleep is at most one
/// frame's worth, which is far inside the several seconds a compositor waits
/// for a pong.
pub struct Pacer {
    interval: Option<Duration>,
    last: Instant,
}

impl Default for Pacer {
    fn default() -> Self {
        Self::new()
    }
}

impl Pacer {
    /// The pacer this session wants: capped when vsync is off, a no-op when it
    /// is on.
    pub fn new() -> Self {
        Self::with_cap(cap())
    }

    fn with_cap(fps: Option<f32>) -> Self {
        Self {
            interval: fps.map(|fps| Duration::from_secs_f32(1.0 / fps)),
            last: Instant::now(),
        }
    }

    /// Whether this pacer does anything at all.
    pub fn is_capped(&self) -> bool {
        self.interval.is_some()
    }

    /// Called once at the top of a frame: sleeps out whatever is left of the
    /// last one's budget.
    pub fn wait(&mut self) {
        let Some(interval) = self.interval else {
            return;
        };
        let elapsed = self.last.elapsed();
        if let Some(left) = interval.checked_sub(elapsed) {
            std::thread::sleep(left);
        }
        self.last = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_uncapped_pacer_never_sleeps() {
        let mut pacer = Pacer::with_cap(None);
        assert!(!pacer.is_capped());
        let before = Instant::now();
        pacer.wait();
        pacer.wait();
        assert!(before.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn a_capped_pacer_holds_the_second_frame_back() {
        let mut pacer = Pacer::with_cap(Some(50.0));
        assert!(pacer.is_capped());
        pacer.wait();
        let before = Instant::now();
        pacer.wait();
        // 20 ms apart, less whatever the first call spent. Sleeping is only
        // ever long, so the lower bound is the one worth asserting on.
        assert!(
            before.elapsed() >= Duration::from_millis(10),
            "second frame was not held back: {:?}",
            before.elapsed()
        );
    }

    #[test]
    fn a_frame_that_took_longer_than_the_budget_is_not_delayed_further() {
        let mut pacer = Pacer::with_cap(Some(1000.0));
        std::thread::sleep(Duration::from_millis(5));
        let before = Instant::now();
        pacer.wait();
        assert!(before.elapsed() < Duration::from_millis(2));
    }
}
