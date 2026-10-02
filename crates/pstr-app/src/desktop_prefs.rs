//! Settings only the desktop app has.
//!
//! Kept out of [`pstr_core::prefs::PlaybackPrefs`] on purpose: that one is
//! shared with Android through a bridge record, and a field Android neither
//! shows nor keeps would be reset to its default every time Android saved.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// What the settings page offers for the streaming cache, in GiB.
pub const CACHE_CHOICES: [u32; 5] = [1, 2, 4, 8, 16];

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
// A file from an older version, or edited by hand, missing a field: the rest
// still counts, and the missing one takes its default.
#[serde(default)]
pub struct DesktopPrefs {
    /// Let mpv decode on the GPU. Off is for a driver that shows green frames
    /// or nothing; it costs CPU, not correctness.
    pub hardware_decoding: bool,
    /// How much decrypted video the disk cache may keep.
    pub cache_gib: u32,
}

impl Default for DesktopPrefs {
    fn default() -> Self {
        Self {
            hardware_decoding: true,
            cache_gib: 4,
        }
    }
}

impl DesktopPrefs {
    pub fn cache_bytes(&self) -> u64 {
        u64::from(self.cache_gib.clamp(1, 64)) << 30
    }
}

/// The prefs and where they live.
pub struct DesktopPrefsFile {
    path: PathBuf,
    /// False when the file was there and would not parse. Such a file is never
    /// overwritten — it is far more likely a bug than something to replace —
    /// so changes then last only for the session.
    writable: bool,
}

impl DesktopPrefsFile {
    pub fn load(path: PathBuf) -> (Self, DesktopPrefs) {
        match pstr_core::config::read_json::<DesktopPrefs>(&path) {
            Ok(prefs) => (
                Self {
                    path,
                    writable: true,
                },
                prefs.unwrap_or_default(),
            ),
            Err(error) => {
                tracing::warn!("read the desktop settings: {error}");
                (
                    Self {
                        path,
                        writable: false,
                    },
                    DesktopPrefs::default(),
                )
            }
        }
    }

    pub fn save(&self, prefs: &DesktopPrefs) -> pstr_core::Result<()> {
        if !self.writable {
            tracing::warn!(
                "not saving the desktop settings over {}, which would not parse",
                self.path.display()
            );
            return Ok(());
        }
        pstr_core::config::write_json(&self.path, prefs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_missing_a_field_keeps_the_rest() {
        let prefs: DesktopPrefs = serde_json::from_str(r#"{ "cache_gib": 8 }"#).unwrap();
        assert_eq!(prefs.cache_gib, 8);
        assert!(prefs.hardware_decoding);
    }

    #[test]
    fn an_unparseable_file_is_never_overwritten() {
        let path = std::env::temp_dir().join(format!("pstr-desktop-{}.json", std::process::id()));
        std::fs::write(&path, "{ not json").unwrap();
        let (file, prefs) = DesktopPrefsFile::load(path.clone());
        assert_eq!(prefs, DesktopPrefs::default());
        file.save(&DesktopPrefs::default()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
        let _ = std::fs::remove_file(path);
    }
}
