//! Watch history kept in the viewer's own Drive, so every device resumes where
//! another left off.
//!
//! ## Layout
//!
//! The history lives on a Drive *device* of its own, named [`DEVICE_NAME`]: the
//! same kind of entry a desktop sync client registers, so it shows up in the
//! web app under Computers rather than as a stray folder in My Files. Inside
//! it, `watch-history/` holds one JSON file per installation of this app,
//! named by an id generated once and kept in [`AppDirs::sync_file`].
//!
//! ## Why a file per installation
//!
//! Drive has no merge: two devices writing one file is last writer wins for
//! the *whole file*, and the loser's positions are gone. With a file each,
//! every installation only ever writes its own and reads everyone else's, so
//! nothing is overwritten and there is no write conflict to resolve.
//!
//! Each file carries the installation's whole watch table, and merging is per
//! episode: the newer `updated_at` wins. That is idempotent and order-free, so
//! it does not matter how many devices there are, which synced first, or that
//! a device writes back entries it learned from another. A device whose clock
//! runs ahead wins ties it should not, which is the price of not having a
//! server.
//!
//! The keys travel: `share-{token}` is derived from the link and a folder's
//! `drive-…` id from its node, so the same library added on two devices has
//! the same `(share_id, link_id)` on both. Nothing secret is in a file — the
//! link token decrypts nothing without the fragment — and the file is
//! end-to-end encrypted by Drive regardless.

use std::collections::HashMap;

use parking_lot::Mutex;
use proton_drive_rs::{DeviceType, NodeKind};
use proton_sdk::ids::NodeUid;
use serde::{Deserialize, Serialize};

use crate::account::Account;
use crate::catalog::{Catalog, WatchState};
use crate::config::{AppDirs, read_json, write_json};
use crate::error::{Error, Result};

/// The Drive device watch history is kept on.
pub const DEVICE_NAME: &str = "proton-stream";

/// The folder on that device holding one file per installation.
const FOLDER: &str = "watch-history";

/// Bumped only for a change an older build would misread. A file of a newer
/// version is skipped rather than half-understood.
const FORMAT_VERSION: u32 = 1;

/// One installation's watch history, as stored in Drive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct HistoryFile {
    version: u32,
    installation: String,
    entries: Vec<HistoryEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct HistoryEntry {
    share: String,
    link: String,
    position: f64,
    #[serde(default)]
    duration: Option<f64>,
    watched: bool,
    updated_at: i64,
}

impl HistoryEntry {
    fn from_state((share, link): (String, String), state: WatchState) -> Self {
        Self {
            share,
            link,
            position: state.position_secs,
            duration: state.duration_secs,
            watched: state.watched,
            updated_at: state.updated_at,
        }
    }

    fn state(&self) -> WatchState {
        WatchState {
            position_secs: self.position,
            duration_secs: self.duration,
            watched: self.watched,
            updated_at: self.updated_at,
        }
    }

    /// Whether this entry could have come from this app. A file is another
    /// device's to write, so its contents are checked before they reach the
    /// catalog.
    fn is_sane(&self) -> bool {
        self.position.is_finite()
            && self.position >= 0.0
            && self
                .duration
                .is_none_or(|duration| duration.is_finite() && duration >= 0.0)
            && !self.share.is_empty()
            && !self.link.is_empty()
    }
}

/// This installation's sync identity, kept beside the share list.
#[derive(Debug, Serialize, Deserialize)]
struct Identity {
    installation: String,
}

/// What one [`WatchSync::sync`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SyncReport {
    /// Episodes whose position another device had newer.
    pub applied: usize,
    /// Whether this installation's file was written.
    pub uploaded: bool,
    /// Other installations whose history was read.
    pub devices: usize,
}

/// A history file as last read, by the revision it was read at.
struct Seen {
    revision: Option<String>,
    entries: Vec<HistoryEntry>,
}

/// Syncs the catalog's watch table with the account's Drive.
pub struct WatchSync {
    installation: String,
    /// Remote files by link id. A file whose active revision has not moved
    /// since it was read is not downloaded again.
    seen: Mutex<HashMap<String, Seen>>,
    /// What this installation's own file holds, once known: what was read
    /// back at the first sync, then what was last written. Compared against
    /// the catalog so an unchanged history is not uploaded as a new revision.
    own: Mutex<Option<Vec<HistoryEntry>>>,
    /// The history folders, resolved once per account. Usually one; see
    /// [`WatchSync::folders`].
    folders: Mutex<Option<Vec<NodeUid>>>,
    /// One sync at a time: two would race on creating the device and the
    /// installation's file.
    running: tokio::sync::Mutex<()>,
}

impl WatchSync {
    /// This installation's sync, with its id read from — or, the first time,
    /// written to — [`AppDirs::sync_file`].
    pub fn open(dirs: &AppDirs) -> Result<Self> {
        let path = dirs.sync_file();
        let installation = match read_json::<Identity>(&path)? {
            Some(identity) => identity.installation,
            None => {
                let identity = Identity {
                    installation: new_installation_id()?,
                };
                write_json(&path, &identity)?;
                identity.installation
            }
        };
        Ok(Self::with_installation(installation))
    }

    fn with_installation(installation: String) -> Self {
        Self {
            installation,
            seen: Mutex::new(HashMap::new()),
            own: Mutex::new(None),
            folders: Mutex::new(None),
            running: tokio::sync::Mutex::new(()),
        }
    }

    /// Forget everything learned about the account, after signing out or in
    /// as someone else.
    pub fn reset(&self) {
        self.seen.lock().clear();
        *self.own.lock() = None;
        *self.folders.lock() = None;
    }

    /// Pull every other installation's history into `catalog`, then write
    /// this one's if it changed.
    ///
    /// The catalog is locked only around its own reads and writes, never
    /// across a request, so playback can keep saving its position while this
    /// runs.
    pub async fn sync(&self, account: &Account, catalog: &Mutex<Catalog>) -> Result<SyncReport> {
        let _running = self.running.lock().await;
        let drive = account.drive();
        let folders = self.folders(account).await?;
        let own_name = format!("{}.json", self.installation);

        let mut own_file = None;
        let mut remote = Vec::new();
        let mut devices = 0;
        for folder in &folders {
            let uids = drive.enumerate_folder_children_node_uids(folder).await?;
            if uids.is_empty() {
                continue;
            }
            for node in drive.enumerate_nodes(&uids).await? {
                let NodeKind::File {
                    active_revision_id, ..
                } = &node.kind
                else {
                    continue;
                };
                if node.trashed || !node.name.ends_with(".json") {
                    continue;
                }
                if node.name == own_name {
                    if own_file.is_none() {
                        own_file = Some((node.uid.clone(), active_revision_id.clone()));
                    }
                    continue;
                }
                devices += 1;
                remote.extend(
                    self.read(account, &node.uid, active_revision_id.clone())
                        .await,
                );
            }
        }

        if self.own.lock().is_none() {
            let entries = match &own_file {
                Some((uid, revision)) => self.read(account, uid, revision.clone()).await,
                None => Vec::new(),
            };
            // What this installation wrote before is history too: after a
            // reinstall that kept the config, or a catalog that was deleted.
            remote.extend(entries.iter().cloned());
            *self.own.lock() = Some(entries);
        }

        let incoming: Vec<(String, String, WatchState)> = remote
            .iter()
            .filter(|entry| entry.is_sane())
            .map(|entry| (entry.share.clone(), entry.link.clone(), entry.state()))
            .collect();
        let (applied, local) = {
            let mut catalog = catalog.lock();
            let applied = catalog.merge_watch_states(&incoming)?;
            (applied, catalog.all_watch_states()?)
        };

        let mut entries: Vec<HistoryEntry> = local
            .into_iter()
            .map(|(key, state)| HistoryEntry::from_state(key, state))
            .collect();
        entries.sort_by(|a, b| (&a.share, &a.link).cmp(&(&b.share, &b.link)));

        let unchanged = self.own.lock().as_ref() == Some(&entries);
        let uploaded = if unchanged || (entries.is_empty() && own_file.is_none()) {
            false
        } else {
            let file = HistoryFile {
                version: FORMAT_VERSION,
                installation: self.installation.clone(),
                entries: entries.clone(),
            };
            let bytes = serde_json::to_vec(&file)
                .map_err(|e| Error::Config(format!("serialize watch history: {e}")))?;
            match &own_file {
                Some((uid, _)) => drive.upload_new_revision(uid, &bytes).await?,
                None => {
                    drive
                        .upload_file(&folders[0], &own_name, "application/json", &bytes)
                        .await?;
                }
            }
            *self.own.lock() = Some(entries);
            true
        };

        Ok(SyncReport {
            applied,
            uploaded,
            devices,
        })
    }

    /// The entries of one history file, from the cache when its revision has
    /// not moved.
    ///
    /// A file that will not download or parse is logged and read as empty: it
    /// is another device's, and one bad file must not stop the rest syncing.
    async fn read(
        &self,
        account: &Account,
        uid: &NodeUid,
        revision: Option<String>,
    ) -> Vec<HistoryEntry> {
        let key = uid.link_id.as_str().to_owned();
        if let Some(seen) = self.seen.lock().get(&key)
            && revision.is_some()
            && seen.revision == revision
        {
            return seen.entries.clone();
        }
        let entries = match account.drive().download_file(uid).await {
            Ok(bytes) => match parse(&bytes) {
                Some(entries) => entries,
                None => {
                    tracing::warn!(%uid, "skipping a watch-history file this build cannot read");
                    Vec::new()
                }
            },
            Err(error) => {
                tracing::warn!(%uid, %error, "a watch-history file could not be downloaded");
                return Vec::new();
            }
        };
        self.seen.lock().insert(
            key,
            Seen {
                revision,
                entries: entries.clone(),
            },
        );
        entries
    }

    /// The history folders, creating the device and folder the first time.
    ///
    /// Two installations signing in at the same moment can each create the
    /// device before seeing the other's. Every device of that name is read
    /// from so neither's history is lost; the oldest is the one written to,
    /// which is the same one on every installation.
    async fn folders(&self, account: &Account) -> Result<Vec<NodeUid>> {
        if let Some(folders) = self.folders.lock().clone() {
            return Ok(folders);
        }
        let drive = account.drive();
        let mut devices: Vec<_> = drive
            .enumerate_devices()
            .await?
            .into_iter()
            .filter(|device| device.name.as_deref().ok() == Some(DEVICE_NAME))
            .collect();
        devices.sort_by_key(|device| device.creation_time);
        if devices.is_empty() {
            devices.push(drive.create_device(DEVICE_NAME, device_type()).await?);
        }
        let mut folders = Vec::with_capacity(devices.len());
        for device in &devices {
            folders.push(
                drive
                    .create_folder_path(&device.root_folder_uid, FOLDER)
                    .await?,
            );
        }
        *self.folders.lock() = Some(folders.clone());
        Ok(folders)
    }
}

/// A history file's entries, or `None` for one this build cannot read.
fn parse(bytes: &[u8]) -> Option<Vec<HistoryEntry>> {
    let file: HistoryFile = serde_json::from_slice(bytes).ok()?;
    (file.version <= FORMAT_VERSION).then_some(file.entries)
}

/// The kind of device to register, which is only the icon the web app draws.
fn device_type() -> DeviceType {
    if cfg!(target_os = "windows") {
        DeviceType::Windows
    } else if cfg!(target_os = "macos") {
        DeviceType::MacOs
    } else {
        DeviceType::Linux
    }
}

fn new_installation_id() -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|e| Error::Config(format!("no randomness for an installation id: {e}")))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(link: &str, position: f64, updated_at: i64) -> HistoryEntry {
        HistoryEntry {
            share: "share-a".to_owned(),
            link: link.to_owned(),
            position,
            duration: Some(1440.0),
            watched: false,
            updated_at,
        }
    }

    #[test]
    fn an_installation_keeps_the_id_it_was_given() {
        let root =
            std::env::temp_dir().join(format!("pstr-sync-{}", new_installation_id().unwrap()));
        let dirs = AppDirs::from_paths(root.join("c"), root.join("d"), root.join("k")).unwrap();
        let first = WatchSync::open(&dirs).unwrap().installation;
        let second = WatchSync::open(&dirs).unwrap().installation;
        assert_eq!(first, second);
        assert_eq!(first.len(), 32);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_history_file_round_trips() {
        let file = HistoryFile {
            version: FORMAT_VERSION,
            installation: "abc".to_owned(),
            entries: vec![entry("a", 12.5, 100)],
        };
        let bytes = serde_json::to_vec(&file).unwrap();
        assert_eq!(parse(&bytes), Some(file.entries));
    }

    #[test]
    fn a_history_file_from_a_newer_format_is_not_read() {
        let file = HistoryFile {
            version: FORMAT_VERSION + 1,
            installation: "abc".to_owned(),
            entries: vec![entry("a", 12.5, 100)],
        };
        assert_eq!(parse(&serde_json::to_vec(&file).unwrap()), None);
        assert_eq!(parse(b"not json"), None);
    }

    #[test]
    fn entries_another_device_could_not_have_written_are_refused() {
        assert!(entry("a", 1.0, 1).is_sane());
        assert!(!entry("a", f64::NAN, 1).is_sane());
        assert!(!entry("a", -1.0, 1).is_sane());
        assert!(!entry("", 1.0, 1).is_sane());
        let mut infinite = entry("a", 1.0, 1);
        infinite.duration = Some(f64::INFINITY);
        assert!(!infinite.is_sane());
    }

    #[test]
    fn an_entry_carries_its_watch_state_unchanged() {
        let state = WatchState {
            position_secs: 61.0,
            duration_secs: None,
            watched: true,
            updated_at: 42,
        };
        let entry = HistoryEntry::from_state(("s".to_owned(), "l".to_owned()), state);
        assert_eq!(entry.state(), state);
    }
}
