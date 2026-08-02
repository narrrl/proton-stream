//! Fixtures shared by this crate's tests.
//!
//! Everything below mpv runs against an in-memory block source, so the cursor's
//! behaviour — seeks, boundaries, EOF, cancellation — is pinned without an
//! account, a network or a window.

use std::sync::Arc;

use async_trait::async_trait;
use pstr_stream::{
    MemoryBlocks, NodeUid, RevisionOpener, SharedBlocks, StreamConfig, StreamSource, VideoStream,
};
use tokio::runtime::Runtime;

/// Non-uniform on purpose, with a short tail: a uniform-block fixture would let
/// an off-by-one in block mapping pass.
pub(crate) const CONTENT_BLOCKS: [usize; 4] = [4096, 4096, 4096, 1000];

/// `byte[i] == (i % 251) as u8`. 251 is prime and coprime with every block
/// size here, so bytes served from the wrong block never coincidentally match.
fn content() -> Vec<Vec<u8>> {
    let mut at = 0_usize;
    CONTENT_BLOCKS
        .iter()
        .map(|len| {
            let block = (at..at + len).map(|i| (i % 251) as u8).collect();
            at += len;
            block
        })
        .collect()
}

struct Fixed(SharedBlocks);

#[async_trait]
impl RevisionOpener for Fixed {
    async fn open(&self, _share: &str, _uid: &NodeUid) -> pstr_stream::Result<SharedBlocks> {
        Ok(Arc::clone(&self.0))
    }
}

pub(crate) fn test_runtime() -> Arc<Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build a test runtime"),
    )
}

pub(crate) fn test_stream(runtime: &Arc<Runtime>) -> VideoStream {
    let blocks: SharedBlocks = Arc::new(MemoryBlocks::new("rev-1", content()));
    runtime.block_on(async {
        let source = StreamSource::new(Arc::new(Fixed(blocks)), StreamConfig::default())
            .await
            .expect("build a stream source");
        source
            .open("share-1", &NodeUid::new("volume-1".into(), "link-1".into()))
            .await
            .expect("open the fixture stream")
    })
}
