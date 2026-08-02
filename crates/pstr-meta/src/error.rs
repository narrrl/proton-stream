//! One error type for looking a title up.

use pstr_core::metadata::ProviderId;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network: {0}")]
    Network(#[from] reqwest::Error),

    /// The provider answered, and the answer was not one.
    #[error("{0}")]
    Http(String),

    /// The provider is throttling.
    ///
    /// Its own variant because the caller must treat it differently from every
    /// other failure: a rate-limited lookup has to be retried later, never
    /// cached as "this title has no match". Caching a throttle would blank a
    /// title for days over a burst that lasted a minute.
    #[error("the provider is rate-limiting; try again shortly")]
    RateLimited,

    /// The chosen provider needs an API key and none is stored.
    #[error("{} needs an API key", .0.label())]
    MissingApiKey(ProviderId),

    /// Enrichment is off. Not a failure — the answer to "look this up" when the
    /// viewer has not agreed to lookups is that there is nothing to do.
    #[error("metadata lookups are turned off")]
    Disabled,

    #[error("config: {0}")]
    Config(String),

    #[error(transparent)]
    Core(#[from] pstr_core::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
