//! The `proton-stream` desktop application, as a library.
//!
//! A window over one or more Proton Drive public links: crawl them once, and
//! what is behind them becomes a library that plays without downloading first.
//!
//! Three threads' worth of work, kept apart on purpose:
//!
//! * **The UI thread** draws. It never opens a share, reads the catalog or
//!   decodes an image — see [`engine`]. It is also the only thread that may
//!   touch OpenGL, which is why [`video`] and the construction of a
//!   [`playback::Playback`] both live on it.
//! * **The tokio runtime** does everything with a network or a disk in it. It is
//!   built by the binary rather than with `#[tokio::main]`, because
//!   [`pstr_player`] needs an `Arc<Runtime>` it can block on from mpv's demuxer
//!   thread, which is outside tokio entirely.
//! * **A player thread** drives mpv for the length of a file — see [`playback`].
//!
//! This is a library as well as a binary so the parts that can only be checked
//! against a real GPU are reachable from `examples/` — see
//! `examples/embedded_video.rs`, which is the smoke test for [`video`].

pub mod app;
pub mod engine;
pub mod playback;
pub mod theme;
pub mod ui;
pub mod video;
