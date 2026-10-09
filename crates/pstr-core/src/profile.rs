//! The rest of what makes the app look the same on every device: the shares,
//! the settings, the hand-picked metadata matches and the per-show track
//! choices. Synced next to watch history by [`crate::sync`], in the same file.
//!
//! ## Registers
//!
//! Each synced thing is one *register*: a key (`share/<id>`,
//! `settings/playback`, `tracks/<title>`, …), its latest value, and when it
//! last changed. Merging is the same rule as watch history — the newer
//! `updated_at` wins, per key — so it is idempotent and order-free across any
//! number of devices. A removed share is a register with no value, so a
//! removal travels like any other change.
//!
//! ## Noticing changes
//!
//! Nothing that writes a setting or a share knows about sync. Instead
//! `sync-state.json` remembers every register as last synced, and each sync
//! compares it with what is on this device: whatever differs changed since,
//! and is stamped with the time of the sync. That costs a change made between
//! syncs its exact time, which matters only when two devices change the same
//! setting within one sync interval of each other.
//!
//! The first sync of an installation stamps what it finds at zero rather than
//! now. A fresh install holds only defaults, and those must not outrank the
//! library every other device already agreed on; what it has that nobody else
//! does is still added, since nothing older competes with it.
//!
//! ## What does not travel
//!
//! The volume and mute, which belong to the speakers in front of the viewer,
//! and every setting the front ends keep themselves: the cache budget, Wi-Fi
//! only, background audio, hardware decoding, window geometry.
//!
//! ## Secrets
//!
//! A share's register carries its link, fragment included, and its custom
//! password, and the TMDB API key travels too: without them another device
//! could list the share but never open it. The file holding them is in the
//! viewer's own Drive, end-to-end encrypted with their account's keys, which is
//! the same trust the credential store gives them locally; they are never
//! written anywhere else in plain text.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::catalog::{Catalog, TitleTrackPrefs};
use crate::config::{AppDirs, read_json, write_json};
use crate::error::Result;
use crate::metadata::{MetadataConfig, MetadataRecord, ProviderId, TitleMetadata};
use crate::shares::{AccountFolder, Share, ShareSecrets, ShareStore};

const SHARE: &str = "share/";
const TRACKS: &str = "tracks/";
const MATCH: &str = "match/";

/// The settings files that travel, by register, and the fields in each that
/// stay on the device that wrote them.
const SETTINGS: [(&str, &str, &[&str]); 3] = [
    ("settings/playback", "playback.json", &["volume", "muted"]),
    ("settings/appearance", "appearance.json", &[]),
    ("settings/metadata", "metadata.json", &[]),
];

const METADATA_SETTINGS: &str = "settings/metadata";

/// One synced value and when it last changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Register {
    /// `None` is a removal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    pub updated_at: i64,
}

pub(crate) type Registers = BTreeMap<String, Register>;

/// What this installation last synced, kept beside its sync identity.
#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    registers: Registers,
}

/// A share as it travels: everything another device needs to open it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SyncedShare {
    name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    custom_password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    folder: Option<AccountFolder>,
}

/// A match the viewer picked by hand. Automatic ones are not synced: every
/// device with enrichment on finds them again, and they expire anyway.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SyncedMatch {
    provider: ProviderId,
    metadata: Option<TitleMetadata>,
    fetched_at: i64,
}

/// What applying other devices' registers changed here, for the front end to
/// act on: reopen and crawl shares, repaint, reload the library.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileChanges {
    /// Shares this device did not have, now configured. Not yet crawled.
    pub shares_added: Vec<String>,
    /// Shares whose name or secrets changed.
    pub shares_changed: Vec<String>,
    /// Shares removed elsewhere, now gone from the configuration here. Their
    /// catalog rows and offline files are the front end's to drop, the same
    /// way it does for a share removed on this device.
    pub shares_removed: Vec<String>,
    /// Whether any settings file or the TMDB key changed.
    pub settings: bool,
    /// Hand-picked matches and per-show track choices taken.
    pub titles: usize,
}

impl ProfileChanges {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn shares(&self) -> bool {
        !(self.shares_added.is_empty()
            && self.shares_changed.is_empty()
            && self.shares_removed.is_empty())
    }
}

/// The local side of profile sync: where the state file lives, and the stores
/// registers are read from and applied to.
pub(crate) struct Profile<'a> {
    pub store: &'a ShareStore,
    pub catalog: &'a Mutex<Catalog>,
}

/// What is on this device, register by register.
#[derive(Debug, Default)]
struct Observed {
    values: BTreeMap<String, Value>,
    /// Keys that exist but could not be read — a share whose secret will not
    /// decrypt. Neither a change nor a removal.
    unknown: BTreeSet<String>,
    /// Whether the share list itself could be read. Without it, a share
    /// missing from `values` says nothing about whether it was removed.
    shares_listed: bool,
}

fn state_file(dirs: &AppDirs) -> PathBuf {
    dirs.config.join("sync-state.json")
}

fn tmdb_secret() -> String {
    format!("metadata-{}", ProviderId::Tmdb.as_str())
}

const TMDB_KEY: &str = "secret/metadata-tmdb";

impl Profile<'_> {
    fn dirs(&self) -> &AppDirs {
        &self.store.dirs
    }

    /// Read the state, notice what changed here since, take what other
    /// devices have newer, and write the state back. Returns the registers to
    /// upload and what changed on this device.
    pub fn sync<'r>(
        &self,
        remote: impl IntoIterator<Item = &'r Registers>,
        now: i64,
    ) -> Result<(Registers, ProfileChanges)> {
        let path = state_file(self.dirs());
        let stored = read_json::<State>(&path)?;
        let first = stored.is_none();
        let mut state = stored.unwrap_or_default();

        let observed = self.observe();
        stamp(&mut state.registers, observed, if first { 0 } else { now });
        let winners = merge(&state.registers, remote);
        let changes = self.apply(&mut state.registers, winners);
        write_json(&path, &state)?;
        Ok((state.registers, changes))
    }

    fn observe(&self) -> Observed {
        let mut observed = Observed::default();
        for (key, file, local) in SETTINGS {
            match read_json::<Value>(&self.dirs().config.join(file)) {
                Ok(Some(value)) => {
                    observed
                        .values
                        .insert(key.to_owned(), without(value, local));
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(%error, file, "a settings file was not synced");
                    observed.unknown.insert(key.to_owned());
                }
            }
        }
        match self.store.secrets.get(&tmdb_secret()) {
            Ok(Some(key)) => {
                observed
                    .values
                    .insert(TMDB_KEY.to_owned(), Value::String(key));
            }
            Ok(None) => {}
            Err(_) => {
                observed.unknown.insert(TMDB_KEY.to_owned());
            }
        }

        if let Ok(shares) = self.store.list() {
            observed.shares_listed = true;
            for share in shares {
                let key = format!("{SHARE}{}", share.id);
                let secrets = if share.folder.is_some() {
                    None
                } else {
                    match self.store.secrets_of(&share.id) {
                        Ok(secrets) => Some(secrets),
                        Err(_) => {
                            observed.unknown.insert(key);
                            continue;
                        }
                    }
                };
                let synced = SyncedShare {
                    name: share.name,
                    token: share.token,
                    url: secrets.as_ref().map(|secrets| secrets.url.clone()),
                    custom_password: secrets.and_then(|secrets| secrets.custom_password),
                    folder: share.folder,
                };
                if let Ok(value) = serde_json::to_value(synced) {
                    observed.values.insert(key, value);
                }
            }
        }

        let catalog = self.catalog.lock();
        if let Ok(tracks) = catalog.all_title_track_prefs() {
            for (title, prefs) in tracks {
                if let Ok(value) = serde_json::to_value(prefs) {
                    observed.values.insert(format!("{TRACKS}{title}"), value);
                }
            }
        }
        if let Ok(records) = catalog.all_metadata() {
            for (title, record) in records {
                if !record.manual {
                    continue;
                }
                let synced = SyncedMatch {
                    provider: record.provider,
                    metadata: record.metadata,
                    fetched_at: record.fetched_at,
                };
                if let Ok(value) = serde_json::to_value(synced) {
                    observed.values.insert(format!("{MATCH}{title}"), value);
                }
            }
        }
        observed
    }

    /// Apply each winning register here, recording it in `registers` once it
    /// took. One that fails is left out, so the next sync tries it again.
    fn apply(&self, registers: &mut Registers, winners: Registers) -> ProfileChanges {
        let mut changes = ProfileChanges::default();
        for (key, register) in winners {
            match self.apply_one(&key, register.value.as_ref(), &mut changes) {
                Ok(()) => {
                    registers.insert(key, register);
                }
                Err(error) => tracing::warn!(%error, key, "a synced change was not applied"),
            }
        }
        changes
    }

    fn apply_one(
        &self,
        key: &str,
        value: Option<&Value>,
        changes: &mut ProfileChanges,
    ) -> Result<()> {
        if let Some(id) = key.strip_prefix(SHARE) {
            return self.apply_share(id, value, changes);
        }
        if let Some(title) = key.strip_prefix(TRACKS) {
            if let Some(value) = value {
                let prefs: TitleTrackPrefs = parse(value)?;
                self.catalog.lock().set_title_track_prefs(title, &prefs)?;
                changes.titles += 1;
            }
            return Ok(());
        }
        if let Some(title) = key.strip_prefix(MATCH) {
            if let Some(value) = value {
                let synced: SyncedMatch = parse(value)?;
                self.catalog.lock().set_metadata(&MetadataRecord {
                    title_key: title.to_owned(),
                    provider: synced.provider,
                    metadata: synced.metadata,
                    fetched_at: synced.fetched_at,
                    manual: true,
                })?;
                changes.titles += 1;
            }
            return Ok(());
        }
        if key == TMDB_KEY {
            match value.and_then(Value::as_str) {
                Some(secret) => self.store.secrets.set(&tmdb_secret(), secret)?,
                None => self.store.secrets.delete(&tmdb_secret())?,
            }
            changes.settings = true;
            return Ok(());
        }
        if let Some((_, file, local)) = SETTINGS.iter().find(|(name, ..)| *name == key) {
            let Some(Value::Object(incoming)) = value else {
                return Ok(());
            };
            let path = self.dirs().config.join(file);
            let before = read_json::<Value>(&path)?;
            let mut merged = match &before {
                Some(Value::Object(existing)) => existing.clone(),
                _ => serde_json::Map::new(),
            };
            for (field, field_value) in incoming {
                if !local.contains(&field.as_str()) {
                    merged.insert(field.clone(), field_value.clone());
                }
            }
            let merged = Value::Object(merged);
            if key == METADATA_SETTINGS {
                self.metadata_settings_changing(before.as_ref(), &merged)?;
            }
            write_json(&path, &merged)?;
            changes.settings = true;
            return Ok(());
        }
        // A register a newer build writes. Kept, so it travels on.
        Ok(())
    }

    fn apply_share(
        &self,
        id: &str,
        value: Option<&Value>,
        changes: &mut ProfileChanges,
    ) -> Result<()> {
        let existing = self.store.list()?.into_iter().find(|share| share.id == id);
        let Some(value) = value else {
            if existing.is_some() {
                self.store.remove(id)?;
                changes.shares_removed.push(id.to_owned());
            }
            return Ok(());
        };
        let synced: SyncedShare = parse(value)?;
        let expected = match (&synced.folder, &synced.url) {
            (Some(folder), _) => folder.share_id(),
            (None, Some(_)) => format!("share-{}", synced.token),
            (None, None) => return Err(invalid("a synced link has no URL")),
        };
        if expected != id {
            return Err(invalid("a synced share's id does not match what it opens"));
        }
        let secrets = synced.url.map(|url| ShareSecrets {
            url,
            custom_password: synced.custom_password,
        });
        let share = Share {
            id: id.to_owned(),
            name: synced.name,
            token: synced.token,
            has_custom_password: secrets
                .as_ref()
                .is_some_and(|secrets| secrets.custom_password.is_some()),
            folder: synced.folder,
        };
        if existing.as_ref() == Some(&share)
            && secrets.as_ref().is_none_or(|secrets| {
                self.store
                    .secrets_of(id)
                    .is_ok_and(|stored| &stored == secrets)
            })
        {
            return Ok(());
        }
        self.store.put_synced(share, secrets.as_ref())?;
        if existing.is_some() {
            changes.shares_changed.push(id.to_owned());
        } else {
            changes.shares_added.push(id.to_owned());
        }
        Ok(())
    }

    /// Turning enrichment off or switching provider drops every stored answer
    /// on the device that did it, which the settings page also does here.
    fn metadata_settings_changing(&self, before: Option<&Value>, after: &Value) -> Result<()> {
        let config = |value: Option<&Value>| -> MetadataConfig {
            value
                .and_then(|value| serde_json::from_value(value.clone()).ok())
                .unwrap_or_default()
        };
        let (before, after) = (config(before), config(Some(after)));
        if !after.enabled || before.provider != after.provider {
            self.catalog.lock().clear_metadata()?;
        }
        Ok(())
    }
}

/// Record what changed on this device since the last sync, stamped `now`.
fn stamp(registers: &mut Registers, observed: Observed, now: i64) {
    for (key, register) in registers.iter_mut() {
        if register.value.is_some()
            && key.starts_with(SHARE)
            && observed.shares_listed
            && !observed.values.contains_key(key)
            && !observed.unknown.contains(key)
        {
            *register = Register {
                value: None,
                updated_at: now,
            };
        }
    }
    for (key, value) in observed.values {
        let unchanged = registers
            .get(&key)
            .is_some_and(|register| same(&key, register.value.as_ref(), Some(&value)));
        if !unchanged {
            registers.insert(
                key,
                Register {
                    value: Some(value),
                    updated_at: now,
                },
            );
        }
    }
}

/// Whether two values of one register are the same choice.
///
/// A match is compared by what it points at: its stored details move every
/// time the provider is asked again, and a register re-stamped by every
/// refresh would bounce between devices for ever.
fn same(key: &str, a: Option<&Value>, b: Option<&Value>) -> bool {
    if key.starts_with(MATCH) {
        let pointer = |value: Option<&Value>| {
            value.map(|value| {
                (
                    value.get("provider").cloned(),
                    value
                        .get("metadata")
                        .and_then(|metadata| metadata.get("remote_id"))
                        .cloned(),
                )
            })
        };
        return pointer(a) == pointer(b);
    }
    a == b
}

/// The registers from other devices that are newer than this one's.
fn merge<'r>(local: &Registers, remote: impl IntoIterator<Item = &'r Registers>) -> Registers {
    let mut winners = Registers::new();
    for registers in remote {
        for (key, register) in registers {
            let current = winners.get(key).or_else(|| local.get(key));
            if current.is_none_or(|current| newer(register, current)) {
                winners.insert(key.clone(), register.clone());
            }
        }
    }
    winners
}

/// Newer wins; a tie between two different values goes the same way on every
/// device, or two of them would each keep their own.
fn newer(candidate: &Register, current: &Register) -> bool {
    if candidate.updated_at != current.updated_at {
        return candidate.updated_at > current.updated_at;
    }
    let order = |register: &Register| {
        register
            .value
            .as_ref()
            .map(Value::to_string)
            .unwrap_or_default()
    };
    order(candidate) > order(current)
}

/// `value` without the fields that stay on this device.
fn without(value: Value, local: &[&str]) -> Value {
    match value {
        Value::Object(mut fields) => {
            for field in local {
                fields.remove(*field);
            }
            Value::Object(fields)
        }
        other => other,
    }
}

fn parse<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T> {
    serde_json::from_value(value.clone()).map_err(|e| invalid(&e.to_string()))
}

fn invalid(reason: &str) -> crate::Error {
    crate::Error::Config(format!("synced setting: {reason}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use super::*;
    use crate::shares::SecretStore;

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

    /// One installation's configuration and catalog.
    pub(crate) struct Device {
        root: PathBuf,
        pub store: ShareStore,
        pub catalog: Mutex<Catalog>,
    }

    impl Device {
        pub(crate) fn new(tag: &str) -> Self {
            let mut bytes = [0_u8; 8];
            getrandom::fill(&mut bytes).unwrap();
            let unique: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            let root = std::env::temp_dir().join(format!("pstr-profile-{tag}-{unique}"));
            let dirs = AppDirs::from_paths(root.join("c"), root.join("d"), root.join("k")).unwrap();
            Self {
                root,
                store: ShareStore::with_secret_store(dirs, Arc::new(MemorySecrets::default())),
                catalog: Mutex::new(Catalog::in_memory().unwrap()),
            }
        }

        pub(crate) fn lock(&self) -> parking_lot::MutexGuard<'_, Catalog> {
            self.catalog.lock()
        }

        pub(crate) fn profile(&self) -> Profile<'_> {
            Profile {
                store: &self.store,
                catalog: &self.catalog,
            }
        }

        fn sync(&self, remote: &[&Registers], now: i64) -> (Registers, ProfileChanges) {
            self.profile().sync(remote.iter().copied(), now).unwrap()
        }

        fn share_names(&self) -> Vec<String> {
            self.store
                .list()
                .unwrap()
                .into_iter()
                .map(|share| share.name)
                .collect()
        }
    }

    impl Drop for Device {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    const LINK: &str = "https://drive.proton.me/urls/ABCDEFGHIJ#secretpart";

    #[test]
    fn a_share_added_on_one_device_arrives_on_another_with_its_secrets() {
        let laptop = Device::new("laptop");
        let phone = Device::new("phone");
        laptop.sync(&[], 10);
        laptop.store.add("Anime", LINK, Some("hunter2")).unwrap();
        let (sent, _) = laptop.sync(&[], 20);

        let (_, changes) = phone.sync(&[&sent], 30);

        assert_eq!(changes.shares_added, ["share-ABCDEFGHIJ"]);
        assert_eq!(phone.share_names(), ["Anime"]);
        let secrets = phone.store.secrets_of("share-ABCDEFGHIJ").unwrap();
        assert_eq!(secrets.url, LINK);
        assert_eq!(secrets.custom_password.as_deref(), Some("hunter2"));
    }

    #[test]
    fn a_share_removed_on_one_device_is_removed_on_the_others() {
        let laptop = Device::new("laptop");
        let phone = Device::new("phone");
        laptop.store.add("Anime", LINK, None).unwrap();
        let (sent, _) = laptop.sync(&[], 10);
        let (phone_sent, _) = phone.sync(&[&sent], 20);

        laptop.store.remove("share-ABCDEFGHIJ").unwrap();
        let (sent, _) = laptop.sync(&[&phone_sent], 30);
        let (_, changes) = phone.sync(&[&sent], 40);

        assert_eq!(changes.shares_removed, ["share-ABCDEFGHIJ"]);
        assert!(phone.share_names().is_empty());
    }

    #[test]
    fn a_fresh_installation_takes_the_library_rather_than_its_defaults() {
        let laptop = Device::new("laptop");
        let phone = Device::new("phone");
        laptop.sync(&[], 10);
        let mut prefs = crate::prefs::load(&laptop.store.dirs).unwrap();
        prefs.autoplay_next = false;
        prefs.speed = 1.5;
        crate::prefs::save(&laptop.store.dirs, &prefs).unwrap();
        let (sent, _) = laptop.sync(&[], 20);

        // The new phone has written its defaults before it ever synced.
        crate::prefs::save(&phone.store.dirs, &crate::prefs::PlaybackPrefs::default()).unwrap();
        let (_, changes) = phone.sync(&[&sent], 30);

        assert!(changes.settings);
        let taken = crate::prefs::load(&phone.store.dirs).unwrap();
        assert!(!taken.autoplay_next);
        assert_eq!(taken.speed, 1.5);
    }

    #[test]
    fn volume_and_mute_stay_on_the_device_that_set_them() {
        let laptop = Device::new("laptop");
        let phone = Device::new("phone");
        laptop.sync(&[], 10);
        let mut prefs = crate::prefs::PlaybackPrefs {
            volume: 20.0,
            muted: true,
            ..Default::default()
        };
        crate::prefs::save(&laptop.store.dirs, &prefs).unwrap();
        let (sent, _) = laptop.sync(&[], 20);
        prefs.volume = 80.0;
        prefs.muted = false;
        prefs.subtitles = false;
        crate::prefs::save(&phone.store.dirs, &prefs).unwrap();
        phone.sync(&[], 5);

        phone.sync(&[&sent], 30);

        let taken = crate::prefs::load(&phone.store.dirs).unwrap();
        assert_eq!(taken.volume, 80.0);
        assert!(!taken.muted);
        assert!(taken.subtitles, "the laptop's later choice wins");
    }

    #[test]
    fn the_newer_change_wins_whichever_device_syncs_first() {
        let laptop = Device::new("laptop");
        let phone = Device::new("phone");
        laptop.sync(&[], 1);
        phone.sync(&[], 1);
        let mut appearance = crate::appearance::load(&laptop.store.dirs).unwrap();
        appearance.gradients = false;
        crate::appearance::save(&laptop.store.dirs, &appearance).unwrap();
        let (from_laptop, _) = laptop.sync(&[], 10);
        appearance.gradients = true;
        appearance.accent = crate::appearance::Accent::Peach;
        crate::appearance::save(&phone.store.dirs, &appearance).unwrap();
        let (from_phone, _) = phone.sync(&[&from_laptop], 20);
        laptop.sync(&[&from_phone], 30);

        for device in [&laptop, &phone] {
            let seen = crate::appearance::load(&device.store.dirs).unwrap();
            assert_eq!(seen.accent, crate::appearance::Accent::Peach);
            assert!(seen.gradients);
        }
    }

    #[test]
    fn an_unchanged_device_sends_the_same_registers_again() {
        let laptop = Device::new("laptop");
        laptop.store.add("Anime", LINK, None).unwrap();
        let (first, _) = laptop.sync(&[], 10);
        let (second, changes) = laptop.sync(&[&first], 20);
        assert_eq!(first, second);
        assert!(changes.is_empty());
    }

    #[test]
    fn a_share_whose_secret_cannot_be_read_is_not_removed_elsewhere() {
        let laptop = Device::new("laptop");
        laptop.store.add("Anime", LINK, None).unwrap();
        laptop.sync(&[], 10);
        laptop.store.secrets.delete("share-ABCDEFGHIJ").unwrap();
        let (sent, _) = laptop.sync(&[], 20);
        assert!(sent["share/share-ABCDEFGHIJ"].value.is_some());
    }

    #[test]
    fn a_hand_picked_match_and_a_shows_track_choice_travel() {
        let laptop = Device::new("laptop");
        let phone = Device::new("phone");
        laptop.sync(&[], 1);
        let metadata = TitleMetadata {
            provider: ProviderId::AniList,
            remote_id: "21".to_owned(),
            name: "One Piece".to_owned(),
            original_name: None,
            overview: None,
            year: Some(1999),
            kind: crate::library::TitleKind::Series,
            poster_url: None,
            backdrop_url: None,
            rating: None,
            genres: Vec::new(),
            episodes: None,
            url: None,
            details: None,
        };
        laptop
            .catalog
            .lock()
            .set_metadata(&MetadataRecord {
                title_key: "one piece".to_owned(),
                provider: ProviderId::AniList,
                metadata: Some(metadata),
                fetched_at: 5,
                manual: true,
            })
            .unwrap();
        let tracks = TitleTrackPrefs {
            audio_language: Some("jpn".to_owned()),
            ..Default::default()
        };
        laptop
            .catalog
            .lock()
            .set_title_track_prefs("one piece", &tracks)
            .unwrap();
        let (sent, _) = laptop.sync(&[], 10);

        let (_, changes) = phone.sync(&[&sent], 20);

        assert_eq!(changes.titles, 2);
        let catalog = phone.catalog.lock();
        let record = catalog.metadata("one piece").unwrap().unwrap();
        assert!(record.manual);
        assert_eq!(record.metadata.unwrap().remote_id, "21");
        assert_eq!(
            catalog.title_track_prefs("one piece").unwrap(),
            Some(tracks)
        );
    }

    #[test]
    fn a_synced_share_that_names_another_id_is_refused() {
        let phone = Device::new("phone");
        let mut forged = Registers::new();
        forged.insert(
            "share/share-OTHER".to_owned(),
            Register {
                value: Some(serde_json::json!({
                    "name": "x", "token": "ABCDEFGHIJ", "url": LINK
                })),
                updated_at: 10,
            },
        );
        let (registers, changes) = phone.sync(&[&forged], 20);
        assert!(changes.shares_added.is_empty());
        assert!(!registers.contains_key("share/share-OTHER"));
    }
}
