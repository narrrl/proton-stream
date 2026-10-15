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

    /// A public link with a password of its own was opened without one. The
    /// handshake refusal says nothing a viewer can act on; this does.
    #[error("This link has a password of its own. Enter it to add the link.")]
    PasswordNeeded,
}

impl Error {
    /// The message, followed by every cause its own text leaves out.
    ///
    /// A dropped connection reads as reqwest's "error sending request for url
    /// (…)" and nothing else: why — a reset, a DNS failure, a certificate — is
    /// further down the source chain. That is the part a viewer reporting a
    /// failed crawl needs, so the URL, which only says which folder, is left
    /// out to keep it on screen.
    pub fn describe(&self) -> String {
        let mut text = match self {
            Self::Drive(proton_sdk::error::ProtonError::Transport(error)) => {
                let url = error.url().map(|url| format!(" for url ({url})"));
                let own = self.to_string();
                url.map_or_else(|| own.clone(), |url| own.replace(&url, ""))
            }
            _ => self.to_string(),
        };
        let mut seen = self.to_string();
        let mut source = std::error::Error::source(self);
        while let Some(cause) = source {
            let line = cause.to_string();
            if !seen.contains(&line) {
                text.push_str(": ");
                text.push_str(&line);
                seen.push_str(&line);
            }
            source = cause.source();
        }
        text
    }

    /// Whether this is the account's session having ended: the refresh token
    /// was revoked or expired, the token lacks a scope only a password
    /// sign-in grants, or the stored key passphrases no longer unlock the
    /// account because its mailbox password changed. Nothing but signing in
    /// again fixes it.
    pub fn is_signed_out(&self) -> bool {
        match self {
            Self::SignedOut(_) => true,
            Self::Drive(proton_sdk::error::ProtonError::KeysLocked) => true,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("connection closed before message completed")]
    struct Closed;

    #[derive(Debug, thiserror::Error)]
    #[error("client error (SendRequest)")]
    struct SendRequest(#[source] Closed);

    #[test]
    fn a_failure_is_described_with_the_causes_its_own_text_leaves_out() {
        let error = Error::Io(std::io::Error::other(SendRequest(Closed)));
        assert_eq!(
            error.describe(),
            "io: client error (SendRequest): connection closed before message completed"
        );
    }

    #[test]
    fn stored_passphrases_that_no_longer_unlock_mean_signed_out() {
        let error = Error::Drive(proton_sdk::error::ProtonError::KeysLocked);
        assert!(error.is_signed_out());
    }
}
