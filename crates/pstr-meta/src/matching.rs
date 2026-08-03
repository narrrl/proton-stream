//! Deciding which of a provider's answers is the title you actually have.
//!
//! A release-named library does not hand you clean titles. `[SubsGroup] Shingeki
//! no Kyojin S3` and `Attack on Titan (2013)` are the same show, and the
//! provider knows it under four names in three scripts. So matching is scoring,
//! not lookup, and the two things that make it work are:
//!
//! * **Every alias counts.** A candidate is scored on its best name, not its
//!   canonical one. AniList's romaji, English, native and synonym lists exist
//!   precisely because none of them is *the* title.
//! * **A bad match is worse than none.** Putting the poster of a different show
//!   on a title is a mistake nobody can see is a mistake — it just looks like
//!   the library is wrong. So the floor is high, and anything under it is a
//!   cached miss rather than the best of a bad set.
//!
//! The similarity itself is Sørensen–Dice over character bigrams. It is not the
//! most sophisticated measure available, but it has the properties that matter
//! here: it is order-insensitive enough to survive `Bebop, Cowboy`, length-aware
//! enough not to match `Gintama` to `Gintama°: Enchousen`, and it needs no
//! tuning.

use pstr_core::library::{TitleKind, title_key};
use pstr_core::metadata::TitleMetadata;

/// A candidate's score has to clear this to be used.
///
/// Chosen from what the failure modes cost rather than from a curve: at 0.72 a
/// missing subtitle or a swapped season number still matches, while two
/// different shows sharing a franchise name — the common near-miss in an anime
/// library — do not. See the tests, which pin both sides.
pub const MATCH_FLOOR: f32 = 0.72;

/// The title being looked up.
#[derive(Debug, Clone)]
pub struct Query {
    /// The title as the library has it, already stripped of release tags by
    /// [`pstr_core::naming`].
    pub name: String,
    pub year: Option<u32>,
    pub kind: TitleKind,
    /// Whether the library *knows* what kind of thing this is.
    ///
    /// The mirror of [`Candidate::kind_known`], and it exists for the same
    /// reason. A folder holding the three `Heaven's Feel` films is a `Series`
    /// to [`pstr_core::library`] only because it holds three files — the films
    /// themselves are numbered nothing — and scoring that guess against the
    /// provider's (correct) `Film` costs the right answer the 0.05 that puts it
    /// under the floor.
    pub kind_known: bool,
}

impl Query {
    pub fn new(name: impl Into<String>, year: Option<u32>, kind: TitleKind) -> Self {
        Self {
            name: name.into(),
            year,
            kind,
            kind_known: true,
        }
    }

    /// Say that [`Query::kind`] was inferred rather than stated — see
    /// [`Query::kind_known`].
    pub fn with_guessed_kind(mut self) -> Self {
        self.kind_known = false;
        self
    }
}

/// One of a provider's answers, with every name it is known by.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub metadata: TitleMetadata,
    /// Romaji, English, native, synonyms — whatever the provider had. The
    /// canonical name is included by the providers that build these.
    pub aliases: Vec<String>,
    /// Whether the provider actually stated what kind of thing this is.
    ///
    /// [`TitleMetadata::kind`] has no "unknown", because a badge has to say
    /// something — but scoring must not read that fallback as a fact. AniList
    /// leaves `format` null on anything unaired, and treating that as "film"
    /// then *penalising* it against a series is how an announced-but-unreleased
    /// entry loses to a worse match. Absence of evidence, not evidence.
    pub kind_known: bool,
}

/// How well `candidate` answers `query`, from 0 to 1.
pub fn score(query: &Query, candidate: &Candidate) -> f32 {
    let wanted = title_key(&query.name);
    if wanted.is_empty() {
        return 0.0;
    }

    let best = candidate
        .aliases
        .iter()
        .map(|alias| similarity(&wanted, &title_key(alias)))
        .fold(0.0_f32, f32::max);

    // Adjustments, not tie-breakers: they move a score across the floor only
    // when it was already close to it.
    let mut score = best;

    match (query.year, candidate.metadata.year) {
        // Same year is strong evidence for two titles that already read alike.
        (Some(wanted), Some(found)) if wanted == found => score += 0.06,
        // A year apart is normal — a season airing across New Year, a provider
        // dating by première rather than release. Further apart is not.
        (Some(wanted), Some(found)) if wanted.abs_diff(found) > 1 => score -= 0.15,
        _ => {}
    }

    if query.kind_known && candidate.kind_known && query.kind != candidate.metadata.kind {
        // A film and a series of the same name are usually genuinely related —
        // an adaptation, a compilation — so this is a nudge, not a veto. And
        // only when both sides actually said; see `Candidate::kind_known` and
        // `Query::kind_known`.
        score -= 0.05;
    }

    score.clamp(0.0, 1.0)
}

/// The best answer above [`MATCH_FLOOR`], if there is one.
///
/// Ties go to whichever the provider listed first, and that is load-bearing
/// rather than arbitrary. Franchises share synonyms — every *Evangelion* entry
/// on AniList carries `EVANGELION:30+;`, so a library folder named plainly
/// `Evangelion` scores several of them identically — and when the text cannot
/// separate them, the provider's own relevance ordering is the only evidence
/// left. Taking the last one instead, which is what `Iterator::max_by` does,
/// picks the *least* relevant of the tied set: it is how a folder of the 1995
/// series came back matched to a 2026 anniversary screening.
pub fn best(query: &Query, candidates: Vec<Candidate>) -> Option<TitleMetadata> {
    let mut best: Option<(TitleMetadata, f32)> = None;
    for candidate in candidates {
        let score = score(query, &candidate);
        // Strictly greater: an equal score leaves the earlier candidate in
        // place, which is what keeps the provider's ordering as the tie-break.
        if best.as_ref().is_none_or(|(_, best)| score > *best) {
            best = Some((candidate.metadata, score));
        }
    }

    let (metadata, score) = best?;
    if score < MATCH_FLOOR {
        tracing::debug!(
            "no match for {:?}: best candidate scored {score:.2}, floor is {MATCH_FLOOR}",
            query.name
        );
        return None;
    }
    Some(metadata)
}

/// Sørensen–Dice over character bigrams, on already-normalised strings.
///
/// Identical strings are 1.0 and a one-character string is compared directly,
/// since it has no bigrams to compare.
fn similarity(left: &str, right: &str) -> f32 {
    if left == right {
        return 1.0;
    }
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }

    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.len() < 2 || right.len() < 2 {
        return 0.0;
    }

    // Multiset intersection: a bigram appearing twice on the left and once on
    // the right counts once, which is what keeps `aaaa` from matching `aa`.
    let mut theirs: Vec<[char; 2]> = right.windows(2).map(|pair| [pair[0], pair[1]]).collect();
    let mut shared = 0usize;
    for pair in left.windows(2) {
        let pair = [pair[0], pair[1]];
        if let Some(position) = theirs.iter().position(|other| *other == pair) {
            theirs.swap_remove(position);
            shared += 1;
        }
    }

    let total = (left.len() - 1) + (right.len() - 1);
    (2.0 * shared as f32) / total as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use pstr_core::metadata::ProviderId;

    fn candidate(name: &str, aliases: &[&str], year: Option<u32>, kind: TitleKind) -> Candidate {
        Candidate {
            metadata: TitleMetadata {
                provider: ProviderId::AniList,
                remote_id: "1".into(),
                name: name.into(),
                original_name: None,
                overview: None,
                year,
                kind,
                poster_url: None,
                backdrop_url: None,
                rating: None,
                genres: Vec::new(),
                episodes: None,
                url: None,
            },
            aliases: std::iter::once(name)
                .chain(aliases.iter().copied())
                .map(str::to_string)
                .collect(),
            kind_known: true,
        }
    }

    #[test]
    fn an_identical_title_scores_perfectly() {
        let query = Query::new("Cowboy Bebop", None, TitleKind::Series);
        let candidate = candidate("Cowboy Bebop", &[], None, TitleKind::Series);
        assert!(score(&query, &candidate) >= 1.0);
    }

    /// The case the alias list exists for: the library has the romaji name and
    /// the provider's canonical name is the English one, or the reverse.
    #[test]
    fn a_match_on_an_alias_is_as_good_as_one_on_the_name() {
        let query = Query::new("Shingeki no Kyojin", None, TitleKind::Series);
        let candidate = candidate(
            "Attack on Titan",
            &["Shingeki no Kyojin"],
            Some(2013),
            TitleKind::Series,
        );
        assert!(score(&query, &candidate) >= MATCH_FLOOR);
    }

    /// Punctuation and case are the parser's business, not the matcher's — and
    /// `title_key` has already removed them.
    #[test]
    fn punctuation_and_case_do_not_affect_the_score() {
        let query = Query::new("FULLMETAL ALCHEMIST: BROTHERHOOD", None, TitleKind::Series);
        let candidate = candidate(
            "Fullmetal Alchemist: Brotherhood",
            &[],
            None,
            TitleKind::Series,
        );
        assert!(score(&query, &candidate) >= 1.0);
    }

    /// The mistake worth preventing: two entries in one franchise, where the
    /// wrong poster looks like a working feature rather than a bug.
    #[test]
    fn a_different_entry_in_the_same_franchise_does_not_match() {
        let query = Query::new("Gintama", None, TitleKind::Series);
        let candidate = candidate("Gintama°: Enchousen", &[], None, TitleKind::Series);
        assert!(
            score(&query, &candidate) < MATCH_FLOOR,
            "scored {}",
            score(&query, &candidate)
        );
    }

    #[test]
    fn a_wholly_unrelated_title_does_not_match() {
        let query = Query::new("Cowboy Bebop", None, TitleKind::Series);
        let candidate = candidate("Neon Genesis Evangelion", &[], None, TitleKind::Series);
        assert!(score(&query, &candidate) < MATCH_FLOOR);
    }

    /// A decade apart is a different work with a reused name — a remake, or a
    /// coincidence. One year apart is the same work dated differently.
    #[test]
    fn a_distant_year_is_penalised_and_a_near_one_is_not() {
        let query = Query::new("Bebop", Some(1998), TitleKind::Series);
        let near = candidate("Bebop", &[], Some(1999), TitleKind::Series);
        let far = candidate("Bebop", &[], Some(2021), TitleKind::Series);
        assert!(score(&query, &near) > score(&query, &far));
        assert!(score(&query, &near) >= MATCH_FLOOR);
    }

    #[test]
    fn the_best_candidate_wins_and_a_weak_field_yields_nothing() {
        let query = Query::new("Cowboy Bebop", Some(1998), TitleKind::Series);
        let found = best(
            &query,
            vec![
                candidate(
                    "Neon Genesis Evangelion",
                    &[],
                    Some(1995),
                    TitleKind::Series,
                ),
                candidate("Cowboy Bebop", &[], Some(1998), TitleKind::Series),
            ],
        );
        assert_eq!(found.map(|found| found.name), Some("Cowboy Bebop".into()));

        assert!(
            best(
                &query,
                vec![candidate(
                    "Serial Experiments Lain",
                    &[],
                    None,
                    TitleKind::Series
                )]
            )
            .is_none()
        );
        assert!(best(&query, Vec::new()).is_none());
    }

    /// Repeated bigrams must not inflate a score: without multiset counting,
    /// a short repetitive name matches a long repetitive one perfectly.
    #[test]
    fn repeated_bigrams_are_counted_once_each() {
        assert!(similarity("aaaa", "aa") < 1.0);
    }

    /// Measured against the real thing: searching AniList for `Evangelion`
    /// returns several entries that all carry the synonym `EVANGELION:30+;`,
    /// so they score identically and nothing in the text can separate them.
    /// The provider's own ordering is then the only evidence there is, and
    /// taking the last of the tied set — which is what `max_by` does — picked
    /// a 2026 anniversary screening over the series the folder was named for.
    #[test]
    fn tied_candidates_are_settled_by_the_providers_own_ordering() {
        let query = Query::new("Evangelion", None, TitleKind::Series);
        let tied = vec![
            candidate(
                "Evangelion (Shinsaku Series)",
                &["EVANGELION:30+;"],
                None,
                TitleKind::Series,
            ),
            candidate(
                "Evangelion 30th Anniversary Special Screening",
                &["EVANGELION:30+;"],
                Some(2026),
                TitleKind::Series,
            ),
        ];

        // Both clear the floor on the shared synonym; the first one listed wins.
        assert!(score(&query, &tied[0]) >= MATCH_FLOOR);
        assert_eq!(
            best(&query, tied).map(|found| found.name),
            Some("Evangelion (Shinsaku Series)".into())
        );
    }

    /// The other half of the Evangelion mismatch, and the more subtle half.
    ///
    /// AniList leaves `format` null on anything unaired. [`TitleMetadata::kind`]
    /// has to say *something*, so it falls back to `Film` — and scoring must not
    /// then read that fallback as the provider having said "film" and dock the
    /// candidate for it. Doing so cost the top-ranked entry 0.05 and handed the
    /// match to a 2026 anniversary screening that happened to have a format.
    #[test]
    fn an_unstated_kind_is_not_penalised_the_way_a_wrong_one_is() {
        let query = Query::new("Evangelion", None, TitleKind::Series);

        let mut unstated = candidate("Evangelion", &[], None, TitleKind::Film);
        unstated.kind_known = false;
        let stated = candidate("Evangelion", &[], None, TitleKind::Film);

        assert!(score(&query, &unstated) > score(&query, &stated));
        assert_eq!(score(&query, &unstated), 1.0);

        // And the same in reverse: a library that guessed its own kind has
        // nothing to disagree with the provider about.
        let guessed = query.clone().with_guessed_kind();
        assert_eq!(score(&guessed, &stated), 1.0);
    }

    /// **The apostrophe case, both shapes of it.** A folder cannot carry the
    /// apostrophe of `Fate/stay night [Heaven's Feel]`, so it is called
    /// `Heavens Feel` — and until [`title_key`] closed the apostrophe up rather
    /// than splitting on it, the provider's own entry scored under the floor.
    ///
    /// One film per folder matches outright. The trilogy in a single folder is
    /// the harder half: the library calls three files a series where the
    /// provider calls each of them a film, so it clears the floor by a hair
    /// with that mismatch subtracted.
    #[test]
    fn a_title_that_lost_its_apostrophe_matches_the_entry_that_kept_one() {
        let film = candidate(
            "Fate/stay night [Heaven's Feel] I. presage flower",
            &[],
            Some(2017),
            TitleKind::Film,
        );

        let alone = Query::new(
            "Fate stay night Heavens Feel I Presage Flower",
            Some(2017),
            TitleKind::Film,
        );
        assert!(score(&alone, &film) >= 1.0);

        // Three films in one folder: a `Series` only because there are three of
        // them, so the kind is a guess and costs the film entry nothing.
        let trilogy =
            Query::new("Fate stay night Heavens Feel", None, TitleKind::Series).with_guessed_kind();
        assert!(
            score(&trilogy, &film) >= MATCH_FLOOR,
            "scored {}",
            score(&trilogy, &film)
        );
    }

    /// The tie-break must not override the score: a later candidate that
    /// genuinely matches better still wins.
    #[test]
    fn a_better_later_candidate_still_beats_an_earlier_weaker_one() {
        let query = Query::new("Cowboy Bebop", None, TitleKind::Series);
        let found = best(
            &query,
            vec![
                candidate("Cowboy Bebop: The Movie", &[], None, TitleKind::Film),
                candidate("Cowboy Bebop", &[], None, TitleKind::Series),
            ],
        );
        assert_eq!(found.map(|found| found.name), Some("Cowboy Bebop".into()));
    }
}
