//! Pictures kept on disk between launches: Proton thumbnails and provider
//! artwork.
//!
//! Backed by `proton_media_rs::ImageCache`, shared with the other Proton apps:
//! a byte budget with least recently used eviction, entries that a crash
//! cannot leave half-written, and hashed paths so no share id, link id or URL
//! reaches the filesystem.
//!
//! Opened on first use. A store that cannot be opened turns every lookup into
//! a miss, which costs a download and nothing else.

use std::path::PathBuf;

use proton_media_rs::{ImageCache, ImageKey};
use tokio::sync::OnceCell;

/// One kind of picture, in its own directory.
pub struct PictureStore {
    root: PathBuf,
    /// The flat directory earlier versions used for the same pictures. Deleted
    /// when the store is first opened; nothing in it is read.
    legacy: Option<PathBuf>,
    cache: OnceCell<Option<ImageCache>>,
}

impl PictureStore {
    pub fn new(root: PathBuf, legacy: Option<PathBuf>) -> Self {
        Self {
            root,
            legacy,
            cache: OnceCell::new(),
        }
    }

    async fn cache(&self) -> Option<&ImageCache> {
        self.cache
            .get_or_init(|| async {
                if let Some(legacy) = &self.legacy {
                    let _ = tokio::fs::remove_dir_all(legacy).await;
                }
                ImageCache::open(&self.root)
                    .await
                    .inspect_err(|error| {
                        tracing::warn!("picture cache {} unavailable: {error}", self.root.display())
                    })
                    .ok()
            })
            .await
            .as_ref()
    }

    /// The kept bytes for `source` in `variant`, if any.
    pub async fn get(&self, source: &str, variant: &str) -> Option<Vec<u8>> {
        self.cache().await?.get(&key(source, variant)).await
    }

    /// Keep `bytes`. Best effort: a failed write only means a later download.
    pub async fn put(&self, source: &str, variant: &str, bytes: &[u8]) {
        if let Some(cache) = self.cache().await {
            cache.put(&key(source, variant), bytes).await;
        }
    }

    /// The file holding `source` in `variant`, when it is kept and the store is
    /// already open. For consumers that need a path, such as a media-session
    /// cover.
    pub fn file(&self, source: &str, variant: &str) -> Option<PathBuf> {
        let cache = self.cache.get()?.as_ref()?;
        let path = cache.path(&key(source, variant));
        path.is_file().then_some(path)
    }

    /// Delete every kept picture.
    pub async fn clear(&self) {
        if let Some(cache) = self.cache().await {
            cache.clear().await;
        }
    }
}

/// Pictures do not carry a version: a re-encoded file keeps its link, and a
/// slightly stale frame of the right episode beats a download on every launch.
/// Clearing the store is what invalidates it.
fn key<'a>(source: &'a str, variant: &'a str) -> ImageKey<'a> {
    ImageKey::new(source, "", variant)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("pstr-pictures-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    #[tokio::test]
    async fn a_kept_picture_survives_a_new_store_and_the_old_directory_goes() {
        let base = temp("reopen");
        let legacy = base.join("thumbs");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("share-link.bin"), b"old").unwrap();
        let root = base.join("thumbnails");

        let first = PictureStore::new(root.clone(), Some(legacy.clone()));
        assert_eq!(first.get("share-link", "card").await, None);
        assert!(!legacy.exists(), "the flat directory is removed on open");
        first.put("share-link", "card", b"jpeg").await;

        let second = PictureStore::new(root.clone(), Some(legacy));
        assert_eq!(second.file("share-link", "card"), None, "not open yet");
        assert_eq!(
            second.get("share-link", "card").await.as_deref(),
            Some(&b"jpeg"[..])
        );
        assert!(second.file("share-link", "card").is_some());
        assert_eq!(second.get("share-link", "logo").await, None);

        second.clear().await;
        assert_eq!(second.get("share-link", "card").await, None);
        let _ = std::fs::remove_dir_all(&base);
    }
}
