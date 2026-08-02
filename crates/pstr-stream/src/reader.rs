//! The bridge to the SDK: a real Proton revision as a [`BlockSource`].
//!
//! Everything else in this crate is toolkit-free and account-free. This is the
//! one module that knows a `RevisionReader` exists.
//!
//! Reads are issued **block-aligned**. `RevisionReader::read_at` fetches every
//! block a range overlaps, so an unaligned read would pull two blocks to serve
//! one, and the caching layer above would then hold neither of them whole.
//! Aligning here means each block is fetched, decrypted and cached exactly once.

use std::sync::Arc;

use async_trait::async_trait;
use proton_drive_rs::ProtonDrivePublicLinkClient;
use proton_sdk::ids::NodeUid;

use crate::block::{BlockMap, BlockSource, SharedBlocks};
use crate::error::{Error, Result};
use crate::source::RevisionOpener;

/// One open revision on a public link.
pub struct RevisionBlocks {
    reader: proton_drive_rs::RevisionReader,
    map: BlockMap,
}

impl RevisionBlocks {
    pub fn new(reader: proton_drive_rs::RevisionReader) -> Self {
        let map = BlockMap::new(reader.block_sizes());
        Self { reader, map }
    }
}

#[async_trait]
impl BlockSource for RevisionBlocks {
    fn revision_id(&self) -> &str {
        self.reader.revision_id()
    }

    fn block_sizes(&self) -> &[u64] {
        self.reader.block_sizes()
    }

    async fn read_block(&self, index: usize) -> Result<Vec<u8>> {
        let (Some(start), Some(size)) = (self.map.start_of(index), self.map.size_of(index)) else {
            return Err(Error::NotFound(format!(
                "block {index} is past the end of revision {}",
                self.reader.revision_id()
            )));
        };

        let block = self.reader.read_at(start, size).await?;
        // A short block here is not a tail — the map says how long it is. It
        // means the revision changed underneath us or the block table lied, and
        // serving it would silently shift every later byte.
        if block.len() as u64 != size {
            return Err(Error::NotFound(format!(
                "block {index} came back {} bytes, expected {size}",
                block.len()
            )));
        }
        Ok(block)
    }
}

/// Opens revisions out of the configured shares.
pub struct LibraryOpener {
    library: Arc<pstr_core::SharedLibrary>,
}

impl LibraryOpener {
    pub fn new(library: Arc<pstr_core::SharedLibrary>) -> Self {
        Self { library }
    }

    fn client(&self, share_id: &str) -> Result<&ProtonDrivePublicLinkClient> {
        self.library
            .client(share_id)
            .ok_or_else(|| Error::NotFound(format!("share {share_id} is not open")))
    }
}

#[async_trait]
impl RevisionOpener for LibraryOpener {
    async fn open(&self, share_id: &str, uid: &NodeUid) -> Result<SharedBlocks> {
        let reader = self.client(share_id)?.open_revision(uid).await?;
        Ok(Arc::new(RevisionBlocks::new(reader)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The player hands its stream to mpv's demuxer thread, so everything from
    /// the opener down has to cross threads. A compile-time guard, because the
    /// failure mode is an unhelpful `FnOnce is not general enough` error deep in
    /// a caller rather than here.
    #[test]
    fn the_library_opener_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<LibraryOpener>();
        assert_send_sync::<RevisionBlocks>();
        assert_send_sync::<crate::StreamSource>();
        assert_send_sync::<crate::VideoStream>();
    }
}
