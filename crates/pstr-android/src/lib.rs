//! Kotlin-facing Android host for the portable proton-stream crates.
//!
//! This boundary deliberately exposes screen-shaped immutable records rather
//! than the catalog's internal Rust types. Kotlin owns lifecycle and drawing;
//! Rust remains the sole owner of Proton sessions, SQLite and decrypted bytes.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use pstr_core::catalog::{Catalog, OfflineFile, TitleTrackPrefs, WatchState, build_rows};
use pstr_core::config::AppDirs;
use pstr_core::library::{Episode, Library, Title, TitleKind};
use pstr_core::metadata::{MetadataConfig, MetadataRecord, ProviderId, TitleMetadata};
use pstr_core::proton_sdk::ids::{LinkId, VolumeId};
use pstr_core::{SecretStore, ShareStore, SharedLibrary};
use pstr_stream::{
    BlockSource, DiskCacheConfig, FileBlocks, LibraryOpener, NodeUid, StreamConfig, StreamSource,
    VideoStream,
};
use serde::{Deserialize, Serialize};

/// Supplies rustls-platform-verifier with the Android application context
/// before the SDK creates its first HTTPS client.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_narl_protonstream_native_NativeRuntime_initTls(
    mut env: jni::JNIEnv<'_>,
    _class: jni::objects::JObject<'_>,
    context: jni::objects::JObject<'_>,
) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Trace)
            .with_tag("protonstream-rust"),
    );
    let _ = rustls_platform_verifier::android::init_with_env(&mut env, context);
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum BridgeError {
    #[error("{reason}")]
    Failure { reason: String },
}

impl BridgeError {
    fn from_display(error: impl std::fmt::Display) -> Self {
        Self::Failure {
            reason: error.to_string(),
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct AndroidPaths {
    pub config: String,
    pub data: String,
    pub cache: String,
}

/// Android implements this with an AES-GCM key held by Android Keystore.
#[uniffi::export(callback_interface)]
pub trait AndroidSecretStore: Send + Sync {
    fn set(&self, key: String, value: String) -> Result<(), BridgeError>;
    fn get(&self, key: String) -> Result<Option<String>, BridgeError>;
    fn delete(&self, key: String) -> Result<(), BridgeError>;
}

struct SecretAdapter(Box<dyn AndroidSecretStore>);

impl SecretStore for SecretAdapter {
    fn set(&self, key: &str, value: &str) -> pstr_core::Result<()> {
        self.0
            .set(key.to_owned(), value.to_owned())
            .map_err(|error| pstr_core::Error::Config(error.to_string()))
    }

    fn get(&self, key: &str) -> pstr_core::Result<Option<String>> {
        self.0
            .get(key.to_owned())
            .map_err(|error| pstr_core::Error::Config(error.to_string()))
    }

    fn delete(&self, key: &str) -> pstr_core::Result<()> {
        self.0
            .delete(key.to_owned())
            .map_err(|error| pstr_core::Error::Config(error.to_string()))
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ShareRecord {
    pub id: String,
    pub name: String,
    pub has_custom_password: bool,
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum TitleType {
    Series,
    Film,
}

#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum MetadataProvider {
    AniList,
    Tmdb,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MetadataSettingsRecord {
    pub enabled: bool,
    pub provider: MetadataProvider,
    pub language: String,
    pub ready: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MatchRecord {
    pub provider: MetadataProvider,
    pub remote_id: String,
    pub name: String,
    pub original_name: Option<String>,
    pub overview: Option<String>,
    pub year: Option<u32>,
    pub kind: TitleType,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub rating: Option<f64>,
    pub genres: Vec<String>,
    pub episode_count: Option<u32>,
    pub external_url: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct EpisodeRecord {
    pub share_id: String,
    pub volume_id: String,
    pub link_id: String,
    pub name: String,
    pub label: String,
    pub detail: String,
    pub season: Option<u32>,
    pub number: Option<u32>,
    pub size: Option<u64>,
    pub progress: Option<f64>,
    pub resume_at: Option<f64>,
    pub watched: bool,
    pub offline: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SeasonRecord {
    pub number: Option<u32>,
    pub label: String,
    pub episodes: Vec<EpisodeRecord>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct TitleRecord {
    pub key: String,
    pub name: String,
    pub year: Option<u32>,
    pub kind: TitleType,
    pub watched_count: u64,
    pub episode_count: u64,
    pub canonical_name: Option<String>,
    pub original_name: Option<String>,
    pub overview: Option<String>,
    pub metadata_provider: Option<MetadataProvider>,
    pub metadata_id: Option<String>,
    pub metadata_year: Option<u32>,
    pub metadata_kind: Option<TitleType>,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub rating: Option<f64>,
    pub genres: Vec<String>,
    pub provider_episode_count: Option<u32>,
    pub external_url: Option<String>,
    pub manual_match: bool,
    pub seasons: Vec<SeasonRecord>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct WatchStateRecord {
    pub position_secs: f64,
    pub duration_secs: Option<f64>,
    pub watched: bool,
    pub updated_at: i64,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct TrackPreferencesRecord {
    pub audio_language: Option<String>,
    pub subtitle_language: Option<String>,
    pub subtitles: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct OfflineRecord {
    pub share_id: String,
    pub link_id: String,
    pub revision_id: String,
    pub size: u64,
    /// Catalog metadata for display/grouping. `None` only when a retained row
    /// no longer has a catalog node; `library()` prunes that state on refresh.
    pub episode: Option<EpisodeRecord>,
}

/// Implemented by a WorkManager worker. Cancellation is polled only between
/// complete Proton blocks, so a retained `.part` file is always resumable.
#[uniffi::export(callback_interface)]
pub trait DownloadObserver: Send + Sync {
    fn on_progress(&self, downloaded: u64, total: u64);
    fn is_cancelled(&self) -> bool;
}

/// A seekable revision for libmpv's Android stream callback.
#[derive(uniffi::Object)]
pub struct AndroidStream {
    runtime: Arc<tokio::runtime::Runtime>,
    stream: VideoStream,
}

#[uniffi::export]
impl AndroidStream {
    pub fn size(&self) -> u64 {
        self.stream.size()
    }

    pub fn revision_id(&self) -> String {
        self.stream.revision_id().to_owned()
    }

    /// Publish this stream to the in-process libmpv adapter. The returned
    /// token contains no secret and is meaningful only in this process.
    pub fn native_handle(self: Arc<Self>) -> u64 {
        let id = NEXT_NATIVE_STREAM.fetch_add(1, Ordering::Relaxed);
        native_streams().lock().insert(id, self);
        id
    }
}

impl AndroidStream {
    /// Read decrypted bytes strictly inside the native boundary. This is not a
    /// UniFFI export: only `pstr_android_stream_read` may move these bytes, and
    /// it copies them directly into libmpv-owned memory.
    fn read_range_for_native(&self, offset: u64, length: u64) -> pstr_stream::Result<Vec<u8>> {
        self.runtime
            .block_on(self.stream.read_range(offset, length))
    }
}

static NEXT_NATIVE_STREAM: AtomicU64 = AtomicU64::new(1);
static NATIVE_STREAMS: OnceLock<Mutex<HashMap<u64, Arc<AndroidStream>>>> = OnceLock::new();

fn native_streams() -> &'static Mutex<HashMap<u64, Arc<AndroidStream>>> {
    NATIVE_STREAMS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Blocking C ABI used only by libmpv's demuxer thread. Kotlin never receives
/// plaintext media bytes, and the handle cannot be resolved outside this
/// process.
///
/// # Safety
///
/// `buffer` must point to at least `length` bytes of writable memory for the
/// duration of this call. The caller must keep the published stream token alive
/// until this call returns; releasing the token concurrently is supported, but
/// may make this read fail with `-1` if release wins the lookup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pstr_android_stream_read(
    handle: u64,
    offset: u64,
    buffer: *mut c_void,
    length: usize,
) -> i64 {
    if buffer.is_null() || length == 0 {
        return if length == 0 { 0 } else { -1 };
    }
    let Some(stream) = native_streams().lock().get(&handle).cloned() else {
        return -1;
    };
    let Ok(length) = u64::try_from(length) else {
        return -1;
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        stream.read_range_for_native(offset, length)
    }));
    let Ok(Ok(bytes)) = result else { return -1 };
    // SAFETY: libmpv supplied `length` writable bytes for the duration of this
    // call, and read_range cannot return more than requested.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.cast(), bytes.len()) };
    i64::try_from(bytes.len()).unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub extern "C" fn pstr_android_stream_size(handle: u64) -> i64 {
    std::panic::catch_unwind(|| {
        native_streams()
            .lock()
            .get(&handle)
            .and_then(|stream| i64::try_from(stream.stream.size()).ok())
            .unwrap_or(-1)
    })
    .unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub extern "C" fn pstr_android_stream_release(handle: u64) {
    let _ = std::panic::catch_unwind(|| {
        // Destructors may transitively release the runtime. Never run them
        // while the global registry mutex is held.
        let stream = native_streams().lock().remove(&handle);
        drop(stream);
    });
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PartialMarker {
    revision_id: String,
    block_sizes: Vec<u64>,
}

struct Connection {
    generation: u64,
    library: Arc<SharedLibrary>,
    open_failures: Vec<String>,
    #[allow(dead_code)]
    source: StreamSource,
}

#[derive(uniffi::Object)]
pub struct AndroidEngine {
    runtime: Arc<tokio::runtime::Runtime>,
    dirs: AppDirs,
    store: Arc<ShareStore>,
    secrets: Arc<dyn SecretStore>,
    catalog: Mutex<Catalog>,
    connection: tokio::sync::Mutex<Option<Connection>>,
    /// Serializes offline publication with share removal cleanup.
    share_publication: Mutex<()>,
    /// Advances after every successful share-store mutation. A connection is
    /// reusable only while it describes this exact generation of the store.
    share_generation: AtomicU64,
}

#[uniffi::export]
impl AndroidEngine {
    #[uniffi::constructor]
    pub fn new(
        paths: AndroidPaths,
        secrets: Box<dyn AndroidSecretStore>,
    ) -> Result<Arc<Self>, BridgeError> {
        let dirs = AppDirs::from_paths(paths.config, paths.data, paths.cache)
            .map_err(BridgeError::from_display)?;
        let secret_store: Arc<dyn SecretStore> = Arc::new(SecretAdapter(secrets));
        let store = Arc::new(ShareStore::with_secret_store(
            dirs.clone(),
            Arc::clone(&secret_store),
        ));
        let catalog = Catalog::open(&dirs.catalog_db()).map_err(BridgeError::from_display)?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map(Arc::new)
            .map_err(BridgeError::from_display)?;

        Ok(Arc::new(Self {
            runtime,
            dirs,
            store,
            secrets: secret_store,
            catalog: Mutex::new(catalog),
            connection: tokio::sync::Mutex::new(None),
            share_publication: Mutex::new(()),
            share_generation: AtomicU64::new(0),
        }))
    }

    pub fn shares(&self) -> Result<Vec<ShareRecord>, BridgeError> {
        self.store
            .list()
            .map_err(BridgeError::from_display)
            .map(|shares| {
                shares
                    .into_iter()
                    .map(|share| ShareRecord {
                        id: share.id,
                        name: share.name,
                        has_custom_password: share.has_custom_password,
                    })
                    .collect()
            })
    }

    pub fn add_share(
        &self,
        name: String,
        url: String,
        custom_password: Option<String>,
    ) -> Result<ShareRecord, BridgeError> {
        let share = self
            .store
            .add(&name, &url, custom_password.as_deref())
            .map_err(BridgeError::from_display)?;
        self.invalidate_connection();
        Ok(ShareRecord {
            id: share.id,
            name: share.name,
            has_custom_password: share.has_custom_password,
        })
    }

    pub fn remove_share(&self, share_id: String) -> Result<(), BridgeError> {
        let _publication = self.share_publication.lock();
        let catalog = self.catalog.lock();
        let offline = catalog
            .all_offline_files()
            .map_err(BridgeError::from_display)?;
        let links: Vec<String> = catalog
            .files(&share_id)
            .map_err(BridgeError::from_display)?
            .into_iter()
            .map(|node| node.link_id)
            .collect();
        drop(catalog);

        let store_result = self.store.remove(&share_id);
        let share_still_present = self
            .store
            .list()
            .map_err(BridgeError::from_display)?
            .iter()
            .any(|share| share.id == share_id);
        if share_still_present {
            return store_result.map_err(BridgeError::from_display);
        }
        // ShareStore removes the config row before deleting its secret. Even
        // when secret cleanup fails, old authenticated clients and catalog
        // rows must not remain usable.
        self.invalidate_connection();

        for ((stored_share_id, link_id), file) in offline {
            if stored_share_id == share_id {
                let path = self
                    .dirs
                    .offline_file(&share_id, &link_id, &file.revision_id);
                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(BridgeError::from_display(error)),
                }
            }
        }
        for link_id in links {
            let (partial, marker) = self.partial_paths(&share_id, &link_id);
            let _ = std::fs::remove_file(partial);
            let _ = std::fs::remove_file(marker);
        }
        self.catalog
            .lock()
            .remove_share(&share_id)
            .map_err(BridgeError::from_display)?;
        store_result.map_err(BridgeError::from_display)
    }

    pub fn library(&self, search: Option<String>) -> Result<Vec<TitleRecord>, BridgeError> {
        let catalog = self.catalog.lock();
        let files = catalog.all_files().map_err(BridgeError::from_display)?;
        let watch = catalog
            .all_watch_states()
            .map_err(BridgeError::from_display)?;
        let metadata = catalog.all_metadata().map_err(BridgeError::from_display)?;
        let mut offline = catalog
            .all_offline_files()
            .map_err(BridgeError::from_display)?;
        let mut invalid = Vec::new();
        offline.retain(|(share_id, link_id), file| {
            let path = self.dirs.offline_file(share_id, link_id, &file.revision_id);
            let valid = std::fs::metadata(path)
                .is_ok_and(|metadata| metadata.len() == file.block_sizes.iter().sum::<u64>());
            if !valid {
                invalid.push((share_id.clone(), link_id.clone()));
            }
            valid
        });
        for (share_id, link_id) in invalid {
            catalog
                .remove_offline_file(&share_id, &link_id)
                .map_err(BridgeError::from_display)?;
        }
        let library = Library::build(files, &watch);
        let titles: Vec<&Title> = match search {
            Some(query) => library.search(&query),
            None => library.titles.iter().collect(),
        };
        Ok(titles
            .into_iter()
            .map(|title| title_record(title, &offline, metadata.get(&title.key)))
            .collect())
    }

    pub fn metadata_settings(&self) -> Result<MetadataSettingsRecord, BridgeError> {
        let config = pstr_meta::settings::load(&self.dirs).map_err(BridgeError::from_display)?;
        let ready = !config.provider.needs_api_key()
            || pstr_meta::settings::api_key_in(self.secrets.as_ref(), config.provider).is_some();
        Ok(MetadataSettingsRecord {
            enabled: config.enabled,
            provider: metadata_provider(config.provider),
            language: config.language,
            ready,
        })
    }

    /// Persist the privacy opt-in and provider choice. Disabling enrichment or
    /// changing provider also removes the stored third-party answers, matching
    /// the desktop client's semantics.
    pub fn set_metadata_settings(
        &self,
        settings: MetadataSettingsRecord,
    ) -> Result<(), BridgeError> {
        let previous = pstr_meta::settings::load(&self.dirs).map_err(BridgeError::from_display)?;
        let config = MetadataConfig {
            enabled: settings.enabled,
            provider: provider_id(settings.provider),
            language: settings.language.trim().to_owned(),
        };
        pstr_meta::settings::save(&self.dirs, &config).map_err(BridgeError::from_display)?;
        if !config.enabled || config.provider != previous.provider {
            self.catalog
                .lock()
                .clear_metadata()
                .map_err(BridgeError::from_display)?;
        }
        Ok(())
    }

    /// Store a provider credential through Android Keystore, never config or
    /// SharedPreferences. An empty key forgets it.
    pub fn set_metadata_api_key(
        &self,
        provider: MetadataProvider,
        key: String,
    ) -> Result<(), BridgeError> {
        pstr_meta::settings::set_api_key_in(self.secrets.as_ref(), provider_id(provider), &key)
            .map_err(BridgeError::from_display)
    }

    pub async fn match_titles(self: Arc<Self>, force: bool) -> Result<(), BridgeError> {
        let runtime = Arc::clone(&self.runtime);
        runtime
            .spawn(async move {
                let service = self.metadata_service()?;
                let provider = service.provider();
                let (titles, stored) = self.titles_and_metadata()?;
                for title in titles {
                    let existing = stored.get(&title.key);
                    let pinned =
                        existing.is_some_and(|record| record.manual && record.provider == provider);
                    if pinned || (!force && pstr_meta::service::is_usable(existing, provider)) {
                        continue;
                    }
                    let record = service
                        .record(&title)
                        .await
                        .map_err(BridgeError::from_display)?;
                    self.catalog
                        .lock()
                        .set_metadata(&record)
                        .map_err(BridgeError::from_display)?;
                }
                Ok(())
            })
            .await
            .map_err(BridgeError::from_display)?
    }

    pub async fn search_matches(
        self: Arc<Self>,
        title_key: String,
        term: String,
    ) -> Result<Vec<MatchRecord>, BridgeError> {
        let runtime = Arc::clone(&self.runtime);
        runtime
            .spawn(async move {
                let service = self.metadata_service()?;
                let title = self.title(&title_key)?;
                service
                    .search(&term, title.kind)
                    .await
                    .map_err(BridgeError::from_display)
                    .map(|matches| matches.into_iter().map(match_record).collect())
            })
            .await
            .map_err(BridgeError::from_display)?
    }

    pub fn choose_match(&self, title_key: String, found: MatchRecord) -> Result<(), BridgeError> {
        let service = self.metadata_service()?;
        let metadata = title_metadata(found);
        if metadata.provider != service.provider() {
            return Err(BridgeError::Failure {
                reason: "match came from a different metadata provider".to_owned(),
            });
        }
        self.title(&title_key)?;
        self.catalog
            .lock()
            .set_metadata(&service.chosen(title_key, metadata))
            .map_err(BridgeError::from_display)
    }

    pub fn forget_match(&self, title_key: String) -> Result<(), BridgeError> {
        self.catalog
            .lock()
            .forget_metadata(&title_key)
            .map_err(BridgeError::from_display)
    }

    pub fn watch_state(
        &self,
        share_id: String,
        link_id: String,
    ) -> Result<Option<WatchStateRecord>, BridgeError> {
        self.catalog
            .lock()
            .watch_state(&share_id, &link_id)
            .map_err(BridgeError::from_display)
            .map(|state| state.map(watch_state_record))
    }

    pub fn save_watch_state(
        &self,
        share_id: String,
        link_id: String,
        position_secs: f64,
        duration_secs: Option<f64>,
        watched: bool,
    ) -> Result<(), BridgeError> {
        if !position_secs.is_finite()
            || position_secs < 0.0
            || duration_secs.is_some_and(|duration| !duration.is_finite() || duration < 0.0)
            || duration_secs.is_some_and(|duration| position_secs > duration)
        {
            return Err(BridgeError::Failure {
                reason: "watch times must be finite, non-negative, and position must not exceed duration"
                    .to_owned(),
            });
        }
        self.catalog
            .lock()
            .set_watch_state(
                &share_id,
                &link_id,
                &WatchState {
                    position_secs,
                    duration_secs,
                    watched,
                    updated_at: now(),
                },
            )
            .map_err(BridgeError::from_display)
    }

    pub fn title_track_preferences(
        &self,
        title_key: String,
    ) -> Result<TrackPreferencesRecord, BridgeError> {
        self.catalog
            .lock()
            .title_track_prefs(&title_key)
            .map_err(BridgeError::from_display)
            .map(|prefs| track_preferences_record(prefs.unwrap_or_default()))
    }

    pub fn set_title_track_preferences(
        &self,
        title_key: String,
        preferences: TrackPreferencesRecord,
    ) -> Result<(), BridgeError> {
        let preferences = TitleTrackPrefs {
            audio_language: normalized_language(preferences.audio_language),
            subtitle_language: normalized_language(preferences.subtitle_language),
            subtitles: preferences.subtitles,
        };
        self.catalog
            .lock()
            .set_title_track_prefs(&title_key, &preferences)
            .map_err(BridgeError::from_display)
    }

    pub fn offline_files(&self) -> Result<Vec<OfflineRecord>, BridgeError> {
        let catalog = self.catalog.lock();
        let files = catalog
            .all_offline_files()
            .map_err(BridgeError::from_display)?;
        let nodes = catalog.all_files().map_err(BridgeError::from_display)?;
        let watch = catalog
            .all_watch_states()
            .map_err(BridgeError::from_display)?;
        let library = Library::build(nodes, &watch);
        let mut episodes = std::collections::HashMap::new();
        for title in &library.titles {
            for episode in title.episodes() {
                episodes.insert(
                    (episode.node.share_id.clone(), episode.node.link_id.clone()),
                    episode_record(episode, &files),
                );
            }
        }
        let mut records: Vec<_> = files
            .into_iter()
            .map(|((share_id, link_id), file)| OfflineRecord {
                episode: episodes.remove(&(share_id.clone(), link_id.clone())),
                share_id,
                link_id,
                revision_id: file.revision_id,
                size: file.block_sizes.iter().sum(),
            })
            .collect();
        records.sort_by(|left, right| {
            (&left.share_id, &left.link_id).cmp(&(&right.share_id, &right.link_id))
        });
        Ok(records)
    }

    pub async fn open_stream(
        self: Arc<Self>,
        share_id: String,
        volume_id: String,
        link_id: String,
    ) -> Result<Arc<AndroidStream>, BridgeError> {
        let runtime = Arc::clone(&self.runtime);
        runtime
            .spawn(async move {
                let stream = self
                    .open_stream_inner(&share_id, &volume_id, &link_id)
                    .await?;
                Ok(Arc::new(AndroidStream {
                    runtime: Arc::clone(&self.runtime),
                    stream,
                }))
            })
            .await
            .map_err(BridgeError::from_display)?
    }

    pub async fn download_episode(
        self: Arc<Self>,
        share_id: String,
        volume_id: String,
        link_id: String,
        observer: Box<dyn DownloadObserver>,
    ) -> Result<OfflineRecord, BridgeError> {
        let runtime = Arc::clone(&self.runtime);
        runtime
            .spawn(async move {
                self.download_episode_inner(&share_id, &volume_id, &link_id, observer)
                    .await
            })
            .await
            .map_err(BridgeError::from_display)?
    }

    pub async fn remove_offline_episode(
        self: Arc<Self>,
        share_id: String,
        link_id: String,
    ) -> Result<(), BridgeError> {
        let runtime = Arc::clone(&self.runtime);
        runtime
            .spawn(async move { self.remove_offline_episode_inner(&share_id, &link_id).await })
            .await
            .map_err(BridgeError::from_display)?
    }

    pub async fn release_stream(
        self: Arc<Self>,
        share_id: String,
        volume_id: String,
        link_id: String,
    ) {
        let runtime = Arc::clone(&self.runtime);
        let _ = runtime
            .spawn(async move {
                let generation = self.share_generation.load(Ordering::Acquire);
                if let Some(connection) = self
                    .connection
                    .lock()
                    .await
                    .as_ref()
                    .filter(|connection| connection.generation == generation)
                {
                    connection
                        .source
                        .close(&share_id, &node_uid(&volume_id, &link_id));
                }
            })
            .await;
    }

    pub async fn crawl(self: Arc<Self>, share_id: Option<String>) -> Result<(), BridgeError> {
        let runtime = Arc::clone(&self.runtime);
        runtime
            .spawn(async move { self.crawl_inner(share_id).await })
            .await
            .map_err(BridgeError::from_display)?
    }
}

impl AndroidEngine {
    fn metadata_service(&self) -> Result<pstr_meta::MetadataService, BridgeError> {
        let config = pstr_meta::settings::load(&self.dirs).map_err(BridgeError::from_display)?;
        if !config.enabled {
            return Err(BridgeError::Failure {
                reason: "turn on metadata enrichment first".to_owned(),
            });
        }
        let key = pstr_meta::settings::api_key_in(self.secrets.as_ref(), config.provider);
        pstr_meta::MetadataService::new(&config, key).map_err(BridgeError::from_display)
    }

    fn titles_and_metadata(
        &self,
    ) -> Result<(Vec<Title>, HashMap<String, MetadataRecord>), BridgeError> {
        let catalog = self.catalog.lock();
        let files = catalog.all_files().map_err(BridgeError::from_display)?;
        let watch = catalog
            .all_watch_states()
            .map_err(BridgeError::from_display)?;
        let metadata = catalog.all_metadata().map_err(BridgeError::from_display)?;
        Ok((Library::build(files, &watch).titles, metadata))
    }

    fn title(&self, key: &str) -> Result<Title, BridgeError> {
        self.titles_and_metadata()?
            .0
            .into_iter()
            .find(|title| title.key == key)
            .ok_or_else(|| BridgeError::Failure {
                reason: format!("title {key:?} is no longer in the library"),
            })
    }

    fn invalidate_connection(&self) {
        self.share_generation.fetch_add(1, Ordering::AcqRel);
    }

    fn partial_paths(
        &self,
        share_id: &str,
        link_id: &str,
    ) -> (std::path::PathBuf, std::path::PathBuf) {
        let partial = self
            .dirs
            .offline_file(share_id, link_id, "partial")
            .with_extension("part");
        let marker = partial.with_extension("revision");
        (partial, marker)
    }

    async fn connection(&self) -> Result<Connection, BridgeError> {
        let mut connection = self.connection.lock().await;
        loop {
            let generation = self.share_generation.load(Ordering::Acquire);
            if let Some(opened) = connection
                .as_ref()
                .filter(|opened| opened.generation == generation)
            {
                return Ok(Connection {
                    generation,
                    library: Arc::clone(&opened.library),
                    open_failures: opened.open_failures.clone(),
                    source: opened.source.clone(),
                });
            }

            let (library, failures) = SharedLibrary::open_all(&self.store)
                .await
                .map_err(BridgeError::from_display)?;
            let open_failures: Vec<String> = failures
                .into_iter()
                .map(|(share, error)| format!("{}: {error}", share.name))
                .collect();
            let library = Arc::new(library);
            let opener = Arc::new(LibraryOpener::new(Arc::clone(&library)));
            let source = StreamSource::new(
                opener,
                StreamConfig::default()
                    .with_disk_cache(DiskCacheConfig::new(self.dirs.block_cache())),
            )
            .await
            .map_err(BridgeError::from_display)?;

            // A concurrent add/remove while public links were opening makes
            // this result stale before it is even cached. Reopen from the new
            // store generation while retaining the single-flight lock.
            if self.share_generation.load(Ordering::Acquire) != generation {
                continue;
            }
            *connection = Some(Connection {
                generation,
                library: Arc::clone(&library),
                open_failures: open_failures.clone(),
                source: source.clone(),
            });
            return Ok(Connection {
                generation,
                library,
                open_failures,
                source,
            });
        }
    }

    async fn crawl_inner(&self, share_id: Option<String>) -> Result<(), BridgeError> {
        let connection = self.connection().await?;
        let targets: Vec<String> = match share_id {
            Some(id) => vec![id],
            None => connection.library.share_ids().map(str::to_owned).collect(),
        };
        for id in targets {
            connection
                .library
                .refresh_session(&id)
                .await
                .map_err(|error| BridgeError::Failure {
                    reason: format!("refresh session for {id}: {error}"),
                })?;
            let before = self
                .catalog
                .lock()
                .all_offline_files()
                .map_err(BridgeError::from_display)?;
            let nodes = connection
                .library
                .crawl(&id)
                .await
                .map_err(BridgeError::from_display)?;
            let rows = build_rows(&id, &nodes);
            self.catalog
                .lock()
                .replace_share(&id, &rows)
                .map_err(BridgeError::from_display)?;
            let after = self
                .catalog
                .lock()
                .all_offline_files()
                .map_err(BridgeError::from_display)?;
            for ((stored_share_id, link_id), file) in before {
                if stored_share_id != id
                    || after.get(&(stored_share_id.clone(), link_id.clone())) == Some(&file)
                {
                    continue;
                }
                remove_file_if_present(self.dirs.offline_file(
                    &stored_share_id,
                    &link_id,
                    &file.revision_id,
                ))
                .await?;
                let (partial, marker) = self.partial_paths(&stored_share_id, &link_id);
                remove_file_if_present(partial).await?;
                remove_file_if_present(marker).await?;
            }
        }
        if !connection.open_failures.is_empty() {
            return Err(BridgeError::Failure {
                reason: format!(
                    "could not open configured share(s): {}",
                    connection.open_failures.join("; ")
                ),
            });
        }
        Ok(())
    }

    async fn open_stream_inner(
        &self,
        share_id: &str,
        volume_id: &str,
        link_id: &str,
    ) -> Result<VideoStream, BridgeError> {
        let offline = self
            .catalog
            .lock()
            .offline_file(share_id, link_id)
            .map_err(BridgeError::from_display)?;
        if let Some(file) = offline {
            let path = self.dirs.offline_file(share_id, link_id, &file.revision_id);
            let expected: u64 = file.block_sizes.iter().sum();
            if tokio::fs::metadata(&path)
                .await
                .is_ok_and(|metadata| metadata.len() == expected)
            {
                let blocks: Arc<dyn BlockSource> =
                    Arc::new(FileBlocks::new(file.revision_id, path, file.block_sizes));
                return Ok(VideoStream::offline(
                    node_uid(volume_id, link_id),
                    blocks,
                    pstr_stream::DEFAULT_RING_BYTES,
                ));
            }
        }

        let connection = self.connection().await?;
        connection
            .source
            .open(share_id, &node_uid(volume_id, link_id))
            .await
            .map_err(BridgeError::from_display)
    }

    async fn download_episode_inner(
        &self,
        share_id: &str,
        volume_id: &str,
        link_id: &str,
        observer: Box<dyn DownloadObserver>,
    ) -> Result<OfflineRecord, BridgeError> {
        let connection = self.connection().await?;
        let stream = connection
            .source
            .open(share_id, &node_uid(volume_id, link_id))
            .await
            .map_err(BridgeError::from_display)?;
        let revision_id = stream.revision_id().to_owned();
        let block_sizes = stream.block_sizes().to_vec();
        let total = stream.size();
        let path = self.dirs.offline_file(share_id, link_id, &revision_id);
        let parent = path.parent().ok_or_else(|| BridgeError::Failure {
            reason: "offline file has no parent directory".to_owned(),
        })?;
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(BridgeError::from_display)?;

        if tokio::fs::metadata(&path)
            .await
            .is_ok_and(|metadata| metadata.len() == total)
        {
            let (partial, marker) = self.partial_paths(share_id, link_id);
            self.record_offline(share_id, link_id, &revision_id, &block_sizes)?;
            remove_file_if_present(partial).await?;
            remove_file_if_present(marker).await?;
            sync_parent(&path)?;
            observer.on_progress(total, total);
            return Ok(OfflineRecord {
                share_id: share_id.to_owned(),
                link_id: link_id.to_owned(),
                revision_id,
                size: total,
                episode: None,
            });
        }

        let (temporary, marker) = self.partial_paths(share_id, link_id);
        let expected_marker = PartialMarker {
            revision_id: revision_id.clone(),
            block_sizes: block_sizes.clone(),
        };
        let marker_matches = read_partial_marker(&marker)
            .await
            .is_some_and(|stored| stored == expected_marker);
        if !marker_matches {
            remove_file_if_present(temporary.clone()).await?;
            write_partial_marker(&marker, &expected_marker).await?;
        }
        let existing = tokio::fs::metadata(&temporary)
            .await
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let (mut block_index, mut offset) = resume_position(existing, &block_sizes);
        let mut output = prepare_partial_file(&temporary, offset).await?;
        observer.on_progress(offset, total);

        use tokio::io::AsyncWriteExt;
        while block_index < block_sizes.len() {
            if observer.is_cancelled() {
                output.sync_all().await.map_err(BridgeError::from_display)?;
                return Err(BridgeError::Failure {
                    reason: "offline download cancelled".to_owned(),
                });
            }
            let size = block_sizes[block_index];
            let bytes = stream
                .read_range(offset, size)
                .await
                .map_err(BridgeError::from_display)?;
            if bytes.len() as u64 != size {
                return Err(BridgeError::Failure {
                    reason: format!(
                        "short offline block {block_index}: received {}, expected {size}",
                        bytes.len()
                    ),
                });
            }
            output
                .write_all(&bytes)
                .await
                .map_err(BridgeError::from_display)?;
            // A marker promises that all preceding blocks are durable. Sync
            // each completed block before reporting progress or cancellation.
            output
                .sync_data()
                .await
                .map_err(BridgeError::from_display)?;
            offset += size;
            block_index += 1;
            observer.on_progress(offset, total);
        }
        output.sync_all().await.map_err(BridgeError::from_display)?;
        drop(output);
        // Windows does not replace an existing destination with rename. A
        // stale/incomplete destination is never authoritative without its
        // catalog record, so remove it first on every platform.
        remove_file_if_present(path.clone()).await?;
        tokio::fs::rename(&temporary, &path)
            .await
            .map_err(BridgeError::from_display)?;
        sync_parent(&path)?;
        // Recovery ordering: retain the marker until both the final file and
        // SQLite offline record are durable.
        self.record_offline(share_id, link_id, &revision_id, &block_sizes)?;
        remove_file_if_present(marker).await?;
        sync_parent(&path)?;
        Ok(OfflineRecord {
            share_id: share_id.to_owned(),
            link_id: link_id.to_owned(),
            revision_id,
            size: total,
            episode: None,
        })
    }

    fn record_offline(
        &self,
        share_id: &str,
        link_id: &str,
        revision_id: &str,
        block_sizes: &[u64],
    ) -> Result<(), BridgeError> {
        let _publication = self.share_publication.lock();
        // Share removal deletes the store row before catalog/file cleanup. A
        // cancelled worker that unwinds late must therefore refuse to publish
        // after removal, even if it already downloaded its final block.
        let share_present = self
            .store
            .list()
            .map_err(BridgeError::from_display)?
            .iter()
            .any(|share| share.id == share_id);
        if !share_present {
            return Err(BridgeError::Failure {
                reason: "share was removed while the offline download was running".to_owned(),
            });
        }
        self.catalog
            .lock()
            .set_offline_file(
                share_id,
                link_id,
                &OfflineFile {
                    revision_id: revision_id.to_owned(),
                    block_sizes: block_sizes.to_vec(),
                },
            )
            .map_err(BridgeError::from_display)
    }

    async fn remove_offline_episode_inner(
        &self,
        share_id: &str,
        link_id: &str,
    ) -> Result<(), BridgeError> {
        let file = self
            .catalog
            .lock()
            .offline_file(share_id, link_id)
            .map_err(BridgeError::from_display)?;
        if let Some(file) = file {
            let path = self.dirs.offline_file(share_id, link_id, &file.revision_id);
            match tokio::fs::remove_file(&path).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(BridgeError::from_display(error)),
            }
        }
        let (partial, marker) = self.partial_paths(share_id, link_id);
        let _ = tokio::fs::remove_file(partial).await;
        let _ = tokio::fs::remove_file(marker).await;
        self.catalog
            .lock()
            .remove_offline_file(share_id, link_id)
            .map_err(BridgeError::from_display)
    }
}

async fn remove_file_if_present(path: std::path::PathBuf) -> Result<(), BridgeError> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(BridgeError::from_display(error)),
    }
}

async fn read_partial_marker(path: &std::path::Path) -> Option<PartialMarker> {
    let bytes = tokio::fs::read(path).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

async fn prepare_partial_file(
    path: &std::path::Path,
    resume_offset: u64,
) -> Result<tokio::fs::File, BridgeError> {
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)
        .await
        .map_err(BridgeError::from_display)?;
    // Drop an incomplete trailing block before positioning the writer. Merely
    // seeking would leave stale bytes after a shorter replacement write.
    file.set_len(resume_offset)
        .await
        .map_err(BridgeError::from_display)?;
    use tokio::io::AsyncSeekExt;
    file.seek(std::io::SeekFrom::Start(resume_offset))
        .await
        .map_err(BridgeError::from_display)?;
    Ok(file)
}

async fn write_partial_marker(
    path: &std::path::Path,
    marker: &PartialMarker,
) -> Result<(), BridgeError> {
    let temporary = path.with_extension("revision.tmp");
    let bytes = serde_json::to_vec(marker).map_err(BridgeError::from_display)?;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .await
        .map_err(BridgeError::from_display)?;
    use tokio::io::AsyncWriteExt;
    file.write_all(&bytes)
        .await
        .map_err(BridgeError::from_display)?;
    file.sync_all().await.map_err(BridgeError::from_display)?;
    drop(file);
    replace_marker(&temporary, path).await?;
    sync_parent(path)
}

#[cfg(unix)]
async fn replace_marker(from: &std::path::Path, to: &std::path::Path) -> Result<(), BridgeError> {
    // POSIX rename replaces the old directory entry atomically, so readers
    // observe either the old complete marker or the new complete marker.
    tokio::fs::rename(from, to)
        .await
        .map_err(BridgeError::from_display)
}

#[cfg(not(unix))]
async fn replace_marker(from: &std::path::Path, to: &std::path::Path) -> Result<(), BridgeError> {
    // pstr-android executes on Unix, but keep host tooling portable where
    // rename cannot replace a destination.
    remove_file_if_present(to.to_path_buf()).await?;
    tokio::fs::rename(from, to)
        .await
        .map_err(BridgeError::from_display)
}

#[cfg(unix)]
fn sync_parent(path: &std::path::Path) -> Result<(), BridgeError> {
    let parent = path.parent().ok_or_else(|| BridgeError::Failure {
        reason: "offline path has no parent directory".to_owned(),
    })?;
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(BridgeError::from_display)
}

#[cfg(not(unix))]
fn sync_parent(_path: &std::path::Path) -> Result<(), BridgeError> {
    // Windows does not expose directory fsync through std. The file itself is
    // synced before rename and its replace semantics are handled explicitly.
    Ok(())
}

fn node_uid(volume_id: &str, link_id: &str) -> NodeUid {
    NodeUid::new(
        VolumeId::new(volume_id.to_owned()),
        LinkId::new(link_id.to_owned()),
    )
}

fn resume_position(existing: u64, block_sizes: &[u64]) -> (usize, u64) {
    let mut offset = 0_u64;
    for (index, size) in block_sizes.iter().copied().enumerate() {
        if existing < offset.saturating_add(size) {
            return (index, offset);
        }
        offset = offset.saturating_add(size);
    }
    if existing == offset {
        (block_sizes.len(), offset)
    } else {
        // A file longer than the declared revision cannot be trusted.
        (0, 0)
    }
}

fn normalized_language(language: Option<String>) -> Option<String> {
    language.and_then(|language| {
        let language = language.trim();
        (!language.is_empty()).then(|| language.to_ascii_lowercase())
    })
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn watch_state_record(state: WatchState) -> WatchStateRecord {
    WatchStateRecord {
        position_secs: state.position_secs,
        duration_secs: state.duration_secs,
        watched: state.watched,
        updated_at: state.updated_at,
    }
}

fn track_preferences_record(preferences: TitleTrackPrefs) -> TrackPreferencesRecord {
    TrackPreferencesRecord {
        audio_language: preferences.audio_language,
        subtitle_language: preferences.subtitle_language,
        subtitles: preferences.subtitles,
    }
}

fn title_record(
    title: &Title,
    offline: &std::collections::HashMap<(String, String), pstr_core::catalog::OfflineFile>,
    record: Option<&MetadataRecord>,
) -> TitleRecord {
    let metadata = record.and_then(|record| record.metadata.as_ref());
    TitleRecord {
        key: title.key.clone(),
        name: title.name.clone(),
        year: title.year,
        kind: match title.kind {
            TitleKind::Series => TitleType::Series,
            TitleKind::Film => TitleType::Film,
        },
        watched_count: title.watched_count() as u64,
        episode_count: title.episode_count() as u64,
        canonical_name: metadata.map(|metadata| metadata.name.clone()),
        original_name: metadata.and_then(|metadata| metadata.original_name.clone()),
        overview: metadata.and_then(|metadata| metadata.overview.clone()),
        metadata_provider: metadata.map(|metadata| metadata_provider(metadata.provider)),
        metadata_id: metadata.map(|metadata| metadata.remote_id.clone()),
        metadata_year: metadata.and_then(|metadata| metadata.year),
        metadata_kind: metadata.map(|metadata| title_type(metadata.kind)),
        poster_url: metadata.and_then(|metadata| metadata.poster_url.clone()),
        backdrop_url: metadata.and_then(|metadata| metadata.backdrop_url.clone()),
        rating: metadata.and_then(|metadata| metadata.rating.map(f64::from)),
        genres: metadata.map_or_else(Vec::new, |metadata| metadata.genres.clone()),
        provider_episode_count: metadata.and_then(|metadata| metadata.episodes),
        external_url: metadata.and_then(|metadata| metadata.url.clone()),
        manual_match: record.is_some_and(|record| record.manual),
        seasons: title
            .seasons
            .iter()
            .map(|season| SeasonRecord {
                number: season.number,
                label: season.label(),
                episodes: season
                    .episodes
                    .iter()
                    .map(|episode| episode_record(episode, offline))
                    .collect(),
            })
            .collect(),
    }
}

fn metadata_provider(provider: ProviderId) -> MetadataProvider {
    match provider {
        ProviderId::AniList => MetadataProvider::AniList,
        ProviderId::Tmdb => MetadataProvider::Tmdb,
    }
}

fn provider_id(provider: MetadataProvider) -> ProviderId {
    match provider {
        MetadataProvider::AniList => ProviderId::AniList,
        MetadataProvider::Tmdb => ProviderId::Tmdb,
    }
}

fn title_type(kind: TitleKind) -> TitleType {
    match kind {
        TitleKind::Series => TitleType::Series,
        TitleKind::Film => TitleType::Film,
    }
}

fn title_kind(kind: TitleType) -> TitleKind {
    match kind {
        TitleType::Series => TitleKind::Series,
        TitleType::Film => TitleKind::Film,
    }
}

fn match_record(metadata: TitleMetadata) -> MatchRecord {
    MatchRecord {
        provider: metadata_provider(metadata.provider),
        remote_id: metadata.remote_id,
        name: metadata.name,
        original_name: metadata.original_name,
        overview: metadata.overview,
        year: metadata.year,
        kind: title_type(metadata.kind),
        poster_url: metadata.poster_url,
        backdrop_url: metadata.backdrop_url,
        rating: metadata.rating.map(f64::from),
        genres: metadata.genres,
        episode_count: metadata.episodes,
        external_url: metadata.url,
    }
}

fn title_metadata(record: MatchRecord) -> TitleMetadata {
    TitleMetadata {
        provider: provider_id(record.provider),
        remote_id: record.remote_id,
        name: record.name,
        original_name: record.original_name,
        overview: record.overview,
        year: record.year,
        kind: title_kind(record.kind),
        poster_url: record.poster_url,
        backdrop_url: record.backdrop_url,
        rating: record.rating.map(|rating| rating as f32),
        genres: record.genres,
        episodes: record.episode_count,
        url: record.external_url,
    }
}

fn episode_record(
    episode: &Episode,
    offline: &std::collections::HashMap<(String, String), pstr_core::catalog::OfflineFile>,
) -> EpisodeRecord {
    let node = &episode.node;
    EpisodeRecord {
        share_id: node.share_id.clone(),
        volume_id: node.volume_id.clone(),
        link_id: node.link_id.clone(),
        name: node.name.clone(),
        label: episode.label(),
        detail: episode.detail().to_owned(),
        season: node.parsed.season,
        number: node.parsed.episode,
        size: node.size.and_then(|size| u64::try_from(size).ok()),
        progress: episode.progress(),
        resume_at: episode.resume_at(),
        watched: episode.is_watched(),
        offline: offline.contains_key(&(node.share_id.clone(), node.link_id.clone())),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use parking_lot::Mutex;
    use pstr_core::library::TitleKind;
    use pstr_core::metadata::{ProviderId, TitleMetadata};
    use pstr_stream::{MemoryBlocks, VideoStream};

    use super::{
        AndroidEngine, AndroidPaths, AndroidSecretStore, AndroidStream, BridgeError, PartialMarker,
        match_record, node_uid, normalized_language, prepare_partial_file,
        pstr_android_stream_read, pstr_android_stream_release, pstr_android_stream_size,
        read_partial_marker, resume_position, title_metadata, write_partial_marker,
    };

    #[derive(Default)]
    struct MemorySecrets(Mutex<HashMap<String, String>>);

    impl AndroidSecretStore for MemorySecrets {
        fn set(&self, key: String, value: String) -> Result<(), BridgeError> {
            self.0.lock().insert(key, value);
            Ok(())
        }

        fn get(&self, key: String) -> Result<Option<String>, BridgeError> {
            Ok(self.0.lock().get(&key).cloned())
        }

        fn delete(&self, key: String) -> Result<(), BridgeError> {
            self.0.lock().remove(&key);
            Err(BridgeError::Failure {
                reason: "simulated secret deletion failure".to_owned(),
            })
        }
    }

    #[test]
    fn a_partial_download_resumes_only_at_a_complete_block() {
        let sizes = [4, 7, 3];

        assert_eq!(resume_position(0, &sizes), (0, 0));
        assert_eq!(resume_position(4, &sizes), (1, 4));
        assert_eq!(resume_position(11, &sizes), (2, 11));
        assert_eq!(resume_position(14, &sizes), (3, 14));
    }

    #[test]
    fn a_partial_block_is_restarted_instead_of_shifting_later_content() {
        assert_eq!(resume_position(6, &[4, 7, 3]), (1, 4));
        assert_eq!(resume_position(99, &[4, 7, 3]), (0, 0));
    }

    #[test]
    fn a_trailing_partial_block_is_physically_truncated_before_resume() {
        let root = std::env::temp_dir().join(format!(
            "pstr-android-partial-{}-{}",
            std::process::id(),
            super::now()
        ));
        std::fs::create_dir_all(&root).expect("partial directory");
        let path = root.join("episode.part");
        std::fs::write(&path, b"abcdef").expect("partial bytes");
        let (_, offset) = resume_position(6, &[4, 7, 3]);
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let file = runtime
            .block_on(prepare_partial_file(&path, offset))
            .expect("prepare partial");
        drop(file);
        assert_eq!(std::fs::read(&path).expect("read partial"), b"abcd");
        std::fs::remove_dir_all(root).expect("remove partial directory");
    }

    #[test]
    fn partial_markers_reject_revision_or_block_layout_mismatches() {
        let expected = PartialMarker {
            revision_id: "revision-two".to_owned(),
            block_sizes: vec![4, 7, 3],
        };
        assert_ne!(
            expected,
            PartialMarker {
                revision_id: "revision-one".to_owned(),
                block_sizes: vec![4, 7, 3],
            }
        );
        assert_ne!(
            expected,
            PartialMarker {
                revision_id: "revision-two".to_owned(),
                block_sizes: vec![4, 8, 2],
            }
        );
    }

    #[test]
    fn partial_markers_are_atomically_readable_with_exact_block_sizes() {
        let root = std::env::temp_dir().join(format!(
            "pstr-android-marker-{}-{}",
            std::process::id(),
            super::now()
        ));
        std::fs::create_dir_all(&root).expect("marker directory");
        let path = root.join("episode.revision");
        let expected = PartialMarker {
            revision_id: "revision".to_owned(),
            block_sizes: vec![4, 7, 3],
        };
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime
            .block_on(write_partial_marker(&path, &expected))
            .expect("write marker");
        assert_eq!(runtime.block_on(read_partial_marker(&path)), Some(expected));
        assert!(!path.with_extension("revision.tmp").exists());
        std::fs::remove_dir_all(root).expect("remove marker directory");
    }

    #[test]
    fn language_preferences_are_trimmed_and_normalized() {
        assert_eq!(
            normalized_language(Some(" JPN ".to_owned())),
            Some("jpn".to_owned())
        );
        assert_eq!(normalized_language(Some("  ".to_owned())), None);
        assert_eq!(normalized_language(None), None);
    }

    #[test]
    fn metadata_match_records_preserve_every_provider_field() {
        let metadata = TitleMetadata {
            provider: ProviderId::AniList,
            remote_id: "1".to_owned(),
            name: "Cowboy Bebop".to_owned(),
            original_name: Some("カウボーイビバップ".to_owned()),
            overview: Some("Bounty hunters in space.".to_owned()),
            year: Some(1998),
            kind: TitleKind::Series,
            poster_url: Some("https://example.test/poster.jpg".to_owned()),
            backdrop_url: Some("https://example.test/backdrop.jpg".to_owned()),
            rating: Some(8.7),
            genres: vec!["Action".to_owned(), "Sci-Fi".to_owned()],
            episodes: Some(26),
            url: Some("https://anilist.co/anime/1".to_owned()),
        };

        assert_eq!(title_metadata(match_record(metadata.clone())), metadata);
    }

    #[test]
    fn native_stream_handles_read_size_and_release_without_uniffi_bytes() {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("runtime"),
        );
        let blocks = Arc::new(MemoryBlocks::new(
            "revision",
            vec![b"first".to_vec(), b"second".to_vec()],
        ));
        let stream = Arc::new(AndroidStream {
            runtime,
            stream: VideoStream::offline(node_uid("volume", "link"), blocks, 1024),
        });
        let handle = stream.native_handle();

        assert_eq!(pstr_android_stream_size(handle), 11);
        let mut bytes = [0_u8; 6];
        // SAFETY: `bytes` is writable for the requested six bytes.
        let read =
            unsafe { pstr_android_stream_read(handle, 5, bytes.as_mut_ptr().cast(), bytes.len()) };
        assert_eq!(read, 6);
        assert_eq!(&bytes, b"second");

        pstr_android_stream_release(handle);
        assert_eq!(pstr_android_stream_size(handle), -1);
    }

    #[test]
    fn share_mutations_cleanup_even_when_secret_deletion_fails() {
        let unique = format!(
            "pstr-android-generation-{}-{}",
            std::process::id(),
            super::now()
        );
        let root = std::env::temp_dir().join(unique);
        let engine = AndroidEngine::new(
            AndroidPaths {
                config: root.join("config").to_string_lossy().into_owned(),
                data: root.join("data").to_string_lossy().into_owned(),
                cache: root.join("cache").to_string_lossy().into_owned(),
            },
            Box::<MemorySecrets>::default(),
        )
        .expect("engine");

        let share = engine
            .add_share(
                "test".to_owned(),
                "https://drive.proton.me/urls/ABC123#s3cr3t".to_owned(),
                None,
            )
            .expect("add share");
        assert_eq!(engine.share_generation.load(Ordering::Acquire), 1);
        assert!(
            engine
                .save_watch_state(
                    share.id.clone(),
                    "episode".to_owned(),
                    11.0,
                    Some(10.0),
                    false
                )
                .is_err()
        );
        engine
            .save_watch_state(
                share.id.clone(),
                "episode".to_owned(),
                5.0,
                Some(10.0),
                false,
            )
            .expect("valid watch state");

        let (partial, marker) = engine.partial_paths(&share.id, "unfinished");
        std::fs::create_dir_all(partial.parent().expect("offline directory"))
            .expect("create offline directory");
        std::fs::write(&partial, b"partial block").expect("partial");
        std::fs::write(&marker, b"revision").expect("marker");
        engine
            .runtime
            .block_on(engine.remove_offline_episode_inner(&share.id, "unfinished"))
            .expect("remove partial");
        assert!(!partial.exists());
        assert!(!marker.exists());

        assert!(engine.remove_share(share.id.clone()).is_err());
        assert_eq!(engine.share_generation.load(Ordering::Acquire), 2);
        assert!(engine.shares().expect("shares").is_empty());
        assert!(
            engine
                .watch_state(share.id, "episode".to_owned())
                .expect("watch state")
                .is_none()
        );

        drop(engine);
        std::fs::remove_dir_all(root).expect("remove test directory");
    }
}

uniffi::setup_scaffolding!();
