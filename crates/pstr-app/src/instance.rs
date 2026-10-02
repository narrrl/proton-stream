//! One window at a time.
//!
//! A second launch hands what it was started with — a share link, or nothing
//! — to the window already open, and exits. Without this, a second click on
//! the launcher opened a second window on the same catalog: two engines
//! writing the same watch state, two crawls of the same shares, and two films
//! playing over each other.
//!
//! The channel between the two is a Unix socket in the user's runtime
//! directory, or a named pipe on Windows. Anything that goes wrong setting it
//! up is logged and the app runs on without it: a second window is a nuisance,
//! and a launch that refuses to start because of this is a bug.

use std::sync::mpsc::{Receiver, Sender, channel};

use pstr_core::config::AppDirs;

/// What a later launch asks of the window already open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Come to the front.
    Raise,
    /// Come to the front, with this share link in the Add form.
    AddShare(String),
}

impl Request {
    /// What a launch was started with: the first argument that is a share
    /// link, so `proton-stream 'https://drive.proton.me/urls/…#…'` adds it.
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Self {
        args.into_iter()
            .map(|arg| arg.trim().to_owned())
            .find(|arg| pstr_core::shares::share_token(arg).is_ok())
            .map_or(Self::Raise, Self::AddShare)
    }

    /// One line on the wire.
    fn encode(&self) -> String {
        match self {
            Self::Raise => "raise\n".to_owned(),
            Self::AddShare(url) => format!("add {url}\n"),
        }
    }

    /// The other end of [`Request::encode`]. Anything else is ignored rather
    /// than guessed at — and a link that is not a share link is not one to
    /// put in the form.
    fn decode(line: &str) -> Option<Self> {
        let line = line.trim();
        if line == "raise" {
            return Some(Self::Raise);
        }
        let url = line.strip_prefix("add ")?.trim();
        pstr_core::shares::share_token(url)
            .is_ok()
            .then(|| Self::AddShare(url.to_owned()))
    }
}

/// How long a launch spends handing its request over before giving up on it.
const HAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
/// The most a request may be. A share link is a few hundred bytes.
const MOST: u64 = 8 * 1024;

/// The outcome of [`claim`].
pub enum Claim {
    /// This is the window. The listener answers later launches.
    First(Listener),
    /// A window was already open and has the request. This launch is done.
    Handed,
}

/// Become the one window, or hand `request` to the one there is.
pub fn claim(runtime: &tokio::runtime::Runtime, dirs: &AppDirs, request: &Request) -> Claim {
    platform::claim(runtime, dirs, request)
}

/// Requests from later launches, until the process ends.
pub struct Listener {
    inner: Option<platform::Inner>,
}

impl Listener {
    /// Start answering. `first` is this launch's own request, which arrives
    /// through the same channel as the rest so there is one place to act on it.
    pub fn serve(
        self,
        runtime: &tokio::runtime::Runtime,
        ctx: &egui::Context,
        first: Request,
    ) -> Receiver<Request> {
        let (sender, receiver) = channel();
        if first != Request::Raise {
            let _ = sender.send(first);
        }
        if let Some(inner) = self.inner {
            platform::serve(
                inner,
                runtime,
                Deliver {
                    sender,
                    ctx: ctx.clone(),
                },
            );
        }
        receiver
    }
}

/// Where a received request goes: the app's queue, and a repaint so it is
/// acted on now rather than when the pointer next moves.
struct Deliver {
    sender: Sender<Request>,
    ctx: egui::Context,
}

impl Deliver {
    fn line(&self, text: &str) {
        let Some(request) = Request::decode(text) else {
            tracing::debug!("instance: ignored a request that did not parse");
            return;
        };
        let _ = self.sender.send(request);
        self.ctx.request_repaint();
    }
}

#[cfg(unix)]
mod platform {
    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;

    use super::{AppDirs, Claim, Deliver, HAND_TIMEOUT, Listener, MOST, Request};

    pub struct Inner(UnixListener);

    /// In the runtime directory where there is one: it is the user's alone and
    /// emptied at logout, which is exactly the life a socket should have.
    fn socket(dirs: &AppDirs) -> PathBuf {
        directories::BaseDirs::new()
            .and_then(|base| base.runtime_dir().map(std::path::Path::to_path_buf))
            .unwrap_or_else(|| dirs.cache.clone())
            .join("io.narl.proton-stream.sock")
    }

    pub fn claim(_: &tokio::runtime::Runtime, dirs: &AppDirs, request: &Request) -> Claim {
        let path = socket(dirs);
        if let Ok(mut stream) = UnixStream::connect(&path) {
            let _ = stream.set_write_timeout(Some(HAND_TIMEOUT));
            match stream.write_all(request.encode().as_bytes()) {
                Ok(()) => return Claim::Handed,
                Err(error) => tracing::warn!("instance: hand over to the open window: {error}"),
            }
        }
        // Nobody answered, so a socket file still there is left over from a
        // window that did not exit cleanly.
        let _ = std::fs::remove_file(&path);
        let listener = match UnixListener::bind(&path) {
            Ok(listener) => listener,
            Err(error) => {
                tracing::warn!("instance: listen at {}: {error}", path.display());
                return Claim::First(Listener { inner: None });
            }
        };
        // The runtime directory is private already; the cache fallback is
        // not necessarily, and a share link is a secret.
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        Claim::First(Listener {
            inner: Some(Inner(listener)),
        })
    }

    pub fn serve(inner: Inner, _: &tokio::runtime::Runtime, deliver: Deliver) {
        let spawned = std::thread::Builder::new()
            .name("instance".into())
            .spawn(move || {
                for stream in inner.0.incoming() {
                    let Ok(stream) = stream else { continue };
                    let _ = stream.set_read_timeout(Some(HAND_TIMEOUT));
                    let mut text = String::new();
                    if stream.take(MOST).read_to_string(&mut text).is_ok() {
                        deliver.line(&text);
                    }
                }
            });
        if let Err(error) = spawned {
            tracing::warn!("instance thread: {error}");
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::hash::{BuildHasher, Hasher};
    use std::io::Write;

    use tokio::io::AsyncReadExt;
    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

    use super::{AppDirs, Claim, Deliver, HAND_TIMEOUT, Listener, MOST, Request};

    pub struct Inner {
        name: String,
        first: NamedPipeServer,
    }

    /// The pipe's name. Pipe names are machine-wide, so a fixed one could be
    /// taken first by another account on the same machine, which would then
    /// be handed this user's share links. The name carries a random part kept
    /// in this user's own config directory, which nobody else can read.
    fn pipe(dirs: &AppDirs) -> String {
        let path = dirs.config.join("instance.key");
        let key = std::fs::read_to_string(&path)
            .ok()
            .map(|key| key.trim().to_owned())
            .filter(|key| key.len() == 32 && key.bytes().all(|b| b.is_ascii_hexdigit()))
            .unwrap_or_else(|| {
                // `RandomState` is seeded from the OS's random source, once
                // per process and stepped per instance after that.
                let mut key = String::new();
                for _ in 0..2 {
                    let word = std::collections::hash_map::RandomState::new()
                        .build_hasher()
                        .finish();
                    key.push_str(&format!("{word:016x}"));
                }
                if let Err(error) = std::fs::write(&path, &key) {
                    tracing::warn!("instance: store the pipe key: {error}");
                }
                key
            });
        format!(r"\\.\pipe\io.narl.proton-stream.{key}")
    }

    fn server(name: &str, first: bool) -> std::io::Result<NamedPipeServer> {
        ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .access_outbound(false)
            .create(name)
    }

    pub fn claim(runtime: &tokio::runtime::Runtime, dirs: &AppDirs, request: &Request) -> Claim {
        let name = pipe(dirs);
        // Registering a pipe with tokio needs its reactor.
        let _entered = runtime.enter();
        match server(&name, true) {
            Ok(first) => Claim::First(Listener {
                inner: Some(Inner { name, first }),
            }),
            // Taken: there is a window. Hand it the request.
            Err(_) => {
                let started = std::time::Instant::now();
                loop {
                    match std::fs::OpenOptions::new().write(true).open(&name) {
                        Ok(mut pipe) => {
                            if let Err(error) = pipe.write_all(request.encode().as_bytes()) {
                                tracing::warn!("instance: hand over to the open window: {error}");
                            }
                            return Claim::Handed;
                        }
                        // Every instance busy with another launch: wait a turn.
                        Err(_) if started.elapsed() < HAND_TIMEOUT => {
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                        Err(error) => {
                            tracing::warn!("instance: reach the open window: {error}");
                            return Claim::First(Listener { inner: None });
                        }
                    }
                }
            }
        }
    }

    pub fn serve(inner: Inner, runtime: &tokio::runtime::Runtime, deliver: Deliver) {
        runtime.spawn(async move {
            let Inner { name, mut first } = inner;
            loop {
                if first.connect().await.is_err() {
                    continue;
                }
                let connected = first;
                // The next instance is up before this one is read, so a launch
                // right behind this one finds a pipe to connect to.
                first = match server(&name, false) {
                    Ok(next) => next,
                    Err(error) => {
                        tracing::warn!("instance: reopen the pipe: {error}");
                        return;
                    }
                };
                let mut text = String::new();
                let mut limited = connected.take(MOST);
                let read = limited.read_to_string(&mut text);
                if let Ok(Ok(_)) = tokio::time::timeout(HAND_TIMEOUT, read).await {
                    deliver.line(&text);
                }
            }
        });
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    use super::{AppDirs, Claim, Deliver, Listener, Request};

    pub struct Inner;

    pub fn claim(_: &tokio::runtime::Runtime, _: &AppDirs, _: &Request) -> Claim {
        Claim::First(Listener { inner: None })
    }

    pub fn serve(_: Inner, _: &tokio::runtime::Runtime, _: Deliver) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK: &str = "https://drive.proton.me/urls/ABCDEFGHIJ#fragmentsecret";

    #[test]
    fn a_launch_with_a_share_link_asks_for_it_to_be_added() {
        let args = ["--verbose".to_owned(), format!("  {LINK} ")];
        assert_eq!(Request::from_args(args), Request::AddShare(LINK.to_owned()));
    }

    #[test]
    fn a_launch_with_no_share_link_only_raises_the_window() {
        let args = ["https://example.com/".to_owned()];
        assert_eq!(Request::from_args(args), Request::Raise);
        assert_eq!(Request::from_args(Vec::new()), Request::Raise);
    }

    #[test]
    fn a_request_survives_the_wire() {
        for request in [Request::Raise, Request::AddShare(LINK.to_owned())] {
            assert_eq!(Request::decode(&request.encode()), Some(request));
        }
    }

    #[test]
    fn a_line_that_is_not_a_request_is_ignored() {
        assert_eq!(Request::decode("add https://example.com/"), None);
        assert_eq!(Request::decode("open the pod bay doors"), None);
    }
}
