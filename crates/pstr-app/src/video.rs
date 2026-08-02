//! The picture, inside this window.
//!
//! mpv draws into a framebuffer we own; the colour texture behind it is handed
//! to egui and painted like any other image. That indirection — rather than
//! letting mpv draw straight into the window's back buffer — is what lets the
//! video sit in a *rectangle*: `mpv_render_context_render` always fills the
//! framebuffer it is given from its origin, with no way to offset it, so a
//! direct draw could only ever be full-window.
//!
//! ```text
//!   App::ui (GL current)
//!        │  size in physical pixels
//!        ▼
//!   VideoSurface::texture ──▶ mpv render ──▶ FBO ──▶ colour texture
//!                                                        │
//!                                    egui::TextureId ◀───┘   painter.image(…)
//! ```
//!
//! Three things here are less obvious than they look:
//!
//! * **The texture is registered once and resized in place.**
//!   [`eframe::Frame::register_native_glow_texture`] takes ownership of the
//!   texture and there is no matching unregister, so registering a fresh one per
//!   resize would leak one texture per drag of the window edge. Reallocating the
//!   *same* GL name with `tex_image_2d` keeps the `TextureId` valid and leaks
//!   nothing.
//! * **A frame is rendered only when mpv says there is one.** Otherwise the
//!   texture from last frame is still correct, and egui repaints far more often
//!   than a 24 fps film changes.
//! * **mpv's GL state churn is safe here by contract, not by luck.** mpv
//!   restores everything except the viewport, the scissor box, the blend
//!   function and the clear colour — all four of which `egui_glow` sets itself
//!   at the start of every paint.

use std::sync::Arc;

use eframe::glow::{self, HasContext as _};
use pstr_player::{Player, VideoRenderer};

/// Smallest framebuffer worth allocating. A window dragged to nothing still has
/// to leave mpv something valid to render into.
const MIN_EDGE: i32 = 16;

/// Largest, in either direction. A 4K display at 200% scaling asks for 7680 px
/// of framebuffer for a rectangle nothing can resolve; this is the ceiling that
/// keeps a stray `pixels_per_point` from asking the driver for half a gigabyte.
const MAX_EDGE: i32 = 4096;

/// mpv's render context plus the framebuffer it draws into.
///
/// Not `Send`: every method here needs the OpenGL context that eframe makes
/// current around [`eframe::App::ui`], so this lives on the UI thread while the
/// [`Player`] it renders is shared with the thread polling mpv's events.
pub struct VideoSurface {
    gl: Arc<glow::Context>,
    renderer: VideoRenderer,
    /// Owned by eframe once registered — deliberately never deleted here.
    texture: Option<glow::Texture>,
    id: Option<egui::TextureId>,
    fbo: Option<glow::Framebuffer>,
    size: [i32; 2],
    /// Set until the first frame actually lands, so the caller can say
    /// "buffering" rather than show a black rectangle it cannot explain.
    painted: bool,
}

impl VideoSurface {
    /// Attach a renderer to `player` on the current OpenGL context.
    ///
    /// `ctx` is woken whenever mpv has a new frame; without that the picture
    /// would only advance when the viewer moved the mouse.
    ///
    /// # Safety
    ///
    /// An OpenGL context must be current on the calling thread — inside
    /// [`eframe::App::ui`] it is — and this value must be dropped on that same
    /// thread with the context still current.
    pub unsafe fn new(
        player: Arc<Player>,
        gl: Arc<glow::Context>,
        ctx: egui::Context,
    ) -> pstr_player::Result<Self> {
        // SAFETY: forwarded from this function's own contract.
        let mut renderer = unsafe { VideoRenderer::new(player)? };
        renderer.on_update(move || ctx.request_repaint());

        Ok(Self {
            gl,
            renderer,
            texture: None,
            id: None,
            fbo: None,
            size: [0, 0],
            painted: false,
        })
    }

    /// Whether a frame has ever reached the screen.
    pub fn has_picture(&self) -> bool {
        self.painted
    }

    /// Render at `size` physical pixels if there is anything new, and give back
    /// the texture to paint.
    ///
    /// `None` means there is nothing to draw yet — the framebuffer could not be
    /// built, or the file has no video track.
    pub fn texture(
        &mut self,
        frame: &mut eframe::Frame,
        size: [i32; 2],
    ) -> Option<egui::TextureId> {
        let size = [
            size[0].clamp(MIN_EDGE, MAX_EDGE),
            size[1].clamp(MIN_EDGE, MAX_EDGE),
        ];

        // A resize invalidates the picture as well as the storage: the old
        // contents are gone, so redraw even if mpv has no new frame.
        let resized = self.ensure(frame, size)?;
        if !resized && !self.renderer.has_new_frame() {
            return self.id;
        }

        let fbo = self.fbo?;
        // SAFETY: the context is current for the whole of `App::ui`, and `fbo`
        // was checked complete when it was built.
        unsafe {
            let previous = self.gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING);
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));

            // No flip: mpv writes the top of the picture into the first row of
            // the texture, which is where egui's V axis starts. `FLIP_Y` is for
            // drawing into the *default* framebuffer, and here it would stand
            // the picture on its head. See `VideoRenderer::render`.
            let rendered = self
                .renderer
                .render(fbo.0.get() as i32, size[0], size[1], false);

            self.gl
                .bind_framebuffer(glow::FRAMEBUFFER, restore(previous));

            if let Err(error) = rendered {
                tracing::warn!("render a video frame: {error}");
                return self.id;
            }
        }

        self.painted = true;
        self.id
    }

    /// Make sure there is a framebuffer of `size`, reporting whether it changed.
    ///
    /// `None` if one could not be built at all, which is the only failure that
    /// makes the surface useless rather than merely late.
    fn ensure(&mut self, frame: &mut eframe::Frame, size: [i32; 2]) -> Option<bool> {
        if self.texture.is_none() {
            // SAFETY: the context is current, and every object created here is
            // either handed to eframe (the texture) or deleted in `Drop` (the
            // framebuffer).
            unsafe {
                let texture = self
                    .gl
                    .create_texture()
                    .inspect_err(|error| tracing::error!("create the video texture: {error}"))
                    .ok()?;
                self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                // Linear, and clamped: the video is scaled to whatever rectangle
                // the window leaves it, and a repeating wrap would fringe the
                // letterbox edges with the opposite side of the picture.
                for (parameter, value) in [
                    (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                    (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                    (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                    (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
                ] {
                    self.gl
                        .tex_parameter_i32(glow::TEXTURE_2D, parameter, value as i32);
                }
                self.gl.bind_texture(glow::TEXTURE_2D, None);

                let fbo = self
                    .gl
                    .create_framebuffer()
                    .inspect_err(|error| tracing::error!("create the video framebuffer: {error}"))
                    .ok()?;
                self.texture = Some(texture);
                self.fbo = Some(fbo);
            }

            // Once: the `TextureId` outlives every resize, because the resize
            // reallocates this same GL name rather than making a new one.
            self.id = Some(frame.register_native_glow_texture(self.texture?));
        }

        if self.size == size {
            return Some(false);
        }

        let (texture, fbo) = (self.texture?, self.fbo?);
        // SAFETY: both objects were created above on this context, which is
        // still current.
        unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            self.gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                size[0],
                size[1],
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            self.gl.bind_texture(glow::TEXTURE_2D, None);

            let previous = self.gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING);
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            self.gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            let status = self.gl.check_framebuffer_status(glow::FRAMEBUFFER);
            self.gl
                .bind_framebuffer(glow::FRAMEBUFFER, restore(previous));

            if status != glow::FRAMEBUFFER_COMPLETE {
                tracing::error!("video framebuffer is incomplete: 0x{status:x}");
                return None;
            }
        }

        self.size = size;
        Some(true)
    }
}

impl Drop for VideoSurface {
    /// Runs on the UI thread with the context current — see the note on
    /// [`VideoRenderer`]'s destructor, which has the same requirement and is
    /// the reason this type is dropped from the draw loop rather than wherever
    /// playback happens to end.
    fn drop(&mut self) {
        if let Some(fbo) = self.fbo.take() {
            // SAFETY: created on this context, deleted once.
            unsafe { self.gl.delete_framebuffer(fbo) };
        }
        // The texture is deliberately not deleted: `register_native_glow_texture`
        // took ownership of it, and eframe frees it with the painter.
    }
}

/// The framebuffer name a `GL_DRAW_FRAMEBUFFER_BINDING` query returned, as
/// something `bind_framebuffer` accepts. Zero is the default framebuffer, which
/// glow spells `None`.
fn restore(binding: i32) -> Option<glow::Framebuffer> {
    std::num::NonZeroU32::new(binding as u32).map(glow::NativeFramebuffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_binding_restores_the_default_framebuffer() {
        assert!(restore(0).is_none());
        assert!(restore(7).is_some());
    }
}
