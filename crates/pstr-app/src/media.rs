//! The desktop's view of what is playing: media keys, the media widget in the
//! panel, the lock screen.
//!
//! MPRIS on Linux, the System Media Transport Controls on Windows, through
//! `souvlaki`. Both are optional in the strongest sense — a session with no
//! D-Bus, or a desktop with nothing listening, plays exactly as before — so
//! every failure here is a debug line and nothing more.

use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};

use crate::app::{Action, Adjacent};
use crate::playback::{Command, Playback};

/// How far one press of a seek key moves, when the desktop does not say.
const SEEK_SECONDS: f64 = 10.0;

/// What was last told to the desktop, so it is told again only on a change.
#[derive(Debug, Clone, PartialEq)]
struct Told {
    /// What the metadata was built from: the player, whether its duration was
    /// known yet, and the cover. Any of them changing means sending it again.
    item: Option<(u64, bool, Option<String>)>,
    paused: bool,
    /// Where the playhead was, to the second. A desktop interpolates between
    /// updates, so this only has to be sent when the playhead jumps.
    second: i64,
}

pub struct MediaSession {
    controls: Option<MediaControls>,
    events: Receiver<MediaControlEvent>,
    told: Option<Told>,
}

impl MediaSession {
    /// Register with the desktop. `window` is the handle SMTC needs on
    /// Windows; elsewhere it is not used.
    pub fn new(ctx: &egui::Context, window: Option<*mut std::ffi::c_void>) -> Self {
        let (sender, events) = channel();
        let config = PlatformConfig {
            display_name: "proton-stream",
            dbus_name: "proton_stream",
            hwnd: window,
        };
        let ctx = ctx.clone();
        let controls = MediaControls::new(config)
            .and_then(|mut controls| {
                controls.attach(move |event| {
                    // The press arrives on the D-Bus thread; it is acted on in
                    // the next frame, which this is what asks for.
                    let _ = sender.send(event);
                    ctx.request_repaint();
                })?;
                Ok(controls)
            })
            .inspect_err(|error| tracing::debug!("media controls unavailable: {error:?}"))
            .ok();
        Self {
            controls,
            events,
            told: None,
        }
    }

    /// Turn the presses since the last frame into actions.
    pub fn drain(&mut self, playback: Option<&Playback>, actions: &mut Vec<Action>) {
        while let Ok(event) = self.events.try_recv() {
            if let Some(action) = action_for(event, playback) {
                actions.push(action);
            }
        }
    }

    /// Tell the desktop what is playing, if that changed since the last frame.
    ///
    /// `cover` is a `file://` URL to the title's artwork when it is on disk.
    pub fn show(&mut self, playback: Option<&Playback>, cover: Option<&str>) {
        let Some(controls) = self.controls.as_mut() else {
            return;
        };
        let told = Told {
            item: playback.map(|playback| {
                (
                    playback.id,
                    playback.duration.is_some(),
                    cover.map(str::to_owned),
                )
            }),
            paused: playback.is_none_or(|playback| playback.paused),
            second: playback.map_or(0, |playback| playback.position as i64),
        };
        let previous = self.told.replace(told.clone());
        // Within the second, or one second on from it: playback carrying on,
        // which the desktop already shows without being told.
        if let Some(previous) = &previous
            && previous.item == told.item
            && previous.paused == told.paused
            && (0..=1).contains(&(told.second - previous.second))
        {
            return;
        }

        let Some(playback) = playback else {
            if let Err(error) = controls.set_playback(MediaPlayback::Stopped) {
                tracing::debug!("media controls: {error:?}");
            }
            return;
        };
        if previous.is_none_or(|previous| previous.item != told.item) {
            let target = &playback.target;
            let episode = target.caption();
            let metadata = MediaMetadata {
                title: Some(if episode.is_empty() {
                    &target.title_name
                } else {
                    &episode
                }),
                album: Some(&target.title_name),
                artist: None,
                cover_url: cover,
                duration: playback.duration.map(Duration::from_secs_f64),
            };
            if let Err(error) = controls.set_metadata(metadata) {
                tracing::debug!("media controls: {error:?}");
            }
        }
        let progress = Some(MediaPosition(Duration::from_secs_f64(
            playback.position.max(0.0),
        )));
        let state = if playback.paused {
            MediaPlayback::Paused { progress }
        } else {
            MediaPlayback::Playing { progress }
        };
        if let Err(error) = controls.set_playback(state) {
            tracing::debug!("media controls: {error:?}");
        }
    }
}

/// What a press means here. `None` for the ones with nothing to act on.
fn action_for(event: MediaControlEvent, playback: Option<&Playback>) -> Option<Action> {
    let playback = playback?;
    let seek = |direction: SeekDirection, seconds: f64| {
        Action::Player(Command::SeekBy(match direction {
            SeekDirection::Forward => seconds,
            SeekDirection::Backward => -seconds,
        }))
    };
    Some(match event {
        MediaControlEvent::Toggle => Action::Player(Command::TogglePause),
        MediaControlEvent::Play if playback.paused => Action::Player(Command::TogglePause),
        MediaControlEvent::Pause if !playback.paused => Action::Player(Command::TogglePause),
        MediaControlEvent::Next => Action::PlayAdjacent(Adjacent::Next),
        MediaControlEvent::Previous => Action::PlayAdjacent(Adjacent::Previous),
        MediaControlEvent::Stop => Action::LeavePlayer,
        MediaControlEvent::Seek(direction) => seek(direction, SEEK_SECONDS),
        MediaControlEvent::SeekBy(direction, by) => seek(direction, by.as_secs_f64()),
        MediaControlEvent::SetPosition(MediaPosition(at)) => {
            Action::Player(Command::SeekTo(at.as_secs_f64()))
        }
        _ => return None,
    })
}
