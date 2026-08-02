//! The table mpv looks a `pstr://` URL up in.
//!
//! mpv is given an opaque token, not an identity: a URL is `pstr://7`, never
//! `pstr://<share>/<link_id>`. Two reasons, both load-bearing.
//!
//! **The open callback must not do work.** Opening a Proton revision means link
//! details, an ancestor-key unlock and a revision listing — seconds, sometimes,
//! and it can fail for reasons a person needs to read (wrong password, revoked
//! link). mpv's open callback can only say *loading failed*. So the caller opens
//! the stream itself, in async code where the error survives, and registers the
//! result. What mpv's open callback then does is a hash lookup.
//!
//! **A share token is a secret.** The URL fragment of a Proton share link *is*
//! the decryption password. Putting it in a URL puts it in mpv's log at
//! `-v`, in the `path` property, and in the on-screen title. A `u64` leaks
//! nothing.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use pstr_stream::VideoStream;
use tokio::runtime::Runtime;

/// The protocol prefix registered with mpv.
pub const PROTOCOL: &str = "pstr";

/// Streams mpv can currently open, and the runtime their reads run on.
pub struct StreamRegistry {
    /// Held as a whole runtime, not a `Handle`: mpv's demuxer thread blocks on
    /// it from outside tokio, so it has to still exist when that thread runs. A
    /// `Handle` would not keep it alive, and a read after shutdown panics.
    runtime: Arc<Runtime>,
    next: AtomicU64,
    open: Mutex<HashMap<u64, VideoStream>>,
}

impl StreamRegistry {
    pub fn new(runtime: Arc<Runtime>) -> Arc<Self> {
        Arc::new(Self {
            runtime,
            next: AtomicU64::new(1),
            open: Mutex::new(HashMap::new()),
        })
    }

    /// Publish a stream and get back the handle that keeps it published.
    pub fn publish(self: &Arc<Self>, stream: VideoStream) -> StreamHandle {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.lock().insert(id, stream);
        StreamHandle {
            registry: Arc::clone(self),
            id,
        }
    }

    /// The stream behind a token, cloned so the caller does not hold the lock
    /// while mpv reads.
    pub(crate) fn lookup(&self, id: u64) -> Option<VideoStream> {
        self.lock().get(&id).cloned()
    }

    pub(crate) fn runtime(&self) -> &Arc<Runtime> {
        &self.runtime
    }

    /// How many streams are published. Diagnostics only.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, VideoStream>> {
        // Non-poisoning: a panic in an unrelated handler must not make every
        // later `pstr://` URL unopenable.
        self.open.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Keeps one stream openable by mpv. Dropping it revokes the URL.
///
/// Revoking does not disturb a playback already in progress — mpv holds a
/// cookie cloned at open time, and only a *re*-open would consult the registry.
/// It does mean the caller decides when a finished episode stops being
/// reachable, rather than leaving every stream it ever played addressable.
pub struct StreamHandle {
    registry: Arc<StreamRegistry>,
    id: u64,
}

impl StreamHandle {
    /// The URL to hand mpv.
    pub fn url(&self) -> String {
        format!("{PROTOCOL}://{}", self.id)
    }

    pub fn id(&self) -> u64 {
        self.id
    }
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        self.registry.lock().remove(&self.id);
    }
}

/// Pull the token out of a `pstr://` URL.
///
/// Deliberately strict — anything that is not exactly the shape we emit is a
/// miss, not a guess. mpv hands us whatever the user typed.
pub(crate) fn parse_url(uri: &str) -> Option<u64> {
    let rest = uri.strip_prefix(PROTOCOL)?.strip_prefix("://")?;
    // A trailing slash is what a URL parser would leave behind; nothing else.
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    rest.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testing::{test_runtime as runtime, test_stream};

    #[test]
    fn a_published_stream_is_reachable_through_its_url() {
        let registry = StreamRegistry::new(runtime());
        let handle = registry.publish(test_stream(registry.runtime()));

        let id = parse_url(&handle.url()).expect("url parses");
        assert_eq!(id, handle.id());
        assert!(registry.lookup(id).is_some());
    }

    #[test]
    fn dropping_the_handle_revokes_the_url() {
        let registry = StreamRegistry::new(runtime());
        let id = {
            let handle = registry.publish(test_stream(registry.runtime()));
            parse_url(&handle.url()).expect("url parses")
        };

        assert!(registry.lookup(id).is_none());
        assert!(registry.is_empty());
    }

    #[test]
    fn two_streams_get_distinct_urls() {
        let registry = StreamRegistry::new(runtime());
        let first = registry.publish(test_stream(registry.runtime()));
        let second = registry.publish(test_stream(registry.runtime()));

        assert_ne!(first.url(), second.url());
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn a_token_is_never_reused_after_the_stream_behind_it_is_gone() {
        // Otherwise a stale URL — mpv retrying a load, a playlist entry — would
        // silently start playing whatever took the number over.
        let registry = StreamRegistry::new(runtime());
        let first_url = {
            let handle = registry.publish(test_stream(registry.runtime()));
            handle.url()
        };
        let second = registry.publish(test_stream(registry.runtime()));

        assert_ne!(first_url, second.url());
        assert!(registry.lookup(parse_url(&first_url).unwrap()).is_none());
    }

    #[test]
    fn only_well_formed_pstr_urls_parse() {
        assert_eq!(parse_url("pstr://12"), Some(12));
        assert_eq!(parse_url("pstr://12/"), Some(12));
        assert_eq!(parse_url("pstr://"), None);
        assert_eq!(parse_url("pstr://abc"), None);
        assert_eq!(parse_url("pstr://12/extra"), None);
        assert_eq!(parse_url("file:///tmp/a.mkv"), None);
        assert_eq!(parse_url("pstr:12"), None);
    }
}
