//! Turning release filenames into titles, seasons and episodes.
//!
//! A pure function over a string, deliberately: no I/O, no state, no network.
//! That is what lets the whole thing be pinned by table-driven tests instead of
//! by squinting at a real library, which is the only way a parser this
//! heuristic stays honest as cases are added.
//!
//! Written rather than bound to `anitomy` because that is C++ and would poison
//! the cross-compilation story the app exists to have.

/// What a filename claims to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedName {
    /// The series or film title, cleaned of groups, tags and separators.
    pub title: String,
    /// Season number, when the name states one. Absent is *not* season 1 — a
    /// film has no season, and an absolute-numbered anime episode states none
    /// either, so the caller decides what to do about it.
    pub season: Option<u32>,
    /// Episode number, when the name states one.
    pub episode: Option<u32>,
    /// Release year, from a parenthesised or bracketed four-digit group.
    pub year: Option<u32>,
    /// The episode's own name, when the filename states one after the
    /// numbering (`S01E01-Mother and Children`). Never the quality and codec
    /// tags that follow numbering in scene releases — see [`episode_title`].
    pub episode_title: Option<String>,
}

impl ParsedName {
    /// Whether this looks like one episode of a series rather than a film.
    pub fn is_episode(&self) -> bool {
        self.episode.is_some()
    }
}

/// Parse a filename (with or without its extension) into a title and numbering.
///
/// Never fails: an unrecognisable name yields its cleaned-up self as the title
/// with nothing else filled in. A catalog with a wrong title is recoverable; one
/// that drops files it could not parse is not.
pub fn parse(file_name: &str) -> ParsedName {
    let stem = strip_extension(file_name);

    // Release-group prefixes (`[SubsPlease] …`) and trailing checksums
    // (`… [A1B2C3D4]`) are noise in every convention that uses them, but a
    // bracketed *year* is signal, so the year is read before they are dropped.
    let bracketed = bracketed_year(stem);
    let without_groups = strip_bracketed(stem);

    // A bare year is only looked for when there is no bracketed one, and only
    // in the stripped text, so its offset lines up with the title span below.
    let bare = bracketed
        .is_none()
        .then(|| bare_year(&without_groups))
        .flatten();
    let year = bracketed.or(bare.map(|(year, _)| year));

    let found = find_numbering(&without_groups);

    // The title ends at the numbering, or — for a film, which has none — at the
    // bare year, because everything past it is source and codec tags.
    let title_end = found
        .as_ref()
        .map(|m| m.start)
        .or(bare.map(|(_, at)| at))
        .unwrap_or(without_groups.len());
    let title = clean_title(&without_groups[..title_end], year);

    let episode_title = found
        .as_ref()
        .and_then(|m| episode_title(&without_groups[m.end..]));

    ParsedName {
        title,
        season: found.as_ref().and_then(|m| m.numbering.season),
        episode: found.as_ref().and_then(|m| m.numbering.episode),
        year,
        episode_title,
    }
}

#[derive(Debug, Clone, Copy)]
struct Numbering {
    season: Option<u32>,
    episode: Option<u32>,
}

/// Where the numbering was found, and how far it ran.
#[derive(Debug, Clone, Copy)]
struct NumberingMatch {
    numbering: Numbering,
    /// The title ends here.
    start: usize,
    /// Just past the numbering token; the episode name, if any, starts here.
    end: usize,
}

/// The episode's own name out of what followed the numbering.
///
/// Only taken when a dash introduces it. Scene releases separate their quality,
/// source and codec tags with the same character as their title words (`.`), so
/// without that requirement `Show.Name.S02E05.1080p.WEB-DL.x265` would yield an
/// "episode title" of `1080p WEB-DL x265`.
fn episode_title(tail: &str) -> Option<String> {
    let rest = tail.trim_start();
    let rest = rest
        .strip_prefix('-')
        .or_else(|| rest.strip_prefix('\u{2013}'))?;

    let cleaned = clean_title(rest, None);
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// Drop a trailing extension, but only one that looks like an extension.
///
/// A bare `rsplit('.')` would eat the tail of `Serial.Experiments.Lain`, so the
/// candidate has to be short and alphanumeric — which every container extension
/// is and no dot-separated title word reliably is.
fn strip_extension(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, ext))
            if !ext.is_empty()
                && ext.len() <= 4
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
                && ext.chars().any(|c| c.is_ascii_alphabetic()) =>
        {
            stem
        }
        _ => name,
    }
}

/// A four-digit year in brackets or parentheses, e.g. `Movie Title (2019)`.
///
/// Bounded to plausible release years so a resolution (`1080`) or a checksum
/// that happens to be four digits cannot be mistaken for one.
/// `Movie Title (2019)` / `Movie Title [2019]`.
fn bracketed_year(text: &str) -> Option<u32> {
    let bytes = text.as_bytes();
    for (index, window) in bytes.windows(6).enumerate() {
        let opens = window[0] == b'(' || window[0] == b'[';
        let closes = window[5] == b')' || window[5] == b']';
        // Checked on the *bytes*, before any slicing: a four-byte run of ASCII
        // digits is four characters, so `text[index + 1..index + 5]` is only a
        // valid slice once this holds. `Death (True)²-007.mkv` is the case that
        // proves it — indexing blind there cuts inside the `²`.
        if !opens || !closes || !window[1..5].iter().all(u8::is_ascii_digit) {
            continue;
        }
        if let Some(year) = plausible_year(&text[index + 1..index + 5]) {
            return Some(year);
        }
    }
    None
}

/// `Movie.Name.2019.1080p.BluRay.mkv` — the scene form for films, where the
/// year is a bare token rather than a bracketed one.
///
/// Only a token of *exactly* four digits counts, which is what keeps a
/// resolution out: `1080` and `720` are outside the plausible range, and `2160`
/// is inside it but never appears bare — it is always `2160p`, whose trailing
/// letter makes the token five characters and disqualifies it.
fn bare_year(text: &str) -> Option<(u32, usize)> {
    // Walked by `char_indices`, not by byte: a token boundary is any character
    // that is not ASCII-alphanumeric, and several of those are multi-byte.
    let mut token_start = 0;
    let mut token_len = 0_usize;

    for (index, ch) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        if ch.is_ascii_alphanumeric() {
            if token_len == 0 {
                token_start = index;
            }
            token_len += 1;
            continue;
        }

        if token_len == 4
            && let Some(year) = plausible_year(&text[token_start..index])
        {
            return Some((year, token_start));
        }
        token_len = 0;
    }
    None
}

/// Four digits that could be a release year.
fn plausible_year(digits: &str) -> Option<u32> {
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits
        .parse()
        .ok()
        .filter(|year| (1900..=2999).contains(year))
}

/// Remove `[...]` and `(...)` spans.
///
/// Release groups, resolutions, codecs, checksums and source tags all live in
/// them, and none of it belongs in a title. Unbalanced brackets are left alone
/// rather than swallowing the rest of the name.
fn strip_bracketed(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0_usize;

    for ch in text.chars() {
        match ch {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }

    if depth != 0 {
        // Unbalanced: the input was not the convention we assumed, so trust the
        // original over a half-stripped guess.
        return text.to_string();
    }
    out
}

/// Find the episode numbering and where the title ends.
///
/// Returns the byte offsets in `text` at which the numbering begins and ends, so
/// the caller keeps everything before it as the title and discards everything
/// after it — which is where quality, codec and source tags live in every
/// convention.
fn find_numbering(text: &str) -> Option<NumberingMatch> {
    // `S02E05` / `s02e05`, the western convention. Checked first because it is
    // unambiguous, where a bare number is not.
    if let Some(matched) = find_season_episode(text) {
        if matched.numbering.episode.is_some() {
            return Some(matched);
        }
        // A season stated *alone* does not end the search: `Oshi no Ko S2 - 04`
        // names its season one way and its episode another, and returning here
        // would drop the episode entirely.
        return Some(with_episode(matched, text));
    }

    // `Oshi no Ko 3rd Season - 07`: the season is spelled out. Same shape as the
    // `S2` case above — a season with the episode stated some other way — and it
    // has to be read, or the words end up in the title and, worse, in the
    // episode's *name*.
    if let Some(matched) = find_worded_season(text) {
        return Some(with_episode(matched, text));
    }

    find_episode_only(text)
}

/// Fold whatever states the episode number into a season-only match.
///
/// The title ends at whichever of the two came first; the quality tags start
/// after whichever came last.
fn with_episode(season: NumberingMatch, text: &str) -> NumberingMatch {
    let Some(episode) = find_episode_only(text) else {
        return season;
    };
    NumberingMatch {
        numbering: Numbering {
            season: season.numbering.season,
            episode: episode.numbering.episode,
        },
        start: season.start.min(episode.start),
        end: season.end.max(episode.end),
    }
}

/// `3rd Season`, `Season 3` — a season stated in words rather than as `S03`.
///
/// Both orders are in use and both are common in anime releases, where a sequel
/// is named rather than numbered. The keyword is required in either position: a
/// bare ordinal (`3rd`) says nothing on its own, and a bare number is the
/// episode.
fn find_worded_season(text: &str) -> Option<NumberingMatch> {
    let lower = text.to_ascii_lowercase();

    for (index, _) in lower.match_indices("season") {
        if !is_token_start(&lower, index) {
            continue;
        }
        let after_keyword = index + "season".len();
        // `Season 3`: the number follows the word. A dash is deliberately *not*
        // a separator here — `Show 2nd Season - 04` puts the episode there, and
        // reading across it makes episode four season four.
        let mut cursor = after_keyword;
        while matches!(lower.as_bytes().get(cursor), Some(b' ' | b'_' | b'.')) {
            cursor += 1;
        }
        if let Some((season, end)) = read_number(&lower, cursor)
            && end > cursor
            && season > 0
            && !is_token_char(lower.as_bytes().get(end).copied())
        {
            return Some(NumberingMatch {
                numbering: Numbering {
                    season: Some(season),
                    episode: None,
                },
                start: index,
                end,
            });
        }

        // `3rd Season`: the ordinal precedes it. The keyword has to *end* a
        // token here — `2nd Seasoning` is a word, not a season — where in the
        // branch above the digits are allowed to run straight on (`Season3`).
        if !is_token_char(lower.as_bytes().get(after_keyword).copied())
            && let Some((season, start)) = ordinal_before(&lower, index)
        {
            return Some(NumberingMatch {
                numbering: Numbering {
                    season: Some(season),
                    episode: None,
                },
                start,
                end: after_keyword,
            });
        }
    }
    None
}

/// The `3rd` of `3rd Season`, if that is what sits before `at`. Returns the
/// number and where it starts, so the title can be cut there.
fn ordinal_before(lower: &str, at: usize) -> Option<(u32, usize)> {
    let head = lower[..at].trim_end_matches([' ', '_', '.', '-']);
    // The suffix is required: `Oshi no Ko 3 Season` is not a thing anyone
    // writes, and without it a title ending in a digit would lose it.
    let head = ["st", "nd", "rd", "th"]
        .into_iter()
        .find_map(|suffix| head.strip_suffix(suffix))?;

    let digits_start = trailing_digits_start(head).unwrap_or(0);
    let digits = &head[digits_start..];
    if digits.is_empty() || digits.len() > 2 {
        return None;
    }
    let season: u32 = digits.parse().ok()?;
    (season > 0).then_some((season, digits_start))
}

/// The conventions that state an episode without stating a season.
fn find_episode_only(text: &str) -> Option<NumberingMatch> {
    // `1x05`, the older western convention.
    find_cross_numbering(text)
        // ` - 12` / ` - E1`, the fansub convention: absolute episode number.
        .or_else(|| find_dash_numbering(text))
        // `E11` standing alone, for libraries that name files with nothing else.
        .or_else(|| find_episode_marker(text))
        // `11.mkv` — the filename *is* the episode number.
        .or_else(|| find_bare_number(text))
}

/// A name that is nothing but a number.
///
/// Capped at three digits so a film named for a year (`1917.mkv`) cannot become
/// episode 1917 — that case is caught by the bare-year rule instead. The title
/// comes from the folder, which for this convention is the only place it exists.
fn find_bare_number(text: &str) -> Option<NumberingMatch> {
    let digits = text.trim();
    if digits.is_empty() || digits.len() > 3 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let start = text.len() - text.trim_start().len();

    Some(NumberingMatch {
        numbering: Numbering {
            season: None,
            episode: digits.parse().ok(),
        },
        start,
        end: start + digits.len(),
    })
}

/// A standalone `E01` / `EP01`, with no dash to introduce it.
///
/// Last in the chain because it is the weakest signal: it is only reached when
/// nothing else matched, which for a real library means the filename *is* the
/// episode number and the title comes from the folder.
fn find_episode_marker(text: &str) -> Option<NumberingMatch> {
    let bytes = text.as_bytes();

    for start in 0..bytes.len() {
        if !matches!(bytes[start], b'e' | b'E') || !is_token_start(text, start) {
            continue;
        }
        let digits = skip_episode_marker(text, start);
        if digits == start {
            continue;
        }
        let Some((episode, after)) = read_number(text, digits) else {
            continue;
        };
        if after == digits {
            continue;
        }
        // The number must end the token, so a checksum fragment cannot match.
        if is_token_char(bytes.get(after).copied()) {
            continue;
        }
        return Some(NumberingMatch {
            numbering: Numbering {
                season: None,
                episode: Some(episode),
            },
            start,
            end: after,
        });
    }
    None
}

/// `S02E05`, or `S02` alone.
fn find_season_episode(text: &str) -> Option<NumberingMatch> {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();

    for start in 0..bytes.len() {
        if bytes[start] != b's' || !is_token_start(&lower, start) {
            continue;
        }
        let (season, after_season) = read_number(&lower, start + 1)?;
        if after_season == start + 1 {
            continue;
        }

        // `S02E05`
        if bytes.get(after_season) == Some(&b'e')
            && let Some((episode, after_episode)) = read_number(&lower, after_season + 1)
            && after_episode > after_season + 1
        {
            return Some(NumberingMatch {
                numbering: Numbering {
                    season: Some(season),
                    episode: Some(episode),
                },
                start,
                end: after_episode,
            });
        }

        // `S02` on its own — a season folder or a season pack.
        if !is_token_char(bytes.get(after_season).copied()) {
            return Some(NumberingMatch {
                numbering: Numbering {
                    season: Some(season),
                    episode: None,
                },
                start,
                end: after_season,
            });
        }
    }
    None
}

/// `1x05`.
fn find_cross_numbering(text: &str) -> Option<NumberingMatch> {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();

    for (index, &byte) in bytes.iter().enumerate() {
        if byte != b'x' || index == 0 {
            continue;
        }
        // Walk back over the season digits to the start of the token.
        let mut start = index;
        while start > 0 && bytes[start - 1].is_ascii_digit() {
            start -= 1;
        }
        if start == index || !is_token_start(&lower, start) {
            continue;
        }
        let season: u32 = lower[start..index].parse().ok()?;
        let (episode, after) = read_number(&lower, index + 1)?;
        if after == index + 1 {
            continue;
        }
        return Some(NumberingMatch {
            numbering: Numbering {
                season: Some(season),
                episode: Some(episode),
            },
            start,
            end: after,
        });
    }
    None
}

/// ` - 12`, the fansub convention: a dash-delimited bare number.
///
/// The *last* such group wins, because a title may contain a dash of its own
/// (`Fate/stay night - Unlimited Blade Works - 06`).
fn find_dash_numbering(text: &str) -> Option<NumberingMatch> {
    let mut found: Option<NumberingMatch> = None;
    let bytes = text.as_bytes();

    for (index, &byte) in bytes.iter().enumerate() {
        if byte != b'-' {
            continue;
        }
        // `_-_01_` is as common as ` - 01 `: some groups separate every field
        // with underscores, dashes included.
        let mut cursor = index + 1;
        while matches!(bytes.get(cursor), Some(b' ' | b'_' | b'.')) {
            cursor += 1;
        }

        // An `E`/`EP` marker may introduce the number (`- E1`, `- Ep 03`).
        // Handled here rather than as its own convention because it only ever
        // appears in this position.
        cursor = skip_episode_marker(text, cursor);

        let Some((episode, after)) = read_number(text, cursor) else {
            continue;
        };
        if after == cursor {
            continue;
        }
        // A version suffix (`12v2`) still names episode 12.
        let tail = bytes.get(after).copied();
        if is_token_char(tail) && tail != Some(b'v') {
            continue;
        }
        found = Some(NumberingMatch {
            numbering: Numbering {
                season: None,
                episode: Some(episode),
            },
            start: index,
            end: after,
        });
    }
    found
}

/// Step over an `E` / `EP` episode marker and any separator after it.
///
/// Returns `at` unchanged when there is none, so a bare number and a marked one
/// are handled identically by the caller.
fn skip_episode_marker(text: &str, at: usize) -> usize {
    let bytes = text.as_bytes();
    if !matches!(bytes.get(at), Some(b'e' | b'E')) {
        return at;
    }
    let mut cursor = at + 1;
    if matches!(bytes.get(cursor), Some(b'p' | b'P')) {
        cursor += 1;
    }
    while matches!(bytes.get(cursor), Some(b'.' | b' ' | b'_')) {
        cursor += 1;
    }
    // Only a marker if a number actually follows; otherwise the `e` was the
    // first letter of a word.
    if bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
        cursor
    } else {
        at
    }
}

/// Read an unsigned number at `from`, returning it and the offset after it.
fn read_number(text: &str, from: usize) -> Option<(u32, usize)> {
    let bytes = text.as_bytes();
    let mut end = from;
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
    }
    if end == from {
        return Some((0, from));
    }
    // A run of digits far longer than any episode number is a checksum or a
    // date, not numbering.
    if end - from > 4 {
        return None;
    }
    text[from..end].parse().ok().map(|n| (n, end))
}

/// Where the trailing run of ASCII digits starts, or `None` when the text is
/// nothing but digits (or empty).
///
/// Walked back by `char_indices`, not by `rfind(…) + 1`: one past the *start*
/// of the character before the digits is inside it whenever that character is
/// multi-byte. `Death (True)² - 007` is the case that proves it — the `+ 1`
/// form cuts the `²` in half and panics.
fn trailing_digits_start(text: &str) -> Option<usize> {
    let mut start = None;

    for (index, ch) in text.char_indices().rev() {
        if ch.is_ascii_digit() {
            start = Some(index);
        } else {
            return Some(start.unwrap_or(text.len()));
        }
    }
    None
}

fn is_token_char(byte: Option<u8>) -> bool {
    byte.is_some_and(|b| b.is_ascii_alphanumeric())
}

/// Whether `at` begins a token rather than sitting inside a word — so the `s`
/// of `Seasons` cannot start a season match.
fn is_token_start(text: &str, at: usize) -> bool {
    if at == 0 {
        return true;
    }
    !text.as_bytes()[at - 1].is_ascii_alphanumeric()
}

/// Normalize the title span: separators to spaces, then trim the debris that
/// sits between the title and whatever followed it.
fn clean_title(raw: &str, year: Option<u32>) -> String {
    let mut text: String = raw
        .chars()
        .map(|c| match c {
            '.' | '_' => ' ',
            other => other,
        })
        .collect();

    // The year was extracted separately; leave it out of the title.
    if let Some(year) = year {
        text = text.replace(&year.to_string(), " ");
    }

    let trimmed = text.trim().trim_end_matches(['-', '–', ' ']).trim();

    // Collapse the runs of spaces the substitutions above create.
    let mut out = String::with_capacity(trimmed.len());
    let mut last_was_space = false;
    for ch in trimmed.chars() {
        let is_space = ch == ' ';
        if is_space && last_was_space {
            continue;
        }
        last_was_space = is_space;
        out.push(ch);
    }
    out
}

/// What a folder contributes to the files beneath it.
///
/// A library is organised as `<Series>/Season 01/<file>.mkv` far more often than
/// it is named `<Series> - S01E01.mkv`, so for most real collections the folder
/// path — not the filename — is where the title lives. This is what
/// [`crate::catalog::build_rows`] walks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderRole {
    /// `Season 01`, `S02` — states a season and nothing else.
    Season(u32),
    /// `Movies`, `OVA`, `Specials` — groups files without naming them. Skipped
    /// when looking for a title; the folder above it is the series.
    Container,
    /// Anything else: the folder names the series or film.
    Title(Box<ParsedName>),
}

/// Folder names that group without naming.
const CONTAINER_FOLDERS: &[&str] = &[
    "movies",
    "movie",
    "films",
    "film",
    "ova",
    "ovas",
    "oad",
    "oads",
    "specials",
    "special",
    "sp",
    "extras",
    "extra",
    "bonus",
    "ncop",
    "nced",
    "creditless",
    "subs",
    "subtitles",
];

/// What a folder name says about the files inside it.
pub fn classify_folder(name: &str) -> FolderRole {
    let trimmed = name.trim();
    let lower = trimmed.to_ascii_lowercase();

    if let Some(season) = season_folder(&lower) {
        return FolderRole::Season(season);
    }
    if CONTAINER_FOLDERS.contains(&lower.as_str()) {
        return FolderRole::Container;
    }

    // `Game of Thrones - Season 1` names the series *and* its season. Read
    // both, or the shelf grows one poster per season of the same show.
    let mut parsed = parse(trimmed);
    if parsed.season.is_none()
        && let Some((series, season)) = trailing_season(trimmed)
    {
        let mut from_series = parse(series);
        if !from_series.title.is_empty() {
            from_series.season = Some(season);
            parsed = from_series;
        }
    }
    FolderRole::Title(Box::new(parsed))
}

/// Split `<series> … Season 3` / `<series> … S03` into the series and the
/// season number.
///
/// The keyword is required. Without it, every title ending in a digit —
/// `Evangelion 3`, `Kaijuu 8-gou` — would lose it to a season it never claimed.
fn trailing_season(name: &str) -> Option<(&str, u32)> {
    let trimmed = name.trim_end();
    let digits_start = trailing_digits_start(trimmed)?;
    let (head, digits) = trimmed.split_at(digits_start);
    let season: u32 = digits.parse().ok()?;

    // `… S03`: the marker is glued to the number and stands alone as a token,
    // so a series whose own name ends in `s` keeps its last letter.
    if let Some(before) = head.strip_suffix(['S', 's'])
        && before.ends_with([' ', '_', '.', '-'])
    {
        let series = before.trim_end_matches([' ', '_', '.', '-']);
        return (!series.is_empty()).then_some((series, season));
    }

    // `… Season 3`.
    let head = head.trim_end_matches([' ', '_', '.', '-']);
    if !head.to_ascii_lowercase().ends_with("season") {
        return None;
    }
    let series = head[..head.len() - "season".len()].trim_end_matches([' ', '_', '.', '-']);
    // `Season 01` alone is a season folder, not a series called nothing.
    (!series.is_empty()).then_some((series, season))
}

/// `season 01`, `season 1`, `s01`, `3rd season` — and nothing else in the name.
fn season_folder(lower: &str) -> Option<u32> {
    // The ordinal form first: `3rd season` names a season and nothing else, and
    // a folder called that must not become a title called that.
    if let Some(matched) = find_worded_season(lower)
        && matched.start == 0
        && lower[matched.end..].trim().is_empty()
    {
        return matched.numbering.season;
    }

    let digits = lower
        .strip_prefix("season")
        .or_else(|| lower.strip_prefix('s'))?
        .trim_start_matches([' ', '_', '.', '-']);

    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Whether a name states a season and nothing else — `3rd Season`, `Season 2`,
/// `S03`.
///
/// What [`crate::catalog::build_rows`] asks before keeping the remains of a
/// filename as the episode's own name: `[DB]Oshi no Ko 3rd Season - 07.mkv`
/// under a folder called `Oshi no Ko` leaves `3rd Season` behind, and that is a
/// season marker, not what episode seven is called.
pub fn is_season_marker(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    if season_folder(&trimmed.to_ascii_lowercase()).is_some() {
        return true;
    }
    let parsed = parse(trimmed);
    parsed.title.is_empty() && parsed.season.is_some() && parsed.episode.is_none()
}

/// Containers this app can hand to mpv.
///
/// Deliberately a list rather than "anything that is not obviously metadata": a
/// share holds `.nfo`, `.txt` and `.jpg` alongside the video, and a catalog that
/// offers those as episodes is worse than one that omits an exotic container.
const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "m4v", "avi", "mov", "webm", "ts", "m2ts", "mts", "mpg", "mpeg", "wmv", "flv",
    "ogv", "ogm", "asf", "rmvb", "divx", "vob",
];

/// Whether a filename names something playable.
pub fn is_video_file(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| VIDEO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(name: &str) -> (String, Option<u32>, Option<u32>, Option<u32>) {
        let p = parse(name);
        (p.title, p.season, p.episode, p.year)
    }

    /// The fansub convention: bracketed group, dash-delimited absolute episode
    /// number, bracketed checksum. No season is stated and none is invented.
    #[test]
    fn a_fansub_release_yields_title_and_absolute_episode() {
        assert_eq!(
            parsed("[SubsPlease] Frieren - 12 (1080p) [A1B2C3D4].mkv"),
            ("Frieren".into(), None, Some(12), None)
        );
    }

    /// A title containing its own dash must not be cut at the first one — the
    /// numbering is the *last* dash-delimited number.
    #[test]
    fn a_dash_in_the_title_survives_absolute_numbering() {
        assert_eq!(
            parsed("[Group] Fate stay night - Unlimited Blade Works - 06 [720p].mkv"),
            (
                "Fate stay night - Unlimited Blade Works".into(),
                None,
                Some(6),
                None
            )
        );
    }

    /// A version suffix still names the same episode.
    #[test]
    fn a_versioned_fansub_episode_keeps_its_number() {
        assert_eq!(
            parsed("[Group] Show Name - 07v2 [1080p].mkv"),
            ("Show Name".into(), None, Some(7), None)
        );
    }

    /// The western convention: dot separators, `SxxEyy`, and a tail of quality
    /// and codec tags that must not reach the title.
    #[test]
    fn a_scene_release_yields_title_season_and_episode() {
        assert_eq!(
            parsed("Show.Name.S02E05.1080p.WEB-DL.x265.mkv"),
            ("Show Name".into(), Some(2), Some(5), None)
        );
    }

    /// Lowercase numbering is the same numbering.
    #[test]
    fn season_episode_matching_is_case_insensitive() {
        assert_eq!(
            parsed("show.name.s01e01.720p.mkv"),
            ("show name".into(), Some(1), Some(1), None)
        );
    }

    /// The older cross convention.
    #[test]
    fn cross_numbering_is_read_as_season_and_episode() {
        assert_eq!(
            parsed("Some Show 1x05 [HDTV].avi"),
            ("Some Show".into(), Some(1), Some(5), None)
        );
    }

    /// A film: a year, no numbering, and tags that are dropped.
    #[test]
    fn a_film_yields_a_title_and_year_and_no_numbering() {
        assert_eq!(
            parsed("Movie Title (2019) [1080p] [HEVC].mkv"),
            ("Movie Title".into(), None, None, Some(2019))
        );
    }

    /// The scene form for films: a bare, dot-delimited year.
    #[test]
    fn a_bare_year_token_is_read_as_a_year() {
        let p = parse("Movie.Name.2019.1080p.BluRay.x264.mkv");
        assert_eq!(p.year, Some(2019));
        assert_eq!(p.title, "Movie Name");
    }

    /// And the resolutions that sit right next to it are not years — `1080` and
    /// `720` are out of range, and `2160` never appears without its `p`.
    #[test]
    fn a_bare_resolution_is_not_mistaken_for_a_year() {
        assert_eq!(parse("Show.Name.S01E01.1080p.mkv").year, None);
        assert_eq!(parse("Show.Name.S01E01.720p.mkv").year, None);
        assert_eq!(parse("Show.Name.S01E01.2160p.HDR.mkv").year, None);
    }

    /// **A panic, not a mis-parse.** Superscripts and other multi-byte
    /// characters appear in real release names (`Death (True)²-007.mkv`), and
    /// byte-indexed slicing around them cuts inside a character.
    #[test]
    fn a_multi_byte_character_does_not_break_parsing() {
        for name in [
            "[Anime Time] Neon Genesis Evangelion - Death (True)\u{b2}-007.mkv",
            "Show \u{2013} S01E01 \u{2013} caf\u{e9}.mkv",
            "\u{5c0f}\u{8aac} Season 01 E03.mkv",
        ] {
            let parsed = parse(name);
            assert!(
                !parsed.title.is_empty() || parsed.episode.is_some(),
                "{name}"
            );
        }
    }

    /// **A panic, not a mis-parse.** A folder name whose trailing digits sit
    /// directly behind a multi-byte character — or that simply *ends* in one —
    /// used to be split one byte past that character's start, i.e. inside it.
    #[test]
    fn a_multi_byte_character_does_not_break_folder_classification() {
        for name in [
            "Neon Genesis Evangelion Death (True)\u{b2}",
            "Show\u{b2}3",
            "Show\u{b2}3rd Season",
            "Caf\u{e9} Season 2",
            "\u{b2}",
        ] {
            // The classification itself is the assertion: any of these used to
            // panic before it could return one.
            let _ = classify_folder(name);
        }

        let FolderRole::Title(parsed) = classify_folder("Caf\u{e9} Season 2") else {
            panic!("expected a title");
        };
        assert_eq!((parsed.title.as_str(), parsed.season), ("Café", Some(2)));
    }

    /// A resolution is four digits in brackets too, and must not read as a year.
    #[test]
    fn a_bracketed_resolution_is_not_mistaken_for_a_year() {
        let p = parse("Show Name - 03 [1080p].mkv");
        assert_eq!(p.year, None, "1080p is not a release year");
        assert_eq!(p.episode, Some(3));
    }

    /// A season pack or season folder states a season and no episode.
    #[test]
    fn a_bare_season_is_read_without_an_episode() {
        assert_eq!(
            parsed("Show Name S03 COMPLETE"),
            ("Show Name".into(), Some(3), None, None)
        );
    }

    /// `Seasons` starts with an `s` and a digit follows nearby; it is still not
    /// numbering.
    #[test]
    fn a_word_beginning_with_s_does_not_start_a_season() {
        let p = parse("Four Seasons 2019.mkv");
        assert_eq!(p.season, None, "'Seasons' is not 'S...'");
    }

    /// Titles with dots in them are exactly why the extension check is
    /// conservative.
    #[test]
    fn a_dotted_title_keeps_its_words() {
        assert_eq!(
            parsed("Serial.Experiments.Lain.S01E03.mkv"),
            ("Serial Experiments Lain".into(), Some(1), Some(3), None)
        );
    }

    /// Nothing recognisable still yields a usable title — a catalog that drops
    /// files it cannot parse is worse than one with an odd name in it.
    #[test]
    fn an_unrecognisable_name_still_yields_a_title() {
        assert_eq!(
            parsed("some_random_file.mkv"),
            ("some random file".into(), None, None, None)
        );
    }

    /// Unbalanced brackets mean the name is not the convention assumed, so the
    /// stripper leaves it alone rather than swallowing the rest.
    #[test]
    fn unbalanced_brackets_do_not_swallow_the_name() {
        let p = parse("[Group Show Name - 05.mkv");
        assert!(p.title.contains("Show Name"), "title survived: {}", p.title);
    }

    /// A name with no extension at all parses the same way.
    #[test]
    fn a_name_without_an_extension_parses_the_same() {
        assert_eq!(
            parsed("Show.Name.S02E05"),
            ("Show Name".into(), Some(2), Some(5), None)
        );
    }

    /// The episode's own name, where the filename states one.
    #[test]
    fn an_episode_name_after_the_numbering_is_captured() {
        let p = parse("S01E01-Mother and Children [B496EBF1].mkv");
        assert_eq!(p.season, Some(1));
        assert_eq!(p.episode, Some(1));
        assert_eq!(p.episode_title.as_deref(), Some("Mother and Children"));
        assert_eq!(p.title, "", "the filename states no series");
    }

    /// **The trap.** Scene releases separate their tags with the same character
    /// they separate title words with, so without requiring a dash the quality
    /// and codec tags would be read as the episode's name.
    #[test]
    fn quality_tags_after_the_numbering_are_not_an_episode_name() {
        let p = parse("Show.Name.S02E05.1080p.WEB-DL.x265.mkv");
        assert_eq!(p.episode_title, None, "1080p.WEB-DL.x265 is not a name");
        assert_eq!(p.title, "Show Name");
    }

    /// A library that names its files with nothing but the episode marker.
    #[test]
    fn a_bare_episode_marker_is_read_as_an_episode() {
        assert_eq!(parse("E11.mkv").episode, Some(11));
        assert_eq!(parse("EP07.mkv").episode, Some(7));
        assert_eq!(parse("Ep 3.mkv").episode, Some(3));
    }

    /// A dash-introduced marker, the form fansubs use for OVAs.
    #[test]
    fn a_dash_introduced_episode_marker_is_read_as_an_episode() {
        let p = parse("[Reaktor] Fullmetal Alchemist Brotherhood OVA - E2 v2 [1080p].mkv");
        assert_eq!(p.episode, Some(2));
        assert_eq!(p.title, "Fullmetal Alchemist Brotherhood OVA");
    }

    /// A filename that is nothing but its episode number — the folder carries
    /// the title in this convention.
    #[test]
    fn a_bare_number_filename_is_read_as_an_episode() {
        for (name, episode) in [("11.mkv", 11), ("01.mkv", 1), ("7.mkv", 7)] {
            let parsed = parse(name);
            assert_eq!(parsed.episode, Some(episode), "{name}");
            assert_eq!(parsed.title, "", "{name} states no title");
        }
    }

    /// A film named for a year must not become episode 1917.
    #[test]
    fn a_year_shaped_filename_is_not_an_episode() {
        let parsed = parse("1917.mkv");
        assert_eq!(parsed.episode, None);
        assert_eq!(parsed.year, Some(1917));
    }

    /// **A season and an episode stated in different conventions.** Matching the
    /// season must not end the search, or the episode is dropped.
    #[test]
    fn a_season_marker_and_a_dash_episode_are_both_read() {
        let p = parse("[EMBER] Oshi no Ko S2 - 04.mkv");
        assert_eq!(p.title, "Oshi no Ko");
        assert_eq!(p.season, Some(2));
        assert_eq!(p.episode, Some(4));
    }

    /// Some groups separate every field with underscores, the dash included —
    /// and spell the season out rather than numbering it `S03`.
    ///
    /// **Both halves matter.** Reading `3rd Season` as part of the title files
    /// the season under its own poster; leaving it in the span after the series
    /// name makes it the *episode's* name, so every episode of season three is
    /// called "3rd Season" in the list.
    #[test]
    fn underscore_separated_numbering_is_read() {
        let p = parse("[DB]Oshi no Ko 3rd Season_-_07_(Dual Audio_10bit_1080p_x265).mkv");
        assert_eq!(p.episode, Some(7));
        assert_eq!(p.season, Some(3));
        assert_eq!(p.title, "Oshi no Ko");
        assert_eq!(p.episode_title, None);
    }

    /// Every way a release spells a season out, in both orders.
    #[test]
    fn a_worded_season_is_read_as_a_season() {
        for (name, season, episode) in [
            ("[Group] Show 2nd Season - 04 [1080p].mkv", 2, 4),
            ("[Group] Show 3rd Season - 11.mkv", 3, 11),
            ("Show 1st Season - 01.mkv", 1, 1),
            ("Show 4th Season - 02.mkv", 4, 2),
            ("Show Season 2 - 06.mkv", 2, 6),
            ("Show.Season.3.E05.mkv", 3, 5),
        ] {
            let parsed = parse(name);
            assert_eq!(parsed.season, Some(season), "{name}");
            assert_eq!(parsed.episode, Some(episode), "{name}");
            assert_eq!(parsed.title, "Show", "{name}");
        }
    }

    /// The ordinal has to belong to the word `Season`. A title that merely ends
    /// in one keeps it, and a season with no keyword is not a season.
    #[test]
    fn an_ordinal_that_is_part_of_the_title_is_not_a_season() {
        for name in [
            "[Group] 3rd Rate Duelist - 04.mkv",
            "[Group] My 1st Kiss - 02.mkv",
            "[Group] Show 3 - 04.mkv",
        ] {
            let parsed = parse(name);
            assert_eq!(parsed.season, None, "{name}");
        }
        // `Seasons` is not `Season`: the keyword has to end the token.
        assert_eq!(parse("Show 2nd Seasoning - 03.mkv").season, None);
    }

    /// A folder that spells its season out is a season folder, not a series
    /// called `3rd Season`.
    #[test]
    fn a_worded_season_folder_is_a_season_folder() {
        assert_eq!(classify_folder("3rd Season"), FolderRole::Season(3));
        assert_eq!(classify_folder("2nd season"), FolderRole::Season(2));
        assert_eq!(classify_folder("Season 3"), FolderRole::Season(3));
    }

    /// What `build_rows` asks before keeping a filename's leftovers as the
    /// episode's name.
    #[test]
    fn a_season_marker_is_recognised_as_one() {
        for name in ["3rd Season", "2nd season", "Season 2", "S03", "season 01"] {
            assert!(is_season_marker(name), "{name} states only a season");
        }
        for name in ["Mother and Children", "The End of Evangelion", ""] {
            assert!(!is_season_marker(name), "{name} is a name");
        }
    }

    /// A title beginning with `E` is not an episode marker.
    #[test]
    fn a_word_beginning_with_e_does_not_start_an_episode() {
        assert_eq!(parse("Evangelion.mkv").episode, None);
        assert_eq!(parse("The End of Evangelion.mkv").episode, None);
    }

    /// Season folders state a season and nothing else.
    #[test]
    fn season_folders_are_classified_as_seasons() {
        assert_eq!(classify_folder("Season 01"), FolderRole::Season(1));
        assert_eq!(classify_folder("season 2"), FolderRole::Season(2));
        assert_eq!(classify_folder("S03"), FolderRole::Season(3));
    }

    /// A folder that names the series *and* its season must give up both, or
    /// the shelf shows one poster per season of the same show.
    #[test]
    fn a_folder_naming_series_and_season_yields_both() {
        for (name, series, season) in [
            ("Game of Thrones - Season 1", "Game of Thrones", 1),
            ("Game of Thrones Season 10", "Game of Thrones", 10),
            ("The Expanse - S03", "The Expanse", 3),
            ("Breaking Bad.season.2", "Breaking Bad", 2),
        ] {
            let FolderRole::Title(parsed) = classify_folder(name) else {
                panic!("{name} was not classified as a title");
            };
            assert_eq!(parsed.title, series, "{name}");
            assert_eq!(parsed.season, Some(season), "{name}");
        }
    }

    /// The keyword is what makes it a season. A title that merely ends in a
    /// number keeps it.
    #[test]
    fn a_title_ending_in_a_number_keeps_it() {
        for name in ["Evangelion 3", "Kaijuu 8", "Alien 3"] {
            let FolderRole::Title(parsed) = classify_folder(name) else {
                panic!("{name} was not classified as a title");
            };
            assert_eq!(parsed.season, None, "{name}");
            assert_eq!(parsed.title, name, "{name}");
        }

        // A four-digit tail is read as a year, as it is everywhere else — but
        // still never as a season.
        let FolderRole::Title(parsed) = classify_folder("Blade Runner 2049") else {
            panic!("not a title");
        };
        assert_eq!(parsed.season, None);
        assert_eq!(parsed.year, Some(2049));
    }

    /// Grouping folders name nothing, so a title has to come from above them.
    #[test]
    fn grouping_folders_are_classified_as_containers() {
        for name in ["Movies", "OVA", "Specials", "extras", "NCED"] {
            assert_eq!(
                classify_folder(name),
                FolderRole::Container,
                "{name} groups without naming"
            );
        }
    }

    /// Everything else names the series or film — and is parsed, so a year in
    /// the folder name is picked up too.
    #[test]
    fn other_folders_name_the_series() {
        match classify_folder("Ghost in the Shell (1995)") {
            FolderRole::Title(parsed) => {
                assert_eq!(parsed.title, "Ghost in the Shell");
                assert_eq!(parsed.year, Some(1995));
            }
            other => panic!("expected a title, got {other:?}"),
        }
        // A folder that merely contains the word is not a container.
        assert!(matches!(
            classify_folder("Satoshi Kon Movies"),
            FolderRole::Title(_)
        ));
    }

    /// A share holds `.nfo` and `.jpg` next to the video; a catalog that offers
    /// those as episodes is worse than one missing an exotic container.
    #[test]
    fn only_playable_containers_count_as_video() {
        for name in ["a.mkv", "a.MP4", "a.webm", "a.m2ts", "a.avi"] {
            assert!(is_video_file(name), "{name} is playable");
        }
        for name in ["torrent.info.nfo", "poster.jpg", "readme.txt", "noext"] {
            assert!(!is_video_file(name), "{name} is not");
        }
    }

    /// `is_episode` is what the catalog groups on.
    #[test]
    fn episodes_and_films_are_distinguishable() {
        assert!(parse("Show.Name.S01E01.mkv").is_episode());
        assert!(!parse("Movie Title (2019).mkv").is_episode());
    }
}
