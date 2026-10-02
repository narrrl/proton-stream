//! Desktop notifications, for things that finish while nobody is looking.
//!
//! Only when the window does not have focus: a viewer who is looking at the
//! app already has the toast, and a second copy of it from the panel is noise.
//! That rule is the caller's — see `App::notify`.
//!
//! Linux over `org.freedesktop.Notifications`; Windows as a toast. Nothing on
//! macOS yet.
//!
//! A Windows toast is shown under an application id, and Windows drops a toast
//! whose id it does not know without a word. The id is registered for the
//! current user under `HKCU\Software\Classes\AppUserModelId` — the same
//! registration the Windows App SDK makes for an app with no package — which
//! works for the MSI and the portable zip alike. If that write fails, the
//! toast goes out under PowerShell's id instead, which Windows always knows: a
//! notification that names the wrong program beats one that never appears.

use std::path::Path;

/// Sends notifications from a thread of its own, so a notification daemon
/// that is slow to answer never costs a frame.
pub struct Notifier {
    #[cfg(any(target_os = "linux", windows))]
    orders: Option<std::sync::mpsc::Sender<(String, String)>>,
}

impl Notifier {
    /// `cache` is where Windows is pointed for the toast's icon.
    pub fn new(cache: &Path) -> Self {
        let _ = cache;
        Self {
            #[cfg(target_os = "linux")]
            orders: linux::spawn(),
            #[cfg(windows)]
            orders: windows::spawn(cache.join("notification-icon.png")),
        }
    }

    pub fn send(&self, summary: impl Into<String>, body: impl Into<String>) {
        #[cfg(any(target_os = "linux", windows))]
        if let Some(orders) = &self.orders {
            let _ = orders.send((summary.into(), body.into()));
        }
        #[cfg(not(any(target_os = "linux", windows)))]
        let _ = (summary.into(), body.into());
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

#[cfg(windows)]
mod windows {
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{Sender, channel};

    use tauri_winrt_notification::Toast;

    /// The id toasts are shown under, and the key it is registered at.
    const APP_ID: &str = "io.narl.proton-stream";
    const ICON: &[u8] = include_bytes!("../assets/icon-256.png");

    pub fn spawn(icon: PathBuf) -> Option<Sender<(String, String)>> {
        let (orders, receiver) = channel::<(String, String)>();
        std::thread::Builder::new()
            .name("notify".into())
            .spawn(move || {
                // Registered on first use rather than at startup: most
                // sessions never send one.
                let mut app_id: Option<&str> = None;
                for (summary, body) in receiver {
                    let id = *app_id.get_or_insert_with(|| {
                        let _ = std::fs::write(&icon, ICON);
                        if register(&icon) {
                            APP_ID
                        } else {
                            Toast::POWERSHELL_APP_ID
                        }
                    });
                    let shown = Toast::new(id).title(&summary).text1(&body).show();
                    if let Err(error) = shown {
                        tracing::debug!("notification: {error}");
                    }
                }
            })
            .inspect_err(|error| tracing::debug!("notify thread: {error}"))
            .ok()?;
        Some(orders)
    }

    /// Tell Windows the app id's display name and icon. Per user, no rights
    /// needed, and written again every session so a moved install points at
    /// a live icon.
    fn register(icon: &Path) -> bool {
        use windows_sys::Win32::System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
            RegCreateKeyExW, RegSetValueExW,
        };

        fn wide(text: &str) -> Vec<u16> {
            text.encode_utf16().chain(std::iter::once(0)).collect()
        }

        let path = wide(&format!(r"Software\Classes\AppUserModelId\{APP_ID}"));
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: every pointer is to a live, NUL-terminated buffer or an out
        // parameter on this stack frame, and the key is closed before return.
        unsafe {
            let created = RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            );
            if created != 0 {
                tracing::debug!("register the notification id: error {created}");
                return false;
            }
            let icon = icon.to_string_lossy();
            let mut written = true;
            for (name, value) in [("DisplayName", "Proton Stream"), ("IconUri", icon.as_ref())] {
                let name = wide(name);
                let value = wide(value);
                let bytes = u32::try_from(value.len() * 2).unwrap_or(u32::MAX);
                let set =
                    RegSetValueExW(key, name.as_ptr(), 0, REG_SZ, value.as_ptr().cast(), bytes);
                written &= set == 0;
            }
            RegCloseKey(key);
            written
        }
    }
}
