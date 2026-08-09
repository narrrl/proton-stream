//! Kotlin-facing Android host for the portable proton-stream crates.
//!
//! This boundary deliberately exposes screen-shaped immutable records rather
//! than the catalog's internal Rust types. Kotlin owns lifecycle and drawing;
//! Rust remains the sole owner of Proton sessions, SQLite and decrypted bytes.

use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use pstr_core::appearance::{Accent, Appearance, Flavor, Palette};
use pstr_core::catalog::{Catalog, OfflineFile, TitleTrackPrefs, WatchState, build_rows};
use pstr_core::chapters::{Chapter, ChapterRole};
use pstr_core::config::AppDirs;
use pstr_core::library::{Episode, Library, Title, TitleKind};
use pstr_core::metadata::{
    EpisodeGuide, MetadataConfig, MetadataRecord, ProviderId, TitleMetadata,
};
use pstr_core::prefs::PlaybackPrefs;
use pstr_core::proton_drive_rs::{ProtonDrivePublicLinkClient, ThumbnailType};
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
) -> jni::sys::jboolean {
    android_logger::init_once(
        android_logger::Config::default()
            // A shipped APK writing rustls handshake internals to logcat is
            // both noise and a disclosure; trace is for a build that asked for
            // it.
            .with_max_level(if cfg!(debug_assertions) {
                log::LevelFilter::Debug
            } else {
                log::LevelFilter::Info
            })
            .with_tag("protonstream-rust"),
    );
    match rustls_platform_verifier::android::init_with_env(&mut env, context) {
        Ok(()) => jni::sys::JNI_TRUE,
        Err(error) => {
            // Every later HTTPS request will fail certificate validation, and
            // without this the viewer only sees that much later and without a
            // reason.
            log::error!("certificate verifier init: {error}");
            jni::sys::JNI_FALSE
        }
    }
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

/// Without this, uniffi routes an unmapped exception thrown by a Kotlin
/// callback into `handle_callback_unexpected_error`, which panics
/// unconditionally — on whichever thread happened to be calling, including a
/// tokio worker in the middle of a download. Every such exception is now an
/// ordinary `BridgeError` the caller can decide about.
impl From<uniffi::UnexpectedUniFFICallbackError> for BridgeError {
    fn from(error: uniffi::UnexpectedUniFFICallbackError) -> Self {
        Self::Failure {
            reason: error.reason,
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

/// A palette family. The shared `pstr_core::appearance::Flavor`.
#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum FlavorChoice {
    Proton,
    Latte,
    Frappe,
    Macchiato,
    Mocha,
}

/// The one strong colour. The shared `pstr_core::appearance::Accent`.
#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum AccentChoice {
    Mauve,
    Pink,
    Sky,
    PinkSky,
    Lavender,
    Blue,
    Teal,
    Peach,
}

/// What the viewer chose, stored where the desktop client reads it too.
#[derive(Debug, Clone, uniffi::Record)]
pub struct AppearanceRecord {
    pub flavor: FlavorChoice,
    pub accent: AccentChoice,
    pub gradients: bool,
}

/// One flavour and accent resolved into colours, packed as `0xAARRGGBB`.
///
/// Resolved in Rust rather than in Kotlin on purpose: the ramps, the accent
/// pairings and the contrast rule that picks readable ink are one
/// implementation shared with the desktop client, so a flavour looks like
/// itself on both.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PaletteRecord {
    pub background: u32,
    pub surface: u32,
    pub sunken: u32,
    pub card: u32,
    pub card_hover: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub accent_alt: u32,
    pub accent_dim: u32,
    pub on_accent: u32,
    pub danger: u32,
    pub light: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MetadataSettingsRecord {
    pub enabled: bool,
    pub provider: MetadataProvider,
    pub language: String,
    pub ready: bool,
}

/// What one enrichment pass did, for the viewer who pressed the button.
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct MatchSummary {
    pub matched: u32,
    pub unmatched: u32,
    pub failed: u32,
    pub episodes: u32,
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
    /// What the metadata provider calls this episode, if it named it.
    pub provider_name: Option<String>,
    pub provider_overview: Option<String>,
    pub still_url: Option<String>,
    pub air_date: Option<String>,
    /// When this episode was last played, as a Unix timestamp; 0 if never.
    ///
    /// What a continue-watching shelf orders by — "where I was last" is a
    /// question about time, and progress alone cannot answer it.
    pub last_played: i64,
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

/// Playback preferences that outlive one file, one title and one launch.
///
/// The bridge shape of [`pstr_core::prefs::PlaybackPrefs`] — deliberately the
/// same store the desktop client uses, so there is one definition of what a
/// viewer's language and volume choices are rather than one per front end.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PlaybackPrefsRecord {
    /// 0–100.
    pub volume: f64,
    pub muted: bool,
    /// The language tag of the audio track to prefer — "jpn", "eng". `None`
    /// leaves the choice to the container's default.
    pub audio_language: Option<String>,
    pub subtitle_language: Option<String>,
    pub subtitles: bool,
    pub autoplay_next: bool,
    pub auto_skip: bool,
    /// 1.0 is the file's own rate; the bridge clamps to what mpv can play.
    pub speed: f64,
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

/// One chapter as the JNI adapter read it out of mpv's `chapter-list`.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ChapterRecord {
    pub index: i64,
    pub title: Option<String>,
    pub start: f64,
}

/// What a chapter is, once the rest of the file has been taken into account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ChapterKind {
    Opening,
    Ending,
    Preview,
    Content,
}

/// A chapter with everything the player needs to draw and jump to it.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ChapterEntry {
    pub index: i64,
    /// Never empty: an unnamed chapter is still a place in the file.
    pub label: String,
    pub start: f64,
    /// The next chapter's start, or the end of the file. `None` while the
    /// duration is still unknown and this is the last chapter — there is
    /// nowhere to skip to, and a button that seeks to zero is worse than none.
    pub end: Option<f64>,
    pub kind: ChapterKind,
}

/// Every chapter of the open file, resolved.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ChapterPlan {
    pub entries: Vec<ChapterEntry>,
    /// Where the run of chapters that ends the episode begins — the point an
    /// "up next" countdown starts from. `None` for a file that ends on content.
    pub credits_start: Option<f64>,
}

/// What the player may offer to skip right now.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SkipOffer {
    pub label: String,
    /// Where skipping lands, which is the end of the chapter it skips.
    pub target: f64,
}

/// Resolve a file's chapters through the same rules the desktop player uses
/// (`pstr_core::chapters`), so an opening called `Intro` is read identically on
/// both. Kotlin reads the list out of mpv; the verdict is reached here.
#[uniffi::export]
pub fn chapter_plan(chapters: Vec<ChapterRecord>, duration: Option<f64>) -> ChapterPlan {
    let duration = duration.filter(|duration| *duration > 0.0);
    let chapters: Vec<Chapter> = chapters
        .into_iter()
        .map(|chapter| Chapter {
            index: chapter.index,
            title: chapter
                .title
                .map(|title| title.trim().to_owned())
                .filter(|title| !title.is_empty()),
            start: chapter.start,
        })
        .collect();
    let roles = pstr_core::chapters::roles(&chapters, duration);
    ChapterPlan {
        credits_start: pstr_core::chapters::credits_start(&chapters, &roles),
        entries: chapters
            .iter()
            .zip(&roles)
            .enumerate()
            .map(|(index, (chapter, role))| ChapterEntry {
                index: chapter.index,
                label: chapter.label(),
                start: chapter.start,
                end: pstr_core::chapters::chapter_end(&chapters, index, duration),
                kind: chapter_kind(*role),
            })
            .collect(),
    }
}

/// The one thing worth offering to skip at this position, if any.
///
/// Nothing is offered once the end of the chapter is behind the playhead: the
/// last chapter of a file "ends" at the duration, and a skip button that sits
/// there through the credits is one that never goes away.
#[uniffi::export]
pub fn skip_offer(plan: &ChapterPlan, position: f64) -> Option<SkipOffer> {
    let entry = plan
        .entries
        .iter()
        .rposition(|entry| position + f64::EPSILON >= entry.start)
        .and_then(|index| plan.entries.get(index))?;
    let label = match entry.kind {
        ChapterKind::Opening => "Skip opening",
        ChapterKind::Ending => "Skip ending",
        ChapterKind::Preview => "Skip preview",
        ChapterKind::Content => return None,
    };
    let target = entry.end.filter(|end| *end > position)?;
    Some(SkipOffer {
        label: label.to_owned(),
        target,
    })
}

fn chapter_kind(role: ChapterRole) -> ChapterKind {
    match role {
        ChapterRole::Opening => ChapterKind::Opening,
        ChapterRole::Ending => ChapterKind::Ending,
        ChapterRole::Preview => ChapterKind::Preview,
        ChapterRole::Content => ChapterKind::Content,
    }
}

/// What the app is holding on disk. Offline episodes are the viewer's, the
/// block cache is not: only the latter may be reclaimed without asking.
#[derive(Debug, Clone, uniffi::Record)]
pub struct StorageUsageRecord {
    pub offline_bytes: u64,
    pub offline_count: u64,
    /// Bytes in `.part` files of downloads that were paused or interrupted.
    pub partial_bytes: u64,
    pub cache_bytes: u64,
}

/// Implemented by a WorkManager worker. Cancellation is polled only between
/// complete Proton blocks, so a retained `.part` file is always resumable.
///
/// Both are fallible because the implementation is Android's: `setForegroundAsync`
/// throws routinely on API 31+ when the app is backgrounded, and a throw that
/// cannot be reported is a panic on the tokio worker carrying the download.
#[uniffi::export(callback_interface)]
pub trait DownloadObserver: Send + Sync {
    fn on_progress(&self, downloaded: u64, total: u64) -> Result<(), BridgeError>;
    fn is_cancelled(&self) -> Result<bool, BridgeError>;
}

/// A spawned task that dies with the caller that is waiting on it.
///
/// UniFFI cancellation drops the Rust future, and dropping a bare `JoinHandle`
/// *detaches* the task rather than aborting it — so backing out of a screen
/// leaves its provider requests and SQLite writes running, and a cancelled
/// download keeps calling back into a `DownloadObserver` WorkManager considers
/// dead. Abort points are await points, so a blocking SQLite statement always
/// completes; what is abandoned is the work after it.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// `runtime.spawn(task)`, with the handle wrapped so the task is aborted if the
/// caller stops waiting. Awaits to exactly what `JoinHandle` does.
fn spawned<T: Send + 'static>(
    runtime: &tokio::runtime::Runtime,
    task: impl std::future::Future<Output = T> + Send + 'static,
) -> AbortOnDrop<T> {
    AbortOnDrop(runtime.spawn(task))
}

impl<T> std::future::Future for AbortOnDrop<T> {
    type Output = std::result::Result<T, tokio::task::JoinError>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::pin::Pin::new(&mut self.get_mut().0).poll(context)
    }
}

/// A progress report that could not be delivered is not a reason to abandon a
/// transfer: the bytes are on disk either way, and the UI it feeds is
/// rebuildable from `DownloadStateStore`.
fn report(observer: &dyn DownloadObserver, downloaded: u64, total: u64) {
    if let Err(error) = observer.on_progress(downloaded, total) {
        log::warn!("download progress could not be reported: {error}");
    }
}

/// An unanswerable cancellation question is answered "no": the download keeps
/// going, WorkManager stops the worker on its own schedule, and the retained
/// `.part` file makes the next attempt a resume rather than a restart.
fn cancelled(observer: &dyn DownloadObserver) -> bool {
    observer.is_cancelled().unwrap_or(false)
}

/// A seekable revision for libmpv's Android stream callback.
#[derive(uniffi::Object)]
pub struct AndroidStream {
    runtime: Arc<tokio::runtime::Runtime>,
    stream: VideoStream,
    /// Tripped by libmpv's `cancel_fn`, through
    /// [`pstr_android_stream_cancel`].
    ///
    /// A block fetch is a network round trip on a 4 MiB body, and the demuxer
    /// thread parked inside one cannot be joined until it returns. That thread
    /// is what `mpv_terminate_destroy` waits on, so without a way to interrupt
    /// the read, closing the player on a degraded connection blocks for a whole
    /// block — and a seek issued during one does not take effect until it
    /// finishes.
    cancel: tokio::sync::Notify,
    /// The registry id this stream was published under, once it has been.
    native_id: Mutex<Option<u64>>,
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
        // One id per stream: a second call must not mint a second registry
        // entry, which nothing would ever release.
        let mut published = self.native_id.lock();
        if let Some(id) = *published {
            return id;
        }
        let id = NEXT_NATIVE_STREAM.fetch_add(1, Ordering::Relaxed);
        native_streams().lock().insert(id, Arc::clone(&self));
        *published = Some(id);
        id
    }
}

impl AndroidStream {
    /// Read decrypted bytes strictly inside the native boundary. This is not a
    /// UniFFI export: only `pstr_android_stream_read` may move these bytes, and
    /// it copies them directly into libmpv-owned memory.
    fn read_range_for_native(&self, offset: u64, length: u64) -> pstr_stream::Result<Vec<u8>> {
        self.runtime.block_on(async {
            tokio::select! {
                result = self.stream.read_range(offset, length) => result,
                () = self.cancel.notified() => Err(pstr_stream::Error::NotFound(
                    "read cancelled".to_owned(),
                )),
            }
        })
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

/// Interrupt whatever read is in flight for this stream, from libmpv's
/// `cancel_fn`.
///
/// Only an in-flight read is affected: a cancel that arrives with no reader
/// waiting is dropped, which is what makes this safe to call speculatively. The
/// read it aborts fails, and libmpv reissues it if it still wants those bytes —
/// a seek and a teardown both want the request slot back rather than the data.
#[unsafe(no_mangle)]
pub extern "C" fn pstr_android_stream_cancel(handle: u64) {
    let _ = std::panic::catch_unwind(|| {
        let stream = native_streams().lock().get(&handle).cloned();
        if let Some(stream) = stream {
            stream.cancel.notify_waiters();
        }
    });
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
    /// The live connection, if one has been built for the current generation.
    ///
    /// Deliberately a sync mutex rather than the single-flight lock below: the
    /// share mutations that invalidate it are sync `&self` methods called from
    /// the JNI thread, and dropping a stale connection must happen *in* them
    /// rather than being deferred to whoever next asks for a connection.
    connection: Mutex<Option<Connection>>,
    /// Held across the open handshakes so concurrent callers share one build.
    connecting: tokio::sync::Mutex<()>,
    /// Clients carried over from a retired connection, for the next build.
    reusable_clients: Mutex<Option<BTreeMap<String, ProtonDrivePublicLinkClient>>>,
    /// The whole library as bridge records, behind the catalog's write counter.
    library_cache: Mutex<Option<CachedLibrary>>,
    /// Serializes offline publication with share removal cleanup.
    share_publication: Mutex<()>,
    /// Advances after every successful share-store mutation. A connection is
    /// reusable only while it describes this exact generation of the store.
    share_generation: AtomicU64,
    /// How many Proton thumbnails may be in flight at once.
    thumbnails: tokio::sync::Semaphore,
    /// When each share's visitor session was last refreshed.
    ///
    /// Sessions are short-lived and the app is routinely backgrounded for hours
    /// between one episode and the next, so age — not just a share-store
    /// mutation — is a reason to reauthenticate before opening anything.
    session_refreshed: Mutex<HashMap<String, std::time::Instant>>,
}

/// How long a visitor session is assumed good for without a refresh.
///
/// A refresh costs one round trip. Not refreshing costs an authentication
/// failure that, before this, the viewer could only clear with a manual
/// pull-to-refresh.
const SESSION_FRESH_FOR: std::time::Duration = std::time::Duration::from_secs(5 * 60);

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
            connection: Mutex::new(None),
            connecting: tokio::sync::Mutex::new(()),
            reusable_clients: Mutex::new(None),
            library_cache: Mutex::new(None),
            share_publication: Mutex::new(()),
            share_generation: AtomicU64::new(0),
            thumbnails: tokio::sync::Semaphore::new(THUMBNAIL_CONCURRENCY),
            session_refreshed: Mutex::new(HashMap::new()),
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
        self.invalidate_connection(&share.id);
        Ok(ShareRecord {
            id: share.id,
            name: share.name,
            has_custom_password: share.has_custom_password,
        })
    }

    /// Re-supply the link behind a share whose stored secret is unreadable.
    ///
    /// The way back from an invalidated Keystore key, which otherwise makes
    /// every share permanently unopenable and takes the catalog and the offline
    /// files with it if the answer is Remove-and-re-add.
    pub fn repair_share(
        &self,
        share_id: String,
        url: String,
        custom_password: Option<String>,
    ) -> Result<ShareRecord, BridgeError> {
        let share = self
            .store
            .replace_secrets(&share_id, &url, custom_password.as_deref())
            .map_err(BridgeError::from_display)?;
        self.invalidate_connection(&share.id);
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
        self.invalidate_connection(&share_id);

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

    /// Drop catalog rows for offline files whose bytes are gone or truncated.
    ///
    /// Its own entry point rather than a side effect of reading the library:
    /// four table scans and a `stat` per offline file is not what a keystroke
    /// in the search box should cost, and a query that writes cannot be cached.
    pub fn prune_offline_files(&self) -> Result<u32, BridgeError> {
        let catalog = self.catalog.lock();
        let offline = catalog
            .all_offline_files()
            .map_err(BridgeError::from_display)?;
        let mut pruned = 0;
        for ((share_id, link_id), file) in offline {
            let path = self
                .dirs
                .offline_file(&share_id, &link_id, &file.revision_id);
            let valid = std::fs::metadata(path)
                .is_ok_and(|metadata| metadata.len() == file.block_sizes.iter().sum::<u64>());
            if !valid {
                catalog
                    .remove_offline_file(&share_id, &link_id)
                    .map_err(BridgeError::from_display)?;
                pruned += 1;
            }
        }
        Ok(pruned)
    }

    /// The library, filtered by `search`.
    ///
    /// The whole conversion is cached behind the catalog's write counter, so a
    /// search is a filter over records that already exist rather than four
    /// table scans, a `Library::build` and a full record conversion per
    /// keystroke.
    pub fn library(&self, search: Option<String>) -> Result<Vec<TitleRecord>, BridgeError> {
        let titles = self.all_title_records()?;
        let needle = search.unwrap_or_default().trim().to_lowercase();
        if needle.is_empty() {
            return Ok(titles);
        }
        Ok(titles
            .into_iter()
            .filter(|title| {
                title.name.to_lowercase().contains(&needle)
                    || title.seasons.iter().any(|season| {
                        season
                            .episodes
                            .iter()
                            .any(|episode| episode.name.to_lowercase().contains(&needle))
                    })
            })
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

    pub async fn match_titles(self: Arc<Self>, force: bool) -> Result<MatchSummary, BridgeError> {
        let runtime = Arc::clone(&self.runtime);
        spawned(
            &runtime,
            async move { self.match_titles_inner(force).await },
        )
        .await
        .map_err(BridgeError::from_display)?
    }

    pub async fn search_matches(
        self: Arc<Self>,
        title_key: String,
        term: String,
    ) -> Result<Vec<MatchRecord>, BridgeError> {
        let runtime = Arc::clone(&self.runtime);
        spawned(&runtime, async move {
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

    /// What this title was last watched as, if anything was ever chosen for it.
    ///
    /// `None` is not the same as the defaults: a title with no choice of its own
    /// falls back to [`Self::playback_prefs`], which is how a language picked on
    /// one show carries to the next.
    pub fn title_track_preferences(
        &self,
        title_key: String,
    ) -> Result<Option<TrackPreferencesRecord>, BridgeError> {
        self.catalog
            .lock()
            .title_track_prefs(&title_key)
            .map_err(BridgeError::from_display)
            .map(|prefs| prefs.map(track_preferences_record))
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

    /// The preferences that hold across titles and launches.
    ///
    /// A per-title choice (`title_track_preferences`) overrides these for the
    /// title it was made on; these are what a title with no choice of its own
    /// starts from, and what the desktop client reads from the same file.
    pub fn playback_prefs(&self) -> Result<PlaybackPrefsRecord, BridgeError> {
        pstr_core::prefs::load(&self.dirs)
            .map_err(BridgeError::from_display)
            .map(playback_prefs_record)
    }

    pub fn set_playback_prefs(&self, prefs: PlaybackPrefsRecord) -> Result<(), BridgeError> {
        // Sanitized rather than trusted: the volume arrives from a slider, and
        // a language from a text field that can hold spaces and nothing else.
        let prefs = playback_prefs(prefs).sanitized();
        pstr_core::prefs::save(&self.dirs, &prefs).map_err(BridgeError::from_display)
    }

    pub fn appearance(&self) -> Result<AppearanceRecord, BridgeError> {
        pstr_core::appearance::load(&self.dirs)
            .map_err(BridgeError::from_display)
            .map(appearance_record)
    }

    pub fn set_appearance(&self, appearance: AppearanceRecord) -> Result<(), BridgeError> {
        pstr_core::appearance::save(&self.dirs, &appearance_choice(appearance))
            .map_err(BridgeError::from_display)
    }

    /// The colours the stored choice resolves to.
    pub fn palette(&self) -> Result<PaletteRecord, BridgeError> {
        self.appearance().map(|record| palette_record(&record))
    }

    /// The colours a choice *would* resolve to, without storing it — what a
    /// picker previews as the viewer moves through the flavours.
    pub fn preview_palette(&self, appearance: AppearanceRecord) -> PaletteRecord {
        palette_record(&appearance)
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
                    episode_record(episode, &files, None),
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

    /// What the app is holding on disk, counted from the files themselves
    /// rather than from the catalog: a `.part` left by a paused download has no
    /// catalog row, and it is exactly the space a viewer cannot account for.
    pub fn storage_usage(&self) -> Result<StorageUsageRecord, BridgeError> {
        let offline = self
            .catalog
            .lock()
            .all_offline_files()
            .map_err(BridgeError::from_display)?;
        let mut offline_bytes = 0;
        let mut offline_count = 0;
        for ((share_id, link_id), file) in &offline {
            let path = self.dirs.offline_file(share_id, link_id, &file.revision_id);
            if let Ok(metadata) = std::fs::metadata(path) {
                offline_bytes += metadata.len();
                offline_count += 1;
            }
        }
        Ok(StorageUsageRecord {
            offline_bytes,
            offline_count,
            partial_bytes: directory_bytes(&self.dirs.offline_content(), Some("part")),
            cache_bytes: directory_bytes(&self.dirs.block_cache(), None),
        })
    }

    /// Forget every offline episode. The block cache is untouched — it is a
    /// cache — and so is anything a running download owns, which the caller
    /// cancels first.
    pub async fn remove_all_offline(self: Arc<Self>) -> Result<(), BridgeError> {
        let runtime = Arc::clone(&self.runtime);
        spawned(&runtime, async move {
            let offline = self
                .catalog
                .lock()
                .all_offline_files()
                .map_err(BridgeError::from_display)?;
            for (share_id, link_id) in offline.keys() {
                self.remove_offline_episode_inner(share_id, link_id).await?;
            }
            // Then sweep the directory, because the catalog is not a complete
            // account of what is on disk: a `.part` from an interrupted download
            // has no catalog row at all, and it is exactly the file this button
            // exists to reclaim. Callers cancel outstanding work first, so
            // nothing here is being written to.
            sweep_directory(&self.dirs.offline_content());
            Ok(())
        })
        .await
        .map_err(BridgeError::from_display)?
    }

    /// Drop the cached blocks. Rebuildable by definition: this is the one thing
    /// here that can be reclaimed without losing something the viewer chose.
    pub fn clear_block_cache(&self) -> Result<u64, BridgeError> {
        let cache = self.dirs.block_cache();
        let reclaimed = directory_bytes(&cache, None);
        match std::fs::remove_dir_all(&cache) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(BridgeError::from_display(error)),
        }
        std::fs::create_dir_all(&cache).map_err(BridgeError::from_display)?;
        Ok(reclaimed)
    }

    pub async fn open_stream(
        self: Arc<Self>,
        share_id: String,
        volume_id: String,
        link_id: String,
    ) -> Result<Arc<AndroidStream>, BridgeError> {
        let runtime = Arc::clone(&self.runtime);
        spawned(&runtime, async move {
            let stream = self
                .open_stream_inner(&share_id, &volume_id, &link_id)
                .await?;
            Ok(Arc::new(AndroidStream {
                runtime: Arc::clone(&self.runtime),
                stream,
                cancel: tokio::sync::Notify::new(),
                native_id: Mutex::new(None),
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
        spawned(&runtime, async move {
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
        spawned(&runtime, async move {
            self.remove_offline_episode_inner(&share_id, &link_id).await
        })
        .await
        .map_err(BridgeError::from_display)?
    }

    pub async fn release_stream(
        self: Arc<Self>,
        share_id: String,
        volume_id: String,
        link_id: String,
    ) -> Result<(), BridgeError> {
        let runtime = Arc::clone(&self.runtime);
        spawned(&runtime, async move {
            // Unconditional: a share mutated while the player was open does not
            // make this stream any less open, and skipping the close leaks the
            // reader and its read-ahead for the life of the source.
            let source = self
                .connection
                .lock()
                .as_ref()
                .map(|connection| connection.source.clone());
            if let Some(source) = source {
                source.close(&share_id, &node_uid(&volume_id, &link_id));
            }
        })
        .await
        .map_err(BridgeError::from_display)
    }

    /// Proton's own thumbnail for one file, decrypted, as encoded image bytes.
    ///
    /// This is the poster of last resort, and with metadata lookups off — the
    /// privacy default — it is the *only* artwork a freshly crawled library
    /// has. `None` means the file has no thumbnail at all, which is common
    /// rather than exceptional: Proton renders them at upload time, so a share
    /// filled by a client that attaches none has none. The caller has to
    /// remember that answer, or every recomposition pays a round trip for it.
    pub async fn thumbnail(
        self: Arc<Self>,
        share_id: String,
        volume_id: String,
        link_id: String,
    ) -> Result<Option<Vec<u8>>, BridgeError> {
        let runtime = Arc::clone(&self.runtime);
        spawned(&runtime, async move {
            self.thumbnail_inner(&share_id, &volume_id, &link_id).await
        })
        .await
        .map_err(BridgeError::from_display)?
    }

    pub async fn crawl(self: Arc<Self>, share_id: Option<String>) -> Result<(), BridgeError> {
        let runtime = Arc::clone(&self.runtime);
        spawned(&runtime, async move { self.crawl_inner(share_id).await })
            .await
            .map_err(BridgeError::from_display)?
    }
}

impl AndroidEngine {
    /// One enrichment pass over the whole library.
    ///
    /// Two kinds of work, and the difference is a request saved: a title that
    /// already has a good match but no episode list needs the episode request
    /// only. Both run fanned out under a semaphore, and a provider error counts
    /// against the run rather than ending it — one rate-limited title must not
    /// leave the other thirty unenriched. This mirrors the desktop engine's
    /// `run_match`, which is the reference for the behaviour.
    async fn match_titles_inner(&self, force: bool) -> Result<MatchSummary, BridgeError> {
        let service = Arc::new(self.metadata_service()?);
        let provider = service.provider();
        let (titles, stored) = self.titles_and_metadata()?;
        let listed = self
            .catalog
            .lock()
            .episode_metadata_ages()
            .map_err(BridgeError::from_display)?;

        let mut pending: Vec<Work> = Vec::new();
        for title in titles {
            let record = stored.get(&title.key);
            // A hand-picked entry is never searched for again, not even by
            // "match again" — that button means "the automatic answers are
            // wrong", and re-deciding the one title the viewer already fixed by
            // hand is the opposite of what they asked for. Its episode list is
            // still fetched below if it is missing.
            let pinned = record.is_some_and(|record| record.manual && record.provider == provider);
            if !pinned && (force || !pstr_meta::service::is_usable(record, provider)) {
                pending.push(Work::Match(title));
                continue;
            }
            let has_episodes = listed
                .get(&title.key)
                .is_some_and(|(asked, _)| *asked == provider);
            if let Some(found) = record.and_then(|record| record.metadata.clone())
                && !has_episodes
            {
                pending.push(Work::Episodes(title, Box::new(found)));
            }
        }

        let permits = Arc::new(tokio::sync::Semaphore::new(LOOKUP_CONCURRENCY));
        let mut lookups = tokio::task::JoinSet::new();
        for work in pending {
            let service = Arc::clone(&service);
            let permits = Arc::clone(&permits);
            lookups.spawn(async move {
                let _permit = permits.acquire().await.ok()?;
                Some(match work {
                    Work::Match(title) => match service.record(&title).await {
                        Ok(record) => {
                            // Only for a title that matched: there is no id to
                            // ask about otherwise.
                            let episodes = match &record.metadata {
                                Some(found) => service.title_episodes(&title, found).await,
                                None => Vec::new(),
                            };
                            Ok((record.title_key.clone(), Some(record), episodes))
                        }
                        Err(error) => Err(error.to_string()),
                    },
                    Work::Episodes(title, found) => {
                        let episodes = service.title_episodes(&title, &found).await;
                        Ok((title.key, None, episodes))
                    }
                })
            });
        }

        let mut summary = MatchSummary::default();
        while let Some(joined) = lookups.join_next().await {
            match joined.map_err(BridgeError::from_display)? {
                Some(Ok((title_key, record, episodes))) => {
                    if let Some(record) = &record {
                        if record.metadata.is_some() {
                            summary.matched += 1;
                        } else {
                            summary.unmatched += 1;
                        }
                        // Misses are stored on purpose; failures are not.
                        if let Err(error) = self.catalog.lock().set_metadata(record) {
                            log::warn!("store metadata for {}: {error}", record.title_key);
                        }
                    }
                    if !episodes.is_empty() {
                        summary.episodes += episodes.len() as u32;
                        let stored = self.catalog.lock().set_episode_metadata(
                            &title_key,
                            provider,
                            now(),
                            &episodes,
                        );
                        if let Err(error) = stored {
                            log::warn!("store episodes for {title_key}: {error}");
                        }
                    }
                }
                Some(Err(error)) => {
                    summary.failed += 1;
                    log::warn!("metadata lookup: {error}");
                }
                // The semaphore is never closed; this is unreachable in
                // practice and is not a failure of the title if it happens.
                None => {}
            }
        }
        Ok(summary)
    }

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

    /// Every title as a bridge record, rebuilt only when the catalog moved.
    fn all_title_records(&self) -> Result<Vec<TitleRecord>, BridgeError> {
        let catalog = self.catalog.lock();
        let writes = catalog.writes();
        if let Some(cached) = self.library_cache.lock().as_ref()
            && cached.writes == writes
        {
            return Ok(cached.titles.clone());
        }

        let files = catalog.all_files().map_err(BridgeError::from_display)?;
        let watch = catalog
            .all_watch_states()
            .map_err(BridgeError::from_display)?;
        let metadata = catalog.all_metadata().map_err(BridgeError::from_display)?;
        let guides = catalog
            .all_episode_metadata()
            .map_err(BridgeError::from_display)?;
        let offline = catalog
            .all_offline_files()
            .map_err(BridgeError::from_display)?;
        drop(catalog);

        let library = Library::build(files, &watch);
        let titles: Vec<TitleRecord> = library
            .titles
            .iter()
            .map(|title| {
                title_record(
                    title,
                    &offline,
                    metadata.get(&title.key),
                    guides.get(&title.key),
                )
            })
            .collect();
        *self.library_cache.lock() = Some(CachedLibrary {
            writes,
            titles: titles.clone(),
        });
        Ok(titles)
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

    /// The cached connection, if it still describes the current store.
    fn cached_connection(&self) -> Option<Connection> {
        let generation = self.share_generation.load(Ordering::Acquire);
        self.connection
            .lock()
            .as_ref()
            .filter(|opened| opened.generation == generation)
            .map(|opened| Connection {
                generation,
                library: Arc::clone(&opened.library),
                open_failures: opened.open_failures.clone(),
                source: opened.source.clone(),
            })
    }

    /// Retire the cached connection after a mutation of `share_id`.
    ///
    /// The cached `Connection` is dropped here rather than left to be noticed
    /// later, so a removed share's authenticated client stops being reachable
    /// at the moment the share stops existing. Every *other* share's client is
    /// kept for the next build: adding one link is not a reason to re-handshake
    /// the links that did not change.
    fn invalidate_connection(&self, share_id: &str) {
        let retired = self.connection.lock().take();
        let mut reusable = self.reusable_clients.lock();
        match retired {
            Some(retired) => *reusable = Some(retired.library.reusable_clients(share_id)),
            // Two mutations in a row: the first parked the set, and this one
            // still has to evict its own share from it.
            None => {
                if let Some(parked) = reusable.as_mut() {
                    parked.remove(share_id);
                }
            }
        }
        drop(reusable);
        self.session_refreshed.lock().remove(share_id);
        self.share_generation.fetch_add(1, Ordering::AcqRel);
    }

    async fn thumbnail_inner(
        &self,
        share_id: &str,
        volume_id: &str,
        link_id: &str,
    ) -> Result<Option<Vec<u8>>, BridgeError> {
        // Keyed by share and link rather than revision: a re-encoded file keeps
        // its link, and one stale frame of the right episode is a better answer
        // than a round trip on every render. Clearing the cache directory is
        // what invalidates it, which is also what a recrawl does not need.
        let path = self
            .dirs
            .thumbnail_cache()
            .join(format!("{share_id}-{link_id}.bin"));
        match tokio::fs::read(&path).await {
            Ok(bytes) if !bytes.is_empty() => return Ok(Some(bytes)),
            // A truncated entry is not worth reporting — fetch it again.
            Ok(_) | Err(_) => {}
        }

        // Past the cache, so this one costs the network. Take a permit before
        // any of the bandwidth playback might want.
        let _permit = self
            .thumbnails
            .acquire()
            .await
            .map_err(BridgeError::from_display)?;
        let connection = self.connection().await?;
        let Some(client) = connection.library.client(share_id) else {
            return Ok(None);
        };
        let uid = node_uid(volume_id, link_id);

        // Preview first: it is the one sized for a card. Not every file has
        // one, and the smaller thumbnail still beats a placeholder.
        let mut found = None;
        for kind in [ThumbnailType::Preview, ThumbnailType::Thumbnail] {
            match client.download_thumbnail(&uid, kind).await {
                Ok(Some(bytes)) => {
                    found = Some(bytes);
                    break;
                }
                Ok(None) => {}
                Err(error) => log::debug!("thumbnail {share_id}/{link_id} ({kind:?}): {error}"),
            }
        }
        let Some(bytes) = found else {
            return Ok(None);
        };

        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        // Renamed into place, so a kill mid-write leaves no half-poster that
        // every later launch reads and fails to decode.
        let temporary = path.with_extension("part");
        if tokio::fs::write(&temporary, &bytes).await.is_ok() {
            let _ = tokio::fs::rename(&temporary, &path).await;
        }
        Ok(Some(bytes))
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
        if let Some(opened) = self.cached_connection() {
            return Ok(opened);
        }
        let _building = self.connecting.lock().await;
        loop {
            // A concurrent builder may have finished while this one waited.
            if let Some(opened) = self.cached_connection() {
                return Ok(opened);
            }
            let generation = self.share_generation.load(Ordering::Acquire);
            let reusable = self.reusable_clients.lock().take().unwrap_or_default();

            let (library, failures) = SharedLibrary::open_all_reusing(&self.store, reusable)
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
            *self.connection.lock() = Some(Connection {
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

    /// Open a revision from the network, reauthenticating around it.
    ///
    /// Two guards, because there are two ways to lose a visitor session. Age is
    /// the common one — the app sits in the background for hours and the next
    /// episode opens against a session Proton has already retired — and a
    /// timestamp catches it before the request. Everything else (a session
    /// dropped early, a device that slept through a clock change) only shows up
    /// as a failure, so one retry behind a refresh covers it. Before this,
    /// either meant an authentication error the viewer could clear only by
    /// pulling to refresh the library.
    async fn open_from_source(
        &self,
        connection: &Connection,
        share_id: &str,
        uid: &NodeUid,
    ) -> Result<VideoStream, BridgeError> {
        let stale = self
            .session_refreshed
            .lock()
            .get(share_id)
            .is_none_or(|at| at.elapsed() >= SESSION_FRESH_FOR);
        if stale {
            self.refresh_session(connection, share_id).await;
        }
        match connection.source.open(share_id, uid).await {
            Ok(stream) => Ok(stream),
            Err(_) if !stale => {
                self.refresh_session(connection, share_id).await;
                connection
                    .source
                    .open(share_id, uid)
                    .await
                    .map_err(BridgeError::from_display)
            }
            Err(error) => Err(BridgeError::from_display(error)),
        }
    }

    /// Best-effort: a refresh that fails leaves the open to report the real
    /// problem, which is more useful than a refresh error standing in for it.
    async fn refresh_session(&self, connection: &Connection, share_id: &str) {
        if connection.library.refresh_session(share_id).await.is_ok() {
            self.session_refreshed
                .lock()
                .insert(share_id.to_owned(), std::time::Instant::now());
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
            self.session_refreshed
                .lock()
                .insert(id.clone(), std::time::Instant::now());
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
        self.open_from_source(&connection, share_id, &node_uid(volume_id, link_id))
            .await
    }

    async fn download_episode_inner(
        &self,
        share_id: &str,
        volume_id: &str,
        link_id: &str,
        observer: Box<dyn DownloadObserver>,
    ) -> Result<OfflineRecord, BridgeError> {
        // A download that is already complete must not need the network to say
        // so. Opening a stream first would make re-running a finished worker —
        // WorkManager does, after a reboot or a constraint flap — fail on a
        // plane with an episode sitting whole on disk.
        if let Some(record) = self
            .completed_offline_download(share_id, link_id, observer.as_ref())
            .await?
        {
            return Ok(record);
        }

        let connection = self.connection().await?;
        let stream = self
            .open_from_source(&connection, share_id, &node_uid(volume_id, link_id))
            .await?;
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
            report(observer.as_ref(), total, total);
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
        report(observer.as_ref(), offset, total);

        use tokio::io::AsyncWriteExt;
        while block_index < block_sizes.len() {
            if cancelled(observer.as_ref()) {
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
            report(observer.as_ref(), offset, total);
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

    /// The record for a download whose bytes are already whole on disk, or
    /// `None` when there is real work to do. Republishes the catalog row and
    /// clears any stale partial, so a retried worker converges without a fetch.
    async fn completed_offline_download(
        &self,
        share_id: &str,
        link_id: &str,
        observer: &dyn DownloadObserver,
    ) -> Result<Option<OfflineRecord>, BridgeError> {
        let Some(file) = self
            .catalog
            .lock()
            .offline_file(share_id, link_id)
            .map_err(BridgeError::from_display)?
        else {
            return Ok(None);
        };
        let path = self.dirs.offline_file(share_id, link_id, &file.revision_id);
        let total: u64 = file.block_sizes.iter().sum();
        if !tokio::fs::metadata(&path)
            .await
            .is_ok_and(|metadata| metadata.len() == total)
        {
            return Ok(None);
        }
        let (partial, marker) = self.partial_paths(share_id, link_id);
        remove_file_if_present(partial).await?;
        remove_file_if_present(marker).await?;
        report(observer, total, total);
        Ok(Some(OfflineRecord {
            share_id: share_id.to_owned(),
            link_id: link_id.to_owned(),
            revision_id: file.revision_id,
            size: total,
            episode: None,
        }))
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

/// Bytes held below a directory, optionally counting one extension only.
///
/// Unreadable entries are skipped rather than failing the walk: this answers a
/// line in a settings screen, and a number that is short by one file is worth
/// more than an error where a number should be.
/// Delete every file under `root`, keeping the directory itself.
///
/// Best-effort per entry: a file that cannot be removed is not a reason to leave
/// the rest of the gigabytes in place.
fn sweep_directory(root: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let _ = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
    }
}

/// How deep [`directory_bytes`] will walk.
///
/// A bound rather than a policy: the cache is two levels deep, and a symlink
/// cycle under it would otherwise be an uncatchable stack overflow reached from
/// the settings screen.
const MAX_WALK_DEPTH: u32 = 16;

fn directory_bytes(root: &std::path::Path, extension: Option<&str>) -> u64 {
    directory_bytes_within(root, extension, MAX_WALK_DEPTH)
}

fn directory_bytes_within(root: &std::path::Path, extension: Option<&str>, depth: u32) -> u64 {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                return match depth.checked_sub(1) {
                    Some(remaining) => directory_bytes_within(&path, extension, remaining),
                    None => 0,
                };
            }
            let wanted = extension.is_none_or(|wanted| {
                path.extension()
                    .is_some_and(|found| found.eq_ignore_ascii_case(wanted))
            });
            match wanted {
                true => std::fs::metadata(&path).map(|file| file.len()).unwrap_or(0),
                false => 0,
            }
        })
        .sum()
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

/// The library conversion, valid while the catalog has not been written to.
struct CachedLibrary {
    writes: u64,
    titles: Vec<TitleRecord>,
}

/// One title's share of an enrichment pass.
enum Work {
    /// Search for it, and take its episodes if it matches.
    Match(Title),
    /// It is already matched; only the episode list is missing.
    Episodes(Title, Box<TitleMetadata>),
}

/// How many provider lookups may be in flight at once.
///
/// Deliberately small. Both providers rate-limit by client, and a phone
/// enriching a library it just crawled is competing with its own playback for
/// the same connection — the desktop engine uses the same number.
const LOOKUP_CONCURRENCY: usize = 2;

/// How many Proton thumbnails may be fetched at once.
///
/// Same number as the desktop engine. They are small and wanted on every
/// render, but they are still competing with playback for one connection.
const THUMBNAIL_CONCURRENCY: usize = 6;

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

fn appearance_record(appearance: Appearance) -> AppearanceRecord {
    AppearanceRecord {
        flavor: match appearance.flavor {
            Flavor::Proton => FlavorChoice::Proton,
            Flavor::Latte => FlavorChoice::Latte,
            Flavor::Frappe => FlavorChoice::Frappe,
            Flavor::Macchiato => FlavorChoice::Macchiato,
            Flavor::Mocha => FlavorChoice::Mocha,
        },
        accent: match appearance.accent {
            Accent::Mauve => AccentChoice::Mauve,
            Accent::Pink => AccentChoice::Pink,
            Accent::Sky => AccentChoice::Sky,
            Accent::PinkSky => AccentChoice::PinkSky,
            Accent::Lavender => AccentChoice::Lavender,
            Accent::Blue => AccentChoice::Blue,
            Accent::Teal => AccentChoice::Teal,
            Accent::Peach => AccentChoice::Peach,
        },
        gradients: appearance.gradients,
    }
}

fn appearance_choice(record: AppearanceRecord) -> Appearance {
    Appearance {
        flavor: match record.flavor {
            FlavorChoice::Proton => Flavor::Proton,
            FlavorChoice::Latte => Flavor::Latte,
            FlavorChoice::Frappe => Flavor::Frappe,
            FlavorChoice::Macchiato => Flavor::Macchiato,
            FlavorChoice::Mocha => Flavor::Mocha,
        },
        accent: match record.accent {
            AccentChoice::Mauve => Accent::Mauve,
            AccentChoice::Pink => Accent::Pink,
            AccentChoice::Sky => Accent::Sky,
            AccentChoice::PinkSky => Accent::PinkSky,
            AccentChoice::Lavender => Accent::Lavender,
            AccentChoice::Blue => Accent::Blue,
            AccentChoice::Teal => Accent::Teal,
            AccentChoice::Peach => Accent::Peach,
        },
        gradients: record.gradients,
    }
}

fn palette_record(record: &AppearanceRecord) -> PaletteRecord {
    let palette = Palette::resolve(appearance_choice(record.clone()));
    PaletteRecord {
        background: palette.background.argb(),
        surface: palette.surface.argb(),
        sunken: palette.sunken.argb(),
        card: palette.card.argb(),
        card_hover: palette.card_hover.argb(),
        border: palette.border.argb(),
        text: palette.text.argb(),
        muted: palette.muted.argb(),
        accent: palette.accent.argb(),
        accent_alt: palette.accent_alt.argb(),
        accent_dim: palette.accent_dim.argb(),
        on_accent: palette.on_accent.argb(),
        danger: palette.danger.argb(),
        light: palette.light,
    }
}

fn playback_prefs_record(prefs: PlaybackPrefs) -> PlaybackPrefsRecord {
    PlaybackPrefsRecord {
        volume: prefs.volume,
        muted: prefs.muted,
        audio_language: prefs.audio_language,
        subtitle_language: prefs.subtitle_language,
        subtitles: prefs.subtitles,
        autoplay_next: prefs.autoplay_next,
        auto_skip: prefs.auto_skip,
        speed: prefs.speed,
    }
}

fn playback_prefs(record: PlaybackPrefsRecord) -> PlaybackPrefs {
    PlaybackPrefs {
        volume: record.volume,
        muted: record.muted,
        audio_language: normalized_language(record.audio_language),
        subtitle_language: normalized_language(record.subtitle_language),
        subtitles: record.subtitles,
        autoplay_next: record.autoplay_next,
        auto_skip: record.auto_skip,
        speed: record.speed,
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
    guide: Option<&EpisodeGuide>,
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
                    .map(|episode| episode_record(episode, offline, guide))
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
    guide: Option<&EpisodeGuide>,
) -> EpisodeRecord {
    let node = &episode.node;
    // Matched on the numbering the *filename* states, which is what
    // `EpisodeGuide::get` is careful about — see its documentation for why the
    // absolute-numbering fallback stops at season one.
    let named = node
        .parsed
        .episode
        .and_then(|number| guide?.get(node.parsed.season, number));
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
        provider_name: named.and_then(|found| found.name.clone()),
        provider_overview: named.and_then(|found| found.overview.clone()),
        still_url: named.and_then(|found| found.still_url.clone()),
        air_date: named.and_then(|found| found.air_date.clone()),
        last_played: episode.last_played(),
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
        AndroidEngine, AndroidPaths, AndroidSecretStore, AndroidStream, BridgeError, ChapterKind,
        ChapterRecord, PartialMarker, chapter_plan, match_record, node_uid, normalized_language,
        prepare_partial_file, pstr_android_stream_read, pstr_android_stream_release,
        pstr_android_stream_size, read_partial_marker, resume_position, skip_offer, title_metadata,
        write_partial_marker,
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

    fn chapter(index: i64, title: &str, start: f64) -> ChapterRecord {
        ChapterRecord {
            index,
            title: Some(title.to_owned()),
            start,
        }
    }

    /// The bridge must reach the desktop's verdict, not a looser one: the whole
    /// point of routing this through `pstr_core::chapters` is that an opening
    /// called `Intro` is an opening on both clients or on neither.
    #[test]
    fn a_chapter_plan_resolves_openings_the_way_the_desktop_player_does() {
        let plan = chapter_plan(
            vec![
                chapter(0, "Intro", 0.0),
                chapter(1, "Part A", 700.0),
                chapter(2, "Part B", 2400.0),
                chapter(3, "Cast", 4680.0),
            ],
            Some(4800.0),
        );

        // Eleven minutes called `Intro` is the story, not a theme song.
        assert_eq!(plan.entries[0].kind, ChapterKind::Content);
        assert_eq!(plan.entries[3].kind, ChapterKind::Ending);
        assert_eq!(plan.entries[0].end, Some(700.0));
        assert_eq!(plan.credits_start, Some(4680.0));
    }

    #[test]
    fn an_unnamed_chapter_is_still_labelled_and_a_missing_duration_ends_nowhere() {
        let plan = chapter_plan(
            vec![
                ChapterRecord {
                    index: 0,
                    title: Some("   ".to_owned()),
                    start: 0.0,
                },
                chapter(1, "OP", 24.0),
            ],
            None,
        );

        assert_eq!(plan.entries[0].label, "Chapter 1");
        assert_eq!(plan.entries[0].end, Some(24.0));
        // Nowhere to skip to: the file's length is not known yet.
        assert_eq!(plan.entries[1].end, None);
        assert_eq!(skip_offer(&plan, 30.0), None);
    }

    /// A skip is offered inside the opening and nowhere else — including at its
    /// very last second, where seeking would land the viewer where they already
    /// are, and after it, where the button would never go away.
    #[test]
    fn a_skip_is_offered_only_while_the_thing_to_skip_is_still_ahead() {
        let plan = chapter_plan(
            vec![
                chapter(0, "Part A", 0.0),
                chapter(1, "OP", 90.0),
                chapter(2, "Part B", 180.0),
                chapter(3, "ED", 1320.0),
            ],
            Some(1440.0),
        );

        assert_eq!(skip_offer(&plan, 10.0), None, "content is not skippable");
        let offer = skip_offer(&plan, 100.0).expect("inside the opening");
        assert_eq!(offer.label, "Skip opening");
        assert_eq!(offer.target, 180.0);
        assert_eq!(skip_offer(&plan, 180.0).map(|offer| offer.label), None);
        assert_eq!(
            skip_offer(&plan, 1400.0).map(|offer| offer.label),
            Some("Skip ending".to_owned()),
        );
        // The last chapter ends at the duration, and nothing is offered there.
        assert_eq!(skip_offer(&plan, 1440.0), None);
    }

    #[test]
    fn storage_counts_partial_downloads_separately_from_finished_ones() {
        let root = std::env::temp_dir().join(format!(
            "pstr-android-storage-{}-{}",
            std::process::id(),
            super::now()
        ));
        let nested = root.join("share");
        std::fs::create_dir_all(&nested).expect("storage directory");
        std::fs::write(nested.join("episode.part"), b"partial").expect("partial");
        std::fs::write(nested.join("episode"), b"whole file").expect("whole");

        assert_eq!(super::directory_bytes(&root, Some("part")), 7);
        assert_eq!(super::directory_bytes(&root, None), 17);
        assert_eq!(super::directory_bytes(&root.join("missing"), None), 0);
        std::fs::remove_dir_all(root).expect("remove storage directory");
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
            cancel: tokio::sync::Notify::new(),
            native_id: Mutex::new(None),
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
