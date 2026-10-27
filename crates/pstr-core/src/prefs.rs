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

/// The range of playback rates offered. Past these, audio stops being speech
/// and video stops being watchable, and mpv's own resampler gives up first.
const MIN_SPEED: f64 = 0.25;
const MAX_SPEED: f64 = 4.0;

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
    /// Whether an opening or ending is seeked past rather than offered.
    ///
    /// Off by default, and deliberately: chapter names are a guess about a file
    /// somebody else muxed, and a wrong guess that only greys out a button
    /// costs nothing while a wrong guess that seeks costs the scene.
    pub auto_skip: bool,
    /// How fast playback runs, 1.0 being the file's own rate.
    ///
    /// Kept across episodes deliberately: someone who watches at 1.25 watches
    /// everything at 1.25, and a speed that reset every episode would be a
    /// setting nobody could use.
    pub speed: f64,
    /// How plain subtitles look. Kept with the rest because it is the same
    /// kind of choice: made once, for every file after.
    pub subtitle_style: SubtitleStyle,
}

/// How subtitles are drawn.
///
/// Every default is mpv's own, so a viewer who never opens the section sees
/// what any mpv shows. Plain (SRT) tracks take all of it. A styled (ASS) track
/// was typeset by somebody, its signs placed and coloured by hand, and painting
/// over that undoes the work — so by default it takes none of this, and moves
/// only to stay clear of the player's controls. All of it reaches a styled
/// track only with [`Self::restyle_styled`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SubtitleStyle {
    /// Relative to mpv's own size, which is itself relative to the video's
    /// height — so one setting reads the same on a phone and a television.
    pub scale: f64,
    /// A font family name. `None` is mpv's `sans-serif`. A name this device
    /// does not have falls back the same way, which is what makes it safe to
    /// sync.
    pub font: Option<String>,
    pub bold: bool,
    pub italic: bool,
    pub colour: Colour,
    pub backdrop: SubtitleBackdrop,
    pub outline_colour: Colour,
    /// In mpv's scaled pixels, as `sub-outline-size`.
    pub outline_size: f64,
    /// The box's colour, its alpha the box's opacity. Only with
    /// [`SubtitleBackdrop::Box`].
    pub box_colour: Colour,
    pub shadow_colour: Colour,
    /// How far the shadow falls; 0 is none, which is mpv's default.
    pub shadow_offset: f64,
    /// How soft the edges are, 0–20.
    pub blur: f64,
    /// Where the line sits, as a percentage of the picture's height from the
    /// top: 100 is the bottom edge, mpv's usual place.
    pub position: f64,
    /// Whether styled (ASS) tracks are drawn in this style too, size and
    /// position included. Off by default: it turns signs and karaoke into
    /// dialogue-looking text.
    pub restyle_styled: bool,
}

impl SubtitleStyle {
    /// The family a style with no font of its own is set in: mpv's default,
    /// which fontconfig resolves to the system's sans-serif face.
    pub const DEFAULT_FONT: &str = "sans-serif";
    pub const MIN_SCALE: f64 = 0.5;
    pub const MAX_SCALE: f64 = 2.5;
    pub const MAX_OUTLINE: f64 = 10.0;
    pub const MAX_SHADOW: f64 = 10.0;
    pub const MAX_BLUR: f64 = 20.0;

    /// The same style with every number brought back into range.
    ///
    /// A hand-edited or synced file can hold anything, and mpv refuses an
    /// out-of-range value outright — which would cost the rest of the style.
    pub fn sanitized(mut self) -> Self {
        let defaults = Self::default();
        let clamp = |value: f64, default: f64, min: f64, max: f64| {
            if value.is_finite() {
                value.clamp(min, max)
            } else {
                default
            }
        };
        self.scale = clamp(self.scale, defaults.scale, Self::MIN_SCALE, Self::MAX_SCALE);
        self.outline_size = clamp(
            self.outline_size,
            defaults.outline_size,
            0.0,
            Self::MAX_OUTLINE,
        );
        self.shadow_offset = clamp(
            self.shadow_offset,
            defaults.shadow_offset,
            0.0,
            Self::MAX_SHADOW,
        );
        self.blur = clamp(self.blur, defaults.blur, 0.0, Self::MAX_BLUR);
        self.position = clamp(self.position, defaults.position, 0.0, 100.0);
        self.font = self
            .font
            .map(|font| font.trim().to_owned())
            .filter(|font| !font.is_empty());
        self
    }
}

impl Default for SubtitleStyle {
    fn default() -> Self {
        Self {
            scale: 1.0,
            font: None,
            bold: false,
            italic: false,
            colour: Colour::rgba(0xFF, 0xFF, 0xFF, 0xFF),
            backdrop: SubtitleBackdrop::Outline,
            outline_colour: Colour::rgba(0, 0, 0, 0xFF),
            outline_size: 1.65,
            // About seventy percent black.
            box_colour: Colour::rgba(0, 0, 0, 0xB0),
            shadow_colour: Colour::rgba(0, 0, 0, 0xAF),
            shadow_offset: 0.0,
            blur: 0.0,
            position: 100.0,
            restyle_styled: false,
        }
    }
}

/// What keeps the text readable against the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubtitleBackdrop {
    /// An outline round each letter: mpv's own, and the default.
    #[default]
    Outline,
    /// A box behind each line. Uglier, and the only thing that holds up on
    /// snow, sky and white walls.
    Box,
}

impl SubtitleBackdrop {
    pub const ALL: [Self; 2] = [Self::Outline, Self::Box];

    pub fn label(self) -> &'static str {
        match self {
            Self::Outline => "Outline",
            Self::Box => "Box",
        }
    }
}

/// A colour with opacity, written `#RRGGBBAA` the way CSS writes one.
///
/// Not mpv's order: mpv reads `#AARRGGBB`, which nobody hand-editing a file
/// would guess. [`Self::to_mpv`] does the translation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Colour {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Colour {
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// `#RRGGBB` or `#RRGGBBAA`, the `#` optional. Six digits are opaque.
    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.trim().strip_prefix('#').unwrap_or(text.trim());
        if !(hex.len() == 6 || hex.len() == 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let byte = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
        let a = if hex.len() == 8 { byte(6)? } else { 0xFF };
        Some(Self::rgba(byte(0)?, byte(2)?, byte(4)?, a))
    }

    /// As mpv's colour options take it.
    pub fn to_mpv(self) -> String {
        format!("#{:02X}{:02X}{:02X}{:02X}", self.a, self.r, self.g, self.b)
    }

    pub fn to_array(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }
}

impl From<[u8; 4]> for Colour {
    fn from([r, g, b, a]: [u8; 4]) -> Self {
        Self::rgba(r, g, b, a)
    }
}

impl std::fmt::Display for Colour {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "#{:02X}{:02X}{:02X}{:02X}",
            self.r, self.g, self.b, self.a
        )
    }
}

impl From<Colour> for String {
    fn from(colour: Colour) -> Self {
        colour.to_string()
    }
}

impl TryFrom<String> for Colour {
    type Error = String;

    fn try_from(text: String) -> std::result::Result<Self, Self::Error> {
        Self::parse(&text).ok_or_else(|| format!("not a colour: {text:?}"))
    }
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
            auto_skip: false,
            speed: 1.0,
            subtitle_style: SubtitleStyle::default(),
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
        if !self.speed.is_finite() {
            self.speed = 1.0;
        }
        self.speed = self.speed.clamp(MIN_SPEED, MAX_SPEED);
        self.audio_language = self.audio_language.filter(|tag| !tag.trim().is_empty());
        self.subtitle_language = self.subtitle_language.filter(|tag| !tag.trim().is_empty());
        self.subtitle_style = self.subtitle_style.sanitized();
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
            auto_skip: true,
            speed: 1.5,
            subtitle_style: SubtitleStyle {
                scale: 1.5,
                font: Some("Noto Sans".into()),
                italic: true,
                colour: Colour::rgba(0xFF, 0xE6, 0x00, 0xFF),
                backdrop: SubtitleBackdrop::Box,
                box_colour: Colour::rgba(0x10, 0x10, 0x10, 0x80),
                restyle_styled: true,
                ..SubtitleStyle::default()
            },
        };
        save(&dirs, &prefs).unwrap();
        assert_eq!(load(&dirs).unwrap(), prefs);

        std::fs::remove_dir_all(&temp).ok();
    }

    #[test]
    fn a_file_from_before_subtitle_styles_loads_with_the_default_style() {
        let prefs: PlaybackPrefs =
            serde_json::from_str(r#"{"volume": 60.0, "subtitles": true, "speed": 1.0}"#).unwrap();
        assert_eq!(prefs.subtitle_style, SubtitleStyle::default());
        assert_eq!(prefs.volume, 60.0);
    }

    #[test]
    fn a_colour_is_written_rgba_and_handed_to_mpv_as_argb() {
        let colour = Colour::parse("#FFE60080").unwrap();
        assert_eq!(colour, Colour::rgba(0xFF, 0xE6, 0x00, 0x80));
        assert_eq!(colour.to_string(), "#FFE60080");
        assert_eq!(colour.to_mpv(), "#80FFE600");
        assert_eq!(
            Colour::parse("ffe600"),
            Some(Colour::rgba(0xFF, 0xE6, 0, 0xFF))
        );
        assert_eq!(Colour::parse("#FFE6"), None);
        assert_eq!(Colour::parse("#GGGGGG"), None);
    }

    #[test]
    fn a_subtitle_style_out_of_range_is_brought_back_into_it() {
        let style = SubtitleStyle {
            scale: 40.0,
            outline_size: -3.0,
            blur: f64::NAN,
            position: 400.0,
            font: Some("   ".into()),
            ..SubtitleStyle::default()
        }
        .sanitized();
        assert_eq!(style.scale, SubtitleStyle::MAX_SCALE);
        assert_eq!(style.outline_size, 0.0);
        assert_eq!(style.blur, 0.0);
        assert_eq!(style.position, 100.0);
        assert_eq!(style.font, None);
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
