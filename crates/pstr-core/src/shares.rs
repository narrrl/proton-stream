//! The links this app knows about, and opening them.
//!
//! ## Where the secrets live
//!
//! A Proton public-link URL carries its decryption password in its **fragment**
//! (`https://drive.proton.me/urls/TOKEN#s3cr3t`), and a custom-password link
//! needs a second secret on top. Both are, in full, the ability to read the
//! share — so neither goes in the config file. The config holds the share's id,
//! its display name and its token; the URL and custom password go in the OS
//! credential store (Secret Service on Linux, Credential Manager on Windows).
//!
//! ## Why several shares
//!
//! A library is not one folder. [`SharedLibrary`] opens every configured share
//! and merges them into one catalog, so the app has a single browsable view
//! regardless of how many links it was given.
//!
//! ## Folders from a signed-in account
//!
//! A share can also be a folder of the viewer's own Drive — My Files, another
//! device, or something shared with them — opened through the account signed
//! in by [`crate::account`] instead of a link. It has no secrets of its own:
//! the account session is the credential, and without it such a share simply
//! fails to open, the same as a revoked link. [`ShareClient`] puts both kinds
//! behind one surface so the crawl, the stream layer and the thumbnail path
//! do not care which one they hold.

use std::collections::BTreeMap;
use std::sync::Arc;

use proton_drive_rs::{
    Node, ProtonDriveClient, ProtonDrivePublicLinkClient, RevisionReader, ThumbnailType,
};
use proton_sdk::config::ProtonClientConfiguration;
use proton_sdk::ids::{LinkId, NodeUid, VolumeId};
use serde::{Deserialize, Serialize};

use crate::account::Account;
use crate::config::{AppDirs, read_json, write_json};
use crate::error::{Error, Result};

/// The credential-store service name. One entry per share, keyed by share id.
#[cfg(not(target_os = "android"))]
const KEYRING_SERVICE: &str = "proton-stream";

/// Secret persistence supplied by the platform.
///
/// Desktop builds use [`KeyringSecretStore`]. Android supplies an
/// implementation backed by a non-exportable Android Keystore key through the
/// language bridge, keeping platform APIs out of this portable crate.
pub trait SecretStore: Send + Sync {
    fn set(&self, key: &str, value: &str) -> Result<()>;
    fn get(&self, key: &str) -> Result<Option<String>>;
    fn delete(&self, key: &str) -> Result<()>;
}

/// The desktop OS credential store.
#[cfg(not(target_os = "android"))]
#[derive(Debug, Default)]
pub struct KeyringSecretStore;

#[cfg(not(target_os = "android"))]
impl KeyringSecretStore {
    fn entry(key: &str) -> Result<keyring::Entry> {
        Ok(keyring::Entry::new(KEYRING_SERVICE, key)?)
    }
}

#[cfg(not(target_os = "android"))]
impl SecretStore for KeyringSecretStore {
    fn set(&self, key: &str, value: &str) -> Result<()> {
        Self::entry(key)?.set_password(value)?;
        Ok(())
    }

    fn get(&self, key: &str) -> Result<Option<String>> {
        match Self::entry(key)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn delete(&self, key: &str) -> Result<()> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

/// What the app records about a share, minus its secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Share {
    /// Stable id, used as the credential-store key and the catalog's share
    /// column. Assigned once and never reused.
    pub id: String,
    /// What to call this share in the UI.
    pub name: String,
    /// The link token — the path segment of the share URL. Not a secret on its
    /// own: without the fragment password it decrypts nothing.
    pub token: String,
    /// Whether opening this share needs a custom password in addition to the
    /// URL fragment. Recorded so the UI knows to prompt before the handshake
    /// rather than after it fails.
    #[serde(default)]
    pub has_custom_password: bool,
    /// Set when this share is a folder of the signed-in account rather than a
    /// public link; `token` is then empty and there are no secrets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<AccountFolder>,
}

/// A folder of the signed-in account, kept as a library share.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountFolder {
    pub volume_id: String,
    pub link_id: String,
}

impl AccountFolder {
    pub fn uid(&self) -> NodeUid {
        NodeUid::new(
            VolumeId::from(self.volume_id.clone()),
            LinkId::from(self.link_id.clone()),
        )
    }

    /// The share id this folder is kept under.
    ///
    /// Derived, like a link's `share-{token}`, so the same folder added on two
    /// devices is the same share — which is what lets watch history recorded
    /// against it on one device land on the other. Hashed rather than spliced
    /// in because Proton ids are base64 and may carry characters a catalog key
    /// is better off without.
    pub fn share_id(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        hash.update(self.volume_id.as_bytes());
        hash.update([0]);
        hash.update(self.link_id.as_bytes());
        let digest = hash.finalize();
        let hex: String = digest[..12]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        format!("drive-{hex}")
    }
}

/// The secrets for one share, as held in the credential store.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ShareSecrets {
    /// The full share URL, fragment included.
    url: String,
    /// The custom password, when the link has one.
    #[serde(default)]
    custom_password: Option<String>,
}

/// The configured shares, and their secrets.
pub struct ShareStore {
    dirs: AppDirs,
    secrets: Arc<dyn SecretStore>,
}

impl ShareStore {
    #[cfg(not(target_os = "android"))]
    pub fn new(dirs: AppDirs) -> Self {
        Self::with_secret_store(dirs, Arc::new(KeyringSecretStore))
    }

    pub fn with_secret_store(dirs: AppDirs, secrets: Arc<dyn SecretStore>) -> Self {
        Self { dirs, secrets }
    }

    /// Every configured share, in the order they were added.
    pub fn list(&self) -> Result<Vec<Share>> {
        Ok(read_json(&self.dirs.shares_file())?.unwrap_or_default())
    }

    /// Record a share and stash its secrets.
    ///
    /// The secrets are written **before** the config entry: a credential-store
    /// failure then leaves no config row pointing at credentials that do not
    /// exist, which would read as a share that exists but can never be opened.
    pub fn add(&self, name: &str, url: &str, custom_password: Option<&str>) -> Result<Share> {
        let token = token_from_url(url)?;
        let id = format!("share-{token}");

        let mut shares = self.list()?;
        if shares.iter().any(|share| share.id == id) {
            return Err(Error::Config(format!(
                "a share for token {token} is already configured"
            )));
        }

        let secrets = ShareSecrets {
            url: url.to_string(),
            custom_password: custom_password
                .filter(|password| !password.is_empty())
                .map(str::to_string),
        };
        self.store_secrets(&id, &secrets)?;

        let share = Share {
            id,
            name: name.to_string(),
            token,
            has_custom_password: secrets.custom_password.is_some(),
            folder: None,
        };
        shares.push(share.clone());
        write_json(&self.dirs.shares_file(), &shares)?;
        Ok(share)
    }

    /// Record a folder of the signed-in account as a share.
    ///
    /// Nothing goes in the credential store: the folder opens through the
    /// account's session, which [`crate::account::AccountStore`] keeps.
    pub fn add_folder(&self, name: &str, folder: AccountFolder) -> Result<Share> {
        let id = folder.share_id();
        let mut shares = self.list()?;
        if shares.iter().any(|share| share.id == id) {
            return Err(Error::Config(
                "that folder is already in the library".to_owned(),
            ));
        }
        let share = Share {
            id,
            name: name.to_string(),
            token: String::new(),
            has_custom_password: false,
            folder: Some(folder),
        };
        shares.push(share.clone());
        write_json(&self.dirs.shares_file(), &shares)?;
        Ok(share)
    }

    /// Re-supply a configured share's secrets, keeping everything else.
    ///
    /// A secret store can lose the ability to read what it wrote: an Android
    /// Keystore key is invalidated by a lockscreen change or a device restore,
    /// and a desktop keyring can be reset. After that every open of the share
    /// fails — and so, often, does removing it, because removal deletes a
    /// secret the store can no longer touch. Re-entering the link is the way
    /// back that does not cost the catalog and the offline files.
    ///
    /// The link must be the same one: a different token is a different share,
    /// and quietly repointing an existing library at it would leave every
    /// catalog row describing files that are no longer there.
    pub fn replace_secrets(
        &self,
        id: &str,
        url: &str,
        custom_password: Option<&str>,
    ) -> Result<Share> {
        let token = token_from_url(url)?;
        let mut shares = self.list()?;
        let share = shares
            .iter_mut()
            .find(|share| share.id == id)
            .ok_or_else(|| Error::NotFound(format!("no share with id {id}")))?;
        if share.folder.is_some() {
            return Err(Error::Config(
                "that share is a folder of your account and has no link".to_owned(),
            ));
        }
        if share.token != token {
            return Err(Error::Config(
                "that link is for a different share; add it as a new one".to_owned(),
            ));
        }

        let secrets = ShareSecrets {
            url: url.to_string(),
            custom_password: custom_password
                .filter(|password| !password.is_empty())
                .map(str::to_string),
        };
        self.store_secrets(id, &secrets)?;
        share.has_custom_password = secrets.custom_password.is_some();
        let updated = share.clone();
        write_json(&self.dirs.shares_file(), &shares)?;
        Ok(updated)
    }

    /// Forget a share and its secrets.
    ///
    /// The config entry goes first here, for the mirror-image reason: if the
    /// credential deletion fails, what is left is an orphaned secret rather than
    /// a listed share whose password is gone.
    pub fn remove(&self, id: &str) -> Result<()> {
        let mut shares = self.list()?;
        let before = shares.len();
        shares.retain(|share| share.id != id);
        if shares.len() == before {
            return Err(Error::NotFound(format!("no share with id {id}")));
        }
        write_json(&self.dirs.shares_file(), &shares)?;

        // A missing entry is the desired end state, not a failure.
        self.secrets.delete(id)
    }

    /// Open a share: a link as a visitor, a folder through `account`.
    pub async fn open(&self, share: &Share, account: Option<&Account>) -> Result<ShareClient> {
        if let Some(folder) = &share.folder {
            let account = account.ok_or_else(|| {
                Error::SignedOut(format!(
                    "sign in to Proton to open {}, a folder of your account",
                    share.name
                ))
            })?;
            return Ok(ShareClient::Account {
                client: account.drive().clone(),
                root: folder.uid(),
            });
        }
        let secrets = self.load_secrets(&share.id)?;
        let client = ProtonDrivePublicLinkClient::open(
            client_configuration(),
            &secrets.url,
            secrets.custom_password.as_deref(),
        )
        .await?
        // The SDK default is sized for a background sync daemon. A player that
        // seeks wants more blocks in flight; at 4 MiB each this is a 192 MiB
        // ceiling, which is unremarkable for a desktop app.
        .with_max_inflight_blocks(48);
        Ok(ShareClient::PublicLink(Box::new(client)))
    }

    fn store_secrets(&self, id: &str, secrets: &ShareSecrets) -> Result<()> {
        let encoded = serde_json::to_string(secrets)
            .map_err(|e| Error::Config(format!("serialize share secrets: {e}")))?;
        self.secrets.set(id, &encoded)
    }

    fn load_secrets(&self, id: &str) -> Result<ShareSecrets> {
        let encoded = self.secrets.get(id)?.ok_or_else(|| {
            Error::NotFound(format!(
                "share {id} has no stored credentials; remove and re-add it"
            ))
        })?;
        serde_json::from_str(&encoded)
            .map_err(|e| Error::Config(format!("stored share secrets are unreadable: {e}")))
    }
}

/// Several opened shares, presented as one library.
pub struct SharedLibrary {
    clients: BTreeMap<String, ShareClient>,
}

/// One opened share, whichever kind it is.
///
/// Cheap to clone: both clients share their caches and sessions between
/// clones.
#[derive(Clone)]
pub enum ShareClient {
    /// A public link, opened as a visitor. Boxed: the visitor client is
    /// several times the size of the account variant, and libraries are
    /// mostly links, so every map entry would otherwise pay for it.
    PublicLink(Box<ProtonDrivePublicLinkClient>),
    /// A folder of the signed-in account.
    Account {
        client: ProtonDriveClient,
        root: NodeUid,
    },
}

impl ShareClient {
    /// The share's top folder.
    pub async fn root_node(&self) -> proton_sdk::error::Result<Node> {
        match self {
            Self::PublicLink(client) => client.get_root_node().await,
            Self::Account { client, root } => client.get_node(root).await?.ok_or_else(|| {
                proton_sdk::error::ProtonError::invalid_operation(
                    "the folder is no longer in your Drive",
                )
            }),
        }
    }

    pub async fn enumerate_folder_children_node_uids(
        &self,
        folder: &NodeUid,
    ) -> proton_sdk::error::Result<Vec<NodeUid>> {
        match self {
            Self::PublicLink(client) => client.enumerate_folder_children_node_uids(folder).await,
            Self::Account { client, .. } => {
                client.enumerate_folder_children_node_uids(folder).await
            }
        }
    }

    pub async fn enumerate_nodes(&self, uids: &[NodeUid]) -> proton_sdk::error::Result<Vec<Node>> {
        match self {
            Self::PublicLink(client) => client.enumerate_nodes(uids).await,
            Self::Account { client, .. } => client.enumerate_nodes(uids).await,
        }
    }

    pub async fn open_revision(&self, uid: &NodeUid) -> proton_sdk::error::Result<RevisionReader> {
        match self {
            Self::PublicLink(client) => client.open_revision(uid).await,
            Self::Account { client, .. } => client.open_revision(uid).await,
        }
    }

    pub async fn download_range(
        &self,
        uid: &NodeUid,
        offset: u64,
        length: u64,
    ) -> proton_sdk::error::Result<Vec<u8>> {
        match self {
            Self::PublicLink(client) => client.download_range(uid, offset, length).await,
            Self::Account { client, .. } => client.download_range(uid, offset, length).await,
        }
    }

    pub async fn download_thumbnail(
        &self,
        uid: &NodeUid,
        kind: ThumbnailType,
    ) -> proton_sdk::error::Result<Option<Vec<u8>>> {
        match self {
            Self::PublicLink(client) => client.download_thumbnail(uid, kind).await,
            Self::Account { client, .. } => client.download_thumbnail(uid, kind).await,
        }
    }

    /// Replay a visitor handshake. An account session refreshes its own
    /// tokens when one expires, so there is nothing to do for a folder.
    pub async fn refresh_session(&self) -> proton_sdk::error::Result<()> {
        match self {
            Self::PublicLink(client) => client.refresh_session().await,
            Self::Account { .. } => Ok(()),
        }
    }
}

impl SharedLibrary {
    /// Open every configured share.
    ///
    /// A share that fails to open is reported and skipped rather than failing
    /// the whole library — one revoked link should not make the other three
    /// unwatchable.
    ///
    /// Folders of the account open through `account`; without one they are
    /// reported as failures like any other share that will not open.
    pub async fn open_all(
        store: &ShareStore,
        account: Option<&Account>,
    ) -> Result<(Self, Vec<(Share, Error)>)> {
        Self::open_all_reusing(store, BTreeMap::new(), account).await
    }

    /// Open every configured share, reusing the clients in `reusable`.
    ///
    /// Adding or removing one link should not cost a fresh handshake for the
    /// shares that were already open, so a caller that still holds them can
    /// hand them back here. A share whose stored secrets changed must *not*
    /// appear in `reusable` — its old client authenticates against the old
    /// link.
    pub async fn open_all_reusing(
        store: &ShareStore,
        mut reusable: BTreeMap<String, ShareClient>,
        account: Option<&Account>,
    ) -> Result<(Self, Vec<(Share, Error)>)> {
        let mut clients = BTreeMap::new();
        let mut failures = Vec::new();

        for share in store.list()? {
            if let Some(client) = reusable.remove(&share.id) {
                clients.insert(share.id, client);
                continue;
            }
            match store.open(&share, account).await {
                Ok(client) => {
                    clients.insert(share.id.clone(), client);
                }
                Err(e) => failures.push((share, e)),
            }
        }

        Ok((Self { clients }, failures))
    }

    /// The opened clients, minus `dropping`, ready to hand to
    /// [`SharedLibrary::open_all_reusing`].
    ///
    /// The excluded share is the one whose credentials or membership just
    /// changed; leaving it out is what makes a removed share's live client
    /// unreachable rather than merely stale.
    ///
    /// Folders of the account are never handed back: they carry the session
    /// they were opened with, and the account may have signed out or changed
    /// since.
    pub fn reusable_clients(&self, dropping: &str) -> BTreeMap<String, ShareClient> {
        self.clients
            .iter()
            .filter(|(id, client)| {
                id.as_str() != dropping && matches!(client, ShareClient::PublicLink(_))
            })
            .map(|(id, client)| (id.clone(), client.clone()))
            .collect()
    }

    /// The client for one share.
    pub fn client(&self, share_id: &str) -> Option<&ShareClient> {
        self.clients.get(share_id)
    }

    /// Every opened share id.
    pub fn share_ids(&self) -> impl Iterator<Item = &str> {
        self.clients.keys().map(String::as_str)
    }

    /// Replay a public-link handshake before performing a user-requested
    /// refresh. Visitor sessions are short-lived, so a cached client is still
    /// useful for streaming but must not be assumed valid for a later crawl.
    pub async fn refresh_session(&self, share_id: &str) -> Result<()> {
        let client = self
            .client(share_id)
            .ok_or_else(|| Error::NotFound(format!("share {share_id} is not open")))?;
        client.refresh_session().await?;
        Ok(())
    }

    /// Walk one share's whole subtree, level by level, yielding every node.
    ///
    /// There is no recursion helper on the visitor path — the authenticated
    /// client's `get_node_hierarchy` / `get_node_by_path` have no counterpart
    /// there — so the walk is here.
    ///
    /// **Breadth first and fanned out, because a media share is wide.** The
    /// shape of one is a root holding a folder per series, each holding a folder
    /// per season: a couple of hundred folders that are each one listing request
    /// plus a detail fetch, and almost all of them siblings. Walking that one
    /// folder at a time spends the whole crawl waiting on a round trip it could
    /// have overlapped — the crawl was minutes of latency and seconds of work.
    /// Taking a whole level at once is what collapses it.
    ///
    /// The client is cheap to clone and its clones share one node-key cache and
    /// one single-flight map, so siblings that need the same ancestor key wait
    /// on one derivation rather than each starting their own. `buffered` rather
    /// than `buffer_unordered`: the crawl's output order is otherwise a race,
    /// and a catalog that reorders itself between two identical crawls is
    /// needlessly hard to diff.
    pub async fn crawl(&self, share_id: &str) -> Result<Vec<Node>> {
        self.crawl_reporting(share_id, |_| {}).await
    }

    /// [`Self::crawl`], calling `progress` with how many nodes have been found
    /// so far each time a folder's listing comes back.
    ///
    /// A crawl of a large share takes minutes, and "crawling…" for minutes
    /// reads as stuck. A count that keeps rising does not.
    pub async fn crawl_reporting(
        &self,
        share_id: &str,
        progress: impl Fn(usize),
    ) -> Result<Vec<Node>> {
        use futures::stream::{StreamExt as _, TryStreamExt as _};

        let client = self
            .client(share_id)
            .ok_or_else(|| Error::NotFound(format!("share {share_id} is not open")))?;

        let root = client.root_node().await?;
        let mut level = vec![root.uid.clone()];
        let mut found = vec![root];

        while !level.is_empty() {
            let listings = level.into_iter().map(|folder| {
                let client = client.clone();
                async move {
                    let child_uids: Vec<NodeUid> =
                        client.enumerate_folder_children_node_uids(&folder).await?;
                    if child_uids.is_empty() {
                        return Ok(Vec::new());
                    }
                    // Chunked and fanned out inside the SDK too, so this is a
                    // multiplier on that concurrency rather than the only one.
                    client.enumerate_nodes(&child_uids).await
                }
            });

            let mut listings = futures::stream::iter(listings).buffered(CRAWL_CONCURRENCY);
            let mut next = Vec::new();
            while let Some(children) = listings.try_next().await? {
                for child in children {
                    if child.is_folder() {
                        next.push(child.uid.clone());
                    }
                    found.push(child);
                }
                progress(found.len());
            }
            level = next;
        }

        Ok(found)
    }
}

/// How many folders of one level to list at once.
///
/// Each of these fans out again inside the SDK — up to four detail batches per
/// folder — so this is not the number of requests in flight but a multiplier on
/// it. Six keeps the ceiling in the low tens: enough to hide the round trip on
/// a wide level, well short of a burst a share server would read as abuse.
const CRAWL_CONCURRENCY: usize = 6;

/// The app-version string Proton identifies this client by.
///
/// The `external-drive-` prefix is **not** decoration: the API parses the part
/// before the first `-` as a platform and rejects anything it does not know
/// with `400 Platform \`…\` is not valid`. A bare `proton-stream@0.1.0` fails
/// every request, including the public-link handshake.
const APP_VERSION: &str = concat!("external-drive-stream@", env!("CARGO_PKG_VERSION"));

const USER_AGENT: &str = concat!("proton-stream/", env!("CARGO_PKG_VERSION"));

/// The API configuration every visitor client is built from.
pub(crate) fn client_configuration() -> ProtonClientConfiguration {
    ProtonClientConfiguration::new(APP_VERSION).with_user_agent(USER_AGENT)
}

/// The token out of a share URL: the path segment after `/urls/`.
///
/// Parsed here as well as in the SDK because the store needs a stable id for a
/// share *before* it has opened it — and the id must not be derived from the
/// password, which would put a secret in the config file by the back door.
/// The share token a link names, or why it is not a usable share link.
///
/// Public so a form can say what is wrong with a link while it is still being
/// pasted, rather than after a round trip through [`ShareStore::add`] — which
/// asks exactly this first.
pub fn share_token(url: &str) -> Result<String> {
    token_from_url(url)
}

fn token_from_url(url: &str) -> Result<String> {
    let (_, tail) = url
        .split_once("/urls/")
        .ok_or_else(|| Error::Config("not a Proton share URL".to_owned()))?;

    let token = tail
        .split(['#', '?'])
        .next()
        .unwrap_or_default()
        .trim_end_matches('/');

    if token.is_empty() {
        return Err(Error::Config(
            "Proton share URL carries no share token".to_owned(),
        ));
    }
    if !url.contains('#') {
        return Err(Error::Config(
            "Proton share URL has no #password fragment; copy the full share link".to_owned(),
        ));
    }
    Ok(token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_share_url_yields_its_token() {
        assert_eq!(
            token_from_url("https://drive.proton.me/urls/ABC123#s3cr3t").unwrap(),
            "ABC123"
        );
    }

    #[test]
    fn a_trailing_slash_before_the_fragment_is_tolerated() {
        assert_eq!(
            token_from_url("https://drive.proton.me/urls/ABC123/#s3cr3t").unwrap(),
            "ABC123"
        );
    }

    /// The fragment is the password. A URL without one cannot open anything, and
    /// saying so at add time beats a confusing failure at open time.
    #[test]
    fn a_url_without_a_fragment_is_refused_at_add_time() {
        let error = token_from_url("https://drive.proton.me/urls/ABC123").unwrap_err();
        assert!(
            error.to_string().contains("#password fragment"),
            "says what is missing: {error}"
        );
    }

    #[test]
    fn a_url_that_is_not_a_share_link_is_refused() {
        assert!(token_from_url("https://example.com/thing#x").is_err());
        assert!(token_from_url("https://drive.proton.me/urls/#s3cr3t").is_err());
    }

    #[test]
    fn malformed_share_errors_never_repeat_the_secret_url_fragment() {
        const SECRET: &str = "fragment-secret-that-must-not-leak";
        for url in [
            format!("https://example.com/not-a-share#{SECRET}"),
            format!("https://drive.proton.me/urls/#{SECRET}"),
        ] {
            let error = token_from_url(&url).unwrap_err().to_string();
            assert!(
                !error.contains(SECRET),
                "error exposed URL fragment: {error}"
            );
            assert!(
                !error.contains(&url),
                "error repeated the full URL: {error}"
            );
        }
    }

    /// A store that has forgotten how to read what it wrote, which is what an
    /// invalidated Android Keystore key looks like from Rust.
    #[derive(Default)]
    struct Unreadable {
        entries: std::sync::Mutex<BTreeMap<String, String>>,
        readable: std::sync::atomic::AtomicBool,
    }

    impl SecretStore for Unreadable {
        fn set(&self, key: &str, value: &str) -> Result<()> {
            self.entries
                .lock()
                .unwrap()
                .insert(key.to_owned(), value.to_owned());
            self.readable
                .store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        }

        fn get(&self, key: &str) -> Result<Option<String>> {
            if !self.readable.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(Error::Config("key permanently invalidated".into()));
            }
            Ok(self.entries.lock().unwrap().get(key).cloned())
        }

        fn delete(&self, key: &str) -> Result<()> {
            self.entries.lock().unwrap().remove(key);
            Ok(())
        }
    }

    fn store_in(name: &str) -> (ShareStore, Arc<Unreadable>) {
        let root = std::env::temp_dir().join(format!("pstr-shares-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dirs = AppDirs::from_paths(root.join("config"), root.join("data"), root.join("cache"))
            .expect("app directories");
        let secrets = Arc::new(Unreadable::default());
        (
            ShareStore::with_secret_store(dirs, secrets.clone()),
            secrets,
        )
    }

    /// The way back from an unreadable secret store. The share keeps its id, so
    /// the catalog rows and offline files keyed by it stay valid — which is the
    /// whole reason this exists rather than Remove-and-re-add.
    #[test]
    fn a_re_entered_link_restores_a_share_whose_secret_cannot_be_read() {
        let (store, secrets) = store_in("repair");
        let url = "https://drive.proton.me/urls/ABC123#s3cr3t";
        let added = store.add("Library", url, None).expect("add");

        secrets
            .readable
            .store(false, std::sync::atomic::Ordering::Relaxed);
        assert!(store.load_secrets(&added.id).is_err());

        let repaired = store
            .replace_secrets(&added.id, url, Some("new-password"))
            .expect("replace");
        assert_eq!(repaired.id, added.id);
        assert!(repaired.has_custom_password);
        assert!(
            store
                .list()
                .expect("list")
                .iter()
                .any(|share| share.id == added.id && share.has_custom_password)
        );
    }

    /// A different link is a different share. Repointing this one at it would
    /// leave every catalog row and offline file describing files that are not
    /// there.
    #[test]
    fn a_re_entered_link_for_a_different_share_is_refused() {
        let (store, _) = store_in("repair-mismatch");
        let added = store
            .add(
                "Library",
                "https://drive.proton.me/urls/ABC123#s3cr3t",
                None,
            )
            .expect("add");

        let error = store
            .replace_secrets(
                &added.id,
                "https://drive.proton.me/urls/DEF456#s3cr3t",
                None,
            )
            .expect_err("a different token must be refused");
        assert!(
            error.to_string().contains("different share"),
            "says what is wrong: {error}"
        );
    }

    /// The id must be derivable from the token alone — deriving it from the URL
    /// would mix a secret into the config file.
    #[test]
    fn the_share_id_carries_no_secret() {
        let token = token_from_url("https://drive.proton.me/urls/ABC123#s3cr3t").unwrap();
        let id = format!("share-{token}");
        assert!(!id.contains("s3cr3t"), "id must not embed the password");
    }
}
