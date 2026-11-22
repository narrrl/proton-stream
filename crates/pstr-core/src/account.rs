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
//! One JSON blob in the [`SecretStore`] holding the session tokens and, per
//! account key, the passphrase that unlocks it. Those passphrases are derived
//! at sign-in from the mailbox password and the key salts, and kept *instead*
//! of the password: they unlock the key chain just the same, but a store that
//! leaks gives away this app's access to the account rather than the password
//! — which, on a single-password account, is the login password itself.
//! Deriving them later is not an option either way: `core/v4/keys/salts`
//! needs a scope only a password sign-in grants, and answers 403 after the
//! first token refresh.
//!
//! Proton refresh tokens are single-use, so every refresh is written back
//! (see [`persist_refreshed_tokens`]). A refresh that is not leaves the store
//! holding a dead token and the next launch signed out. A refresh that lands
//! after a sign-out, or after another sign-in replaced the session, must *not*
//! be written: it would bring back the session the viewer just left, or
//! overwrite the new one with the old. [`Generation`] keeps those out.
//!
//! ## Signing in is several steps
//!
//! Password, then maybe a second-factor code, then — on an account with a
//! separate mailbox password — that. Each front end asks for them in its own
//! way, so [`AccountStore::sign_in`] returns a [`SignIn`] saying what it needs
//! next and holding the half-made session until it is given.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use proton_drive_rs::{NodeKind, ProtonDriveClient};
use proton_sdk::account::KeyPassphrases;
use proton_sdk::api::{HumanVerification, HumanVerificationCredential};
use proton_sdk::ids::NodeUid;
use proton_sdk::login::{self, LoginError, Step};
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
    /// Key id → the passphrase that unlocks it. ASCII: they are bcrypt output.
    key_passphrases: HashMap<String, String>,
}

impl StoredSession {
    fn capture(params: ResumeParameters, key_passphrases: HashMap<String, String>) -> Self {
        Self {
            session_id: params.session_id.as_str().to_owned(),
            username: params.username,
            user_id: params.user_id.as_str().to_owned(),
            access_token: params.access_token,
            refresh_token: params.refresh_token,
            scopes: params.scopes,
            // Proton's numbers are 1 and 2.
            password_mode: params.password_mode.to_wire() as u8,
            key_passphrases,
        }
    }

    fn unlock(&self) -> KeyPassphrases {
        self.key_passphrases
            .iter()
            .map(|(id, passphrase)| (id.as_str(), passphrase.as_bytes()))
            .collect()
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
            password_mode: PasswordMode::from_wire(self.password_mode.into()),
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

/// Which stored session the token write-back may touch.
///
/// Every sign-in and sign-out starts a new generation. A session's refresh
/// callback remembers the generation it was installed under and writes only
/// while that is still current, checked under the same lock a sign-out holds
/// while it deletes — so no refresh can slip in between and store again.
///
/// A sign-out on a plane cannot end the session on Proton's side, and copies
/// of the account held by the sync and crawl tasks keep refreshing it; this is
/// what stops them signing the viewer back in on the next launch.
#[derive(Clone, Default)]
struct Generation(Arc<Mutex<u64>>);

impl Generation {
    fn current(&self) -> u64 {
        *self.0.lock()
    }

    /// Start a new generation and run `then` before any write-back of the old
    /// one can run. Returns the new generation with `then`'s result.
    fn advance<T>(&self, then: impl FnOnce() -> T) -> (u64, T) {
        let mut generation = self.0.lock();
        *generation += 1;
        (*generation, then())
    }

    /// Run `write` if `generation` is still current.
    fn write_if_current(&self, generation: u64, write: impl FnOnce()) {
        let current = self.0.lock();
        if *current == generation {
            write();
        }
    }
}

/// What a finished sign-in writes to.
#[derive(Clone)]
struct Sink {
    secrets: Arc<dyn SecretStore>,
    generation: Generation,
}

/// A sign-in waiting on the viewer.
pub struct PendingSignIn {
    sink: Sink,
    pending: login::Pending,
}

impl PendingSignIn {
    /// Submit a second-factor code.
    ///
    /// A wrong code fails here and leaves nothing behind; the front end starts
    /// the sign-in over, because Proton does not accept a second try on the
    /// same half-made session.
    pub async fn second_factor(self, code: &str) -> Result<SignIn> {
        let step = self
            .pending
            .second_factor(code)
            .await
            .map_err(sign_in_error)?;
        advance(self.sink, step).await
    }

    /// Submit the mailbox password, on an account that has one.
    ///
    /// By reference, unlike the code: a mistyped mailbox password is checked
    /// on this side, the session survives it, and the viewer can try again
    /// without starting over.
    pub async fn mailbox_password(&self, mailbox_password: &str) -> Result<Account> {
        let unlocked = self
            .pending
            .mailbox_password(mailbox_password)
            .await
            .map_err(sign_in_error)?;
        finish(self.sink.clone(), unlocked).await
    }
}

/// The signed-in account, if any, and signing in and out.
#[derive(Clone)]
pub struct AccountStore {
    secrets: Arc<dyn SecretStore>,
    generation: Generation,
}

impl AccountStore {
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            secrets,
            generation: Generation::default(),
        }
    }

    fn sink(&self) -> Sink {
        Sink {
            secrets: Arc::clone(&self.secrets),
            generation: self.generation.clone(),
        }
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
        let step = login::begin(
            client_configuration(),
            username,
            password,
            verification.as_ref(),
        )
        .await
        .map_err(sign_in_error)?;
        advance(self.sink(), step).await
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
            &self.sink(),
            self.generation.current(),
            &session,
            stored.key_passphrases.clone(),
        );
        let drive = ProtonDriveClient::with_key_passphrases(&session, stored.unlock());
        Ok(Some(account(session, drive)))
    }

    /// Forget the account: end its session and delete what was stored.
    ///
    /// The stored session goes even when ending it fails — signing out on a
    /// plane must still sign out — and stays gone: a refresh the session
    /// still makes afterwards is not written back.
    pub async fn sign_out(&self, account: Option<&Account>) -> Result<()> {
        // Before ending it: ending the session can itself refresh the token.
        self.generation
            .advance(|| self.secrets.delete(SESSION_KEY))
            .1?;
        if let Some(account) = account {
            account.end().await;
        }
        Ok(())
    }

    /// The stored session, or `None` for none — or for one this build cannot
    /// read, such as the shape development builds before 1.2.0 stored, which
    /// kept the mailbox password. Signing in again is the only fix for either,
    /// and it replaces what is there.
    fn load(&self) -> Result<Option<StoredSession>> {
        let Some(encoded) = self.secrets.get(SESSION_KEY)? else {
            return Ok(None);
        };
        Ok(serde_json::from_str(&encoded)
            .inspect_err(|error| {
                tracing::warn!(%error, "the stored Proton session is unreadable; sign in again");
            })
            .ok())
    }
}

/// Where the shared sign-in stands, in this app's terms.
async fn advance(sink: Sink, step: Step) -> Result<SignIn> {
    Ok(match step {
        Step::SecondFactor(pending) => SignIn::SecondFactor(PendingSignIn { sink, pending }),
        Step::MailboxPassword(pending) => SignIn::MailboxPassword(PendingSignIn { sink, pending }),
        Step::Unlock(pending) => {
            let unlocked = pending.unlock().await.map_err(sign_in_error)?;
            SignIn::Done(finish(sink, unlocked).await?)
        }
    })
}

/// Store a finished sign-in and hand back the account.
///
/// Only the key passphrases are stored; the passwords go no further.
async fn finish(sink: Sink, unlocked: login::Unlocked) -> Result<Account> {
    let key_passphrases = unlocked
        .key_passphrases
        .iter()
        .map(|(id, passphrase)| {
            std::str::from_utf8(passphrase)
                .map(|passphrase| (id.to_owned(), passphrase.to_owned()))
                .map_err(|_| Error::Config("a derived key passphrase is not text".to_owned()))
        })
        .collect::<Result<HashMap<_, _>>>()?;

    let stored =
        StoredSession::capture(unlocked.resume_parameters().await, key_passphrases.clone());
    let encoded = serde_json::to_string(&stored)
        .map_err(|e| Error::Config(format!("serialize the Proton session: {e}")))?;
    // A new generation: the session this one replaces, if any, must not
    // write its refreshes over it.
    let (generation, stored_it) = sink
        .generation
        .advance(|| sink.secrets.set(SESSION_KEY, &encoded));
    stored_it?;
    persist_refreshed_tokens(&sink, generation, &unlocked.session, key_passphrases);

    let drive = ProtonDriveClient::with_key_passphrases(&unlocked.session, stored.unlock());
    Ok(account(unlocked.session, drive))
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
///
/// Only while `generation` is current: see [`Generation`].
fn persist_refreshed_tokens(
    sink: &Sink,
    generation: u64,
    session: &ProtonApiSession,
    key_passphrases: HashMap<String, String>,
) {
    let sink = sink.clone();
    let template = session.clone();
    session.http().set_on_tokens_refreshed(move |tokens| {
        let stored = StoredSession::capture(
            template.resume_parameters_with(tokens),
            key_passphrases.clone(),
        );
        store_refreshed(&sink, generation, &stored);
    });
}

/// Write one refreshed session, if its generation is still current.
fn store_refreshed(sink: &Sink, generation: u64, stored: &StoredSession) {
    sink.generation.write_if_current(generation, || {
        let written = serde_json::to_string(stored)
            .map_err(|e| Error::Config(e.to_string()))
            .and_then(|encoded| Ok(sink.secrets.set(SESSION_KEY, &encoded)?));
        if let Err(error) = written {
            tracing::warn!(%error, "the refreshed Proton session could not be stored");
        }
    });
}

/// A failed sign-in step as this app's error. Only a CAPTCHA gate keeps its
/// own variant, because the front ends open the verification page for it;
/// every other refusal is a message to show.
fn sign_in_error(error: LoginError) -> Error {
    match error {
        LoginError::HumanVerification(challenge) => Error::HumanVerification(challenge),
        LoginError::Proton(error) => error.into(),
        refused => Error::SignIn(refused.to_string()),
    }
}

/// The token out of a message posted by Proton's verification page, or `None`
/// for anything that is not a completion.
pub fn verification_token(raw: &str) -> Option<String> {
    HumanVerification::token_from_message(raw)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use parking_lot::Mutex;

    use super::*;

    #[derive(Default)]
    struct MemorySecrets(Mutex<HashMap<String, String>>);

    impl SecretStore for MemorySecrets {
        fn set(&self, key: &str, value: &str) -> proton_auth_rs::Result<()> {
            self.0.lock().insert(key.to_owned(), value.to_owned());
            Ok(())
        }
        fn get(&self, key: &str) -> proton_auth_rs::Result<Option<String>> {
            Ok(self.0.lock().get(key).cloned())
        }
        fn delete(&self, key: &str) -> proton_auth_rs::Result<()> {
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
            key_passphrases: HashMap::from([("key-1".to_owned(), "derived".to_owned())]),
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

    #[tokio::test]
    async fn a_refresh_after_signing_out_does_not_sign_the_viewer_back_in() {
        let store = AccountStore::new(Arc::new(MemorySecrets::default()));
        let (generation, _) = store.generation.advance(|| ());
        store_refreshed(&store.sink(), generation, &stored());
        assert!(store.username().expect("read").is_some());

        store.sign_out(None).await.expect("sign out");
        store_refreshed(&store.sink(), generation, &stored());
        assert_eq!(store.username().expect("read"), None);
    }

    #[test]
    fn a_refresh_of_a_replaced_session_does_not_overwrite_the_new_one() {
        let store = AccountStore::new(Arc::new(MemorySecrets::default()));
        let (old, _) = store.generation.advance(|| ());
        let (new, _) = store.generation.advance(|| ());
        let mut newer = stored();
        newer.username = "other@proton.me".to_owned();
        store_refreshed(&store.sink(), new, &newer);

        store_refreshed(&store.sink(), old, &stored());
        assert_eq!(
            store.username().expect("read").as_deref(),
            Some("other@proton.me")
        );
    }

    #[test]
    fn the_stored_session_holds_no_password() {
        let encoded = serde_json::to_string(&stored()).unwrap();
        assert!(!encoded.contains("password\""), "{encoded}");
        assert!(!encoded.contains("key_salts"), "{encoded}");
    }

    #[test]
    fn a_session_stored_by_an_older_build_reads_as_signed_out() {
        let secrets = Arc::new(MemorySecrets::default());
        secrets
            .set(
                SESSION_KEY,
                r#"{"session_id":"s","username":"u","user_id":"i","access_token":"a",
                    "refresh_token":"r","scopes":[],"password_mode":1,
                    "mailbox_password":"hunter2","key_salts":[]}"#,
            )
            .unwrap();
        let store = AccountStore::new(secrets);
        assert_eq!(store.username().expect("read"), None);
        assert!(store.resume().expect("resume").is_none());
    }

    #[test]
    fn a_stored_session_unlocks_with_its_passphrases() {
        let unlock = stored().unlock();
        assert_eq!(unlock.len(), 1);
        assert_eq!(unlock.get("key-1"), Some(b"derived".as_slice()));
    }

    #[test]
    fn a_blocked_sign_in_does_not_claim_the_app_is_outdated() {
        let error = sign_in_error(LoginError::Blocked(proton_sdk::error::ProtonApiError {
            code: proton_sdk::api::ResponseCode::Blocked,
            http_status: 422,
            message: "This version of the app is no longer supported".to_owned(),
            details: None,
        }));
        assert!(
            matches!(&error, Error::SignIn(text) if text.contains("blocked sign-ins")),
            "{error}"
        );
    }

    #[test]
    fn a_captcha_gate_stays_its_own_error() {
        let challenge = proton_sdk::api::HumanVerification {
            token: "t".to_owned(),
            methods: vec!["captcha".to_owned()],
        };
        let error = sign_in_error(LoginError::HumanVerification(Box::new(challenge)));
        assert!(matches!(error, Error::HumanVerification(_)), "{error}");
    }

    #[test]
    fn a_stored_session_keeps_its_password_mode() {
        let params = stored().resume_parameters();
        assert_eq!(
            params.password_mode,
            proton_sdk::session::PasswordMode::Dual
        );
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
