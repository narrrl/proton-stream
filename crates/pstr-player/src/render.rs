//! Handing mpv an OpenGL context to draw into, instead of a window of its own.
//!
//! Step two. Everything about *getting the bytes* — the `pstr://` protocol, the
//! cursor, the block layer under it — is unchanged; this is only about where the
//! decoded picture lands. With a render context alive and
//! [`crate::VideoOutput::Embedded`] set, mpv's `libmpv` video output hands each
//! frame to [`VideoRenderer::render`] instead of putting it on screen.
//!
//! ```text
//!   ui thread (GL current)              player thread
//!   ──────────────────────              ─────────────
//!   renderer.update()  ─▶ new frame?    poll_event / seek / pause
//!   renderer.render(fbo, w, h)                  │
//!         │                                     ▼
//!         └──────────────▶ mpv core ◀───────────┘
//!                            ▲
//!                            └── demuxer thread ── pstr:// ── blocks
//! ```
//!
//! Three constraints run through the whole file, all of them mpv's:
//!
//! * **The GL context must be current on the calling thread** for
//!   [`VideoRenderer::new`], [`VideoRenderer::render`] *and* the destructor.
//!   That is why this type is not `Send`: it belongs to the UI thread, while the
//!   [`Player`] it renders is shared with the thread polling events.
//! * **The render context must outlive nothing and predecease the core.** It is
//!   freed in `Drop`, and the `Arc<Player>` held here is what guarantees the mpv
//!   handle is still alive when that happens.
//! * **`render` must not block.** Left to itself mpv waits inside it until the
//!   frame's display time, which would peg the UI thread at the video's frame
//!   rate. [`crate::VideoOutput::Embedded`] sets `video-timing-offset` to 0 to
//!   turn that off, and the update callback is what drives repaints instead.
//!
//! `MPV_RENDER_PARAM_ADVANCED_CONTROL` is deliberately *not* set. It buys
//! finer-grained scheduling in exchange for a contract about calling
//! `mpv_render_context_update` from the right thread at the right time, and the
//! thing it optimises — frame pacing on a local file — is not what limits
//! playback over a public link.

use std::ffi::{c_char, c_void};
use std::ptr::NonNull;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::gl::GlLoader;
use crate::player::Player;

/// mpv's callback when a new frame is ready, or the video needs redrawing.
///
/// Called from mpv's own threads, so it has to be `Send + Sync`. The one thing
/// it may do is wake the UI — mpv forbids calling back into its API from here.
type UpdateHook = Box<dyn Fn() + Send + Sync>;

/// mpv rendering into a framebuffer the caller owns.
pub struct VideoRenderer {
    ctx: NonNull<libmpv2_sys::mpv_render_context>,
    /// Keeps the mpv core alive: freeing the render context after the core is
    /// gone is undefined, and this makes that unrepresentable.
    _player: Arc<Player>,
    /// Kept alive because mpv may resolve further GL entry points long after
    /// construction, when the video chain reconfigures.
    _loader: Box<GlLoader>,
    /// Freed after the render context, so mpv cannot call a dropped closure.
    hook: Option<*mut UpdateHook>,
}

impl VideoRenderer {
    /// Create the render context on the current OpenGL context.
    ///
    /// # Safety
    ///
    /// An OpenGL context must be current on the calling thread, and must stay
    /// current for every later [`Self::render`] call and for the drop. The
    /// caller must also drop this on that same thread.
    pub unsafe fn new(player: Arc<Player>) -> Result<Self> {
        let loader = Box::new(GlLoader::new());

        let mut init = libmpv2_sys::mpv_opengl_init_params {
            get_proc_address: Some(get_proc_address),
            // Borrowed by mpv for the life of the context; `loader` is boxed and
            // moved into the returned value, so the address stays valid.
            get_proc_address_ctx: (&raw const *loader) as *mut c_void,
        };

        let mut params = [
            // `MPV_RENDER_PARAM_API_TYPE` is typed `char*`, so the data field is
            // the string itself rather than a pointer to it — unlike every other
            // parameter here.
            libmpv2_sys::mpv_render_param {
                type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
                data: libmpv2_sys::MPV_RENDER_API_TYPE_OPENGL.as_ptr() as *mut c_void,
            },
            libmpv2_sys::mpv_render_param {
                type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
                data: (&raw mut init) as *mut c_void,
            },
            // The array is NUL-terminated by a zero type, exactly like a C
            // string; without it mpv walks off the end.
            libmpv2_sys::mpv_render_param {
                type_: 0,
                data: std::ptr::null_mut(),
            },
        ];

        let mut ctx: *mut libmpv2_sys::mpv_render_context = std::ptr::null_mut();
        // SAFETY: the handle is live for as long as `player` is held, the
        // parameter array is well-formed and only read during the call, and the
        // caller has promised a current GL context.
        let status = unsafe {
            libmpv2_sys::mpv_render_context_create(
                &raw mut ctx,
                player.raw_handle().as_ptr(),
                params.as_mut_ptr(),
            )
        };
        if status < 0 {
            return Err(Error::Mpv(libmpv2::Error::Raw(status)));
        }

        let ctx = NonNull::new(ctx).ok_or(Error::RenderContext)?;
        Ok(Self {
            ctx,
            _player: player,
            _loader: loader,
            hook: None,
        })
    }

    /// Ask to be told when there is a new frame to draw.
    ///
    /// The callback runs on an mpv thread and must do nothing but wake the UI —
    /// no mpv API calls, no locks the UI thread might already hold. Calling this
    /// raises the callback once immediately, which is what gets the first frame
    /// on screen.
    pub fn on_update(&mut self, hook: impl Fn() + Send + Sync + 'static) {
        let hook: *mut UpdateHook = Box::into_raw(Box::new(Box::new(hook)));

        // SAFETY: `ctx` is live, and `hook` stays valid until `Drop` frees it —
        // which it does only after `mpv_render_context_free`, so mpv cannot call
        // a dangling closure.
        unsafe {
            libmpv2_sys::mpv_render_context_set_update_callback(
                self.ctx.as_ptr(),
                Some(on_update),
                hook as *mut c_void,
            );
        }

        // Replacing a previous hook: mpv has stopped calling the old one, so it
        // is safe to reclaim now.
        if let Some(previous) = self.hook.replace(hook) {
            // SAFETY: created by this function's earlier call and no longer
            // reachable from mpv.
            drop(unsafe { Box::from_raw(previous) });
        }
    }

    /// Whether mpv has a new frame waiting.
    ///
    /// Cheap, and safe from any thread. A `false` means the last rendered
    /// picture is still current — the caller should reuse its texture rather
    /// than re-render, which is the whole point of asking.
    pub fn has_new_frame(&self) -> bool {
        // SAFETY: `ctx` is live, and this call is documented as thread-safe.
        let flags = unsafe { libmpv2_sys::mpv_render_context_update(self.ctx.as_ptr()) };
        flags & u64::from(libmpv2_sys::mpv_render_update_flag_MPV_RENDER_UPDATE_FRAME) != 0
    }

    /// Draw the current frame into `fbo`, which must be `width` × `height`.
    ///
    /// mpv scales and letterboxes the picture to fill that rectangle itself, so
    /// the caller does not have to know the video's aspect ratio — the bars are
    /// painted in the same pass.
    ///
    /// `flip` is mpv's `MPV_RENDER_PARAM_FLIP_Y`, for the case where the target
    /// is scanned out bottom-up — the OpenGL default framebuffer. An FBO whose
    /// colour texture is then *sampled* is not that case: unflipped, mpv puts
    /// the top of the picture in the first row, which is where egui's V axis
    /// starts. Passing `true` there stands the picture on its head.
    ///
    /// # Safety
    ///
    /// The same OpenGL context that was current for [`Self::new`] must be
    /// current on the calling thread, and `fbo` must name a complete
    /// framebuffer in it.
    pub unsafe fn render(&self, fbo: i32, width: i32, height: i32, flip: bool) -> Result<()> {
        let mut target = libmpv2_sys::mpv_opengl_fbo {
            fbo,
            w: width,
            h: height,
            internal_format: 0,
        };
        let mut flip_y: std::ffi::c_int = i32::from(flip);

        let mut params = [
            libmpv2_sys::mpv_render_param {
                type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_FBO,
                data: (&raw mut target) as *mut c_void,
            },
            libmpv2_sys::mpv_render_param {
                type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_FLIP_Y,
                data: (&raw mut flip_y) as *mut c_void,
            },
            libmpv2_sys::mpv_render_param {
                type_: 0,
                data: std::ptr::null_mut(),
            },
        ];

        // SAFETY: `ctx` is live, the parameters are only read during the call,
        // and the caller has promised the right context is current.
        let status = unsafe {
            libmpv2_sys::mpv_render_context_render(self.ctx.as_ptr(), params.as_mut_ptr())
        };
        if status < 0 {
            return Err(Error::Mpv(libmpv2::Error::Raw(status)));
        }
        Ok(())
    }
}

impl Drop for VideoRenderer {
    /// Free the render context.
    ///
    /// Must run on the thread whose OpenGL context this was created against,
    /// with that context still current — mpv frees GL objects here. The app
    /// drops this from its draw function or from `on_exit`, both of which
    /// satisfy that; dropping it anywhere else is a bug this type cannot catch.
    fn drop(&mut self) {
        // SAFETY: `ctx` was created here and is freed exactly once. mpv
        // guarantees no further update callbacks after this returns, which is
        // what makes reclaiming the hook below sound.
        unsafe {
            libmpv2_sys::mpv_render_context_free(self.ctx.as_ptr());
        }
        if let Some(hook) = self.hook.take() {
            // SAFETY: allocated in `on_update`, and now unreachable from mpv.
            drop(unsafe { Box::from_raw(hook) });
        }
    }
}

/// mpv asking for one GL entry point by name.
unsafe extern "C" fn get_proc_address(ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    if ctx.is_null() || name.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: `ctx` is the `GlLoader` this renderer boxed and keeps alive, and
    // `name` is a NUL-terminated string owned by mpv for the length of the call.
    let loader = unsafe { &*(ctx as *const GlLoader) };
    let Ok(name) = (unsafe { std::ffi::CStr::from_ptr(name) }).to_str() else {
        return std::ptr::null_mut();
    };
    loader.resolve(name)
}

/// mpv reporting that there is something new to draw.
///
/// Deliberately does nothing but call the hook: this runs on an mpv thread, and
/// mpv forbids re-entering its API from here.
unsafe extern "C" fn on_update(ctx: *mut c_void) {
    if ctx.is_null() {
        return;
    }
    // SAFETY: `ctx` is the boxed hook, alive until the render context is freed.
    let hook = unsafe { &*(ctx as *const UpdateHook) };
    hook();
}
