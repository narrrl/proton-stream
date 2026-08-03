//! Looking a title up, once, and remembering the answer.
//!
//! ```text
//!   Title ──▶ catalog cache ──hit──▶ MetadataRecord
//!               │ miss / stale
//!               ▼
//!            provider.search ──▶ matching::best ──▶ store ──▶ MetadataRecord
//! ```
//!
//! Two rules govern what gets written back, and they are the difference between
//! a library that settles down and one that hammers a third party forever:
//!
//! * **A miss is stored.** "The provider looked and had nothing" is an answer.
//!   Without storing it, every render of the grid re-asks for every unmatched
//!   title — the trap `proton-drive-linux`'s photo grid hit.
//! * **A failure is not.** A timeout, a 500, a rate-limit or a missing API key
//!   are failures to *ask*. Storing them as misses would blank a title for days
//!   over a minute of trouble, so they propagate and leave the title askable.
//!
//! Nothing here is requested unless [`MetadataConfig::enabled`] is set. See the
//! note in `pstr_core::metadata` on why that is off by default.

use std::sync::Arc;

use pstr_core::library::{Title, TitleKind, title_key};
use pstr_core::metadata::{
    EpisodeMetadata, MetadataConfig, MetadataRecord, ProviderId, TitleMetadata,
};

use crate::anilist::AniList;
use crate::error::{Error, Result};
use crate::matching::{self, Query};
use crate::provider::Provider;
use crate::tmdb::Tmdb;

/// How long to wait on a provider.
///
/// Short: this is decoration. A poster that takes fifteen seconds to arrive has
/// already lost its argument with the placeholder that is on screen.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Identifies this client to the providers, which both ask for it.
const USER_AGENT: &str = concat!("proton-stream/", env!("CARGO_PKG_VERSION"));

/// The configured provider, resolved.
enum Source {
    AniList(AniList),
    Tmdb(Tmdb),
}

impl Source {
    fn id(&self) -> ProviderId {
        match self {
            Self::AniList(provider) => provider.id(),
            Self::Tmdb(provider) => provider.id(),
        }
    }

    async fn search(&self, query: &Query) -> Result<Vec<matching::Candidate>> {
        match self {
            Self::AniList(provider) => provider.search(query).await,
            Self::Tmdb(provider) => provider.search(query).await,
        }
    }

    async fn episodes(&self, title: &TitleMetadata) -> Result<Vec<EpisodeMetadata>> {
        match self {
            Self::AniList(provider) => provider.episodes(title).await,
            Self::Tmdb(provider) => provider.episodes(title).await,
        }
    }

    fn seasons_are_separate_entries(&self) -> bool {
        match self {
            Self::AniList(provider) => provider.seasons_are_separate_entries(),
            Self::Tmdb(provider) => provider.seasons_are_separate_entries(),
        }
    }
}

/// Metadata lookups, against one provider.
///
/// Cheap to clone — the HTTP client inside is a connection pool that wants
/// sharing, not duplicating.
#[derive(Clone)]
pub struct MetadataService {
    source: Arc<Source>,
    http: reqwest::Client,
}

impl MetadataService {
    /// Build a service for `config`.
    ///
    /// `api_key` is only consulted for a provider that needs one; for AniList it
    /// is ignored, and passing `None` there is not an error.
    pub fn new(config: &MetadataConfig, api_key: Option<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(TIMEOUT)
            .build()
            .map_err(Error::Network)?;

        let source = match config.provider {
            ProviderId::AniList => Source::AniList(AniList::new(http.clone())),
            ProviderId::Tmdb => Source::Tmdb(Tmdb::new(
                http.clone(),
                api_key.ok_or(Error::MissingApiKey(ProviderId::Tmdb))?,
                config.language.clone(),
            )),
        };

        Ok(Self {
            source: Arc::new(source),
            http,
        })
    }

    pub fn provider(&self) -> ProviderId {
        self.source.id()
    }

    /// Ask the provider about one title.
    ///
    /// `Ok(None)` is a real answer — nothing matched well enough — and the
    /// caller should store it. An `Err` is not, and the caller should not.
    pub async fn lookup(&self, title: &Title) -> Result<Option<TitleMetadata>> {
        let mut query = Query::new(title.name.clone(), title.year, title.kind);
        // Nothing in the files numbered itself, so the kind came from counting
        // them — see `Query::kind_known`.
        if !title.states_its_numbering() {
            query = query.with_guessed_kind();
        }
        let candidates = self.source.search(&query).await?;
        let found = candidates.len();
        let best = matching::best(&query, candidates);

        tracing::debug!(
            "{}: {found} candidates from {}, {}",
            title.name,
            self.provider().label(),
            match &best {
                Some(found) => format!("matched {:?}", found.name),
                None => "no match".to_string(),
            }
        );
        Ok(best)
    }

    /// The same lookup, as a record ready to store — misses included.
    pub async fn record(&self, title: &Title) -> Result<MetadataRecord> {
        let metadata = self.lookup(title).await?;
        Ok(MetadataRecord {
            title_key: title_key(&title.name),
            provider: self.provider(),
            metadata,
            fetched_at: now(),
            manual: false,
        })
    }

    /// Everything the provider thinks `name` might be, unscored and in its own
    /// order.
    ///
    /// The escape hatch from [`matching::best`], for a viewer picking an entry
    /// by hand. Nothing is filtered here and nothing is ranked: the floor exists
    /// to stop the *matcher* guessing, and a person reading the list is not
    /// guessing. The `Fate/stay night [Heaven's Feel]` trilogy is the case that
    /// wants it — three films in one folder, matched against a provider that
    /// files each of them separately, where no single entry is the right answer
    /// and only the viewer knows which one they meant.
    ///
    /// Takes a kind because the providers key their search on it, and no year:
    /// the point of a hand search is that the library's own guesses are what
    /// went wrong.
    pub async fn search(&self, name: &str, kind: TitleKind) -> Result<Vec<TitleMetadata>> {
        let query = Query::new(name.trim().to_string(), None, kind).with_guessed_kind();
        if query.name.is_empty() {
            return Ok(Vec::new());
        }
        let candidates = self.source.search(&query).await?;
        tracing::debug!(
            "hand search {:?}: {} candidates from {}",
            query.name,
            candidates.len(),
            self.provider().label()
        );
        Ok(candidates
            .into_iter()
            .map(|candidate| candidate.metadata)
            .collect())
    }

    /// A record for an entry the viewer picked themselves.
    ///
    /// Marked [`MetadataRecord::manual`], which is what keeps the next match run
    /// — including a forced one — from undoing it.
    pub fn chosen(&self, title_key: String, found: TitleMetadata) -> MetadataRecord {
        MetadataRecord {
            title_key,
            provider: self.provider(),
            metadata: Some(found),
            fetched_at: now(),
            manual: true,
        }
    }

    /// What the provider lists as the episodes of a title it matched.
    ///
    /// Only ever called for a title that already has a match, because the
    /// provider's own id is what it takes — there is no second search here, and
    /// nothing about the library goes out that the match did not already send.
    /// An empty list is a real answer and belongs in the catalog: a film has no
    /// episodes, and asking again per render is the trap `MetadataRecord`
    /// exists to avoid.
    pub async fn episodes(&self, title: &TitleMetadata) -> Result<Vec<EpisodeMetadata>> {
        let episodes = self.source.episodes(title).await?;
        tracing::debug!(
            "{}: {} episodes from {}",
            title.name,
            episodes.len(),
            self.provider().label()
        );
        Ok(episodes)
    }

    /// Whether this provider files a sequel as its own entry, so that a title
    /// with several seasons has to be searched for once per season.
    pub fn splits_seasons(&self) -> bool {
        self.source.seasons_are_separate_entries()
    }

    /// The episodes of one season of `title`, for a provider that files each
    /// season separately.
    ///
    /// A second search, not a second guess: the title's own match is season
    /// one's entry, and asking it about episode one of season two would answer
    /// with episode one of season one. So the season is searched for by the
    /// name the provider itself uses — `Oshi no Ko 2nd Season`, and `Oshi no Ko
    /// Season 2` when that finds nothing, which are the two conventions between
    /// them covering everything AniList carries.
    ///
    /// A miss is an empty list rather than an error: a season the provider has
    /// never heard of is ordinary, and it leaves those rows named by their
    /// filenames exactly as they were before.
    pub async fn season_episodes(
        &self,
        title: &Title,
        season: u32,
    ) -> Result<Vec<EpisodeMetadata>> {
        if !self.splits_seasons() {
            return Ok(Vec::new());
        }

        let mut found = None;
        for name in season_names(&title.name, season) {
            // No year: a sequel airs years after the series the library named
            // the folder for, so carrying the title's year over would penalise
            // the very entry being looked for.
            let query = Query::new(name, None, title.kind);
            let candidates = self.source.search(&query).await?;
            if let Some(matched) = matching::best(&query, candidates) {
                found = Some(matched);
                break;
            }
        }

        let Some(found) = found else {
            tracing::debug!("{}: no entry for season {season}", title.name);
            return Ok(Vec::new());
        };

        let episodes = self.source.episodes(&found).await?;
        tracing::debug!(
            "{} season {season}: {} episodes from {:?}",
            title.name,
            episodes.len(),
            found.name
        );
        // The entry numbers its own episodes from one; what makes them season
        // two's is which entry they came from, and that has to be recorded here
        // or nothing can look them up again.
        Ok(episodes
            .into_iter()
            .map(|episode| EpisodeMetadata {
                season: Some(season),
                ..episode
            })
            .collect())
    }

    /// Download one piece of artwork.
    ///
    /// Separate from the lookup because artwork is fetched from a CDN on a
    /// different schedule to the metadata that names it — a cached record still
    /// needs its poster on a machine that has never had one.
    pub async fn artwork(&self, url: &str) -> Result<Vec<u8>> {
        let response = self.http.get(url).send().await?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Http(format!("artwork answered {status}")));
        }
        Ok(response.bytes().await?.to_vec())
    }
}

/// What to search for when looking for one season of a series, best first.
///
/// `2nd Season` is AniList's own convention and matches its romaji titles
/// outright; `Season 2` is what its English titles and most western libraries
/// use. Season one is never searched for this way — that is the title's own
/// match.
fn season_names(title: &str, season: u32) -> Vec<String> {
    vec![
        format!("{title} {} Season", ordinal(season)),
        format!("{title} Season {season}"),
    ]
}

/// `2` → `2nd`. English ordinals, because that is what the provider writes.
fn ordinal(number: u32) -> String {
    let suffix = match (number % 10, number % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{number}{suffix}")
}

/// Whether a stored answer can be used as-is.
pub fn is_usable(record: Option<&MetadataRecord>, provider: ProviderId) -> bool {
    record.is_some_and(|record| record.is_fresh(now(), provider))
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anilist_needs_no_api_key_and_tmdb_does() {
        let anilist = MetadataConfig {
            enabled: true,
            provider: ProviderId::AniList,
            ..MetadataConfig::default()
        };
        assert!(MetadataService::new(&anilist, None).is_ok());

        let tmdb = MetadataConfig {
            provider: ProviderId::Tmdb,
            ..anilist
        };
        assert!(matches!(
            MetadataService::new(&tmdb, None),
            Err(Error::MissingApiKey(ProviderId::Tmdb))
        ));
        assert!(MetadataService::new(&tmdb, Some("key".into())).is_ok());
    }

    /// The names a season is searched for by, in the order they are tried.
    #[test]
    fn a_season_is_searched_for_the_way_the_provider_names_it() {
        assert_eq!(
            season_names("Oshi no Ko", 2),
            vec!["Oshi no Ko 2nd Season", "Oshi no Ko Season 2"]
        );
        assert_eq!(ordinal(1), "1st");
        assert_eq!(ordinal(3), "3rd");
        assert_eq!(ordinal(4), "4th");
        // The teens are the ones a naive rule gets wrong, and a show does reach
        // them: `11th` is not `11st`.
        assert_eq!(ordinal(11), "11th");
        assert_eq!(ordinal(12), "12th");
        assert_eq!(ordinal(13), "13th");
        assert_eq!(ordinal(21), "21st");
    }

    /// A record from the other provider is never usable — switching provider has
    /// to re-ask, including about titles the previous one found nothing for.
    #[test]
    fn a_stored_answer_is_only_usable_for_the_provider_that_gave_it() {
        let record = MetadataRecord {
            title_key: "cowboy bebop".into(),
            provider: ProviderId::AniList,
            metadata: None,
            fetched_at: now(),
            manual: false,
        };
        assert!(is_usable(Some(&record), ProviderId::AniList));
        assert!(!is_usable(Some(&record), ProviderId::Tmdb));
        assert!(!is_usable(None, ProviderId::AniList));
    }
}
