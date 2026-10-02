//! The window's size, kept between launches.
//!
//! Size and maximized only. Position is left to the window manager: Wayland
//! does not let a client choose it, and on X11 a remembered position on a
//! monitor that has since been unplugged opens the window off screen.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What the next launch should open at.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

/// The smallest size worth restoring. Anything below is a window that was
/// being dragged about when it closed, or a file someone edited by hand.
const SMALLEST: [f32; 2] = [720.0, 480.0];

impl WindowState {
    /// The saved state, if there is a sensible one. A missing or broken file
    /// means the default size, never an error: this is a convenience.
    pub fn load(path: &Path) -> Option<Self> {
        let state: Self = pstr_core::config::read_json(path).ok().flatten()?;
        state.sensible().then_some(state)
    }

    fn sensible(&self) -> bool {
        self.width.is_finite()
            && self.height.is_finite()
            && self.width >= SMALLEST[0]
            && self.height >= SMALLEST[1]
    }

    /// Fold this frame's viewport into `last`.
    ///
    /// A fullscreen window says nothing about how big the window should be, and
    /// a maximized one says nothing about how big it should be once it is not,
    /// so neither changes the size kept.
    pub fn observe(ctx: &egui::Context, last: Option<Self>) -> Option<Self> {
        let (size, maximized, fullscreen) = ctx.input(|input| {
            let viewport = input.viewport();
            (
                viewport.inner_rect.map(|rect| rect.size()),
                viewport.maximized.unwrap_or(false),
                viewport.fullscreen.unwrap_or(false),
            )
        });
        if fullscreen {
            return last;
        }
        let (width, height) = match (maximized, last, size) {
            (true, Some(last), _) => (last.width, last.height),
            (_, _, Some(size)) => (size.x, size.y),
            _ => return last,
        };
        let state = Self {
            width,
            height,
            maximized,
        };
        state.sensible().then_some(state).or(last)
    }
}

/// Writes the state when it changes, once, at exit.
pub struct WindowMemory {
    path: PathBuf,
    saved: Option<WindowState>,
    pub current: Option<WindowState>,
}

impl WindowMemory {
    pub fn new(path: PathBuf) -> Self {
        let saved = WindowState::load(&path);
        Self {
            path,
            saved,
            current: saved,
        }
    }

    pub fn observe(&mut self, ctx: &egui::Context) {
        self.current = WindowState::observe(ctx, self.current);
    }

    pub fn save(&self) {
        let Some(current) = self.current.filter(|current| Some(*current) != self.saved) else {
            return;
        };
        if let Err(error) = pstr_core::config::write_json(&self.path, &current) {
            tracing::debug!("save the window size: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_smaller_than_the_minimum_is_not_restored() {
        let path = std::env::temp_dir().join(format!("pstr-window-{}.json", std::process::id()));
        let tiny = WindowState {
            width: 200.0,
            height: 100.0,
            maximized: false,
        };
        pstr_core::config::write_json(&path, &tiny).unwrap();
        assert_eq!(WindowState::load(&path), None);

        let normal = WindowState {
            width: 1400.0,
            height: 900.0,
            maximized: true,
        };
        pstr_core::config::write_json(&path, &normal).unwrap();
        assert_eq!(WindowState::load(&path), Some(normal));
        let _ = std::fs::remove_file(path);
    }
}
