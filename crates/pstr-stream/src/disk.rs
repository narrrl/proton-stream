//! The on-disk block cache: re-watching an episode should not re-download it.
//!
//! ## Why a sidecar, and why it is written last
//!
//! An entry is two files — `b{index}` holding the decrypted block, and
//! `b{index}.meta` holding its length. The block is written and synced **first**,
//! the sidecar **last**. That ordering is the whole validation scheme: a crash
//! or a full disk mid-write leaves a block with no sidecar, or a sidecar whose
//! length disagrees with the file, and both read as a miss. There is no state in
//! which a truncated block is served as if it were whole — which for a video
//! stream would not error, it would just decode garbage.
//!
//! Lifted from `proton-drive-linux`'s `pdfs-core/src/cache.rs`, which learned
//! the ordering the hard way.
//!
//! ## Why the path is hashed
//!
//! `{sha256(uid + revision)}` keyed by revision, not by name or mtime. A file
//! that gains a new revision lands in an entirely different directory, so stale
//! bytes are unreachable rather than merely unlikely. Names never touch the
//! filesystem: they are the one part of a share that is genuinely private, and
//! writing them into cache paths would leak the library's contents to anyone who
//! can list a directory.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use lru::LruCache;
use sha2::{Digest, Sha256};

use crate::error::Result;
use crate::ring::BlockKey;

/// How much disk the cache may use, and where.
#[derive(Debug, Clone)]
pub struct DiskCacheConfig {
    pub root: PathBuf,
    pub budget_bytes: u64,
}

impl DiskCacheConfig {
    /// 4 GiB — roughly two 1080p episodes' worth of blocks, small enough to be
    /// unremarkable on any machine that plays video.
    pub const DEFAULT_BUDGET_BYTES: u64 = 4 * 1024 * 1024 * 1024;

    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            budget_bytes: Self::DEFAULT_BUDGET_BYTES,
        }
    }

    pub fn with_budget(mut self, bytes: u64) -> Self {
        self.budget_bytes = bytes;
        self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiskStats {
    pub hits: u64,
    pub misses: u64,
    /// Entries dropped to stay inside the budget.
    pub evictions: u64,
    /// Entries found unreadable or inconsistent, and therefore ignored.
    pub rejected: u64,
    pub stored_bytes: u64,
    pub entries: usize,
}

pub(crate) struct DiskCache {
    root: PathBuf,
    budget: u64,
    index: Mutex<Index>,
}

struct Index {
    /// Keyed by the block file's path, because that is all a startup scan can
    /// recover — the key it was written under is hashed away by design.
    lru: LruCache<PathBuf, u64>,
    bytes: u64,
    stats: DiskStats,
}

impl DiskCache {
    /// Open the cache, adopting whatever a previous run left behind.
    ///
    /// The scan is ordered by modification time so the recovered LRU is at least
    /// approximately right; within a session, real access order takes over.
    pub(crate) async fn open(config: DiskCacheConfig) -> Result<Self> {
        let root = config.root.clone();
        let scanned = tokio::task::spawn_blocking(move || scan(&root)).await;

        let entries = match scanned {
            Ok(Ok(entries)) => entries,
            // A cache that cannot be scanned is a cache that starts empty. It is
            // disposable by definition; refusing to play a film over it would be
            // absurd.
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "block cache could not be scanned; starting empty");
                Vec::new()
            }
            Err(e) => {
                tracing::warn!(error = %e, "block cache scan failed to run; starting empty");
                Vec::new()
            }
        };

        let mut lru = LruCache::unbounded();
        let mut bytes = 0_u64;
        for (path, len) in entries {
            bytes += len;
            lru.put(path, len);
        }

        let cache = Self {
            root: config.root,
            budget: config.budget_bytes,
            index: Mutex::new(Index {
                lru,
                bytes,
                stats: DiskStats {
                    stored_bytes: bytes,
                    ..DiskStats::default()
                },
            }),
        };
        cache.evict_to_budget().await;
        Ok(cache)
    }

    /// Read a block back, or `None` when it is absent or fails validation.
    pub(crate) async fn get(&self, key: &BlockKey) -> Option<Vec<u8>> {
        let path = self.block_path(key);
        match read_entry(&path).await {
            Ok(Some(block)) => {
                let mut index = self.lock();
                index.stats.hits += 1;
                // Promotes it, and adopts an entry a concurrent writer added.
                let len = block.len() as u64;
                if index.lru.get(&path).is_none() {
                    index.bytes += len;
                    index.lru.put(path, len);
                }
                Some(block)
            }
            Ok(None) => {
                self.lock().stats.misses += 1;
                None
            }
            Err(e) => {
                tracing::debug!(path = %path.display(), error = %e, "cached block rejected");
                // Scoped, not `drop`ped: a guard merely *moved* before an await
                // is still captured by the generated future, which would make
                // every read non-`Send` and unspawnable.
                {
                    let mut index = self.lock();
                    index.stats.misses += 1;
                    index.stats.rejected += 1;
                }
                // A half-written entry is never going to become valid.
                self.discard(&path).await;
                None
            }
        }
    }

    /// Store a block. Failures are logged and swallowed: a cache that cannot be
    /// written is a slower app, not a broken one.
    pub(crate) async fn put(&self, key: &BlockKey, block: &[u8]) {
        let len = block.len() as u64;
        if len > self.budget {
            return;
        }

        let path = self.block_path(key);
        if let Err(e) = write_entry(&path, block).await {
            tracing::warn!(path = %path.display(), error = %e, "could not cache block to disk");
            self.discard(&path).await;
            return;
        }

        {
            let mut index = self.lock();
            if let Some(previous) = index.lru.put(path, len) {
                index.bytes -= previous;
            }
            index.bytes += len;
        }
        self.evict_to_budget().await;
    }

    pub(crate) fn stats(&self) -> DiskStats {
        let index = self.lock();
        DiskStats {
            stored_bytes: index.bytes,
            entries: index.lru.len(),
            ..index.stats
        }
    }

    /// Drop entries, least-recently-used first, until inside the budget.
    async fn evict_to_budget(&self) {
        loop {
            let doomed = {
                let mut index = self.lock();
                if index.bytes <= self.budget {
                    return;
                }
                match index.lru.pop_lru() {
                    Some((path, len)) => {
                        index.bytes -= len;
                        index.stats.evictions += 1;
                        path
                    }
                    None => {
                        index.bytes = 0;
                        return;
                    }
                }
            };
            self.discard(&doomed).await;
        }
    }

    /// Remove both halves of an entry. The sidecar goes first, so an
    /// interrupted deletion leaves an entry that fails validation rather than
    /// one that claims to be valid.
    async fn discard(&self, path: &Path) {
        let _ = tokio::fs::remove_file(meta_path(path)).await;
        let _ = tokio::fs::remove_file(path).await;
    }

    /// `{root}/{aa}/{hash}/b{index}` — two levels of fan-out so a large library
    /// does not put tens of thousands of entries in one directory.
    fn block_path(&self, key: &BlockKey) -> PathBuf {
        let mut hasher = Sha256::new();
        hasher.update(key.uid.to_string().as_bytes());
        hasher.update(b"\0");
        hasher.update(key.revision.as_bytes());
        let digest = hasher.finalize();
        let hash = hex(&digest);

        self.root
            .join(&hash[..2])
            .join(&hash)
            .join(format!("b{}", key.index))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Index> {
        self.index.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn meta_path(block: &Path) -> PathBuf {
    let mut name = block.as_os_str().to_os_string();
    name.push(".meta");
    PathBuf::from(name)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// Read an entry, validating it against its sidecar.
///
/// `Ok(None)` is "not cached". `Err` is "cached, but wrong" — the caller deletes
/// those.
async fn read_entry(path: &Path) -> Result<Option<Vec<u8>>> {
    let claimed = match tokio::fs::read_to_string(meta_path(path)).await {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };

    let claimed_len: u64 = claimed.trim().parse().map_err(|_| {
        crate::Error::NotFound(format!(
            "{} holds no usable length",
            meta_path(path).display()
        ))
    })?;

    let block = match tokio::fs::read(path).await {
        Ok(block) => block,
        // Sidecar without a block: the block was evicted or half-deleted.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(crate::Error::NotFound("cached block is missing".into()));
        }
        Err(e) => return Err(e.into()),
    };

    if block.len() as u64 != claimed_len {
        return Err(crate::Error::NotFound(format!(
            "cached block is {} bytes, sidecar claims {claimed_len}",
            block.len()
        )));
    }
    Ok(Some(block))
}

/// Write an entry: block, synced, then sidecar. See the module note.
async fn write_entry(path: &Path, block: &[u8]) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    let parent = path
        .parent()
        .ok_or_else(|| crate::Error::NotFound(format!("{} has no parent", path.display())))?;
    tokio::fs::create_dir_all(parent).await?;

    // Through a temp file so a concurrent reader never sees a partial block even
    // momentarily.
    let temp = meta_path(path).with_extension("part");
    {
        let mut file = tokio::fs::File::create(&temp).await?;
        file.write_all(block).await?;
        file.sync_all().await?;
    }
    tokio::fs::rename(&temp, path).await?;

    let mut sidecar = tokio::fs::File::create(meta_path(path)).await?;
    sidecar
        .write_all(block.len().to_string().as_bytes())
        .await?;
    sidecar.sync_all().await?;
    Ok(())
}

/// Walk the cache tree, returning `(block path, size)` oldest-first.
///
/// Blocking, so it runs on the blocking pool. Anything that is not a valid pair
/// is skipped rather than deleted — a cache directory the user pointed at by
/// mistake should not be eaten.
fn scan(root: &Path) -> std::io::Result<Vec<(PathBuf, u64)>> {
    let mut found: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();

    let shards = match std::fs::read_dir(root) {
        Ok(shards) => shards,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };

    for shard in shards.flatten() {
        let Ok(revisions) = std::fs::read_dir(shard.path()) else {
            continue;
        };
        for revision in revisions.flatten() {
            let Ok(blocks) = std::fs::read_dir(revision.path()) else {
                continue;
            };
            for block in blocks.flatten() {
                let path = block.path();
                // Sidecars and interrupted writes are accounted through their
                // block, not on their own.
                if path.extension().is_some() {
                    continue;
                }
                let Ok(metadata) = block.metadata() else {
                    continue;
                };
                if !metadata.is_file() || !meta_path(&path).exists() {
                    continue;
                }
                let modified = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
                found.push((path, metadata.len(), modified));
            }
        }
    }

    found.sort_by_key(|(_, _, modified)| *modified);
    Ok(found
        .into_iter()
        .map(|(path, len, _)| (path, len))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proton_sdk::ids::{LinkId, NodeUid, VolumeId};

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pstr-disk-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn key(link: &str, revision: &str, index: usize) -> BlockKey {
        BlockKey::new(
            &NodeUid::new(VolumeId::new("vol"), LinkId::new(link)),
            revision,
            index,
        )
    }

    #[tokio::test]
    async fn a_stored_block_reads_back() {
        let root = temp_root("roundtrip");
        let cache = DiskCache::open(DiskCacheConfig::new(&root)).await.unwrap();

        let block = vec![9_u8; 4096];
        cache.put(&key("a", "rev1", 3), &block).await;
        assert_eq!(cache.get(&key("a", "rev1", 3)).await, Some(block));
        assert_eq!(cache.stats().hits, 1);

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn an_absent_block_is_a_miss_and_not_an_error() {
        let root = temp_root("absent");
        let cache = DiskCache::open(DiskCacheConfig::new(&root)).await.unwrap();

        assert!(cache.get(&key("a", "rev1", 0)).await.is_none());
        assert_eq!(cache.stats().misses, 1);
        assert_eq!(cache.stats().rejected, 0);

        std::fs::remove_dir_all(&root).ok();
    }

    /// The reason the sidecar is written last. A truncated block must never be
    /// served — for video it would not fail, it would decode as corruption.
    #[tokio::test]
    async fn a_block_that_disagrees_with_its_sidecar_is_rejected_and_removed() {
        let root = temp_root("torn");
        let cache = DiskCache::open(DiskCacheConfig::new(&root)).await.unwrap();
        let k = key("a", "rev1", 0);
        cache.put(&k, &[1_u8; 100]).await;

        // Simulate the torn write the ordering is designed to catch.
        let path = cache.block_path(&k);
        std::fs::write(&path, vec![1_u8; 40]).unwrap();

        assert!(cache.get(&k).await.is_none());
        assert_eq!(cache.stats().rejected, 1);
        assert!(!path.exists(), "invalid entry should be cleaned up");

        std::fs::remove_dir_all(&root).ok();
    }

    /// A block with no sidecar at all — the crash-between-writes case.
    #[tokio::test]
    async fn a_block_without_a_sidecar_is_a_miss() {
        let root = temp_root("nosidecar");
        let cache = DiskCache::open(DiskCacheConfig::new(&root)).await.unwrap();
        let k = key("a", "rev1", 0);
        cache.put(&k, &[1_u8; 100]).await;

        std::fs::remove_file(meta_path(&cache.block_path(&k))).unwrap();
        assert!(cache.get(&k).await.is_none());

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn the_cache_stays_inside_its_budget() {
        let root = temp_root("budget");
        let cache = DiskCache::open(DiskCacheConfig::new(&root).with_budget(300))
            .await
            .unwrap();

        for index in 0..10 {
            cache.put(&key("a", "rev1", index), &[1_u8; 100]).await;
            assert!(cache.stats().stored_bytes <= 300, "over budget at {index}");
        }
        assert_eq!(cache.stats().entries, 3);
        assert!(cache.stats().evictions > 0);

        std::fs::remove_dir_all(&root).ok();
    }

    /// Eviction must delete the files, not merely forget them, or the budget is
    /// a fiction and the disk fills up anyway.
    #[tokio::test]
    async fn eviction_removes_the_files_from_disk() {
        let root = temp_root("evictfiles");
        let cache = DiskCache::open(DiskCacheConfig::new(&root).with_budget(150))
            .await
            .unwrap();

        let first = key("a", "rev1", 0);
        cache.put(&first, &[1_u8; 100]).await;
        cache.put(&key("a", "rev1", 1), &[2_u8; 100]).await;

        let path = cache.block_path(&first);
        assert!(!path.exists(), "evicted block still on disk");
        assert!(!meta_path(&path).exists(), "evicted sidecar still on disk");

        std::fs::remove_dir_all(&root).ok();
    }

    /// The point of the cache: a second run reuses the first run's bytes.
    #[tokio::test]
    async fn a_reopened_cache_adopts_what_the_previous_run_wrote() {
        let root = temp_root("reopen");
        {
            let cache = DiskCache::open(DiskCacheConfig::new(&root)).await.unwrap();
            cache.put(&key("a", "rev1", 0), &[7_u8; 256]).await;
        }

        let cache = DiskCache::open(DiskCacheConfig::new(&root)).await.unwrap();
        assert_eq!(cache.stats().entries, 1);
        assert_eq!(cache.stats().stored_bytes, 256);
        assert_eq!(cache.get(&key("a", "rev1", 0)).await, Some(vec![7_u8; 256]));

        std::fs::remove_dir_all(&root).ok();
    }

    /// Reopening with a smaller budget must shrink the cache, not sit over it
    /// until enough new blocks arrive to trigger eviction.
    #[tokio::test]
    async fn reopening_with_a_smaller_budget_evicts_immediately() {
        let root = temp_root("shrink");
        {
            let cache = DiskCache::open(DiskCacheConfig::new(&root)).await.unwrap();
            for index in 0..5 {
                cache.put(&key("a", "rev1", index), &[1_u8; 100]).await;
            }
        }

        let cache = DiskCache::open(DiskCacheConfig::new(&root).with_budget(250))
            .await
            .unwrap();
        assert!(cache.stats().stored_bytes <= 250);

        std::fs::remove_dir_all(&root).ok();
    }

    /// Two revisions of the same file are different entries, so stale bytes are
    /// unreachable rather than merely unlikely.
    #[tokio::test]
    async fn a_new_revision_does_not_read_the_old_ones_blocks() {
        let root = temp_root("revision");
        let cache = DiskCache::open(DiskCacheConfig::new(&root)).await.unwrap();
        cache.put(&key("a", "rev1", 0), &[1_u8; 64]).await;

        assert!(cache.get(&key("a", "rev2", 0)).await.is_none());

        std::fs::remove_dir_all(&root).ok();
    }

    /// Nothing about the library should be legible from the cache directory.
    #[tokio::test]
    async fn cache_paths_carry_no_names() {
        let root = temp_root("opaque");
        let cache = DiskCache::open(DiskCacheConfig::new(&root)).await.unwrap();
        let path = cache.block_path(&key("Frieren-S01E01", "rev1", 0));
        let rendered = path.display().to_string();

        assert!(
            !rendered.contains("Frieren"),
            "path leaks content: {rendered}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A cache root that does not exist yet is the first-run case.
    #[tokio::test]
    async fn a_missing_cache_root_opens_empty() {
        let root = temp_root("firstrun");
        let cache = DiskCache::open(DiskCacheConfig::new(&root)).await.unwrap();
        assert_eq!(cache.stats().entries, 0);
    }
}
