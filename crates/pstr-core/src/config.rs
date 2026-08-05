//! Where the app keeps its state, and how it writes there safely.
//!
//! Two rules carried over from `proton-drive-linux`'s config handling, both
//! learned the hard way:
//!
//! 1. **Write atomically.** Temp file → `sync_all` → rename → fsync the parent
//!    directory. A crash mid-write then leaves either the old file or the new
//!    one, never a truncated one.
//! 2. **Never overwrite a config that would not parse.** An unreadable file is
//!    far more likely to be a bug of ours, or a half-finished hand edit, than
//!    something to be silently replaced — and replacing it destroys the only
//!    record of the user's shares.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// The app's directories, resolved once.
#[derive(Debug, Clone)]
pub struct AppDirs {
    /// Config: the share list. Small, hand-editable, worth backing up.
    pub config: PathBuf,
    /// State: the catalog database and metadata cache. Rebuildable.
    pub data: PathBuf,
    /// Cache: downloaded content blocks and poster images. Disposable — the app
    /// must work correctly after this is deleted while it is not running.
    pub cache: PathBuf,
}

impl AppDirs {
    /// Use directories supplied by a platform host.
    ///
    /// Android owns its application directories and exposes them through
    /// `Context`; resolving them through desktop conventions would put state in
    /// a path that does not exist inside the application sandbox.
    pub fn from_paths(
        config: impl Into<PathBuf>,
        data: impl Into<PathBuf>,
        cache: impl Into<PathBuf>,
    ) -> Result<Self> {
        let dirs = Self {
            config: config.into(),
            data: data.into(),
            cache: cache.into(),
        };
        dirs.create()?;
        Ok(dirs)
    }

    /// Resolve the platform directories and create them.
    pub fn ensure() -> Result<Self> {
        let dirs =
            directories::ProjectDirs::from("io", "narl", "proton-stream").ok_or_else(|| {
                Error::Config("no home directory to resolve app directories from".into())
            })?;

        let dirs = Self {
            config: dirs.config_dir().to_path_buf(),
            data: dirs.data_dir().to_path_buf(),
            cache: dirs.cache_dir().to_path_buf(),
        };

        dirs.create()?;
        Ok(dirs)
    }

    fn create(&self) -> Result<()> {
        for path in [&self.config, &self.data, &self.cache] {
            fs::create_dir_all(path)?;
        }
        Ok(())
    }

    /// The share list.
    pub fn shares_file(&self) -> PathBuf {
        self.config.join("shares.json")
    }

    /// The catalog database.
    pub fn catalog_db(&self) -> PathBuf {
        self.data.join("catalog.db")
    }

    /// The content-block cache root.
    pub fn block_cache(&self) -> PathBuf {
        self.cache.join("blocks")
    }

    /// Complete files explicitly kept for disconnected playback.
    pub fn offline_content(&self) -> PathBuf {
        self.data.join("offline")
    }

    /// Opaque path for one complete offline revision. No media name or share
    /// secret reaches the filesystem.
    pub fn offline_file(&self, share_id: &str, link_id: &str, revision: &str) -> PathBuf {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        hash.update(share_id.as_bytes());
        hash.update([0]);
        hash.update(link_id.as_bytes());
        hash.update([0]);
        hash.update(revision.as_bytes());
        let digest = hash.finalize();
        let name: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        self.offline_content().join(format!("{name}.media"))
    }

    /// Where decrypted poster thumbnails are kept.
    ///
    /// Separate from the block cache because it is cheap, tiny and wanted on
    /// every render, where a block is expensive, large and wanted once. Clearing
    /// either must leave the app correct.
    pub fn thumbnail_cache(&self) -> PathBuf {
        self.cache.join("thumbs")
    }

    /// Where artwork downloaded from a metadata provider is kept.
    ///
    /// Separate from the Proton thumbnails next to it because the two are
    /// invalidated by different things — a recrawl for one, a provider switch
    /// for the other — and because deleting all of one must not touch the other.
    pub fn poster_cache(&self) -> PathBuf {
        self.cache.join("posters")
    }
}

/// Read and deserialize a JSON config, or `None` when it does not exist yet.
///
/// A file that exists but will not parse is an error, never a `None` — see the
/// module note on why that distinction is load-bearing.
pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };

    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| Error::Config(format!("{} is not valid config: {e}", path.display())))
}

/// Serialize and write a JSON config atomically.
pub fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| Error::Config(format!("serialize config: {e}")))?;

    let parent = path
        .parent()
        .ok_or_else(|| Error::Config(format!("{} has no parent directory", path.display())))?;
    fs::create_dir_all(parent)?;

    // Named for the target so two concurrent writers of *different* configs
    // cannot collide on one temp path.
    let temp = parent.join(format!(
        ".{}.tmp",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "config".into())
    ));

    {
        let mut file = fs::File::create(&temp)?;
        file.write_all(text.as_bytes())?;
        // Before the rename, so the rename cannot publish an empty file.
        file.sync_all()?;
    }

    fs::rename(&temp, path)?;

    // The rename itself needs durability, or a crash can lose the whole file
    // even though its contents were synced.
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Sample {
        name: String,
        count: u32,
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pstr-config-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn a_config_round_trips_through_an_atomic_write() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("sample.json");
        let value = Sample {
            name: "anime".into(),
            count: 42,
        };

        write_json(&path, &value).expect("write");
        let read: Sample = read_json(&path).expect("read").expect("present");
        assert_eq!(read, value);

        fs::remove_dir_all(&dir).ok();
    }

    /// A missing config is the first-run case, not a failure.
    #[test]
    fn a_missing_config_reads_as_none() {
        let dir = temp_dir("missing");
        let read: Option<Sample> = read_json(&dir.join("absent.json")).expect("read");
        assert!(read.is_none());
        fs::remove_dir_all(&dir).ok();
    }

    /// The distinction that protects the share list: unparseable is an error,
    /// so a caller doing read-modify-write cannot mistake it for "empty" and
    /// overwrite the user's shares with nothing.
    #[test]
    fn an_unparseable_config_is_an_error_and_not_an_empty_one() {
        let dir = temp_dir("corrupt");
        let path = dir.join("broken.json");
        fs::write(&path, b"{ this is not json").expect("write garbage");

        let read: Result<Option<Sample>> = read_json(&path);
        assert!(read.is_err(), "must refuse rather than report absence");

        // And the bad file is still there to be inspected or repaired.
        assert!(path.exists());
        fs::remove_dir_all(&dir).ok();
    }

    /// The temp file must not survive a successful write.
    #[test]
    fn an_atomic_write_leaves_no_temp_file_behind() {
        let dir = temp_dir("notemp");
        let path = dir.join("sample.json");
        write_json(
            &path,
            &Sample {
                name: "x".into(),
                count: 1,
            },
        )
        .expect("write");

        let leftovers: Vec<_> = fs::read_dir(&dir)
            .expect("read dir")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");

        fs::remove_dir_all(&dir).ok();
    }
}
