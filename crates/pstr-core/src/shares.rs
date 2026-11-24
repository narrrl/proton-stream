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

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use proton_drive_rs::{Node, ProtonDrivePublicLinkClient};
use proton_drive_tree_rs::changes::{self, TreeIndex};
use proton_drive_tree_rs::{WalkError, WalkOptions, retry, walk};
use proton_sdk::config::ProtonClientConfiguration;
use proton_sdk::error::ProtonError;
use proton_sdk::ids::{DriveEventId, LinkId, NodeUid, VolumeId};
use serde::{Deserialize, Serialize};

use crate::account::Account;
use crate::config::{AppDirs, read_json, write_json};
use crate::error::{Error, Result};

/// The credential-store service name. One entry per share, keyed by share id.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
const KEYRING_SERVICE: &str = "proton-stream";

/// Secret persistence supplied by the platform.
///
/// Desktop builds use [`keyring_secrets`]. Android and iOS supply an
/// implementation backed by their platform keystore through the language
/// bridge, keeping platform APIs out of this portable crate.
pub use proton_auth_rs::SecretStore;

/// The desktop OS credential store, shared by the whole process.
///
/// Values too long for one entry (Windows caps an entry at 1280 UTF-16 units)
/// are split across several; see [`proton_auth_rs::Chunked`]. One instance,
/// so the store connection and its reconnect-on-failure are shared.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn keyring_secrets() -> Arc<dyn SecretStore> {
    static STORE: std::sync::OnceLock<Arc<dyn SecretStore>> = std::sync::OnceLock::new();
    Arc::clone(STORE.get_or_init(|| {
        Arc::new(proton_auth_rs::Chunked::new(
            proton_auth_rs::KeyringStore::new(KEYRING_SERVICE),
        ))
    }))
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ShareSecrets {
    /// The full share URL, fragment included.
    pub(crate) url: String,
    /// The custom password, when the link has one.
    #[serde(default)]
    pub(crate) custom_password: Option<String>,
}

/// The configured shares, and their secrets.
pub struct ShareStore {
    pub(crate) dirs: AppDirs,
    pub(crate) secrets: Arc<dyn SecretStore>,
}

impl ShareStore {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub fn new(dirs: AppDirs) -> Self {
        Self::with_secret_store(dirs, keyring_secrets())
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
            return Err(Error::Config(
                "that link is already in the library".to_owned(),
            ));
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

    /// [`Self::add`], but only once the link has been opened with the
    /// password given. An empty `name` takes the shared folder's own.
    ///
    /// A link that is mistyped, expired or behind the wrong password used to
    /// be stored anyway, so the first try failed on the crawl and every retry
    /// failed on "already configured" — the only way out was to remove a share
    /// that had never worked. Opening first means a refused link leaves
    /// nothing behind, and the client it opened is handed back for the crawl.
    pub async fn add_verified(
        &self,
        name: &str,
        url: &str,
        custom_password: Option<&str>,
    ) -> Result<(Share, ShareClient)> {
        let token = token_from_url(url)?;
        let id = format!("share-{token}");
        if self.list()?.iter().any(|share| share.id == id) {
            return Err(Error::Config(
                "that link is already in the library".to_owned(),
            ));
        }
        let custom_password = custom_password.filter(|password| !password.is_empty());
        let client = match open_link(url, custom_password).await {
            Ok(client) => client,
            // Asked only after a refusal: the link's public flags cost a round
            // trip, and most links open without one.
            Err(error) if custom_password.is_none() => {
                let protected = ProtonDrivePublicLinkClient::info(client_configuration(), url)
                    .await
                    .is_ok_and(|info| info.is_custom_password_protected);
                return Err(if protected {
                    Error::PasswordNeeded
                } else {
                    error
                });
            }
            Err(error) => return Err(error),
        };
        // A name is the viewer's to give, not theirs to invent before they
        // can see anything: without one, the shared folder's own name.
        let name = match name.trim() {
            "" => client
                .root_node()
                .await
                .ok()
                .map(|root| root.name)
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "Shared folder".to_owned()),
            given => given.to_owned(),
        };
        let share = self.add(&name, url, custom_password)?;
        Ok((share, client))
    }

    /// The `pstr://` invite for the shares named by `ids`, in library order,
    /// or for every share when `ids` is empty; see [`crate::invite`].
    ///
    /// Folders of the signed-in account have no secret to pass on and are
    /// left out; the second value is how many were. A selection of nothing
    /// but folders is an error, since there is no link to give.
    pub fn invite(&self, ids: &[&str]) -> Result<(String, usize)> {
        let mut invited = Vec::new();
        let mut left_out = 0;
        for share in self.list()? {
            if !ids.is_empty() && !ids.contains(&share.id.as_str()) {
                continue;
            }
            if share.folder.is_some() {
                left_out += 1;
                continue;
            }
            let secrets = self.load_secrets(&share.id)?;
            invited.push(crate::invite::InvitedShare {
                name: share.name,
                url: secrets.url,
                password: secrets.custom_password,
            });
        }
        if invited.is_empty() {
            return Err(Error::Config(if left_out > 0 {
                "folders of your Proton account cannot be shared by invite".to_owned()
            } else {
                "nothing to invite to".to_owned()
            }));
        }
        Ok((crate::invite::encode(&invited)?, left_out))
    }

    /// Whether the library already holds the share `url` links to.
    pub fn contains_link(&self, url: &str) -> Result<bool> {
        let id = format!("share-{}", token_from_url(url)?);
        Ok(self.list()?.iter().any(|share| share.id == id))
    }

    /// Record a share another installation has, with the secrets it opens
    /// with, replacing what this one holds under the same id. See
    /// [`crate::profile`].
    pub(crate) fn put_synced(&self, share: Share, secrets: Option<&ShareSecrets>) -> Result<()> {
        if let Some(secrets) = secrets {
            self.store_secrets(&share.id, secrets)?;
        }
        let mut shares = self.list()?;
        match shares.iter_mut().find(|existing| existing.id == share.id) {
            Some(existing) => *existing = share,
            None => shares.push(share),
        }
        write_json(&self.dirs.shares_file(), &shares)
    }

    /// A share's secrets, for syncing them to another installation.
    pub(crate) fn secrets_of(&self, id: &str) -> Result<ShareSecrets> {
        self.load_secrets(id)
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

    /// Call a share something else. Only the name changes: the id, which the
    /// catalog, watch state and offline files are keyed by, stays. Sync sees
    /// the new name as a change to the share's register, like any other.
    pub fn rename(&self, id: &str, name: &str) -> Result<Share> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::Config("a share needs a name".to_owned()));
        }
        let mut shares = self.list()?;
        let share = shares
            .iter_mut()
            .find(|share| share.id == id)
            .ok_or_else(|| Error::NotFound(format!("no share with id {id}")))?;
        share.name = name.to_owned();
        let renamed = share.clone();
        write_json(&self.dirs.shares_file(), &shares)?;
        Ok(renamed)
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
        Ok(self.secrets.delete(id)?)
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
        open_link(&secrets.url, secrets.custom_password.as_deref()).await
    }

    fn store_secrets(&self, id: &str, secrets: &ShareSecrets) -> Result<()> {
        let encoded = serde_json::to_string(secrets)
            .map_err(|e| Error::Config(format!("serialize share secrets: {e}")))?;
        Ok(self.secrets.set(id, &encoded)?)
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

/// Open a public link as a visitor.
async fn open_link(url: &str, custom_password: Option<&str>) -> Result<ShareClient> {
    let client = ProtonDrivePublicLinkClient::open(client_configuration(), url, custom_password)
        .await?
        // The SDK default is sized for a background sync daemon. A player that
        // seeks wants more blocks in flight; at 4 MiB each this is a 192 MiB
        // ceiling, which is unremarkable for a desktop app.
        .with_max_inflight_blocks(48);
    Ok(ShareClient::PublicLink(Box::new(client)))
}

/// Several opened shares, presented as one library.
pub struct SharedLibrary {
    clients: BTreeMap<String, ShareClient>,
    sessions: SessionAges,
    /// What the account's event feed has been read up to, and the trees it
    /// is read for. See [`SharedLibrary::changed_shares`].
    watch: tokio::sync::Mutex<Watch>,
}

/// The account folders whose changes [`SharedLibrary::changed_shares`] looks
/// for: the event cursor of each volume, and each crawled folder's node ids.
#[derive(Default)]
struct Watch {
    cursors: HashMap<VolumeId, DriveEventId>,
    trees: HashMap<String, TreeIndex>,
}

/// How long a visitor session is assumed good for without a refresh.
///
/// A refresh costs one round trip. Not refreshing costs an authentication
/// failure that the viewer can only clear by restarting or recrawling.
pub const SESSION_FRESH_FOR: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// When each share's visitor session was last known good.
///
/// Sessions are short-lived and an app routinely sits open, or in the
/// background, for hours between one episode and the next, so age is a reason
/// to reauthenticate before opening anything. A share with no entry is stale.
#[derive(Debug, Default)]
pub struct SessionAges {
    refreshed: parking_lot::Mutex<std::collections::HashMap<String, std::time::Instant>>,
}

impl SessionAges {
    /// Record that `share_id`'s session was good just now.
    pub fn mark(&self, share_id: &str) {
        self.refreshed
            .lock()
            .insert(share_id.to_owned(), std::time::Instant::now());
    }

    fn is_stale(&self, share_id: &str) -> bool {
        self.refreshed
            .lock()
            .get(share_id)
            .is_none_or(|at| at.elapsed() >= SESSION_FRESH_FOR)
    }

    /// Run `operation`, reauthenticating around it.
    ///
    /// Two guards, because there are two ways to lose a visitor session. Age is
    /// the common one, and a timestamp catches it before the request. Anything
    /// else — a session dropped early, a machine that slept through a clock
    /// change — only shows up as a failure, so a fresh-looking session gets one
    /// retry behind a refresh. `refresh` is best-effort: when it fails the
    /// operation reports the real problem, which is more useful than a refresh
    /// error standing in for it.
    pub async fn run<T, E, R, RF, O, OF>(
        &self,
        share_id: &str,
        refresh: R,
        operation: O,
    ) -> std::result::Result<T, E>
    where
        R: Fn() -> RF,
        RF: std::future::Future<Output = bool>,
        O: Fn() -> OF,
        OF: std::future::Future<Output = std::result::Result<T, E>>,
    {
        let stale = self.is_stale(share_id);
        if stale && refresh().await {
            self.mark(share_id);
        }
        match operation().await {
            Err(_) if !stale => {
                if refresh().await {
                    self.mark(share_id);
                }
                operation().await
            }
            result => result,
        }
    }
}

/// One opened share, whichever kind it is: a public link, or a folder of the
/// signed-in account.
pub use proton_drive_tree_rs::DriveTree as ShareClient;

/// What the catalog holds for a share's files, by link id: the active
/// revision a row was built from and the size it recorded. See
/// [`SharedLibrary::crawl_known`] and `Catalog::known_files`.
pub type KnownFiles = HashMap<String, (String, i64)>;

/// A share listed without unlocking file keys, where the client can.
struct LightTree<'a>(&'a ShareClient);

#[async_trait::async_trait]
impl proton_drive_tree_rs::TreeSource for LightTree<'_> {
    type Node = Node;
    type Error = ProtonError;

    async fn children(&self, folder: &NodeUid) -> std::result::Result<Vec<Node>, ProtonError> {
        match self.0 {
            ShareClient::Account { client, .. } => {
                let uids = client.enumerate_folder_children_node_uids(folder).await?;
                if uids.is_empty() {
                    return Ok(Vec::new());
                }
                client.enumerate_nodes_light(&uids).await
            }
            public => public.children(folder).await,
        }
    }

    fn is_retriable(error: &ProtonError) -> bool {
        error.is_retriable()
    }
}

/// Give every file of a light walk its claimed size: from `known` when the
/// catalog has the file at this revision, otherwise by opening it in full.
async fn complete_files(
    client: &ShareClient,
    mut nodes: Vec<Node>,
    known: &KnownFiles,
    options: &WalkOptions,
) -> Result<Vec<Node>> {
    let ShareClient::Account { client: drive, .. } = client else {
        return Ok(nodes);
    };
    let mut missing = Vec::new();
    for (index, node) in nodes.iter_mut().enumerate() {
        let proton_drive_rs::NodeKind::File {
            active_revision_id,
            claimed_size,
            ..
        } = &mut node.kind
        else {
            continue;
        };
        if claimed_size.is_some() {
            continue;
        }
        let link_id = node.uid.link_id.to_string();
        match (known.get(&link_id), active_revision_id.as_deref()) {
            (Some((revision, size)), Some(current)) if revision == current => {
                *claimed_size = Some(*size);
            }
            _ => missing.push(index),
        }
    }
    if missing.is_empty() {
        return Ok(nodes);
    }
    let uids: Vec<NodeUid> = missing
        .iter()
        .map(|&index| nodes[index].uid.clone())
        .collect();
    let full = retry(&options.retry_delays, ProtonError::is_retriable, || {
        drive.enumerate_nodes(&uids)
    })
    .await?;
    let mut full: HashMap<NodeUid, Node> = full
        .into_iter()
        .map(|node| (node.uid.clone(), node))
        .collect();
    for index in missing {
        if let Some(node) = full.remove(&nodes[index].uid) {
            nodes[index] = node;
        }
    }
    Ok(nodes)
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
        // A reused client's session is of unknown age, so it is left stale; one
        // opened here has just completed its handshake.
        let sessions = SessionAges::default();

        for share in store.list()? {
            if let Some(client) = reusable.remove(&share.id) {
                clients.insert(share.id, client);
                continue;
            }
            match store.open(&share, account).await {
                Ok(client) => {
                    sessions.mark(&share.id);
                    clients.insert(share.id.clone(), client);
                }
                Err(e) => failures.push((share, e)),
            }
        }

        Ok((
            Self {
                clients,
                sessions,
                watch: Default::default(),
            },
            failures,
        ))
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
        self.sessions.mark(share_id);
        Ok(())
    }

    /// Run `operation` against one share, refreshing its visitor session first
    /// when it is old and once more if the operation fails. See
    /// [`SessionAges::run`]. Every open of something on a share goes through
    /// here, so an app left running for hours does not need restarting to play.
    pub async fn reauthenticating<T, E, O, OF>(
        &self,
        share_id: &str,
        operation: O,
    ) -> std::result::Result<T, E>
    where
        O: Fn() -> OF,
        OF: std::future::Future<Output = std::result::Result<T, E>>,
    {
        self.sessions
            .run(
                share_id,
                || async {
                    match self.client(share_id) {
                        Some(client) => client.refresh_session().await.is_ok(),
                        None => false,
                    }
                },
                operation,
            )
            .await
    }

    /// Walk one share's whole subtree, level by level, yielding every node.
    ///
    /// The walk is [`proton_drive_tree_rs::walk`]: breadth first, six folders
    /// of a level at a time, each listing retried after a dropped connection.
    /// A media share is a root holding a folder per series and a folder per
    /// season, hundreds of siblings, so overlapping their round trips is what
    /// keeps a crawl to seconds of latency instead of minutes.
    ///
    /// An account folder is also put under watch here: see
    /// [`Self::changed_shares`].
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
        self.crawl_known(share_id, &KnownFiles::new(), progress)
            .await
    }

    /// [`Self::crawl_reporting`], told what the catalog already holds, so a
    /// recrawl only pays for what changed.
    ///
    /// A file's claimed size sits behind its own node key, and unlocking that
    /// key is an S2K derivation of tens of milliseconds — over a thousand
    /// files, most of a crawl. So an account folder is walked *light*
    /// ([`LightTree`]): names, revisions and folders, no file keys. Then only
    /// the files `known` does not already hold at their current revision are
    /// opened in full; the rest take the size the catalog has. A first crawl
    /// opens every file, as before; a recrawl of an unchanged library opens
    /// none. Public links have no light listing and are walked in full.
    pub async fn crawl_known(
        &self,
        share_id: &str,
        known: &KnownFiles,
        progress: impl Fn(usize),
    ) -> Result<Vec<Node>> {
        let client = self
            .client(share_id)
            .ok_or_else(|| Error::NotFound(format!("share {share_id} is not open")))?;

        // Before the walk, so a change made while it runs is still reported.
        self.seed_cursor(client).await;

        let options = WalkOptions::default();
        let root = retry(&options.retry_delays, ProtonError::is_retriable, || {
            client.root_node()
        })
        .await?;
        let nodes = walk(&LightTree(client), root, &options, progress)
            .await
            .map_err(|error| match error {
                WalkError::Source(error) => Error::from(error),
                too_many => Error::Config(too_many.to_string()),
            })?;
        let nodes = complete_files(client, nodes, known, &options).await?;

        if client.event_scope().is_some() {
            self.watch
                .lock()
                .await
                .trees
                .insert(share_id.to_owned(), TreeIndex::from_nodes(&nodes));
        }
        Ok(nodes)
    }

    /// Start following the event feed of `client`'s volume, if it has one and
    /// nothing follows it yet. Best-effort: without a cursor the share is
    /// simply left to the scheduled recrawl.
    async fn seed_cursor(&self, client: &ShareClient) {
        let Some((drive, scope)) = client.event_scope() else {
            return;
        };
        let mut watch = self.watch.lock().await;
        if watch.cursors.contains_key(scope.volume_id()) {
            return;
        }
        match changes::poll(drive, &scope, None).await {
            Ok(poll) => {
                watch.cursors.insert(scope.volume_id().clone(), poll.cursor);
            }
            Err(error) => tracing::debug!(%error, "cannot follow the Drive event feed"),
        }
    }

    /// The account folders that changed since they were last crawled.
    ///
    /// A folder of the viewer's own Drive has an event feed; a public link
    /// does not. Reading the feed is one request per volume, against a full
    /// crawl's hundreds, so this can run every few minutes where a recrawl
    /// runs every few hours: a file added to the folder shows up minutes
    /// later. Only folders crawled since the library opened are watched, and
    /// a feed that fails to read reports nothing; both fall back to the
    /// scheduled recrawl.
    ///
    /// The cursor moves past what this reports, so the caller is expected to
    /// crawl what it returns.
    pub async fn changed_shares(&self) -> Vec<String> {
        let mut watch = self.watch.lock().await;
        let mut changed = Vec::new();
        let volumes: Vec<(VolumeId, DriveEventId)> = watch
            .cursors
            .iter()
            .map(|(volume, cursor)| (volume.clone(), cursor.clone()))
            .collect();
        for (volume, cursor) in volumes {
            let watched: Vec<&str> = watch
                .trees
                .keys()
                .map(String::as_str)
                .filter(|id| {
                    self.client(id)
                        .and_then(ShareClient::event_scope)
                        .is_some_and(|(_, scope)| *scope.volume_id() == volume)
                })
                .collect();
            let Some((drive, scope)) = watched
                .first()
                .and_then(|id| self.client(id))
                .and_then(ShareClient::event_scope)
            else {
                continue;
            };
            let poll = match changes::poll(drive, &scope, Some(&cursor)).await {
                Ok(poll) => poll,
                Err(error) => {
                    tracing::debug!(%error, "read the Drive event feed");
                    continue;
                }
            };
            changed.extend(
                watched
                    .iter()
                    .filter(|id| watch.trees[**id].is_affected_by(&poll))
                    .map(|id| (*id).to_owned()),
            );
            watch.cursors.insert(volume, poll.cursor);
        }
        changed
    }
}

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

/// The share token a link names, or why it is not a usable share link.
///
/// The store needs a stable id for a share *before* it has opened it, and the
/// id must not be derived from the password, which would put a secret in the
/// config file by the back door.
///
/// Public so a form can say what is wrong with a link while it is still being
/// pasted, rather than after a round trip through [`ShareStore::add`] — which
/// asks exactly this first.
pub fn share_token(url: &str) -> Result<String> {
    token_from_url(url)
}

/// The SDK's parser, with messages that say what to fix. None of them repeat
/// the URL, whose fragment is the password.
fn token_from_url(url: &str) -> Result<String> {
    match proton_drive_rs::parse_public_link_url(url) {
        Ok((token, _password)) => Ok(token),
        Err(_) if !url.contains("/urls/") => {
            Err(Error::Config("not a Proton share URL".to_owned()))
        }
        Err(_) if !url.contains('#') => Err(Error::Config(
            "Proton share URL has no #password fragment; copy the full share link".to_owned(),
        )),
        Err(_) => Err(Error::Config(
            "Proton share URL is incomplete; copy the full share link".to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// An operation that fails its first `failures` calls, counting every call.
    fn flaky(
        failures: usize,
        calls: &AtomicUsize,
    ) -> impl Fn() -> std::future::Ready<std::result::Result<(), ()>> {
        move || {
            let call = calls.fetch_add(1, Ordering::SeqCst);
            std::future::ready(if call < failures { Err(()) } else { Ok(()) })
        }
    }

    #[tokio::test]
    async fn a_share_never_refreshed_is_refreshed_before_the_operation() {
        let ages = SessionAges::default();
        let (refreshes, calls) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let refresh = || {
            refreshes.fetch_add(1, Ordering::SeqCst);
            std::future::ready(true)
        };
        ages.run("s", refresh, flaky(0, &calls)).await.unwrap();
        assert_eq!(refreshes.load(Ordering::SeqCst), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_fresh_session_is_not_refreshed_when_the_operation_succeeds() {
        let ages = SessionAges::default();
        ages.mark("s");
        let (refreshes, calls) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let refresh = || {
            refreshes.fetch_add(1, Ordering::SeqCst);
            std::future::ready(true)
        };
        ages.run("s", refresh, flaky(0, &calls)).await.unwrap();
        assert_eq!(refreshes.load(Ordering::SeqCst), 0);
    }

    /// A session Proton dropped early looks fresh by age, and only the failure
    /// gives it away.
    #[tokio::test]
    async fn a_fresh_session_that_fails_is_refreshed_once_and_retried() {
        let ages = SessionAges::default();
        ages.mark("s");
        let (refreshes, calls) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let refresh = || {
            refreshes.fetch_add(1, Ordering::SeqCst);
            std::future::ready(true)
        };
        ages.run("s", refresh, flaky(1, &calls)).await.unwrap();
        assert_eq!(refreshes.load(Ordering::SeqCst), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    /// A refresh just happened, so a second one would learn nothing: the
    /// failure is the operation's own and is reported as it is.
    #[tokio::test]
    async fn a_stale_session_that_still_fails_after_its_refresh_is_not_retried() {
        let ages = SessionAges::default();
        let calls = AtomicUsize::new(0);
        let result = ages
            .run("s", || std::future::ready(true), flaky(5, &calls))
            .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_failed_refresh_leaves_the_session_stale() {
        let ages = SessionAges::default();
        let calls = AtomicUsize::new(0);
        ages.run("s", || std::future::ready(false), flaky(0, &calls))
            .await
            .unwrap();
        assert!(ages.is_stale("s"));
    }

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
        // An empty fragment is no password either.
        assert!(token_from_url("https://drive.proton.me/urls/ABC123#").is_err());
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
        fn set(&self, key: &str, value: &str) -> proton_auth_rs::Result<()> {
            self.entries
                .lock()
                .unwrap()
                .insert(key.to_owned(), value.to_owned());
            self.readable
                .store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        }

        fn get(&self, key: &str) -> proton_auth_rs::Result<Option<String>> {
            if !self.readable.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(proton_auth_rs::Error::Store(
                    "key permanently invalidated".into(),
                ));
            }
            Ok(self.entries.lock().unwrap().get(key).cloned())
        }

        fn delete(&self, key: &str) -> proton_auth_rs::Result<()> {
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

    #[test]
    fn an_invite_for_the_whole_library_carries_every_link_and_password_but_no_folder() {
        let (store, _) = store_in("invite");
        store
            .add(
                "Anime",
                "https://drive.proton.me/urls/ABC123#s3cr3t",
                Some("pw"),
            )
            .expect("add");
        store
            .add("Films", "https://drive.proton.me/urls/DEF456#s3cr3t", None)
            .expect("add");
        store
            .add_folder(
                "Mine",
                AccountFolder {
                    volume_id: "v".into(),
                    link_id: "l".into(),
                },
            )
            .expect("add folder");

        let (link, left_out) = store.invite(&[]).expect("invite");
        assert_eq!(left_out, 1);
        let shares = crate::invite::decode(&link).expect("decode");
        assert_eq!(shares.len(), 2);
        assert_eq!(shares[0].name, "Anime");
        assert_eq!(shares[0].password.as_deref(), Some("pw"));
        assert_eq!(shares[1].url, "https://drive.proton.me/urls/DEF456#s3cr3t");

        let (one, _) = store.invite(&["share-DEF456"]).expect("invite one");
        assert_eq!(crate::invite::decode(&one).expect("decode").len(), 1);
        assert!(
            store
                .contains_link("https://drive.proton.me/urls/ABC123#other")
                .unwrap()
        );
        assert!(
            !store
                .contains_link("https://drive.proton.me/urls/XYZ789#s3cr3t")
                .unwrap()
        );
    }

    #[test]
    fn an_invite_of_only_account_folders_is_refused() {
        let (store, _) = store_in("invite-folders");
        let folder = store
            .add_folder(
                "Mine",
                AccountFolder {
                    volume_id: "v".into(),
                    link_id: "l".into(),
                },
            )
            .expect("add folder");
        assert!(store.invite(&[folder.id.as_str()]).is_err());
    }

    #[test]
    fn a_renamed_share_keeps_its_id_and_refuses_an_empty_name() {
        let (store, _) = store_in("rename");
        let added = store
            .add(
                "Library",
                "https://drive.proton.me/urls/ABC123#s3cr3t",
                None,
            )
            .expect("add");

        let renamed = store.rename(&added.id, "  Anime  ").expect("rename");
        assert_eq!(renamed.id, added.id);
        assert_eq!(renamed.name, "Anime");
        assert_eq!(store.list().expect("list")[0].name, "Anime");
        assert!(store.rename(&added.id, "   ").is_err());
        assert!(store.rename("share-missing", "Anime").is_err());
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
