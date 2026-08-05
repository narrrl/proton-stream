//! A seekable byte stream over one revision, with caching and read-ahead.
//!
//! This is what the player reads from. mpv's demuxer asks for small, mostly
//! forward, occasionally wildly-seeking ranges; Proton serves 4 MiB encrypted
//! blocks. Everything between those two facts lives here:
//!
//! * a **byte range → blocks** mapping that never assumes a block size,
//! * a **memory ring** and an optional **disk cache**, checked in that order,
//! * **deduplication**, so read-ahead and the demand read never fetch the same
//!   block twice,
//! * **forward read-ahead**, so the demuxer's next read is usually already in
//!   memory rather than a 4 MiB round-trip away.
//!
//! ## Read-ahead has to yield to seeks
//!
//! Measured against a real 761 MiB episode over a public link, naive read-ahead
//! made everything *worse*: worst-case cold seek 2895 ms with six blocks of
//! prefetch versus 572 ms with none, and sustained throughput 5.0 MiB/s versus
//! 7.2. The cause is not subtle — prefetches queued from the position the viewer
//! just left keep running, and the block they are fetching is exactly the
//! bandwidth the seek needs.
//!
//! So a read that is not a continuation of the previous one cancels every
//! outstanding prefetch before it fetches anything. Read-ahead resumes from
//! wherever the seek settles. What is thrown away is speculative by definition;
//! what is protected is the only latency the viewer can feel.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use futures::stream::{self, StreamExt, TryStreamExt};
use proton_sdk::ids::NodeUid;
use tokio::sync::Semaphore;

use crate::block::{BlockMap, SharedBlocks, splice};
use crate::disk::{DiskCache, DiskStats};
use crate::error::Result;
use crate::ring::{BlockKey, BlockRing, CachedBlock, RingStats};
use crate::single_flight::SingleFlight;

/// Blocks fetched concurrently to satisfy one read.
///
/// A read spanning more than a couple of blocks only happens on a cold seek;
/// steady-state playback reads well inside one. Kept modest because the SDK
/// enforces its own client-wide in-flight ceiling underneath this.
const MAX_CONCURRENT_BLOCKS_PER_READ: usize = 4;

/// How much of the file to pull ahead of the reader, in blocks.
///
/// Measured on a 761 MiB episode over a real public link, reading in 64 KiB
/// chunks against the default 128 MiB ring:
///
/// | depth | sustained | note |
/// |------:|----------:|------|
/// | 0     | 7.2 MiB/s | strictly one block at a time — latency-bound |
/// | 6     | 5.8 MiB/s | *worse* than serial: too shallow to overlap, still competing |
/// | 12    | 8.7 MiB/s | no evictions |
/// | 16    | 8.1–9.6 MiB/s | occasional evictions |
/// | 32    | 7.8 MiB/s | regresses — see [`clamp_readahead`] |
///
/// Run-to-run spread is wide enough that 12 and 16 are not meaningfully
/// different; 12 is chosen because it never evicted and wasted a third less
/// bandwidth on cancelled prefetches. Well past a 1080p bitrate either way.
pub const DEFAULT_READAHEAD_BLOCKS: usize = 12;

/// How far a read may land from where the previous one ended and still count as
/// a continuation rather than a seek.
///
/// One 4 MiB block's worth. Demuxers routinely re-read a little behind
/// themselves for headers and index entries, and a small forward hop is a
/// container skipping a chunk, not a viewer moving the seek bar. Treating either
/// as a seek would cancel read-ahead constantly and give back everything it
/// buys.
const SEQUENTIAL_SLACK: u64 = 4 * 1024 * 1024;

/// What a stream did, for the benchmark and the settings screen.
#[derive(Debug, Clone, Copy)]
pub struct StreamStats {
    pub ring: RingStats,
    pub disk: Option<DiskStats>,
    /// Blocks that actually came off the network.
    pub blocks_fetched: u64,
    pub bytes_fetched: u64,
    /// Read-ahead fetches that were started.
    pub readahead_blocks: u64,
    /// Read-ahead fetches cancelled by a seek.
    pub readahead_cancelled: u64,
    /// Reads that were not a continuation of the previous one.
    pub seeks: u64,
}

/// One open revision, readable at any offset.
///
/// Cheap to clone — clones share the same reader, cache state and read-ahead.
#[derive(Clone)]
pub struct VideoStream {
    inner: Arc<Inner>,
}

struct Inner {
    uid: NodeUid,
    blocks: SharedBlocks,
    map: BlockMap,
    ring: Arc<BlockRing>,
    disk: Option<Arc<DiskCache>>,
    /// One fetch per block, however many readers want it.
    flight: Arc<SingleFlight<BlockKey, CachedBlock>>,
    readahead: usize,
    /// Bounds detached read-ahead work, so a seek-happy viewer cannot queue an
    /// unbounded pile of prefetches.
    readahead_slots: Arc<Semaphore>,
    /// Live prefetches, so a seek can cancel them. See the module note.
    readahead_tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Plaintext offset just past the previous read, for seek detection.
    last_end: AtomicU64,
    fetched_blocks: AtomicU64,
    fetched_bytes: AtomicU64,
    readahead_started: AtomicU64,
    readahead_cancelled: AtomicU64,
    seeks: AtomicU64,
}

impl VideoStream {
    /// Build a seekable stream from complete local content.
    pub fn offline(uid: NodeUid, blocks: SharedBlocks, ring_bytes: u64) -> Self {
        Self::new(uid, blocks, Arc::new(BlockRing::new(ring_bytes)), None, 0)
    }
    pub(crate) fn new(
        uid: NodeUid,
        blocks: SharedBlocks,
        ring: Arc<BlockRing>,
        disk: Option<Arc<DiskCache>>,
        readahead: usize,
    ) -> Self {
        let map = BlockMap::new(blocks.block_sizes());
        let readahead = clamp_readahead(readahead, ring.budget(), blocks.block_sizes());
        Self {
            inner: Arc::new(Inner {
                uid,
                blocks,
                map,
                ring,
                disk,
                flight: Arc::new(SingleFlight::new()),
                readahead,
                readahead_slots: Arc::new(Semaphore::new(readahead.max(1))),
                readahead_tasks: Mutex::new(Vec::new()),
                last_end: AtomicU64::new(0),
                fetched_blocks: AtomicU64::new(0),
                fetched_bytes: AtomicU64::new(0),
                readahead_started: AtomicU64::new(0),
                readahead_cancelled: AtomicU64::new(0),
                seeks: AtomicU64::new(0),
            }),
        }
    }

    /// Total plaintext size of the revision.
    pub fn size(&self) -> u64 {
        self.inner.map.size()
    }

    /// The node this stream reads.
    pub fn uid(&self) -> &NodeUid {
        &self.inner.uid
    }

    /// The revision this stream is pinned to. It does not follow a reseal —
    /// reopen for that.
    pub fn revision_id(&self) -> &str {
        self.inner.blocks.revision_id()
    }

    /// Plaintext size of each block, in block order.
    pub fn block_sizes(&self) -> &[u64] {
        self.inner.blocks.block_sizes()
    }

    /// Fill `buf` from `offset`, returning how many bytes were written.
    ///
    /// A read at or past EOF returns 0 rather than failing — that is how a
    /// demuxer discovers the end of a file.
    pub async fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let inner = &self.inner;
        if buf.is_empty() || offset >= inner.map.size() {
            return Ok(0);
        }
        let end = offset
            .saturating_add(buf.len() as u64)
            .min(inner.map.size());

        // Before anything is fetched: a seek must not queue behind the
        // speculation it just invalidated. See the module note — this is worth
        // roughly 5x on worst-case seek latency.
        let previous_end = inner.last_end.swap(end, Ordering::Relaxed);
        if !is_continuation(offset, previous_end) {
            inner.seeks.fetch_add(1, Ordering::Relaxed);
            self.cancel_read_ahead();
        }

        let wanted = inner.map.blocks_in(offset, end);
        // Taken by value: a closure borrowing from the range would give each
        // fetch future a higher-ranked lifetime, which `tokio::spawn` rejects in
        // callers with "implementation of `FnOnce` is not general enough".
        let indices: Vec<usize> = wanted.clone().collect();
        let last = wanted.end;

        let mut fetches = stream::iter(
            indices
                .into_iter()
                .map(|index| block_at(Arc::clone(inner), index)),
        )
        .buffered(MAX_CONCURRENT_BLOCKS_PER_READ);

        let mut written = 0_usize;
        let mut index = wanted.start;
        // `buffered` yields in input order, so blocks arrive in file order and
        // each one splices into its own part of `buf`.
        while let Some(block) = fetches.try_next().await? {
            let start = inner.map.start_of(index).unwrap_or(0);
            written += splice(buf, offset, &block, start, offset, end);
            index += 1;
        }

        self.read_ahead_from(last);
        Ok(written)
    }

    /// [`Self::read_at`] into a fresh buffer. Convenient for tests and for the
    /// benchmark; the player uses the buffer form.
    pub async fn read_range(&self, offset: u64, length: u64) -> Result<Vec<u8>> {
        let end = offset.saturating_add(length).min(self.size());
        if length == 0 || offset >= end {
            return Ok(Vec::new());
        }
        let mut buf = vec![0_u8; (end - offset) as usize];
        let read = self.read_at(offset, &mut buf).await?;
        buf.truncate(read);
        Ok(buf)
    }

    pub fn stats(&self) -> StreamStats {
        StreamStats {
            ring: self.inner.ring.stats(),
            disk: self.inner.disk.as_ref().map(|disk| disk.stats()),
            blocks_fetched: self.inner.fetched_blocks.load(Ordering::Relaxed),
            bytes_fetched: self.inner.fetched_bytes.load(Ordering::Relaxed),
            readahead_blocks: self.inner.readahead_started.load(Ordering::Relaxed),
            readahead_cancelled: self.inner.readahead_cancelled.load(Ordering::Relaxed),
            seeks: self.inner.seeks.load(Ordering::Relaxed),
        }
    }

    /// Abandon every outstanding prefetch.
    ///
    /// Aborting mid-fetch throws away whatever that block had downloaded. That
    /// is the intended trade: those bytes were speculative, and the request slot
    /// they occupy is what the seek is waiting for. A demand read that later
    /// wants an aborted block simply refetches it — the single-flight entry is
    /// released by its guard when the task is dropped, so nothing waits on a
    /// call that will never finish.
    fn cancel_read_ahead(&self) {
        let tasks = std::mem::take(&mut *self.inner.tasks());
        let mut cancelled = 0_u64;
        for task in tasks {
            if !task.is_finished() {
                task.abort();
                cancelled += 1;
            }
        }
        if cancelled > 0 {
            self.inner
                .readahead_cancelled
                .fetch_add(cancelled, Ordering::Relaxed);
        }
    }

    /// Queue the next few blocks after `from`.
    ///
    /// Detached and best-effort by design: a prefetch that fails, or that never
    /// gets a slot, costs the next demand read a round-trip and nothing else.
    /// Blocks already resident are skipped without touching their recency, so
    /// speculation cannot reorder the LRU against real playback.
    fn read_ahead_from(&self, from: usize) {
        let inner = &self.inner;
        if inner.readahead == 0 {
            return;
        }

        let revision = inner.blocks.revision_id();
        let upto = (from + inner.readahead).min(inner.map.len());
        for index in from..upto {
            let key = BlockKey::new(&inner.uid, revision, index);
            if inner.ring.contains(&key) {
                continue;
            }
            let Ok(slot) = Arc::clone(&inner.readahead_slots).try_acquire_owned() else {
                // Already as far ahead as this stream is allowed to run.
                return;
            };

            inner.readahead_started.fetch_add(1, Ordering::Relaxed);
            let spawned = Arc::clone(inner);
            let task = tokio::spawn(async move {
                let _slot = slot;
                if let Err(e) = block_at(spawned, index).await {
                    tracing::debug!(index, error = %e, "read-ahead block failed");
                }
            });

            let mut tasks = inner.tasks();
            // Finished handles are dead weight; drop them while we are here so
            // the list stays the size of the read-ahead window.
            tasks.retain(|task| !task.is_finished());
            tasks.push(task);
        }
    }
}

/// Cap read-ahead at half the ring's capacity in blocks.
///
/// Reading further ahead than the ring can hold is self-defeating: the blocks at
/// the front of the window are evicted before the player reaches them, so they
/// are fetched twice and the bandwidth spent on them is lost. Measured at depth
/// 32 against a 128 MiB ring (32 blocks), sustained throughput fell from
/// 8.7 MiB/s to 7.8 with six evictions — worse than depth 12.
///
/// Half, not all, because the ring also has to hold what the player has just
/// *passed*: a scrub a few seconds backwards is common and should not refetch.
fn clamp_readahead(requested: usize, ring_budget: u64, block_sizes: &[u64]) -> usize {
    let block = block_sizes.iter().copied().max().unwrap_or(0);
    if block == 0 {
        return requested;
    }
    let capacity = (ring_budget / block) as usize;
    requested.min((capacity / 2).max(1))
}

/// Whether a read starting at `offset` continues from where the last one ended.
///
/// Deliberately generous in both directions — see [`SEQUENTIAL_SLACK`]. The
/// first read of a stream (`previous_end == 0`, `offset == 0`) is a
/// continuation, so opening a file does not count as a seek.
fn is_continuation(offset: u64, previous_end: u64) -> bool {
    offset >= previous_end.saturating_sub(SEQUENTIAL_SLACK)
        && offset <= previous_end.saturating_add(SEQUENTIAL_SLACK)
}

impl Inner {
    fn tasks(&self) -> std::sync::MutexGuard<'_, Vec<tokio::task::JoinHandle<()>>> {
        self.readahead_tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Resolve one block: memory, then disk, then the network.
///
/// Takes an owned `Arc` so the future is `'static` and can be both spawned for
/// read-ahead and buffered inside a read.
async fn block_at(inner: Arc<Inner>, index: usize) -> Result<CachedBlock> {
    let key = BlockKey::new(&inner.uid, inner.blocks.revision_id(), index);

    if let Some(block) = inner.ring.get(&key) {
        return Ok(block);
    }

    let flight = Arc::clone(&inner.flight);
    let fetch_key = key.clone();
    flight
        .run(key, move || async move {
            if let Some(disk) = inner.disk.as_ref()
                && let Some(bytes) = disk.get(&fetch_key).await
            {
                let block: CachedBlock = bytes.into();
                inner.ring.insert(fetch_key, Arc::clone(&block));
                return Ok(block);
            }

            let bytes = inner.blocks.read_block(index).await?;
            inner.fetched_blocks.fetch_add(1, Ordering::Relaxed);
            inner
                .fetched_bytes
                .fetch_add(bytes.len() as u64, Ordering::Relaxed);

            if let Some(disk) = inner.disk.as_ref() {
                disk.put(&fetch_key, &bytes).await;
            }
            let block: CachedBlock = bytes.into();
            inner.ring.insert(fetch_key, Arc::clone(&block));
            Ok(block)
        })
        .await
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use async_trait::async_trait;
    use proton_sdk::ids::{LinkId, VolumeId};

    use super::*;
    use crate::block::BlockSource;
    use crate::ring::DEFAULT_RING_BYTES;

    fn uid() -> NodeUid {
        NodeUid::new(VolumeId::new("vol"), LinkId::new("link"))
    }

    /// Deterministic content: byte `i` of the file is `(i % 251) as u8`, so any
    /// range can be checked without holding the whole file.
    fn expected(offset: u64, len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| ((offset + i as u64) % 251) as u8)
            .collect()
    }

    /// A block source that counts fetches per block and can be made slow or
    /// failing.
    ///
    /// Per-block counters rather than one total: read-ahead runs detached, so a
    /// total is inherently racy, whereas "block 1 was fetched exactly once" is
    /// the property the tests actually care about and is stable regardless of
    /// when the prefetch tasks happen to be scheduled.
    struct Fake {
        sizes: Vec<u64>,
        starts: Vec<u64>,
        per_block: Vec<AtomicUsize>,
        delay: std::time::Duration,
        fail_block: Option<usize>,
    }

    impl Fake {
        fn new(sizes: &[u64]) -> Arc<Self> {
            let mut starts = Vec::new();
            let mut offset = 0;
            for &size in sizes {
                starts.push(offset);
                offset += size;
            }
            Arc::new(Self {
                sizes: sizes.to_vec(),
                starts,
                per_block: sizes.iter().map(|_| AtomicUsize::new(0)).collect(),
                delay: std::time::Duration::ZERO,
                fail_block: None,
            })
        }

        fn slow(sizes: &[u64], delay: std::time::Duration) -> Arc<Self> {
            let mut fake = Fake::new(sizes);
            Arc::get_mut(&mut fake).unwrap().delay = delay;
            fake
        }

        fn failing(sizes: &[u64], block: usize) -> Arc<Self> {
            let mut fake = Fake::new(sizes);
            Arc::get_mut(&mut fake).unwrap().fail_block = Some(block);
            fake
        }

        fn fetches_of(&self, index: usize) -> usize {
            self.per_block[index].load(Ordering::SeqCst)
        }

        fn fetches(&self) -> usize {
            self.per_block
                .iter()
                .map(|count| count.load(Ordering::SeqCst))
                .sum()
        }
    }

    #[async_trait]
    impl BlockSource for Fake {
        fn revision_id(&self) -> &str {
            "rev1"
        }

        fn block_sizes(&self) -> &[u64] {
            &self.sizes
        }

        async fn read_block(&self, index: usize) -> Result<Vec<u8>> {
            if Some(index) == self.fail_block {
                return Err(crate::Error::NotFound(format!("block {index} is broken")));
            }
            self.per_block[index].fetch_add(1, Ordering::SeqCst);
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            let start = self.starts[index];
            Ok(expected(start, self.sizes[index] as usize))
        }
    }

    fn stream_over(fake: Arc<Fake>, readahead: usize) -> VideoStream {
        VideoStream::new(
            uid(),
            fake,
            Arc::new(BlockRing::new(DEFAULT_RING_BYTES)),
            None,
            readahead,
        )
    }

    #[tokio::test]
    async fn a_stream_reports_the_revisions_size() {
        let stream = stream_over(Fake::new(&[1000, 1000, 500]), 0);
        assert_eq!(stream.size(), 2500);
        assert_eq!(stream.block_sizes(), &[1000, 1000, 500]);
    }

    #[tokio::test]
    async fn a_read_inside_one_block_returns_the_right_bytes() {
        let stream = stream_over(Fake::new(&[1000, 1000, 500]), 0);
        assert_eq!(stream.read_range(10, 20).await.unwrap(), expected(10, 20));
    }

    /// The case that matters for playback: a read straddling a block boundary
    /// must stitch two blocks without a seam.
    #[tokio::test]
    async fn a_read_across_a_boundary_stitches_blocks_correctly() {
        let stream = stream_over(Fake::new(&[1000, 1000, 500]), 0);
        assert_eq!(stream.read_range(990, 30).await.unwrap(), expected(990, 30));
    }

    #[tokio::test]
    async fn a_read_spanning_every_block_returns_the_whole_file() {
        let stream = stream_over(Fake::new(&[1000, 1000, 500]), 0);
        assert_eq!(stream.read_range(0, 2500).await.unwrap(), expected(0, 2500));
    }

    /// A cold seek to the middle must fetch only the block it lands in — that is
    /// the entire reason this app can exist over a 4 MiB block store.
    #[tokio::test]
    async fn a_seek_fetches_only_the_blocks_it_touches() {
        let fake = Fake::new(&[1000; 100]);
        let stream = stream_over(Arc::clone(&fake), 0);

        stream.read_range(75_000, 100).await.unwrap();
        assert_eq!(fake.fetches(), 1, "seek pulled more than the landing block");
    }

    /// The short tail. Reading past it yields what exists, not an error.
    #[tokio::test]
    async fn a_read_past_the_end_is_clamped_rather_than_failing() {
        let stream = stream_over(Fake::new(&[1000, 500]), 0);
        let bytes = stream.read_range(1400, 1000).await.unwrap();
        assert_eq!(bytes, expected(1400, 100));
    }

    #[tokio::test]
    async fn a_read_at_or_past_the_end_returns_nothing() {
        let stream = stream_over(Fake::new(&[100]), 0);
        assert!(stream.read_range(100, 10).await.unwrap().is_empty());
        assert!(stream.read_range(5_000, 10).await.unwrap().is_empty());
    }

    /// Rewatching a scene must come out of memory, not off the wire.
    #[tokio::test]
    async fn a_repeated_read_is_served_from_the_ring() {
        let fake = Fake::new(&[1000, 1000]);
        let stream = stream_over(Arc::clone(&fake), 0);

        stream.read_range(0, 100).await.unwrap();
        stream.read_range(200, 100).await.unwrap();
        stream.read_range(500, 100).await.unwrap();

        assert_eq!(fake.fetches(), 1);
        assert!(stream.stats().ring.hits >= 2);
    }

    /// Read-ahead exists so the demuxer's *next* read is already resident.
    #[tokio::test]
    async fn read_ahead_pulls_the_following_blocks() {
        let fake = Fake::new(&[1000; 10]);
        let stream = stream_over(Arc::clone(&fake), 3);

        stream.read_range(0, 100).await.unwrap();
        // Detached tasks; give them a chance to run.
        for _ in 0..50 {
            if fake.fetches() >= 4 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        for index in 0..4 {
            assert_eq!(fake.fetches_of(index), 1, "block {index} not read ahead");
        }
        assert_eq!(fake.fetches_of(4), 0, "read-ahead ran past its window");

        // And the next read is served from what read-ahead already pulled.
        assert_eq!(
            stream.read_range(1000, 50).await.unwrap(),
            expected(1000, 50)
        );
        assert_eq!(fake.fetches_of(1), 1, "read-ahead's block was refetched");
    }

    /// Read-ahead must stop at EOF rather than fetching blocks that do not
    /// exist.
    #[tokio::test]
    async fn read_ahead_stops_at_the_end_of_the_file() {
        let fake = Fake::new(&[1000, 1000]);
        let stream = stream_over(Arc::clone(&fake), 8);

        stream.read_range(0, 10).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(fake.fetches(), 2, "fetched past the last block");
    }

    /// The collision read-ahead makes inevitable: the demand read wants the
    /// block a prefetch is already fetching. It must wait for it, not start a
    /// second fetch.
    #[tokio::test]
    async fn a_demand_read_joins_an_in_flight_read_ahead_instead_of_refetching() {
        let fake = Fake::slow(&[1000; 10], std::time::Duration::from_millis(80));
        let stream = stream_over(Arc::clone(&fake), 4);

        stream.read_range(0, 10).await.unwrap();
        // Read-ahead for blocks 1..5 is now in flight against an 80 ms source.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        assert_eq!(
            stream.read_range(1000, 10).await.unwrap(),
            expected(1000, 10)
        );
        assert_eq!(
            fake.fetches_of(1),
            1,
            "the demand read started a second fetch of an in-flight block"
        );
    }

    /// Read-ahead is speculative; its failures must never surface to the player.
    #[tokio::test]
    async fn a_failing_read_ahead_does_not_fail_the_read() {
        let fake = Fake::failing(&[1000; 5], 2);
        let stream = stream_over(Arc::clone(&fake), 4);

        assert_eq!(stream.read_range(0, 10).await.unwrap(), expected(0, 10));
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        // And the demand read for that block still reports the real failure.
        assert!(stream.read_range(2000, 10).await.is_err());
    }

    /// A source failure on the demand path is an error, not silence — silence
    /// would look like EOF and the player would stop mid-episode.
    #[tokio::test]
    async fn a_failing_block_surfaces_as_an_error() {
        let fake = Fake::failing(&[1000; 3], 0);
        let stream = stream_over(fake, 0);
        assert!(stream.read_range(0, 10).await.is_err());
    }

    /// Concurrent readers of one stream — mpv's demuxer and its seek probe do
    /// exactly this — must not multiply the fetches.
    #[tokio::test]
    async fn concurrent_reads_of_the_same_block_fetch_it_once() {
        let fake = Fake::slow(&[4096], std::time::Duration::from_millis(50));
        let stream = stream_over(Arc::clone(&fake), 0);

        let mut handles = Vec::new();
        for i in 0..8_u64 {
            let stream = stream.clone();
            handles.push(tokio::spawn(
                async move { stream.read_range(i * 10, 10).await },
            ));
        }
        for (i, handle) in handles.into_iter().enumerate() {
            let bytes = handle.await.unwrap().unwrap();
            assert_eq!(bytes, expected(i as u64 * 10, 10));
        }
        assert_eq!(fake.fetches(), 1);
    }

    /// Non-uniform block sizes must read correctly — the SDK warns that
    /// assuming 4 MiB serves bytes from the wrong offset, silently.
    #[tokio::test]
    async fn non_uniform_block_sizes_read_correctly() {
        let fake = Fake::new(&[13, 1021, 7, 4096, 1]);
        let stream = stream_over(fake, 0);
        assert_eq!(stream.size(), 5138);
        assert_eq!(stream.read_range(0, 5138).await.unwrap(), expected(0, 5138));
        assert_eq!(
            stream.read_range(1030, 40).await.unwrap(),
            expected(1030, 40)
        );
    }

    /// The memory promise holds even for a file far larger than the ring.
    #[tokio::test]
    async fn streaming_a_file_larger_than_the_ring_stays_inside_the_budget() {
        let fake = Fake::new(&[1024; 200]);
        let stream = VideoStream::new(uid(), fake, Arc::new(BlockRing::new(8 * 1024)), None, 0);

        let mut offset = 0;
        while offset < stream.size() {
            stream.read_range(offset, 512).await.unwrap();
            assert!(stream.stats().ring.resident_bytes <= 8 * 1024);
            offset += 512;
        }
    }

    #[tokio::test]
    async fn read_at_fills_the_callers_buffer() {
        let stream = stream_over(Fake::new(&[1000, 1000]), 0);
        let mut buf = vec![0_u8; 64];
        assert_eq!(stream.read_at(990, &mut buf).await.unwrap(), 64);
        assert_eq!(buf, expected(990, 64));
    }

    /// Reading further ahead than the ring can hold evicts the window's own
    /// blocks before the player reaches them.
    #[test]
    fn read_ahead_is_capped_at_half_the_rings_capacity() {
        // 128 MiB of 4 MiB blocks is 32 blocks, so at most 16 ahead.
        let four_mib = 4 * 1024 * 1024;
        assert_eq!(clamp_readahead(32, 128 * 1024 * 1024, &[four_mib; 200]), 16);
        assert_eq!(clamp_readahead(12, 128 * 1024 * 1024, &[four_mib; 200]), 12);
    }

    /// A ring too small for even one block must still read ahead by one, or
    /// playback degenerates to a stall per block with nothing overlapping.
    #[test]
    fn a_tiny_ring_still_reads_one_block_ahead() {
        assert_eq!(clamp_readahead(12, 1024, &[4 * 1024 * 1024]), 1);
    }

    /// A stream configured with no read-ahead must keep none — the clamp raises
    /// a floor for what to *fetch*, not for whether to fetch at all.
    #[tokio::test]
    async fn read_ahead_stays_off_when_it_is_configured_off() {
        let fake = Fake::new(&[1024; 32]);
        let stream = VideoStream::new(
            uid(),
            Arc::clone(&fake) as SharedBlocks,
            Arc::new(BlockRing::new(1024)),
            None,
            0,
        );
        stream.read_range(0, 100).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(fake.fetches(), 1);
    }

    #[test]
    fn a_forward_read_within_one_block_continues_the_previous_one() {
        assert!(is_continuation(0, 0), "the first read is not a seek");
        assert!(is_continuation(1_000, 1_000));
        assert!(is_continuation(1_000 + SEQUENTIAL_SLACK, 1_000));
    }

    /// Demuxers re-read a little behind themselves for headers and index
    /// entries. Calling that a seek would cancel read-ahead constantly.
    #[test]
    fn a_small_backward_read_still_continues_the_previous_one() {
        assert!(is_continuation(100_000_000 - SEQUENTIAL_SLACK, 100_000_000));
        assert!(is_continuation(0, SEQUENTIAL_SLACK));
    }

    #[test]
    fn a_jump_past_the_slack_is_a_seek() {
        assert!(!is_continuation(100_000_000, 1_000));
        assert!(!is_continuation(0, 100_000_000));
    }

    /// The measured bug: prefetches from the position the viewer left were
    /// holding the bandwidth the seek needed, costing 5x on worst-case latency.
    #[tokio::test]
    async fn a_seek_cancels_the_read_ahead_it_invalidates() {
        let fake = Fake::slow(&[1024 * 1024; 200], std::time::Duration::from_millis(200));
        let stream = stream_over(Arc::clone(&fake), 4);

        stream.read_range(0, 1_000).await.unwrap();
        // Prefetches for blocks 1..5 are now in flight against a slow source.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(stream.stats().readahead_blocks > 0, "nothing to cancel");

        // Far past the slack: a seek-bar drag.
        stream.read_range(150 * 1024 * 1024, 1_000).await.unwrap();

        let stats = stream.stats();
        assert_eq!(stats.seeks, 1);
        assert!(
            stats.readahead_cancelled > 0,
            "seek did not cancel the stale prefetches"
        );
    }

    /// And the mirror image: steady playback must not keep throwing its own
    /// read-ahead away.
    #[tokio::test]
    async fn sequential_playback_never_cancels_its_own_read_ahead() {
        let fake = Fake::new(&[1024; 64]);
        let stream = stream_over(fake, 4);

        let mut offset = 0;
        while offset < stream.size() {
            stream.read_range(offset, 256).await.unwrap();
            offset += 256;
        }

        let stats = stream.stats();
        assert_eq!(stats.seeks, 0, "sequential reads read as seeks");
        assert_eq!(stats.readahead_cancelled, 0);
    }

    /// A cancelled prefetch must not leave the block unfetchable — the
    /// single-flight entry has to be released when the task is dropped.
    #[tokio::test]
    async fn a_block_whose_prefetch_was_cancelled_can_still_be_read() {
        let fake = Fake::slow(&[1024 * 1024; 200], std::time::Duration::from_millis(100));
        let stream = stream_over(Arc::clone(&fake), 4);

        stream.read_range(0, 1_000).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        stream.read_range(150 * 1024 * 1024, 1_000).await.unwrap();

        // Block 1 was almost certainly aborted mid-fetch; reading it must work.
        assert_eq!(
            stream.read_range(1024 * 1024, 16).await.unwrap(),
            expected(1024 * 1024, 16)
        );
    }

    /// The player passes a fixed-size buffer at every offset, including the last
    /// one; a short read there is normal and must be reported honestly.
    #[tokio::test]
    async fn a_short_read_at_the_tail_reports_what_it_wrote() {
        let stream = stream_over(Fake::new(&[100]), 0);
        let mut buf = vec![0_u8; 64];
        assert_eq!(stream.read_at(80, &mut buf).await.unwrap(), 20);
        assert_eq!(&buf[..20], &expected(80, 20)[..]);
    }
}
