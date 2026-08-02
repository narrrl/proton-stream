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

use std::collections::BTreeMap;

use proton_drive_rs::{Node, ProtonDrivePublicLinkClient};
use proton_sdk::config::ProtonClientConfiguration;
use proton_sdk::ids::NodeUid;
use serde::{Deserialize, Serialize};

use crate::config::{AppDirs, read_json, write_json};
use crate::error::{Error, Result};

/// The credential-store service name. One entry per share, keyed by share id.
const KEYRING_SERVICE: &str = "proton-stream";

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
}

impl ShareStore {
    pub fn new(dirs: AppDirs) -> Self {
        Self { dirs }
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
        };
        shares.push(share.clone());
        write_json(&self.dirs.shares_file(), &shares)?;
        Ok(share)
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
        match self.entry(id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Open a share as a visitor.
    pub async fn open(&self, share: &Share) -> Result<ProtonDrivePublicLinkClient> {
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
        Ok(client)
    }

    fn entry(&self, id: &str) -> Result<keyring::Entry> {
        Ok(keyring::Entry::new(KEYRING_SERVICE, id)?)
    }

    fn store_secrets(&self, id: &str, secrets: &ShareSecrets) -> Result<()> {
        let encoded = serde_json::to_string(secrets)
            .map_err(|e| Error::Config(format!("serialize share secrets: {e}")))?;
        self.entry(id)?.set_password(&encoded)?;
        Ok(())
    }

    fn load_secrets(&self, id: &str) -> Result<ShareSecrets> {
        let encoded = match self.entry(id)?.get_password() {
            Ok(encoded) => encoded,
            Err(keyring::Error::NoEntry) => {
                return Err(Error::NotFound(format!(
                    "share {id} has no stored credentials; remove and re-add it"
                )));
            }
            Err(e) => return Err(e.into()),
        };
        serde_json::from_str(&encoded)
            .map_err(|e| Error::Config(format!("stored share secrets are unreadable: {e}")))
    }
}

/// Several opened shares, presented as one library.
pub struct SharedLibrary {
    clients: BTreeMap<String, ProtonDrivePublicLinkClient>,
}

impl SharedLibrary {
    /// Open every configured share.
    ///
    /// A share that fails to open is reported and skipped rather than failing
    /// the whole library — one revoked link should not make the other three
    /// unwatchable.
    pub async fn open_all(store: &ShareStore) -> Result<(Self, Vec<(Share, Error)>)> {
        let mut clients = BTreeMap::new();
        let mut failures = Vec::new();

        for share in store.list()? {
            match store.open(&share).await {
                Ok(client) => {
                    clients.insert(share.id.clone(), client);
                }
                Err(e) => failures.push((share, e)),
            }
        }

        Ok((Self { clients }, failures))
    }

    /// The client for one share.
    pub fn client(&self, share_id: &str) -> Option<&ProtonDrivePublicLinkClient> {
        self.clients.get(share_id)
    }

    /// Every opened share id.
    pub fn share_ids(&self) -> impl Iterator<Item = &str> {
        self.clients.keys().map(String::as_str)
    }

    /// Walk one share's whole subtree, depth first, yielding every node.
    ///
    /// There is no recursion helper on the visitor path — the authenticated
    /// client's `get_node_hierarchy` / `get_node_by_path` have no counterpart
    /// there — so the walk is here.
    pub async fn crawl(&self, share_id: &str) -> Result<Vec<Node>> {
        let client = self
            .client(share_id)
            .ok_or_else(|| Error::NotFound(format!("share {share_id} is not open")))?;

        let root = client.get_root_node().await?;
        let mut found = Vec::new();
        let mut folders = vec![root.uid.clone()];
        found.push(root);

        while let Some(folder) = folders.pop() {
            let child_uids: Vec<NodeUid> =
                client.enumerate_folder_children_node_uids(&folder).await?;
            if child_uids.is_empty() {
                continue;
            }
            // Chunked and fanned out inside the SDK, and the node keys it
            // unlocks along the way are cached there, so a deep tree does not
            // re-derive the same ancestors per child.
            let children = client.enumerate_nodes(&child_uids).await?;
            for child in children {
                if child.is_folder() {
                    folders.push(child.uid.clone());
                }
                found.push(child);
            }
        }

        Ok(found)
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
fn client_configuration() -> ProtonClientConfiguration {
    ProtonClientConfiguration::new(APP_VERSION).with_user_agent(USER_AGENT)
}

/// The token out of a share URL: the path segment after `/urls/`.
///
/// Parsed here as well as in the SDK because the store needs a stable id for a
/// share *before* it has opened it — and the id must not be derived from the
/// password, which would put a secret in the config file by the back door.
fn token_from_url(url: &str) -> Result<String> {
    let (_, tail) = url
        .split_once("/urls/")
        .ok_or_else(|| Error::Config(format!("{url} is not a Proton share URL")))?;

    let token = tail
        .split(['#', '?'])
        .next()
        .unwrap_or_default()
        .trim_end_matches('/');

    if token.is_empty() {
        return Err(Error::Config(format!("{url} carries no share token")));
    }
    if !url.contains('#') {
        return Err(Error::Config(format!(
            "{url} has no #password fragment; copy the full share link"
        )));
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

    /// The id must be derivable from the token alone — deriving it from the URL
    /// would mix a secret into the config file.
    #[test]
    fn the_share_id_carries_no_secret() {
        let token = token_from_url("https://drive.proton.me/urls/ABC123#s3cr3t").unwrap();
        let id = format!("share-{token}");
        assert!(!id.contains("s3cr3t"), "id must not embed the password");
    }
}
