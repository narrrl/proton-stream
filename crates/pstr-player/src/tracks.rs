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
use pstr_core::subtitles::{SubtitleTrack, full_instead};

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
    /// Whether the container marks it as forced: a subtitle track of the
    /// on-screen text alone.
    pub forced: bool,
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

/// The track of `kind` that matches a choice remembered from another file.
///
/// A title outranks a language: two English subtitle tracks — "Signs & Songs"
/// and "Full Subtitles" — share a language and differ only by name, and the
/// name is what was picked. A title in the wrong language is not a match, since
/// "Full Subtitles" in German is not what someone reading English chose. Without
/// a title match, a track in the language wins, the container's default first.
/// `None` when nothing matches, which leaves the current selection standing.
pub fn pick_track(
    tracks: &[Track],
    kind: TrackKind,
    language: Option<&str>,
    title: Option<&str>,
) -> Option<i64> {
    let in_language = |track: &Track| match (language, track.language.as_deref()) {
        (None, _) => true,
        (Some(wanted), Some(have)) => same_language(wanted, have),
        (Some(_), None) => false,
    };
    let of_kind = || tracks.iter().filter(move |track| track.kind == kind);

    if let Some(title) = title
        && let Some(track) = of_kind().find(|track| {
            in_language(track)
                && track
                    .title
                    .as_deref()
                    .is_some_and(|have| have.eq_ignore_ascii_case(title))
        })
    {
        return Some(track.id);
    }

    language?;
    let mut matching = of_kind().filter(|track| in_language(track));
    let first = matching.next()?;
    Some(
        std::iter::once(first)
            .chain(matching)
            .find(|track| track.default)
            .unwrap_or(first)
            .id,
    )
}

/// The dialogue track to play in place of the subtitle track mpv chose, when
/// mpv chose signs and songs; see [`pstr_core::subtitles::full_instead`].
pub fn full_subtitles_instead(tracks: &[Track]) -> Option<i64> {
    let subtitles: Vec<_> = tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Subtitle)
        .map(|track| SubtitleTrack {
            id: track.id,
            language: track.language.clone(),
            title: track.title.clone(),
            default: track.default,
            forced: track.forced,
            selected: track.selected,
        })
        .collect();
    full_instead(&subtitles)
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
        forced: flag(mpv, index, "forced"),
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

pub use pstr_core::language::{LANGUAGES, language_name, same_language};

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
            forced: false,
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

    fn named(kind: TrackKind, id: i64, language: Option<&str>, title: Option<&str>) -> Track {
        Track {
            language: language.map(Into::into),
            title: title.map(Into::into),
            ..track(kind, id)
        }
    }

    #[test]
    fn a_remembered_title_picks_between_tracks_of_one_language() {
        let tracks = [
            named(TrackKind::Subtitle, 1, Some("eng"), Some("Signs & Songs")),
            named(TrackKind::Subtitle, 2, Some("eng"), Some("Full Subtitles")),
        ];
        let mut signs = tracks[0].clone();
        signs.default = true;
        let tracks = [signs, tracks[1].clone()];
        assert_eq!(
            pick_track(
                &tracks,
                TrackKind::Subtitle,
                Some("eng"),
                Some("full subtitles")
            ),
            Some(2)
        );
    }

    #[test]
    fn a_title_in_another_language_is_not_the_one_that_was_chosen() {
        let tracks = [
            named(TrackKind::Subtitle, 1, Some("ger"), Some("Full Subtitles")),
            named(TrackKind::Subtitle, 2, Some("en"), Some("Dialogue")),
        ];
        // Falls back to the language, and "en" is the same language as "eng".
        assert_eq!(
            pick_track(
                &tracks,
                TrackKind::Subtitle,
                Some("eng"),
                Some("Full Subtitles")
            ),
            Some(2)
        );
    }

    #[test]
    fn an_untagged_track_is_found_by_its_title_alone() {
        let tracks = [
            named(TrackKind::Audio, 1, None, Some("Stereo")),
            named(TrackKind::Audio, 2, None, Some("Commentary")),
        ];
        assert_eq!(
            pick_track(&tracks, TrackKind::Audio, None, Some("Commentary")),
            Some(2)
        );
    }

    #[test]
    fn a_language_match_prefers_the_default_track_and_nothing_matching_is_none() {
        let mut second = named(TrackKind::Audio, 2, Some("jpn"), None);
        second.default = true;
        let tracks = [
            named(TrackKind::Audio, 1, Some("jpn"), Some("Commentary")),
            second,
            named(TrackKind::Subtitle, 3, Some("jpn"), None),
        ];
        assert_eq!(
            pick_track(&tracks, TrackKind::Audio, Some("ja"), None),
            Some(2)
        );
        assert_eq!(
            pick_track(&tracks, TrackKind::Audio, Some("fre"), None),
            None
        );
        assert_eq!(
            pick_track(&tracks, TrackKind::Audio, None, Some("Stereo")),
            None
        );
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
