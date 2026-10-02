//! The window: state, the event pump, and what a click turns into.
//!
//! One rule holds this together — **drawing never mutates**. A page is handed
//! what it needs by reference and pushes [`Action`]s onto a list; the list is
//! applied after the frame. That is what lets a card click change the page it
//! is drawn on, and it is why none of the `ui` modules take an `&mut App`.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;

use pstr_core::Share;
use pstr_core::appearance::Appearance;
use pstr_core::config::AppDirs;
use pstr_core::library::Library;
use pstr_core::metadata::{EpisodeGuide, MetadataConfig, MetadataRecord};

use crate::engine::{
    DownloadItem, DownloadKey, Engine, Event, ImageCache, describe_failures, watch_state,
};
use crate::pacing::Pacer;
use crate::playback::{Playback, PlaybackTarget};
use crate::ui::player::UpNextCard;
use crate::{theme, ui};

/// A frame slower than this is not a slow frame, it is a hang.
///
/// The UI thread has only a handful of blocking calls in it — building an mpv
/// instance, tearing one down, the OpenGL work in between — and every one of
/// them is somewhere the compositor will decide the window has stopped
/// answering. There is no way to tell afterwards which one it was, so the ones
/// that can block say how long they took, and the frame as a whole says so too.
const STALL: std::time::Duration = std::time::Duration::from_millis(300);

/// Time a frame, and log what it was doing if it ran long.
///
/// A guard rather than a wrapper because [`eframe::App::ui`] returns from two
/// places, and the interesting one is the early return for the player page.
struct FrameTimer {
    started: std::time::Instant,
    page: &'static str,
}

impl FrameTimer {
    fn new(page: &Page) -> Self {
        Self {
            started: std::time::Instant::now(),
            page: match page {
                Page::Library => "library",
                Page::Title(_) => "title",
                Page::Shares => "shares",
                Page::Downloads => "downloads",
                Page::Settings => "settings",
                Page::Player => "player",
            },
        }
    }
}

impl Drop for FrameTimer {
    fn drop(&mut self) {
        let took = self.started.elapsed();
        if took >= STALL {
            tracing::warn!(
                "ui thread blocked for {} ms drawing the {} page",
                took.as_millis(),
                self.page
            );
        }
    }
}

/// How long the "up next" card counts down before the next episode starts.
///
/// Ten seconds is what every service settled on, and the reason is the same
/// here: long enough to read what is coming and to say no, short enough that
/// sitting through it is not a decision.
const UP_NEXT_SECONDS: f64 = 10.0;

/// Which page is showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Page {
    Library,
    /// One title, by [`pstr_core::library::Title::key`].
    Title(String),
    Shares,
    Downloads,
    /// How the app looks, how it plays, and what it looks up.
    Settings,
    /// The picture, filling the window. Leaving this page does not stop
    /// playback — the transport bar at the bottom is how you get back to it.
    Player,
}

/// Something a click asked for, applied once the frame is drawn.
pub enum Action {
    Goto(Page),
    Play(PlaybackTarget),
    /// Download one or more files as complete local copies.
    MakeOffline(Vec<PlaybackTarget>),
    PauseDownload(DownloadKey),
    ResumeDownload(DownloadKey),
    CancelDownload(DownloadKey),
    RemoveDownload(DownloadKey, bool),
    /// Crawl one share, or every share.
    Crawl(Option<String>),
    /// Stop the crawls still listing.
    StopCrawl,
    AddShare {
        name: String,
        url: String,
        password: Option<String>,
    },
    /// Ask whether to forget a share. [`Action::ForgetShare`] does it.
    RemoveShare(String),
    ForgetShare(String),
    /// Flip an episode between seen and unseen by hand.
    SetWatched {
        share_id: String,
        link_id: String,
        watched: bool,
        duration: Option<f64>,
    },
    Player(crate::playback::Command),
    /// Change the volume, 0–100. `commit` writes it to the preferences; a
    /// slider mid-drag does not.
    SetVolume {
        volume: f64,
        commit: bool,
    },
    ToggleMute,
    /// Play this track of that kind, or none of it, and remember the language
    /// for the next file.
    SelectTrack {
        kind: pstr_player::TrackKind,
        id: Option<i64>,
    },
    /// Turn enrichment on or off, or change provider.
    SetMetadataConfig(MetadataConfig),
    SetApiKey {
        provider: pstr_core::metadata::ProviderId,
        key: String,
    },
    /// Look every title up. `force` re-asks about ones already matched.
    MatchTitles {
        force: bool,
    },
    /// Open the hand-matching search for one title, seeded with its own name.
    OpenMatcher(String),
    CloseMatcher,
    /// Ask the provider what the text in the box might be.
    SearchMatches,
    /// Pin the open title to this entry.
    ChooseMatch(Box<pstr_core::metadata::TitleMetadata>),
    /// Forget what is stored for a title, so it is matched from scratch again.
    ForgetMatch(String),
    /// Play the file before or after the one playing, within its title.
    PlayAdjacent(Adjacent),
    /// Start or stop the next episode playing on its own at the end of one.
    SetAutoplay(bool),
    /// Play faster or slower, and keep doing so for the next file.
    SetSpeed(f64),
    /// Seek past openings and credits without being asked.
    SetAutoSkip(bool),
    /// Repaint the window in a different palette.
    SetAppearance(Appearance),
    /// In or out of fullscreen, from the player page.
    ToggleFullscreen,
    /// Stop watching, keep playing: back to the page this film came from, with
    /// the transport bar at the bottom still driving it.
    LeavePlayer,
    /// Call off the end-of-episode countdown and let this file play out.
    WatchToEnd,
    /// Put the caret in the search box, from anywhere in the library.
    FocusSearch,
    /// Show a different slice of the library, or in a different order.
    SetShelf(Filter, Sort),
    /// Open or close the list of keyboard shortcuts.
    ToggleShortcuts,
    /// One page back: a title to the library, the library to nowhere.
    Back,
}

/// The end-of-episode countdown.
///
/// Playback reaching the credits is not the same thing as the file ending, and
/// this is the difference: the run of chapters that closes an episode is known
/// (see [`pstr_player::credits_start`]), so the next episode can be offered
/// while the last one is still playing rather than after a black screen.
///
/// It is deliberately per-file — a new player resets it — and deliberately
/// dismissable *for the rest of the file*: a viewer who wants to hear the
/// ending song should be asked once, not once a second.
#[derive(Debug, Default)]
pub struct UpNext {
    /// Which player this belongs to. A different one means a new file, and a
    /// new file means the viewer's last answer no longer applies.
    playback_id: u64,
    /// When the countdown started, on egui's clock. `None` while there is
    /// nothing to count down.
    started: Option<f64>,
    /// The viewer said to play this one out, or the countdown has already
    /// fired. Either way, do not ask again about this file.
    dismissed: bool,
}

/// Which way to step through a title's files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adjacent {
    Previous,
    Next,
}

/// Picking a title's entry by hand.
///
/// The escape hatch from the matcher, which is deliberately unwilling to guess:
/// [`pstr_meta::matching::MATCH_FLOOR`] is set where a wrong poster is worse
/// than none, and the cost of that is a handful of titles that match nothing.
/// The `Fate/stay night [Heaven's Feel]` films are the standing example — three
/// films in one folder, filed by AniList as three separate entries, so there is
/// no answer for the scorer to find and only the viewer knows which one the
/// folder means.
pub struct Matcher {
    /// The title being matched, by [`pstr_core::library::Title::key`].
    pub title_key: String,
    /// Its name as the library has it, for the dialog's own heading.
    pub title_name: String,
    pub kind: pstr_core::library::TitleKind,
    /// What is in the search box. Seeded with the library's name for the title,
    /// which is the thing that failed — so it is the right starting point to
    /// edit rather than to re-send.
    pub query: String,
    pub results: Vec<pstr_core::metadata::TitleMetadata>,
    /// A request is out. The box stays usable; only the button is held.
    pub searching: bool,
    /// Whether a search has come back yet, so an empty list can read as "nothing
    /// found" rather than "nothing asked".
    pub asked: bool,
    pub error: Option<String>,
    /// Set for the frame the dialog opens, to put the caret in the box.
    pub focus: bool,
}

impl Matcher {
    pub fn new(title: &pstr_core::library::Title) -> Self {
        Self {
            title_key: title.key.clone(),
            title_name: title.name.clone(),
            kind: title.kind,
            query: title.name.clone(),
            results: Vec::new(),
            searching: false,
            asked: false,
            error: None,
            focus: true,
        }
    }
}

/// What the library page lists, worked out once per change rather than once a
/// frame.
///
/// A search lowercases every filename in the library, and the shelf sorts
/// every title by when it was last played. Neither is slow once; both were
/// being done on every frame, and while a film plays under the transport bar
/// that is the film's frame rate.
#[derive(Default)]
pub struct LibraryView {
    /// The query `matches` answers, as typed.
    query: String,
    /// Which titles the grid shows, and in what order.
    pub filter: Filter,
    pub sort: Sort,
    /// The filter and order `matches` was built with.
    built_with: (Filter, Sort),
    /// Indices into [`Library::titles`].
    pub matches: Vec<usize>,
    /// The "Continue watching" shelf, most recent first, as indices.
    pub resumable: Vec<usize>,
    /// The library changed under the lists.
    stale: bool,
}

/// Which titles the grid shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    Series,
    Films,
    /// Started and not finished.
    Watching,
    /// Not started.
    Unwatched,
}

impl Filter {
    fn admits(self, title: &pstr_core::library::Title) -> bool {
        use pstr_core::library::TitleKind;
        let watched = title.watched_count();
        match self {
            Self::All => true,
            Self::Series => title.kind == TitleKind::Series,
            Self::Films => title.kind == TitleKind::Film,
            Self::Watching => {
                title.resume().is_some() || (watched > 0 && watched < title.episode_count())
            }
            Self::Unwatched => watched == 0 && title.resume().is_none(),
        }
    }
}

/// What order the grid is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    /// The library's own order: by name, ignoring a leading article.
    #[default]
    Name,
    /// Most recently played first.
    Recent,
    /// Newest first, undated last.
    Year,
}

impl LibraryView {
    fn refresh(&mut self, library: &Library, query: &str) {
        if !self.stale && self.query == query && self.built_with == (self.filter, self.sort) {
            return;
        }
        let index_of = |title: &pstr_core::library::Title| {
            library
                .titles
                .iter()
                .position(|candidate| std::ptr::eq(candidate, title))
        };
        let mut matches: Vec<&pstr_core::library::Title> = library
            .search(query)
            .into_iter()
            .filter(|title| self.filter.admits(title))
            .collect();
        match self.sort {
            Sort::Name => {}
            // Stable, so titles never played keep their alphabetical order
            // after the ones that have been.
            Sort::Recent => matches.sort_by_key(|title| std::cmp::Reverse(title.last_played())),
            Sort::Year => matches.sort_by_key(|title| std::cmp::Reverse(title.year.unwrap_or(0))),
        }
        self.matches = matches.into_iter().filter_map(index_of).collect();
        self.built_with = (self.filter, self.sort);
        if self.stale {
            self.resumable = library
                .continue_watching()
                .into_iter()
                .filter_map(index_of)
                .collect();
        }
        self.query = query.to_owned();
        self.stale = false;
    }
}

/// The share the viewer is typing in.
#[derive(Default)]
pub struct ShareForm {
    pub name: String,
    pub url: String,
    pub has_password: bool,
    pub password: String,
    /// Sent, and not answered yet. The form keeps what was typed until it is,
    /// so a refused link can be corrected rather than pasted again.
    pub sending: bool,
    /// Why the last one was refused.
    pub error: Option<String>,
}

pub struct App {
    engine: Engine,
    events: Receiver<Event>,
    pub page: Page,
    pub library: Library,
    pub shares: Vec<Share>,
    /// Proton's own per-file thumbnails.
    pub thumbs: ImageCache,
    /// Artwork from a metadata provider, keyed by title key.
    pub posters: ImageCache,
    /// What the providers have said, keyed by title key.
    pub metadata: HashMap<String, MetadataRecord>,
    /// What they said about the episodes under those titles.
    pub episodes: HashMap<String, EpisodeGuide>,
    pub settings: MetadataConfig,
    /// What the viewer is typing into the API key box. Never persisted here —
    /// it goes straight to the credential store on save.
    pub api_key: String,
    pub matching: bool,
    /// The hand-matching dialog, while it is open.
    pub matcher: Option<Matcher>,
    pub search: String,
    /// Set for the frame the search box should take the caret.
    focus_search: bool,
    /// Whether the list of keyboard shortcuts is open.
    pub shortcuts_open: bool,
    /// What a key just did in the player, and when: "Volume 55%", "+30 s".
    osd: Option<(String, f64)>,
    pub view: LibraryView,
    /// Whether the catalog has been read at all yet. Until it has, an empty
    /// library means "not loaded", not "nothing in it".
    pub loaded: bool,
    pub form: ShareForm,
    pub playback: Option<Playback>,
    /// Whether the player page's controls are showing, and why.
    pub overlay: ui::player::Overlay,
    /// The end-of-episode countdown for whatever is playing.
    pub up_next: UpNext,
    /// The player whose next episode has already been fetched ahead, so it is
    /// asked for once per file rather than once a frame.
    warmed: u64,
    /// Chapters already skipped on their own in this player, so seeking back
    /// into an opening on purpose is not undone a frame later.
    auto_skipped: (u64, Vec<i64>),
    /// Set while the window is fullscreen, so `F` can toggle rather than only
    /// ever entering. egui has no way to ask the platform.
    pub fullscreen: bool,
    /// The file being opened, if a click is still waiting on the network.
    pub opening: Option<String>,
    pub connecting: bool,
    pub crawling: bool,
    /// Which share the crawl is on, and how far it has got.
    /// Files found so far, by share name, for each share still listing.
    crawl_progress: std::collections::BTreeMap<String, usize>,
    /// The window's size, written at exit for the next launch.
    window: crate::window::WindowMemory,
    pub downloads: Vec<DownloadItem>,
    pub offline_files: std::collections::HashSet<DownloadKey>,
    pub confirm_partial_delete: Option<DownloadKey>,
    /// A share the viewer asked to remove, waiting on their say-so.
    pub confirm_remove_share: Option<String>,
    /// Messages in the corner. See [`ui::toast`].
    pub toasts: ui::toast::Toasts,
    /// The frame ceiling, where this session is pacing itself rather than
    /// letting vsync do it — see [`crate::pacing`].
    pacer: Pacer,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        runtime: std::sync::Arc<tokio::runtime::Runtime>,
        dirs: AppDirs,
    ) -> anyhow::Result<Self> {
        // After the engine, not before: the engine is what reads the stored
        // theme, and a window that paints one frame in the default palette
        // before switching is a window that flashes on every launch.
        let window = crate::window::WindowMemory::new(dirs.window_file());
        let (engine, events) = Engine::new(runtime, dirs, cc.egui_ctx.clone())?;
        theme::install_font_fallbacks(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx, engine.appearance());

        // Paint from the catalog immediately; the network catches up. A library
        // that was crawled yesterday is on screen before the shares open.
        engine.load_shares();
        engine.load_library();
        engine.load_metadata();
        engine.connect();
        let settings = engine.metadata_config();

        Ok(Self {
            engine,
            events,
            page: Page::Library,
            library: Library::default(),
            shares: Vec::new(),
            thumbs: ImageCache::default(),
            posters: ImageCache::default(),
            metadata: HashMap::new(),
            episodes: HashMap::new(),
            settings,
            api_key: String::new(),
            matching: false,
            matcher: None,
            search: String::new(),
            focus_search: false,
            shortcuts_open: false,
            osd: None,
            view: LibraryView::default(),
            loaded: false,
            form: ShareForm::default(),
            playback: None,
            overlay: ui::player::Overlay::default(),
            up_next: UpNext::default(),
            warmed: 0,
            auto_skipped: (0, Vec::new()),
            fullscreen: false,
            opening: None,
            connecting: true,
            crawling: false,
            crawl_progress: Default::default(),
            window,
            downloads: Vec::new(),
            offline_files: std::collections::HashSet::new(),
            confirm_partial_delete: None,
            confirm_remove_share: None,
            toasts: ui::toast::Toasts::default(),
            pacer: Pacer::new(),
        })
    }

    /// Say over the picture what a control just did. Only on the player page;
    /// anywhere else the transport bar already shows it.
    fn flash(&mut self, ctx: &egui::Context, text: impl Into<String>) {
        if self.page == Page::Player {
            self.osd = Some((text.into(), ctx.input(|input| input.time)));
        }
    }

    fn note(&mut self, ctx: &egui::Context, text: impl Into<String>, error: bool) {
        let now = ctx.input(|input| input.time);
        self.toasts.push(now, text, error);
    }

    /// Drain everything the background side has said since the last frame.
    ///
    /// Takes `frame` because starting a player is one of the things that can
    /// come out of this channel, and building the mpv render context needs
    /// eframe's OpenGL context — which is current here and nowhere else. See
    /// [`crate::playback`].
    fn pump(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Shares(shares) => self.shares = shares,
                Event::ShareAdded(name) => {
                    self.form = ShareForm::default();
                    self.note(ctx, format!("added {name}"), false);
                }
                Event::ShareRejected(error) => {
                    // Nothing was crawled, so nothing will say it finished.
                    self.crawling = false;
                    self.form.sending = false;
                    self.form.error = Some(error);
                }
                Event::Connected { failures } => {
                    self.connecting = false;
                    if let Some(text) = describe_failures(&failures) {
                        self.note(ctx, text, true);
                    }
                }
                Event::ConnectFailed(error) => {
                    self.connecting = false;
                    self.note(ctx, format!("could not open the shares: {error}"), true);
                }
                Event::LibraryLoaded(library) => {
                    self.library = library;
                    self.loaded = true;
                    self.view.stale = true;
                }
                Event::Crawled {
                    share_id,
                    nodes,
                    files,
                    seconds,
                } => {
                    let name = self.share_name(&share_id);
                    self.crawl_progress.remove(&name);
                    self.note(
                        ctx,
                        format!("{name}: {files} playable of {nodes} nodes in {seconds:.0}s"),
                        false,
                    );
                }
                Event::CrawlFinished => {
                    self.crawling = false;
                    self.crawl_progress.clear();
                }
                Event::CrawlProgress { share, found } => {
                    self.crawling = true;
                    self.crawl_progress.insert(share, found);
                }
                Event::CrawlStopped { share_id } => {
                    let name = self.share_name(&share_id);
                    self.crawl_progress.remove(&name);
                    self.note(ctx, format!("stopped crawling {name}"), false);
                }
                Event::Thumbnail { key, image } => self.thumbs.insert(key, image),
                Event::ThumbnailMissing { key } => self.thumbs.mark_missing(key),
                Event::Poster { key, image } => self.posters.insert(key, image),
                Event::PosterMissing { key } => self.posters.mark_missing(key),
                Event::Metadata(records) => {
                    // Artwork keyed by a title whose record changed may now
                    // point somewhere else, and a title that was a miss may now
                    // have a poster to ask for — so those are forgotten. Only
                    // those: clearing the lot sent every tile in the library
                    // back to its initials after each match run, to fade the
                    // very same picture in again.
                    for key in changed_art(&self.metadata, &records) {
                        self.posters.forget(&key);
                    }
                    self.metadata = records;
                }
                Event::EpisodeMetadata(episodes) => self.episodes = episodes,
                Event::MetadataConfig(config) => {
                    self.settings = config;
                    self.posters.clear();
                    // Turning enrichment off, or switching provider, clears the
                    // stored answers — including these, which would otherwise
                    // go on naming episodes after the provider that named them
                    // was dropped.
                    self.episodes.clear();
                }
                Event::Matched {
                    matched,
                    unmatched,
                    failed,
                } => {
                    let mut text = format!("matched {matched}, no match for {unmatched}");
                    if failed > 0 {
                        // Worth its own clause: a failure is not a miss, and
                        // those titles will be asked about again.
                        text.push_str(&format!(", {failed} could not be looked up"));
                    }
                    self.note(ctx, text, failed > 0);
                }
                Event::MatchFinished => self.matching = false,
                // Both of these are addressed to a dialog, and the viewer may
                // have closed it or opened another one while the request was
                // out — so an answer for a title that is not the open one is
                // dropped rather than shown under the wrong heading.
                Event::MatchOptions { title_key, options } => {
                    if let Some(matcher) = &mut self.matcher
                        && matcher.title_key == title_key
                    {
                        matcher.results = options;
                        matcher.searching = false;
                        matcher.asked = true;
                        matcher.error = None;
                    }
                }
                Event::MatchSearchFailed { title_key, error } => {
                    if let Some(matcher) = &mut self.matcher
                        && matcher.title_key == title_key
                    {
                        matcher.searching = false;
                        matcher.error = Some(error);
                    }
                }
                Event::PlaybackReady { target, stream } => {
                    self.opening = None;
                    // Dropped before the new one is built: two mpv cores would
                    // fight over the ring and the bandwidth, and the old one's
                    // render context has to be freed on this thread anyway.
                    //
                    // Both halves of this are blocking calls into mpv on the UI
                    // thread — destroying a core waits for its demuxer, and
                    // creating one compiles shaders — so both are timed. If the
                    // window ever stops answering while episodes change, this is
                    // the line that says which half did it.
                    let swap = std::time::Instant::now();
                    self.playback = None;
                    let torn_down = swap.elapsed();
                    let gl = frame.gl().cloned();
                    let started = Playback::start(&self.engine, *target, stream, gl.as_ref());
                    if swap.elapsed() >= STALL {
                        tracing::warn!(
                            "swapping players held the ui thread for {} ms, {} ms of it \
                             destroying the previous mpv core",
                            swap.elapsed().as_millis(),
                            torn_down.as_millis()
                        );
                    }
                    match started {
                        Ok(playback) => {
                            self.overlay = ui::player::Overlay::default();
                            self.page = Page::Player;
                            self.playback = Some(playback);
                        }
                        Err(error) => self.note(ctx, error, true),
                    }
                }
                // Only from the player currently on the bar: one being replaced
                // goes on reporting for a moment after its successor started.
                Event::Player { id, event } => {
                    let mine = self.playback.as_ref().is_some_and(|p| p.id == id);
                    if let Some(playback) = self.playback.as_mut().filter(|p| p.id == id) {
                        playback.apply(&event);
                    }
                    // Played to the end, rather than stopped or failed: the one
                    // ending that means "and now the next one".
                    if mine
                        && matches!(
                            event,
                            pstr_player::PlayerEvent::EndFile(pstr_player::EndReason::Eof)
                        )
                    {
                        self.autoplay_next(ctx);
                    }
                }
                Event::PlayerStopped { id } => {
                    if self.playback.as_ref().is_some_and(|p| p.id == id) {
                        // Dropping this frees the mpv render context, which has
                        // to happen on this thread with the GL context current.
                        // `pump` is called from `ui`, which is exactly that.
                        let target = self.playback.take().map(|p| p.target.title_key.clone());
                        // Nothing to show on the player page any more. Back to
                        // where the click came from, rather than a black window
                        // — unless the next episode is already opening, which is
                        // exactly where the viewer wants to stay.
                        if self.page == Page::Player && self.opening.is_none() {
                            self.page = match target {
                                Some(key) => Page::Title(key),
                                None => Page::Library,
                            };
                        }
                    }
                }
                Event::Watched {
                    share_id,
                    link_id,
                    state,
                } => {
                    if self.library.set_watch(&share_id, &link_id, state) {
                        self.view.stale = true;
                    }
                }
                Event::Error(text) => {
                    self.opening = None;
                    self.note(ctx, text, true);
                }
                Event::Status(text) => self.note(ctx, text, false),
                Event::Downloads(downloads) => self.downloads = downloads,
                Event::DownloadProgress(item) => {
                    if let Some(row) = self.downloads.iter_mut().find(|row| row.key == item.key) {
                        *row = *item;
                    }
                }
                Event::OfflineFiles(files) => self.offline_files = files,
            }
        }
        // Pictures become textures a few per frame; the rest wait for the next.
        let thumbs_waiting = self.thumbs.upload(ctx);
        let posters_waiting = self.posters.upload(ctx);
        if thumbs_waiting || posters_waiting {
            ctx.request_repaint();
        }
    }

    fn share_name(&self, id: &str) -> String {
        self.shares
            .iter()
            .find(|share| share.id == id)
            .map(|share| share.name.clone())
            .unwrap_or_else(|| id.to_string())
    }

    /// The file before or after what is playing, within its own title.
    ///
    /// `None` at either end, and for a title that has been recrawled out from
    /// under the player — both are "there is nothing to step to", which is all
    /// the caller does with it.
    pub fn adjacent(&self, direction: Adjacent) -> Option<PlaybackTarget> {
        let target = &self.playback.as_ref()?.target;
        let title = self.library.get(&target.title_key)?;
        let episode = match direction {
            Adjacent::Previous => title.preceding(&target.share_id, &target.link_id),
            Adjacent::Next => title.following(&target.share_id, &target.link_id),
        }?;
        Some(PlaybackTarget::new(title, episode))
    }

    /// What the provider calls the episode this target is, if anything.
    fn episode_name(&self, target: &PlaybackTarget) -> Option<String> {
        let number = target.number?;
        self.episodes
            .get(&target.title_key)?
            .get(target.season, number)?
            .name
            .clone()
    }

    /// Whether there is anything to step to in either direction, for the
    /// controls that offer it.
    fn neighbours(&self) -> ui::transport::Neighbours {
        ui::transport::Neighbours {
            previous: self.adjacent(Adjacent::Previous).is_some(),
            next: self.adjacent(Adjacent::Next).is_some(),
        }
    }

    /// Advance the end-of-episode countdown, and say what the card should show.
    ///
    /// Called once a frame from the player page, *before* it is drawn, because
    /// drawing does not mutate — the card is told how many seconds are left and
    /// nothing more. Returns `None` whenever there is nothing to offer, which
    /// is nearly always.
    ///
    /// Four things have to hold before a viewer is interrupted, and each of
    /// them is a way this has gone wrong elsewhere: the file has to actually be
    /// in its credits, there has to be a next episode to go to, autoplay has to
    /// be on, and playback has to be *running* — a paused episode is one
    /// somebody walked away from, and coming back to the next one already
    /// playing is the opposite of helpful.
    fn tick_up_next(
        &mut self,
        ctx: &egui::Context,
        actions: &mut Vec<Action>,
    ) -> Option<UpNextCard> {
        let has_next = self.neighbours().next;
        let autoplay = self.engine.playback_prefs().autoplay_next;
        let next = self.adjacent(Adjacent::Next);

        let Some(playback) = &self.playback else {
            self.up_next = UpNext::default();
            return None;
        };
        if self.up_next.playback_id != playback.id {
            self.up_next = UpNext {
                playback_id: playback.id,
                ..UpNext::default()
            };
        }

        let counting = playback.loaded
            && !playback.paused
            && playback.in_credits()
            && has_next
            && autoplay
            && !self.up_next.dismissed;
        if !counting {
            self.up_next.started = None;
            return None;
        }

        let now = ctx.input(|input| input.time);
        let started = *self.up_next.started.get_or_insert(now);
        let left = UP_NEXT_SECONDS - (now - started);
        if left <= 0.0 {
            // Latched, not cleared: the next file takes a moment to open, and
            // without this the countdown would fire again on every frame of
            // that moment.
            self.up_next.dismissed = true;
            actions.push(Action::PlayAdjacent(Adjacent::Next));
            return None;
        }

        // Nothing else causes a frame while a film plays and the mouse is
        // still, and a countdown that only ticks when the pointer moves is
        // worse than none.
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
        let still = next.as_ref().and_then(|target| {
            let title = self.library.get(&target.title_key)?;
            let episode = title.episodes().find(|episode| {
                episode.node.share_id == target.share_id && episode.node.link_id == target.link_id
            })?;
            ui::Art {
                engine: &self.engine,
                thumbs: &mut self.thumbs,
                posters: &mut self.posters,
                metadata: &self.metadata,
                episodes: &self.episodes,
            }
            .still(&title.key, episode)
        });
        Some(UpNextCard {
            seconds: left,
            left: (left / UP_NEXT_SECONDS) as f32,
            caption: next.map(|target| target.caption()).unwrap_or_default(),
            still,
        })
    }

    /// Near the end of a file, fetch the opening blocks of the next one.
    ///
    /// The last two minutes, or the credits if the chapters say where they
    /// are — whichever comes first. Late enough that a viewer who stops after
    /// one episode has not cost a download, early enough that the swap starts
    /// from the cache.
    fn warm_next(&mut self) {
        /// How close to the end counts as "about to want the next one".
        const LEAD_SECONDS: f64 = 120.0;

        let Some(playback) = &self.playback else {
            return;
        };
        if playback.id == self.warmed || !playback.loaded {
            return;
        }
        let near_end = playback
            .duration
            .is_some_and(|duration| duration - playback.position < LEAD_SECONDS);
        if !(near_end || playback.in_credits()) {
            return;
        }
        self.warmed = playback.id;
        if let Some(next) = self.adjacent(Adjacent::Next) {
            self.engine.warm(&next);
        }
    }

    /// Seek past an opening or credits on arrival, when the viewer asked for
    /// that.
    ///
    /// Once per chapter per file. A viewer who skipped back into the opening
    /// wants to hear it.
    fn auto_skip(&mut self, ctx: &egui::Context) {
        let Some(playback) = &self.playback else {
            return;
        };
        if !playback.loaded || playback.paused || playback.seeking {
            return;
        }
        let Some((label, end)) = playback.skippable() else {
            return;
        };
        let Some(index) = playback.chapter().map(|chapter| chapter.index) else {
            return;
        };
        if self.auto_skipped.0 != playback.id {
            self.auto_skipped = (playback.id, Vec::new());
        }
        if self.auto_skipped.1.contains(&index) || !self.engine.playback_prefs().auto_skip {
            return;
        }
        self.auto_skipped.1.push(index);
        playback.send(crate::playback::Command::SeekTo(end));
        // Said, because a jump nobody asked for this second reads as a glitch.
        let what = label.trim_start_matches("Skip ").to_lowercase();
        self.note(ctx, format!("skipped the {what}"), false);
    }

    /// Start the next episode, if there is one and the viewer wants it.
    ///
    /// Called on a clean end of file only. A file that failed to load has an
    /// end too, and auto-advancing through a broken share one episode at a
    /// time is how a player earns being turned off.
    fn autoplay_next(&mut self, ctx: &egui::Context) {
        if !self.engine.playback_prefs().autoplay_next {
            return;
        }
        let Some(target) = self.adjacent(Adjacent::Next) else {
            return;
        };
        self.note(ctx, format!("next: {}", target.caption()), false);
        self.apply(ctx, Action::Play(target));
    }

    fn send_player(&self, command: crate::playback::Command) {
        if let Some(playback) = &self.playback {
            playback.send(command);
        }
    }

    /// Set the volume everywhere it is kept: mpv, and — when the change is
    /// settled rather than mid-drag — the preferences the next player is built
    /// from.
    ///
    /// Moving the slider off zero unmutes, because a slider that visibly
    /// changes while nothing gets louder is a bug from the viewer's side.
    fn set_volume(&mut self, volume: f64, commit: bool) {
        let mut prefs = self.engine.playback_prefs();
        prefs.volume = volume;
        self.send_player(crate::playback::Command::SetVolume(volume));

        let muted = self.playback.as_ref().map_or(prefs.muted, |p| p.muted);
        if muted && volume > 0.0 {
            prefs.muted = false;
            self.send_player(crate::playback::Command::SetMuted(false));
        }
        self.engine.set_playback_prefs(prefs, commit);
    }

    fn apply(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::Goto(page) => self.page = page,
            Action::Play(mut target) => {
                // One player at a time. The old one is only *asked* to stop
                // here; it is dropped when its replacement is ready, so a click
                // that turns out not to open leaves the current film playing.
                if let Some(playback) = &self.playback {
                    playback.send(crate::playback::Command::Stop);
                }
                // The one place every route into playback passes through — a
                // click on a row, a keypress, autoplay — so the episode's name
                // is looked up here rather than in each of them.
                target.episode_name = self.episode_name(&target);
                target.track_prefs = self
                    .engine
                    .title_track_prefs(&target.title_key)
                    .map(Box::new);
                self.opening = Some(target.name.clone());
                self.engine.play(target);
            }
            Action::MakeOffline(targets) => {
                self.note(ctx, "downloading for offline use…", false);
                self.engine.make_offline(targets);
            }
            Action::PauseDownload(key) => self.engine.pause_download(&key),
            Action::ResumeDownload(key) => self.engine.resume_download(&key),
            Action::CancelDownload(key) => self.engine.cancel_download(&key),
            Action::RemoveDownload(key, remove_partial) => {
                if remove_partial {
                    self.confirm_partial_delete = Some(key);
                } else {
                    self.engine.remove_download(key, false);
                }
            }
            Action::SetMetadataConfig(config) => {
                self.settings = config.clone();
                self.engine.set_metadata_config(config);
            }
            Action::SetApiKey { provider, key } => self.engine.set_api_key(provider, key),
            Action::MatchTitles { force } => {
                if self.library.is_empty() {
                    self.note(ctx, "crawl a share first — there is nothing to match", true);
                } else {
                    self.matching = true;
                    self.engine.match_titles(self.library.titles.clone(), force);
                }
            }
            Action::OpenMatcher(key) => {
                self.matcher = self.library.get(&key).map(Matcher::new);
                // The library's own name is what the automatic search already
                // failed on, so the first search is the viewer's to press —
                // except that pressing it unchanged is exactly what someone
                // wants when the *scorer* was the problem rather than the name.
                // So it is sent, and the box stays theirs to edit.
                if self.matcher.is_some() {
                    self.apply(ctx, Action::SearchMatches);
                }
            }
            Action::CloseMatcher => self.matcher = None,
            Action::SearchMatches => {
                if let Some(matcher) = &mut self.matcher {
                    matcher.searching = true;
                    matcher.error = None;
                    self.engine.search_matches(
                        matcher.title_key.clone(),
                        matcher.query.clone(),
                        matcher.kind,
                    );
                }
            }
            Action::ChooseMatch(found) => {
                if let Some(title) = self
                    .matcher
                    .as_ref()
                    .and_then(|matcher| self.library.get(&matcher.title_key))
                {
                    self.engine.choose_match(title.clone(), *found);
                }
                self.matcher = None;
            }
            Action::ForgetMatch(key) => {
                self.engine.forget_match(key);
                self.matcher = None;
            }
            Action::ToggleFullscreen => {
                self.fullscreen = !self.fullscreen;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
            }
            Action::WatchToEnd => self.up_next.dismissed = true,
            Action::FocusSearch => {
                if !matches!(self.page, Page::Library | Page::Title(_)) {
                    self.page = Page::Library;
                }
                self.focus_search = true;
            }
            Action::ToggleShortcuts => self.shortcuts_open = !self.shortcuts_open,
            Action::SetShelf(filter, sort) => {
                self.view.filter = filter;
                self.view.sort = sort;
            }
            Action::Back => match self.page {
                Page::Title(_) => self.page = Page::Library,
                // Back out of a search before out of anything else: it is the
                // last thing the viewer did.
                Page::Library if !self.search.is_empty() => self.search.clear(),
                _ => {}
            },
            Action::LeavePlayer => {
                if self.fullscreen {
                    self.fullscreen = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                }
                self.page = match &self.playback {
                    Some(playback) => Page::Title(playback.target.title_key.clone()),
                    None => Page::Library,
                };
            }
            Action::Crawl(share) => {
                self.crawling = true;
                self.note(ctx, "crawling…", false);
                self.engine.crawl(share);
            }
            Action::StopCrawl => self.engine.stop_crawl(),
            Action::AddShare {
                name,
                url,
                password,
            } => {
                self.crawling = true;
                self.form.sending = true;
                self.form.error = None;
                self.engine.add_share(name, url, password);
            }
            Action::RemoveShare(id) => self.confirm_remove_share = Some(id),
            Action::ForgetShare(id) => {
                self.confirm_remove_share = None;
                self.thumbs.clear();
                self.engine.remove_share(id);
            }
            Action::SetWatched {
                share_id,
                link_id,
                watched,
                duration,
            } => {
                // Unwatching rewinds: a file marked unseen that still holds a
                // position would come back as "resume at 19:04".
                let position = if watched {
                    duration.unwrap_or(0.0)
                } else {
                    0.0
                };
                self.engine.save_watch_state(
                    share_id,
                    link_id,
                    watch_state(position, duration, watched),
                );
            }
            Action::Player(command) => {
                if let crate::playback::Command::SeekBy(seconds) = command {
                    self.flash(
                        ctx,
                        if seconds < 0.0 {
                            format!("− {:.0} s", -seconds)
                        } else {
                            format!("+ {seconds:.0} s")
                        },
                    );
                }
                if let Some(playback) = &self.playback {
                    playback.send(command);
                }
            }
            Action::PlayAdjacent(direction) => match self.adjacent(direction) {
                Some(target) => self.apply(ctx, Action::Play(target)),
                None => self.note(
                    ctx,
                    match direction {
                        Adjacent::Previous => "this is the first one",
                        Adjacent::Next => "this is the last one",
                    },
                    false,
                ),
            },
            Action::SetSpeed(speed) => {
                let mut prefs = self.engine.playback_prefs();
                prefs.speed = speed;
                self.engine.set_playback_prefs(prefs, true);
                let speed = self.engine.playback_prefs().speed;
                if let Some(playback) = &mut self.playback {
                    playback.speed = speed;
                    playback.send(crate::playback::Command::SetSpeed(speed));
                }
                let text = format!("Speed {}", ui::format_speed(speed));
                if self.page == Page::Player {
                    self.flash(ctx, text);
                } else {
                    self.note(ctx, text, false);
                }
            }
            Action::SetAutoSkip(auto_skip) => {
                let mut prefs = self.engine.playback_prefs();
                prefs.auto_skip = auto_skip;
                self.engine.set_playback_prefs(prefs, true);
            }
            Action::SetAutoplay(autoplay) => {
                let mut prefs = self.engine.playback_prefs();
                prefs.autoplay_next = autoplay;
                self.engine.set_playback_prefs(prefs, true);
            }
            Action::SetAppearance(appearance) => {
                self.engine.set_appearance(appearance);
                theme::apply(ctx, appearance);
            }
            Action::SetVolume { volume, commit } => {
                self.set_volume(volume, commit);
                // A slider shows its own value; a key press shows it here.
                if commit {
                    self.flash(ctx, format!("Volume {volume:.0}%"));
                }
            }
            Action::ToggleMute => {
                let mut prefs = self.engine.playback_prefs();
                // From what is playing, not from the preferences: mpv is the
                // one that knows, and a keypress in its own window changes it
                // without going through here.
                prefs.muted = match &self.playback {
                    Some(playback) => !playback.muted,
                    None => !prefs.muted,
                };
                self.send_player(crate::playback::Command::SetMuted(prefs.muted));
                self.flash(ctx, if prefs.muted { "Muted" } else { "Sound on" });
                self.engine.set_playback_prefs(prefs, true);
            }
            Action::SelectTrack { kind, id } => {
                self.send_player(crate::playback::Command::SelectTrack(kind, id));

                // Remember the language and the track's name, not its number:
                // the next episode is a different file, where track 3 may be a
                // commentary and Japanese may be track 2. The name is what
                // tells "Signs & Songs" from "Full Subtitles" in one language.
                let (language, title) = id
                    .and_then(|id| {
                        self.playback.as_ref().and_then(|playback| {
                            playback
                                .tracks_of(kind)
                                .find(|track| track.id == id)
                                .map(|track| (track.language.clone(), track.title.clone()))
                        })
                    })
                    .unwrap_or_default();
                if let Some(playback) = &self.playback {
                    let mut show = self
                        .engine
                        .title_track_prefs(&playback.target.title_key)
                        .unwrap_or_default();
                    match kind {
                        pstr_player::TrackKind::Audio => {
                            show.audio_language = language;
                            show.audio_title = title;
                        }
                        pstr_player::TrackKind::Subtitle => {
                            show.subtitles = id.is_some();
                            if id.is_some() {
                                show.subtitle_language = language;
                                show.subtitle_title = title;
                            }
                        }
                        pstr_player::TrackKind::Video => {}
                    }
                    self.engine
                        .set_title_track_prefs(playback.target.title_key.clone(), show);
                }
            }
        }
    }

    /// The bar across the top: where you are, and what to search.
    fn navigation(
        &self,
        ui: &mut egui::Ui,
        actions: &mut Vec<Action>,
        search: &mut String,
        focus_search: bool,
    ) {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(
                theme::Role::Heading
                    .rich("proton-stream")
                    .strong()
                    .color(theme::accent()),
            );
            ui.add_space(12.0);

            let on_library = matches!(self.page, Page::Library | Page::Title(_));
            let active = self
                .downloads
                .iter()
                .filter(|download| {
                    matches!(
                        download.state,
                        crate::engine::DownloadState::Queued
                            | crate::engine::DownloadState::Running
                            | crate::engine::DownloadState::Paused
                    )
                })
                .count();
            let downloads = if active == 0 {
                "Downloads".to_owned()
            } else {
                format!("Downloads ({active})")
            };
            let pages = [Page::Library, Page::Shares, Page::Downloads, Page::Settings];
            let clicked = ui::tabs(
                ui,
                ui.id().with("nav"),
                &[
                    ("Library", on_library),
                    ("Shares", self.page == Page::Shares),
                    (&downloads, self.page == Page::Downloads),
                    ("Settings", self.page == Page::Settings),
                ],
            );
            if let Some(index) = clicked {
                actions.push(Action::Goto(pages[index].clone()));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(4.0);
                if self.crawling {
                    if ui
                        .small_button("Stop")
                        .on_hover_text("Stop listing; shares already listed are still stored")
                        .clicked()
                    {
                        actions.push(Action::StopCrawl);
                    }
                    ui.add(egui::Spinner::new().size(16.0));
                    ui.label(ui::muted(crawl_label(&self.crawl_progress)));
                } else if self.matching {
                    ui.add(egui::Spinner::new().size(16.0));
                    ui.label(ui::muted("matching"));
                } else if self.connecting {
                    ui.add(egui::Spinner::new().size(16.0));
                    ui.label(ui::muted("connecting"));
                } else if ui
                    .button("Refresh")
                    .on_hover_text("Re-crawl every share")
                    .clicked()
                {
                    actions.push(Action::Crawl(None));
                }

                if on_library && !self.library.is_empty() {
                    ui.add_space(8.0);
                    ui::search_field(ui, search, focus_search);
                }
            });
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // Before the timer: this is the frame budget being spent waiting, not
        // the frame taking longer, and `FrameTimer` reports slow frames.
        self.pacer.wait();

        let _timer = FrameTimer::new(&self.page);
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        self.pump(ctx, frame);
        self.warm_next();
        self.auto_skip(ctx);
        self.window.observe(ctx);

        let mut actions: Vec<Action> = Vec::new();

        // A player page with nothing playing and nothing opening is a black
        // window with no way out. It should be unreachable — `pump` routes away
        // when playback stops — but the failure mode is bad enough to guard.
        if self.page == Page::Player && self.playback.is_none() && self.opening.is_none() {
            self.page = Page::Library;
        }
        self.engine.set_picture_shown(self.page == Page::Player);

        if self.page == Page::Player {
            self.overlay.observe(ctx);
            let state = self.playback.as_ref().map(|playback| PlayerKeys {
                volume: playback.volume,
                speed: playback.speed,
                skip_to: playback.skippable().map(|(_, end)| end),
            });
            shortcuts(ctx, self.fullscreen, state, &mut actions);

            // Before the draw, because drawing does not mutate — and it can
            // push an action of its own, which is why it takes the same list.
            let up_next = self.tick_up_next(ctx, &mut actions);
            let neighbours = self.neighbours();
            let App {
                playback,
                opening,
                overlay,
                ..
            } = self;
            // No panels: the controls are drawn over the picture, and a nav bar
            // above a film is the one thing every player agrees not to do.
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
                .show(ui, |ui| {
                    ui::player::show(
                        ui,
                        frame,
                        playback.as_mut(),
                        opening.as_deref(),
                        ui::player::Chrome {
                            overlay,
                            up_next,
                            neighbours,
                        },
                        &mut actions,
                    );
                });
            if let Some((text, at)) = &self.osd
                && !ui::player::osd(ctx, text, *at)
            {
                self.osd = None;
            }
            self.toasts.show(ctx, self.overlay.covered());

            for action in actions {
                self.apply(ctx, action);
            }
            return;
        }

        // Not under a dialog: Escape there closes the dialog, and should not
        // also leave the page behind it.
        let dialog = self.matcher.is_some()
            || self.confirm_partial_delete.is_some()
            || self.confirm_remove_share.is_some()
            || self.shortcuts_open;
        if !dialog {
            global_shortcuts(ctx, &self.page, &mut actions);
        }

        // Split the borrow up front: the pages get read-only state and a place
        // to put actions, which is what keeps them from mutating mid-draw.
        {
            let mut search = std::mem::take(&mut self.search);
            let neighbours = self.neighbours();
            let playback_prefs = self.engine.playback_prefs();
            let prefs = ui::settings::Prefs {
                autoplay: playback_prefs.autoplay_next,
                auto_skip: playback_prefs.auto_skip,
                appearance: self.engine.appearance(),
            };

            const NAV_MARGIN: egui::Vec2 = egui::vec2(14.0, 10.0);
            egui::Panel::top("nav")
                .resizable(false)
                // No fill: the bar is a gradient, and a `Frame` only takes a
                // colour. It is painted below, into a shape reserved before the
                // contents are laid out — which is the only point at which the
                // height they came to is known.
                .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(
                    NAV_MARGIN.x as i8,
                    NAV_MARGIN.y as i8,
                )))
                .show(ui, |ui| {
                    let shadow = ui.painter().add(egui::Shape::Noop);
                    let background = ui.painter().add(egui::Shape::Noop);
                    self.navigation(ui, &mut actions, &mut search, self.focus_search);
                    // Full width from the space the panel was given, height from
                    // the space its contents took.
                    let content = ui.min_rect();
                    let bar = egui::Rect::from_min_max(
                        egui::pos2(ui.max_rect().left(), content.top()),
                        egui::pos2(ui.max_rect().right(), content.bottom()),
                    )
                    .expand2(NAV_MARGIN);
                    ui.painter()
                        .set(background, theme::bar_shape(ui.ctx(), bar));
                    // Past the panel's own edge, or the half of the shadow that
                    // falls on the page below it — the only half worth drawing
                    // — is clipped away.
                    ui.painter().with_clip_rect(ui.ctx().viewport_rect()).set(
                        shadow,
                        theme::bar_shadow(true).as_shape(bar, egui::CornerRadius::ZERO),
                    );
                });
            self.search = search;
            self.focus_search = false;
            self.view.refresh(&self.library, &self.search);

            let App {
                engine,
                page,
                library,
                shares,
                thumbs,
                posters,
                metadata,
                episodes,
                settings,
                api_key,
                search,
                view,
                loaded,
                form,
                matcher,
                playback,
                opening,
                downloads,
                offline_files,
                confirm_partial_delete,
                confirm_remove_share,
                ..
            } = self;

            let mut covered = 0.0;
            if playback.is_some() || opening.is_some() {
                covered = egui::Panel::bottom("transport")
                    .resizable(false)
                    .frame(
                        egui::Frame::new()
                            .fill(theme::surface())
                            .inner_margin(egui::Margin::symmetric(16, 10)),
                    )
                    .show(ui, |ui| {
                        // Above the bar rather than below it: this one is at the
                        // bottom of the window, so the page it separates itself
                        // from is the one over it.
                        //
                        // Clipped to the strip it falls on, because a `Frame`
                        // paints its fill *under* its contents and a shadow
                        // added here would otherwise lay its opaque middle over
                        // the bar it is cast by.
                        let bar = ui.max_rect().expand2(egui::vec2(16.0, 10.0));
                        let above = egui::Rect::from_min_max(
                            egui::pos2(bar.left(), bar.top() - 16.0),
                            egui::pos2(bar.right(), bar.top()),
                        );
                        ui.painter()
                            .with_clip_rect(above)
                            .add(theme::bar_shadow(false).as_shape(bar, egui::CornerRadius::ZERO));
                        transport_panel(
                            ui,
                            playback.as_ref(),
                            opening.as_deref(),
                            neighbours,
                            &mut actions,
                        );
                    })
                    .response
                    .rect
                    .height();
            }

            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(theme::background())
                        .inner_margin(egui::Margin::symmetric(18, 12)),
                )
                .show(ui, |ui| {
                    // Reborrowed rather than moved: the matching dialog below is
                    // drawn from the same caches, and a moved `&mut` would leave
                    // nothing to draw it with.
                    let mut art = ui::Art {
                        engine,
                        thumbs: &mut *thumbs,
                        posters: &mut *posters,
                        metadata,
                        episodes,
                    };
                    match page {
                        Page::Library => ui::library::show(
                            ui,
                            &mut art,
                            ui::library::Shelves {
                                library,
                                view,
                                loaded: *loaded,
                                search,
                            },
                            &mut actions,
                        ),
                        Page::Title(key) => ui::title::show(
                            ui,
                            &mut art,
                            library,
                            key,
                            ui::title::OfflineView {
                                downloads,
                                files: offline_files,
                            },
                            &mut actions,
                        ),
                        Page::Shares => ui::shares::show(ui, shares, library, form, &mut actions),
                        Page::Settings => {
                            ui::settings::show(ui, settings, prefs, api_key, &mut actions)
                        }
                        Page::Downloads => ui::downloads::show(ui, downloads, &mut actions),
                        // Drawn above, without any of these panels.
                        Page::Player => {}
                    }
                });

            // Over everything, and after it: a modal takes the input the page
            // under it would otherwise get, and it can only do that for widgets
            // laid out after it claims the layer.
            if let Some(open) = matcher {
                let mut art = ui::Art {
                    engine,
                    thumbs,
                    posters,
                    metadata,
                    episodes,
                };
                ui::matcher::show(ctx, open, &mut art, settings.provider, &mut actions);
            }

            if let Some(key) = confirm_partial_delete.clone() {
                match ui::confirm(
                    ctx,
                    "partial",
                    "Delete the partial download?",
                    "What has been downloaded so far is discarded. The file in the share is \
                     not touched.",
                    "Delete",
                ) {
                    ui::Answer::Confirmed => {
                        engine.remove_download(key, true);
                        *confirm_partial_delete = None;
                    }
                    ui::Answer::Declined => *confirm_partial_delete = None,
                    ui::Answer::Pending => {}
                }
            }

            if let Some(id) = confirm_remove_share.clone() {
                let name = shares
                    .iter()
                    .find(|share| share.id == id)
                    .map_or(id.as_str(), |share| share.name.as_str());
                match ui::confirm(
                    ctx,
                    "remove-share",
                    &format!("Remove {name}?"),
                    "Its titles leave the library, along with what you have watched of them. \
                     Its downloads are deleted, and the link and password are removed from the \
                     keyring.",
                    "Remove",
                ) {
                    ui::Answer::Confirmed => actions.push(Action::ForgetShare(id)),
                    ui::Answer::Declined => *confirm_remove_share = None,
                    ui::Answer::Pending => {}
                }
            }
            if self.shortcuts_open && ui::shortcuts::show(ctx) {
                actions.push(Action::ToggleShortcuts);
            }
            self.toasts.show(ctx, covered);
        }

        for action in actions {
            self.apply(ctx, action);
        }
    }

    /// Save where playback got to before the window goes away. mpv is still
    /// running at this point, so this is the last chance to ask it.
    ///
    /// The player is then dropped *here* rather than left to the app's own
    /// destructor: dropping it frees the mpv render context, which has to
    /// happen while the OpenGL context is still current. eframe calls this
    /// immediately before tearing the painter down, which is the last moment
    /// that is true.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.window.save();
        if let Some(playback) = &self.playback {
            playback.stop_and_wait(std::time::Duration::from_secs(2));
        }
        self.playback = None;
        self.engine
            .shutdown_downloads(std::time::Duration::from_secs(3));
    }
}

/// What the top bar says while shares are being listed: one share by name,
/// several as a count, with the files found across all of them.
fn crawl_label(progress: &std::collections::BTreeMap<String, usize>) -> String {
    let found: usize = progress.values().sum();
    let which = match progress.len() {
        0 => return "crawling".to_owned(),
        1 => progress.keys().next().cloned().unwrap_or_default(),
        shares => format!("{shares} shares"),
    };
    if found == 0 {
        format!("crawling {which}")
    } else {
        format!("crawling {which} · {found} found")
    }
}

/// Titles whose tile art is not what it was.
///
/// Compared by URL: the same URL is the same picture, and anything else —
/// a different one, a new one, or none any more — has to be fetched or
/// dropped.
fn changed_art(
    before: &HashMap<String, MetadataRecord>,
    after: &HashMap<String, MetadataRecord>,
) -> Vec<String> {
    let url = |records: &HashMap<String, MetadataRecord>, key: &str| {
        records
            .get(key)
            .and_then(|record| record.metadata.as_ref())
            .and_then(|metadata| metadata.tile_art())
            .map(|(url, _)| url.to_owned())
    };
    let mut keys: Vec<String> = before
        .keys()
        .chain(after.keys())
        .filter(|key| url(before, key) != url(after, key))
        .cloned()
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// Keys every page but the player answers to.
///
/// Plain keys — `/`, `?` — only while nothing is being typed into, or a search
/// for "a/b" would jump to the search box halfway through it.
fn global_shortcuts(ctx: &egui::Context, page: &Page, actions: &mut Vec<Action>) {
    use egui::{Key, Modifiers};

    let typing = ctx.egui_wants_keyboard_input();
    let mut tab = None;
    ctx.input_mut(|input| {
        if input.consume_key(Modifiers::COMMAND, Key::F)
            || (!typing && input.key_pressed(Key::Slash))
        {
            actions.push(Action::FocusSearch);
        }
        if input.consume_key(Modifiers::NONE, Key::F5)
            || input.consume_key(Modifiers::COMMAND, Key::R)
        {
            actions.push(Action::Crawl(None));
        }
        if !typing && input.key_pressed(Key::Questionmark) {
            actions.push(Action::ToggleShortcuts);
        }
        if input.consume_key(Modifiers::ALT, Key::ArrowLeft)
            || input.pointer.button_pressed(egui::PointerButton::Extra1)
            || (!typing && matches!(page, Page::Title(_)) && input.key_pressed(Key::Escape))
            || (!typing && *page == Page::Library && input.key_pressed(Key::Backspace))
        {
            actions.push(Action::Back);
        }
        for (key, index) in [
            (Key::Num1, 0),
            (Key::Num2, 1),
            (Key::Num3, 2),
            (Key::Num4, 3),
        ] {
            if input.consume_key(Modifiers::COMMAND, key) {
                tab = Some(index);
            }
        }
        if input.consume_key(Modifiers::COMMAND, Key::Comma) {
            tab = Some(3);
        }
    });
    if let Some(index) = tab {
        let page = [Page::Library, Page::Shares, Page::Downloads, Page::Settings][index].clone();
        actions.push(Action::Goto(page));
    }
}

/// Keys the player page answers to.
///
/// Only there: elsewhere space is a button press and the arrows move between
/// widgets, and taking them would break the rest of the app to serve a page
/// that is not on screen.
/// What the player keys need to know about what is playing.
struct PlayerKeys {
    volume: f64,
    speed: f64,
    /// Where the skip button would go, while there is one.
    skip_to: Option<f64>,
}

/// The speeds `[` and `]` step through. Fixed steps rather than a nudge of a
/// tenth, because 1.0 has to be somewhere a viewer can land on again.
const SPEEDS: [f64; 8] = [0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 3.0];

/// The next step from `speed` in `direction`, from wherever it is now.
fn step_speed(speed: f64, faster: bool) -> f64 {
    if faster {
        SPEEDS
            .iter()
            .copied()
            .find(|&step| step > speed + 1e-6)
            .unwrap_or(SPEEDS[SPEEDS.len() - 1])
    } else {
        SPEEDS
            .iter()
            .rev()
            .copied()
            .find(|&step| step < speed - 1e-6)
            .unwrap_or(SPEEDS[0])
    }
}

fn shortcuts(
    ctx: &egui::Context,
    fullscreen: bool,
    playing: Option<PlayerKeys>,
    actions: &mut Vec<Action>,
) {
    use crate::playback::Command;
    use egui::Key;

    /// What one press of the volume keys is worth. mpv's own step, and small
    /// enough that holding the key is a ramp rather than a switch.
    const VOLUME_STEP: f64 = 5.0;

    let pressed: Vec<Key> = ctx.input(|input| {
        [
            Key::Space,
            Key::K,
            Key::ArrowLeft,
            Key::ArrowRight,
            Key::ArrowUp,
            Key::ArrowDown,
            Key::M,
            Key::N,
            Key::P,
            Key::F,
            Key::S,
            Key::OpenBracket,
            Key::CloseBracket,
            Key::Escape,
        ]
        .into_iter()
        .filter(|key| input.key_pressed(*key))
        .collect()
    });

    let volume = playing.as_ref().map_or(0.0, |playing| playing.volume);
    for key in pressed {
        match key {
            Key::Space | Key::K => actions.push(Action::Player(Command::TogglePause)),
            Key::ArrowLeft => actions.push(Action::Player(Command::SeekBy(-10.0))),
            Key::ArrowRight => actions.push(Action::Player(Command::SeekBy(30.0))),
            // Written straight to the preferences rather than debounced: a
            // keypress is one change, not a drag.
            Key::ArrowUp => actions.push(Action::SetVolume {
                volume: (volume + VOLUME_STEP).min(pstr_player::MAX_VOLUME),
                commit: true,
            }),
            Key::ArrowDown => actions.push(Action::SetVolume {
                volume: (volume - VOLUME_STEP).max(0.0),
                commit: true,
            }),
            Key::M => actions.push(Action::ToggleMute),
            Key::N => actions.push(Action::PlayAdjacent(Adjacent::Next)),
            Key::P => actions.push(Action::PlayAdjacent(Adjacent::Previous)),
            Key::F => actions.push(Action::ToggleFullscreen),
            Key::S => {
                if let Some(end) = playing.as_ref().and_then(|playing| playing.skip_to) {
                    actions.push(Action::Player(Command::SeekTo(end)));
                }
            }
            Key::OpenBracket | Key::CloseBracket => {
                if let Some(playing) = &playing {
                    actions.push(Action::SetSpeed(step_speed(
                        playing.speed,
                        key == Key::CloseBracket,
                    )));
                }
            }
            // Escape leaves fullscreen first and the page second, which is what
            // it does everywhere else and what stops one press from both
            // un-maximising the window and hiding the film.
            Key::Escape if fullscreen => actions.push(Action::ToggleFullscreen),
            Key::Escape => actions.push(Action::LeavePlayer),
            _ => {}
        }
    }
}

/// The transport bar, or the line that says a file is still opening.
fn transport_panel(
    ui: &mut egui::Ui,
    playback: Option<&Playback>,
    opening: Option<&str>,
    neighbours: ui::transport::Neighbours,
    actions: &mut Vec<Action>,
) {
    match playback {
        Some(playback) => ui::transport::mini(ui, playback, neighbours, actions),
        None => {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(ui::muted(format!(
                    "opening {}…",
                    opening.unwrap_or("the file")
                )));
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use pstr_core::library::TitleKind;
    use pstr_core::metadata::{ProviderId, TitleMetadata};

    use super::*;

    fn record(key: &str, poster: Option<&str>) -> (String, MetadataRecord) {
        let metadata = poster.map(|url| TitleMetadata {
            provider: ProviderId::AniList,
            remote_id: key.into(),
            name: key.into(),
            original_name: None,
            overview: None,
            year: None,
            kind: TitleKind::Series,
            poster_url: Some(url.into()),
            backdrop_url: None,
            rating: None,
            genres: Vec::new(),
            episodes: None,
            url: None,
        });
        (
            key.into(),
            MetadataRecord {
                title_key: key.into(),
                provider: ProviderId::AniList,
                metadata,
                fetched_at: 0,
                manual: false,
            },
        )
    }

    #[test]
    fn the_filters_split_the_library_by_kind() {
        use pstr_core::library::{Title, TitleKind};
        let title = |kind| Title {
            key: "k".into(),
            name: "n".into(),
            year: None,
            kind,
            seasons: Vec::new(),
            share_ids: Vec::new(),
        };
        let film = title(TitleKind::Film);
        let series = title(TitleKind::Series);
        assert!(Filter::All.admits(&film) && Filter::All.admits(&series));
        assert!(Filter::Films.admits(&film) && !Filter::Films.admits(&series));
        assert!(Filter::Series.admits(&series) && !Filter::Series.admits(&film));
        // Nothing watched of either: not started, and not being watched.
        assert!(Filter::Unwatched.admits(&film));
        assert!(!Filter::Watching.admits(&film));
    }

    #[test]
    fn the_crawl_label_names_one_share_and_counts_several() {
        let mut progress = std::collections::BTreeMap::new();
        assert_eq!(crawl_label(&progress), "crawling");
        progress.insert("anime".to_owned(), 0);
        assert_eq!(crawl_label(&progress), "crawling anime");
        progress.insert("films".to_owned(), 40);
        progress.insert("anime".to_owned(), 2);
        assert_eq!(crawl_label(&progress), "crawling 2 shares · 42 found");
    }

    #[test]
    fn speed_steps_land_back_on_normal() {
        assert_eq!(step_speed(1.0, true), 1.25);
        assert_eq!(step_speed(1.25, false), 1.0);
        // From somewhere between steps — a speed set on Android — to the
        // nearest step in the direction asked.
        assert_eq!(step_speed(1.1, true), 1.25);
        assert_eq!(step_speed(1.1, false), 1.0);
        assert_eq!(step_speed(3.0, true), 3.0);
        assert_eq!(step_speed(0.5, false), 0.5);
    }

    #[test]
    fn a_match_run_that_changes_nothing_keeps_every_poster() {
        let before: HashMap<_, _> = [record("a", Some("x")), record("b", None)].into();
        let after = before.clone();
        assert!(changed_art(&before, &after).is_empty());
    }

    #[test]
    fn only_titles_whose_art_moved_lose_their_poster() {
        let before: HashMap<_, _> = [
            record("same", Some("x")),
            record("moved", Some("old")),
            record("dropped", Some("y")),
            record("found", None),
        ]
        .into();
        let after: HashMap<_, _> = [
            record("same", Some("x")),
            record("moved", Some("new")),
            record("found", Some("z")),
            record("added", Some("w")),
        ]
        .into();
        assert_eq!(
            changed_art(&before, &after),
            vec!["added", "dropped", "found", "moved"]
        );
    }
}
