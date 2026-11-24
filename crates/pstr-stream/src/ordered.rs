//! The files an ordered-chapters episode takes its opening and ending from.
//!
//! mpv looks for them in the episode's directory, which a stream does not
//! have, and plays the episode with those chapters missing. So before the
//! episode loads, [`find_parts`] reads which segments it links to, reads the
//! `SegmentUID` of each candidate file the caller offers — the unnumbered
//! files in the same folder, from
//! `pstr_core::catalog::Catalog::ordered_part_candidates` — and returns the
//! streams that match. The caller publishes them and hands mpv the list as
//! `ordered-chapters-files`.
//!
//! An episode that links nothing costs one read of its first bytes, which the
//! player is about to read anyway. One that does costs a block fetch per
//! candidate the first time; a candidate's `SegmentUID` is remembered for the
//! life of the process, by revision, so the next episode opens only the files
//! it needs.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Mutex, OnceLock, PoisonError};

use futures::StreamExt;
use pstr_core::matroska::{self, HEAD_BYTES, Head, MAX_CHAPTERS_BYTES, SegmentUid};

use crate::{Result, VideoStream};

/// How many candidate files are opened at once.
const CANDIDATES_IN_FLIGHT: usize = 6;

/// What a file's head says, by its revision. `None` is a file that is not
/// Matroska or has no `SegmentUID`.
fn known_uids() -> &'static Mutex<HashMap<String, Option<SegmentUid>>> {
    static KNOWN: OnceLock<Mutex<HashMap<String, Option<SegmentUid>>>> = OnceLock::new();
    KNOWN.get_or_init(Default::default)
}

fn remembered(revision: &str) -> Option<Option<SegmentUid>> {
    known_uids()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(revision)
        .copied()
}

fn remember(revision: &str, uid: Option<SegmentUid>) {
    known_uids()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(revision.to_owned(), uid);
}

/// The segments `episode`'s chapters link to, other than its own. Empty for
/// anything that is not an ordered-chapters Matroska file.
pub async fn linked_segments(episode: &VideoStream) -> Result<Vec<SegmentUid>> {
    let read = |offset, length| episode.read_range(offset, length);
    linked_in(episode.size(), read).await
}

/// A file's `SegmentUID`, when it is a Matroska file with one.
pub async fn segment_uid(file: &VideoStream) -> Result<Option<SegmentUid>> {
    if let Some(known) = remembered(file.revision_id()) {
        return Ok(known);
    }
    let read = |offset, length| file.read_range(offset, length);
    let uid = uid_in(file.size(), read).await;
    // Only the head was wanted: the blocks after it are not about to play.
    file.cancel_read_ahead();
    let uid = uid?;
    remember(file.revision_id(), uid);
    Ok(uid)
}

/// The files among `candidates` that `episode` links to, opened.
///
/// `revision` names a candidate's revision when the catalog knows it, so one
/// already read is not opened again to be turned down. `open` opens one. A
/// candidate that fails to open or read is left out: the episode plays either
/// way, with the chapters it could not find skipped, as before.
pub async fn find_parts<C, R, F, Fut, E>(
    episode: &VideoStream,
    candidates: Vec<C>,
    revision: R,
    open: F,
) -> Vec<VideoStream>
where
    R: Fn(&C) -> Option<String>,
    F: Fn(C) -> Fut,
    Fut: Future<Output = std::result::Result<VideoStream, E>>,
    E: std::fmt::Display,
{
    if candidates.is_empty() {
        return Vec::new();
    }
    let wanted = match linked_segments(episode).await {
        Ok(wanted) if !wanted.is_empty() => wanted,
        Ok(_) => {
            tracing::debug!("the episode links no other segment");
            return Vec::new();
        }
        Err(error) => {
            tracing::debug!(%error, "reading an episode's chapters failed");
            return Vec::new();
        }
    };

    let worth_opening = candidates.into_iter().filter(|candidate| {
        match revision(candidate).and_then(|revision| remembered(&revision)) {
            Some(uid) => uid.is_some_and(|uid| wanted.contains(&uid)),
            None => true,
        }
    });
    let mut checked = futures::stream::iter(worth_opening.map(|candidate| {
        let opening = open(candidate);
        async move {
            let file = opening.await.map_err(|error| error.to_string())?;
            let uid = segment_uid(&file)
                .await
                .map_err(|error| error.to_string())?;
            Ok::<_, String>((uid, file))
        }
    }))
    .buffer_unordered(CANDIDATES_IN_FLIGHT);

    let mut found: Vec<(SegmentUid, VideoStream)> = Vec::new();
    while let Some(result) = checked.next().await {
        match result {
            Ok((Some(uid), file)) if wanted.contains(&uid) => {
                if !found.iter().any(|(have, _)| *have == uid) {
                    found.push((uid, file));
                }
                if found.len() == wanted.len() {
                    break;
                }
            }
            Ok(_) => {}
            Err(error) => tracing::debug!(%error, "checking an ordered-chapters part failed"),
        }
    }
    if found.len() < wanted.len() {
        tracing::info!(
            wanted = wanted.len(),
            found = found.len(),
            "not every ordered-chapters part is beside the episode"
        );
    }
    found.sort_by_key(|(uid, _)| wanted.iter().position(|want| want == uid));
    found.into_iter().map(|(_, file)| file).collect()
}

/// The value for mpv's `ordered-chapters-files`: a playlist of `urls`, inline,
/// so there is no file to write. Empty for none, which puts mpv back to its own
/// directory scan.
pub fn chapters_files_option(urls: &[String]) -> String {
    if urls.is_empty() {
        String::new()
    } else {
        format!("memory://{}", urls.join("\n"))
    }
}

async fn linked_in<R, Fut>(size: u64, read: R) -> Result<Vec<SegmentUid>>
where
    R: Fn(u64, u64) -> Fut,
    Fut: Future<Output = Result<Vec<u8>>>,
{
    let head_bytes = read(0, HEAD_BYTES.min(size)).await?;
    let Some(head) = Head::parse(&head_bytes) else {
        return Ok(Vec::new());
    };
    if let Some(linked) = head.linked {
        return Ok(linked);
    }
    let Some(at) = head.chapters_at else {
        return Ok(Vec::new());
    };
    let own = match head.segment_uid {
        Some(uid) => Some(uid),
        None => match head.info_at {
            Some(info) => element_at(&read, size, info, HEAD_BYTES)
                .await?
                .and_then(|bytes| matroska::info_element_uid(&bytes)),
            None => None,
        },
    };
    Ok(element_at(&read, size, at, MAX_CHAPTERS_BYTES)
        .await?
        .and_then(|bytes| matroska::chapters_element_links(&bytes, own))
        .unwrap_or_default())
}

async fn uid_in<R, Fut>(size: u64, read: R) -> Result<Option<SegmentUid>>
where
    R: Fn(u64, u64) -> Fut,
    Fut: Future<Output = Result<Vec<u8>>>,
{
    let head_bytes = read(0, HEAD_BYTES.min(size)).await?;
    let Some(head) = Head::parse(&head_bytes) else {
        return Ok(None);
    };
    if head.segment_uid.is_some() {
        return Ok(head.segment_uid);
    }
    let Some(at) = head.info_at else {
        return Ok(None);
    };
    Ok(element_at(&read, size, at, HEAD_BYTES)
        .await?
        .and_then(|bytes| matroska::info_element_uid(&bytes)))
}

/// The whole element at `at`, when it is no longer than `cap`.
async fn element_at<R, Fut>(read: &R, size: u64, at: u64, cap: u64) -> Result<Option<Vec<u8>>>
where
    R: Fn(u64, u64) -> Fut,
    Fut: Future<Output = Result<Vec<u8>>>,
{
    if at >= size {
        return Ok(None);
    }
    let header = read(at, 12).await?;
    let Some(len) = matroska::element_len(&header).filter(|len| *len <= cap) else {
        return Ok(None);
    };
    Ok(Some(read(at, len).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One element: its id, as written, then an 8-byte size and the data.
    fn el(id: u32, data: &[u8]) -> Vec<u8> {
        let mut out: Vec<u8> = id
            .to_be_bytes()
            .into_iter()
            .skip_while(|b| *b == 0)
            .collect();
        out.push(0x01);
        out.extend_from_slice(&(data.len() as u64).to_be_bytes()[1..]);
        out.extend_from_slice(data);
        out
    }

    fn reader(bytes: Vec<u8>) -> impl Fn(u64, u64) -> std::future::Ready<Result<Vec<u8>>> {
        move |offset, length| {
            let start = (offset as usize).min(bytes.len());
            let end = (start + length as usize).min(bytes.len());
            std::future::ready(Ok(bytes[start..end].to_vec()))
        }
    }

    /// An episode whose `Chapters` sit past [`HEAD_BYTES`], behind a large
    /// attachment, as some muxers write it: found through the `SeekHead`.
    #[test]
    fn chapters_past_a_large_attachment_are_still_read() {
        let own = [1u8; 16];
        let opening = [7u8; 16];
        let info = el(0x1549_A966, &el(0x73A4, &own));
        let attachment = el(0x1941_A469, &vec![0; HEAD_BYTES as usize]);
        let atom = |link: [u8; 16]| el(0xB6, &el(0x6E67, &link));
        let edition = el(
            0x45B9,
            &[el(0x45DD, &[1]), atom(opening), atom(own)].concat(),
        );
        let chapters = el(0x1043_A770, &edition);

        // The SeekHead is a fixed size, so where Chapters lands is known
        // before it is written.
        let seek_head = |at: u64| {
            el(
                0x114D_9B74,
                &el(
                    0x4DBB,
                    &[
                        el(0x53AB, &0x1043_A770u32.to_be_bytes()),
                        el(0x53AC, &at.to_be_bytes()),
                    ]
                    .concat(),
                ),
            )
        };
        let at = (seek_head(0).len() + info.len() + attachment.len()) as u64;
        let segment = [
            seek_head(at),
            info,
            attachment,
            chapters,
            el(0x1F43_B675, &[0; 8]),
        ]
        .concat();
        let bytes = [
            el(0x1A45_DFA3, &el(0x4282, b"matroska")),
            el(0x1853_8067, &segment),
        ]
        .concat();

        let size = bytes.len() as u64;
        let linked = futures::executor::block_on(linked_in(size, reader(bytes.clone())));
        assert_eq!(linked.unwrap(), vec![opening]);
        let uid = futures::executor::block_on(uid_in(size, reader(bytes)));
        assert_eq!(uid.unwrap(), Some(own));
    }

    #[test]
    fn a_file_that_is_not_matroska_links_nothing() {
        let bytes = b"\x00\x00\x00\x18ftypmp42 and the rest of an mp4".to_vec();
        let size = bytes.len() as u64;
        let linked = futures::executor::block_on(linked_in(size, reader(bytes)));
        assert!(linked.unwrap().is_empty());
    }

    #[test]
    fn mpv_gets_the_parts_as_an_inline_playlist() {
        assert_eq!(chapters_files_option(&[]), "");
        assert_eq!(
            chapters_files_option(&["pstr://4".into(), "pstr://5".into()]),
            "memory://pstr://4\npstr://5"
        );
    }
}
