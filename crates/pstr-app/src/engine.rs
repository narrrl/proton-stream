//! Everything the UI is not allowed to do on its own thread.
//!
//! egui redraws at the display's rate; opening a share takes seconds, a crawl
//! takes minutes, and a thumbnail is a network round-trip plus a PGP decrypt.
//! So the UI holds an [`Engine`], calls a method that returns immediately, and
//! learns what happened from an [`Event`] on a channel. Nothing here blocks, and
//! nothing here draws.
//!
//! ```text
//!   ui thread                    tokio runtime                worker threads
//!   ─────────                    ─────────────                ──────────────
//!   engine.crawl()  ──spawn──▶   SharedLibrary::crawl  ──▶  block decrypt
//!        ▲                              │
//!        └──── Event::LibraryLoaded ◀────┘   + ctx.request_repaint()
//! ```
//!
//! The repaint call is the part that is easy to forget: without it an event
//! lands in the channel and sits there until the viewer happens to move the
//! mouse.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use parking_lot::Mutex;
use pstr_core::appearance::Appearance;
use pstr_core::catalog::{
    Catalog, CatalogNode, OfflineFile, TitleTrackPrefs, WatchState, build_rows,
};
use pstr_core::config::AppDirs;
use pstr_core::library::{Library, Title, TitleKind};
use pstr_core::metadata::{EpisodeGuide, MetadataConfig, MetadataRecord, ProviderId};
use pstr_core::prefs::PlaybackPrefs;
use pstr_core::proton_drive_rs::ThumbnailType;
use pstr_core::proton_sdk::ids::{LinkId, NodeUid, VolumeId};
use pstr_core::{Share, ShareStore, SharedLibrary};
use pstr_meta::MetadataService;
use pstr_stream::{
    BlockSource, DiskCacheConfig, FileBlocks, LibraryOpener, StreamConfig, StreamSource,
    VideoStream,
};
use tokio::runtime::Runtime;

use crate::playback::PlaybackTarget;

/// Stable identity for an episode download. No share secret is included.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct DownloadKey {
    pub share_id: String,
    pub link_id: String,
}

impl From<&PlaybackTarget> for DownloadKey {
    fn from(target: &PlaybackTarget) -> Self {
        Self {
            share_id: target.share_id.clone(),
            link_id: target.link_id.clone(),
        }
    }
}

/// A download state suitable for presenting directly to the viewer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadState {
    Queued,
    Running,
    Paused,
    Completed,
    Failed(String),
    /// Work stopped, but its complete-block partial is retained for Resume.
    Cancelled,
}

/// Current, inspectable state for one episode in a download batch.
#[derive(Debug, Clone)]
pub struct DownloadItem {
    pub key: DownloadKey,
    pub target: PlaybackTarget,
    pub state: DownloadState,
    pub downloaded: u64,
    pub total: u64,
}

impl DownloadItem {
    pub fn percent(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.downloaded as f64 / self.total as f64).clamp(0.0, 1.0) as f32
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DownloadCommand {
    Run,
    Pause,
    Cancel,
}

struct DownloadControl {
    command: tokio::sync::watch::Sender<DownloadCommand>,
}

impl DownloadControl {
    fn new() -> Self {
        let (command, _) = tokio::sync::watch::channel(DownloadCommand::Run);
        Self { command }
    }

    fn set(&self, command: DownloadCommand) {
        self.command.send_replace(command);
    }

    fn subscribe(&self) -> tokio::sync::watch::Receiver<DownloadCommand> {
        self.command.subscribe()
    }
}

struct DownloadJob {
    control: Arc<DownloadControl>,
    handle: tokio::task::JoinHandle<()>,
    live: Arc<AtomicBool>,
}

#[derive(Clone, Copy)]
enum DownloadEnd {
    Completed,
    Cancelled,
}

#[derive(Default)]
struct Downloads {
    items: HashMap<DownloadKey, DownloadItem>,
    jobs: HashMap<DownloadKey, DownloadJob>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PartialMarker {
    revision_id: String,
    block_sizes: Vec<u64>,
    #[serde(default)]
    target: Option<PlaybackTarget>,
}

/// Poster thumbnails are decoded to at most this edge before they become a
/// texture. Proton's preview is larger than any tile this UI draws, and the
/// difference is VRAM held for every title in the library.
const THUMBNAIL_MAX_EDGE: u32 = 640;

/// What the background side tells the UI.
pub enum Event {
    /// The share list changed on disk.
    Shares(Vec<Share>),
    /// Shares opened; playback is possible from here.
    Connected {
        /// Shares that did not open, with why.
        failures: Vec<(String, String)>,
    },
    /// Shares could not be opened at all.
    ConnectFailed(String),
    /// The catalog was re-read.
    LibraryLoaded(Library),
    /// A crawl finished for one share.
    Crawled {
        share_id: String,
        nodes: usize,
        files: usize,
        seconds: f64,
    },
    /// Every requested crawl is done.
    CrawlFinished,
    /// A decoded Proton thumbnail, keyed as [`thumbnail_key`].
    Thumbnail {
        key: String,
        image: egui::ColorImage,
    },
    /// No thumbnail for this file — remembered, so the grid stops asking.
    ThumbnailMissing { key: String },
    /// Everything the catalog knows about titles, keyed by title key.
    Metadata(HashMap<String, MetadataRecord>),
    /// What the provider said about individual episodes, keyed by title key.
    EpisodeMetadata(HashMap<String, EpisodeGuide>),
    /// The enrichment settings changed, on disk.
    MetadataConfig(MetadataConfig),
    /// A matching run finished, with how it went.
    Matched {
        matched: usize,
        unmatched: usize,
        failed: usize,
    },
    /// Every requested match is done.
    MatchFinished,
    /// What the provider answered a hand search with, unscored and in its own
    /// order. `title_key` says which title asked, because the viewer can close
    /// the search and open another one while a request is still out.
    MatchOptions {
        title_key: String,
        options: Vec<pstr_core::metadata::TitleMetadata>,
    },
    /// A hand search did not get an answer. Distinct from an empty one: nothing
    /// found is a result, and a rate limit is not.
    MatchSearchFailed { title_key: String, error: String },
    /// A decoded piece of provider artwork, keyed by title key.
    Poster {
        key: String,
        image: egui::ColorImage,
    },
    /// The artwork for this title could not be had.
    PosterMissing { key: String },
    /// A stream opened and is ready to hand to mpv.
    PlaybackReady {
        target: Box<PlaybackTarget>,
        stream: VideoStream,
    },
    /// mpv said something about what is playing. `id` is the player it came
    /// from — a player being replaced goes on talking for a moment after the
    /// next one has started.
    Player {
        id: u64,
        event: pstr_player::PlayerEvent,
    },
    /// That player is gone: its window was closed, or playback ended.
    PlayerStopped { id: u64 },
    /// Something the viewer should see, phrased for them.
    Error(String),
    /// Something the viewer might like to see, briefly.
    Status(String),
    /// Full download-manager snapshot. Replacing rather than patching avoids
    /// stale rows when a task is removed.
    Downloads(Vec<DownloadItem>),
    /// Completed local copies, for badges and online-only actions.
    OfflineFiles(HashSet<DownloadKey>),
}

/// The share clients and the block layer, once they exist.
struct Connection {
    library: Arc<SharedLibrary>,
    source: StreamSource,
}

/// The background half of the app.
///
/// Cheap to clone: everything inside is shared. The UI keeps one, the tasks it
/// spawns keep clones.
#[derive(Clone)]
pub struct Engine {
    runtime: Arc<Runtime>,
    dirs: AppDirs,
    store: Arc<ShareStore>,
    /// One connection, guarded rather than owned by the UI, because a task that
    /// finishes long after the click that started it still needs it.
    connection: Arc<Mutex<Option<Connection>>>,
    /// Held across the handshake, which a `parking_lot` guard cannot be.
    ///
    /// Without it the first frame is a stampede: `connect`, the click that
    /// wanted a stream and every visible poster all find no connection and open
    /// every share at once. One winner, everyone else waits and takes its
    /// result.
    connecting: Arc<tokio::sync::Mutex<()>>,
    /// `rusqlite::Connection` is `Send` but not `Sync`, and both the UI (watch
    /// state) and crawls (rows) write. One lock, held for the length of a
    /// statement.
    catalog: Arc<Mutex<Catalog>>,
    /// Thumbnail fetches in flight. A fast scroll can ask for a hundred, and
    /// each is a round-trip plus a decrypt — unbounded, they crowd out the reads
    /// the player is blocked on.
    thumbnails: Arc<tokio::sync::Semaphore>,
    /// Provider lookups in flight. Far tighter than the thumbnail limit, and
    /// not for our benefit: both providers rate-limit by the minute, and a
    /// library of three hundred titles fired off at once earns a 429 for the
    /// whole run rather than for the tail of it.
    lookups: Arc<tokio::sync::Semaphore>,
    /// The enrichment settings and the provider they resolve to. Rebuilt
    /// whenever the settings change, so nothing holds a client for a provider
    /// the viewer has switched away from.
    metadata: Arc<Mutex<Enrichment>>,
    /// Volume, mute and language preferences. Held here rather than in the UI
    /// because a new player is built from them, and the player is started from
    /// a background event.
    prefs: Arc<Mutex<PlaybackPrefs>>,
    /// Which theme the window wears. Kept beside the playback preferences for
    /// the same reason: it is read from the UI and written to disk off it.
    appearance: Arc<Mutex<Appearance>>,
    /// Preloaded so clicking Play never runs SQLite on egui's thread.
    track_prefs: Arc<Mutex<HashMap<String, TitleTrackPrefs>>>,
    downloads: Arc<Mutex<Downloads>>,
    download_permits: Arc<tokio::sync::Semaphore>,
    events: Sender<Event>,
    ctx: egui::Context,
}

/// How many posters may be in flight at once.
const THUMBNAIL_CONCURRENCY: usize = 6;

/// How many provider lookups may be in flight at once. See [`Engine::lookups`].
const LOOKUP_CONCURRENCY: usize = 2;
const DOWNLOAD_CONCURRENCY: usize = 3;

/// Enrichment as currently configured.
struct Enrichment {
    config: MetadataConfig,
    /// `None` when enrichment is off, or when the provider needs an API key
    /// that is not stored. Both are ordinary states, not errors: the difference
    /// only matters when the viewer asks for a match, which is where it is
    /// reported.
    service: Option<MetadataService>,
}

impl Enrichment {
    /// Resolve a service for `config`, if one can be had.
    fn build(config: MetadataConfig) -> Self {
        let service = config.enabled.then(|| {
            let key = pstr_meta::settings::api_key(config.provider);
            MetadataService::new(&config, key)
                .inspect_err(|error| tracing::debug!("metadata provider unavailable: {error}"))
                .ok()
        });

        Self {
            config,
            service: service.flatten(),
        }
    }
}

impl Engine {
    /// Build the engine and the channel the UI drains.
    pub fn new(
        runtime: Arc<Runtime>,
        dirs: AppDirs,
        ctx: egui::Context,
    ) -> anyhow::Result<(Self, Receiver<Event>)> {
        let catalog = Catalog::open(&dirs.catalog_db())?;
        let (events, receiver) = channel();

        // An unreadable settings file must not stop the app starting: the
        // library is the point and enrichment is decoration. It is reported and
        // treated as "off", which is also the safe reading — never as "on".
        let config = pstr_meta::settings::load(&dirs).unwrap_or_else(|error| {
            tracing::warn!("read the metadata settings: {error}");
            MetadataConfig::default()
        });

        // Same reading as the metadata settings above: a preferences file that
        // will not parse is worth a line in the log and nothing more. It is
        // volume and a language, and the defaults play the film.
        let prefs = pstr_core::prefs::load(&dirs).unwrap_or_else(|error| {
            tracing::warn!("read the playback preferences: {error}");
            PlaybackPrefs::default()
        });

        // And the same again for the theme, which is the least load-bearing
        // file of the three: an unreadable one costs the viewer their colours
        // until they pick them again.
        let appearance = pstr_core::appearance::load(&dirs).unwrap_or_else(|error| {
            tracing::warn!("read the appearance: {error}");
            Appearance::default()
        });

        let track_prefs = catalog.all_title_track_prefs().unwrap_or_else(|error| {
            tracing::warn!("read show track preferences: {error}");
            HashMap::new()
        });
        let engine = Self {
            runtime,
            store: Arc::new(ShareStore::new(dirs.clone())),
            dirs,
            connection: Arc::new(Mutex::new(None)),
            connecting: Arc::new(tokio::sync::Mutex::new(())),
            catalog: Arc::new(Mutex::new(catalog)),
            thumbnails: Arc::new(tokio::sync::Semaphore::new(THUMBNAIL_CONCURRENCY)),
            lookups: Arc::new(tokio::sync::Semaphore::new(LOOKUP_CONCURRENCY)),
            metadata: Arc::new(Mutex::new(Enrichment::build(config))),
            prefs: Arc::new(Mutex::new(prefs)),
            appearance: Arc::new(Mutex::new(appearance)),
            track_prefs: Arc::new(Mutex::new(track_prefs)),
            downloads: Arc::new(Mutex::new(Downloads::default())),
            download_permits: Arc::new(tokio::sync::Semaphore::new(DOWNLOAD_CONCURRENCY)),
            events,
            ctx,
        };
        Ok((engine, receiver))
    }

    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.runtime
    }

    /// The playback preferences as they stand.
    pub fn playback_prefs(&self) -> PlaybackPrefs {
        self.prefs.lock().clone()
    }

    /// Change the preferences, and write them if asked.
    ///
    /// `commit` is what keeps a dragged volume slider from being a file write
    /// per frame: every change is kept in memory, so a player started mid-drag
    /// is built from the current value, and the disk is touched when the drag
    /// ends. Losing the last few points of a drag to a crash costs nothing.
    pub fn set_playback_prefs(&self, prefs: PlaybackPrefs, commit: bool) {
        let prefs = prefs.sanitized();
        *self.prefs.lock() = prefs.clone();
        if !commit {
            return;
        }
        let engine = self.clone();
        self.runtime.spawn_blocking(move || {
            if let Err(error) = pstr_core::prefs::save(&engine.dirs, &prefs) {
                // Not `fail`: the viewer asked to change the volume, not to
                // save a file, and the change did happen. Telling them their
                // config directory is unwritable in a red line under the film
                // helps nobody mid-episode.
                tracing::warn!("save the playback preferences: {error}");
            }
        });
    }

    /// Which theme the window wears.
    pub fn appearance(&self) -> Appearance {
        *self.appearance.lock()
    }

    /// Change the theme and write it.
    ///
    /// Always committed, unlike the volume: a theme is picked with a click and
    /// there is no drag to coalesce.
    pub fn set_appearance(&self, appearance: Appearance) {
        *self.appearance.lock() = appearance;
        let engine = self.clone();
        self.runtime.spawn_blocking(move || {
            if let Err(error) = pstr_core::appearance::save(&engine.dirs, &appearance) {
                // Not `fail`, for the same reason the playback preferences are
                // not: the change happened, and only the remembering did not.
                tracing::warn!("save the appearance: {error}");
            }
        });
    }

    /// Send an event and wake the UI. A closed channel means the window is
    /// gone, which is not an error worth propagating anywhere.
    pub(crate) fn emit(&self, event: Event) {
        if self.events.send(event).is_ok() {
            self.ctx.request_repaint();
        }
    }

    fn fail(&self, context: &str, error: impl std::fmt::Display) {
        tracing::warn!("{context}: {error}");
        self.emit(Event::Error(format!("{context}: {error}")));
    }

    // ---------------------------------------------------------------- shares

    /// Re-read the share list.
    pub fn load_shares(&self) {
        match self.store.list() {
            Ok(shares) => self.emit(Event::Shares(shares)),
            Err(error) => self.fail("read the share list", error),
        }
    }

    /// Add a share, then open and crawl it — which is what "add" means to
    /// someone who just pasted a link.
    pub fn add_share(&self, name: String, url: String, password: Option<String>) {
        let engine = self.clone();
        self.runtime.spawn(async move {
            let share = match engine.store.add(&name, &url, password.as_deref()) {
                Ok(share) => share,
                Err(error) => return engine.fail("add that share", error),
            };
            engine.emit(Event::Status(format!("added {}", share.name)));
            engine.load_shares();
            engine.connect_and_crawl(Some(share.id)).await;
        });
    }

    /// Forget a share and its stored secrets.
    pub fn remove_share(&self, id: String) {
        let engine = self.clone();
        self.runtime.spawn(async move {
            let keys: Vec<_> = engine
                .downloads
                .lock()
                .jobs
                .keys()
                .filter(|key| key.share_id == id)
                .cloned()
                .collect();
            for key in &keys {
                engine.cancel_and_join(key).await;
            }

            let offline = match engine.catalog.lock().offline_files_for_share(&id) {
                Ok(files) => files,
                Err(error) => return engine.fail("inspect that share's downloads", error),
            };
            for (link_id, file) in &offline {
                let path = engine.dirs.offline_file(&id, link_id, &file.revision_id);
                if let Err(error) = remove_if_present(&path).await {
                    return engine.fail("delete that share's offline bytes", error);
                }
            }
            let item_keys: Vec<_> = engine
                .downloads
                .lock()
                .items
                .keys()
                .filter(|key| key.share_id == id)
                .cloned()
                .collect();
            for key in &item_keys {
                let (partial, marker) = engine.partial_paths(key);
                if let Err(error) = remove_if_present(&partial).await {
                    return engine.fail("delete that share's partial download", error);
                }
                if let Err(error) = remove_if_present(&marker).await {
                    return engine.fail("delete that share's partial marker", error);
                }
            }
            if let Err(error) = engine.catalog.lock().remove_share(&id) {
                return engine.fail("drop that share's catalog rows", error);
            }
            if let Err(error) = engine.store.remove(&id) {
                return engine.fail("remove that share", error);
            }
            {
                let mut downloads = engine.downloads.lock();
                downloads.items.retain(|key, _| key.share_id != id);
                downloads.jobs.retain(|key, _| key.share_id != id);
            }
            engine.emit_downloads();
            engine.load_shares();
            // Its clients are stale now; the next action reopens what is left.
            engine.connection.lock().take();
            engine.load_library();
            engine.emit(Event::Status("removed".into()));
        });
    }

    // ------------------------------------------------------------ connection

    /// The open connection, if there is one. Never held across an await — the
    /// guard is `parking_lot`'s.
    fn opened(&self) -> Option<(Arc<SharedLibrary>, StreamSource)> {
        let connection = self.connection.lock();
        let connection = connection.as_ref()?;
        Some((Arc::clone(&connection.library), connection.source.clone()))
    }

    /// Open every configured share and build the block layer.
    ///
    /// Idempotent: a second call while one connection exists is a no-op, so the
    /// UI can call it whenever it needs the network without tracking whether it
    /// already has it.
    pub fn connect(&self) {
        if self.opened().is_some() {
            return;
        }
        let engine = self.clone();
        self.runtime.spawn(async move {
            engine.ensure_connected().await;
        });
    }

    /// Open the shares if they are not open. Returns the pieces playback needs.
    async fn ensure_connected(&self) -> Option<(Arc<SharedLibrary>, StreamSource)> {
        if let Some(opened) = self.opened() {
            return Some(opened);
        }

        // One handshake at a time, and re-check after waiting: whoever was
        // ahead in the queue has almost certainly just done it.
        let _connecting = self.connecting.lock().await;
        if let Some(opened) = self.opened() {
            return Some(opened);
        }

        let (library, failures) = match SharedLibrary::open_all(&self.store).await {
            Ok(opened) => opened,
            Err(error) => {
                self.emit(Event::ConnectFailed(error.to_string()));
                return None;
            }
        };
        let library = Arc::new(library);

        let opener = Arc::new(LibraryOpener::new(Arc::clone(&library)));
        let config = StreamConfig::default()
            // mpv brings its own read-ahead; a second one under it only competes
            // for the bandwidth mpv is blocked on. See `pstr_player::READAHEAD_BLOCKS`.
            .with_readahead(pstr_player::READAHEAD_BLOCKS)
            .with_disk_cache(DiskCacheConfig::new(self.dirs.block_cache()));

        let source = match StreamSource::new(opener, config).await {
            Ok(source) => source,
            Err(error) => {
                self.emit(Event::ConnectFailed(error.to_string()));
                return None;
            }
        };

        *self.connection.lock() = Some(Connection {
            library: Arc::clone(&library),
            source: source.clone(),
        });

        self.emit(Event::Connected {
            failures: failures
                .into_iter()
                .map(|(share, error)| (share.name, error.to_string()))
                .collect(),
        });
        Some((library, source))
    }

    // ------------------------------------------------------------- catalogue

    /// Re-read the catalog into a [`Library`].
    pub fn load_library(&self) {
        let engine = self.clone();
        self.runtime.spawn_blocking(move || {
            let result = {
                let catalog = engine.catalog.lock();
                catalog
                    .all_files()
                    .and_then(|files| Ok((files, catalog.all_watch_states()?)))
            };
            match result {
                Ok((files, watch)) => {
                    engine.emit(Event::LibraryLoaded(Library::build(files, &watch)));
                    engine.load_offline_files();
                }
                Err(error) => engine.fail("read the catalog", error),
            }
        });
    }

    /// Refresh the lightweight offline index used by title badges and actions.
    pub fn load_offline_files(&self) {
        let engine = self.clone();
        self.runtime.spawn_blocking(move || {
            let result = (|| {
                let catalog = engine.catalog.lock();
                let files = catalog.all_offline_files()?;
                let library = Library::build(catalog.all_files()?, &catalog.all_watch_states()?);
                let mut targets = HashMap::new();
                for title in &library.titles {
                    for episode in title.episodes() {
                        let target = PlaybackTarget::new(title, episode);
                        targets.insert(DownloadKey::from(&target), target);
                    }
                }
                let mut valid = HashSet::new();
                let mut hydrated = Vec::new();
                for ((share_id, link_id), file) in files {
                    let key = DownloadKey { share_id, link_id };
                    let path =
                        engine
                            .dirs
                            .offline_file(&key.share_id, &key.link_id, &file.revision_id);
                    let Some(expected) = file
                        .block_sizes
                        .iter()
                        .try_fold(0_u64, |sum, size| sum.checked_add(*size))
                    else {
                        catalog.remove_offline_file(&key.share_id, &key.link_id)?;
                        continue;
                    };
                    if std::fs::metadata(path).is_ok_and(|metadata| metadata.len() == expected) {
                        valid.insert(key.clone());
                        if let Some(target) = targets.get(&key).cloned() {
                            hydrated.push(DownloadItem {
                                key,
                                target,
                                state: DownloadState::Completed,
                                downloaded: expected,
                                total: expected,
                            });
                        }
                    } else {
                        // The catalog is an index, not the bytes. A manually
                        // removed or truncated file must stop wearing an
                        // offline badge and remain playable from the share.
                        catalog.remove_offline_file(&key.share_id, &key.link_id)?;
                    }
                }
                Ok::<_, pstr_core::Error>((valid, hydrated))
            })();
            match result {
                Ok((files, hydrated)) => {
                    {
                        let mut downloads = engine.downloads.lock();
                        for item in hydrated {
                            downloads.items.entry(item.key.clone()).or_insert(item);
                        }
                    }
                    engine.hydrate_partial_downloads();
                    engine.emit_downloads();
                    engine.emit(Event::OfflineFiles(files));
                }
                Err(error) => engine.fail("read offline downloads", error),
            }
        });
    }

    /// Crawl one share, or all of them, and reload the library.
    pub fn crawl(&self, share: Option<String>) {
        let engine = self.clone();
        self.runtime.spawn(async move {
            engine.connect_and_crawl(share).await;
        });
    }

    async fn connect_and_crawl(&self, share: Option<String>) {
        let Some((library, _)) = self.ensure_connected().await else {
            self.emit(Event::CrawlFinished);
            return;
        };

        let targets: Vec<String> = match share {
            Some(id) => vec![id],
            None => library.share_ids().map(str::to_string).collect(),
        };

        for share_id in targets {
            let started = std::time::Instant::now();
            let nodes = match library.crawl(&share_id).await {
                Ok(nodes) => nodes,
                Err(error) => {
                    self.fail(&format!("crawl {share_id}"), error);
                    continue;
                }
            };
            let rows = build_rows(&share_id, &nodes);
            let files = rows.len();

            // A writer opened before the crawl may be producing the revision
            // this crawl is about to supersede. Quiesce all writers for the
            // share before inspecting or deleting any revision paths.
            let active_downloads: Vec<_> = self
                .downloads
                .lock()
                .jobs
                .keys()
                .filter(|key| key.share_id == share_id)
                .cloned()
                .collect();
            for key in &active_downloads {
                self.cancel_and_join(key).await;
            }

            // Paths are derived from the old revision id, so inspect and
            // remove stale bytes before changing or dropping their index rows.
            let retained = match self.catalog.lock().offline_files_for_share(&share_id) {
                Ok(files) => files,
                Err(error) => {
                    self.fail(&format!("inspect downloads for {share_id}"), error);
                    continue;
                }
            };
            let revisions: HashMap<&str, Option<&str>> = rows
                .iter()
                .map(|row| (row.link_id.as_str(), row.active_revision_id.as_deref()))
                .collect();
            let stale: Vec<_> = retained
                .iter()
                .filter(|(link_id, file)| {
                    revisions.get(link_id.as_str()).copied().flatten()
                        != Some(file.revision_id.as_str())
                })
                .map(|(link_id, file)| (link_id.clone(), file.clone()))
                .collect();
            let mut cleanup_failed = false;
            for (link_id, file) in &stale {
                let path = self
                    .dirs
                    .offline_file(&share_id, link_id, &file.revision_id);
                if let Err(error) = remove_if_present(&path).await {
                    self.fail(&format!("delete stale download for {share_id}"), error);
                    cleanup_failed = true;
                    break;
                }
            }
            if cleanup_failed {
                continue;
            }

            let stored = {
                let mut catalog = self.catalog.lock();
                catalog
                    .replace_share_retaining_offline(&share_id, &rows)
                    .and_then(|()| {
                        for (link_id, _) in &stale {
                            catalog.remove_offline_file(&share_id, link_id)?;
                        }
                        Ok(())
                    })
            };
            if let Err(error) = stored {
                self.fail(&format!("store {share_id}"), error);
                continue;
            }

            self.emit(Event::Crawled {
                share_id,
                nodes: nodes.len(),
                files,
                seconds: started.elapsed().as_secs_f64(),
            });
            self.load_library();
        }
        self.emit(Event::CrawlFinished);
    }

    /// Record where playback got to.
    ///
    /// Fire and forget: a watch position that fails to save is worth a log line,
    /// not an interruption to what the viewer is watching.
    pub fn save_watch_state(&self, share_id: String, link_id: String, state: WatchState) {
        let engine = self.clone();
        self.runtime.spawn_blocking(move || {
            engine.save_watch_state_now(&share_id, &link_id, &state);
        });
    }

    /// The same write, on the calling thread.
    ///
    /// What the player thread uses for its last save: a task spawned as the
    /// window is closing may never be polled, and the position at the moment
    /// the viewer quit is exactly the one worth keeping.
    pub fn save_watch_state_now(&self, share_id: &str, link_id: &str, state: &WatchState) {
        if let Err(error) = self
            .catalog
            .lock()
            .set_watch_state(share_id, link_id, state)
        {
            tracing::warn!("save watch state for {link_id}: {error}");
        }
    }

    pub fn title_track_prefs(&self, key: &str) -> Option<TitleTrackPrefs> {
        self.track_prefs.lock().get(key).cloned()
    }
    pub fn set_title_track_prefs(&self, key: String, prefs: TitleTrackPrefs) {
        self.track_prefs.lock().insert(key.clone(), prefs.clone());
        let engine = self.clone();
        self.runtime.spawn_blocking(move || {
            if let Err(e) = engine.catalog.lock().set_title_track_prefs(&key, &prefs) {
                tracing::warn!("save show track preferences: {e}");
            }
        });
    }

    // ------------------------------------------------------------- playback

    /// Open a stream on `node` and hand it back through
    /// [`Event::PlaybackReady`].
    ///
    /// Opening is done here rather than in mpv's open callback for the reason
    /// `pstr-player` documents: opening can take seconds and fail for reasons a
    /// person needs to read, and mpv's callback can only say "loading failed".
    pub fn play(&self, target: PlaybackTarget) {
        let engine = self.clone();
        self.runtime.spawn(async move {
            // A completed local copy is authoritative until the catalog sees a
            // new revision; opening it never needs a public-link session.
            let offline = {
                engine
                    .catalog
                    .lock()
                    .offline_file(&target.share_id, &target.link_id)
                    .ok()
                    .flatten()
            };
            if let Some(file) = offline {
                let path =
                    engine
                        .dirs
                        .offline_file(&target.share_id, &target.link_id, &file.revision_id);
                if tokio::fs::metadata(&path)
                    .await
                    .map(|m| m.len() == file.block_sizes.iter().sum::<u64>())
                    .unwrap_or(false)
                {
                    let blocks: std::sync::Arc<dyn BlockSource> = std::sync::Arc::new(
                        FileBlocks::new(file.revision_id, path, file.block_sizes),
                    );
                    let uid = node_uid(&target.volume_id, &target.link_id);
                    engine.emit(Event::PlaybackReady {
                        target: Box::new(target),
                        stream: VideoStream::offline(uid, blocks, pstr_stream::DEFAULT_RING_BYTES),
                    });
                    return;
                }
            }
            let Some((_, source)) = engine.ensure_connected().await else {
                return;
            };
            let uid = node_uid(&target.volume_id, &target.link_id);
            match source.open(&target.share_id, &uid).await {
                Ok(stream) if stream.size() == 0 => {
                    engine.emit(Event::Error(format!("{} has no content", target.name)));
                }
                Ok(stream) => engine.emit(Event::PlaybackReady {
                    target: Box::new(target),
                    stream,
                }),
                Err(error) => engine.fail(&format!("open {}", target.name), error),
            }
        });
    }

    /// Queue complete plaintext copies. Jobs for the same episode are
    /// de-duplicated; a failed or cancelled row is a resumable retry.
    pub fn make_offline(&self, targets: Vec<PlaybackTarget>) {
        for target in targets {
            let key = DownloadKey::from(&target);
            if !self.job_is_live(&key) {
                self.spawn_download(target);
            }
        }
    }

    fn job_is_live(&self, key: &DownloadKey) -> bool {
        self.downloads
            .lock()
            .jobs
            .get(key)
            .is_some_and(|job| job.live.load(Ordering::Acquire))
    }

    fn spawn_download(&self, target: PlaybackTarget) {
        let key = DownloadKey::from(&target);
        let control = Arc::new(DownloadControl::new());
        let live = Arc::new(AtomicBool::new(true));
        {
            let mut downloads = self.downloads.lock();
            let previous = downloads.items.get(&key).cloned();
            downloads.items.insert(
                key.clone(),
                DownloadItem {
                    key: key.clone(),
                    target: target.clone(),
                    state: DownloadState::Queued,
                    downloaded: previous.as_ref().map_or(0, |item| item.downloaded),
                    total: previous.as_ref().map_or(0, |item| item.total),
                },
            );
        }
        self.emit_downloads();
        let engine = self.clone();
        let task_live = Arc::clone(&live);
        let task_target = target.clone();
        let task_control = Arc::clone(&control);
        let handle = self.runtime.spawn(async move {
            let result = engine.download_one(task_target.clone(), task_control).await;
            task_live.store(false, Ordering::Release);
            let task_key = DownloadKey::from(&task_target);
            match result {
                Ok(DownloadEnd::Completed) => {
                    engine.update_download(&task_key, |item| {
                        item.downloaded = item.total;
                        item.state = DownloadState::Completed;
                    });
                    engine.load_offline_files();
                    engine.emit(Event::Status(format!(
                        "{} is available offline",
                        task_target.name
                    )));
                }
                Ok(DownloadEnd::Cancelled) => {
                    engine.update_download(&task_key, |item| item.state = DownloadState::Cancelled);
                }
                Err(error) => {
                    engine.update_download(&task_key, |item| {
                        item.state = DownloadState::Failed(error.to_string());
                    });
                }
            }
        });
        self.downloads.lock().jobs.insert(
            key,
            DownloadJob {
                control,
                handle,
                live,
            },
        );
    }

    pub fn pause_download(&self, key: &DownloadKey) {
        if let Some(control) = self.live_control(key) {
            control.set(DownloadCommand::Pause);
            self.update_download(key, |item| item.state = DownloadState::Paused);
        }
    }

    pub fn resume_download(&self, key: &DownloadKey) {
        let control = self.live_control(key);
        if let Some(control) = control {
            control.set(DownloadCommand::Run);
            self.update_download(key, |item| item.state = DownloadState::Running);
            return;
        }
        if let Some(target) = self
            .downloads
            .lock()
            .items
            .get(key)
            .map(|item| item.target.clone())
        {
            self.make_offline(vec![target]);
        }
    }

    /// Stop network work after the current complete block. The partial remains
    /// valid and Resume starts at its next block.
    pub fn cancel_download(&self, key: &DownloadKey) {
        if let Some(control) = self.live_control(key) {
            control.set(DownloadCommand::Cancel);
        }
    }

    fn live_control(&self, key: &DownloadKey) -> Option<Arc<DownloadControl>> {
        self.downloads
            .lock()
            .jobs
            .get(key)
            .filter(|job| job.live.load(Ordering::Acquire))
            .map(|job| Arc::clone(&job.control))
    }

    /// Remove completed offline bytes and the catalog record. With
    /// `remove_partial`, also discard resumable work after cancelling it.
    pub fn remove_download(&self, key: DownloadKey, remove_partial: bool) {
        self.cancel_download(&key);
        let engine = self.clone();
        self.runtime.spawn(async move {
            engine.cancel_and_join(&key).await;
            let offline = {
                engine
                    .catalog
                    .lock()
                    .offline_file(&key.share_id, &key.link_id)
                    .ok()
                    .flatten()
            };
            if let Some(file) = offline {
                let path = engine
                    .dirs
                    .offline_file(&key.share_id, &key.link_id, &file.revision_id);
                if let Err(error) = remove_if_present(&path).await {
                    return engine.fail("delete offline bytes", error);
                }
            }
            if remove_partial {
                let (partial, marker) = engine.partial_paths(&key);
                if let Err(error) = remove_if_present(&partial).await {
                    return engine.fail("delete partial download", error);
                }
                if let Err(error) = remove_if_present(&marker).await {
                    return engine.fail("delete partial marker", error);
                }
            }
            if let Err(error) = engine
                .catalog
                .lock()
                .remove_offline_file(&key.share_id, &key.link_id)
            {
                return engine.fail("make online-only", error);
            }
            {
                let mut downloads = engine.downloads.lock();
                downloads.jobs.remove(&key);
                downloads.items.remove(&key);
            }
            engine.emit_downloads();
            engine.load_offline_files();
            engine.emit(Event::Status(
                "download removed; online source was kept".to_owned(),
            ));
        });
    }

    async fn cancel_and_join(&self, key: &DownloadKey) {
        let job = self.downloads.lock().jobs.remove(key);
        if let Some(job) = job {
            job.control.set(DownloadCommand::Cancel);
            if let Err(error) = job.handle.await
                && !error.is_cancelled()
            {
                tracing::warn!("join offline writer: {error}");
            }
        }
    }

    /// Ask every writer to stop at its next durable block boundary and wait a
    /// bounded time. If a network read outlives the bound, the marker and last
    /// `sync_data` boundary remain a coherent handoff for the next launch.
    pub fn shutdown_downloads(&self, timeout: std::time::Duration) {
        let jobs: Vec<_> = {
            let mut downloads = self.downloads.lock();
            downloads
                .jobs
                .drain()
                .map(|(_, job)| {
                    job.control.set(DownloadCommand::Cancel);
                    job.handle
                })
                .collect()
        };
        self.runtime.block_on(async move {
            let wait = async move {
                for handle in jobs {
                    let _ = handle.await;
                }
            };
            let _ = tokio::time::timeout(timeout, wait).await;
        });
    }

    fn emit_downloads(&self) {
        let mut items: Vec<_> = self.downloads.lock().items.values().cloned().collect();
        items.sort_by(|a, b| {
            a.target
                .title_key
                .cmp(&b.target.title_key)
                .then(a.target.season.cmp(&b.target.season))
                .then(a.target.number.cmp(&b.target.number))
        });
        self.emit(Event::Downloads(items));
    }

    fn update_download(&self, key: &DownloadKey, update: impl FnOnce(&mut DownloadItem)) {
        if let Some(item) = self.downloads.lock().items.get_mut(key) {
            update(item);
        }
        self.emit_downloads();
    }

    fn partial_paths(&self, key: &DownloadKey) -> (std::path::PathBuf, std::path::PathBuf) {
        let partial = self
            .dirs
            .offline_file(&key.share_id, &key.link_id, "partial");
        let marker = partial.with_extension("partial.json");
        (partial, marker)
    }

    fn hydrate_partial_downloads(&self) {
        let Ok(entries) = std::fs::read_dir(self.dirs.offline_content()) else {
            return;
        };
        let mut found = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".partial.json"))
            {
                continue;
            }
            let Some(marker) = std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<PartialMarker>(&bytes).ok())
            else {
                continue;
            };
            let Some(target) = marker.target else {
                continue;
            };
            let key = DownloadKey::from(&target);
            let (partial, expected_marker) = self.partial_paths(&key);
            if expected_marker != path {
                continue;
            }
            let existing = std::fs::metadata(partial).map_or(0, |metadata| metadata.len());
            let (_, downloaded) = resume_position(existing, &marker.block_sizes);
            let Some(total) = marker
                .block_sizes
                .iter()
                .try_fold(0_u64, |sum, size| sum.checked_add(*size))
            else {
                continue;
            };
            found.push(DownloadItem {
                key,
                target,
                state: DownloadState::Cancelled,
                downloaded,
                total,
            });
        }
        let mut downloads = self.downloads.lock();
        for item in found {
            downloads.items.entry(item.key.clone()).or_insert(item);
        }
    }

    async fn download_one(
        &self,
        target: PlaybackTarget,
        control: Arc<DownloadControl>,
    ) -> anyhow::Result<DownloadEnd> {
        use tokio::io::{AsyncSeekExt, AsyncWriteExt};

        let key = DownloadKey::from(&target);
        let mut command = control.subscribe();
        if !wait_until_runnable(&mut command).await {
            return Ok(DownloadEnd::Cancelled);
        }

        let _permit = loop {
            self.update_download(&key, |item| item.state = DownloadState::Queued);
            let permits = Arc::clone(&self.download_permits);
            tokio::select! {
                permit = permits.acquire_owned() => break permit?,
                changed = command.changed() => {
                    if changed.is_err() || !wait_until_runnable(&mut command).await {
                        return Ok(DownloadEnd::Cancelled);
                    }
                }
            }
        };
        self.update_download(&key, |item| item.state = DownloadState::Running);
        let Some((_, source)) = self.ensure_connected().await else {
            anyhow::bail!("share is not connected");
        };
        let stream = source
            .open(
                &target.share_id,
                &node_uid(&target.volume_id, &target.link_id),
            )
            .await?;
        let revision_id = stream.revision_id().to_owned();
        let block_sizes = stream.block_sizes().to_vec();
        let total = block_sizes
            .iter()
            .try_fold(0_u64, |sum, size| sum.checked_add(*size))
            .ok_or_else(|| anyhow::anyhow!("offline file size overflow"))?;
        self.update_download(&key, |item| item.total = total);

        let destination = self
            .dirs
            .offline_file(&target.share_id, &target.link_id, &revision_id);
        let parent = destination
            .parent()
            .ok_or_else(|| anyhow::anyhow!("offline file has no parent"))?;
        tokio::fs::create_dir_all(parent).await?;
        if tokio::fs::metadata(&destination)
            .await
            .is_ok_and(|metadata| metadata.len() == total)
        {
            let (partial, marker) = self.partial_paths(&key);
            remove_if_present(&partial).await?;
            self.finish_download(&key, &target, revision_id, block_sizes, total, &marker)
                .await?;
            return Ok(DownloadEnd::Completed);
        }

        let (partial, marker_path) = self.partial_paths(&key);
        let marker = PartialMarker {
            revision_id: revision_id.clone(),
            block_sizes: block_sizes.clone(),
            target: Some(target.clone()),
        };
        let matches = tokio::fs::read(&marker_path)
            .await
            .ok()
            .and_then(|bytes| serde_json::from_slice::<PartialMarker>(&bytes).ok())
            .is_some_and(|stored| {
                stored.revision_id == revision_id && stored.block_sizes == block_sizes
            });
        if !matches {
            remove_if_present(&partial).await?;
            // The old marker describes bytes we just discarded. Remove it
            // before publishing the new journal so rename never relies on
            // platform-specific replace-existing behaviour.
            remove_if_present(&marker_path).await?;
            write_marker_atomically(&marker_path, &marker).await?;
        }

        let existing = tokio::fs::metadata(&partial)
            .await
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let (mut block_index, mut offset) = resume_position(existing, &block_sizes);
        let mut output = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&partial)
            .await?;
        output.set_len(offset).await?;
        output.seek(std::io::SeekFrom::Start(offset)).await?;
        self.update_download(&key, |item| item.downloaded = offset);

        let mut last_snapshot = std::time::Instant::now();
        while block_index < block_sizes.len() {
            if !wait_until_runnable(&mut command).await {
                output.sync_all().await?;
                return Ok(DownloadEnd::Cancelled);
            }
            let size = block_sizes[block_index];
            let bytes = stream.read_range(offset, size).await?;
            if bytes.len() as u64 != size {
                anyhow::bail!(
                    "short block {block_index}: received {}, expected {size}",
                    bytes.len()
                );
            }
            output.write_all(&bytes).await?;
            // The reported boundary must survive a crash; otherwise the next
            // run could trust a file length whose trailing block never reached
            // storage.
            output.sync_data().await?;
            offset += size;
            block_index += 1;
            if last_snapshot.elapsed() >= std::time::Duration::from_millis(100)
                || block_index == block_sizes.len()
            {
                self.update_download(&key, |item| {
                    item.state = DownloadState::Running;
                    item.downloaded = offset;
                });
                last_snapshot = std::time::Instant::now();
            }
        }
        output.sync_all().await?;
        drop(output);
        if !wait_until_runnable(&mut command).await {
            return Ok(DownloadEnd::Cancelled);
        }
        remove_if_present(&destination).await?;
        tokio::fs::rename(&partial, &destination).await?;
        sync_parent(&destination).await?;
        self.finish_download(&key, &target, revision_id, block_sizes, total, &marker_path)
            .await?;
        Ok(DownloadEnd::Completed)
    }

    async fn finish_download(
        &self,
        key: &DownloadKey,
        target: &PlaybackTarget,
        revision_id: String,
        block_sizes: Vec<u64>,
        total: u64,
        marker_path: &std::path::Path,
    ) -> anyhow::Result<()> {
        self.catalog.lock().set_offline_file_durable(
            &target.share_id,
            &target.link_id,
            &OfflineFile {
                revision_id,
                block_sizes,
            },
        )?;
        remove_if_present(marker_path).await?;
        sync_parent(marker_path).await?;
        self.update_download(key, |item| item.total = total);
        Ok(())
    }

    /// Drop a stream the player is done with, so its reader and blocks are not
    /// held for the rest of the session.
    pub fn release(&self, share_id: String, volume_id: String, link_id: String) {
        if let Some(connection) = self.connection.lock().as_ref() {
            connection
                .source
                .close(&share_id, &node_uid(&volume_id, &link_id));
        }
    }

    // ------------------------------------------------------------ thumbnails

    /// Fetch, decode and post a poster for `node`.
    ///
    /// The disk cache is checked first and written on success, so a restart
    /// paints the grid without touching the network. A file with no thumbnail
    /// answers [`Event::ThumbnailMissing`] — the grid must remember that, or
    /// every render pays a round-trip per unmatched title.
    pub fn request_thumbnail(&self, node: &CatalogNode) {
        let engine = self.clone();
        let key = thumbnail_key(node);
        let share_id = node.share_id.clone();
        let uid = node_uid(&node.volume_id, &node.link_id);
        let path = self.dirs.thumbnail_cache().join(format!("{key}.bin"));

        self.runtime.spawn(async move {
            if let Ok(bytes) = tokio::fs::read(&path).await {
                match decode_thumbnail(&bytes) {
                    Ok(image) => return engine.emit(Event::Thumbnail { key, image }),
                    // A truncated or corrupt cache entry is not worth reporting;
                    // fall through and fetch it again.
                    Err(error) => tracing::debug!("cached thumbnail {key}: {error}"),
                }
            }

            // Past the cache, so this one costs the network. Wait for a permit
            // before taking any of the bandwidth playback might want.
            let permits = Arc::clone(&engine.thumbnails);
            let Ok(_permit) = permits.acquire().await else {
                return;
            };

            let Some((library, _)) = engine.ensure_connected().await else {
                return;
            };
            let Some(client) = library.client(&share_id) else {
                return engine.emit(Event::ThumbnailMissing { key });
            };

            // Preview first: it is the one sized for a card. Not every file has
            // one, and the small thumbnail is better than a grey rectangle.
            let mut bytes = None;
            for kind in [ThumbnailType::Preview, ThumbnailType::Thumbnail] {
                match client.download_thumbnail(&uid, kind).await {
                    Ok(Some(found)) => {
                        bytes = Some(found);
                        break;
                    }
                    Ok(None) => {}
                    Err(error) => tracing::debug!("thumbnail {key} ({kind:?}): {error}"),
                }
            }
            let Some(bytes) = bytes else {
                // Common, not exceptional: Proton renders thumbnails on upload,
                // and a share filled by a client that does not attach any — the
                // FUSE client among them — has none at all. The tile falls back
                // to initials until a metadata provider gives it a poster.
                tracing::debug!("no thumbnail for {key}");
                return engine.emit(Event::ThumbnailMissing { key });
            };

            if let Some(parent) = path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let _ = tokio::fs::write(&path, &bytes).await;

            match decode_thumbnail(&bytes) {
                Ok(image) => engine.emit(Event::Thumbnail { key, image }),
                Err(error) => {
                    tracing::debug!("decode thumbnail {key}: {error}");
                    engine.emit(Event::ThumbnailMissing { key });
                }
            }
        });
    }
}

// ---------------------------------------------------------------- enrichment

impl Engine {
    /// The enrichment settings as they stand.
    pub fn metadata_config(&self) -> MetadataConfig {
        self.metadata.lock().config.clone()
    }

    /// Whether a provider is configured *and* usable right now.
    ///
    /// False for "switched off" and for "TMDB with no API key stored" alike —
    /// the UI distinguishes them from the config, not from here.
    pub fn enrichment_ready(&self) -> bool {
        self.metadata.lock().service.is_some()
    }

    /// Change the settings, write them, and rebuild the provider.
    ///
    /// Turning enrichment *off* clears the stored answers as well. That is the
    /// point of the switch: someone who turns this off has decided they would
    /// rather the third party's answers were not on their disk either, and
    /// leaving them there would make "off" mean only "stop asking".
    pub fn set_metadata_config(&self, config: MetadataConfig) {
        let engine = self.clone();
        self.runtime.spawn_blocking(move || {
            let previous = engine.metadata_config();
            if let Err(error) = pstr_meta::settings::save(&engine.dirs, &config) {
                return engine.fail("save the metadata settings", error);
            }

            let forget = !config.enabled || config.provider != previous.provider;
            if forget && let Err(error) = engine.catalog.lock().clear_metadata() {
                tracing::warn!("clear stored metadata: {error}");
            }

            *engine.metadata.lock() = Enrichment::build(config.clone());
            engine.emit(Event::MetadataConfig(config));
            engine.load_metadata();
        });
    }

    /// Store an API key for a provider and rebuild against it.
    pub fn set_api_key(&self, provider: ProviderId, key: String) {
        let engine = self.clone();
        self.runtime.spawn_blocking(move || {
            if let Err(error) = pstr_meta::settings::set_api_key(provider, &key) {
                return engine.fail("store the API key", error);
            }
            let config = engine.metadata_config();
            *engine.metadata.lock() = Enrichment::build(config);
            engine.emit(Event::Status(if key.trim().is_empty() {
                format!("cleared the {} key", provider.label())
            } else {
                format!("saved the {} key", provider.label())
            }));
        });
    }

    /// Re-read every stored provider answer.
    ///
    /// Read whole rather than per tile, for the same reason watch state is —
    /// see [`Catalog::all_metadata`].
    pub fn load_metadata(&self) {
        let engine = self.clone();
        self.runtime.spawn_blocking(move || {
            // One lock, both reads: they are drawn together, and a UI that has
            // the titles but not their episodes for a frame flickers the rows.
            let answers = {
                let catalog = engine.catalog.lock();
                catalog
                    .all_metadata()
                    .and_then(|records| Ok((records, catalog.all_episode_metadata()?)))
            };
            match answers {
                Ok((records, episodes)) => {
                    engine.emit(Event::Metadata(records));
                    engine.emit(Event::EpisodeMetadata(episodes));
                }
                Err(error) => engine.fail("read stored metadata", error),
            }
        });
    }

    /// Look up every title that has no fresh answer.
    ///
    /// `force` re-asks about titles that already have one, which is what
    /// "match again" means. Otherwise a run over an already-matched library
    /// makes no requests at all, which is what makes it safe to call whenever
    /// the library changes.
    pub fn match_titles(&self, titles: Vec<Title>, force: bool) {
        let engine = self.clone();
        self.runtime.spawn(async move {
            engine.run_match(titles, force).await;
            engine.emit(Event::MatchFinished);
        });
    }

    async fn run_match(&self, titles: Vec<Title>, force: bool) {
        let Some(service) = self.metadata.lock().service.clone() else {
            let config = self.metadata_config();
            return self.emit(Event::Error(if !config.enabled {
                "turn on metadata enrichment first".into()
            } else {
                format!("{} needs an API key", config.provider.label())
            }));
        };
        let provider = service.provider();

        let (stored, listed) = {
            let catalog = self.catalog.lock();
            match catalog
                .all_metadata()
                .and_then(|stored| Ok((stored, catalog.episode_metadata_ages()?)))
            {
                Ok(pair) => pair,
                Err(error) => return self.fail("read stored metadata", error),
            }
        };

        // Two kinds of work, and the difference is a request saved: a title
        // that already has a good match but no episode list needs the episode
        // request only — searching for it again would ask the provider
        // something it has already answered.
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

        if pending.is_empty() {
            return self.emit(Event::Status("everything is already matched".into()));
        }
        self.emit(Event::Status(format!(
            "matching {} titles against {}…",
            pending.len(),
            provider.label()
        )));

        let (mut matched, mut unmatched, mut failed) = (0usize, 0usize, 0usize);
        let mut lookups = futures::stream::FuturesUnordered::new();
        for work in &pending {
            let service = service.clone();
            let permits = Arc::clone(&self.lookups);
            lookups.push(async move {
                let Ok(_permit) = permits.acquire().await else {
                    return None;
                };
                Some(match work {
                    Work::Match(title) => match service.record(title).await {
                        Ok(record) => {
                            // Only for a title that matched: there is no id to
                            // ask about otherwise.
                            let episodes = match &record.metadata {
                                Some(found) => episodes_of(&service, title, found).await,
                                None => Vec::new(),
                            };
                            Ok((record.title_key.clone(), Some(record), episodes))
                        }
                        Err(error) => Err(error),
                    },
                    Work::Episodes(title, found) => Ok((
                        title.key.clone(),
                        None,
                        episodes_of(&service, title, found).await,
                    )),
                })
            });
        }

        use futures::StreamExt as _;
        let mut episodes_found = 0usize;
        while let Some(result) = lookups.next().await {
            match result {
                Some(Ok((title_key, record, episodes))) => {
                    if let Some(record) = &record {
                        if record.metadata.is_some() {
                            matched += 1;
                        } else {
                            unmatched += 1;
                        }
                        // Misses are stored on purpose; failures below are not.
                        // See `pstr_meta::service`.
                        if let Err(error) = self.catalog.lock().set_metadata(record) {
                            tracing::warn!("store metadata for {}: {error}", record.title_key);
                        }
                    }
                    if !episodes.is_empty() {
                        episodes_found += episodes.len();
                        let stored = self.catalog.lock().set_episode_metadata(
                            &title_key,
                            provider,
                            now(),
                            &episodes,
                        );
                        if let Err(error) = stored {
                            tracing::warn!("store episodes for {title_key}: {error}");
                        }
                    }
                }
                Some(Err(error)) => {
                    failed += 1;
                    tracing::warn!("metadata lookup: {error}");
                }
                None => {}
            }
        }

        if episodes_found > 0 {
            self.emit(Event::Status(format!("{episodes_found} episodes named")));
        }
        self.emit(Event::Matched {
            matched,
            unmatched,
            failed,
        });
        self.load_metadata();
    }

    /// Ask the provider what `term` might be, for a viewer choosing by hand.
    ///
    /// Unscored and unfiltered — see [`MetadataService::search`]. Nothing is
    /// stored: this is a look, and only [`Engine::choose_match`] writes.
    pub fn search_matches(&self, title_key: String, term: String, kind: TitleKind) {
        let Some(service) = self.metadata.lock().service.clone() else {
            let config = self.metadata_config();
            return self.emit(Event::MatchSearchFailed {
                title_key,
                error: if !config.enabled {
                    "turn on metadata enrichment first".into()
                } else {
                    format!("{} needs an API key", config.provider.label())
                },
            });
        };

        let engine = self.clone();
        let permits = Arc::clone(&self.lookups);
        self.runtime.spawn(async move {
            // The same permit the batch run takes: a viewer typing in the search
            // box while a match run is going must not be what earns the 429.
            let Ok(_permit) = permits.acquire().await else {
                return;
            };
            match service.search(&term, kind).await {
                Ok(options) => engine.emit(Event::MatchOptions { title_key, options }),
                Err(error) => {
                    tracing::warn!("search {term:?}: {error}");
                    engine.emit(Event::MatchSearchFailed {
                        title_key,
                        error: error.to_string(),
                    });
                }
            }
        });
    }

    /// Pin one title to the entry the viewer picked, and take its episodes.
    ///
    /// Stored as [`MetadataRecord::manual`], so it outlives every TTL and
    /// survives "match again". The episode list is fetched here rather than left
    /// to the next match run, because the point of choosing an entry by hand is
    /// usually that its episode names were wrong too.
    pub fn choose_match(&self, title: Title, found: pstr_core::metadata::TitleMetadata) {
        let Some(service) = self.metadata.lock().service.clone() else {
            return self.emit(Event::Error("turn on metadata enrichment first".into()));
        };

        let engine = self.clone();
        self.runtime.spawn(async move {
            let name = found.name.clone();
            let record = service.chosen(title.key.clone(), found.clone());
            if let Err(error) = engine.catalog.lock().set_metadata(&record) {
                return engine.fail("store the match", error);
            }

            let episodes = episodes_of(&service, &title, &found).await;
            if !episodes.is_empty() {
                let stored = engine.catalog.lock().set_episode_metadata(
                    &title.key,
                    service.provider(),
                    now(),
                    &episodes,
                );
                if let Err(error) = stored {
                    tracing::warn!("store episodes for {}: {error}", title.key);
                }
            }

            engine.emit(Event::Status(format!("{} is now {name}", title.name)));
            engine.load_metadata();
        });
    }

    /// Drop what is stored for one title, so it is matched from scratch again.
    ///
    /// The undo for [`Engine::choose_match`], and the repair for a bad automatic
    /// match: the row goes entirely rather than becoming a stored miss, which is
    /// what makes the next match run treat the title as one it has never asked
    /// about.
    pub fn forget_match(&self, title_key: String) {
        let engine = self.clone();
        self.runtime.spawn_blocking(move || {
            if let Err(error) = engine.catalog.lock().forget_metadata(&title_key) {
                return engine.fail("forget the match", error);
            }
            engine.emit(Event::Status("match cleared".into()));
            engine.load_metadata();
        });
    }

    /// Fetch, decode and post one title's artwork.
    ///
    /// Cached on disk under a hash of the URL, so a provider that moves its CDN
    /// paths invalidates by itself and the same picture shared by two titles is
    /// fetched once.
    pub fn request_poster(&self, title_key: String, url: String) {
        let engine = self.clone();
        let path = self
            .dirs
            .poster_cache()
            .join(format!("{}.img", digest(&url)));

        self.runtime.spawn(async move {
            if let Ok(bytes) = tokio::fs::read(&path).await {
                match decode_thumbnail(&bytes) {
                    Ok(image) => {
                        return engine.emit(Event::Poster {
                            key: title_key,
                            image,
                        });
                    }
                    Err(error) => tracing::debug!("cached poster {title_key}: {error}"),
                }
            }

            let Some(service) = engine.metadata.lock().service.clone() else {
                // Enrichment was turned off between the request and now.
                return engine.emit(Event::PosterMissing { key: title_key });
            };

            let bytes = match service.artwork(&url).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    tracing::debug!("fetch poster for {title_key}: {error}");
                    return engine.emit(Event::PosterMissing { key: title_key });
                }
            };

            if let Some(parent) = path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let _ = tokio::fs::write(&path, &bytes).await;

            match decode_thumbnail(&bytes) {
                Ok(image) => engine.emit(Event::Poster {
                    key: title_key,
                    image,
                }),
                Err(error) => {
                    tracing::debug!("decode poster for {title_key}: {error}");
                    engine.emit(Event::PosterMissing { key: title_key })
                }
            }
        });
    }
}

/// A short, filesystem-safe name for a URL.
fn digest(url: &str) -> String {
    use sha2::{Digest as _, Sha256};
    let hash = Sha256::digest(url.as_bytes());
    hash.iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The cache and texture key for a file's poster.
/// One title's worth of work in a match run.
enum Work {
    /// Search for it, and take its episodes if it matches.
    Match(Title),
    /// It is already matched; only the episode list is missing.
    Episodes(Title, Box<pstr_core::metadata::TitleMetadata>),
}

/// The episode list for a whole title, with a failure treated as an empty one.
///
/// Deliberately not fatal to the title it belongs to: a poster and a synopsis
/// that arrived are worth keeping even when the episode request was the one
/// that hit the rate limit. The empty result is not cached as an answer — the
/// next match run asks again — because the only thing that triggers one is a
/// viewer pressing the button.
///
/// The match itself only ever answers for *one* entry, and on a provider that
/// files each sequel separately — AniList — that entry is season one. So each
/// further season of the title is searched for by name and its episodes are
/// tagged with the season they came from; without that, seasons two and three
/// have no episode names at all. See
/// [`MetadataService::season_episodes`](pstr_meta::MetadataService::season_episodes).
async fn episodes_of(
    service: &MetadataService,
    title: &Title,
    found: &pstr_core::metadata::TitleMetadata,
) -> Vec<pstr_core::metadata::EpisodeMetadata> {
    let mut episodes = service
        .episodes(found)
        .await
        .inspect_err(|error| tracing::warn!("episodes for {}: {error}", found.name))
        .unwrap_or_default();

    if !service.splits_seasons() {
        return episodes;
    }

    // Season one is what the title's own match already answered for.
    let later: Vec<u32> = title
        .seasons
        .iter()
        .filter_map(|season| season.number)
        .filter(|number| *number > 1)
        .collect();
    for season in later {
        match service.season_episodes(title, season).await {
            Ok(found) => episodes.extend(found),
            Err(error) => {
                tracing::warn!("episodes for {} season {season}: {error}", title.name);
            }
        }
    }
    episodes
}

/// Unix seconds, for the "when was this asked" columns.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or_default()
}

pub fn thumbnail_key(node: &CatalogNode) -> String {
    format!("{}-{}", node.share_id, node.link_id)
}

fn node_uid(volume_id: &str, link_id: &str) -> NodeUid {
    NodeUid::new(
        VolumeId::new(volume_id.to_string()),
        LinkId::new(link_id.to_string()),
    )
}

/// Resume only after the last completely durable Proton block. A process may
/// die during a write, so trailing bytes inside a block are deliberately
/// discarded rather than trusted.
fn resume_position(existing: u64, block_sizes: &[u64]) -> (usize, u64) {
    let mut offset = 0_u64;
    for (index, size) in block_sizes.iter().copied().enumerate() {
        let Some(next) = offset.checked_add(size) else {
            break;
        };
        if next > existing {
            return (index, offset);
        }
        offset = next;
    }
    (block_sizes.len(), offset)
}

async fn wait_until_runnable(command: &mut tokio::sync::watch::Receiver<DownloadCommand>) -> bool {
    loop {
        let current = *command.borrow_and_update();
        match current {
            DownloadCommand::Run => return true,
            DownloadCommand::Cancel => return false,
            DownloadCommand::Pause => {
                if command.changed().await.is_err() {
                    return false;
                }
            }
        }
    }
}

async fn remove_if_present(path: &std::path::Path) -> std::io::Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
async fn sync_parent(path: &std::path::Path) -> std::io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    tokio::fs::File::open(parent).await?.sync_all().await
}

#[cfg(not(unix))]
async fn sync_parent(_path: &std::path::Path) -> std::io::Result<()> {
    // Windows does not expose directory handles through std/tokio. The file
    // itself is synced before publication and rename is still atomic.
    Ok(())
}

async fn write_marker_atomically(
    path: &std::path::Path,
    marker: &PartialMarker,
) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;
    let temporary = path.with_extension("partial.json.tmp");
    let bytes = serde_json::to_vec(marker)?;
    let mut output = tokio::fs::File::create(&temporary).await?;
    output.write_all(&bytes).await?;
    output.sync_all().await?;
    drop(output);
    tokio::fs::rename(temporary, path).await?;
    sync_parent(path).await?;
    Ok(())
}

/// Decode image bytes into something egui can upload, scaled down to
/// [`THUMBNAIL_MAX_EDGE`].
///
/// Runs on the runtime rather than the UI thread: a JPEG decode per tile during
/// the first paint of a large library is visible as dropped frames.
fn decode_thumbnail(bytes: &[u8]) -> anyhow::Result<egui::ColorImage> {
    let decoded = image::load_from_memory(bytes)?;
    let decoded = if decoded.width().max(decoded.height()) > THUMBNAIL_MAX_EDGE {
        decoded.thumbnail(THUMBNAIL_MAX_EDGE, THUMBNAIL_MAX_EDGE)
    } else {
        decoded
    };
    let rgba = decoded.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        size,
        rgba.as_flat_samples().as_slice(),
    ))
}

/// Watch state as of now, for a position in a file.
pub fn watch_state(position: f64, duration: Option<f64>, watched: bool) -> WatchState {
    WatchState {
        position_secs: position,
        duration_secs: duration,
        watched,
        updated_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_secs() as i64)
            .unwrap_or(0),
    }
}

/// Shares that failed to open, formatted for one status line.
pub fn describe_failures(failures: &[(String, String)]) -> Option<String> {
    if failures.is_empty() {
        return None;
    }
    let names: Vec<&str> = failures.iter().map(|(name, _)| name.as_str()).collect();
    Some(format!("could not open: {}", names.join(", ")))
}

/// Pictures the UI has, plus the ones it has already asked for.
///
/// Two instances exist and they hold different things: one keyed by file for
/// Proton's own thumbnails, one keyed by title for a provider's artwork. What
/// they share is the part that matters — a fast scroll must not re-ask for the
/// same picture sixty times a second, and a picture that turned out not to exist
/// must be remembered as not existing. Without that second half, every render
/// pays a round-trip per unmatched tile, which is exactly what
/// `proton-drive-linux`'s photo grid had to learn.
#[derive(Default)]
pub struct ImageCache {
    textures: HashMap<String, egui::TextureHandle>,
    requested: std::collections::HashSet<String>,
    missing: std::collections::HashSet<String>,
}

impl ImageCache {
    /// The texture for `key`, calling `fetch` the first time it is asked for.
    ///
    /// `None` while a fetch is in flight, and forever for a picture that turned
    /// out not to exist — the negative answer is remembered deliberately.
    pub fn texture(&mut self, key: String, fetch: impl FnOnce()) -> Option<egui::TextureHandle> {
        if let Some(texture) = self.textures.get(&key) {
            return Some(texture.clone());
        }
        if !self.missing.contains(&key) && self.requested.insert(key) {
            fetch();
        }
        None
    }

    pub fn insert(&mut self, ctx: &egui::Context, key: String, image: egui::ColorImage) {
        let texture = ctx.load_texture(&key, image, egui::TextureOptions::LINEAR);
        self.textures.insert(key, texture);
    }

    pub fn mark_missing(&mut self, key: String) {
        self.missing.insert(key);
    }

    /// Forget everything. Used when the catalog is replaced, so a recrawl that
    /// changed link ids does not leave stale pictures behind, and when the
    /// metadata provider changes, so the old provider's artwork goes with it.
    pub fn clear(&mut self) {
        self.textures.clear();
        self.requested.clear();
        self.missing.clear();
    }
}

#[cfg(test)]
mod download_tests {
    use super::*;

    #[test]
    fn a_partial_download_resumes_only_after_a_complete_block() {
        let blocks = [3, 5, 2];
        assert_eq!(resume_position(0, &blocks), (0, 0));
        assert_eq!(resume_position(3, &blocks), (1, 3));
        assert_eq!(resume_position(7, &blocks), (1, 3));
        assert_eq!(resume_position(8, &blocks), (2, 8));
        assert_eq!(resume_position(99, &blocks), (3, 10));
    }

    #[test]
    fn pause_resume_and_cancel_controls_are_distinct() {
        let control = DownloadControl::new();
        let command = control.subscribe();
        assert_eq!(*command.borrow(), DownloadCommand::Run);
        control.set(DownloadCommand::Pause);
        assert_eq!(*command.borrow(), DownloadCommand::Pause);
        control.set(DownloadCommand::Run);
        assert_eq!(*command.borrow(), DownloadCommand::Run);
        control.set(DownloadCommand::Cancel);
        assert_eq!(*command.borrow(), DownloadCommand::Cancel);
    }

    #[tokio::test]
    async fn resume_before_wait_cannot_be_lost() {
        let control = DownloadControl::new();
        let mut command = control.subscribe();
        control.set(DownloadCommand::Pause);
        control.set(DownloadCommand::Run);
        assert!(wait_until_runnable(&mut command).await);
    }

    #[tokio::test]
    async fn cancellation_wakes_a_paused_writer_deterministically() {
        let control = Arc::new(DownloadControl::new());
        let mut command = control.subscribe();
        control.set(DownloadCommand::Pause);
        let waiter = tokio::spawn(async move { wait_until_runnable(&mut command).await });
        tokio::task::yield_now().await;
        control.set(DownloadCommand::Cancel);
        assert!(
            !tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
                .await
                .expect("the paused waiter woke")
                .expect("waiter did not panic")
        );
    }

    #[test]
    fn startup_hydrates_a_durable_partial_as_resumable() {
        let unique = format!(
            "pstr-download-hydration-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        let dirs = AppDirs::from_paths(root.join("config"), root.join("data"), root.join("cache"))
            .expect("temporary app dirs");
        let runtime = Arc::new(Runtime::new().expect("runtime"));
        let (engine, _) =
            Engine::new(Arc::clone(&runtime), dirs, egui::Context::default()).expect("engine");
        let target = PlaybackTarget {
            share_id: "share".to_owned(),
            volume_id: "volume".to_owned(),
            link_id: "episode".to_owned(),
            name: "Show.S01E01.mkv".to_owned(),
            title_key: "show".to_owned(),
            title_name: "Show".to_owned(),
            subtitle: "S01E01".to_owned(),
            season: Some(1),
            number: Some(1),
            episode_name: None,
            resume_at: None,
            track_prefs: None,
        };
        let key = DownloadKey::from(&target);
        let (partial, marker_path) = engine.partial_paths(&key);
        std::fs::create_dir_all(partial.parent().expect("offline parent"))
            .expect("create offline parent");
        std::fs::write(&partial, [0_u8; 7]).expect("write one block and a fragment");
        runtime
            .block_on(write_marker_atomically(
                &marker_path,
                &PartialMarker {
                    revision_id: "revision".to_owned(),
                    block_sizes: vec![4, 5],
                    target: Some(target),
                },
            ))
            .expect("durable marker");

        engine.hydrate_partial_downloads();
        let downloads = engine.downloads.lock();
        let item = downloads.items.get(&key).expect("hydrated row");
        assert_eq!(item.state, DownloadState::Cancelled);
        assert_eq!(item.downloaded, 4, "only a complete block is resumable");
        assert_eq!(item.total, 9);
        drop(downloads);
        std::fs::remove_dir_all(root).expect("remove temporary app dirs");
    }
}
