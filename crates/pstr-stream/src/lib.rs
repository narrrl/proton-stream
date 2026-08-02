//! Seekable, cached byte streams over Proton Drive revisions.
//!
//! Proton stores a file as a list of independently-encrypted 4 MiB blocks, each
//! a self-contained PGP packet under one session key. Nothing about that is
//! sequential — which is the fact this whole crate rests on. Seeking is not an
//! approximation of streaming here; block 400 costs exactly what block 0 costs.
//!
//! What sits between the SDK and the player:
//!
//! ```text
//!   mpv demuxer            VideoStream          BlockRing → DiskCache → Proton
//!   (small, forward,  →   byte range →     →    memory      disk        network
//!    seeking reads)        blocks              (128 MiB)   (4 GiB)
//! ```
//!
//! * [`StreamSource`] opens files and keeps the open ones in an LRU.
//! * [`VideoStream`] answers byte ranges, reading ahead of the player.
//! * The ring and the disk cache are byte-budgeted, shared across streams, and
//!   keyed by revision id so a resealed file can never serve stale bytes.
//!
//! Only [`reader`] touches the SDK. Everything else runs against a
//! [`BlockSource`], which is what makes the caching and read-ahead testable
//! without an account, a network or a share.
#![forbid(unsafe_code)]

pub mod block;
pub mod disk;
pub mod error;
pub mod reader;
pub mod ring;
mod single_flight;
pub mod source;
pub mod stream;

pub use block::{BlockMap, BlockSource, MemoryBlocks, SharedBlocks};
pub use disk::{DiskCacheConfig, DiskStats};
pub use error::{Error, Result};
/// Re-exported because every caller of this crate names one — `VideoStream::uid`
/// returns it and `StreamSource::open` takes it — and taking a dependency on the
/// whole SDK for a newtype is not a reasonable price for that.
pub use proton_sdk::ids::NodeUid;
pub use reader::{LibraryOpener, RevisionBlocks};
pub use ring::{DEFAULT_RING_BYTES, RingStats};
pub use source::{RevisionOpener, StreamConfig, StreamSource};
pub use stream::{DEFAULT_READAHEAD_BLOCKS, StreamStats, VideoStream};
