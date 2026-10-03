//! What a metadata provider knows about a title, and whether to ask one.
//!
//! The *model* is here rather than in `pstr-meta` because the catalog stores it
//! and the UI draws it, and neither of those should have to depend on the crate
//! that talks HTTP to AniList. `pstr-meta` fills these in; everything else only
//! reads them.
//!
//! ## Enrichment is off by default, and says why
//!
//! Matching a library against a provider means sending that provider the titles
//! in it. For a library of public-domain films that is nothing; for a library of
//! anything else it is a list of what someone watches, attached to an IP
//! address, held by a third party under their retention policy and not ours.
//! That is a real cost and the viewer is the only one who can weigh it, so
//! [`MetadataConfig::enabled`] starts `false`, the UI states the cost in the
//! same breath as the switch, and nothing here is requested until it is on.
//!
//! With it off, posters fall back to Proton's own thumbnails — which for a share
//! filled by the Linux client means no posters at all, because Proton renders
//! thumbnails at upload time and that client attaches none. See
//! `docs/DEVELOPMENT.md`.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::library::TitleKind;

/// Which provider a match came from.
///
/// Stored as a string in the catalog rather than an integer: a row whose
/// provider this build does not recognise should read as "some other provider",
/// which is a re-match, not a parse failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    /// AniList. Anime only, no API key, no account.
    #[default]
    AniList,
    /// The Movie Database. Film and television, needs a free API key.
    Tmdb,
}

impl ProviderId {
    pub const ALL: [Self; 2] = [Self::AniList, Self::Tmdb];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AniList => "anilist",
            Self::Tmdb => "tmdb",
        }
    }

    /// What to call it in the UI.
    pub fn label(self) -> &'static str {
        match self {
            Self::AniList => "AniList",
            Self::Tmdb => "TMDB",
        }
    }

    /// One line on what it covers and what it costs to use.
    pub fn description(self) -> &'static str {
        match self {
            Self::AniList => {
                "Anime, with no API key and no account. Episode names and wide art come from \
                 ani.zip, which is sent the matched ids."
            }
            Self::Tmdb => "Film and television. Needs a free API key from themoviedb.org.",
        }
    }

    /// Whether this provider cannot be used without a key.
    pub fn needs_api_key(self) -> bool {
        matches!(self, Self::Tmdb)
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|provider| provider.as_str() == text)
    }
}

/// What a provider said about one title.
///
/// Every field past the identity is optional, because every field is optional at
/// some provider: AniList has no backdrop for most shows, TMDB has no episode
/// count on a film, and a brand-new entry may be nothing but a name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TitleMetadata {
    pub provider: ProviderId,
    /// The provider's own id, so a re-fetch does not re-search.
    pub remote_id: String,
    /// The canonical title, in the viewer's language where the provider has one.
    pub name: String,
    /// The title in its original language, when it differs.
    pub original_name: Option<String>,
    pub overview: Option<String>,
    pub year: Option<u32>,
    pub kind: TitleKind,
    /// Portrait art, around 2:3.
    pub poster_url: Option<String>,
    /// Landscape art, around 16:9. Much rarer than a poster, and the reason the
    /// grid cannot simply prefer one shape.
    pub backdrop_url: Option<String>,
    /// Out of 10, however the provider scores.
    pub rating: Option<f32>,
    pub genres: Vec<String>,
    /// How many episodes the provider thinks there are — useful next to how
    /// many the library actually holds.
    pub episodes: Option<u32>,
    /// The title's page, for a viewer who wants the rest of it.
    pub url: Option<String>,
}

impl TitleMetadata {
    /// The art to put on a 16:9 tile, most appropriate first.
    ///
    /// A backdrop is the right shape; a poster is the wrong shape but the right
    /// picture, and is drawn letterboxed rather than cropped — cropping a 2:3
    /// poster to 16:9 removes most of what makes it recognisable.
    pub fn tile_art(&self) -> Option<(&str, ArtShape)> {
        if let Some(url) = &self.backdrop_url {
            return Some((url, ArtShape::Landscape));
        }
        self.poster_url
            .as_deref()
            .map(|url| (url, ArtShape::Portrait))
    }
}

/// What a provider said about one episode.
///
/// Thin on purpose. The point of this is the two lines a viewer reads before
/// picking an episode — what it is called and what happens in it — and every
/// extra field is another column to migrate for something no row shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EpisodeMetadata {
    /// The season the provider files it under, or `None` when it numbers
    /// episodes straight through — which is how AniList counts, and how most
    /// anime releases name their files.
    pub season: Option<u32>,
    pub number: u32,
    /// Its place counting straight through every season, when the provider
    /// knows it. What an absolutely-numbered file inside a season folder —
    /// `Attack on Titan/Season 2/… - 26.mkv` — is matched on.
    #[serde(default)]
    pub absolute: Option<u32>,
    /// Its season and number counting each provider *entry* as a season: the
    /// position of the entry it belongs to among the franchise's series
    /// entries, and its number within that entry.
    ///
    /// Not TVDB's season. A release that follows AniList rather than TVDB
    /// names Mushishi's *Zoku Shou 2* `S03`, where TVDB files it as the second
    /// half of season two; this is what that file is answered from. See
    /// [`EpisodeGuide::get`] for when it is allowed to be.
    #[serde(default)]
    pub entry_season: Option<u32>,
    #[serde(default)]
    pub entry_number: Option<u32>,
    pub name: Option<String>,
    pub overview: Option<String>,
    /// A frame from the episode, around 16:9.
    pub still_url: Option<String>,
    /// As the provider writes it — `2009-10-10`. Not parsed: it is shown, not
    /// sorted on, and a provider that answers `2009` should not be a parse
    /// error.
    pub air_date: Option<String>,
}

/// What enriching a matched title found: its episodes, and art the search
/// answer did not carry.
///
/// Stored as one unit by `Catalog::set_enrichment`, and recorded as asked even
/// when both are empty — a film has no episode list, and without the record it
/// would be asked again on every match run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Enrichment {
    pub episodes: Vec<EpisodeMetadata>,
    /// A 16:9 picture of the whole title, when one was found. `None` leaves
    /// whatever backdrop the match already had.
    pub backdrop_url: Option<String>,
}

/// Every episode a provider listed for one title, ready to look up.
///
/// The lookup is the whole reason this is a type rather than a `Vec`: a file
/// numbered `#057` and a provider that files episodes under seasons have to
/// meet somewhere, and that reconciliation should happen in one tested place
/// rather than in each caller.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EpisodeGuide {
    /// Keyed `(season, number)`, where `None` is the provider's own absolute
    /// numbering.
    entries: HashMap<(Option<u32>, u32), EpisodeMetadata>,
    /// [`EpisodeMetadata::absolute`] to the key of the entry holding it.
    absolute: HashMap<u32, (Option<u32>, u32)>,
    /// [`EpisodeMetadata::entry_season`] and `entry_number` to the key of the
    /// entry holding them.
    by_entry: HashMap<(u32, u32), (Option<u32>, u32)>,
    /// Every season some entry is filed under, for deciding when the entry
    /// numbering may answer.
    stated: HashSet<u32>,
}

impl EpisodeGuide {
    pub fn new(episodes: Vec<EpisodeMetadata>) -> Self {
        let mut guide = Self::default();
        for episode in episodes {
            let key = (episode.season, episode.number);
            if let Some(absolute) = episode.absolute {
                guide.absolute.insert(absolute, key);
            }
            if let (Some(season), Some(number)) = (episode.entry_season, episode.entry_number) {
                guide.by_entry.insert((season, number), key);
            }
            if let Some(season) = episode.season {
                guide.stated.insert(season);
            }
            guide.entries.insert(key, episode);
        }
        guide
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// What the provider says about the file numbered like this.
    ///
    /// Tried in order:
    ///
    /// 1. the season and number the filename states;
    /// 2. for a file in no season or in season one, the provider's own absolute
    ///    list, and for a file in no season, season one — what makes `#057`
    ///    line up with TMDB filing a 64-episode anime under one season;
    /// 3. the absolute number, into the season the file states — what makes
    ///    `Season 2/… - 26.mkv` the second season's first episode;
    /// 4. the entry numbering, for a season the provider files nothing under.
    ///
    /// **No fallback crosses into another season.** Step 2 stops at season one
    /// and step 3 only answers inside the season the file named, because
    /// answering `S02E07` with season one's seventh episode captions every
    /// episode of every later season with the wrong name — and a wrong caption
    /// does not look like a bug, it looks like the library is wrong. Step 4
    /// only answers a season the provider has nothing filed under, so a release
    /// numbered the provider's way is never re-read the other way: Mushoku
    /// Tensei's second *entry* is the second half of season one, and its
    /// `S02E01` is season two's.
    pub fn get(&self, season: Option<u32>, number: u32) -> Option<&EpisodeMetadata> {
        if let Some(found) = self.entries.get(&(season, number)) {
            return Some(found);
        }
        if matches!(season, None | Some(1))
            && let Some(found) = self.entries.get(&(None, number))
        {
            return Some(found);
        }
        if season.is_none()
            && let Some(found) = self.entries.get(&(Some(1), number))
        {
            return Some(found);
        }
        if let Some(found) = self
            .absolute
            .get(&number)
            .and_then(|key| self.entries.get(key))
            && (season.is_none() || found.season == season)
        {
            return Some(found);
        }
        let season = season?;
        if self.stated.contains(&season) {
            return None;
        }
        self.by_entry
            .get(&(season, number))
            .and_then(|key| self.entries.get(key))
    }

    /// Which seasons this guide actually has answers for. `None` is the
    /// provider's own absolute numbering.
    pub fn seasons(&self) -> HashSet<Option<u32>> {
        self.entries
            .values()
            .map(|episode| episode.season)
            .collect()
    }

    /// Every entry, for storing.
    pub fn episodes(&self) -> impl Iterator<Item = &EpisodeMetadata> {
        self.entries.values()
    }
}

/// How a piece of art should be fitted into a tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtShape {
    /// Roughly the tile's shape: crop to fill.
    Landscape,
    /// Taller than the tile: fit inside it, and leave the sides.
    Portrait,
}

/// What the catalog remembers about one title's metadata, including that there
/// was none.
#[derive(Debug, Clone, PartialEq)]
pub struct MetadataRecord {
    /// [`crate::library::title_key`] of the title this describes.
    pub title_key: String,
    /// The provider that was asked. Kept even on a miss, so switching provider
    /// re-asks rather than trusting the other one's silence.
    pub provider: ProviderId,
    /// `None` means the provider was asked and had nothing. That is a real
    /// answer and it is cached: without it every grid render pays a round-trip
    /// per unmatched title, which is the trap `proton-drive-linux`'s photo grid
    /// hit and this one is not going to.
    pub metadata: Option<TitleMetadata>,
    /// Unix seconds. What makes a miss expire rather than being forever.
    pub fetched_at: i64,
    /// The viewer picked this entry themselves.
    ///
    /// A hand-picked match outranks anything the matcher would decide: it never
    /// expires, and "match again" leaves it alone. Otherwise the one title the
    /// scorer gets wrong — the case this flag exists for — would be corrected by
    /// hand and then quietly un-corrected by the next run.
    pub manual: bool,
}

/// How long a stored answer is trusted.
///
/// A match is effectively permanent — a show does not stop being that show — but
/// artwork and ratings move, and more to the point a *miss* has to expire, or a
/// title the provider had not indexed yet stays blank for the life of the
/// install. Misses are retried far sooner than matches are refreshed.
pub const MATCH_TTL_SECS: i64 = 60 * 60 * 24 * 30;
pub const MISS_TTL_SECS: i64 = 60 * 60 * 24 * 3;

impl MetadataRecord {
    /// Whether this answer is still worth using rather than asking again.
    pub fn is_fresh(&self, now: i64, provider: ProviderId) -> bool {
        if self.provider != provider {
            return false;
        }
        // A hand-picked match does not go stale: nothing a re-ask could learn
        // would be better than what the viewer already said it was.
        if self.manual {
            return true;
        }
        let ttl = if self.metadata.is_some() {
            MATCH_TTL_SECS
        } else {
            MISS_TTL_SECS
        };
        now.saturating_sub(self.fetched_at) < ttl
    }
}

/// Whether to enrich, with what.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetadataConfig {
    /// Off until the viewer turns it on. See the module note.
    pub enabled: bool,
    pub provider: ProviderId,
    /// BCP-47-ish, as the provider wants it. Only TMDB uses it; AniList
    /// answers in romaji and English and the match picks between them.
    pub language: String,
}

impl Default for MetadataConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: ProviderId::AniList,
            language: "en".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> TitleMetadata {
        TitleMetadata {
            provider: ProviderId::AniList,
            remote_id: "1".into(),
            name: "Cowboy Bebop".into(),
            original_name: None,
            overview: None,
            year: Some(1998),
            kind: TitleKind::Series,
            poster_url: None,
            backdrop_url: None,
            rating: None,
            genres: Vec::new(),
            episodes: Some(26),
            url: None,
        }
    }

    fn episode(season: Option<u32>, number: u32, name: &str) -> EpisodeMetadata {
        EpisodeMetadata {
            season,
            number,
            absolute: None,
            entry_season: None,
            entry_number: None,
            name: Some(name.into()),
            overview: None,
            still_url: None,
            air_date: None,
        }
    }

    /// An episode as ani.zip describes one: TVDB's season and number, TVDB's
    /// absolute number, and its place in the AniList entry it came from.
    fn numbered(
        (season, number): (u32, u32),
        absolute: u32,
        (entry_season, entry_number): (u32, u32),
        name: &str,
    ) -> EpisodeMetadata {
        EpisodeMetadata {
            absolute: Some(absolute),
            entry_season: Some(entry_season),
            entry_number: Some(entry_number),
            ..episode(Some(season), number, name)
        }
    }

    fn name_of(guide: &EpisodeGuide, season: Option<u32>, number: u32) -> Option<String> {
        guide.get(season, number).and_then(|e| e.name.clone())
    }

    /// `Attack on Titan/Season 2/[Anime Time] Attack On Titan - 26.mkv`: a
    /// season folder full of absolutely-numbered files.
    #[test]
    fn an_absolute_number_in_a_season_folder_finds_that_seasons_episode() {
        let guide = EpisodeGuide::new(vec![
            numbered((1, 1), 1, (1, 1), "To You, in 2000 Years"),
            numbered((2, 1), 26, (2, 1), "Beast Titan"),
        ]);
        assert_eq!(name_of(&guide, Some(2), 26).as_deref(), Some("Beast Titan"));
        // And with no season at all, the absolute number is the whole answer.
        assert_eq!(name_of(&guide, None, 26).as_deref(), Some("Beast Titan"));
    }

    /// The absolute fallback must not carry a file into a season it did not
    /// name: `S03E13` of a show whose 13th episode is in season one is not that
    /// episode.
    #[test]
    fn an_absolute_number_never_answers_for_another_season() {
        let guide = EpisodeGuide::new(vec![
            numbered((1, 13), 13, (1, 13), "Season one's thirteenth"),
            numbered((3, 1), 38, (3, 1), "Smoke Signal"),
        ]);
        assert_eq!(guide.get(Some(3), 13), None);
    }

    /// Mushishi: TVDB files both halves of *Zoku Shou* as season two, and the
    /// release calls the second half `S03`. Season three exists nowhere at the
    /// provider, so the entry numbering answers it.
    #[test]
    fn a_season_the_provider_does_not_have_is_answered_by_entry() {
        let guide = EpisodeGuide::new(vec![
            numbered((1, 1), 1, (1, 1), "The Green Seat"),
            numbered((2, 1), 28, (2, 1), "The Hand That Caresses the Night"),
            numbered((2, 11), 39, (3, 1), "Lightning's End"),
        ]);
        assert_eq!(
            name_of(&guide, Some(3), 1).as_deref(),
            Some("Lightning's End")
        );
        assert_eq!(
            name_of(&guide, Some(2), 1).as_deref(),
            Some("The Hand That Caresses the Night")
        );
    }

    /// Mushoku Tensei: AniList's second entry is the second half of TVDB's
    /// season one. A release numbered TVDB's way names season two `S02`, and
    /// the entry numbering must not answer it with that second half.
    #[test]
    fn the_entry_numbering_never_overrides_a_season_the_provider_has() {
        let guide = EpisodeGuide::new(vec![
            numbered((1, 12), 12, (2, 1), "Cour two's first"),
            numbered((2, 1), 26, (3, 1), "Season two's first"),
        ]);
        assert_eq!(
            name_of(&guide, Some(2), 1).as_deref(),
            Some("Season two's first")
        );
        // A second-season episode the provider does not list is unanswered,
        // not answered from the entry numbering.
        assert_eq!(guide.get(Some(2), 2), None);
    }

    #[test]
    fn a_file_that_states_its_season_is_answered_from_that_season() {
        let guide = EpisodeGuide::new(vec![
            episode(Some(1), 2, "Stray Dog Strut"),
            episode(Some(2), 2, "Somewhere else entirely"),
        ]);
        assert_eq!(
            guide.get(Some(1), 2).and_then(|e| e.name.clone()),
            Some("Stray Dog Strut".to_string())
        );
        // The fallback to season one must never reach a file that said it was
        // in season two — that would caption every episode with the wrong one.
        assert_eq!(guide.get(Some(3), 2), None);
    }

    #[test]
    fn an_absolutely_numbered_file_finds_an_absolutely_numbered_answer() {
        let guide = EpisodeGuide::new(vec![episode(None, 57, "The Immortal Legion")]);
        assert_eq!(
            guide.get(None, 57).and_then(|e| e.name.clone()),
            Some("The Immortal Legion".to_string())
        );
        // A provider that numbers straight through answers a file that states
        // season *one*: the release split nothing there.
        assert_eq!(guide.get(Some(1), 57).map(|e| e.number), Some(57));
    }

    /// **What captioned every episode of Oshi no Ko season two with season
    /// one's names.** AniList files a sequel as its own entry numbered from
    /// one, so its "episode 1" is season one's — and a file that says it is in
    /// season two must not be answered with it.
    #[test]
    fn an_absolutely_numbered_answer_never_reaches_a_later_season() {
        let guide = EpisodeGuide::new(vec![episode(None, 1, "Mother and Children")]);
        assert_eq!(guide.get(Some(2), 1), None);
        assert_eq!(guide.get(Some(3), 1), None);
        // And once the season's own entry has been fetched, it answers.
        let guide = EpisodeGuide::new(vec![
            episode(None, 1, "Mother and Children"),
            episode(Some(2), 1, "The Franchise"),
        ]);
        assert_eq!(
            guide.get(Some(2), 1).and_then(|e| e.name.clone()),
            Some("The Franchise".to_string())
        );
        assert_eq!(
            guide.get(None, 1).and_then(|e| e.name.clone()),
            Some("Mother and Children".to_string())
        );
    }

    #[test]
    fn an_absolutely_numbered_file_falls_back_to_the_only_season_there_is() {
        // TMDB files a 64-episode anime under one season; the release numbers
        // its files `#001`–`#064`. Without this they never meet.
        let guide = EpisodeGuide::new(vec![episode(Some(1), 57, "The Immortal Legion")]);
        assert_eq!(guide.get(None, 57).map(|e| e.number), Some(57));
        assert_eq!(guide.get(None, 99), None);
    }

    #[test]
    fn enrichment_is_off_until_it_is_asked_for() {
        assert!(!MetadataConfig::default().enabled);
    }

    #[test]
    fn provider_names_round_trip_through_the_catalog_column() {
        for provider in ProviderId::ALL {
            assert_eq!(ProviderId::parse(provider.as_str()), Some(provider));
        }
        assert_eq!(ProviderId::parse("letterboxd"), None);
    }

    /// A backdrop is the tile's shape and wins; a poster still gets used, but
    /// says it needs fitting rather than cropping.
    #[test]
    fn tile_art_prefers_the_landscape_picture() {
        let mut data = metadata();
        data.poster_url = Some("poster".into());
        assert_eq!(data.tile_art(), Some(("poster", ArtShape::Portrait)));

        data.backdrop_url = Some("backdrop".into());
        assert_eq!(data.tile_art(), Some(("backdrop", ArtShape::Landscape)));
    }

    /// A miss goes stale far sooner than a match: a show the provider had not
    /// indexed yet must not stay blank forever.
    #[test]
    fn a_miss_expires_sooner_than_a_match() {
        let now = 1_000_000_000;
        let miss = MetadataRecord {
            title_key: "k".into(),
            provider: ProviderId::AniList,
            metadata: None,
            fetched_at: now - MISS_TTL_SECS - 1,
            manual: false,
        };
        assert!(!miss.is_fresh(now, ProviderId::AniList));

        let matched = MetadataRecord {
            metadata: Some(metadata()),
            ..miss.clone()
        };
        assert!(matched.is_fresh(now, ProviderId::AniList));
    }

    /// What the viewer picked by hand outlives every TTL: it is not an answer
    /// that could go stale, it is an instruction.
    #[test]
    fn a_hand_picked_match_never_expires() {
        let record = MetadataRecord {
            title_key: "k".into(),
            provider: ProviderId::AniList,
            metadata: Some(metadata()),
            fetched_at: 0,
            manual: true,
        };
        assert!(record.is_fresh(MATCH_TTL_SECS * 100, ProviderId::AniList));
        // But it is still one provider's id, and it means nothing to the other.
        assert!(!record.is_fresh(MATCH_TTL_SECS * 100, ProviderId::Tmdb));
    }

    /// Switching provider must re-ask rather than trust the other one's answer —
    /// especially its silence.
    #[test]
    fn a_record_from_another_provider_is_never_fresh() {
        let record = MetadataRecord {
            title_key: "k".into(),
            provider: ProviderId::AniList,
            metadata: Some(metadata()),
            fetched_at: 1_000_000_000,
            manual: false,
        };
        assert!(!record.is_fresh(1_000_000_000, ProviderId::Tmdb));
    }
}
