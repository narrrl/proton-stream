//! Which of a file's subtitle tracks is the one to read.
//!
//! Anime releases often carry two subtitle tracks in one language: the
//! dialogue, and a "Signs & Songs" track that only translates the on-screen
//! text and the lyrics. The second is the one marked default or forced as
//! often as not, and mpv, picking by language, plays it — an episode where
//! nobody seems to speak. A language cannot tell the two apart; the track's
//! name and its forced flag can.
//!
//! Nothing here talks to a player. Every client reads the track list out of
//! mpv once a file is loaded and asks [`full_instead`] whether to switch.

use crate::language::same_language;

/// One subtitle track of the open file, as mpv lists it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SubtitleTrack {
    /// mpv's id for it, what `sid` is set to.
    pub id: i64,
    pub language: Option<String>,
    /// The name the muxer gave it: "Dialogue", "Signs & Songs".
    pub title: Option<String>,
    /// The container marks it as the default choice.
    pub default: bool,
    /// The container marks it as forced: on-screen text only, by convention.
    pub forced: bool,
    /// It is the one playing.
    pub selected: bool,
}

/// How much of what is said a track carries, as far as its name and flags say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Coverage {
    /// Signs, songs, or forced: not the dialogue.
    Partial,
    /// Nothing says either way.
    Unknown,
    /// Named as the dialogue: "Dialogue", "Full Subtitles", "SDH".
    Full,
}

/// Words in a track's name that make it the whole dialogue. They win over the
/// partial ones: "Full (with Signs)" is still the full track.
const FULL_WORDS: [&str; 6] = ["full", "dialogue", "dialog", "dialogues", "sdh", "cc"];

/// Words in a track's name that make it something other than the dialogue.
const PARTIAL_WORDS: [&str; 13] = [
    "sign", "signs", "song", "songs", "s&s", "forced", "karaoke", "lyrics", "op", "ed", "oped",
    "opening", "ending",
];

impl SubtitleTrack {
    pub fn coverage(&self) -> Coverage {
        let title = self.title.as_deref().unwrap_or_default().to_lowercase();
        let words: Vec<&str> = title
            .split(|c: char| !c.is_alphanumeric() && c != '&')
            .filter(|word| !word.is_empty())
            .collect();
        let has = |list: &[&str]| words.iter().any(|word| list.contains(word));
        if has(&FULL_WORDS) {
            Coverage::Full
        } else if self.forced || has(&PARTIAL_WORDS) {
            Coverage::Partial
        } else {
            Coverage::Unknown
        }
    }
}

/// The track to play in place of the one mpv chose, if it chose the wrong one.
///
/// Only a track in the same language as the playing one is considered — the
/// language is the viewer's choice, and this only corrects which of its tracks
/// was taken. The playing track is replaced when it is partial and the file has
/// one in its language that is not, or when nothing names it and another is
/// named as the dialogue. Among the rest, a track named as the dialogue comes
/// first, then the container's default, then the file's order. `None` leaves
/// the selection as it is, which includes subtitles being off.
pub fn full_instead(tracks: &[SubtitleTrack]) -> Option<i64> {
    let playing = tracks.iter().find(|track| track.selected)?;
    let current = playing.coverage();
    if current == Coverage::Full {
        return None;
    }
    let same = |track: &SubtitleTrack| match (&playing.language, &track.language) {
        (Some(a), Some(b)) => same_language(a, b),
        (None, None) => true,
        _ => false,
    };
    tracks
        .iter()
        .filter(|track| track.id != playing.id && same(track))
        .filter(|track| track.coverage() > current)
        // `max_by_key` keeps the last of equals; reversed, the first.
        .rev()
        .max_by_key(|track| (track.coverage(), track.default))
        .map(|track| track.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: i64, language: &str, title: &str) -> SubtitleTrack {
        SubtitleTrack {
            id,
            language: Some(language.into()),
            title: (!title.is_empty()).then(|| title.into()),
            ..SubtitleTrack::default()
        }
    }

    fn playing(mut track: SubtitleTrack) -> SubtitleTrack {
        track.selected = true;
        track
    }

    #[test]
    fn signs_and_songs_playing_gives_way_to_the_dialogue_in_its_language() {
        let tracks = [
            playing(track(1, "eng", "Signs & Songs")),
            track(2, "eng", "Dialogue"),
        ];
        assert_eq!(full_instead(&tracks), Some(2));
    }

    #[test]
    fn an_unnamed_track_gives_way_to_one_named_as_the_dialogue() {
        let tracks = [playing(track(1, "en", "")), track(2, "en", "Dialogue")];
        assert_eq!(full_instead(&tracks), Some(2));
    }

    #[test]
    fn a_forced_track_gives_way_to_an_unnamed_one_and_the_default_comes_first() {
        let mut forced = playing(track(1, "eng", "English"));
        forced.forced = true;
        let mut default = track(3, "eng", "");
        default.default = true;
        let tracks = [forced, track(2, "eng", ""), default];
        assert_eq!(full_instead(&tracks), Some(3));
    }

    #[test]
    fn equal_candidates_resolve_to_the_first_in_the_file() {
        let tracks = [
            playing(track(1, "eng", "Songs")),
            track(2, "eng", ""),
            track(3, "eng", ""),
        ];
        assert_eq!(full_instead(&tracks), Some(2));
    }

    #[test]
    fn a_track_in_another_language_is_never_switched_to() {
        let tracks = [
            playing(track(1, "eng", "Signs")),
            track(2, "ger", "Dialogue"),
        ];
        assert_eq!(full_instead(&tracks), None);
    }

    #[test]
    fn two_letter_and_three_letter_tags_are_one_language() {
        let tracks = [playing(track(1, "en", "OP/ED")), track(2, "eng", "")];
        assert_eq!(full_instead(&tracks), Some(2));
    }

    #[test]
    fn the_dialogue_playing_stays_and_subtitles_off_stay_off() {
        let tracks = [
            track(1, "eng", "Signs & Songs"),
            playing(track(2, "eng", "Dialogue")),
        ];
        assert_eq!(full_instead(&tracks), None);
        let off = [
            track(1, "eng", "Signs & Songs"),
            track(2, "eng", "Dialogue"),
        ];
        assert_eq!(full_instead(&off), None);
    }

    #[test]
    fn a_full_word_outweighs_a_partial_one_in_the_same_name() {
        assert_eq!(
            track(1, "eng", "Full (with Signs)").coverage(),
            Coverage::Full
        );
        assert_eq!(track(1, "eng", "Signs/Songs").coverage(), Coverage::Partial);
        assert_eq!(track(1, "eng", "English").coverage(), Coverage::Unknown);
        // A word inside another is not that word.
        assert_eq!(
            track(1, "eng", "Designer cut").coverage(),
            Coverage::Unknown
        );
    }

    #[test]
    fn a_partial_track_with_only_partial_company_stays() {
        let tracks = [playing(track(1, "eng", "Signs")), track(2, "eng", "Songs")];
        assert_eq!(full_instead(&tracks), None);
    }
}
