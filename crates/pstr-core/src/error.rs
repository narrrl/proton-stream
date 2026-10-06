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

    /// Proton gated a sign-in behind a CAPTCHA. The front end presents
    /// [`HumanVerification::verification_url`](proton_sdk::api::HumanVerification::verification_url)
    /// and signs in again with the token it yields.
    #[error("Proton wants to confirm you are human before signing in")]
    HumanVerification(Box<proton_sdk::api::HumanVerification>),

    /// No account is signed in, or its session can no longer be refreshed and
    /// the viewer has to sign in again.
    #[error("{0}")]
    SignedOut(String),

    /// Signing in failed for a reason the viewer can act on, such as a wrong
    /// mailbox password.
    #[error("{0}")]
    SignIn(String),
}

impl Error {
    /// Whether this is the account's session having ended: the refresh token
    /// was revoked or expired, or the token lacks a scope only a password
    /// sign-in grants. Nothing but signing in again fixes it.
    pub fn is_signed_out(&self) -> bool {
        match self {
            Self::SignedOut(_) => true,
            Self::Drive(proton_sdk::error::ProtonError::Api(api)) => {
                api.is_invalid_refresh_token() || api.is_insufficient_scope()
            }
            _ => false,
        }
    }
}

#[cfg(not(target_os = "android"))]
impl From<keyring::Error> for Error {
    fn from(error: keyring::Error) -> Self {
        Self::Keyring(Box::new(error))
    }
}

pub type Result<T> = std::result::Result<T, Error>;
