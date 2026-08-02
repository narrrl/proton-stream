//! What is inside the file, and which of it is playing.
//!
//! mpv exposes this as `track-list`, a node property. It is read here one
//! sub-property at a time — `track-list/N/lang` and friends — rather than as a
//! whole node, because those are plain strings, ints and flags that
//! [`libmpv2::Mpv::get_property`] can already return, and a node walk would mean
//! reaching past the wrapper into `libmpv2_sys` for a shape that does not change
//! how any of this behaves.
//!
//! A missing sub-property is not an error: most tracks have no title, plenty
//! have no language, and mpv answers those with *unavailable* rather than with
//! an empty string.

use libmpv2::Mpv;

/// What a track carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackKind {
    Video,
    Audio,
    Subtitle,
}

impl TrackKind {
    /// mpv's own name for the kind, as `track-list/N/type` reports it.
    fn from_mpv(value: &str) -> Option<Self> {
        match value {
            "video" => Some(Self::Video),
            "audio" => Some(Self::Audio),
            "sub" => Some(Self::Subtitle),
            // `sub2` is the secondary subtitle slot, which this player does not
            // offer; anything else is a kind mpv grew after this was written.
            _ => None,
        }
    }

    /// The mpv property that selects a track of this kind.
    pub fn property(self) -> &'static str {
        match self {
            Self::Video => "vid",
            Self::Audio => "aid",
            Self::Subtitle => "sid",
        }
    }

    /// What to call this in a menu.
    pub fn label(self) -> &'static str {
        match self {
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::Subtitle => "Subtitles",
        }
    }
}

/// One track of a file, as mpv sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    /// mpv's id for the track — what `aid`/`sid` are set to. One-based, and
    /// unique only within a kind.
    pub id: i64,
    pub kind: TrackKind,
    /// The name the muxer gave it, if any: "Signs & Songs", "Commentary".
    pub title: Option<String>,
    /// The language tag, as it sits in the container: "eng", "jpn".
    pub language: Option<String>,
    pub codec: Option<String>,
    /// Whether this is the track currently playing.
    pub selected: bool,
    /// Whether the container marks it as the default choice.
    pub default: bool,
    /// True for a track added from outside the file — a sidecar subtitle. None
    /// of ours are, yet.
    pub external: bool,
}

impl Track {
    /// What the menu shows: language first, because that is what a viewer is
    /// choosing between, then whatever distinguishes two tracks of the same
    /// language.
    pub fn label(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(language) = &self.language {
            parts.push(language_name(language).to_string());
        }
        if let Some(title) = &self.title {
            parts.push(title.clone());
        }
        if parts.is_empty() {
            // Nothing named it. The id is at least stable and distinct, which
            // is more than "Unknown" twice over would be.
            parts.push(format!("{} {}", self.kind.label(), self.id));
        }
        let mut label = parts.join(" — ");
        if let Some(codec) = &self.codec {
            label.push_str(&format!(" [{codec}]"));
        }
        label
    }
}

/// Every track of the file mpv currently has open.
pub(crate) fn read(mpv: &Mpv) -> Vec<Track> {
    let count: i64 = mpv.get_property("track-list/count").unwrap_or(0);
    (0..count)
        .filter_map(|index| read_one(mpv, index))
        .collect()
}

fn read_one(mpv: &Mpv, index: i64) -> Option<Track> {
    let kind = TrackKind::from_mpv(&text(mpv, index, "type")?)?;
    let id: i64 = mpv
        .get_property(&format!("track-list/{index}/id"))
        .ok()
        .filter(|id| *id > 0)?;

    Some(Track {
        id,
        kind,
        title: text(mpv, index, "title"),
        language: text(mpv, index, "lang"),
        codec: text(mpv, index, "codec"),
        selected: flag(mpv, index, "selected"),
        default: flag(mpv, index, "default"),
        external: flag(mpv, index, "external"),
    })
}

/// A string sub-property, with "absent" and "present but empty" collapsed:
/// a track whose title is the empty string has no title.
fn text(mpv: &Mpv, index: i64, field: &str) -> Option<String> {
    mpv.get_property::<String>(&format!("track-list/{index}/{field}"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn flag(mpv: &Mpv, index: i64, field: &str) -> bool {
    mpv.get_property(&format!("track-list/{index}/{field}"))
        .unwrap_or(false)
}

/// A readable name for a language tag, falling back to the tag itself.
///
/// Deliberately a short list rather than a full ISO 639 table: it covers what
/// the shares this app is pointed at actually contain, and an unrecognised tag
/// still shows as something a viewer can pick between — `"por-BR"` reads fine
/// on its own.
pub fn language_name(tag: &str) -> String {
    // Container tags come in both the two- and three-letter forms, and often
    // with a region suffix: "en", "eng", "en-US", "pt-BR".
    let base = tag
        .split(['-', '_'])
        .next()
        .unwrap_or(tag)
        .to_ascii_lowercase();

    let name = match base.as_str() {
        "en" | "eng" => "English",
        "ja" | "jpn" => "Japanese",
        "de" | "ger" | "deu" => "German",
        "fr" | "fre" | "fra" => "French",
        "es" | "spa" => "Spanish",
        "it" | "ita" => "Italian",
        "pt" | "por" => "Portuguese",
        "nl" | "dut" | "nld" => "Dutch",
        "ru" | "rus" => "Russian",
        "pl" | "pol" => "Polish",
        "sv" | "swe" => "Swedish",
        "da" | "dan" => "Danish",
        "no" | "nor" => "Norwegian",
        "fi" | "fin" => "Finnish",
        "cs" | "cze" | "ces" => "Czech",
        "hu" | "hun" => "Hungarian",
        "tr" | "tur" => "Turkish",
        "ar" | "ara" => "Arabic",
        "he" | "heb" => "Hebrew",
        "hi" | "hin" => "Hindi",
        "ko" | "kor" => "Korean",
        "zh" | "chi" | "zho" => "Chinese",
        "th" | "tha" => "Thai",
        "vi" | "vie" => "Vietnamese",
        "uk" | "ukr" => "Ukrainian",
        "ro" | "rum" | "ron" => "Romanian",
        "el" | "gre" | "ell" => "Greek",
        "id" | "ind" => "Indonesian",
        "ms" | "may" | "msa" => "Malay",
        _ => return tag.to_string(),
    };

    // The region matters when there is one — Brazilian Portuguese is not
    // Portuguese to anyone choosing between the two.
    match tag.split_once(['-', '_']) {
        Some((_, region)) if !region.is_empty() => {
            format!("{name} ({})", region.to_ascii_uppercase())
        }
        _ => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(kind: TrackKind, id: i64) -> Track {
        Track {
            id,
            kind,
            title: None,
            language: None,
            codec: None,
            selected: false,
            default: false,
            external: false,
        }
    }

    #[test]
    fn a_track_with_nothing_but_an_id_still_has_a_distinct_label() {
        // Two nameless audio tracks are a real container; a menu of two
        // identical entries is not a choice.
        assert_eq!(track(TrackKind::Audio, 1).label(), "Audio 1");
        assert_eq!(track(TrackKind::Audio, 2).label(), "Audio 2");
    }

    #[test]
    fn a_labelled_track_reads_language_first_then_what_distinguishes_it() {
        let mut sub = track(TrackKind::Subtitle, 2);
        sub.language = Some("eng".into());
        sub.title = Some("Signs & Songs".into());
        sub.codec = Some("ass".into());
        assert_eq!(sub.label(), "English — Signs & Songs [ass]");
    }

    #[test]
    fn language_tags_resolve_in_both_iso_forms_and_keep_their_region() {
        assert_eq!(language_name("jpn"), "Japanese");
        assert_eq!(language_name("ja"), "Japanese");
        assert_eq!(language_name("pt-BR"), "Portuguese (BR)");
        // An unknown tag is shown as it stands rather than as "Unknown", which
        // would make two of them indistinguishable.
        assert_eq!(language_name("mis"), "mis");
    }

    #[test]
    fn only_the_kinds_the_player_can_select_are_tracks() {
        assert_eq!(TrackKind::from_mpv("audio"), Some(TrackKind::Audio));
        assert_eq!(TrackKind::from_mpv("sub"), Some(TrackKind::Subtitle));
        // The secondary subtitle slot is a separate property this player does
        // not drive; treating it as a subtitle track would put an entry in the
        // menu that `sid` cannot select.
        assert_eq!(TrackKind::from_mpv("sub2"), None);
    }
}
