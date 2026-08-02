//! What every metadata source has to be able to do.

use pstr_core::metadata::{EpisodeMetadata, ProviderId, TitleMetadata};

use crate::error::Result;
use crate::matching::{Candidate, Query};

/// A source of titles.
///
/// Deliberately narrow: search, and the episode list of something search
/// already matched. Both are requests to a third party, and neither happens
/// until the viewer has turned enrichment on — but note what the second one
/// does *not* need: it takes the id search returned, so it sends the provider
/// nothing about the library it did not already learn from the match.
///
/// Not `#[async_trait]`: this is only ever used through a concrete type or an
/// enum, never as `dyn Provider`, so the native `async fn` costs nothing here.
pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;

    /// Whether a sequel is a *separate entry* rather than a second season of
    /// the same one.
    ///
    /// AniList works that way — `Oshi no Ko`, `Oshi no Ko 2nd Season` and
    /// `Oshi no Ko 3rd Season` are three ids, each numbering its episodes from
    /// one — so a library that files all three under one title has to search
    /// again per season or seasons two upwards get no episode names at all (and,
    /// worse, would be answered with season one's). TMDB files them as seasons
    /// of one show and its episode list already carries season numbers, so
    /// searching per season there would only find the wrong entry.
    fn seasons_are_separate_entries(&self) -> bool;

    /// Everything the provider thinks `query` might be, in its own order.
    ///
    /// Scoring is not the provider's job — it returns candidates and
    /// [`crate::matching::best`] decides. That is what keeps the "wrong poster"
    /// threshold in one place rather than one per provider.
    fn search(
        &self,
        query: &Query,
    ) -> impl std::future::Future<Output = Result<Vec<Candidate>>> + Send;

    /// Every episode the provider lists for a title it already matched.
    ///
    /// An empty list is a real answer — a film has no episodes, and plenty of
    /// series entries have none listed — and the caller caches it rather than
    /// asking again on the next render.
    fn episodes(
        &self,
        title: &TitleMetadata,
    ) -> impl std::future::Future<Output = Result<Vec<EpisodeMetadata>>> + Send;
}
