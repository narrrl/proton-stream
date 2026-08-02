//! The in-memory block cache: a byte-budgeted LRU shared by every open stream.
//!
//! Sized in **bytes**, not entries. A block is 4 MiB in practice but nothing
//! guarantees it, and a count-based bound would turn into a wildly different
//! memory ceiling on a library uploaded by a different client. The budget is the
//! app's actual resident-memory promise, so it is what gets counted.
//!
//! Shared across streams rather than one per stream: a viewer who abandons an
//! episode after ninety seconds should not keep 128 MiB of it resident while the
//! next one starves.

use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, PoisonError};

use lru::LruCache;
use proton_sdk::ids::NodeUid;

/// Identifies one decrypted block, globally.
///
/// The revision id is part of the key on purpose: a file that gains a new
/// revision must not serve a byte of the old one out of cache. It is a stable
/// identity that advances iff a new revision was sealed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct BlockKey {
    pub uid: NodeUid,
    pub revision: String,
    pub index: usize,
}

impl BlockKey {
    pub(crate) fn new(uid: &NodeUid, revision: &str, index: usize) -> Self {
        Self {
            uid: uid.clone(),
            revision: revision.to_string(),
            index,
        }
    }
}

/// A decrypted block, shared by reference so a cache hit costs no copy.
pub(crate) type CachedBlock = Arc<[u8]>;

/// Hit/miss counters, for the benchmark and the settings screen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RingStats {
    pub hits: u64,
    pub misses: u64,
    /// Blocks dropped to stay inside the budget.
    pub evictions: u64,
    /// Bytes currently resident.
    pub resident_bytes: u64,
    pub blocks: usize,
}

pub(crate) struct BlockRing {
    inner: Mutex<Inner>,
    budget: u64,
}

struct Inner {
    /// Unbounded by entry count — eviction is driven by `bytes` against the
    /// budget, below.
    lru: LruCache<BlockKey, CachedBlock>,
    bytes: u64,
    hits: u64,
    misses: u64,
    evictions: u64,
}

impl BlockRing {
    pub(crate) fn new(budget_bytes: u64) -> Self {
        Self {
            inner: Mutex::new(Inner {
                lru: LruCache::unbounded(),
                bytes: 0,
                hits: 0,
                misses: 0,
                evictions: 0,
            }),
            budget: budget_bytes,
        }
    }

    /// Look a block up, marking it most-recently-used on a hit.
    pub(crate) fn get(&self, key: &BlockKey) -> Option<CachedBlock> {
        let mut inner = self.lock();
        match inner.lru.get(key) {
            Some(block) => {
                let block = Arc::clone(block);
                inner.hits += 1;
                Some(block)
            }
            None => {
                inner.misses += 1;
                None
            }
        }
    }

    /// Insert a block, evicting until the budget is met.
    ///
    /// A block larger than the whole budget is *not* stored — storing it would
    /// evict everything else and then still be over. It is still returned to the
    /// caller that fetched it; it simply is not cached.
    pub(crate) fn insert(&self, key: BlockKey, block: CachedBlock) {
        let len = block.len() as u64;
        if len > self.budget {
            return;
        }

        let mut inner = self.lock();
        if let Some(previous) = inner.lru.put(key, block) {
            inner.bytes -= previous.len() as u64;
        }
        inner.bytes += len;

        while inner.bytes > self.budget {
            match inner.lru.pop_lru() {
                Some((_, evicted)) => {
                    inner.bytes -= evicted.len() as u64;
                    inner.evictions += 1;
                }
                // Unreachable while `bytes` and `lru` agree; not worth a panic.
                None => {
                    inner.bytes = 0;
                    break;
                }
            }
        }
    }

    /// Whether a block is resident, *without* counting a hit or touching its
    /// recency. Read-ahead asks this to decide what to prefetch, and it must not
    /// look like demand traffic in the stats or in the LRU order.
    pub(crate) fn contains(&self, key: &BlockKey) -> bool {
        self.lock().lru.contains(key)
    }

    /// The byte budget this ring was built with.
    pub(crate) fn budget(&self) -> u64 {
        self.budget
    }

    pub(crate) fn stats(&self) -> RingStats {
        let inner = self.lock();
        RingStats {
            hits: inner.hits,
            misses: inner.misses,
            evictions: inner.evictions,
            resident_bytes: inner.bytes,
            blocks: inner.lru.len(),
        }
    }

    /// Drop every block of one revision. Used when a stream is closed and its
    /// bytes are known to be dead.
    pub(crate) fn forget(&self, uid: &NodeUid, revision: &str) {
        let mut inner = self.lock();
        let doomed: Vec<BlockKey> = inner
            .lru
            .iter()
            .filter(|(key, _)| &key.uid == uid && key.revision == revision)
            .map(|(key, _)| key.clone())
            .collect();
        for key in doomed {
            if let Some(block) = inner.lru.pop(&key) {
                inner.bytes -= block.len() as u64;
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // Non-poisoning for the same reason the rest of the stack is: a panic in
        // one read must not make every later read fail. The cache holds no
        // invariant a panic could leave half-updated.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The default in-memory budget: 128 MiB, matching the FUSE client's stream
/// ring. At 4 MiB a block that is 32 blocks — about two minutes of a 1080p
/// episode, enough to absorb a scrub backwards without refetching.
pub const DEFAULT_RING_BYTES: u64 = 128 * 1024 * 1024;

/// `lru` wants a non-zero capacity for bounded caches, and a configured zero
/// must clamp rather than panic.
pub(crate) fn non_zero(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value.max(1)).expect("max(1) is non-zero")
}

#[cfg(test)]
mod tests {
    use super::*;
    use proton_sdk::ids::{LinkId, VolumeId};

    fn uid(link: &str) -> NodeUid {
        NodeUid::new(VolumeId::new("vol"), LinkId::new(link))
    }

    fn block(byte: u8, len: usize) -> CachedBlock {
        vec![byte; len].into()
    }

    #[test]
    fn a_stored_block_reads_back() {
        let ring = BlockRing::new(1024);
        let key = BlockKey::new(&uid("a"), "rev1", 0);
        ring.insert(key.clone(), block(7, 16));

        assert_eq!(ring.get(&key).expect("hit").as_ref(), &[7_u8; 16]);
        assert_eq!(ring.stats().hits, 1);
    }

    /// The budget is the app's memory promise. Exceeding it is the one thing
    /// this type must not do.
    #[test]
    fn the_ring_never_exceeds_its_byte_budget() {
        let ring = BlockRing::new(100);
        for index in 0..10 {
            ring.insert(BlockKey::new(&uid("a"), "rev1", index), block(1, 30));
            assert!(
                ring.stats().resident_bytes <= 100,
                "over budget at block {index}"
            );
        }
        assert_eq!(ring.stats().blocks, 3);
        assert!(ring.stats().evictions > 0);
    }

    /// Eviction order must be least-recently-*used*, not insertion order, or a
    /// viewer scrubbing back and forth loses the block they keep returning to.
    #[test]
    fn the_least_recently_used_block_is_evicted_first() {
        let ring = BlockRing::new(60);
        let (a, b) = (
            BlockKey::new(&uid("x"), "rev1", 0),
            BlockKey::new(&uid("x"), "rev1", 1),
        );
        ring.insert(a.clone(), block(1, 30));
        ring.insert(b.clone(), block(2, 30));

        // Touch `a`, making `b` the eviction candidate.
        assert!(ring.get(&a).is_some());
        ring.insert(BlockKey::new(&uid("x"), "rev1", 2), block(3, 30));

        assert!(ring.get(&a).is_some(), "recently used block was evicted");
        assert!(ring.get(&b).is_none(), "stale block survived");
    }

    /// The reason the revision id is in the key: a resealed file must never
    /// serve a byte of its previous content.
    #[test]
    fn a_new_revision_does_not_hit_the_old_revisions_blocks() {
        let ring = BlockRing::new(1024);
        ring.insert(BlockKey::new(&uid("a"), "rev1", 0), block(1, 8));

        assert!(ring.get(&BlockKey::new(&uid("a"), "rev2", 0)).is_none());
    }

    /// And neither must two different files at the same block index.
    #[test]
    fn two_files_do_not_collide_at_the_same_block_index() {
        let ring = BlockRing::new(1024);
        ring.insert(BlockKey::new(&uid("a"), "rev1", 0), block(1, 8));
        ring.insert(BlockKey::new(&uid("b"), "rev1", 0), block(2, 8));

        assert_eq!(
            ring.get(&BlockKey::new(&uid("a"), "rev1", 0)).unwrap()[0],
            1
        );
        assert_eq!(
            ring.get(&BlockKey::new(&uid("b"), "rev1", 0)).unwrap()[0],
            2
        );
    }

    /// Read-ahead polls this constantly; it must not distort demand statistics
    /// or promote a block it only *considered* fetching.
    #[test]
    fn a_containment_check_records_no_traffic_and_no_recency() {
        let ring = BlockRing::new(60);
        let (a, b) = (
            BlockKey::new(&uid("x"), "rev1", 0),
            BlockKey::new(&uid("x"), "rev1", 1),
        );
        ring.insert(a.clone(), block(1, 30));
        ring.insert(b.clone(), block(2, 30));

        assert!(ring.contains(&a));
        assert_eq!(ring.stats().hits, 0);
        assert_eq!(ring.stats().misses, 0);

        // `a` is still the LRU victim despite having been asked about.
        ring.insert(BlockKey::new(&uid("x"), "rev1", 2), block(3, 30));
        assert!(!ring.contains(&a));
    }

    /// A block bigger than the whole budget must not flush the cache to make
    /// room for something that cannot fit anyway.
    #[test]
    fn a_block_larger_than_the_budget_is_not_stored() {
        let ring = BlockRing::new(50);
        let keeper = BlockKey::new(&uid("a"), "rev1", 0);
        ring.insert(keeper.clone(), block(1, 40));
        ring.insert(BlockKey::new(&uid("a"), "rev1", 1), block(2, 500));

        assert!(ring.get(&keeper).is_some(), "existing block was flushed");
        assert_eq!(ring.stats().blocks, 1);
    }

    #[test]
    fn forgetting_a_revision_reclaims_its_bytes() {
        let ring = BlockRing::new(1024);
        for index in 0..4 {
            ring.insert(BlockKey::new(&uid("a"), "rev1", index), block(1, 32));
        }
        ring.insert(BlockKey::new(&uid("b"), "rev1", 0), block(2, 32));

        ring.forget(&uid("a"), "rev1");
        let stats = ring.stats();
        assert_eq!(stats.blocks, 1);
        assert_eq!(stats.resident_bytes, 32);
    }

    /// Re-inserting the same key must not double-count its bytes; a refetched
    /// block would otherwise inflate the accounting until the ring evicted
    /// everything.
    #[test]
    fn reinserting_a_block_does_not_double_count_its_bytes() {
        let ring = BlockRing::new(1024);
        let key = BlockKey::new(&uid("a"), "rev1", 0);
        ring.insert(key.clone(), block(1, 64));
        ring.insert(key.clone(), block(1, 64));

        assert_eq!(ring.stats().resident_bytes, 64);
        assert_eq!(ring.stats().blocks, 1);
    }
}
