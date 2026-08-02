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

/// Where each block starts in the plaintext, so a byte range can be resolved to
/// blocks without a scan.
#[derive(Debug, Clone)]
pub struct BlockMap {
    /// Plaintext offset of each block. `starts[i] + sizes[i] == starts[i + 1]`.
    starts: Vec<u64>,
    sizes: Vec<u64>,
    size: u64,
}

impl BlockMap {
    pub fn new(sizes: &[u64]) -> Self {
        let mut starts = Vec::with_capacity(sizes.len());
        let mut offset = 0_u64;
        for &size in sizes {
            starts.push(offset);
            offset = offset.saturating_add(size);
        }
        Self {
            starts,
            sizes: sizes.to_vec(),
            size: offset,
        }
    }

    /// Total plaintext size of the revision.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// How many blocks the revision has.
    pub fn len(&self) -> usize {
        self.sizes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sizes.is_empty()
    }

    /// Plaintext offset where block `index` starts.
    pub fn start_of(&self, index: usize) -> Option<u64> {
        self.starts.get(index).copied()
    }

    /// Plaintext size of block `index`.
    pub fn size_of(&self, index: usize) -> Option<u64> {
        self.sizes.get(index).copied()
    }

    /// The block holding byte `offset`, or `None` at or past EOF.
    ///
    /// Binary search rather than a divide: block sizes are uniform in practice
    /// but nothing guarantees it, and a wrong division here is the exact bug the
    /// SDK warns about — bytes served from the wrong offset, silently.
    pub fn block_at(&self, offset: u64) -> Option<usize> {
        if offset >= self.size {
            return None;
        }
        match self.starts.binary_search(&offset) {
            Ok(index) => Some(index),
            // `starts` is sorted and starts at 0, so a miss is never at 0.
            Err(index) => Some(index - 1),
        }
    }

    /// Every block index overlapping `[offset, end)`, clamped to the revision.
    pub fn blocks_in(&self, offset: u64, end: u64) -> std::ops::Range<usize> {
        let end = end.min(self.size);
        if offset >= end {
            return 0..0;
        }
        let first = self.block_at(offset).unwrap_or(0);
        // `end` is exclusive, so the last byte read is `end - 1`.
        let last = self.block_at(end - 1).unwrap_or(first);
        first..last + 1
    }
}

/// Copy the part of `block` that falls inside `[offset, end)` into `out`.
///
/// `block_start` is where this block begins in the plaintext. Returns how many
/// bytes were written.
pub(crate) fn splice(
    out: &mut [u8],
    out_base: u64,
    block: &[u8],
    block_start: u64,
    offset: u64,
    end: u64,
) -> usize {
    let block_end = block_start.saturating_add(block.len() as u64);
    let from = offset.max(block_start);
    let to = end.min(block_end);
    if from >= to {
        return 0;
    }

    let src = (from - block_start) as usize..(to - block_start) as usize;
    let dst_start = (from - out_base) as usize;
    let len = src.end - src.start;
    out[dst_start..dst_start + len].copy_from_slice(&block[src]);
    len
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The realistic shape: uniform blocks with a short tail.
    fn map() -> BlockMap {
        BlockMap::new(&[4096, 4096, 1000])
    }

    #[test]
    fn a_block_map_sums_to_the_revision_size() {
        assert_eq!(map().size(), 9192);
        assert_eq!(map().len(), 3);
    }

    #[test]
    fn an_offset_resolves_to_the_block_holding_it() {
        let map = map();
        assert_eq!(map.block_at(0), Some(0));
        assert_eq!(map.block_at(4095), Some(0));
        assert_eq!(map.block_at(4096), Some(1));
        assert_eq!(map.block_at(8191), Some(1));
        assert_eq!(map.block_at(8192), Some(2));
        assert_eq!(map.block_at(9191), Some(2));
    }

    #[test]
    fn an_offset_at_or_past_the_end_resolves_to_nothing() {
        assert_eq!(map().block_at(9192), None);
        assert_eq!(map().block_at(u64::MAX), None);
    }

    #[test]
    fn a_range_covers_every_block_it_touches() {
        let map = map();
        assert_eq!(map.blocks_in(0, 1), 0..1);
        assert_eq!(map.blocks_in(4095, 4097), 0..2);
        assert_eq!(map.blocks_in(0, 9192), 0..3);
        assert_eq!(map.blocks_in(8192, 9192), 2..3);
    }

    /// A read ending exactly on a boundary must not pull the next block — that
    /// would be a 4 MiB fetch for zero bytes.
    #[test]
    fn a_range_ending_on_a_boundary_stops_short_of_the_next_block() {
        assert_eq!(map().blocks_in(0, 4096), 0..1);
        assert_eq!(map().blocks_in(4096, 8192), 1..2);
    }

    #[test]
    fn an_empty_or_past_the_end_range_covers_nothing() {
        let map = map();
        assert!(map.blocks_in(100, 100).is_empty());
        assert!(map.blocks_in(9192, 10000).is_empty());
        assert!(map.blocks_in(500, 100).is_empty());
    }

    /// Non-uniform block sizes are the case a divide-by-block-size would get
    /// wrong, so they are pinned explicitly.
    #[test]
    fn non_uniform_blocks_still_resolve_correctly() {
        let map = BlockMap::new(&[10, 1000, 5]);
        assert_eq!(map.block_at(9), Some(0));
        assert_eq!(map.block_at(10), Some(1));
        assert_eq!(map.block_at(1009), Some(1));
        assert_eq!(map.block_at(1010), Some(2));
        assert_eq!(map.size(), 1015);
    }

    #[test]
    fn a_spliced_block_lands_at_the_right_place_in_the_output() {
        // Reading bytes 4090..4106 of the map above: 6 bytes from block 0, 10
        // from block 1.
        let (offset, end) = (4090_u64, 4106_u64);
        let mut out = vec![0_u8; (end - offset) as usize];

        let block0 = vec![b'a'; 4096];
        let block1 = vec![b'b'; 4096];
        assert_eq!(splice(&mut out, offset, &block0, 0, offset, end), 6);
        assert_eq!(splice(&mut out, offset, &block1, 4096, offset, end), 10);

        assert_eq!(&out[..6], b"aaaaaa");
        assert_eq!(&out[6..], b"bbbbbbbbbb");
    }

    /// A block wholly outside the range writes nothing rather than corrupting
    /// the buffer.
    #[test]
    fn a_block_outside_the_range_writes_nothing() {
        let mut out = vec![0_u8; 8];
        let block = vec![b'x'; 4096];
        assert_eq!(splice(&mut out, 0, &block, 8192, 0, 8), 0);
        assert_eq!(out, vec![0_u8; 8]);
    }
}
