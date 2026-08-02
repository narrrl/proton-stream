//! Finding OpenGL entry points for mpv's render API.
//!
//! mpv deliberately does not link to a GL library: it asks the embedder to
//! resolve every function it needs, through the `get_proc_address` callback in
//! `mpv_opengl_init_params`. The embedder is normally the one that created the
//! context, so it just forwards to `eglGetProcAddress` or `glXGetProcAddress`.
//!
//! We are not that embedder. eframe creates the context through glutin and
//! exposes it only as a [`glow::Context`] full of already-resolved pointers —
//! there is no way to ask it for one by name. So this module resolves them
//! itself, out of the GL libraries the process has *already* loaded.
//!
//! That works because of how the loaders are built rather than by luck:
//!
//! * On Linux the near-universal `libglvnd` makes `eglGetProcAddress`,
//!   `glXGetProcAddress` and a plain `dlsym` of `libGL.so.1` all return dispatch
//!   stubs that resolve against whatever context is current on the calling
//!   thread. Which of the three answers is therefore not load-bearing, which is
//!   what makes it safe not to know whether glutin picked EGL or GLX.
//! * On Windows `wglGetProcAddress` answers for everything past GL 1.1 and
//!   `opengl32.dll` exports the rest, so the two together are complete — and the
//!   order below tries them in exactly that order.
//!
//! A name that resolves nowhere comes back null, which is what mpv's callback
//! contract expects.
//!
//! **A non-null answer is not evidence the function exists.** Both
//! `glXGetProcAddressARB` and `eglGetProcAddress` are specified as free to hand
//! back a pointer for a name they have never heard of, and under `libglvnd` they
//! do exactly that: every plausible name gets a lazily generated dispatch stub,
//! including ones no driver implements. Measured on this repo's own machine,
//! *no* string resolves to null. So there is nothing to be gained by inspecting
//! the return value here, and nothing is: mpv checks what the driver actually
//! supports through `glGetString`, and refuses to create the render context with
//! its own message if something is missing.

use std::ffi::{CString, c_char, c_void};

use libloading::Library;

/// The libraries to look in, most likely first.
///
/// EGL leads on a Wayland session and GLX on an X11 one, matching what glutin
/// will have picked — though with `libglvnd` in the way, either order works.
#[cfg(unix)]
fn candidates() -> Vec<&'static str> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    if wayland {
        vec!["libEGL.so.1", "libGL.so.1"]
    } else {
        vec!["libGL.so.1", "libEGL.so.1"]
    }
}

#[cfg(windows)]
fn candidates() -> Vec<&'static str> {
    vec!["opengl32.dll"]
}

/// The `get_proc_address`-shaped entry points to try before falling back to a
/// plain symbol lookup.
///
/// Order matters on Windows: `wglGetProcAddress` returns null for the GL 1.1
/// functions that `opengl32.dll` exports directly, so the direct lookup has to
/// be the fallback rather than the other way round.
const GETTERS: [&[u8]; 4] = [
    b"eglGetProcAddress\0",
    b"glXGetProcAddressARB\0",
    b"glXGetProcAddress\0",
    b"wglGetProcAddress\0",
];

type GetProcAddress = unsafe extern "C" fn(*const c_char) -> *mut c_void;

/// Opened GL libraries, kept alive for as long as mpv might ask for a symbol.
///
/// mpv resolves most of what it needs while the render context is being
/// created, but it may resolve more later when the video chain reconfigures, so
/// this must outlive the context rather than just its construction.
pub struct GlLoader {
    libraries: Vec<Library>,
}

impl GlLoader {
    /// Open whatever GL libraries this platform has.
    ///
    /// Never fails: a library that will not open is simply not searched, and a
    /// loader with nothing in it resolves nothing — which surfaces as mpv
    /// refusing to create the render context, with mpv's own message about
    /// which function was missing. That is a better error than one from here.
    pub fn new() -> Self {
        let libraries = candidates()
            .into_iter()
            .filter_map(|name| {
                // SAFETY: these are the process's own already-loaded GL
                // libraries; `dlopen` of a loaded library bumps a refcount and
                // runs no new initialisers.
                match unsafe { Library::new(name) } {
                    Ok(library) => Some(library),
                    Err(error) => {
                        tracing::debug!("open {name}: {error}");
                        None
                    }
                }
            })
            .collect();

        Self { libraries }
    }

    /// Resolve one GL function, or null.
    pub fn resolve(&self, name: &str) -> *mut c_void {
        let Ok(symbol) = CString::new(name) else {
            // Only reachable if mpv asked for a name with an interior NUL,
            // which it does not do.
            return std::ptr::null_mut();
        };

        for library in &self.libraries {
            for getter in GETTERS {
                // SAFETY: the signature matches all four of these entry points,
                // and the symbol borrow does not outlive this block.
                let found = unsafe {
                    library
                        .get::<GetProcAddress>(getter)
                        .ok()
                        .map(|get| get(symbol.as_ptr()))
                };
                if let Some(address) = found
                    && !address.is_null()
                {
                    return address;
                }
            }
        }

        // Past the dispatchers: the plain exported symbol. This is what answers
        // for core functions on `opengl32.dll`, and it is a second chance on any
        // driver whose `GetProcAddress` is narrower than its export table.
        let with_nul = symbol.into_bytes_with_nul();
        for library in &self.libraries {
            // SAFETY: the address of a function symbol is wanted, not a call
            // through it; the cast to a data pointer is why the type here is a
            // bare `fn()` rather than the real signature.
            let found = unsafe {
                library
                    .get::<unsafe extern "C" fn()>(&with_nul)
                    .ok()
                    .map(|function| *function as *const () as *mut c_void)
            };
            if let Some(address) = found
                && !address.is_null()
            {
                return address;
            }
        }

        tracing::debug!("no GL entry point named {name}");
        std::ptr::null_mut()
    }
}

impl Default for GlLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for GlLoader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlLoader")
            .field("libraries", &self.libraries.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A loader must be constructible on a machine with no GL at all — a
    /// headless CI runner is the normal case for this crate's test job — and a
    /// name nothing has ever heard of must not panic on the way to an answer.
    ///
    /// Note what is *not* asserted: that the answer is null. `libglvnd`'s
    /// `glXGetProcAddress` manufactures a dispatch stub for any `gl`-prefixed
    /// name it is handed, whether or not the driver implements it, so a non-null
    /// pointer here is not evidence the function exists. That is fine and it is
    /// GLX's documented behaviour — mpv checks the extensions it needs through
    /// `glGetString`, not by seeing whether resolution succeeded.
    #[test]
    fn a_loader_is_always_constructible() {
        let loader = GlLoader::new();
        let _ = loader.resolve("glThisDoesNotExistAnywhere");
    }

    /// The name mpv passes is turned into a C string; a name that cannot be
    /// must come back null rather than panic inside a callback mpv is calling.
    #[test]
    fn a_name_with_an_interior_nul_resolves_to_null() {
        let loader = GlLoader::new();
        assert!(loader.resolve("glGet\0String").is_null());
    }
}
