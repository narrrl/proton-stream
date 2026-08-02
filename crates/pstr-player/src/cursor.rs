//! The read/seek state behind one open `pstr://` URL.
//!
//! Deliberately a plain safe type with no mpv in it. Everything mpv's stream
//! callbacks actually *do* — advance a position, clamp a seek, turn a short read
//! into an EOF, honour a cancellation — lives here and is unit-tested against an
//! in-memory block source. [`crate::protocol`] is then only pointer plumbing,
//! which is the part that has to be read rather than tested.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pstr_stream::VideoStream;
use tokio::runtime::Runtime;
use tokio::sync::Notify;

/// What mpv's callbacks return on failure.
///
/// `read_fn` documents -1; `seek_fn` and `size_fn` want an `MPV_ERROR_*`.
pub(crate) const READ_ERROR: i64 = -1;
pub(crate) const SEEK_ERROR: i64 = libmpv2::mpv_error::Generic as i64;
pub(crate) const SIZE_UNKNOWN: i64 = libmpv2::mpv_error::Unsupported as i64;

/// One-way "this stream is going away".
///
/// mpv calls `cancel_fn` from a thread other than the demuxer thread, precisely
/// so it can interrupt a read that is already blocked. Without it, tearing down
/// a player — closing the window, playing something else — waits out whatever
/// block fetch was in flight, which is seconds over a cold network.
///
/// It latches, and nothing un-latches it. That matches mpv's own model: the
/// cancellation is tied to the stream instance's lifetime, not to an individual
/// operation, and `close_fn` follows it. A cursor that tried to re-arm itself
/// would be guessing at a contract mpv does not offer.
#[derive(Default)]
pub(crate) struct CancelLatch {
    flagged: AtomicBool,
    notify: Notify,
}

impl CancelLatch {
    pub(crate) fn cancel(&self) {
        self.flagged.store(true, Ordering::Release);
        // `notify_one`, not `notify_waiters`: it leaves a permit behind when no
        // one is waiting yet, which closes the window between a reader checking
        // the flag and actually parking on the notify.
        self.notify.notify_one();
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.flagged.load(Ordering::Acquire)
    }

    async fn cancelled(&self) {
        self.notify.notified().await;
    }
}

/// A position inside one [`VideoStream`], plus the runtime its reads run on.
pub(crate) struct StreamCursor {
    stream: VideoStream,
    /// mpv's demuxer thread is not a tokio thread, so it can block on this. The
    /// whole runtime is held rather than a `Handle` because a read arriving
    /// after the runtime shut down would panic, and mpv may well outlive the
    /// code that built it.
    runtime: Arc<Runtime>,
    size: u64,
    pos: u64,
}

impl StreamCursor {
    pub(crate) fn new(stream: VideoStream, runtime: Arc<Runtime>) -> Self {
        let size = stream.size();
        Self {
            stream,
            runtime,
            size,
            pos: 0,
        }
    }

    /// Total size, or [`SIZE_UNKNOWN`] if it does not fit mpv's `i64`.
    pub(crate) fn size(&self) -> i64 {
        i64::try_from(self.size).unwrap_or(SIZE_UNKNOWN)
    }

    /// Where the next read starts. mpv tracks this itself, so nothing outside
    /// the tests asks.
    #[cfg(test)]
    pub(crate) fn position(&self) -> u64 {
        self.pos
    }

    /// Move to an absolute offset, returning the new one.
    ///
    /// Seeking exactly to EOF is allowed — that is how a demuxer asks for the
    /// end — and reads there return 0. Past it is an error rather than a clamp,
    /// so a mistake upstream shows up as a failure instead of as bytes from the
    /// wrong place.
    pub(crate) fn seek(&mut self, offset: i64) -> i64 {
        let Ok(target) = u64::try_from(offset) else {
            return SEEK_ERROR;
        };
        if target > self.size {
            return SEEK_ERROR;
        }
        self.pos = target;
        offset
    }

    /// Fill `buf` from the current position, advancing it.
    ///
    /// Returns the byte count, 0 at EOF, or [`READ_ERROR`]. A read is allowed to
    /// come up short — mpv treats that as "call again", which is exactly right
    /// when the range spanned a block boundary.
    pub(crate) fn read(&mut self, buf: &mut [u8], cancel: &CancelLatch) -> i64 {
        if cancel.is_cancelled() {
            return READ_ERROR;
        }
        if buf.is_empty() || self.pos >= self.size {
            return 0;
        }

        // Named up front so the future below borrows these and not `self`,
        // which `buf` is already borrowed out of.
        let stream = &self.stream;
        let pos = self.pos;

        let outcome = self.runtime.block_on(async {
            tokio::select! {
                // Biased so an already-cancelled stream loses no time to a
                // fetch nobody will use.
                biased;
                () = cancel.cancelled() => None,
                read = stream.read_at(pos, buf) => Some(read),
            }
        });

        match outcome {
            None => READ_ERROR,
            Some(Err(error)) => {
                tracing::warn!(%error, offset = pos, "stream read failed");
                READ_ERROR
            }
            Some(Ok(read)) => {
                self.pos += read as u64;
                read as i64
            }
        }
    }

    /// [`Self::read`] with a panic in the layers below turned into a read
    /// error.
    ///
    /// `extern "C"` aborts the process on unwind, so a bug anywhere under
    /// `read_at` would take the whole app down mid-episode. Failing the stream
    /// instead lets the app say so and keep its window.
    pub(crate) fn read_caught(&mut self, buf: &mut [u8], cancel: &CancelLatch) -> i64 {
        std::panic::catch_unwind(AssertUnwindSafe(|| self.read(buf, cancel))).unwrap_or_else(|_| {
            tracing::error!("panic while reading a stream; failing it");
            READ_ERROR
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{CONTENT_BLOCKS, test_runtime, test_stream};

    fn cursor() -> (StreamCursor, CancelLatch) {
        let runtime = test_runtime();
        let stream = test_stream(&runtime);
        (StreamCursor::new(stream, runtime), CancelLatch::default())
    }

    fn total() -> u64 {
        CONTENT_BLOCKS.iter().sum::<usize>() as u64
    }

    #[test]
    fn the_cursor_reports_the_whole_revision_size() {
        let (cursor, _) = cursor();
        assert_eq!(cursor.size(), total() as i64);
    }

    #[test]
    fn sequential_reads_walk_the_file_and_stop_at_eof() {
        let (mut cursor, cancel) = cursor();
        let mut got = Vec::new();
        let mut buf = [0_u8; 700];

        loop {
            let read = cursor.read(&mut buf, &cancel);
            assert!(read >= 0, "read failed at {}", cursor.position());
            if read == 0 {
                break;
            }
            got.extend_from_slice(&buf[..read as usize]);
        }

        assert_eq!(got.len() as u64, total());
        assert!(got.iter().enumerate().all(|(i, b)| *b == (i % 251) as u8));
        assert_eq!(cursor.position(), total());
    }

    #[test]
    fn a_seek_moves_where_the_next_read_starts() {
        let (mut cursor, cancel) = cursor();
        let offset = 5_000_i64;
        assert_eq!(cursor.seek(offset), offset);

        let mut buf = [0_u8; 16];
        let read = cursor.read(&mut buf, &cancel);
        assert_eq!(read, 16);
        assert_eq!(buf[0], (offset as usize % 251) as u8);
        assert_eq!(cursor.position(), offset as u64 + 16);
    }

    #[test]
    fn a_read_crossing_a_block_boundary_is_allowed_to_come_up_short() {
        // mpv reads again rather than treating a short read as EOF, so this is
        // not a bug to paper over — but the bytes it does return must be right.
        let (mut cursor, cancel) = cursor();
        let boundary = CONTENT_BLOCKS[0] as i64;
        cursor.seek(boundary - 8);

        let mut buf = [0_u8; 64];
        let read = cursor.read(&mut buf, &cancel);
        assert!(read > 0 && read <= 64);
        let start = boundary as usize - 8;
        assert!(
            buf[..read as usize]
                .iter()
                .enumerate()
                .all(|(i, b)| *b == ((start + i) % 251) as u8)
        );
    }

    #[test]
    fn seeking_exactly_to_eof_is_allowed_and_reads_zero() {
        let (mut cursor, cancel) = cursor();
        let end = total() as i64;
        assert_eq!(cursor.seek(end), end);

        let mut buf = [0_u8; 16];
        assert_eq!(cursor.read(&mut buf, &cancel), 0);
    }

    #[test]
    fn seeking_past_the_end_or_before_the_start_is_an_error() {
        let (mut cursor, _) = cursor();
        assert_eq!(cursor.seek(total() as i64 + 1), SEEK_ERROR);
        assert_eq!(cursor.seek(-1), SEEK_ERROR);
        // And the position is untouched by a refused seek.
        assert_eq!(cursor.position(), 0);
    }

    #[test]
    fn mpvs_seekability_probe_to_zero_succeeds() {
        // mpv issues a seek to 0 immediately after open purely to find out
        // whether the stream is seekable. Failing it would cost every seek bar.
        let (mut cursor, _) = cursor();
        assert_eq!(cursor.seek(0), 0);
    }

    #[test]
    fn a_cancelled_stream_fails_its_reads_instead_of_serving_them() {
        let (mut cursor, cancel) = cursor();
        cancel.cancel();

        let mut buf = [0_u8; 16];
        assert_eq!(cursor.read(&mut buf, &cancel), READ_ERROR);
        // And stays cancelled — the latch is the stream's lifetime, not one op.
        assert_eq!(cursor.read(&mut buf, &cancel), READ_ERROR);
    }

    #[test]
    fn an_empty_buffer_reads_nothing_rather_than_failing() {
        let (mut cursor, cancel) = cursor();
        assert_eq!(cursor.read(&mut [], &cancel), 0);
    }
}
