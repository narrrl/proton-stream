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
use crate::playback::{Playback, PlaybackTarget};
use crate::ui::player::UpNextCard;
use crate::{theme, ui};

/// How long a status line stays up before it fades on its own.
const STATUS_SECONDS: f64 = 6.0;

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
    AddShare {
        name: String,
        url: String,
        password: Option<String>,
    },
    RemoveShare(String),
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
    /// Repaint the window in a different palette.
    SetAppearance(Appearance),
    /// In or out of fullscreen, from the player page.
    ToggleFullscreen,
    /// Stop watching, keep playing: back to the page this film came from, with
    /// the transport bar at the bottom still driving it.
    LeavePlayer,
    /// Call off the end-of-episode countdown and let this file play out.
    WatchToEnd,
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

/// A line at the bottom of the window, for a few seconds.
pub struct Status {
    pub text: String,
    pub error: bool,
    pub at: f64,
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

/// The share the viewer is typing in.
#[derive(Default)]
pub struct ShareForm {
    pub name: String,
    pub url: String,
    pub has_password: bool,
    pub password: String,
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
    pub form: ShareForm,
    pub playback: Option<Playback>,
    /// Whether the player page's controls are showing, and why.
    pub overlay: ui::player::Overlay,
    /// The end-of-episode countdown for whatever is playing.
    pub up_next: UpNext,
    /// Set while the window is fullscreen, so `F` can toggle rather than only
    /// ever entering. egui has no way to ask the platform.
    pub fullscreen: bool,
    /// The file being opened, if a click is still waiting on the network.
    pub opening: Option<String>,
    pub connecting: bool,
    pub crawling: bool,
    pub downloads: Vec<DownloadItem>,
    pub offline_files: std::collections::HashSet<DownloadKey>,
    pub confirm_partial_delete: Option<DownloadKey>,
    pub status: Option<Status>,
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
            form: ShareForm::default(),
            playback: None,
            overlay: ui::player::Overlay::default(),
            up_next: UpNext::default(),
            fullscreen: false,
            opening: None,
            connecting: true,
            crawling: false,
            downloads: Vec::new(),
            offline_files: std::collections::HashSet::new(),
            confirm_partial_delete: None,
            status: None,
        })
    }

    fn note(&mut self, ctx: &egui::Context, text: impl Into<String>, error: bool) {
        self.status = Some(Status {
            text: text.into(),
            error,
            at: ctx.input(|input| input.time),
        });
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
                Event::LibraryLoaded(library) => self.library = library,
                Event::Crawled {
                    share_id,
                    nodes,
                    files,
                    seconds,
                } => {
                    let name = self.share_name(&share_id);
                    self.note(
                        ctx,
                        format!("{name}: {files} playable of {nodes} nodes in {seconds:.0}s"),
                        false,
                    );
                }
                Event::CrawlFinished => self.crawling = false,
                Event::Thumbnail { key, image } => self.thumbs.insert(ctx, key, image),
                Event::ThumbnailMissing { key } => self.thumbs.mark_missing(key),
                Event::Poster { key, image } => self.posters.insert(ctx, key, image),
                Event::PosterMissing { key } => self.posters.mark_missing(key),
                Event::Metadata(records) => {
                    // Artwork keyed by a title whose record changed may now
                    // point somewhere else, and a title that was a miss may now
                    // have a poster to ask for.
                    self.posters.clear();
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
                    let started = Playback::start(&self.engine, *target, stream, gl.as_ref(), ctx);
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
                    // The position that was just saved is what the library
                    // should now show.
                    self.engine.load_library();
                }
                Event::Error(text) => {
                    self.opening = None;
                    self.note(ctx, text, true);
                }
                Event::Status(text) => self.note(ctx, text, false),
                Event::Downloads(downloads) => self.downloads = downloads,
                Event::OfflineFiles(files) => self.offline_files = files,
            }
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
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
        Some(UpNextCard {
            seconds: left,
            caption: next.map(|target| target.caption()).unwrap_or_default(),
        })
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
            Action::AddShare {
                name,
                url,
                password,
            } => {
                self.crawling = true;
                self.engine.add_share(name, url, password);
                self.form = ShareForm::default();
            }
            Action::RemoveShare(id) => {
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
                self.engine.load_library();
            }
            Action::Player(command) => {
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
            Action::SetAutoplay(autoplay) => {
                let mut prefs = self.engine.playback_prefs();
                prefs.autoplay_next = autoplay;
                self.engine.set_playback_prefs(prefs, true);
            }
            Action::SetAppearance(appearance) => {
                self.engine.set_appearance(appearance);
                theme::apply(ctx, appearance);
            }
            Action::SetVolume { volume, commit } => self.set_volume(volume, commit),
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
                self.engine.set_playback_prefs(prefs, true);
            }
            Action::SelectTrack { kind, id } => {
                self.send_player(crate::playback::Command::SelectTrack(kind, id));

                // Remember the *language*, not the track number: the next
                // episode is a different file, where track 3 may be a
                // commentary and Japanese may be track 2.
                let language = id.and_then(|id| {
                    self.playback.as_ref().and_then(|playback| {
                        playback
                            .tracks_of(kind)
                            .find(|track| track.id == id)
                            .and_then(|track| track.language.clone())
                    })
                });
                if let Some(playback) = &self.playback {
                    let mut show = self
                        .engine
                        .title_track_prefs(&playback.target.title_key)
                        .unwrap_or_default();
                    match kind {
                        pstr_player::TrackKind::Audio => show.audio_language = language,
                        pstr_player::TrackKind::Subtitle => {
                            show.subtitles = id.is_some();
                            if id.is_some() {
                                show.subtitle_language = language;
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
    fn navigation(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>, search: &mut String) {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("proton-stream")
                    .size(18.0)
                    .strong()
                    .color(theme::accent()),
            );
            ui.add_space(12.0);

            let on_library = matches!(self.page, Page::Library | Page::Title(_));
            if ui::tab(ui, on_library, "Library").clicked() {
                actions.push(Action::Goto(Page::Library));
            }
            if ui::tab(ui, self.page == Page::Shares, "Shares").clicked() {
                actions.push(Action::Goto(Page::Shares));
            }
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
            let label = if active == 0 {
                "Downloads".to_owned()
            } else {
                format!("Downloads ({active})")
            };
            if ui::tab(ui, self.page == Page::Downloads, &label).clicked() {
                actions.push(Action::Goto(Page::Downloads));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(4.0);
                if self.crawling {
                    ui.add(egui::Spinner::new().size(16.0));
                    ui.label(ui::muted("crawling"));
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
                    ui.add(
                        egui::TextEdit::singleline(search)
                            .hint_text("Search")
                            .desired_width(220.0),
                    );
                }
            });
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let _timer = FrameTimer::new(&self.page);
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        self.pump(ctx, frame);

        let mut actions: Vec<Action> = Vec::new();

        // A player page with nothing playing and nothing opening is a black
        // window with no way out. It should be unreachable — `pump` routes away
        // when playback stops — but the failure mode is bad enough to guard.
        if self.page == Page::Player && self.playback.is_none() && self.opening.is_none() {
            self.page = Page::Library;
        }

        if self.page == Page::Player {
            self.overlay.observe(ctx);
            let volume = self
                .playback
                .as_ref()
                .map_or(0.0, |playback| playback.volume);
            shortcuts(ctx, self.fullscreen, volume, &mut actions);

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

            for action in actions {
                self.apply(ctx, action);
            }
            return;
        }

        // Split the borrow up front: the pages get read-only state and a place
        // to put actions, which is what keeps them from mutating mid-draw.
        {
            let mut search = std::mem::take(&mut self.search);
            let neighbours = self.neighbours();
            let prefs = ui::shares::Prefs {
                autoplay: self.engine.playback_prefs().autoplay_next,
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
                    let background = ui.painter().add(egui::Shape::Noop);
                    self.navigation(ui, &mut actions, &mut search);
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
                });
            self.search = search;

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
                form,
                matcher,
                playback,
                opening,
                status,
                downloads,
                offline_files,
                confirm_partial_delete,
                ..
            } = self;

            if playback.is_some() || opening.is_some() {
                egui::Panel::bottom("transport")
                    .resizable(false)
                    .frame(
                        egui::Frame::new()
                            .fill(theme::surface())
                            .inner_margin(egui::Margin::symmetric(16, 10)),
                    )
                    .show(ui, |ui| {
                        transport_panel(
                            ui,
                            playback.as_ref(),
                            opening.as_deref(),
                            neighbours,
                            &mut actions,
                        );
                    });
            }

            if let Some(line) = status {
                let age = ctx.input(|input| input.time) - line.at;
                if age < STATUS_SECONDS {
                    egui::Panel::bottom("status")
                        .resizable(false)
                        .frame(
                            egui::Frame::new()
                                .fill(theme::background())
                                .inner_margin(egui::Margin::symmetric(16, 6)),
                        )
                        .show(ui, |ui| {
                            let colour = if line.error {
                                theme::danger()
                            } else {
                                theme::muted()
                            };
                            ui.label(egui::RichText::new(&line.text).size(12.0).color(colour));
                        });
                    // Repaint once the line is due to disappear, or it lingers
                    // until something else happens to cause a frame.
                    ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                        STATUS_SECONDS - age,
                    ));
                }
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
                        Page::Library => {
                            ui::library::show(ui, &mut art, library, search, &mut actions)
                        }
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
                        Page::Shares => ui::shares::show(
                            ui,
                            shares,
                            form,
                            settings,
                            prefs,
                            api_key,
                            &mut actions,
                        ),
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
                egui::Window::new("Delete partial download?")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                    .show(ctx, |ui| {
                        ui.label(
                            "This discards downloaded partial bytes. The online source is not changed.",
                        );
                        ui.horizontal(|ui| {
                            if ui.button("Keep partial").clicked() {
                                *confirm_partial_delete = None;
                            }
                            if ui.button("Delete partial").clicked() {
                                engine.remove_download(key.clone(), true);
                                *confirm_partial_delete = None;
                            }
                        });
                    });
            }
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
        if let Some(playback) = &self.playback {
            playback.stop_and_wait(std::time::Duration::from_secs(2));
        }
        self.playback = None;
        self.engine
            .shutdown_downloads(std::time::Duration::from_secs(3));
    }
}

/// Keys the player page answers to.
///
/// Only there: elsewhere space is a button press and the arrows move between
/// widgets, and taking them would break the rest of the app to serve a page
/// that is not on screen.
fn shortcuts(ctx: &egui::Context, fullscreen: bool, volume: f64, actions: &mut Vec<Action>) {
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
            Key::Escape,
        ]
        .into_iter()
        .filter(|key| input.key_pressed(*key))
        .collect()
    });

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
