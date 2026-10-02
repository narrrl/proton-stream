//! The bridge to the SDK: a real Proton revision as a [`BlockSource`].
//!
//! Everything else in this crate is toolkit-free and account-free. This is the
//! one module that knows a `RevisionReader` exists.
//!
//! Reads are issued **block-aligned**. `RevisionReader::read_at` fetches every
//! block a range overlaps, so an unaligned read would pull two blocks to serve
//! one, and the caching layer above would then hold neither of them whole.
//! Aligning here means each block is fetched, decrypted and cached exactly once.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use proton_drive_rs::ProtonDrivePublicLinkClient;
use proton_sdk::error::ProtonError;
use proton_sdk::ids::NodeUid;

use crate::block::{BlockMap, BlockSource, SharedBlocks};
use crate::error::{Error, Result};
use crate::source::RevisionOpener;

/// How many times one block is fetched before its failure reaches the reader.
const BLOCK_ATTEMPTS: u32 = 3;

/// The pause before a block's second fetch, doubled before each one after.
const BLOCK_RETRY_DELAY: Duration = Duration::from_millis(250);

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

        let block = fetch_retrying(BLOCK_RETRY_DELAY, || self.reader.read_at(start, size)).await?;
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

/// Fetch a block, again if the failure is one a replay could get past.
///
/// The SDK retries a request that fails before its response starts, but not a
/// block body cut off part way through: that error reaches us as it happened,
/// and without this it stops playback or fails a download. Its keep-alive pings
/// turn a silently dead connection into exactly that error, seconds in rather
/// than at the storage timeout, and the next attempt goes out on a new
/// connection. [`ProtonError::is_retriable`] decides what counts, so a
/// permanent refusal still fails on the first attempt.
async fn fetch_retrying<T, F, Fut>(
    first_delay: Duration,
    mut fetch: F,
) -> std::result::Result<T, ProtonError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = std::result::Result<T, ProtonError>>,
{
    let mut delay = first_delay;
    let mut attempt = 1;
    loop {
        match fetch().await {
            Err(error) if attempt < BLOCK_ATTEMPTS && error.is_retriable() => {
                tracing::debug!(attempt, %error, "block fetch failed; fetching again");
                tokio::time::sleep(delay).await;
                delay *= 2;
                attempt += 1;
            }
            result => return result,
        }
    }
}

/// A fully-downloaded revision. It uses the same block seam as Proton so mpv
/// can seek normally without a second playback path.
pub struct FileBlocks {
    revision_id: String,
    path: PathBuf,
    sizes: Vec<u64>,
    map: BlockMap,
}
impl FileBlocks {
    pub fn new(revision_id: String, path: PathBuf, sizes: Vec<u64>) -> Self {
        let map = BlockMap::new(&sizes);
        Self {
            revision_id,
            path,
            sizes,
            map,
        }
    }
}
#[async_trait]
impl BlockSource for FileBlocks {
    fn revision_id(&self) -> &str {
        &self.revision_id
    }
    fn block_sizes(&self) -> &[u64] {
        &self.sizes
    }
    async fn read_block(&self, index: usize) -> Result<Vec<u8>> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let (start, size) = match (self.map.start_of(index), self.map.size_of(index)) {
            (Some(a), Some(b)) => (a, b),
            _ => {
                return Err(Error::NotFound(format!(
                    "offline block {index} is past the end"
                )));
            }
        };
        let mut file = tokio::fs::File::open(&self.path).await?;
        file.seek(std::io::SeekFrom::Start(start)).await?;
        let mut data = vec![0; size as usize];
        file.read_exact(&mut data).await?;
        Ok(data)
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
    use std::sync::atomic::{AtomicU32, Ordering};

    use proton_sdk::api::ResponseCode;
    use proton_sdk::error::ProtonApiError;

    use super::*;

    fn api_error(http_status: u16) -> ProtonError {
        ProtonError::Api(ProtonApiError {
            code: ResponseCode::Unknown,
            http_status,
            message: String::new(),
            details: None,
        })
    }

    /// Run [`fetch_retrying`] over a fetch that fails with `failures` in turn and
    /// then succeeds, returning the result and how many fetches it took.
    async fn fetch_through(
        failures: Vec<ProtonError>,
    ) -> (std::result::Result<u8, ProtonError>, u32) {
        let calls = AtomicU32::new(0);
        let failures = std::sync::Mutex::new(failures.into_iter());
        let result = fetch_retrying(Duration::ZERO, || {
            calls.fetch_add(1, Ordering::Relaxed);
            let next = failures.lock().unwrap().next();
            async move { next.map_or(Ok(7), Err) }
        })
        .await;
        (result, calls.into_inner())
    }

    #[tokio::test]
    async fn a_block_that_fails_retriably_is_fetched_again() {
        let (result, calls) = fetch_through(vec![api_error(503)]).await;
        assert_eq!(result.expect("second fetch succeeds"), 7);
        assert_eq!(calls, 2);
    }

    #[tokio::test]
    async fn a_permanent_refusal_is_not_fetched_again() {
        let (result, calls) = fetch_through(vec![api_error(403)]).await;
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }

    #[tokio::test]
    async fn a_block_that_keeps_failing_returns_its_last_error() {
        let (result, calls) =
            fetch_through((0..BLOCK_ATTEMPTS + 1).map(|_| api_error(503)).collect()).await;
        assert!(result.is_err());
        assert_eq!(calls, BLOCK_ATTEMPTS);
    }

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
