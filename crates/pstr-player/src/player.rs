//! An mpv instance that plays [`VideoStream`]s.
//!
//! Where the picture goes is one option — [`VideoOutput`] — and nothing else in
//! this file depends on it. mpv can put up a window of its own, which is what
//! `pstr play` still does and what validates the whole Proton → blocks →
//! decrypt → demux → decode chain with no UI in the way; or it can hand each
//! frame to a [`crate::VideoRenderer`] the caller draws inside its own OpenGL
//! surface, which is what the app does.

use std::ptr::NonNull;
use std::sync::Arc;

use libmpv2::events::{Event, PropertyData};
use libmpv2::{Format, Mpv};
use pstr_stream::VideoStream;
use tokio::runtime::Runtime;

use crate::chapters::{self, Chapter};
use crate::error::{Error, Result};
use crate::protocol;
use crate::registry::{StreamHandle, StreamRegistry};
use crate::tracks::{self, Track, TrackKind};

/// How many blocks the stream layer should read ahead when mpv is the consumer:
/// **none**.
///
/// [`pstr_stream::DEFAULT_READAHEAD_BLOCKS`] is tuned for a reader that has no
/// read-ahead of its own, which mpv is not — its demuxer cache already runs
/// [`PlayerConfig::readahead_seconds`] ahead of the picture, sequentially and
/// eagerly. Stacking a second speculative layer under that one does not add
/// buffer, it takes bandwidth away from the reads mpv is waiting on right now.
/// Measured cold, mid-file seek to 15:00 of a 761 MiB episode over a real link:
///
/// | blocks | first frame | seek resumed | blocks fetched |
/// |---:|---:|---:|---:|
/// | **0** | **2.5 s** | **1479 ms** | **12** |
/// | 6 | 3.7 s | 2225 ms | 21 |
/// | 12 | 5.5 s | 3581 ms | 31 |
/// | 24 | 7.7 s | 6122 ms | 42 |
///
/// Monotonic, and sustained playback holds realtime at 0 just as well. See
/// `docs/BUGS.md` B8.
pub const READAHEAD_BLOCKS: usize = 0;

/// Why playback of a file ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// Played to the end.
    Eof,
    /// Stopped, or a different file was loaded.
    Stopped,
    /// The player is quitting.
    Quit,
    /// mpv could not play it.
    Failed,
    Other,
}

impl From<libmpv2::EndFileReason> for EndReason {
    fn from(reason: libmpv2::EndFileReason) -> Self {
        use libmpv2::mpv_end_file_reason as raw;
        match reason {
            raw::Eof => Self::Eof,
            raw::Stop => Self::Stopped,
            raw::Quit => Self::Quit,
            raw::Error => Self::Failed,
            _ => Self::Other,
        }
    }
}

/// What the UI needs from mpv, with mpv's borrowed types resolved away.
///
/// Owned on purpose: `libmpv2::Event` borrows out of the event queue slot, which
/// mpv reuses on the next `wait_event`, so it cannot cross a channel or outlive
/// a poll.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayerEvent {
    /// mpv is going away — the window was closed, or `quit` was issued. The
    /// only event that must be acted on.
    Shutdown,
    StartFile,
    /// Demuxing succeeded: duration and tracks are readable from here.
    FileLoaded,
    EndFile(EndReason),
    /// A seek was issued. Playback has not resumed yet.
    Seek,
    /// Playback actually resumed, after a seek or after buffering. This, not
    /// [`Self::Seek`], is when a seek is visibly done.
    PlaybackRestart,
    Position(f64),
    Duration(f64),
    Paused(bool),
    /// Output volume, 0–100.
    Volume(f64),
    Muted(bool),
    /// What the file contains and what is playing of it.
    ///
    /// Not an mpv event: mpv reports a track list as a node property, and this
    /// is emitted by whoever polls the player after a load or a selection. It
    /// travels with the others because it lands in the same place.
    Tracks(Vec<Track>),
    /// The file's chapters. Emitted with the tracks, and for the same reason.
    Chapters(Vec<Chapter>),
    /// Anything not worth a variant. Ignorable.
    Other,
}

/// Where mpv puts the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoOutput {
    /// mpv creates and owns a window, draws into it, and handles its own input.
    /// Nothing else is needed to see a film.
    #[default]
    Window,
    /// mpv draws nothing on its own. Frames go to a [`crate::VideoRenderer`]
    /// the caller creates against its own OpenGL context and renders from its
    /// own draw loop.
    ///
    /// Two option changes come with this and neither is optional:
    /// `vo=libmpv`, which is the video output the render API attaches to, and
    /// `video-timing-offset=0`, without which
    /// [`crate::VideoRenderer::render`] blocks until the frame's display time
    /// and takes the caller's UI thread down to the video's frame rate.
    Embedded,
}

/// How the mpv instance is set up. The defaults are the ones a person would
/// expect from the `mpv` command, which libmpv does *not* apply on its own.
#[derive(Debug, Clone)]
pub struct PlayerConfig {
    /// Where the picture goes.
    pub video: VideoOutput,
    /// The window title, when mpv owns the window. Ignored when embedded.
    pub window_title: String,
    /// Hardware decode where the driver supports it. `auto-safe` rather than
    /// `auto`, which is the setting that avoids the known-broken combinations.
    pub hardware_decoding: bool,
    /// mpv's built-in on-screen controller — the seek bar. Off in libmpv by
    /// default; on here, because in step one it is the only UI there is.
    pub on_screen_controller: bool,
    /// mpv's usual keys: space, arrows, `q`. Also off in libmpv by default.
    pub default_keybindings: bool,
    /// How many seconds of media mpv buffers ahead of the playhead.
    ///
    /// This is the one that actually governs how far mpv's reads run ahead of
    /// what is on screen, and it is deliberately expressed in *seconds* rather
    /// than bytes: a byte budget means two minutes of buffer on a 4 Mbit/s
    /// episode and eight seconds of it on a 4K remux, which is exactly backwards
    /// from what either wants.
    pub readahead_seconds: f64,
    /// Hard ceiling on that buffer.
    ///
    /// Keep it comfortably under [`pstr_stream::DEFAULT_RING_BYTES`]. mpv's
    /// buffer and our block read-ahead are stacked — mpv reads this far ahead of
    /// the picture, and the block layer reads ahead of *that* — so the two
    /// together are the resident working set. Sized larger than the ring, the
    /// prefetch evicts its own blocks before the player reaches them, which is
    /// the same trap `docs/BUGS.md` B7 records one layer down.
    pub demuxer_max_bytes: Option<u64>,
    /// Start paused. Useful when the caller wants to seek to a resume position
    /// before the first frame.
    pub start_paused: bool,
    /// Output volume, 0–100. Clamped, because mpv will happily amplify past
    /// 100 and nothing in this app offers that.
    pub volume: f64,
    pub muted: bool,
    /// Which audio track to pick when the file offers a choice, as a language
    /// tag ("jpn"). `None` leaves mpv's own default — the container's.
    ///
    /// A preference rather than a selection on purpose: it is set once per
    /// player and applies to every file it goes on to load, which is what makes
    /// "Japanese audio" hold for the next episode as well as this one.
    pub audio_language: Option<String>,
    pub subtitle_language: Option<String>,
    /// Whether subtitles are shown at all. Off is a choice a viewer makes once
    /// and expects to keep, and no language preference can express it.
    pub subtitles: bool,
    /// Raw mpv options, applied last, so anything here wins.
    pub options: Vec<(String, String)>,
}

/// The loudest this player goes. mpv's own ceiling is `volume-max`, which
/// defaults higher; amplifying past the source is a good way to find out what
/// a codec's clipping sounds like.
pub const MAX_VOLUME: f64 = 100.0;

impl Default for PlayerConfig {
    fn default() -> Self {
        Self {
            video: VideoOutput::Window,
            window_title: "proton-stream".into(),
            hardware_decoding: true,
            on_screen_controller: true,
            default_keybindings: true,
            readahead_seconds: 30.0,
            demuxer_max_bytes: Some(48 * 1024 * 1024),
            start_paused: false,
            volume: MAX_VOLUME,
            muted: false,
            audio_language: None,
            subtitle_language: None,
            subtitles: true,
            options: Vec::new(),
        }
    }
}

/// An mpv instance wired to the block layer.
pub struct Player {
    /// Declared before `registry` so it is destroyed first: after this drops,
    /// no stream callback can be running.
    mpv: Mpv,
    registry: Arc<StreamRegistry>,
}

impl Player {
    /// Build an mpv instance with the `pstr://` protocol registered.
    ///
    /// `runtime` is where every read mpv makes will run. It has to be
    /// multi-threaded and it has to outlive the player, because mpv's demuxer
    /// thread blocks on it from outside tokio.
    pub fn new(runtime: Arc<Runtime>, config: PlayerConfig) -> Result<Self> {
        let registry = StreamRegistry::new(runtime);

        let mpv = Mpv::with_initializer(|init| {
            init.set_property("title", config.window_title.as_str())?;
            init.set_property(
                "hwdec",
                if config.hardware_decoding {
                    "auto-safe"
                } else {
                    "no"
                },
            )?;
            init.set_property("osc", config.on_screen_controller)?;
            init.set_property("input-default-bindings", config.default_keybindings)?;
            init.set_property("input-vo-keyboard", config.default_keybindings)?;
            match config.video {
                VideoOutput::Window => {
                    // Without a window there is nothing to see while audio-only
                    // or pre-roll frames are decoding, and the OSC has nowhere
                    // to draw.
                    init.set_property("force-window", true)?;
                }
                VideoOutput::Embedded => {
                    // The video output the render API attaches to. Set before
                    // initialisation, because mpv picks a vo once.
                    init.set_property("vo", "libmpv")?;
                    // Whatever the caller asked for, an embedded player must not
                    // conjure a second window.
                    init.set_property("force-window", false)?;
                    // Otherwise `mpv_render_context_render` blocks until the
                    // frame's display time — on the caller's UI thread, which
                    // would then run at the video's frame rate rather than the
                    // display's. See `render.rs`.
                    init.set_property("video-timing-offset", 0.0)?;
                }
            }
            // The cache is what turns our 4 MiB blocks into smooth playback:
            // without it every demuxer read would be a synchronous trip through
            // the block layer at exactly the wrong moment.
            init.set_property("cache", "yes")?;
            init.set_property("cache-secs", config.readahead_seconds)?;
            init.set_property("demuxer-readahead-secs", config.readahead_seconds)?;
            if let Some(bytes) = config.demuxer_max_bytes {
                init.set_property("demuxer-max-bytes", bytes as i64)?;
            }
            if config.start_paused {
                init.set_property("pause", true)?;
            }
            init.set_property("volume", config.volume.clamp(0.0, MAX_VOLUME))?;
            init.set_property("mute", config.muted)?;
            // `alang`/`slang` are how mpv is asked to *prefer* a language: it
            // picks the best match at load and falls back to the container's
            // default when the file has nothing in that language, which is the
            // behaviour that makes the preference survive a file that cannot
            // honour it.
            if let Some(language) = &config.audio_language {
                init.set_property("alang", language.as_str())?;
            }
            if let Some(language) = &config.subtitle_language {
                init.set_property("slang", language.as_str())?;
            }
            if !config.subtitles {
                init.set_property("sid", "no")?;
            }
            for (name, value) in &config.options {
                init.set_property(name.as_str(), value.as_str())?;
            }
            Ok(())
        })?;

        protocol::register(&mpv, &registry)?;

        let player = Self { mpv, registry };
        player.observe_playback_properties()?;
        Ok(player)
    }

    /// Ask mpv to report the handful of properties the UI draws from.
    fn observe_playback_properties(&self) -> Result<()> {
        self.mpv.observe_property("time-pos", Format::Double, 0)?;
        self.mpv.observe_property("duration", Format::Double, 0)?;
        self.mpv.observe_property("pause", Format::Flag, 0)?;
        // Volume and mute are observed rather than only written, because mpv
        // changes them on its own: a keypress in its own window, or the volume
        // it clamps a restored value to.
        self.mpv.observe_property("volume", Format::Double, 0)?;
        self.mpv.observe_property("mute", Format::Flag, 0)?;
        Ok(())
    }

    /// Start playing a stream.
    ///
    /// The returned handle keeps the URL resolvable; drop it once the file is
    /// done with. Dropping it does not interrupt playback already underway.
    pub fn play(&self, stream: VideoStream) -> Result<StreamHandle> {
        let handle = self.registry.publish(stream);
        self.mpv.command("loadfile", &[&handle.url()])?;
        Ok(handle)
    }

    /// Play anything else mpv can open: a local path, or one of its own
    /// pseudo-protocols.
    ///
    /// Not how the app plays a film — that goes through [`Self::play`], because
    /// the point of this crate is that mpv never learns a share's URL. What this
    /// is for is checking the parts of the pipeline that have nothing to do with
    /// Proton against something that always works: `av://lavfi:testsrc` renders
    /// a moving pattern with no file, no account and no network, which is what
    /// makes the render path testable at all.
    pub fn play_url(&self, url: &str) -> Result<()> {
        self.mpv.command("loadfile", &[url])?;
        Ok(())
    }

    /// Take the next event, waiting up to `timeout` seconds. `0.0` polls.
    ///
    /// Returns `None` when nothing happened. Errors from mpv are logged and
    /// reported as [`PlayerEvent::Other`] rather than ending the loop — a
    /// property that briefly has no value is normal, not fatal.
    pub fn poll_event(&self, timeout: f64) -> Option<PlayerEvent> {
        match self.mpv.wait_event(timeout)? {
            Ok(event) => Some(translate(event)),
            Err(error) => {
                tracing::debug!(%error, "mpv event carried an error");
                Some(PlayerEvent::Other)
            }
        }
    }

    pub fn set_paused(&self, paused: bool) -> Result<()> {
        self.mpv.set_property("pause", paused)?;
        Ok(())
    }

    pub fn is_paused(&self) -> Result<bool> {
        Ok(self.mpv.get_property("pause")?)
    }

    /// Current playback position in seconds, or `None` before there is one.
    pub fn position(&self) -> Option<f64> {
        self.mpv.get_property("time-pos").ok()
    }

    pub fn duration(&self) -> Option<f64> {
        self.mpv.get_property("duration").ok()
    }

    /// Seek to an absolute position, in seconds.
    pub fn seek_to(&self, seconds: f64) -> Result<()> {
        self.mpv
            .command("seek", &[&format!("{seconds}"), "absolute"])?;
        Ok(())
    }

    /// Seek by a signed number of seconds.
    pub fn seek_by(&self, seconds: f64) -> Result<()> {
        self.mpv
            .command("seek", &[&format!("{seconds}"), "relative"])?;
        Ok(())
    }

    /// Every track of the file that is open, with the playing ones marked.
    ///
    /// Empty before [`PlayerEvent::FileLoaded`]: there is no file to have
    /// tracks yet, which is not an error.
    pub fn tracks(&self) -> Vec<Track> {
        tracks::read(&self.mpv)
    }

    /// Every chapter of the file that is open, in order.
    ///
    /// Empty for a file muxed without them, which most are and anime releases
    /// are not.
    pub fn chapters(&self) -> Vec<Chapter> {
        chapters::read(&self.mpv)
    }

    /// Play the track with this id, or none of that kind at all.
    ///
    /// `None` turns the kind off — the only sensible thing for subtitles, and
    /// legal for audio too. The id is one of [`Self::tracks`]; mpv is told as a
    /// string because "no" and "3" go to the same property.
    pub fn select_track(&self, kind: TrackKind, id: Option<i64>) -> Result<()> {
        let value = match id {
            Some(id) => id.to_string(),
            None => "no".to_string(),
        };
        self.mpv.set_property(kind.property(), value.as_str())?;
        Ok(())
    }

    /// Prefer this language for a kind of track from the *next* file onwards.
    ///
    /// It does not re-pick a track in what is already playing — that is
    /// [`Self::select_track`] — which is exactly the split a viewer expects:
    /// choosing Japanese here changes this episode, and the next one starts in
    /// Japanese because of this.
    pub fn prefer_language(&self, kind: TrackKind, language: Option<&str>) -> Result<()> {
        let property = match kind {
            TrackKind::Audio => "alang",
            TrackKind::Subtitle => "slang",
            // There is no `vlang`, and nobody picks a video track by language.
            TrackKind::Video => return Ok(()),
        };
        self.mpv.set_property(property, language.unwrap_or(""))?;
        Ok(())
    }

    /// Output volume, 0–100.
    pub fn volume(&self) -> Option<f64> {
        self.mpv.get_property("volume").ok()
    }

    pub fn set_volume(&self, volume: f64) -> Result<()> {
        self.mpv
            .set_property("volume", volume.clamp(0.0, MAX_VOLUME))?;
        Ok(())
    }

    pub fn is_muted(&self) -> Result<bool> {
        Ok(self.mpv.get_property("mute")?)
    }

    pub fn set_muted(&self, muted: bool) -> Result<()> {
        self.mpv.set_property("mute", muted)?;
        Ok(())
    }

    pub fn stop(&self) -> Result<()> {
        self.mpv.command("stop", &[])?;
        Ok(())
    }

    pub fn quit(&self) -> Result<()> {
        self.mpv.command("quit", &[])?;
        Ok(())
    }

    /// Escape hatch for options and commands this type does not wrap.
    pub fn mpv(&self) -> &Mpv {
        &self.mpv
    }

    /// The raw mpv handle, for the render API.
    ///
    /// Only [`crate::VideoRenderer`] needs this, and only because
    /// `mpv_render_context_create` takes the core rather than anything
    /// libmpv2 wraps. The handle is valid for as long as this `Player` is.
    pub(crate) fn raw_handle(&self) -> NonNull<libmpv2_sys::mpv_handle> {
        self.mpv.ctx
    }

    /// The decoded video's size in pixels, once mpv has one.
    ///
    /// `None` before the file is demuxed, and for a file with no video at all.
    /// An embedded caller wants this to size the rectangle it gives mpv; a
    /// windowed one has no use for it.
    pub fn video_size(&self) -> Option<(i64, i64)> {
        let width: i64 = self.mpv.get_property("dwidth").ok()?;
        let height: i64 = self.mpv.get_property("dheight").ok()?;
        (width > 0 && height > 0).then_some((width, height))
    }

    /// Set any mpv property. Same escape hatch, typed.
    pub fn set_option(&self, name: &str, value: &str) -> Result<()> {
        self.mpv.set_property(name, value).map_err(Error::from)
    }
}

/// mpv's borrowed event to ours.
fn translate(event: Event<'_>) -> PlayerEvent {
    match event {
        Event::Shutdown => PlayerEvent::Shutdown,
        Event::StartFile => PlayerEvent::StartFile,
        Event::FileLoaded => PlayerEvent::FileLoaded,
        Event::EndFile(reason) => PlayerEvent::EndFile(reason.into()),
        Event::Seek => PlayerEvent::Seek,
        Event::PlaybackRestart => PlayerEvent::PlaybackRestart,
        Event::PropertyChange { name, change, .. } => match (name, change) {
            ("time-pos", PropertyData::Double(value)) => PlayerEvent::Position(value),
            ("duration", PropertyData::Double(value)) => PlayerEvent::Duration(value),
            ("pause", PropertyData::Flag(value)) => PlayerEvent::Paused(value),
            ("volume", PropertyData::Double(value)) => PlayerEvent::Volume(value),
            ("mute", PropertyData::Flag(value)) => PlayerEvent::Muted(value),
            _ => PlayerEvent::Other,
        },
        _ => PlayerEvent::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_end_reason_maps_to_something_the_ui_can_act_on() {
        use libmpv2::mpv_end_file_reason as raw;

        // Eof is "play the next episode"; the rest are not. Getting this
        // backwards would auto-advance on a failed load.
        assert_eq!(EndReason::from(raw::Eof), EndReason::Eof);
        assert_eq!(EndReason::from(raw::Stop), EndReason::Stopped);
        assert_eq!(EndReason::from(raw::Quit), EndReason::Quit);
        assert_eq!(EndReason::from(raw::Error), EndReason::Failed);
        assert_eq!(EndReason::from(raw::Redirect), EndReason::Other);
    }

    #[test]
    fn observed_properties_translate_to_typed_events() {
        assert_eq!(
            translate(Event::PropertyChange {
                name: "time-pos",
                change: PropertyData::Double(12.5),
                reply_userdata: 0,
            }),
            PlayerEvent::Position(12.5)
        );
        assert_eq!(
            translate(Event::PropertyChange {
                name: "pause",
                change: PropertyData::Flag(true),
                reply_userdata: 0,
            }),
            PlayerEvent::Paused(true)
        );
        // A property we observe arriving in an unexpected format must not be
        // read as a position of zero.
        assert_eq!(
            translate(Event::PropertyChange {
                name: "time-pos",
                change: PropertyData::Int64(3),
                reply_userdata: 0,
            }),
            PlayerEvent::Other
        );
    }
}
