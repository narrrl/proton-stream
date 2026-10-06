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
use proton_drive_rs::{DeviceType, NodeKind, ProtonDriveClient};
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
    /// Files of installations gone quiet that were moved to the trash, their
    /// history now carried in this installation's own file.
    pub retired: usize,
}

/// How long an installation's file may go unwritten before another
/// installation retires it.
///
/// Reinstalling — routine on Android, where uninstalling wipes the config —
/// starts a new id, and every file ever written would otherwise be read by
/// every device forever. Retiring is safe at any age: a file is only trashed
/// after its entries were merged and this installation's file, which then
/// holds them too, is written. An installation that was merely idle finds its
/// file gone on its next sync and writes it again.
const RETIRE_AFTER_SECS: i64 = 90 * 24 * 60 * 60;

/// One history file in Drive, as listed.
#[derive(Debug, Clone)]
pub(crate) struct RemoteFile {
    pub uid: NodeUid,
    pub name: String,
    /// The active revision, which moves with every write.
    pub revision: Option<String>,
    /// Last modification, epoch seconds.
    pub modified: i64,
}

/// The few Drive operations sync needs: the account's Drive in the app, an
/// in-memory one in the tests.
pub(crate) trait HistoryDrive: Sync {
    /// The history folders, creating the device and folder the first time.
    /// See [`WatchSync::folders`].
    fn history_folders(&self) -> impl Future<Output = Result<Vec<NodeUid>>> + Send;
    /// The untrashed `.json` files directly in `folder`.
    fn list(&self, folder: &NodeUid) -> impl Future<Output = Result<Vec<RemoteFile>>> + Send;
    fn download(&self, uid: &NodeUid) -> impl Future<Output = Result<Vec<u8>>> + Send;
    fn create(
        &self,
        folder: &NodeUid,
        name: &str,
        bytes: &[u8],
    ) -> impl Future<Output = Result<()>> + Send;
    fn replace(&self, uid: &NodeUid, bytes: &[u8]) -> impl Future<Output = Result<()>> + Send;
    fn trash(&self, uids: &[NodeUid]) -> impl Future<Output = Result<()>> + Send;
}

impl HistoryDrive for ProtonDriveClient {
    /// Two installations signing in at the same moment can each create the
    /// device before seeing the other's. Every device of that name is read
    /// from so neither's history is lost; the oldest is the one written to,
    /// which is the same one on every installation.
    async fn history_folders(&self) -> Result<Vec<NodeUid>> {
        let mut devices: Vec<_> = self
            .enumerate_devices()
            .await?
            .into_iter()
            .filter(|device| device.name.as_deref().ok() == Some(DEVICE_NAME))
            .collect();
        devices.sort_by_key(|device| device.creation_time);
        if devices.is_empty() {
            devices.push(self.create_device(DEVICE_NAME, device_type()).await?);
        }
        let mut folders = Vec::with_capacity(devices.len());
        for device in &devices {
            folders.push(
                self.create_folder_path(&device.root_folder_uid, FOLDER)
                    .await?,
            );
        }
        Ok(folders)
    }

    async fn list(&self, folder: &NodeUid) -> Result<Vec<RemoteFile>> {
        let uids = self.enumerate_folder_children_node_uids(folder).await?;
        if uids.is_empty() {
            return Ok(Vec::new());
        }
        Ok(self
            .enumerate_nodes(&uids)
            .await?
            .into_iter()
            .filter_map(|node| {
                let NodeKind::File {
                    active_revision_id, ..
                } = node.kind
                else {
                    return None;
                };
                (!node.trashed && node.name.ends_with(".json")).then_some(RemoteFile {
                    uid: node.uid,
                    name: node.name,
                    revision: active_revision_id,
                    modified: node.modification_time,
                })
            })
            .collect())
    }

    async fn download(&self, uid: &NodeUid) -> Result<Vec<u8>> {
        Ok(self.download_file(uid).await?)
    }

    async fn create(&self, folder: &NodeUid, name: &str, bytes: &[u8]) -> Result<()> {
        self.upload_file(folder, name, "application/json", bytes)
            .await?;
        Ok(())
    }

    async fn replace(&self, uid: &NodeUid, bytes: &[u8]) -> Result<()> {
        Ok(self.upload_new_revision(uid, bytes).await?)
    }

    async fn trash(&self, uids: &[NodeUid]) -> Result<()> {
        for (uid, outcome) in self.trash_nodes(uids).await? {
            if let Err(error) = outcome {
                tracing::warn!(%uid, %error, "a retired watch-history file was not trashed");
            }
        }
        Ok(())
    }
}

/// A history file as last read, by the revision it was read at. `None`
/// entries for one this build could not read.
struct Seen {
    revision: Option<String>,
    entries: Option<Vec<HistoryEntry>>,
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
    /// [`HistoryDrive::history_folders`].
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

    /// Pull every other installation's history into `catalog`, write this
    /// one's if it changed, then retire the files of installations gone quiet.
    ///
    /// The catalog is locked only around its own reads and writes, never
    /// across a request, so playback can keep saving its position while this
    /// runs.
    pub async fn sync(&self, account: &Account, catalog: &Mutex<Catalog>) -> Result<SyncReport> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs() as i64);
        self.sync_with(account.drive(), catalog, now).await
    }

    pub(crate) async fn sync_with<D: HistoryDrive>(
        &self,
        drive: &D,
        catalog: &Mutex<Catalog>,
        now: i64,
    ) -> Result<SyncReport> {
        let _running = self.running.lock().await;
        let folders = self.folders(drive).await?;
        let own_name = format!("{}.json", self.installation);

        let mut own_file = None;
        let mut others = Vec::new();
        for folder in &folders {
            for file in drive.list(folder).await? {
                if file.name == own_name {
                    own_file.get_or_insert(file);
                } else {
                    others.push(file);
                }
            }
        }

        let mut remote = Vec::new();
        // Files whose every entry has been read, and so can be retired once
        // this installation's file carries them.
        let mut readable = Vec::new();
        for file in &others {
            if let Some(entries) = self.read(drive, file).await {
                remote.extend(entries);
                readable.push(file);
            }
        }

        // What this installation wrote before is history too: after a
        // reinstall that kept the config, or a catalog that was deleted. One
        // that cannot be read now is left alone rather than overwritten with
        // less than it holds, and read again next time.
        let mut own_unreadable = false;
        if self.own.lock().is_none() {
            let entries = match &own_file {
                Some(file) => self.read(drive, file).await,
                None => Some(Vec::new()),
            };
            match entries {
                Some(entries) => {
                    remote.extend(entries.iter().cloned());
                    *self.own.lock() = Some(entries);
                }
                None => own_unreadable = true,
            }
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

        // A file that is missing is written even when what it would hold has
        // not changed: another installation retired it, or it was deleted by
        // hand, and either way the history it carried lives only here now.
        let unchanged = own_file.is_some() && self.own.lock().as_ref() == Some(&entries);
        let uploaded = if own_unreadable || unchanged || (entries.is_empty() && own_file.is_none())
        {
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
                Some(file) => drive.replace(&file.uid, &bytes).await?,
                None => drive.create(&folders[0], &own_name, &bytes).await?,
            }
            *self.own.lock() = Some(entries);
            true
        };

        // Only now: every entry of a retired file was merged above, and this
        // installation's file — just written, or already up to date — holds
        // the merged table.
        let retired: Vec<NodeUid> = readable
            .into_iter()
            .filter(|file| !own_unreadable && now - file.modified >= RETIRE_AFTER_SECS)
            .map(|file| file.uid.clone())
            .collect();
        if !retired.is_empty() {
            match drive.trash(&retired).await {
                Ok(()) => {
                    let mut seen = self.seen.lock();
                    for uid in &retired {
                        seen.remove(uid.link_id.as_str());
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "retiring old watch-history files failed");
                }
            }
        }

        Ok(SyncReport {
            applied,
            uploaded,
            devices: others.len(),
            retired: retired.len(),
        })
    }

    /// The entries of one history file, from the cache when its revision has
    /// not moved, or `None` when it cannot be read.
    ///
    /// A file that will not download or parse is logged and skipped: it is
    /// another device's, and one bad file must not stop the rest syncing.
    async fn read<D: HistoryDrive>(
        &self,
        drive: &D,
        file: &RemoteFile,
    ) -> Option<Vec<HistoryEntry>> {
        let key = file.uid.link_id.as_str().to_owned();
        if let Some(seen) = self.seen.lock().get(&key)
            && file.revision.is_some()
            && seen.revision == file.revision
        {
            return seen.entries.clone();
        }
        let uid = &file.uid;
        let entries = match drive.download(uid).await {
            Ok(bytes) => {
                let entries = parse(&bytes);
                if entries.is_none() {
                    tracing::warn!(%uid, "skipping a watch-history file this build cannot read");
                }
                entries
            }
            Err(error) => {
                // Not cached: a download that failed is tried again next time.
                tracing::warn!(%uid, %error, "a watch-history file could not be downloaded");
                return None;
            }
        };
        self.seen.lock().insert(
            key,
            Seen {
                revision: file.revision.clone(),
                entries: entries.clone(),
            },
        );
        entries
    }

    /// The history folders, resolved once per account.
    async fn folders<D: HistoryDrive>(&self, drive: &D) -> Result<Vec<NodeUid>> {
        if let Some(folders) = self.folders.lock().clone() {
            return Ok(folders);
        }
        let folders = drive.history_folders().await?;
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

    /// One history folder in memory, standing in for the account's Drive.
    #[derive(Default)]
    struct FakeDrive {
        files: Mutex<Vec<FakeFile>>,
        /// What the next write is stamped with, epoch seconds.
        clock: std::sync::atomic::AtomicI64,
        /// Downloads fail while set, as on a dropped connection.
        offline: std::sync::atomic::AtomicBool,
        uploads: std::sync::atomic::AtomicUsize,
    }

    struct FakeFile {
        uid: NodeUid,
        name: String,
        bytes: Vec<u8>,
        revision: u32,
        modified: i64,
        trashed: bool,
    }

    fn uid(link: &str) -> NodeUid {
        NodeUid::new("volume".to_owned().into(), link.to_owned().into())
    }

    impl FakeDrive {
        fn at(&self, now: i64) {
            self.clock.store(now, std::sync::atomic::Ordering::SeqCst);
        }

        fn names(&self) -> Vec<String> {
            let mut names: Vec<_> = self
                .files
                .lock()
                .iter()
                .filter(|file| !file.trashed)
                .map(|file| file.name.clone())
                .collect();
            names.sort();
            names
        }

        fn uploads(&self) -> usize {
            self.uploads.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn put(&self, name: &str, bytes: Vec<u8>) {
            let modified = self.clock.load(std::sync::atomic::Ordering::SeqCst);
            let mut files = self.files.lock();
            let link = format!("file-{}", files.len());
            files.push(FakeFile {
                uid: uid(&link),
                name: name.to_owned(),
                bytes,
                revision: 1,
                modified,
                trashed: false,
            });
        }
    }

    impl HistoryDrive for FakeDrive {
        async fn history_folders(&self) -> Result<Vec<NodeUid>> {
            Ok(vec![uid("folder")])
        }

        async fn list(&self, _folder: &NodeUid) -> Result<Vec<RemoteFile>> {
            Ok(self
                .files
                .lock()
                .iter()
                .filter(|file| !file.trashed)
                .map(|file| RemoteFile {
                    uid: file.uid.clone(),
                    name: file.name.clone(),
                    revision: Some(file.revision.to_string()),
                    modified: file.modified,
                })
                .collect())
        }

        async fn download(&self, uid: &NodeUid) -> Result<Vec<u8>> {
            if self.offline.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(Error::NotFound("offline".to_owned()));
            }
            self.files
                .lock()
                .iter()
                .find(|file| &file.uid == uid)
                .map(|file| file.bytes.clone())
                .ok_or_else(|| Error::NotFound(uid.to_string()))
        }

        async fn create(&self, _folder: &NodeUid, name: &str, bytes: &[u8]) -> Result<()> {
            self.uploads
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.put(name, bytes.to_vec());
            Ok(())
        }

        async fn replace(&self, uid: &NodeUid, bytes: &[u8]) -> Result<()> {
            self.uploads
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let modified = self.clock.load(std::sync::atomic::Ordering::SeqCst);
            let mut files = self.files.lock();
            let file = files
                .iter_mut()
                .find(|file| &file.uid == uid)
                .ok_or_else(|| Error::NotFound(uid.to_string()))?;
            file.bytes = bytes.to_vec();
            file.revision += 1;
            file.modified = modified;
            Ok(())
        }

        async fn trash(&self, uids: &[NodeUid]) -> Result<()> {
            for file in self.files.lock().iter_mut() {
                if uids.contains(&file.uid) {
                    file.trashed = true;
                }
            }
            Ok(())
        }
    }

    /// An installation: its sync and its catalog.
    fn device(id: &str) -> (WatchSync, Mutex<Catalog>) {
        (
            WatchSync::with_installation(id.to_owned()),
            Mutex::new(Catalog::in_memory().unwrap()),
        )
    }

    fn watched_at(position_secs: f64, updated_at: i64) -> WatchState {
        WatchState {
            position_secs,
            duration_secs: Some(1440.0),
            watched: false,
            updated_at,
        }
    }

    const DAY: i64 = 24 * 60 * 60;

    #[tokio::test]
    async fn a_position_saved_on_one_device_resumes_on_another() {
        let drive = FakeDrive::default();
        let (laptop, laptop_catalog) = device("laptop");
        let (phone, phone_catalog) = device("phone");
        laptop_catalog
            .lock()
            .set_watch_state("share-a", "ep1", &watched_at(600.0, 100))
            .unwrap();

        let first = laptop
            .sync_with(&drive, &laptop_catalog, 100)
            .await
            .unwrap();
        assert!(first.uploaded);
        let second = phone.sync_with(&drive, &phone_catalog, 110).await.unwrap();

        assert_eq!(second.applied, 1);
        assert_eq!(second.devices, 1);
        assert_eq!(
            phone_catalog.lock().watch_state("share-a", "ep1").unwrap(),
            Some(watched_at(600.0, 100))
        );
        assert_eq!(drive.names(), ["laptop.json", "phone.json"]);
    }

    #[tokio::test]
    async fn the_newer_position_wins_whichever_device_syncs_first() {
        let drive = FakeDrive::default();
        let (laptop, laptop_catalog) = device("laptop");
        let (phone, phone_catalog) = device("phone");
        laptop_catalog
            .lock()
            .set_watch_state("share-a", "ep1", &watched_at(300.0, 100))
            .unwrap();
        phone_catalog
            .lock()
            .set_watch_state("share-a", "ep1", &watched_at(900.0, 200))
            .unwrap();

        laptop
            .sync_with(&drive, &laptop_catalog, 300)
            .await
            .unwrap();
        phone.sync_with(&drive, &phone_catalog, 300).await.unwrap();
        laptop
            .sync_with(&drive, &laptop_catalog, 300)
            .await
            .unwrap();

        for catalog in [&laptop_catalog, &phone_catalog] {
            assert_eq!(
                catalog.lock().watch_state("share-a", "ep1").unwrap(),
                Some(watched_at(900.0, 200))
            );
        }
    }

    #[tokio::test]
    async fn an_unchanged_history_is_not_uploaded_again() {
        let drive = FakeDrive::default();
        let (laptop, catalog) = device("laptop");
        catalog
            .lock()
            .set_watch_state("share-a", "ep1", &watched_at(60.0, 100))
            .unwrap();

        laptop.sync_with(&drive, &catalog, 100).await.unwrap();
        let again = laptop.sync_with(&drive, &catalog, 200).await.unwrap();

        assert!(!again.uploaded);
        assert_eq!(drive.uploads(), 1);
    }

    #[tokio::test]
    async fn nothing_is_uploaded_before_anything_was_watched() {
        let drive = FakeDrive::default();
        let (laptop, catalog) = device("laptop");
        let report = laptop.sync_with(&drive, &catalog, 100).await.unwrap();
        assert!(!report.uploaded);
        assert!(drive.names().is_empty());
    }

    #[tokio::test]
    async fn a_file_that_went_missing_is_written_again() {
        let drive = FakeDrive::default();
        let (laptop, catalog) = device("laptop");
        catalog
            .lock()
            .set_watch_state("share-a", "ep1", &watched_at(60.0, 100))
            .unwrap();
        laptop.sync_with(&drive, &catalog, 100).await.unwrap();

        let own: Vec<NodeUid> = drive.files.lock().iter().map(|f| f.uid.clone()).collect();
        drive.trash(&own).await.unwrap();
        let report = laptop.sync_with(&drive, &catalog, 200).await.unwrap();

        assert!(report.uploaded);
        assert_eq!(drive.names(), ["laptop.json"]);
    }

    #[tokio::test]
    async fn history_this_installation_wrote_before_comes_back_into_an_empty_catalog() {
        let drive = FakeDrive::default();
        let (laptop, catalog) = device("laptop");
        catalog
            .lock()
            .set_watch_state("share-a", "ep1", &watched_at(60.0, 100))
            .unwrap();
        laptop.sync_with(&drive, &catalog, 100).await.unwrap();

        // The catalog was deleted; the config, and with it the id, was not.
        let (relaunched, empty) = device("laptop");
        let report = relaunched.sync_with(&drive, &empty, 200).await.unwrap();

        assert_eq!(report.applied, 1);
        assert!(!report.uploaded);
        assert_eq!(
            empty.lock().watch_state("share-a", "ep1").unwrap(),
            Some(watched_at(60.0, 100))
        );
    }

    #[tokio::test]
    async fn an_own_file_that_cannot_be_read_is_not_overwritten() {
        let drive = FakeDrive::default();
        let (laptop, catalog) = device("laptop");
        catalog
            .lock()
            .set_watch_state("share-a", "ep1", &watched_at(60.0, 100))
            .unwrap();
        laptop.sync_with(&drive, &catalog, 100).await.unwrap();

        let (relaunched, other) = device("laptop");
        other
            .lock()
            .set_watch_state("share-a", "ep2", &watched_at(5.0, 150))
            .unwrap();
        drive
            .offline
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let report = relaunched.sync_with(&drive, &other, 200).await.unwrap();
        assert!(!report.uploaded);

        drive
            .offline
            .store(false, std::sync::atomic::Ordering::SeqCst);
        relaunched.sync_with(&drive, &other, 300).await.unwrap();
        let state = other.lock().all_watch_states().unwrap();
        assert_eq!(state.len(), 2, "both episodes survive: {state:?}");
    }

    #[tokio::test]
    async fn a_file_from_a_newer_build_is_neither_merged_nor_retired() {
        let drive = FakeDrive::default();
        let future = HistoryFile {
            version: FORMAT_VERSION + 1,
            installation: "future".to_owned(),
            entries: vec![entry("ep1", 60.0, 100)],
        };
        drive.put("future.json", serde_json::to_vec(&future).unwrap());
        let (laptop, catalog) = device("laptop");
        catalog
            .lock()
            .set_watch_state("share-a", "ep2", &watched_at(5.0, 100))
            .unwrap();

        let report = laptop
            .sync_with(&drive, &catalog, RETIRE_AFTER_SECS + DAY)
            .await
            .unwrap();

        assert_eq!(report.applied, 0);
        assert_eq!(report.retired, 0);
        assert_eq!(drive.names(), ["future.json", "laptop.json"]);
    }

    #[tokio::test]
    async fn a_quiet_installations_file_is_retired_once_its_history_is_carried() {
        let drive = FakeDrive::default();
        let (old_phone, old_catalog) = device("old-phone");
        old_catalog
            .lock()
            .set_watch_state("share-a", "ep1", &watched_at(600.0, 100))
            .unwrap();
        drive.at(100);
        old_phone
            .sync_with(&drive, &old_catalog, 100)
            .await
            .unwrap();

        let (phone, catalog) = device("phone");
        let later = 100 + RETIRE_AFTER_SECS;
        drive.at(later);
        let report = phone.sync_with(&drive, &catalog, later).await.unwrap();

        assert_eq!(report.retired, 1);
        assert_eq!(drive.names(), ["phone.json"]);
        // Nothing was lost: the newcomer's file carries it now.
        let (fresh, fresh_catalog) = device("fresh");
        fresh
            .sync_with(&drive, &fresh_catalog, later)
            .await
            .unwrap();
        assert_eq!(
            fresh_catalog.lock().watch_state("share-a", "ep1").unwrap(),
            Some(watched_at(600.0, 100))
        );
    }

    #[tokio::test]
    async fn a_recent_installations_file_is_kept() {
        let drive = FakeDrive::default();
        let (laptop, laptop_catalog) = device("laptop");
        laptop_catalog
            .lock()
            .set_watch_state("share-a", "ep1", &watched_at(600.0, 100))
            .unwrap();
        drive.at(100);
        laptop
            .sync_with(&drive, &laptop_catalog, 100)
            .await
            .unwrap();

        let (phone, catalog) = device("phone");
        let report = phone
            .sync_with(&drive, &catalog, 100 + RETIRE_AFTER_SECS - DAY)
            .await
            .unwrap();

        assert_eq!(report.retired, 0);
        assert_eq!(drive.names(), ["laptop.json", "phone.json"]);
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
