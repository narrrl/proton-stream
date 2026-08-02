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

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use parking_lot::Mutex;
use pstr_core::catalog::{Catalog, CatalogNode, WatchState, build_rows};
use pstr_core::config::AppDirs;
use pstr_core::library::{Library, Title};
use pstr_core::metadata::{EpisodeGuide, MetadataConfig, MetadataRecord, ProviderId};
use pstr_core::prefs::PlaybackPrefs;
use pstr_core::proton_drive_rs::ThumbnailType;
use pstr_core::proton_sdk::ids::{LinkId, NodeUid, VolumeId};
use pstr_core::{Share, ShareStore, SharedLibrary};
use pstr_meta::MetadataService;
use pstr_stream::{DiskCacheConfig, LibraryOpener, StreamConfig, StreamSource, VideoStream};
use tokio::runtime::Runtime;

use crate::playback::PlaybackTarget;

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
    events: Sender<Event>,
    ctx: egui::Context,
}

/// How many posters may be in flight at once.
const THUMBNAIL_CONCURRENCY: usize = 6;

/// How many provider lookups may be in flight at once. See [`Engine::lookups`].
const LOOKUP_CONCURRENCY: usize = 2;

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
            if let Err(error) = engine.store.remove(&id) {
                return engine.fail("remove that share", error);
            }
            {
                let catalog = engine.catalog.lock();
                if let Err(error) = catalog.remove_share(&id) {
                    tracing::warn!("drop catalog rows for {id}: {error}");
                }
            }
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
                    engine.emit(Event::LibraryLoaded(Library::build(files, &watch)))
                }
                Err(error) => engine.fail("read the catalog", error),
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

            let stored = {
                let mut catalog = self.catalog.lock();
                catalog.replace_share(&share_id, &rows)
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
            if force || !pstr_meta::service::is_usable(record, provider) {
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
