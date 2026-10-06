//! Signing in to a Proton account, and keeping the session.
//!
//! The rest of the app works without one — public links need no account. A
//! signed-in account adds two things: folders of the viewer's own Drive (My
//! Files, other devices, what was shared with them) as library shares, and
//! watch history kept in their Drive so every device resumes where another
//! left off ([`crate::sync`]).
//!
//! ## What is kept, and where
//!
//! Following `proton-drive-linux`: one JSON blob in the [`SecretStore`] holding
//! the session tokens, the mailbox password and the account's key salts. The
//! mailbox password is what unlocks the key chain, so without it a resumed
//! session could list nothing. The key salts are kept because
//! `core/v4/keys/salts` needs a scope only a password sign-in grants: after the
//! first token refresh it answers 403, and a session resumed without them
//! could never unlock its keys again.
//!
//! Proton refresh tokens are single-use, so every refresh is written back
//! (see [`persist_refreshed_tokens`]). A refresh that is not leaves the store
//! holding a dead token and the next launch signed out.
//!
//! ## Signing in is several steps
//!
//! Password, then maybe a second-factor code, then — on an account with a
//! separate mailbox password — that. Each front end asks for them in its own
//! way, so [`AccountStore::sign_in`] returns a [`SignIn`] saying what it needs
//! next and holding the half-made session until it is given.

use std::sync::Arc;

use proton_drive_rs::{KeySalt, NodeKind, ProtonDriveClient};
use proton_sdk::api::HumanVerificationCredential;
use proton_sdk::error::ProtonError;
use proton_sdk::ids::NodeUid;
use proton_sdk::session::{PasswordMode, ProtonApiSession, ResumeParameters};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::shares::{SecretStore, client_configuration};

/// The secret-store key the session is kept under. Share secrets are keyed
/// `share-…`, so the two cannot collide.
const SESSION_KEY: &str = "proton-account";

/// Blocks in flight for streaming from the account, as for a public link —
/// see `ShareStore::open`.
const MAX_INFLIGHT_BLOCKS: usize = 48;

/// Everything needed to resume the session unattended.
#[derive(Serialize, Deserialize, Clone)]
struct StoredSession {
    session_id: String,
    username: String,
    user_id: String,
    access_token: String,
    refresh_token: String,
    scopes: Vec<String>,
    /// `1` single password, `2` separate mailbox password — Proton's values.
    password_mode: u8,
    mailbox_password: String,
    key_salts: Vec<KeySalt>,
}

impl StoredSession {
    fn capture(
        session: &ProtonApiSession,
        tokens: proton_sdk::http::Tokens,
        mailbox_password: &str,
        key_salts: Vec<KeySalt>,
    ) -> Self {
        Self {
            session_id: session.session_id().as_str().to_owned(),
            username: session.username().to_owned(),
            user_id: session.user_id().as_str().to_owned(),
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            scopes: session.scopes().to_vec(),
            password_mode: match session.password_mode() {
                PasswordMode::Single => 1,
                PasswordMode::Dual => 2,
            },
            mailbox_password: mailbox_password.to_owned(),
            key_salts,
        }
    }

    fn resume_parameters(&self) -> ResumeParameters {
        ResumeParameters {
            session_id: self.session_id.clone().into(),
            username: self.username.clone(),
            user_id: self.user_id.clone().into(),
            access_token: self.access_token.clone(),
            refresh_token: self.refresh_token.clone(),
            scopes: self.scopes.clone(),
            is_waiting_for_second_factor_code: false,
            password_mode: match self.password_mode {
                1 => PasswordMode::Single,
                _ => PasswordMode::Dual,
            },
        }
    }
}

/// A signed-in account, ready for Drive work.
///
/// Cheap to clone; clones share one session and one set of caches.
#[derive(Clone)]
pub struct Account {
    session: ProtonApiSession,
    drive: ProtonDriveClient,
}

impl Account {
    /// The address the viewer signed in with.
    pub fn username(&self) -> &str {
        self.session.username()
    }

    /// The Drive client the account's folders and watch history go through.
    pub fn drive(&self) -> &ProtonDriveClient {
        &self.drive
    }

    /// Where a library folder can come from: My Files, each device, and each
    /// folder someone shared with the viewer.
    ///
    /// The device this app keeps watch history in is left out — it holds
    /// nothing to watch. A device or share whose name will not decrypt is
    /// still listed, under a placeholder, rather than hidden: it is the
    /// viewer's, and they can still open it.
    pub async fn places(&self) -> Result<Vec<DrivePlace>> {
        let mut places = Vec::new();
        let my_files = self.drive.get_my_files_folder().await?;
        places.push(DrivePlace {
            kind: PlaceKind::MyFiles,
            name: "My files".to_owned(),
            uid: my_files.uid,
        });

        let mut devices = self.drive.enumerate_devices().await?;
        devices.sort_by_key(|device| device.creation_time);
        for device in devices {
            let name = device.name.unwrap_or_else(|_| "Unnamed device".to_owned());
            if name == crate::sync::DEVICE_NAME {
                continue;
            }
            places.push(DrivePlace {
                kind: PlaceKind::Device,
                name,
                uid: device.root_folder_uid,
            });
        }

        let shared: Vec<NodeUid> = self
            .drive
            .enumerate_shared_with_me()
            .await?
            .into_iter()
            .map(|item| item.uid)
            .collect();
        if !shared.is_empty() {
            let mut nodes = self.drive.enumerate_nodes(&shared).await?;
            nodes.retain(|node| node.is_folder() && !node.trashed);
            nodes.sort_by_key(|node| node.name.to_lowercase());
            places.extend(nodes.into_iter().map(|node| DrivePlace {
                kind: PlaceKind::SharedWithMe,
                name: node.name,
                uid: node.uid,
            }));
        }
        Ok(places)
    }

    /// What one folder of the account holds: folders first, then files, each
    /// by name. Trashed nodes are left out.
    pub async fn folder_children(&self, folder: &NodeUid) -> Result<Vec<DriveEntry>> {
        let uids = self
            .drive
            .enumerate_folder_children_node_uids(folder)
            .await?;
        if uids.is_empty() {
            return Ok(Vec::new());
        }
        let mut entries: Vec<DriveEntry> = self
            .drive
            .enumerate_nodes(&uids)
            .await?
            .into_iter()
            .filter(|node| !node.trashed)
            .map(|node| {
                let (size, media_type) = match &node.kind {
                    NodeKind::File {
                        media_type,
                        claimed_size,
                        ..
                    } => (*claimed_size, Some(media_type.clone())),
                    _ => (None, None),
                };
                DriveEntry {
                    is_folder: node.is_folder(),
                    uid: node.uid,
                    name: node.name,
                    size,
                    media_type,
                }
            })
            .collect();
        entries.sort_by(|a, b| {
            b.is_folder
                .cmp(&a.is_folder)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    }

    /// End the session on Proton's side. Best-effort: a session that is
    /// already dead has nothing to end.
    async fn end(&self) {
        if let Err(error) = self.session.end().await {
            tracing::debug!(%error, "ending the Proton session failed");
        }
    }
}

/// Which kind of place a [`DrivePlace`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceKind {
    MyFiles,
    Device,
    SharedWithMe,
}

/// A top-level place in the account's Drive.
#[derive(Debug, Clone)]
pub struct DrivePlace {
    pub kind: PlaceKind,
    pub name: String,
    pub uid: NodeUid,
}

/// One node of a folder listing.
#[derive(Debug, Clone)]
pub struct DriveEntry {
    pub uid: NodeUid,
    pub name: String,
    pub is_folder: bool,
    /// Plaintext size, when the uploader recorded one.
    pub size: Option<i64>,
    pub media_type: Option<String>,
}

/// Where a sign-in stands.
pub enum SignIn {
    /// Signed in; the session is stored.
    Done(Account),
    /// The account has two-factor authentication: ask for a code and pass it to
    /// [`PendingSignIn::second_factor`].
    SecondFactor(PendingSignIn),
    /// The account has a separate mailbox password: ask for it and pass it to
    /// [`PendingSignIn::mailbox_password`].
    MailboxPassword(PendingSignIn),
}

/// A sign-in waiting on the viewer.
pub struct PendingSignIn {
    secrets: Arc<dyn SecretStore>,
    session: ProtonApiSession,
    password: String,
}

impl PendingSignIn {
    /// Submit a second-factor code.
    ///
    /// A wrong code fails here and leaves nothing behind; the front end starts
    /// the sign-in over, because Proton does not accept a second try on the
    /// same half-made session.
    pub async fn second_factor(mut self, code: &str) -> Result<SignIn> {
        self.session
            .apply_second_factor_code(code.trim())
            .await
            .map_err(|error| match error {
                ProtonError::Api(api) if (400..500).contains(&api.http_status) => {
                    Error::SignIn("that code was not accepted".to_owned())
                }
                other => other.into(),
            })?;
        self.after_second_factor().await
    }

    /// Submit the mailbox password, on an account that has one.
    ///
    /// By reference, unlike the code: a mistyped mailbox password is checked
    /// on this side, the session survives it, and the viewer can try again
    /// without starting over.
    pub async fn mailbox_password(&self, mailbox_password: &str) -> Result<Account> {
        finish(
            Arc::clone(&self.secrets),
            self.session.clone(),
            mailbox_password,
        )
        .await
    }

    async fn after_second_factor(self) -> Result<SignIn> {
        if self.session.password_mode() == PasswordMode::Dual {
            return Ok(SignIn::MailboxPassword(self));
        }
        let password = self.password.clone();
        finish(self.secrets, self.session, &password)
            .await
            .map(SignIn::Done)
    }
}

/// The signed-in account, if any, and signing in and out.
#[derive(Clone)]
pub struct AccountStore {
    secrets: Arc<dyn SecretStore>,
}

impl AccountStore {
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self { secrets }
    }

    /// The address of the stored account, without touching the network.
    pub fn username(&self) -> Result<Option<String>> {
        Ok(self.load()?.map(|stored| stored.username))
    }

    /// Start signing in.
    ///
    /// `verification` is the token from a solved CAPTCHA, after a first try
    /// failed with [`Error::HumanVerification`]; the sign-in starts over with
    /// it, because the gated attempt's SRP handshake is spent.
    pub async fn sign_in(
        &self,
        username: &str,
        password: &str,
        verification: Option<HumanVerificationCredential>,
    ) -> Result<SignIn> {
        let session = ProtonApiSession::begin_verified(
            client_configuration(),
            username.trim(),
            password.as_bytes(),
            verification.as_ref(),
        )
        .await
        .map_err(classify_sign_in_failure)?;
        let pending = PendingSignIn {
            secrets: Arc::clone(&self.secrets),
            session,
            password: password.to_owned(),
        };
        if pending.session.is_waiting_for_second_factor() {
            return Ok(SignIn::SecondFactor(pending));
        }
        pending.after_second_factor().await
    }

    /// The stored account, resumed, or `None` when nobody is signed in.
    ///
    /// Makes no request: a session whose refresh token has died is found out
    /// by the first call that uses it, as [`Error::is_signed_out`].
    pub fn resume(&self) -> Result<Option<Account>> {
        let Some(stored) = self.load()? else {
            return Ok(None);
        };
        let session = ProtonApiSession::resume(client_configuration(), stored.resume_parameters())?;
        persist_refreshed_tokens(
            &self.secrets,
            &session,
            stored.mailbox_password.clone(),
            stored.key_salts.clone(),
        );
        let drive = ProtonDriveClient::with_key_salts(
            &session,
            stored.mailbox_password.into_bytes(),
            stored.key_salts,
        );
        Ok(Some(account(session, drive)))
    }

    /// Forget the account: end its session and delete what was stored.
    ///
    /// The stored session goes even when ending it fails — signing out on a
    /// plane must still sign out.
    pub async fn sign_out(&self, account: Option<&Account>) -> Result<()> {
        if let Some(account) = account {
            account.end().await;
        }
        self.secrets.delete(SESSION_KEY)
    }

    fn load(&self) -> Result<Option<StoredSession>> {
        let Some(encoded) = self.secrets.get(SESSION_KEY)? else {
            return Ok(None);
        };
        serde_json::from_str(&encoded)
            .map(Some)
            .map_err(|e| Error::Config(format!("the stored Proton session is unreadable: {e}")))
    }
}

/// Unlock the key chain with `mailbox_password`, store the session, and hand
/// back the account.
///
/// The key salts are read here, while the token still has the scope that
/// endpoint needs. Loading the addresses is what proves the mailbox password:
/// it unlocks the user keys, and a wrong password unlocks none of them.
async fn finish(
    secrets: Arc<dyn SecretStore>,
    session: ProtonApiSession,
    mailbox_password: &str,
) -> Result<Account> {
    let probe = ProtonDriveClient::new(&session, mailbox_password.as_bytes().to_vec());
    let key_salts = probe.account().key_salts().await?;
    probe
        .account()
        .addresses()
        .await
        .map_err(|error| match error {
            ProtonError::InvalidOperation(_) => {
                Error::SignIn("that mailbox password did not unlock the account".to_owned())
            }
            other => other.into(),
        })?;

    let stored = StoredSession::capture(
        &session,
        session.current_tokens().await,
        mailbox_password,
        key_salts.clone(),
    );
    let encoded = serde_json::to_string(&stored)
        .map_err(|e| Error::Config(format!("serialize the Proton session: {e}")))?;
    secrets.set(SESSION_KEY, &encoded)?;
    persist_refreshed_tokens(
        &secrets,
        &session,
        mailbox_password.to_owned(),
        key_salts.clone(),
    );

    let drive = ProtonDriveClient::with_key_salts(
        &session,
        mailbox_password.as_bytes().to_vec(),
        key_salts,
    );
    Ok(account(session, drive))
}

fn account(session: ProtonApiSession, drive: ProtonDriveClient) -> Account {
    let drive = drive
        .with_max_inflight_blocks(MAX_INFLIGHT_BLOCKS)
        // A watch-history file is one small block; the single-request upload
        // is one failure point where the draft dance is three.
        .with_small_file_upload(true);
    Account { session, drive }
}

/// Write every refreshed token pair back to the secret store.
///
/// Refresh tokens are single-use: once the HTTP client has spent the stored
/// one, only the pair it got back can resume the session.
fn persist_refreshed_tokens(
    secrets: &Arc<dyn SecretStore>,
    session: &ProtonApiSession,
    mailbox_password: String,
    key_salts: Vec<KeySalt>,
) {
    let secrets = Arc::clone(secrets);
    let template = session.clone();
    session.http().set_on_tokens_refreshed(move |tokens| {
        let stored =
            StoredSession::capture(&template, tokens, &mailbox_password, key_salts.clone());
        let written = serde_json::to_string(&stored)
            .map_err(|e| Error::Config(e.to_string()))
            .and_then(|encoded| secrets.set(SESSION_KEY, &encoded));
        if let Err(error) = written {
            tracing::warn!(%error, "the refreshed Proton session could not be stored");
        }
    });
}

/// Lift a CAPTCHA gate and a wrong password out of the generic API error.
///
/// Only a gate that can be solved in an app becomes
/// [`Error::HumanVerification`]; one that wants an e-mail or SMS code is a plain
/// failure, which reads better than an empty verification page.
fn classify_sign_in_failure(error: ProtonError) -> Error {
    if let ProtonError::Api(api) = &error {
        if let Some(challenge) = api.human_verification()
            && challenge.supports_captcha()
        {
            return Error::HumanVerification(Box::new(challenge));
        }
        if api.http_status == 422 && !api.is_human_verification_required() {
            return Error::SignIn(if api.message.is_empty() {
                "Proton did not accept that address and password".to_owned()
            } else {
                api.message.clone()
            });
        }
    }
    error.into()
}

/// The token out of a message posted by Proton's verification page, or `None`
/// for anything that is not a completion.
///
/// The page posts plenty besides — resize requests, readiness pings — and
/// taking an unrecognised message for success would retry the sign-in with a
/// garbage token. Both spellings of the success type have shipped, and some
/// hosts hand the message over stringified twice.
pub fn verification_token(raw: &str) -> Option<String> {
    let mut value: serde_json::Value = serde_json::from_str(raw).ok()?;
    if let Some(inner) = value.as_str()
        && let Ok(parsed) = serde_json::from_str(inner)
    {
        value = parsed;
    }
    let kind = value.get("type")?.as_str()?;
    if !matches!(
        kind,
        "HUMAN_VERIFICATION_SUCCESS" | "human_verification_success"
    ) {
        return None;
    }
    let token = value.get("payload")?.get("token")?.as_str()?;
    (!token.is_empty()).then(|| token.to_owned())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use parking_lot::Mutex;

    use super::*;

    #[derive(Default)]
    struct MemorySecrets(Mutex<HashMap<String, String>>);

    impl SecretStore for MemorySecrets {
        fn set(&self, key: &str, value: &str) -> Result<()> {
            self.0.lock().insert(key.to_owned(), value.to_owned());
            Ok(())
        }
        fn get(&self, key: &str) -> Result<Option<String>> {
            Ok(self.0.lock().get(key).cloned())
        }
        fn delete(&self, key: &str) -> Result<()> {
            self.0.lock().remove(key);
            Ok(())
        }
    }

    fn stored() -> StoredSession {
        StoredSession {
            session_id: "session".to_owned(),
            username: "viewer@proton.me".to_owned(),
            user_id: "user".to_owned(),
            access_token: "access".to_owned(),
            refresh_token: "refresh".to_owned(),
            scopes: vec!["drive".to_owned()],
            password_mode: 2,
            mailbox_password: "mailbox".to_owned(),
            key_salts: Vec::new(),
        }
    }

    #[test]
    fn nobody_is_signed_in_until_a_session_is_stored() {
        let store = AccountStore::new(Arc::new(MemorySecrets::default()));
        assert_eq!(store.username().expect("read"), None);
        assert!(store.resume().expect("resume").is_none());
    }

    #[test]
    fn a_stored_session_resumes_as_its_account_without_a_request() {
        let secrets = Arc::new(MemorySecrets::default());
        secrets
            .set(SESSION_KEY, &serde_json::to_string(&stored()).unwrap())
            .unwrap();
        let store = AccountStore::new(secrets);
        assert_eq!(
            store.username().expect("read").as_deref(),
            Some("viewer@proton.me")
        );
        let account = store.resume().expect("resume").expect("signed in");
        assert_eq!(account.username(), "viewer@proton.me");
    }

    #[tokio::test]
    async fn signing_out_forgets_the_stored_session() {
        let secrets = Arc::new(MemorySecrets::default());
        secrets
            .set(SESSION_KEY, &serde_json::to_string(&stored()).unwrap())
            .unwrap();
        let store = AccountStore::new(secrets);
        store.sign_out(None).await.expect("sign out");
        assert_eq!(store.username().expect("read"), None);
    }

    #[test]
    fn a_stored_session_keeps_its_password_mode() {
        let params = stored().resume_parameters();
        assert_eq!(params.password_mode, PasswordMode::Dual);
        assert!(!params.is_waiting_for_second_factor_code);
    }

    #[test]
    fn a_verification_completion_yields_its_token() {
        let raw =
            r#"{"type":"HUMAN_VERIFICATION_SUCCESS","payload":{"token":"tok-1","type":"captcha"}}"#;
        assert_eq!(verification_token(raw).as_deref(), Some("tok-1"));
        let twice = r#""{\"type\":\"HUMAN_VERIFICATION_SUCCESS\",\"payload\":{\"token\":\"tok-1\",\"type\":\"captcha\"}}""#;
        assert_eq!(verification_token(twice).as_deref(), Some("tok-1"));
    }

    #[test]
    fn messages_that_are_not_a_verification_completion_are_ignored() {
        for raw in [
            r#"{"type":"resize","payload":{"height":400}}"#,
            r#"{"type":"HUMAN_VERIFICATION_SUCCESS"}"#,
            r#"{"type":"HUMAN_VERIFICATION_SUCCESS","payload":{"token":""}}"#,
            "not json",
            "",
        ] {
            assert!(verification_token(raw).is_none(), "accepted: {raw}");
        }
    }
}
