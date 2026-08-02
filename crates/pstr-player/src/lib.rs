//! Playback, through libmpv, of streams that only exist behind a Proton link.
//!
//! mpv is not asked to download anything. A custom `pstr://` protocol is
//! registered with `mpv_stream_cb_add_ro`, and mpv's demuxer reads through it
//! exactly as it would read a local file — small reads, seeks, an EOF. Every one
//! of those lands in [`pstr_stream`], which serves it from the block ring, the
//! disk cache or Proton, in that order.
//!
//! ```text
//!   Player::play(stream)                    registry: 7 → VideoStream
//!        │  loadfile pstr://7                        ▲
//!        ▼                                           │ hash lookup, no I/O
//!   mpv demuxer thread ── read/seek/size ──▶ StreamCursor ──▶ VideoStream
//!        │                                           │
//!        └── cancel (another thread) ──▶ CancelLatch ─┘
//! ```
//!
//! Three things about that shape are deliberate:
//!
//! * **mpv gets a token, not an identity.** The URL fragment of a Proton share
//!   link *is* its decryption password, and mpv puts a stream's URL in its log,
//!   its `path` property and its window title. A `u64` leaks nothing.
//! * **The open callback does no work.** Opening a revision can take seconds and
//!   fail for reasons a person needs to read; mpv's open callback can only say
//!   "loading failed". So the caller opens first and registers the result.
//! * **`stream_cb`, not a loopback HTTP server.** No port, no other local
//!   process able to ask for the plaintext, no decrypted bytes leaving the
//!   process.
//!
//! Where the picture ends up is a separate axis, and the streaming path above
//! is identical either way: [`VideoOutput::Window`] lets mpv make a window, and
//! [`VideoOutput::Embedded`] plus a [`VideoRenderer`] draws each frame into an
//! OpenGL framebuffer the caller owns. Only [`PlayerConfig`] and [`render`] know
//! the difference.

mod chapters;
mod cursor;
mod error;
mod gl;
mod player;
mod protocol;
mod registry;
mod render;
#[cfg(test)]
mod testing;
mod tracks;

pub use chapters::{Chapter, ChapterRole, chapter_at, chapter_end, credits_start, roles};
pub use error::{Error, Result};
pub use player::{
    EndReason, MAX_VOLUME, Player, PlayerConfig, PlayerEvent, READAHEAD_BLOCKS, VideoOutput,
};
pub use registry::{PROTOCOL, StreamHandle, StreamRegistry};
pub use render::VideoRenderer;
pub use tracks::{Track, TrackKind, language_name};
