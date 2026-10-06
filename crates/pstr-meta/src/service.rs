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

use std::collections::HashMap;
use std::sync::Arc;

use pstr_core::library::{Title, TitleKind, title_key};
use pstr_core::metadata::{
    Enrichment, MATCH_TTL_SECS, MetadataConfig, MetadataRecord, ProviderId, TitleMetadata,
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

    /// The entry a stored match names, fetched again by its id. `None` from a
    /// provider that has no details to add.
    async fn entry(&self, id: &str) -> Result<Option<TitleMetadata>> {
        match self {
            Self::AniList(provider) => provider.entry(id).await,
            Self::Tmdb(_) => Ok(None),
        }
    }

    /// Whether [`Source::entry`] has anything to give.
    fn has_details(&self) -> bool {
        matches!(self, Self::AniList(_))
    }

    async fn enrich(&self, title: &Title, found: &TitleMetadata) -> Result<Enrichment> {
        match self {
            Self::AniList(provider) => provider.enrich(title, found).await,
            Self::Tmdb(provider) => provider.enrich(title, found).await,
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
            ProviderId::AniList => {
                Source::AniList(AniList::new(http.clone(), config.language.clone()))
            }
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

    /// The episodes of a matched title, and any art its match lacked.
    ///
    /// Only ever called for a title that already has a match, because the
    /// provider's own id is what it takes — there is no second search here, and
    /// nothing about the library goes out that the match did not already send.
    ///
    /// An `Err` is a failure to ask and must not be stored: the title stays
    /// un-enriched and the next match run asks again. An empty answer is an
    /// answer, and is — see `Catalog::set_enrichment`.
    pub async fn enrich(&self, title: &Title, found: &TitleMetadata) -> Result<Enrichment> {
        let enrichment = self.source.enrich(title, found).await?;
        tracing::debug!(
            "{}: {} episodes from {}{}",
            title.name,
            enrichment.episodes.len(),
            self.provider().label(),
            if enrichment.backdrop_url.is_some() {
                ", with a backdrop"
            } else {
                ""
            }
        );
        Ok(enrichment)
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

/// What one match run has to do about one title.
#[derive(Debug, Clone)]
pub enum Work {
    /// Search for it, and enrich it if it matches.
    Match(Title),
    /// It is already matched; only its enrichment is missing or old.
    Enrich(Title, Box<TitleMetadata>),
    /// It is matched, but stored before [`TitleMetadata::details`] existed:
    /// fetch the same entry again by id — never searched for, so a match the
    /// viewer picked by hand stays theirs — and enrich it too if `enrich`.
    Refresh {
        title: Title,
        record: Box<MetadataRecord>,
        enrich: bool,
    },
}

impl Work {
    pub fn title(&self) -> &Title {
        match self {
            Self::Match(title) | Self::Enrich(title, _) | Self::Refresh { title, .. } => title,
        }
    }
}

/// What a piece of [`Work`] found, ready to store.
#[derive(Debug)]
pub struct Outcome {
    pub title_key: String,
    /// The new answer, for [`Work::Match`] and [`Work::Refresh`]. Store it —
    /// misses included.
    pub record: Option<MetadataRecord>,
    /// What enriching the match found, when it was asked and answered. `None`
    /// after a failure, which must leave the title un-enriched so the next run
    /// asks again.
    pub enrichment: Option<Enrichment>,
}

/// Decide what a match run over `titles` has to do.
///
/// Three kinds of work, and the difference is requests saved: a title that
/// already has a good match but has not been enriched — or was enriched longer
/// ago than a match is trusted — needs enriching only, and searching for it
/// again would ask the provider something it has already answered. One whose
/// match predates its details is fetched again by id, which is one request
/// and decides nothing.
///
/// A hand-picked entry is never searched for again, not even when `force`d —
/// that means "the automatic answers are wrong", and re-deciding the one title
/// the viewer already fixed by hand is the opposite of what they asked for. It
/// is still enriched and refreshed.
///
/// `details` is whether the provider has [`TitleMetadata::details`] to give
/// — see [`MetadataService::has_details`].
pub fn plan(
    titles: Vec<Title>,
    stored: &HashMap<String, MetadataRecord>,
    enriched: &HashMap<String, (ProviderId, i64)>,
    provider: ProviderId,
    details: bool,
    force: bool,
) -> Vec<Work> {
    let now = now();
    let mut work = Vec::new();
    for title in titles {
        let record = stored.get(&title.key);
        let pinned = record.is_some_and(|record| record.manual && record.provider == provider);
        if !pinned && (force || !is_usable(record, provider)) {
            work.push(Work::Match(title));
            continue;
        }
        let Some(record) = record.filter(|record| record.metadata.is_some()) else {
            continue;
        };
        let fresh = enriched.get(&title.key).is_some_and(|(asked, at)| {
            *asked == provider && now.saturating_sub(*at) < MATCH_TTL_SECS
        });
        let stale_details = details
            && record
                .metadata
                .as_ref()
                .is_some_and(|found| found.details.is_none());
        if stale_details {
            work.push(Work::Refresh {
                title,
                record: Box::new(record.clone()),
                enrich: !fresh,
            });
        } else if !fresh && let Some(found) = record.metadata.clone() {
            work.push(Work::Enrich(title, Box::new(found)));
        }
    }
    work
}

impl MetadataService {
    /// Whether this provider fills in [`TitleMetadata::details`], and so
    /// whether a match stored without them is worth fetching again.
    pub fn has_details(&self) -> bool {
        self.source.has_details()
    }

    /// Do one piece of [`Work`].
    ///
    /// A failed search is an `Err` and nothing is stored. A failed enrichment
    /// after a successful search is not: the match is worth keeping, and the
    /// title is simply left un-enriched — a poster and a synopsis that arrived
    /// are worth showing even when the episode request was the one that hit the
    /// rate limit.
    pub async fn work(&self, work: &Work) -> Result<Outcome> {
        match work {
            Work::Match(title) => {
                let record = self.record(title).await?;
                let enrichment = match &record.metadata {
                    Some(found) => self.enrich_or_log(title, found).await,
                    None => None,
                };
                Ok(Outcome {
                    title_key: record.title_key.clone(),
                    record: Some(record),
                    enrichment,
                })
            }
            Work::Enrich(title, found) => Ok(Outcome {
                title_key: title.key.clone(),
                record: None,
                enrichment: Some(self.enrich(title, found).await?),
            }),
            Work::Refresh {
                title,
                record,
                enrich,
            } => {
                let Some(stored) = &record.metadata else {
                    return Ok(Outcome {
                        title_key: title.key.clone(),
                        record: None,
                        enrichment: None,
                    });
                };
                // An id the provider no longer has keeps what was stored, with
                // empty details so it is not asked about on every run.
                let found = self
                    .source
                    .entry(&stored.remote_id)
                    .await?
                    .unwrap_or_else(|| TitleMetadata {
                        details: Some(Default::default()),
                        ..stored.clone()
                    });
                let enrichment = match enrich {
                    true => self.enrich_or_log(title, &found).await,
                    false => None,
                };
                Ok(Outcome {
                    title_key: title.key.clone(),
                    record: Some(MetadataRecord {
                        metadata: Some(found),
                        fetched_at: now(),
                        ..(**record).clone()
                    }),
                    enrichment,
                })
            }
        }
    }

    /// Pin `title` to the entry the viewer picked, and enrich it.
    ///
    /// Both halves, always: the point of choosing an entry by hand is usually
    /// that the episode names were wrong too, so a choice that stored only the
    /// title record left the episodes, the backdrop and the franchise chain of
    /// the *old* match — or none — until some later match run got round to it.
    /// A failed enrichment still keeps the choice, as in [`Self::work`].
    pub async fn choose(&self, title: &Title, found: TitleMetadata) -> Outcome {
        let enrichment = self.enrich_or_log(title, &found).await;
        Outcome {
            title_key: title.key.clone(),
            record: Some(self.chosen(title.key.clone(), found)),
            enrichment,
        }
    }

    async fn enrich_or_log(&self, title: &Title, found: &TitleMetadata) -> Option<Enrichment> {
        self.enrich(title, found)
            .await
            .inspect_err(|error| tracing::warn!("enrich {}: {error}", title.name))
            .ok()
    }
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

    fn title(key: &str) -> Title {
        Title {
            key: key.into(),
            name: key.into(),
            year: None,
            kind: TitleKind::Series,
            seasons: Vec::new(),
            share_ids: Vec::new(),
        }
    }

    fn matched(key: &str, manual: bool) -> MetadataRecord {
        MetadataRecord {
            title_key: key.into(),
            provider: ProviderId::AniList,
            metadata: Some(TitleMetadata {
                provider: ProviderId::AniList,
                remote_id: "1".into(),
                name: key.into(),
                original_name: None,
                overview: None,
                year: None,
                kind: TitleKind::Series,
                poster_url: None,
                backdrop_url: None,
                rating: None,
                genres: Vec::new(),
                episodes: None,
                url: None,
                details: None,
            }),
            fetched_at: now(),
            manual,
        }
    }

    fn kinds(work: &[Work]) -> Vec<(&str, &str)> {
        work.iter()
            .map(|work| match work {
                Work::Match(title) => (title.key.as_str(), "match"),
                Work::Enrich(title, _) => (title.key.as_str(), "enrich"),
                Work::Refresh { title, .. } => (title.key.as_str(), "refresh"),
            })
            .collect()
    }

    /// A matched title is enriched once and then left alone until its
    /// enrichment is as old as a match is trusted; an unasked title is matched.
    #[test]
    fn a_run_matches_the_unasked_and_enriches_the_unenriched() {
        let stored = HashMap::from([
            ("enriched".to_string(), matched("enriched", false)),
            ("stale".to_string(), matched("stale", false)),
            ("bare".to_string(), matched("bare", false)),
        ]);
        let enriched = HashMap::from([
            ("enriched".to_string(), (ProviderId::AniList, now())),
            (
                "stale".to_string(),
                (ProviderId::AniList, now() - MATCH_TTL_SECS - 1),
            ),
        ]);
        let titles = ["enriched", "stale", "bare", "new"].map(title).to_vec();

        let work = plan(
            titles,
            &stored,
            &enriched,
            ProviderId::AniList,
            false,
            false,
        );
        assert_eq!(
            kinds(&work),
            vec![("stale", "enrich"), ("bare", "enrich"), ("new", "match")]
        );
    }

    /// "Match again" re-searches everything except what the viewer pinned —
    /// and a pinned title that was never enriched is still enriched.
    #[test]
    fn a_forced_run_leaves_a_pinned_match_alone_but_still_enriches_it() {
        let stored = HashMap::from([
            ("pinned".to_string(), matched("pinned", true)),
            ("automatic".to_string(), matched("automatic", false)),
        ]);
        let titles = ["pinned", "automatic"].map(title).to_vec();

        let work = plan(
            titles,
            &stored,
            &HashMap::new(),
            ProviderId::AniList,
            false,
            true,
        );
        assert_eq!(
            kinds(&work),
            vec![("pinned", "enrich"), ("automatic", "match")]
        );
    }

    /// A match stored before details existed is fetched again by id — a
    /// pinned one included, and without being searched for — and only by a
    /// provider that has details to give.
    #[test]
    fn a_match_without_details_is_refreshed_rather_than_searched_for() {
        let stored = HashMap::from([
            ("pinned".to_string(), matched("pinned", true)),
            ("automatic".to_string(), matched("automatic", false)),
        ]);
        let enriched = HashMap::from([("pinned".to_string(), (ProviderId::AniList, now()))]);
        let titles = ["pinned", "automatic"].map(title).to_vec();

        let work = plan(
            titles.clone(),
            &stored,
            &enriched,
            ProviderId::AniList,
            true,
            false,
        );
        assert_eq!(
            kinds(&work),
            vec![("pinned", "refresh"), ("automatic", "refresh")]
        );
        assert!(matches!(&work[0], Work::Refresh { enrich: false, .. }));
        assert!(matches!(&work[1], Work::Refresh { enrich: true, .. }));

        let work = plan(
            titles,
            &stored,
            &enriched,
            ProviderId::AniList,
            false,
            false,
        );
        assert_eq!(kinds(&work), vec![("automatic", "enrich")]);
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
