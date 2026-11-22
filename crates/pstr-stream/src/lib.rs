//! Seekable, cached byte streams over Proton Drive revisions.
//!
//! The engine (memory ring, disk cache, read-ahead that yields to seeks, the
//! LRU of open streams) lives in `proton-media-rs`'s `stream` module, shared
//! with the other Proton apps. This crate pins it to Proton node UIDs and adds
//! the one piece that knows about this app's shares: [`LibraryOpener`].
//!
//! A Proton file's content blocks each decrypt on their own, so seeking costs
//! one block fetch, not a re-stream. Never assume a 4 MiB block: the sizes come
//! from the revision.
#![forbid(unsafe_code)]

pub mod reader;

pub use proton_media_rs::stream::{
    BlockSource, COPY_BLOCKS_IN_FLIGHT, DEFAULT_READAHEAD_BLOCKS, DEFAULT_RING_BYTES, FileBlocks,
    MemoryBlocks, RevisionBlocks, RingStats, SharedBlocks, StreamConfig, StreamStats,
};
pub use proton_media_rs::{BlockMap, DiskCacheConfig, DiskStats, Error, Result};
/// Re-exported because every caller of this crate names one, and taking a
/// dependency on the whole SDK for a newtype is not a reasonable price for that.
pub use proton_sdk::ids::NodeUid;
pub use reader::LibraryOpener;

/// What a share's file is opened as: one revision, readable at any offset.
pub type VideoStream = proton_media_rs::stream::RangeStream<NodeUid>;
/// Opens and caches [`VideoStream`]s.
pub type StreamSource = proton_media_rs::stream::StreamSource<NodeUid>;

/// How a [`StreamSource`] turns a node into blocks.
pub use proton_media_rs::stream::RevisionOpener;
