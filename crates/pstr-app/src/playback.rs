//! The player, driven from the UI without blocking it.
//!
//! [`pstr_player::Player`] is polled, not callback-driven: something has to sit
//! in `poll_event` for the life of the file. That cannot be the UI thread, so a
//! dedicated thread owns the `Player` outright — it is the only thing that ever
//! touches mpv — and the UI talks to it through a command channel and hears back
//! as [`Event::Player`].
//!
//! ```text
//!   ui thread  ──Command::Seek──▶  player thread  ──▶ mpv
//!       ▲            │                  │
//!       │            └─ VideoSurface ───┤ frames, on the UI thread's GL context
//!       └──── Event::Player(Position) ──┘   + watch state, every few seconds
//! ```
//!
//! The picture is drawn inside this window, which is why the `Player` is built
//! *here* rather than on the player thread: `mpv_render_context_create` has to
//! run on the thread whose OpenGL context it renders into, and it has to run
//! before the file is loaded, or mpv's video output has nowhere to send its
//! first frames. So the UI thread creates the player and its
//! [`crate::video::VideoSurface`], and only then hands the player to the thread
//! that will poll it. Both hold an `Arc`; mpv is destroyed when the last one
//! goes, which is always the surface, on the UI thread.
//!
//! Without a usable OpenGL context — which should not happen under `eframe`'s
//! glow backend, but is cheap to survive — the player falls back to letting mpv
//! open a window of its own, exactly as `pstr play` does.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, TryRecvError, channel};

use eframe::glow;
use pstr_core::catalog::CatalogNode;
use pstr_core::library::{Episode, Title};
use pstr_core::prefs::PlaybackPrefs;
use pstr_player::{
    Chapter, ChapterRole, EndReason, Player, PlayerConfig, PlayerEvent, Track, TrackKind,
    VideoOutput,
};
use pstr_stream::VideoStream;

use crate::engine::{Engine, Event, watch_state};
use crate::video::VideoSurface;

/// How often playback position is written to the catalog.
///
/// Every position event would be a write per frame-ish; only on shutdown would
/// lose the position when mpv is killed. Five seconds is the compromise, and it
/// is what the viewer loses at worst.
const SAVE_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

/// How long the player thread waits for mpv before checking for a command.
/// Short enough that a click on pause feels immediate.
const POLL_TIMEOUT: f64 = 0.05;

/// What is being played, and where it came from.
#[derive(Debug, Clone)]
pub struct PlaybackTarget {
    pub share_id: String,
    pub volume_id: String,
    pub link_id: String,
    /// The file name, for mpv's window title.
    pub name: String,
    /// The title this belongs to, for routing back to its page.
    pub title_key: String,
    /// What the transport bar shows.
    pub title_name: String,
    pub subtitle: String,
    /// The numbering the filename states, for looking the episode up.
    pub season: Option<u32>,
    pub number: Option<u32>,
    /// What the provider calls this episode, once someone has looked. Filled in
    /// by the app rather than here: this type is built in the pages, and the
    /// answers live next to the library.
    pub episode_name: Option<String>,
    /// Where to resume, when the viewer had been here before.
    pub resume_at: Option<f64>,
}

impl PlaybackTarget {
    /// The target for one episode of a title, resuming where it left off.
    pub fn new(title: &Title, episode: &Episode) -> Self {
        Self::from_node(title, &episode.node, episode.resume_at())
    }

    /// The target for one file, starting at `resume_at`.
    pub fn from_node(title: &Title, node: &CatalogNode, resume_at: Option<f64>) -> Self {
        Self {
            share_id: node.share_id.clone(),
            volume_id: node.volume_id.clone(),
            link_id: node.link_id.clone(),
            name: node.name.clone(),
            title_key: title.key.clone(),
            title_name: title.name.clone(),
            subtitle: Episode {
                node: node.clone(),
                watch: None,
            }
            .label(),
            season: node.parsed.season,
            number: node.parsed.episode,
            episode_name: None,
            resume_at,
        }
    }

    /// The line under the title while this plays: the numbering, and the
    /// episode's name when a provider has given one.
    pub fn caption(&self) -> String {
        match &self.episode_name {
            Some(name) if !self.subtitle.is_empty() => format!("{}  ·  {name}", self.subtitle),
            Some(name) => name.clone(),
            None => self.subtitle.clone(),
        }
    }
}

/// What the UI can ask the player to do.
#[derive(Debug, Clone, Copy)]
pub enum Command {
    TogglePause,
    SeekBy(f64),
    SeekTo(f64),
    /// 0–100.
    SetVolume(f64),
    SetMuted(bool),
    /// Play this track of that kind, or `None` for none of it.
    ///
    /// The player also takes the chosen track's language as the preference for
    /// every file it loads afterwards, so picking Japanese audio for one
    /// episode picks it for the next one too.
    SelectTrack(TrackKind, Option<i64>),
    Stop,
}

/// Identifies one player instance for the length of its file.
///
/// Starting a file while another is playing leaves two players alive for a
/// moment — the old one is still shutting down. Without an id its parting
/// events would land on the new one's transport bar and its `PlayerStopped`
/// would clear it.
static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// A running player, from the UI's side.
pub struct Playback {
    pub id: u64,
    commands: Sender<Command>,
    /// Set by the player thread once it has stopped mpv and written the final
    /// watch position.
    finished: Arc<AtomicBool>,
    /// Where mpv's frames land, when the picture is inside this window. `None`
    /// means mpv has a window of its own.
    pub video: Option<VideoSurface>,
    pub target: PlaybackTarget,
    pub position: f64,
    pub duration: Option<f64>,
    pub paused: bool,
    /// 0–100, as mpv has it.
    pub volume: f64,
    pub muted: bool,
    /// What the file contains. Empty until mpv has demuxed it.
    pub tracks: Vec<Track>,
    /// The file's chapters, in order. Empty for a file muxed without them.
    pub chapters: Vec<Chapter>,
    /// What each of those chapters is, resolved against the rest of the file —
    /// see [`pstr_player::roles`]. Same length as `chapters`, and recomputed
    /// whenever either the chapters or the duration change.
    pub roles: Vec<ChapterRole>,
    /// Where the run of credits and preview that ends the file begins, when it
    /// has one. What the "up next" countdown waits for.
    pub credits_at: Option<f64>,
    /// Set once mpv has demuxed the file. Before that there is nothing to seek
    /// within and no duration to show.
    pub loaded: bool,
    /// A seek was issued and the picture has not come back yet — the thing
    /// worth showing a spinner for on a link this slow.
    pub seeking: bool,
}

impl Playback {
    /// Build a player for `stream` and start it on its own thread.
    ///
    /// Returns immediately; the first frame lands when mpv gets to it. `gl` is
    /// eframe's context — with it the picture is drawn in this window, without
    /// it mpv opens its own.
    ///
    /// Must be called from the UI thread, inside the draw function: creating the
    /// render context needs `gl` to be current, and eframe only guarantees that
    /// around [`eframe::App::ui`].
    pub fn start(
        engine: &Engine,
        target: PlaybackTarget,
        stream: VideoStream,
        gl: Option<&Arc<glow::Context>>,
        ctx: &egui::Context,
    ) -> Result<Self, String> {
        let prefs = engine.playback_prefs();
        let (player, video) = build_player(engine, &target, &prefs, gl, ctx)?;

        let (commands, orders) = channel::<Command>();
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let finished = Arc::new(AtomicBool::new(false));
        let thread_engine = engine.clone();
        let thread_target = target.clone();
        let thread_finished = Arc::clone(&finished);
        let thread_player = Arc::clone(&player);

        std::thread::Builder::new()
            .name("pstr-player".into())
            .spawn(move || {
                run(
                    id,
                    thread_engine,
                    thread_target,
                    stream,
                    thread_player,
                    orders,
                    thread_finished,
                );
            })
            // Only fails when the OS refuses a thread, at which point the app has
            // larger problems than this file.
            .expect("spawn the player thread");

        Ok(Self {
            id,
            commands,
            finished,
            video,
            paused: false,
            volume: prefs.volume,
            muted: prefs.muted,
            tracks: Vec::new(),
            chapters: Vec::new(),
            roles: Vec::new(),
            credits_at: None,
            position: target.resume_at.unwrap_or(0.0),
            duration: None,
            loaded: false,
            seeking: false,
            target,
        })
    }

    /// Whether the picture is drawn in this window rather than mpv's own.
    pub fn is_embedded(&self) -> bool {
        self.video.is_some()
    }

    /// Ask the player to do something. A dead player is not an error — the
    /// window may simply have been closed a frame ago.
    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    /// The tracks of one kind, in the order the file lists them.
    pub fn tracks_of(&self, kind: TrackKind) -> impl Iterator<Item = &Track> {
        self.tracks.iter().filter(move |track| track.kind == kind)
    }

    /// Which track of a kind is playing, if any. `None` for subtitles that are
    /// off, which is an ordinary state rather than a missing answer.
    pub fn selected_track(&self, kind: TrackKind) -> Option<&Track> {
        self.tracks_of(kind).find(|track| track.selected)
    }

    /// The chapter the playhead is in, if the file has any.
    pub fn chapter(&self) -> Option<&Chapter> {
        let index = pstr_player::chapter_at(&self.chapters, self.position)?;
        self.chapters.get(index)
    }

    /// The one thing worth offering to skip right now: the opening, the ending
    /// or a next-episode preview the viewer is sitting in.
    ///
    /// Returns what the button should say and where it should seek to. `None`
    /// most of the time, which is what keeps it from being another permanent
    /// control on top of the picture. The role comes from
    /// [`pstr_player::roles`], not from the chapter's name alone, so a chapter
    /// called `Intro` that is ten minutes of story offers nothing.
    pub fn skippable(&self) -> Option<(&'static str, f64)> {
        let index = pstr_player::chapter_at(&self.chapters, self.position)?;
        let label = self.roles.get(index)?.skip_label()?;
        let end = pstr_player::chapter_end(&self.chapters, index, self.duration)?;
        // Nothing to skip once the end of it is behind us, which happens for a
        // last chapter whose "end" is the duration.
        (end > self.position + 0.5).then_some((label, end))
    }

    /// Whether the playhead is inside the run of credits and preview that ends
    /// the file — what "the episode is over" means when a file states its
    /// chapters.
    pub fn in_credits(&self) -> bool {
        self.credits_at
            .is_some_and(|start| self.position + f64::EPSILON >= start)
    }

    /// Re-read what the chapters are, now that either they or the duration have
    /// changed. Both are needed: the length of a chapter is what settles a name
    /// that could mean two things.
    fn reclassify(&mut self) {
        self.roles = pstr_player::roles(&self.chapters, self.duration);
        self.credits_at = pstr_player::credits_start(&self.chapters, &self.roles);
    }

    /// Fold an mpv event into what the transport bar shows.
    pub fn apply(&mut self, event: &PlayerEvent) {
        match event {
            PlayerEvent::FileLoaded => self.loaded = true,
            PlayerEvent::Volume(volume) => self.volume = *volume,
            PlayerEvent::Muted(muted) => self.muted = *muted,
            PlayerEvent::Tracks(tracks) => self.tracks = tracks.clone(),
            PlayerEvent::Chapters(chapters) => {
                self.chapters = chapters.clone();
                self.reclassify();
            }
            PlayerEvent::Position(position) => {
                self.position = *position;
                self.seeking = false;
            }
            PlayerEvent::Duration(duration) => {
                self.duration = Some(*duration);
                self.reclassify();
            }
            PlayerEvent::Paused(paused) => self.paused = *paused,
            PlayerEvent::Seek => self.seeking = true,
            PlayerEvent::PlaybackRestart => self.seeking = false,
            _ => {}
        }
    }

    /// Stop the player and wait, briefly, for it to write the final position.
    ///
    /// Called as the window closes: the runtime is about to go away with the
    /// process, so a save that has only been *spawned* may never run. The wait
    /// is bounded — a wedged mpv must not hold the app open.
    pub fn stop_and_wait(&self, timeout: std::time::Duration) {
        self.send(Command::Stop);
        let deadline = std::time::Instant::now() + timeout;
        while !self.finished.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Fraction played, for the seek bar. `None` until mpv reports a duration.
    pub fn progress(&self) -> Option<f32> {
        let duration = self.duration.filter(|duration| *duration > 0.0)?;
        Some((self.position / duration).clamp(0.0, 1.0) as f32)
    }
}

/// Create the mpv instance, and the surface it draws into when there is one.
///
/// The fallback is deliberate rather than defensive: if the render context
/// cannot be created — a driver without the GL version mpv needs, a context
/// eframe built in a way `pstr_player::VideoRenderer` cannot resolve entry
/// points for — the player is rebuilt for a window of its own. A film in the
/// wrong window beats an error message.
fn build_player(
    engine: &Engine,
    target: &PlaybackTarget,
    prefs: &PlaybackPrefs,
    gl: Option<&Arc<glow::Context>>,
    ctx: &egui::Context,
) -> Result<(Arc<Player>, Option<VideoSurface>), String> {
    let title = format!("proton-stream — {}", target.name);

    if let Some(gl) = gl {
        let config = PlayerConfig {
            video: VideoOutput::Embedded,
            window_title: title.clone(),
            // This window draws the transport and owns the keyboard; mpv's own
            // controller would be a second set of controls over the same file.
            on_screen_controller: false,
            default_keybindings: false,
            ..from_prefs(prefs)
        };

        match Player::new(engine.runtime().clone(), config) {
            Ok(player) => {
                let player = Arc::new(player);
                // SAFETY: called from `App::ui`, where eframe has made this
                // context current, and the surface is dropped from the same
                // place — see `video.rs`.
                match unsafe { VideoSurface::new(Arc::clone(&player), Arc::clone(gl), ctx.clone()) }
                {
                    Ok(video) => return Ok((player, Some(video))),
                    Err(error) => {
                        tracing::warn!("embed the video: {error}; falling back to a window");
                        // Dropping the player here destroys the mpv core that
                        // was configured for `vo=libmpv`, which would show
                        // nothing at all now that there is no render context.
                        drop(player);
                    }
                }
            }
            Err(error) => {
                tracing::warn!("start an embedded mpv: {error}; falling back to a window");
            }
        }
    }

    let config = PlayerConfig {
        window_title: title,
        ..from_prefs(prefs)
    };
    let player = Player::new(engine.runtime().clone(), config)
        .map_err(|error| format!("start mpv: {error}"))?;
    Ok((Arc::new(player), None))
}

/// The player defaults with the viewer's preferences folded in.
fn from_prefs(prefs: &PlaybackPrefs) -> PlayerConfig {
    PlayerConfig {
        volume: prefs.volume,
        muted: prefs.muted,
        audio_language: prefs.audio_language.clone(),
        subtitle_language: prefs.subtitle_language.clone(),
        subtitles: prefs.subtitles,
        ..PlayerConfig::default()
    }
}

/// The player thread: drives mpv, forwards its events, saves watch state.
///
/// It does not own the player — the UI thread's [`VideoSurface`] holds the other
/// `Arc`, and mpv is destroyed when that one goes. What this thread owns is the
/// *event loop*: `wait_event` has to be called from one place, for the length of
/// the file, and that place cannot be the thread that draws.
fn run(
    id: u64,
    engine: Engine,
    target: PlaybackTarget,
    stream: VideoStream,
    player: Arc<Player>,
    orders: std::sync::mpsc::Receiver<Command>,
    finished: Arc<AtomicBool>,
) {
    let handle = match player.play(stream) {
        Ok(handle) => handle,
        Err(error) => {
            engine.emit(Event::Error(format!("load {}: {error}", target.name)));
            finished.store(true, Ordering::Release);
            engine.emit(Event::PlayerStopped { id });
            return;
        }
    };

    let mut position = target.resume_at.unwrap_or(0.0);
    let mut duration: Option<f64> = None;
    let mut watched = false;
    let mut resumed = target.resume_at.is_none();
    let mut last_save = std::time::Instant::now();

    'playback: loop {
        match orders.try_recv() {
            Ok(command) => {
                if !apply_command(&engine, id, &player, command, position) {
                    break 'playback;
                }
                continue;
            }
            Err(TryRecvError::Empty) => {}
            // The UI dropped its end: it is not going to ask for anything else.
            // Not a reason to stop — mpv's own window may still be up, and if
            // the picture is embedded the surface is about to drop the player
            // anyway, which ends this loop through `Shutdown`.
            Err(TryRecvError::Disconnected) => {}
        }

        let Some(event) = player.poll_event(POLL_TIMEOUT) else {
            continue;
        };

        match &event {
            PlayerEvent::FileLoaded => {
                duration = player.duration();
                // The first moment there is a track list to read: mpv has
                // demuxed the file and made its own default selection, which
                // is what the menus have to open showing.
                emit_tracks(&engine, id, &player);
                // Only now is there a timeline to seek within.
                if !resumed && let Some(at) = target.resume_at {
                    resumed = true;
                    if let Err(error) = player.seek_to(at) {
                        tracing::warn!("resume at {at}: {error}");
                    }
                }
            }
            PlayerEvent::Duration(value) => duration = Some(*value),
            PlayerEvent::Position(value) => {
                position = *value;
                if last_save.elapsed() >= SAVE_EVERY {
                    last_save = std::time::Instant::now();
                    save(&engine, &target, position, duration, watched);
                }
            }
            // Played to the end: mark it seen, and store the duration as the
            // position so nothing offers to resume the last ten seconds.
            PlayerEvent::EndFile(EndReason::Eof) => {
                watched = true;
                position = duration.unwrap_or(position);
            }
            _ => {}
        }

        let stop = matches!(event, PlayerEvent::Shutdown)
            || matches!(
                event,
                PlayerEvent::EndFile(EndReason::Eof | EndReason::Failed)
            );

        engine.emit(Event::Player { id, event });
        if stop {
            break;
        }
    }

    // Synchronously, not spawned: the runtime may be going away with the
    // process a moment from now.
    engine.save_watch_state_now(
        &target.share_id,
        &target.link_id,
        &watch_state(position, duration, watched),
    );
    // The stream can go now. The player cannot: the UI thread's surface holds
    // the other `Arc`, and mpv must be destroyed there, on the thread whose
    // OpenGL context its render context was built against.
    drop(handle);
    drop(player);

    engine.release(
        target.share_id.clone(),
        target.volume_id.clone(),
        target.link_id.clone(),
    );
    finished.store(true, Ordering::Release);
    engine.emit(Event::PlayerStopped { id });
}

/// Returns whether playback should continue.
fn apply_command(
    engine: &Engine,
    id: u64,
    player: &Player,
    command: Command,
    position: f64,
) -> bool {
    let result = match command {
        Command::TogglePause => player
            .is_paused()
            .and_then(|paused| player.set_paused(!paused)),
        Command::SeekBy(delta) => player.seek_by(delta),
        Command::SeekTo(seconds) => player.seek_to(seconds),
        Command::SetVolume(volume) => player.set_volume(volume),
        Command::SetMuted(muted) => player.set_muted(muted),
        Command::SelectTrack(kind, track) => select_track(engine, id, player, kind, track),
        Command::Stop => {
            let _ = player.quit();
            return false;
        }
    };
    if let Err(error) = result {
        tracing::warn!("player command at {position:.0}s: {error}");
    }
    true
}

/// Switch track, then carry the choice forward to the files after this one.
///
/// The language is read back from mpv rather than taken from what the UI knew,
/// because between the click and here the selection is mpv's to interpret — and
/// a track with no language tag has to clear the preference rather than leave
/// the old one standing over a choice that contradicts it.
fn select_track(
    engine: &Engine,
    id: u64,
    player: &Player,
    kind: TrackKind,
    track: Option<i64>,
) -> pstr_player::Result<()> {
    player.select_track(kind, track)?;

    let tracks = player.tracks();
    let language = track
        .and_then(|track| {
            tracks
                .iter()
                .find(|candidate| candidate.kind == kind && candidate.id == track)
        })
        .and_then(|track| track.language.clone());
    if let Err(error) = player.prefer_language(kind, language.as_deref()) {
        tracing::warn!("prefer {} {language:?}: {error}", kind.label());
    }

    engine.emit(Event::Player {
        id,
        event: PlayerEvent::Tracks(tracks),
    });
    Ok(())
}

/// Tell the UI what the file contains and what of it is playing.
fn emit_tracks(engine: &Engine, id: u64, player: &Player) {
    engine.emit(Event::Player {
        id,
        event: PlayerEvent::Tracks(player.tracks()),
    });
    engine.emit(Event::Player {
        id,
        event: PlayerEvent::Chapters(player.chapters()),
    });
}

fn save(
    engine: &Engine,
    target: &PlaybackTarget,
    position: f64,
    duration: Option<f64>,
    watched: bool,
) {
    engine.save_watch_state(
        target.share_id.clone(),
        target.link_id.clone(),
        watch_state(position, duration, watched),
    );
}
