//! What every metadata source has to be able to do.

use pstr_core::library::Title;
use pstr_core::metadata::{Enrichment, ProviderId, TitleMetadata};

use crate::error::Result;
use crate::matching::{Candidate, Query};

/// A source of titles.
///
/// Deliberately narrow: search, and enriching something search already
/// matched. Both are requests to a third party, and neither happens until the
/// viewer has turned enrichment on — but note what the second one does *not*
/// need: it takes the id search returned, so it sends nothing about the library
/// it did not already learn from the match.
///
/// Not `#[async_trait]`: this is only ever used through a concrete type or an
/// enum, never as `dyn Provider`, so the native `async fn` costs nothing here.
pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;

    /// Everything the provider thinks `query` might be, in its own order.
    ///
    /// Scoring is not the provider's job — it returns candidates and
    /// [`crate::matching::best`] decides. That is what keeps the "wrong poster"
    /// threshold in one place rather than one per provider.
    fn search(
        &self,
        query: &Query,
    ) -> impl std::future::Future<Output = Result<Vec<Candidate>>> + Send;

    /// The episodes, and any art the search answer lacked, of `found` — the
    /// entry `title` matched.
    ///
    /// `title` is there for its files: a provider that files each sequel as a
    /// separate entry (AniList) has to know which seasons the library holds to
    /// know how far along the franchise to look. An empty episode list is a
    /// real answer — a film has none — and the caller records it.
    fn enrich(
        &self,
        title: &Title,
        found: &TitleMetadata,
    ) -> impl std::future::Future<Output = Result<Enrichment>> + Send;
}
