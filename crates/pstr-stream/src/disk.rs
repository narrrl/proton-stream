//! The on-disk block cache: re-watching an episode should not re-download it.
//!
//! The cache itself lives in `proton-media-rs` (a block is synced before its
//! length sidecar, paths are `sha256(uid + revision)`, byte-budgeted LRU).
//! This wrapper keys it by [`BlockKey`], with the node UID as the file identity,
//! which is the layout earlier versions of this crate wrote: existing caches
//! are adopted as they are.

use proton_media_rs::BlockId;
pub use proton_media_rs::{DiskCacheConfig, DiskStats};

use crate::error::Result;
use crate::ring::BlockKey;

pub(crate) struct DiskCache(proton_media_rs::DiskCache);

impl DiskCache {
    /// Open the cache, adopting whatever a previous run left behind.
    pub(crate) async fn open(config: DiskCacheConfig) -> Result<Self> {
        Ok(Self(proton_media_rs::DiskCache::open(config).await?))
    }

    /// Read a block back, or `None` when it is absent or fails validation.
    pub(crate) async fn get(&self, key: &BlockKey) -> Option<Vec<u8>> {
        let file = key.uid.to_string();
        self.0.get(&id(&file, key)).await
    }

    /// Store a block. Failures are logged and swallowed.
    pub(crate) async fn put(&self, key: &BlockKey, block: &[u8]) {
        let file = key.uid.to_string();
        self.0.put(&id(&file, key), block).await
    }

    pub(crate) async fn set_budget(&self, bytes: u64) {
        self.0.set_budget(bytes).await
    }

    pub(crate) async fn clear(&self) {
        self.0.clear().await
    }

    pub(crate) fn stats(&self) -> DiskStats {
        self.0.stats()
    }
}

fn id<'a>(file: &'a str, key: &'a BlockKey) -> BlockId<'a> {
    BlockId {
        file,
        revision: &key.revision,
        index: key.index,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proton_sdk::ids::{LinkId, NodeUid, VolumeId};

    /// The node UID is the file identity, so a block cached before the move to
    /// proton-media-rs is still found.
    #[tokio::test]
    async fn blocks_are_keyed_by_node_uid_and_revision() {
        let root = std::env::temp_dir().join(format!("pstr-disk-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let uid = NodeUid::new(VolumeId::new("vol"), LinkId::new("a"));
        let key = BlockKey::new(&uid, "rev1", 2);

        let shared = proton_media_rs::DiskCache::open(DiskCacheConfig::new(&root))
            .await
            .unwrap();
        let file = uid.to_string();
        shared.put(&id(&file, &key), &[3_u8; 64]).await;
        drop(shared);

        let cache = DiskCache::open(DiskCacheConfig::new(&root)).await.unwrap();
        assert_eq!(cache.get(&key).await, Some(vec![3_u8; 64]));
        assert!(cache.get(&BlockKey::new(&uid, "rev2", 2)).await.is_none());
        std::fs::remove_dir_all(&root).ok();
    }
}
