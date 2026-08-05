//! One error type for everything this crate does.

/// What can go wrong opening a share, crawling it, or persisting what was found.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The Drive SDK failed — a network error, an API refusal, or a decrypt.
    #[error(transparent)]
    Drive(#[from] proton_sdk::error::ProtonError),

    /// The catalog database failed.
    #[error("catalog database: {0}")]
    Db(#[from] rusqlite::Error),

    /// The OS credential store failed.
    ///
    /// Boxed because `keyring::Error` is large and this variant is rare; an
    /// un-boxed one would inflate every `Result` in the crate.
    #[cfg(not(target_os = "android"))]
    #[error("credential store: {0}")]
    Keyring(#[from] Box<keyring::Error>),

    /// A config file could not be read, written or parsed.
    #[error("config: {0}")]
    Config(String),

    /// Filesystem failure outside of config handling.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// The caller asked for something that does not exist or does not apply.
    #[error("{0}")]
    NotFound(String),
}

#[cfg(not(target_os = "android"))]
impl From<keyring::Error> for Error {
    fn from(error: keyring::Error) -> Self {
        Self::Keyring(Box::new(error))
    }
}

pub type Result<T> = std::result::Result<T, Error>;
