//! Opening streams, and keeping the open ones around.
//!
//! A cold open is expensive: link details, an ancestor-key unlock (an S2K
//! derivation, tens of milliseconds), then a revision listing — all before the
//! first byte. Two things make that cost once instead of repeatedly:
//!
//! * an **LRU of open streams**, so going back to the episode you paused, or
//!   scrubbing after the player reopened its demuxer, is instant;
//! * **single-flight on the open itself**, because the UI's "play" click and the
//!   player's first read arrive milliseconds apart and would otherwise both pay
//!   for the same handshake, then race to insert into the LRU.

use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use lru::LruCache;
use proton_sdk::ids::NodeUid;

use crate::block::SharedBlocks;
use crate::disk::{DiskCache, DiskCacheConfig};
use crate::error::Result;
use crate::ring::{BlockRing, DEFAULT_RING_BYTES, non_zero};
use crate::single_flight::SingleFlight;
use crate::stream::{DEFAULT_READAHEAD_BLOCKS, VideoStream};

/// How the source turns a node into blocks.
///
/// A trait so the cache, ring and read-ahead can be exercised without an
/// account: the only implementation that talks to Proton is
/// [`crate::reader::LibraryOpener`].
#[async_trait]
pub trait RevisionOpener: Send + Sync + 'static {
    /// Open the active revision of `uid` inside `share_id`.
    async fn open(&self, share_id: &str, uid: &NodeUid) -> Result<SharedBlocks>;
}

/// How much memory, how much disk, and how far ahead to read.
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Resident block budget, shared by every open stream.
    pub ring_bytes: u64,
    /// Blocks to pull ahead of the reader. Zero disables read-ahead.
    pub readahead_blocks: usize,
    /// How many opened streams to keep. Cheap to hold — the bytes live in the
    /// ring, not here — so this is generous.
    pub max_open_streams: usize,
    /// The on-disk block cache, when enabled.
    pub disk_cache: Option<DiskCacheConfig>,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            ring_bytes: DEFAULT_RING_BYTES,
            readahead_blocks: DEFAULT_READAHEAD_BLOCKS,
            max_open_streams: 64,
            disk_cache: None,
        }
    }
}

impl StreamConfig {
    pub fn with_disk_cache(mut self, cache: DiskCacheConfig) -> Self {
        self.disk_cache = Some(cache);
        self
    }

    pub fn with_readahead(mut self, blocks: usize) -> Self {
        self.readahead_blocks = blocks;
        self
    }

    pub fn with_ring_bytes(mut self, bytes: u64) -> Self {
        self.ring_bytes = bytes;
        self
    }
}

/// Identifies an open stream. The share is part of the key: the same file can
/// legitimately be reachable through two different links.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StreamKey {
    share: String,
    uid: NodeUid,
}

/// Opens and caches [`VideoStream`]s.
#[derive(Clone)]
pub struct StreamSource {
    inner: Arc<Inner>,
}

struct Inner {
    opener: Arc<dyn RevisionOpener>,
    ring: Arc<BlockRing>,
    disk: Option<Arc<DiskCache>>,
    readahead: usize,
    open: Mutex<LruCache<StreamKey, VideoStream>>,
    flight: Arc<SingleFlight<StreamKey, VideoStream>>,
}

impl StreamSource {
    /// Build a source, opening the disk cache if one is configured.
    pub async fn new(opener: Arc<dyn RevisionOpener>, config: StreamConfig) -> Result<Self> {
        let disk = match config.disk_cache {
            Some(cache) => Some(Arc::new(DiskCache::open(cache).await?)),
            None => None,
        };

        Ok(Self {
            inner: Arc::new(Inner {
                opener,
                ring: Arc::new(BlockRing::new(config.ring_bytes)),
                disk,
                readahead: config.readahead_blocks,
                open: Mutex::new(LruCache::new(non_zero(config.max_open_streams))),
                flight: Arc::new(SingleFlight::new()),
            }),
        })
    }

    /// Open a file for playback, or hand back the stream already open on it.
    pub async fn open(&self, share_id: &str, uid: &NodeUid) -> Result<VideoStream> {
        let key = StreamKey {
            share: share_id.to_string(),
            uid: uid.clone(),
        };

        if let Some(stream) = self.inner.lock().get(&key) {
            return Ok(stream.clone());
        }

        let inner = Arc::clone(&self.inner);
        let flight = Arc::clone(&self.inner.flight);
        let share = share_id.to_string();
        let uid = uid.clone();
        let cached_key = key.clone();

        flight
            .run(key, move || async move {
                let blocks = inner.opener.open(&share, &uid).await?;
                let stream = VideoStream::new(
                    uid,
                    blocks,
                    Arc::clone(&inner.ring),
                    inner.disk.clone(),
                    inner.readahead,
                );
                // Evicting a stream here only drops the handle; its blocks stay
                // in the ring under the ring's own budget, so a viewer who
                // reopens still gets them.
                inner.lock().put(cached_key, stream.clone());
                Ok(stream)
            })
            .await
    }

    /// Drop a stream and everything cached for it in memory.
    ///
    /// The disk cache is deliberately untouched: it exists precisely so the next
    /// viewing is cheap.
    pub fn close(&self, share_id: &str, uid: &NodeUid) {
        let key = StreamKey {
            share: share_id.to_string(),
            uid: uid.clone(),
        };
        if let Some(stream) = self.inner.lock().pop(&key) {
            self.inner.ring.forget(uid, stream.revision_id());
        }
    }

    /// Ring statistics, across every stream.
    pub fn ring_stats(&self) -> crate::ring::RingStats {
        self.inner.ring.stats()
    }

    /// Disk-cache statistics, when one is configured.
    pub fn disk_stats(&self) -> Option<crate::disk::DiskStats> {
        self.inner.disk.as_ref().map(|disk| disk.stats())
    }
}

impl Inner {
    fn lock(&self) -> std::sync::MutexGuard<'_, LruCache<StreamKey, VideoStream>> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use proton_sdk::ids::{LinkId, VolumeId};

    use super::*;
    use crate::block::MemoryBlocks;

    fn uid(link: &str) -> NodeUid {
        NodeUid::new(VolumeId::new("vol"), LinkId::new(link))
    }

    struct CountingOpener {
        opens: AtomicUsize,
        delay: std::time::Duration,
    }

    impl CountingOpener {
        fn new(delay: std::time::Duration) -> Arc<Self> {
            Arc::new(Self {
                opens: AtomicUsize::new(0),
                delay,
            })
        }

        fn opens(&self) -> usize {
            self.opens.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl RevisionOpener for CountingOpener {
        async fn open(&self, _share: &str, _uid: &NodeUid) -> Result<SharedBlocks> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            Ok(Arc::new(MemoryBlocks::new(
                "rev1",
                vec![vec![1_u8; 64], vec![2_u8; 64]],
            )))
        }
    }

    async fn source(opener: Arc<CountingOpener>) -> StreamSource {
        StreamSource::new(opener, StreamConfig::default().with_readahead(0))
            .await
            .expect("source")
    }

    #[tokio::test]
    async fn an_opened_stream_reads_its_blocks() {
        let source = source(CountingOpener::new(std::time::Duration::ZERO)).await;
        let stream = source.open("share", &uid("a")).await.unwrap();

        assert_eq!(stream.size(), 128);
        assert_eq!(
            stream.read_range(60, 8).await.unwrap(),
            vec![1, 1, 1, 1, 2, 2, 2, 2]
        );
    }

    /// Reopening the episode you paused must not re-run the handshake.
    #[tokio::test]
    async fn reopening_the_same_file_reuses_the_open_stream() {
        let opener = CountingOpener::new(std::time::Duration::ZERO);
        let source = source(Arc::clone(&opener)).await;

        source.open("share", &uid("a")).await.unwrap();
        source.open("share", &uid("a")).await.unwrap();
        source.open("share", &uid("a")).await.unwrap();

        assert_eq!(opener.opens(), 1);
    }

    /// The race the single-flight exists for: the UI click and the player's
    /// first read, milliseconds apart, against a slow open.
    #[tokio::test]
    async fn concurrent_opens_of_one_file_handshake_once() {
        let opener = CountingOpener::new(std::time::Duration::from_millis(40));
        let source = source(Arc::clone(&opener)).await;

        let mut handles = Vec::new();
        for _ in 0..6 {
            let source = source.clone();
            handles.push(tokio::spawn(async move {
                source.open("share", &uid("a")).await
            }));
        }
        for handle in handles {
            handle.await.unwrap().unwrap();
        }

        assert_eq!(opener.opens(), 1);
    }

    /// The same file behind two links is two streams — the shares may have
    /// different lifetimes and different credentials.
    #[tokio::test]
    async fn the_same_file_in_two_shares_is_two_streams() {
        let opener = CountingOpener::new(std::time::Duration::ZERO);
        let source = source(Arc::clone(&opener)).await;

        source.open("share-one", &uid("a")).await.unwrap();
        source.open("share-two", &uid("a")).await.unwrap();
        assert_eq!(opener.opens(), 2);
    }

    /// Closing must reclaim the memory, or a long session leaks a ring's worth
    /// of every episode watched.
    #[tokio::test]
    async fn closing_a_stream_releases_its_blocks() {
        let source = source(CountingOpener::new(std::time::Duration::ZERO)).await;
        let stream = source.open("share", &uid("a")).await.unwrap();
        stream.read_range(0, 128).await.unwrap();
        assert!(source.ring_stats().resident_bytes > 0);

        source.close("share", &uid("a"));
        assert_eq!(source.ring_stats().resident_bytes, 0);
    }

    /// Beyond the LRU's capacity the oldest handle is dropped; the next open
    /// re-runs the handshake rather than failing.
    #[tokio::test]
    async fn the_open_stream_lru_is_bounded() {
        let opener = CountingOpener::new(std::time::Duration::ZERO);
        let source = StreamSource::new(
            Arc::clone(&opener) as Arc<dyn RevisionOpener>,
            StreamConfig {
                max_open_streams: 2,
                readahead_blocks: 0,
                ..StreamConfig::default()
            },
        )
        .await
        .unwrap();

        for link in ["a", "b", "c"] {
            source.open("share", &uid(link)).await.unwrap();
        }
        // "a" was evicted, so this is a fourth open.
        source.open("share", &uid("a")).await.unwrap();
        assert_eq!(opener.opens(), 4);
    }
}
