//! One error type for the stream layer.

/// What can go wrong opening a stream or reading bytes out of it.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A share could not be opened, or its catalog could not be read.
    #[error(transparent)]
    Core(#[from] pstr_core::Error),

    /// The Drive SDK failed — network, API refusal, or decrypt.
    #[error(transparent)]
    Drive(#[from] proton_sdk::error::ProtonError),

    /// The block cache's filesystem failed.
    #[error("block cache: {0}")]
    Io(#[from] std::io::Error),

    /// The caller asked for something that does not exist.
    #[error("{0}")]
    NotFound(String),

    /// A failure that happened in *another* task.
    ///
    /// Block fetches are deduplicated: when several readers want the same block
    /// only one of them fetches it, and the rest wait. The waiters cannot be
    /// handed the leader's typed error — errors are not `Clone` — so they get
    /// its message. The leader itself always sees the real error.
    #[error("{0}")]
    Shared(String),
}

pub type Result<T> = std::result::Result<T, Error>;
