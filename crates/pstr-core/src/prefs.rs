//! Playback preferences that outlive one file.
//!
//! Volume, mute, and which language of audio and subtitles to prefer. All four
//! are things a viewer sets once and expects to still hold for the next
//! episode, the next film and the next launch — a player that starts every file
//! at full volume in the container's default language is one that has to be
//! corrected every time.
//!
//! Stored beside the share list and written the same way: atomically, and never
//! overwritten when it will not parse. Losing this file costs nothing more than
//! a re-set, so unlike `shares.json` an unreadable one is reported and *stepped
//! over* rather than being allowed to stop anything.

use crate::config::{AppDirs, read_json, write_json};
use crate::error::Result;

/// The loudest this app plays. Matches `pstr_player::MAX_VOLUME`, which is not
/// referenced here because `pstr-core` does not depend on the player.
const MAX_VOLUME: f64 = 100.0;

/// How playback should sound, and in which language.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
// A file written by an older version, or hand-edited, is missing fields rather
// than invalid: every one of them has a sensible default.
#[serde(default)]
pub struct PlaybackPrefs {
    /// 0–100.
    pub volume: f64,
    pub muted: bool,
    /// The language tag of the audio track to prefer — "jpn", "eng". `None`
    /// leaves the choice to the container's default.
    pub audio_language: Option<String>,
    pub subtitle_language: Option<String>,
    /// Whether subtitles are wanted at all.
    ///
    /// Separate from the language: "off" is a decision, and an absent language
    /// cannot express it — that means "no preference", which is what a viewer
    /// who has never touched the menu has.
    pub subtitles: bool,
    /// Whether reaching the end of an episode starts the next one.
    ///
    /// On by default, because that is what every service this is modelled on
    /// does and what a season of anything is watched like. Off is one checkbox
    /// away for someone who falls asleep to it.
    pub autoplay_next: bool,
}

impl Default for PlaybackPrefs {
    fn default() -> Self {
        Self {
            volume: MAX_VOLUME,
            muted: false,
            audio_language: None,
            subtitle_language: None,
            subtitles: true,
            autoplay_next: true,
        }
    }
}

impl PlaybackPrefs {
    /// The same preferences with anything out of range brought back into it.
    ///
    /// A hand-edited or older file can hold a volume of 900 or of NaN, and both
    /// would otherwise reach mpv.
    pub fn sanitized(mut self) -> Self {
        if !self.volume.is_finite() {
            self.volume = MAX_VOLUME;
        }
        self.volume = self.volume.clamp(0.0, MAX_VOLUME);
        self.audio_language = self.audio_language.filter(|tag| !tag.trim().is_empty());
        self.subtitle_language = self.subtitle_language.filter(|tag| !tag.trim().is_empty());
        self
    }
}

fn prefs_file(dirs: &AppDirs) -> std::path::PathBuf {
    dirs.config.join("playback.json")
}

/// The stored preferences, or the defaults on a first run.
pub fn load(dirs: &AppDirs) -> Result<PlaybackPrefs> {
    Ok(read_json::<PlaybackPrefs>(&prefs_file(dirs))?
        .unwrap_or_default()
        .sanitized())
}

/// Write the preferences.
pub fn save(dirs: &AppDirs, prefs: &PlaybackPrefs) -> Result<()> {
    write_json(&prefs_file(dirs), prefs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(root: &std::path::Path) -> AppDirs {
        AppDirs {
            config: root.to_path_buf(),
            data: root.to_path_buf(),
            cache: root.to_path_buf(),
        }
    }

    #[test]
    fn a_first_run_gets_full_volume_and_no_language_preference() {
        let prefs = PlaybackPrefs::default();
        assert_eq!(prefs.volume, MAX_VOLUME);
        assert!(!prefs.muted);
        assert!(prefs.subtitles);
        assert_eq!(prefs.audio_language, None);
    }

    #[test]
    fn what_was_saved_is_what_loads() {
        let temp = std::env::temp_dir().join(format!("pstr-prefs-{}", std::process::id()));
        std::fs::create_dir_all(&temp).unwrap();
        let dirs = dirs(&temp);

        let prefs = PlaybackPrefs {
            volume: 42.0,
            muted: true,
            audio_language: Some("jpn".into()),
            subtitle_language: Some("eng".into()),
            subtitles: true,
            autoplay_next: false,
        };
        save(&dirs, &prefs).unwrap();
        assert_eq!(load(&dirs).unwrap(), prefs);

        std::fs::remove_dir_all(&temp).ok();
    }

    #[test]
    fn a_volume_no_player_could_honour_is_clamped_rather_than_passed_on() {
        let prefs = PlaybackPrefs {
            volume: 900.0,
            ..PlaybackPrefs::default()
        }
        .sanitized();
        assert_eq!(prefs.volume, MAX_VOLUME);

        let prefs = PlaybackPrefs {
            volume: f64::NAN,
            ..PlaybackPrefs::default()
        }
        .sanitized();
        // NaN clamps to NaN, which reaches mpv as a volume of nothing at all.
        assert_eq!(prefs.volume, MAX_VOLUME);
    }

    #[test]
    fn a_language_set_to_blank_is_no_preference_rather_than_a_language_named_nothing() {
        let prefs = PlaybackPrefs {
            audio_language: Some("  ".into()),
            ..PlaybackPrefs::default()
        }
        .sanitized();
        assert_eq!(prefs.audio_language, None);
    }
}
