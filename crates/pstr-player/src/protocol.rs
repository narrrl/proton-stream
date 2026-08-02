//! Registering `pstr://` with mpv.
//!
//! This is the only unsafe code in the crate, and it is deliberately thin: the
//! callbacks translate pointers and hand straight over to [`StreamCursor`],
//! which is a safe type with tests. Nothing here decides anything.
//!
//! Feeding mpv through `stream_cb` rather than a loopback HTTP server is a
//! security choice as much as a convenience one — there is no port to open, no
//! way for another local process to ask for the plaintext, and decrypted bytes
//! never leave the process.

use std::cell::UnsafeCell;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::{Arc, Weak};

use libmpv2::Mpv;

use crate::cursor::{CancelLatch, StreamCursor};
use crate::error::{Error, Result};
use crate::registry::{PROTOCOL, StreamRegistry, parse_url};

const LOADING_FAILED: c_int = libmpv2::mpv_error::LoadingFailed;

/// Per-open-stream state, owned by mpv through a raw pointer.
///
/// The split matters. mpv serialises `read`/`seek`/`size`/`close` on one demuxer
/// thread, but calls `cancel` from another *while a read is blocked* — that is
/// the entire point of the callback. So the cursor lives behind an `UnsafeCell`
/// touched only by the serialised callbacks, the latch is a `Sync` type touched
/// by any of them, and no `&mut Cookie` is ever created. Taking `&mut Cookie` in
/// `read` and `&Cookie` in `cancel` would alias.
struct Cookie {
    /// Read by whichever thread cancels; never exclusively borrowed.
    cancel: CancelLatch,
    /// Exclusively used, but only from mpv's demuxer thread.
    cursor: UnsafeCell<StreamCursor>,
}

// SAFETY: see the note above — the `UnsafeCell` is only ever reached from the
// callbacks mpv serialises, and everything inside the cursor is `Send`.
unsafe impl Sync for Cookie {}

/// Register the `pstr://` protocol on an mpv handle.
///
/// The registration lasts until the mpv core is destroyed and cannot be undone,
/// which is what makes the `user_data` lifetime awkward: mpv may still call into
/// it while it is tearing itself down. Rather than reason about whether
/// `mpv_destroy` has joined every thread that could reach us, this leaks a
/// `Box<Weak<StreamRegistry>>` — a pointer and an `Arc` control block, tens of
/// bytes per player, permanently valid. The `Weak` is what keeps it honest: once
/// the player is dropped the upgrade fails and every `pstr://` URL cleanly stops
/// resolving, so the leak buys soundness without keeping any stream alive.
pub(crate) fn register(mpv: &Mpv, registry: &Arc<StreamRegistry>) -> Result<()> {
    let name = CString::new(PROTOCOL).map_err(|_| Error::NulByte("the protocol name"))?;
    let user_data = Box::into_raw(Box::new(Arc::downgrade(registry))).cast::<c_void>();

    // SAFETY: `mpv` is a live handle, `name` outlives the call (mpv copies it),
    // and `user_data` is valid forever by construction.
    let status = unsafe {
        libmpv2_sys::mpv_stream_cb_add_ro(mpv.ctx.as_ptr(), name.as_ptr(), user_data, Some(open))
    };

    if status < 0 {
        return Err(Error::Mpv(libmpv2::Error::Raw(status)));
    }
    Ok(())
}

/// Resolve a `pstr://` URL to a cursor. Never blocks: the stream was opened,
/// and any failure reported, before this URL existed. See [`crate::registry`].
unsafe extern "C" fn open(
    user_data: *mut c_void,
    uri: *mut c_char,
    info: *mut libmpv2_sys::mpv_stream_cb_info,
) -> c_int {
    // SAFETY: the leaked `Weak` from `register`, valid for the process lifetime.
    let registry = unsafe { &*user_data.cast::<Weak<StreamRegistry>>() };
    let Some(registry) = registry.upgrade() else {
        tracing::debug!("pstr:// opened after its player went away");
        return LOADING_FAILED;
    };

    // SAFETY: mpv passes a NUL-terminated string valid for this call.
    let uri = unsafe { CStr::from_ptr(uri) };
    let Some(id) = uri.to_str().ok().and_then(parse_url) else {
        tracing::warn!(uri = %uri.to_string_lossy(), "not a pstr:// URL");
        return LOADING_FAILED;
    };

    let Some(stream) = registry.lookup(id) else {
        tracing::warn!(id, "pstr:// token is not published");
        return LOADING_FAILED;
    };

    let cookie = Box::into_raw(Box::new(Cookie {
        cancel: CancelLatch::default(),
        cursor: UnsafeCell::new(StreamCursor::new(stream, Arc::clone(registry.runtime()))),
    }));

    // SAFETY: `info` is ours to fill for the duration of this callback.
    unsafe {
        (*info).cookie = cookie.cast::<c_void>();
        (*info).read_fn = Some(read);
        (*info).seek_fn = Some(seek);
        (*info).size_fn = Some(size);
        (*info).close_fn = Some(close);
        (*info).cancel_fn = Some(cancel);
    }
    0
}

unsafe extern "C" fn read(cookie: *mut c_void, buf: *mut c_char, nbytes: u64) -> i64 {
    if nbytes == 0 {
        return 0;
    }
    // SAFETY: our cookie, alive until `close`.
    let cookie = unsafe { &*cookie.cast::<Cookie>() };
    let len = usize::try_from(nbytes).unwrap_or(usize::MAX);
    // SAFETY: mpv guarantees `nbytes` of writable buffer.
    let buf = unsafe { std::slice::from_raw_parts_mut(buf.cast::<u8>(), len) };
    // SAFETY: read/seek/size/close are serialised on one thread, so this is the
    // only live borrow of the cursor.
    let cursor = unsafe { &mut *cookie.cursor.get() };
    cursor.read_caught(buf, &cookie.cancel)
}

unsafe extern "C" fn seek(cookie: *mut c_void, offset: i64) -> i64 {
    // SAFETY: as in `read`.
    let cookie = unsafe { &*cookie.cast::<Cookie>() };
    let cursor = unsafe { &mut *cookie.cursor.get() };
    cursor.seek(offset)
}

unsafe extern "C" fn size(cookie: *mut c_void) -> i64 {
    // SAFETY: as in `read`.
    let cookie = unsafe { &*cookie.cast::<Cookie>() };
    let cursor = unsafe { &*cookie.cursor.get() };
    cursor.size()
}

/// mpv calls this from another thread to interrupt a blocked read. Must not
/// block, and does not: it sets a flag and wakes the waiter.
unsafe extern "C" fn cancel(cookie: *mut c_void) {
    // SAFETY: our cookie. Only the latch is touched, which is `Sync`.
    let cookie = unsafe { &*cookie.cast::<Cookie>() };
    cookie.cancel.cancel();
}

unsafe extern "C" fn close(cookie: *mut c_void) {
    // SAFETY: mpv calls this exactly once per successful `open`, after which no
    // callback for this stream runs again. Dropping releases the stream's
    // reference to the ring and to the runtime.
    drop(unsafe { Box::from_raw(cookie.cast::<Cookie>()) });
}
