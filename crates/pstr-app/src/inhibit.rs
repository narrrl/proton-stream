//! Keeping the screen on while a film plays.
//!
//! mpv does this itself for a window of its own (`stop-screensaver`), but here
//! the picture is drawn into the app's window through the render API, and mpv
//! has no window to hold the screensaver off for. So the app asks.
//!
//! Best effort, like the media controls: a desktop that does not answer means
//! the screen may blank during a film, which is what happened before this.

/// Holds the screensaver off while [`Inhibitor::set`] says so.
pub struct Inhibitor {
    held: bool,
    #[cfg(target_os = "linux")]
    orders: Option<std::sync::mpsc::Sender<bool>>,
}

impl Inhibitor {
    pub fn new() -> Self {
        Self {
            held: false,
            #[cfg(target_os = "linux")]
            orders: linux::spawn(),
        }
    }

    /// Hold or release. Called every frame; acts only on a change.
    pub fn set(&mut self, hold: bool) {
        if hold == self.held {
            return;
        }
        self.held = hold;
        #[cfg(target_os = "linux")]
        if let Some(orders) = &self.orders {
            let _ = orders.send(hold);
        }
        #[cfg(windows)]
        windows::set(hold);
    }
}

impl Default for Inhibitor {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Inhibitor {
    fn drop(&mut self) {
        self.set(false);
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::mpsc::{Sender, channel};
    use std::time::Duration;

    use dbus::Path;
    use dbus::arg::PropMap;
    use dbus::blocking::Connection;

    const TIMEOUT: Duration = Duration::from_secs(2);
    const REASON: &str = "Playing a video";

    /// What is holding the screensaver off, and how to let go of it.
    enum Hold {
        /// The desktop portal: released by closing the request it returned.
        Portal(Path<'static>),
        /// The older `org.freedesktop.ScreenSaver`: released by its cookie.
        ScreenSaver(u32),
    }

    /// A thread of its own, holding the session bus connection. The hold is
    /// tied to that connection — closing it releases the hold — so it has to
    /// outlive any one call, and a D-Bus call that stalls must not stall a
    /// frame.
    pub fn spawn() -> Option<Sender<bool>> {
        let (orders, receiver) = channel::<bool>();
        std::thread::Builder::new()
            .name("inhibit".into())
            .spawn(move || {
                let connection = match Connection::new_session() {
                    Ok(connection) => connection,
                    Err(error) => {
                        tracing::debug!("no session bus to inhibit the screensaver on: {error}");
                        return;
                    }
                };
                let mut hold = None;
                for wanted in receiver {
                    match (wanted, hold.take()) {
                        (true, None) => hold = acquire(&connection),
                        (false, Some(held)) => release(&connection, held),
                        (_, held) => hold = held,
                    }
                }
                if let Some(held) = hold {
                    release(&connection, held);
                }
            })
            .inspect_err(|error| tracing::debug!("inhibit thread: {error}"))
            .ok()?;
        Some(orders)
    }

    fn acquire(connection: &Connection) -> Option<Hold> {
        // The portal first: it is what works under Flatpak and on GNOME, which
        // does not implement the screensaver interface's `Inhibit`.
        let portal = connection.with_proxy(
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            TIMEOUT,
        );
        let mut options = PropMap::new();
        options.insert(
            "reason".into(),
            dbus::arg::Variant(Box::new(REASON.to_owned())),
        );
        // 8 is "idle": the screen blanking or locking for want of input.
        let answer: Result<(Path<'static>,), _> = portal.method_call(
            "org.freedesktop.portal.Inhibit",
            "Inhibit",
            ("", 8u32, options),
        );
        match answer {
            Ok((request,)) => return Some(Hold::Portal(request)),
            Err(error) => tracing::debug!("portal inhibit: {error}"),
        }

        let screensaver = connection.with_proxy(
            "org.freedesktop.ScreenSaver",
            "/org/freedesktop/ScreenSaver",
            TIMEOUT,
        );
        let answer: Result<(u32,), _> = screensaver.method_call(
            "org.freedesktop.ScreenSaver",
            "Inhibit",
            ("proton-stream", REASON),
        );
        match answer {
            Ok((cookie,)) => Some(Hold::ScreenSaver(cookie)),
            Err(error) => {
                tracing::debug!("screensaver inhibit: {error}");
                None
            }
        }
    }

    fn release(connection: &Connection, hold: Hold) {
        let result: Result<(), dbus::Error> = match hold {
            Hold::Portal(request) => connection
                .with_proxy("org.freedesktop.portal.Desktop", request, TIMEOUT)
                .method_call("org.freedesktop.portal.Request", "Close", ()),
            Hold::ScreenSaver(cookie) => connection
                .with_proxy(
                    "org.freedesktop.ScreenSaver",
                    "/org/freedesktop/ScreenSaver",
                    TIMEOUT,
                )
                .method_call("org.freedesktop.ScreenSaver", "UnInhibit", (cookie,)),
        };
        if let Err(error) = result {
            tracing::debug!("release the screensaver: {error}");
        }
    }
}

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::System::Power::{
        ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
    };

    pub fn set(hold: bool) {
        let flags = if hold {
            ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED
        } else {
            ES_CONTINUOUS
        };
        // SAFETY: takes flags by value and touches nothing of ours.
        unsafe {
            SetThreadExecutionState(flags);
        }
    }
}
