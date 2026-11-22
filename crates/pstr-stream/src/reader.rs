//! Opening a share's files for streaming.

use std::sync::Arc;

use async_trait::async_trait;

use crate::{Error, NodeUid, Result, RevisionBlocks, RevisionOpener, SharedBlocks};

/// Opens revisions out of the configured shares.
pub struct LibraryOpener {
    library: Arc<pstr_core::SharedLibrary>,
}

impl LibraryOpener {
    pub fn new(library: Arc<pstr_core::SharedLibrary>) -> Self {
        Self { library }
    }

    fn client(&self, share_id: &str) -> Result<&pstr_core::ShareClient> {
        self.library
            .client(share_id)
            .ok_or_else(|| Error::NotFound(format!("share {share_id} is not open")))
    }
}

#[async_trait]
impl RevisionOpener<NodeUid> for LibraryOpener {
    async fn open(&self, share_id: &str, uid: &NodeUid) -> Result<SharedBlocks> {
        let reader = self
            .client(share_id)?
            .open_revision(uid)
            .await
            .map_err(|error| Error::Source(Box::new(error)))?;
        Ok(Arc::new(RevisionBlocks::new(reader)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The player hands its stream to mpv's demuxer thread, so everything from
    /// the opener down has to cross threads.
    #[test]
    fn the_library_opener_and_streams_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<LibraryOpener>();
        assert_send_sync::<crate::StreamSource>();
        assert_send_sync::<crate::VideoStream>();
    }
}
