//! Desktop notifications, for things that finish while nobody is looking.
//!
//! Only when the window does not have focus: a viewer who is looking at the
//! app already has the toast, and a second copy of it from the panel is noise.
//! That rule is the caller's — see `App::notify`.
//!
//! Linux only, over `org.freedesktop.Notifications`. Windows wants an
//! application id registered by an installer before a toast will show, and a
//! notification that silently never appears is worse than none.

/// Sends notifications from a thread of its own, so a notification daemon
/// that is slow to answer never costs a frame.
pub struct Notifier {
    #[cfg(target_os = "linux")]
    orders: Option<std::sync::mpsc::Sender<(String, String)>>,
}

impl Notifier {
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            orders: linux::spawn(),
        }
    }

    pub fn send(&self, summary: impl Into<String>, body: impl Into<String>) {
        #[cfg(target_os = "linux")]
        if let Some(orders) = &self.orders {
            let _ = orders.send((summary.into(), body.into()));
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (summary.into(), body.into());
    }
}

impl Default for Notifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::mpsc::{Sender, channel};
    use std::time::Duration;

    use dbus::arg::PropMap;
    use dbus::blocking::Connection;

    pub fn spawn() -> Option<Sender<(String, String)>> {
        let (orders, receiver) = channel::<(String, String)>();
        std::thread::Builder::new()
            .name("notify".into())
            .spawn(move || {
                // Connected on first use rather than at startup: most sessions
                // never send one.
                let mut connection: Option<Connection> = None;
                for (summary, body) in receiver {
                    if connection.is_none() {
                        connection = Connection::new_session()
                            .inspect_err(|error| tracing::debug!("notifications: {error}"))
                            .ok();
                    }
                    let Some(connection) = &connection else {
                        continue;
                    };
                    let proxy = connection.with_proxy(
                        "org.freedesktop.Notifications",
                        "/org/freedesktop/Notifications",
                        Duration::from_secs(2),
                    );
                    let answer: Result<(u32,), _> = proxy.method_call(
                        "org.freedesktop.Notifications",
                        "Notify",
                        (
                            "proton-stream",
                            0u32,
                            "io.narl.proton-stream",
                            summary,
                            body,
                            Vec::<String>::new(),
                            PropMap::new(),
                            -1i32,
                        ),
                    );
                    if let Err(error) = answer {
                        tracing::debug!("notification: {error}");
                    }
                }
            })
            .inspect_err(|error| tracing::debug!("notify thread: {error}"))
            .ok()?;
        Some(orders)
    }
}
