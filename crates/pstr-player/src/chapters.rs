//! The chapters a file was muxed with, and what they are for.
//!
//! Read the same way as the track list — `chapter-list/N/title`, one
//! sub-property at a time — for the same reason: those are plain strings and
//! doubles, and a node walk would buy nothing.
//!
//! Anime releases mark their opening and ending as chapters, which is the whole
//! reason this exists: with them a player can offer to skip the ninety seconds
//! everyone skips, and without them it cannot.

use libmpv2::Mpv;

/// One chapter of the file.
#[derive(Debug, Clone, PartialEq)]
pub struct Chapter {
    /// Position in the list, which is what mpv's `chapter` property is set to.
    pub index: i64,
    /// The name the muxer gave it, if any.
    pub title: Option<String>,
    /// Where it starts, in seconds.
    pub start: f64,
}

impl Chapter {
    /// What to show in a list. A nameless chapter is still worth an entry —
    /// it is a place in the file — so it gets its number.
    pub fn label(&self) -> String {
        match &self.title {
            Some(title) => title.clone(),
            None => format!("Chapter {}", self.index + 1),
        }
    }

    /// What this chapter is, from its name alone.
    ///
    /// Prefer [`roles`], which resolves the names that only mean one thing *in
    /// context* — a chapter called `Intro` is an opening in one release and the
    /// first ten minutes of the story in another.
    pub fn role(&self) -> ChapterRole {
        match Claim::of(self.title.as_deref().unwrap_or_default()) {
            Claim::Opening => ChapterRole::Opening,
            Claim::Ending => ChapterRole::Ending,
            Claim::Preview => ChapterRole::Preview,
            // Unresolved without the rest of the file: assume it is content,
            // which is the reading that never skips anything.
            _ => ChapterRole::Content,
        }
    }
}

/// The kinds of chapter worth treating differently from "part of the episode".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChapterRole {
    /// The opening titles.
    Opening,
    /// The ending titles.
    Ending,
    /// A preview of the next episode, after the ending.
    Preview,
    /// Anything else, including everything unnamed.
    Content,
}

impl ChapterRole {
    /// What a button offering to skip this should say. `None` for a chapter
    /// nobody wants skipped.
    pub fn skip_label(self) -> Option<&'static str> {
        match self {
            Self::Opening => Some("Skip opening"),
            Self::Ending => Some("Skip ending"),
            Self::Preview => Some("Skip preview"),
            Self::Content => None,
        }
    }

    /// Whether this is something the file could end on without the viewer
    /// missing anything: credits and a trailer for next week.
    ///
    /// An *opening* is not, even though it is skippable — a file does not end
    /// with one, and treating it as tail would make a mis-tagged chapter
    /// auto-advance out of the episode.
    pub fn ends_the_episode(self) -> bool {
        matches!(self, Self::Ending | Self::Preview)
    }
}

/// What a chapter's *name* claims, before the rest of the file is consulted.
///
/// The two `Maybe` variants are the whole reason this is separate from
/// [`ChapterRole`]. `Intro` is the opening in one release and, in the first
/// episode of Oshi no Ko, a ten-minute introduction to the series that a viewer
/// must not be skipped out of. `Credits` and `Cast` are the ending in most
/// files and a mid-episode scene in a few. Neither can be settled by the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Claim {
    Opening,
    Ending,
    Preview,
    /// Reads like an opening, but only if the file has no real one and it is
    /// the length of a theme song.
    MaybeOpening,
    /// Reads like credits, but only near the end of the file or at theme
    /// length.
    MaybeEnding,
    Content,
}

/// How long an opening or ending theme runs.
///
/// Anime themes are ninety seconds by convention and rarely stray far. This is
/// the corroboration for a name that could go either way: a chapter called
/// `Intro` that runs eleven minutes is not a theme song, whatever it is called.
const THEME_SECONDS: std::ops::RangeInclusive<f64> = 45.0..=150.0;

/// Credits can run longer than a theme — a full cast roll over a still — so a
/// chapter that only *reads* like an ending gets this much room when it sits at
/// the end of the file.
const CREDITS_SECONDS: std::ops::RangeInclusive<f64> = 30.0..=300.0;

/// An opening that far into the file is not an opening. Cold opens run long in
/// a first episode, so this is generous.
const OPENING_BEFORE_SECONDS: f64 = 480.0;

/// Where the last part of a file starts, as a fraction of it. A chapter that
/// merely reads like credits is taken as credits past this point.
const TAIL_FRACTION: f64 = 0.75;

impl Claim {
    /// Read a claim out of a chapter name.
    ///
    /// Matched on whole words from the front, not on substrings: `Operation
    /// Briefing` starts with `op` and is not an opening, `Ending Note` is not
    /// an ending, and skipping a viewer out of either is a mistake they cannot
    /// undo without finding the seek bar. Leading numbering (`01 - OP`, `1.
    /// Opening`) is stepped over first, because plenty of muxes carry it.
    fn of(title: &str) -> Self {
        // Japanese names appear verbatim in releases from JP sources, and they
        // are unambiguous where the English ones are not. Checked before the
        // word split, which only knows about ASCII and would find no words at
        // all in a name written entirely in kana.
        if title.contains("オープニング") {
            return Self::Opening;
        }
        if title.contains("エンディング") {
            return Self::Ending;
        }
        if title.contains("予告") {
            return Self::Preview;
        }

        let words = words(title);
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        let Some((head, rest)) = words.split_first() else {
            return Self::Content;
        };
        let second = rest.first().copied().unwrap_or_default();

        match (*head, second) {
            // Two-word forms first: the second word is what tells `next
            // episode` from `next`, and `end credits` from `end`.
            ("next", _) | ("nep", _) => Self::Preview,
            ("end" | "ending" | "closing", "credits") => Self::Ending,
            ("cast" | "staff", "roll" | "credits") => Self::Ending,
            ("opening" | "op", "theme" | "song" | "credits") => Self::Opening,
            ("ending" | "ed", "theme" | "song") => Self::Ending,

            ("op" | "opening" | "openning" | "ncop", _) => Self::Opening,
            ("ed" | "ending" | "nced" | "outro" | "endcard", _) => Self::Ending,
            ("preview" | "pv" | "trailer" | "teaser" | "yokoku", _) => Self::Preview,

            ("intro", _) => Self::MaybeOpening,
            ("credits" | "closing" | "cast" | "staff", _) => Self::MaybeEnding,
            _ => Self::Content,
        }
    }
}

/// A chapter name as lowercase words, with any leading numbering dropped.
///
/// `OP1` and `ED2` keep their tag and lose their number — the digit is part of
/// the tag, not a word — while `02 - Opening` loses its leading `02` entirely.
fn words(title: &str) -> Vec<String> {
    let mut words: Vec<String> = title
        .trim()
        .to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| {
            word.trim_end_matches(|c: char| c.is_ascii_digit())
                .to_string()
        })
        .collect();
    // A leading token that was *only* digits is numbering, not a name.
    if words.first().is_some_and(String::is_empty) {
        words.remove(0);
    }
    words.retain(|word| !word.is_empty());
    words
}

/// The role of every chapter in a file, with the file's own shape taken into
/// account.
///
/// This is the function to use. [`Chapter::role`] reads a name in isolation and
/// cannot tell an opening called `Intro` from ten minutes of story called the
/// same thing; here the other chapters and the running time settle it:
///
/// * a chapter that merely *reads* like an opening is one only when the file
///   has no chapter that says so outright, it runs the length of a theme song,
///   and it is near the front;
/// * a chapter that reads like credits is the ending when it runs to about that
///   length, or when it sits in the last quarter of the file.
///
/// Both rules fail *towards content*: not offering a skip costs a button, and
/// skipping the viewer out of the story costs them the story.
pub fn roles(chapters: &[Chapter], duration: Option<f64>) -> Vec<ChapterRole> {
    let claims: Vec<Claim> = chapters
        .iter()
        .map(|chapter| Claim::of(chapter.title.as_deref().unwrap_or_default()))
        .collect();
    let has_real_opening = claims.contains(&Claim::Opening);

    claims
        .iter()
        .enumerate()
        .map(|(index, claim)| {
            let length = chapter_end(chapters, index, duration)
                .map(|end| end - chapters[index].start)
                .filter(|length| *length > 0.0);
            let start = chapters[index].start;

            match claim {
                Claim::Opening => ChapterRole::Opening,
                Claim::Ending => ChapterRole::Ending,
                Claim::Preview => ChapterRole::Preview,
                Claim::MaybeOpening
                    if !has_real_opening
                        && start <= OPENING_BEFORE_SECONDS
                        && length.is_some_and(|length| THEME_SECONDS.contains(&length)) =>
                {
                    ChapterRole::Opening
                }
                Claim::MaybeEnding
                    if length.is_some_and(|length| CREDITS_SECONDS.contains(&length))
                        || duration.is_some_and(|total| start >= total * TAIL_FRACTION) =>
                {
                    ChapterRole::Ending
                }
                _ => ChapterRole::Content,
            }
        })
        .collect()
}

/// Where the run of chapters that ends the file — credits, a preview, and
/// nothing the viewer would miss — begins.
///
/// **The suffix, not the last chapter.** A release that puts a Part C after the
/// ending and a preview after that (Fullmetal Alchemist Brotherhood does,
/// exactly once) must give up only the preview, or auto-advancing skips a scene
/// that matters. Walking back from the end and stopping at the first chapter
/// that is content is what expresses that: `… ED · Part C · Preview` yields the
/// preview alone, while the ordinary `… ED · Preview` yields both.
///
/// `None` when the file ends on content, has no chapters, or is *entirely*
/// skippable — the last of which is a mis-tagged file, not an episode that is
/// all credits.
pub fn credits_start(chapters: &[Chapter], roles: &[ChapterRole]) -> Option<f64> {
    let first = roles
        .iter()
        .rposition(|role| !role.ends_the_episode())
        .map(|last_content| last_content + 1)?;
    if first >= chapters.len() {
        return None;
    }
    Some(chapters[first].start)
}

/// Every chapter of the file mpv currently has open.
pub(crate) fn read(mpv: &Mpv) -> Vec<Chapter> {
    let count: i64 = mpv.get_property("chapter-list/count").unwrap_or(0);
    (0..count)
        .map(|index| Chapter {
            index,
            title: mpv
                .get_property::<String>(&format!("chapter-list/{index}/title"))
                .ok()
                .map(|title| title.trim().to_string())
                .filter(|title| !title.is_empty()),
            start: mpv
                .get_property(&format!("chapter-list/{index}/time"))
                .unwrap_or(0.0),
        })
        .collect()
}

/// Where the chapter starting at `index` ends: the next one's start, or the end
/// of the file.
pub fn chapter_end(chapters: &[Chapter], index: usize, duration: Option<f64>) -> Option<f64> {
    chapters
        .get(index + 1)
        .map(|next| next.start)
        .or(duration)
        .filter(|end| *end > 0.0)
}

/// The chapter a position falls in.
///
/// Linear rather than a binary search: a file has a dozen chapters, and this is
/// called once a frame at most.
pub fn chapter_at(chapters: &[Chapter], position: f64) -> Option<usize> {
    chapters
        .iter()
        .rposition(|chapter| position + f64::EPSILON >= chapter.start)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chapter(index: i64, title: &str, start: f64) -> Chapter {
        Chapter {
            index,
            title: Some(title.to_string()),
            start,
        }
    }

    #[test]
    fn the_names_releases_actually_use_are_recognised() {
        for (name, claim) in [
            ("OP", Claim::Opening),
            ("Opening", Claim::Opening),
            ("OP1 - Again", Claim::Opening),
            ("02 - Opening", Claim::Opening),
            ("Opening Song", Claim::Opening),
            ("NCOP", Claim::Opening),
            ("オープニング", Claim::Opening),
            ("ED", Claim::Ending),
            ("Ending 2", Claim::Ending),
            ("End Credits", Claim::Ending),
            ("Cast Roll", Claim::Ending),
            ("Endcard", Claim::Ending),
            ("エンディング", Claim::Ending),
            ("Preview", Claim::Preview),
            ("Next Episode", Claim::Preview),
            ("Next Episode Preview", Claim::Preview),
            ("予告", Claim::Preview),
        ] {
            assert_eq!(Claim::of(name), claim, "{name}");
        }
    }

    #[test]
    fn a_chapter_that_merely_starts_with_those_letters_is_not_one() {
        // The reason this matches words and not prefixes: skipping the viewer
        // out of "Operation Briefing" would be a bug they cannot undo without
        // finding the seek bar.
        for name in ["Operation Briefing", "Editor's cut", "Part A", "Part C", ""] {
            assert_eq!(Claim::of(name), Claim::Content, "{name}");
        }
    }

    /// The names that cannot be settled by reading them.
    #[test]
    fn an_ambiguous_name_is_not_decided_by_itself() {
        assert_eq!(Claim::of("Intro"), Claim::MaybeOpening);
        assert_eq!(Claim::of("Credits"), Claim::MaybeEnding);
        assert_eq!(Claim::of("Cast"), Claim::MaybeEnding);
        // And a chapter read in isolation never resolves to a skip.
        assert_eq!(chapter(0, "Intro", 0.0).role(), ChapterRole::Content);
    }

    /// **The first episode of Oshi no Ko.** No opening, no ending — an eighty
    /// minute episode that begins with a long `Intro` that is the story, and
    /// ends with a cast scroll. Offering "skip opening" over the first ninety
    /// seconds of *that* skips the beginning of the series.
    #[test]
    fn a_long_intro_is_the_episode_and_not_an_opening() {
        let chapters = vec![
            chapter(0, "Intro", 0.0),
            chapter(1, "Part A", 700.0),
            chapter(2, "Part B", 2400.0),
            chapter(3, "Cast", 4680.0),
        ];
        let roles = roles(&chapters, Some(4800.0));
        assert_eq!(
            roles[0],
            ChapterRole::Content,
            "eleven minutes is not a theme"
        );
        assert_eq!(
            roles[3],
            ChapterRole::Ending,
            "a cast roll at the end is one"
        );
        // Only the cast scroll is the tail; the episode is not skipped into.
        assert_eq!(credits_start(&chapters, &roles), Some(4680.0));
    }

    /// The same name in the release that uses it for the theme song: ninety
    /// seconds, near the front, and no other opening in the file.
    #[test]
    fn a_theme_length_intro_near_the_front_is_an_opening() {
        let chapters = vec![
            chapter(0, "Avant", 0.0),
            chapter(1, "Intro", 90.0),
            chapter(2, "Part A", 180.0),
        ];
        assert_eq!(roles(&chapters, Some(1440.0))[1], ChapterRole::Opening);

        // …but not when the file already says which chapter the opening is.
        let chapters = vec![
            chapter(0, "Intro", 0.0),
            chapter(1, "OP", 90.0),
            chapter(2, "Part A", 180.0),
        ];
        let roles = roles(&chapters, Some(1440.0));
        assert_eq!(roles[0], ChapterRole::Content);
        assert_eq!(roles[1], ChapterRole::Opening);
    }

    /// The ordinary shape: the credits and the trailer for next week are one
    /// run at the end, and the countdown starts where the credits do.
    #[test]
    fn the_tail_is_the_whole_run_of_credits_and_preview() {
        let chapters = vec![
            chapter(0, "OP", 0.0),
            chapter(1, "Part A", 90.0),
            chapter(2, "Part B", 700.0),
            chapter(3, "ED", 1320.0),
            chapter(4, "Preview", 1410.0),
        ];
        let roles = roles(&chapters, Some(1440.0));
        assert_eq!(credits_start(&chapters, &roles), Some(1320.0));
    }

    /// **Episode 46 of Fullmetal Alchemist Brotherhood.** A Part C *after* the
    /// ending, and the preview after that. Only the preview may be skipped —
    /// treating everything from the ending on as tail would drop a scene.
    #[test]
    fn content_after_the_ending_keeps_the_ending_out_of_the_tail() {
        let chapters = vec![
            chapter(0, "Part A", 0.0),
            chapter(1, "Part B", 700.0),
            chapter(2, "ED", 1260.0),
            chapter(3, "Part C", 1350.0),
            chapter(4, "Preview", 1400.0),
        ];
        let roles = roles(&chapters, Some(1440.0));
        assert_eq!(roles[2], ChapterRole::Ending, "it is still an ending");
        assert_eq!(credits_start(&chapters, &roles), Some(1400.0));
    }

    /// A file that ends on the story has no tail at all, and one with no
    /// chapters has nothing to read.
    #[test]
    fn a_file_that_ends_on_content_has_no_tail() {
        let chapters = vec![chapter(0, "OP", 0.0), chapter(1, "Part A", 90.0)];
        let roles = roles(&chapters, Some(1440.0));
        assert_eq!(credits_start(&chapters, &roles), None);
        assert_eq!(credits_start(&[], &[]), None);

        // Everything skippable is a mis-tagged file, not an episode that is
        // entirely credits: auto-advancing out of it at second zero would be
        // worse than doing nothing.
        let chapters = vec![chapter(0, "OP", 0.0), chapter(1, "ED", 90.0)];
        let roles = vec![ChapterRole::Ending, ChapterRole::Ending];
        assert_eq!(credits_start(&chapters, &roles), None);
    }

    #[test]
    fn a_position_lands_in_the_chapter_that_contains_it() {
        let chapters = vec![
            chapter(0, "Intro", 0.0),
            chapter(1, "OP", 24.0),
            chapter(2, "Part A", 114.0),
        ];
        assert_eq!(chapter_at(&chapters, 0.0), Some(0));
        assert_eq!(chapter_at(&chapters, 23.9), Some(0));
        // Exactly on a boundary belongs to the chapter that starts there,
        // which is what makes "skip" land *out* of the opening rather than at
        // its last frame.
        assert_eq!(chapter_at(&chapters, 24.0), Some(1));
        assert_eq!(chapter_at(&chapters, 3000.0), Some(2));
    }

    #[test]
    fn a_chapter_before_the_first_one_starts_is_in_no_chapter() {
        let chapters = vec![chapter(0, "OP", 24.0)];
        assert_eq!(chapter_at(&chapters, 0.0), None);
    }

    #[test]
    fn the_last_chapter_ends_where_the_file_does() {
        let chapters = vec![chapter(0, "OP", 0.0), chapter(1, "Part A", 90.0)];
        assert_eq!(chapter_end(&chapters, 0, Some(1440.0)), Some(90.0));
        assert_eq!(chapter_end(&chapters, 1, Some(1440.0)), Some(1440.0));
        // No duration yet and nothing after it: there is nowhere to skip to,
        // and a button that seeks to zero would be worse than no button.
        assert_eq!(chapter_end(&chapters, 1, None), None);
    }
}
