//! Just enough of a Matroska file's head to play an ordered-chapters release.
//!
//! Some releases keep an opening or an ending once, in a file of its own, and
//! have every episode point at it: the episode's chapters name a segment by its
//! 16-byte `SegmentUID` instead of holding the frames. mpv stitches the two
//! together, but it finds the other file by scanning the episode's directory,
//! and a stream has no directory. So the caller reads the episode's head with
//! [`Head::parse`], asks it which segments it links to, reads the same of each
//! file beside it, and hands mpv the ones that match. See
//! `pstr_stream::ordered`.
//!
//! Only the elements that question needs are read: `SeekHead`, `Info`'s
//! `SegmentUID` and `Chapters`. Everything stops at the first `Cluster`, past
//! which nothing of the kind lives.

/// A Matroska segment's identity: the `SegmentUID` in its `Info`, which an
/// ordered chapter names in its `ChapterSegmentUID`.
pub type SegmentUid = [u8; 16];

const EBML: u32 = 0x1A45_DFA3;
const SEGMENT: u32 = 0x1853_8067;
const SEEK_HEAD: u32 = 0x114D_9B74;
const SEEK: u32 = 0x4DBB;
const SEEK_ID: u32 = 0x53AB;
const SEEK_POSITION: u32 = 0x53AC;
const INFO: u32 = 0x1549_A966;
const SEGMENT_UID: u32 = 0x73A4;
const CHAPTERS: u32 = 0x1043_A770;
const EDITION_ENTRY: u32 = 0x45B9;
const EDITION_FLAG_DEFAULT: u32 = 0x45DB;
const EDITION_FLAG_ORDERED: u32 = 0x45DD;
const CHAPTER_ATOM: u32 = 0xB6;
const CHAPTER_SEGMENT_UID: u32 = 0x6E67;
const CLUSTER: u32 = 0x1F43_B675;

/// How much of a file [`Head::parse`] wants to see. `SeekHead`, `Info` and
/// `Chapters` sit well inside it in every muxer's output; what does not —
/// `Chapters` written after a font attachment, say — is found through the
/// `SeekHead` and read on its own.
pub const HEAD_BYTES: u64 = 64 * 1024;

/// The largest `Chapters` element worth reading. A real one is a few
/// kilobytes; anything near this is not one.
pub const MAX_CHAPTERS_BYTES: u64 = 1024 * 1024;

/// What the start of a Matroska file says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Head {
    /// The segment's own identity, when its `Info` was in the bytes read.
    pub segment_uid: Option<SegmentUid>,
    /// The segments the chapters link to, when the `Chapters` element was in
    /// the bytes read: empty for a file that is whole on its own.
    pub linked: Option<Vec<SegmentUid>>,
    /// Where `Info` starts in the file, from the `SeekHead`, for a head that
    /// ended before it.
    pub info_at: Option<u64>,
    /// Where `Chapters` starts in the file, from the `SeekHead`.
    pub chapters_at: Option<u64>,
}

impl Head {
    /// Read the start of a file. `None` when it is not Matroska at all.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let (id, header, size) = element(bytes)?;
        if id != EBML {
            return None;
        }
        let mut at = header.checked_add(usize::try_from(size?).ok()?)?;
        let (id, header, _) = element(bytes.get(at..)?)?;
        if id != SEGMENT {
            return None;
        }
        at += header;
        let segment_start = at as u64;

        let mut head = Self::default();
        // A child whose size is unknown, or that runs past the bytes read,
        // ends the walk: nothing after it can be located.
        while let Some((id, header, size)) = bytes.get(at..).and_then(element) {
            if id == CLUSTER {
                break;
            }
            let body = at + header;
            let end = size.and_then(|size| body.checked_add(usize::try_from(size).ok()?));
            let whole = end.and_then(|end| bytes.get(body..end));
            match (id, whole) {
                (SEEK_HEAD, Some(data)) => {
                    for (target, position) in seek_entries(data) {
                        let position = segment_start.checked_add(position);
                        match target {
                            INFO => head.info_at = position,
                            CHAPTERS => head.chapters_at = position,
                            _ => {}
                        }
                    }
                }
                (INFO, Some(data)) => head.segment_uid = info_uid(data),
                (CHAPTERS, Some(data)) => head.linked = Some(linked(data, head.segment_uid)),
                _ => {}
            }
            match end {
                Some(end) if end <= bytes.len() => at = end,
                _ => break,
            }
        }
        // A link to itself is a chapter of this file, not another one.
        if let (Some(own), Some(linked)) = (head.segment_uid, head.linked.as_mut()) {
            linked.retain(|uid| *uid != own);
        }
        Some(head)
    }
}

/// How long the element starting at `bytes` is, header included: what to read
/// at a position the `SeekHead` gave. `None` when the header is not all there,
/// or the size is unknown.
pub fn element_len(bytes: &[u8]) -> Option<u64> {
    let (_, header, size) = element(bytes)?;
    size?.checked_add(header as u64)
}

/// The `SegmentUID` of an `Info` element read on its own, header included.
pub fn info_element_uid(bytes: &[u8]) -> Option<SegmentUid> {
    let (id, header, size) = element(bytes)?;
    if id != INFO {
        return None;
    }
    info_uid(bytes.get(header..header + usize::try_from(size?).ok()?)?)
}

/// The segments a `Chapters` element read on its own, header included, links
/// to — all but `own`.
pub fn chapters_element_links(bytes: &[u8], own: Option<SegmentUid>) -> Option<Vec<SegmentUid>> {
    let (id, header, size) = element(bytes)?;
    if id != CHAPTERS {
        return None;
    }
    let data = bytes.get(header..header + usize::try_from(size?).ok()?)?;
    let mut links = linked(data, own);
    if let Some(own) = own {
        links.retain(|uid| *uid != own);
    }
    Some(links)
}

/// The segments the edition mpv plays links to: the one flagged default, or
/// the first, as mpv picks with `--edition=auto`. Only an ordered edition links
/// anything; in any other a `ChapterSegmentUID` is ignored.
fn linked(chapters: &[u8], own: Option<SegmentUid>) -> Vec<SegmentUid> {
    let editions: Vec<&[u8]> = children(chapters)
        .filter(|(id, _)| *id == EDITION_ENTRY)
        .map(|(_, data)| data)
        .collect();
    let chosen = editions
        .iter()
        .find(|edition| flag(edition, EDITION_FLAG_DEFAULT))
        .or_else(|| editions.first());
    let Some(edition) = chosen else {
        return Vec::new();
    };
    if !flag(edition, EDITION_FLAG_ORDERED) {
        return Vec::new();
    }
    let mut links = Vec::new();
    collect_links(edition, &mut links);
    links.retain(|uid| Some(*uid) != own);
    links
}

/// Every `ChapterSegmentUID` under the atoms in `data`, nested atoms included,
/// each once and in order.
fn collect_links(data: &[u8], links: &mut Vec<SegmentUid>) {
    for (id, atom) in children(data) {
        if id != CHAPTER_ATOM {
            continue;
        }
        for (id, field) in children(atom) {
            if id == CHAPTER_SEGMENT_UID
                && let Ok(uid) = SegmentUid::try_from(field)
                && !links.contains(&uid)
            {
                links.push(uid);
            }
        }
        collect_links(atom, links);
    }
}

fn flag(data: &[u8], wanted: u32) -> bool {
    children(data)
        .find(|(id, _)| *id == wanted)
        .is_some_and(|(_, value)| value.iter().any(|byte| *byte != 0))
}

fn info_uid(info: &[u8]) -> Option<SegmentUid> {
    children(info)
        .find(|(id, _)| *id == SEGMENT_UID)
        .and_then(|(_, value)| SegmentUid::try_from(value).ok())
}

/// The `(SeekID, SeekPosition)` pairs of a `SeekHead`.
fn seek_entries(data: &[u8]) -> impl Iterator<Item = (u32, u64)> + '_ {
    children(data)
        .filter(|(id, _)| *id == SEEK)
        .filter_map(|(_, seek)| {
            let mut target = None;
            let mut position = None;
            for (id, value) in children(seek) {
                match id {
                    SEEK_ID => target = Some(be_uint(value)? as u32),
                    SEEK_POSITION => position = be_uint(value),
                    _ => {}
                }
            }
            Some((target?, position?))
        })
}

fn be_uint(bytes: &[u8]) -> Option<u64> {
    if bytes.len() > 8 {
        return None;
    }
    Some(
        bytes
            .iter()
            .fold(0, |value, byte| value << 8 | u64::from(*byte)),
    )
}

/// The whole, sized children of a master element's data, in order. Stops at
/// the first one that is not.
fn children(data: &[u8]) -> impl Iterator<Item = (u32, &[u8])> {
    let mut at = 0usize;
    std::iter::from_fn(move || {
        let (id, header, size) = element(data.get(at..)?)?;
        let body = at.checked_add(header)?;
        let end = body.checked_add(usize::try_from(size?).ok()?)?;
        let value = data.get(body..end)?;
        at = end;
        Some((id, value))
    })
}

/// An element header: its id, how many bytes the header takes, and the size of
/// its data — `None` for the all-ones "unknown" size.
fn element(bytes: &[u8]) -> Option<(u32, usize, Option<u64>)> {
    let (id, id_len) = vint(bytes, 4, true)?;
    let (size, size_len) = vint(bytes.get(id_len..)?, 8, false)?;
    let unknown = size == (1u64 << (7 * size_len)) - 1;
    Some((id as u32, id_len + size_len, (!unknown).then_some(size)))
}

/// A variable-length integer: the value and how many bytes it took. An id
/// keeps its length marker; a size drops it.
fn vint(bytes: &[u8], max: usize, keep_marker: bool) -> Option<(u64, usize)> {
    let first = *bytes.first()?;
    let len = first.leading_zeros() as usize + 1;
    if len > max {
        return None;
    }
    let tail = bytes.get(1..len)?;
    let lead = if keep_marker {
        u64::from(first)
    } else {
        u64::from(first) & ((1 << (8 - len)) - 1)
    };
    Some((
        tail.iter()
            .fold(lead, |value, byte| value << 8 | u64::from(*byte)),
        len,
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// One element: its id, as written, then an 8-byte size and the data.
    pub(crate) fn el(id: u32, data: &[u8]) -> Vec<u8> {
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

    fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
        parts.concat()
    }

    pub(crate) fn uid(n: u8) -> SegmentUid {
        [n; 16]
    }

    fn atom(link: Option<SegmentUid>) -> Vec<u8> {
        let mut data = el(0x73C4, &[1]);
        if let Some(link) = link {
            data.extend(el(CHAPTER_SEGMENT_UID, &link));
        }
        el(CHAPTER_ATOM, &data)
    }

    fn edition(ordered: bool, default: bool, atoms: &[Vec<u8>]) -> Vec<u8> {
        let mut data = el(EDITION_FLAG_ORDERED, &[u8::from(ordered)]);
        data.extend(el(EDITION_FLAG_DEFAULT, &[u8::from(default)]));
        data.extend(atoms.concat());
        el(EDITION_ENTRY, &data)
    }

    /// A file: the EBML header, then a segment of `children` and a cluster.
    pub(crate) fn file(children: &[Vec<u8>]) -> Vec<u8> {
        let mut segment = children.concat();
        segment.extend(el(CLUSTER, &[0; 32]));
        cat(&[el(EBML, &el(0x4282, b"matroska")), el(SEGMENT, &segment)])
    }

    pub(crate) fn info(own: SegmentUid) -> Vec<u8> {
        el(INFO, &el(SEGMENT_UID, &own))
    }

    pub(crate) fn episode(own: SegmentUid, links: &[SegmentUid]) -> Vec<u8> {
        let mut atoms = vec![atom(None)];
        atoms.extend(links.iter().map(|link| atom(Some(*link))));
        atoms.push(atom(Some(own)));
        file(&[info(own), el(CHAPTERS, &edition(true, true, &atoms))])
    }

    #[test]
    fn an_episode_names_the_openings_and_endings_it_links_to_and_not_itself() {
        let head = Head::parse(&episode(uid(1), &[uid(7), uid(9)])).unwrap();
        assert_eq!(head.segment_uid, Some(uid(1)));
        assert_eq!(head.linked, Some(vec![uid(7), uid(9)]));
    }

    #[test]
    fn a_part_file_is_known_by_its_segment_uid() {
        let head = Head::parse(&file(&[info(uid(7))])).unwrap();
        assert_eq!(head.segment_uid, Some(uid(7)));
        assert_eq!(head.linked, None);
    }

    #[test]
    fn chapters_that_are_not_ordered_link_nothing() {
        let chapters = el(CHAPTERS, &edition(false, true, &[atom(Some(uid(7)))]));
        let head = Head::parse(&file(&[info(uid(1)), chapters])).unwrap();
        assert_eq!(head.linked, Some(vec![]));
    }

    #[test]
    fn the_default_edition_is_the_one_read_as_mpv_would_play_it() {
        let chapters = el(
            CHAPTERS,
            &cat(&[
                edition(true, false, &[atom(Some(uid(5)))]),
                edition(true, true, &[atom(Some(uid(6)))]),
            ]),
        );
        let head = Head::parse(&file(&[info(uid(1)), chapters])).unwrap();
        assert_eq!(head.linked, Some(vec![uid(6)]));
    }

    #[test]
    fn a_nested_chapter_links_too() {
        let inner = atom(Some(uid(8)));
        let outer = el(CHAPTER_ATOM, &cat(&[el(0x73C4, &[2]), inner]));
        let chapters = el(CHAPTERS, &edition(true, true, &[outer]));
        let head = Head::parse(&file(&[info(uid(1)), chapters])).unwrap();
        assert_eq!(head.linked, Some(vec![uid(8)]));
    }

    #[test]
    fn chapters_past_the_head_are_found_through_the_seek_head() {
        let seek = el(
            SEEK_HEAD,
            &el(
                SEEK,
                &cat(&[
                    el(SEEK_ID, &CHAPTERS.to_be_bytes()),
                    el(SEEK_POSITION, &[0x01, 0x00]),
                ]),
            ),
        );
        let bytes = file(&[seek, info(uid(1))]);
        let head = Head::parse(&bytes).unwrap();
        assert_eq!(head.linked, None);
        // The segment's data starts after the EBML header and its own header.
        let segment_start = el(EBML, &el(0x4282, b"matroska")).len() as u64 + 12;
        assert_eq!(head.chapters_at, Some(segment_start + 0x100));

        let chapters = el(CHAPTERS, &edition(true, true, &[atom(Some(uid(3)))]));
        assert_eq!(element_len(&chapters[..12]), Some(chapters.len() as u64));
        assert_eq!(
            chapters_element_links(&chapters, Some(uid(1))),
            Some(vec![uid(3)])
        );
        assert_eq!(info_element_uid(&info(uid(4))), Some(uid(4)));
    }

    #[test]
    fn a_head_cut_short_still_says_what_it_holds() {
        let bytes = episode(uid(1), &[uid(2)]);
        let info_end = file(&[info(uid(1))]).len() - el(CLUSTER, &[0; 32]).len();
        let head = Head::parse(&bytes[..info_end + 4]).unwrap();
        assert_eq!(head.segment_uid, Some(uid(1)));
        assert_eq!(head.linked, None);
    }

    #[test]
    fn something_that_is_not_matroska_is_not_read() {
        assert_eq!(Head::parse(b"\x00\x00\x00\x18ftypmp42"), None);
        assert_eq!(Head::parse(&[]), None);
    }
}
