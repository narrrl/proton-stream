//! What the stream layer needs from whatever is holding the bytes.
//!
//! A Proton revision is a list of independently-encrypted content blocks, and
//! every layer above this one — the memory ring, the disk cache, the read-ahead
//! — thinks purely in whole blocks. So the seam is a *block* source, not a byte
//! source: [`BlockSource`] hands over one decrypted block at a time, and
//! [`BlockMap`] turns a byte range into the blocks that overlap it.
//!
//! The trait exists for a second reason, and it is the one that pays: with it,
//! everything in this crate is testable against a deterministic fake, with no
//! account, no network and no share. Only [`crate::reader`] touches the SDK.

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::Result;

/// A revision's decrypted blocks, addressed by index.
///
/// Implementors must be cheap to keep alive — one is held per open stream.
#[async_trait]
pub trait BlockSource: Send + Sync + 'static {
    /// The revision these blocks belong to.
    ///
    /// This is the cache validity key. Unlike a modification time it advances
    /// **iff** a new revision was sealed, so bytes cached under one revision id
    /// can never be served for another one's content.
    fn revision_id(&self) -> &str;

    /// Plaintext size of each block, in block order.
    ///
    /// Never assume 4 MiB. Proton's block size is a property of the *uploader*,
    /// the last block is short, and a padded size vector would silently serve
    /// bytes from the wrong offset.
    fn block_sizes(&self) -> &[u64];

    /// Fetch and decrypt one block.
    async fn read_block(&self, index: usize) -> Result<Vec<u8>>;
}

/// Byte-range math over the block sizes; shared with the other Proton apps.
pub use proton_media_rs::BlockMap;
pub(crate) use proton_media_rs::splice;

/// A `BlockSource` over bytes already in memory. Used by the crate's own tests
/// and by the benchmark's warm-up path.
#[derive(Debug)]
pub struct MemoryBlocks {
    revision: String,
    blocks: Vec<Vec<u8>>,
    sizes: Vec<u64>,
}

impl MemoryBlocks {
    pub fn new(revision: impl Into<String>, blocks: Vec<Vec<u8>>) -> Self {
        let sizes = blocks.iter().map(|block| block.len() as u64).collect();
        Self {
            revision: revision.into(),
            blocks,
            sizes,
        }
    }
}

#[async_trait]
impl BlockSource for MemoryBlocks {
    fn revision_id(&self) -> &str {
        &self.revision
    }

    fn block_sizes(&self) -> &[u64] {
        &self.sizes
    }

    async fn read_block(&self, index: usize) -> Result<Vec<u8>> {
        self.blocks
            .get(index)
            .cloned()
            .ok_or_else(|| crate::Error::NotFound(format!("block {index} is past the end")))
    }
}

/// Boxed for storage in the stream.
pub type SharedBlocks = Arc<dyn BlockSource>;
