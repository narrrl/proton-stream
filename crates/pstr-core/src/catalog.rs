//! The catalog: what a crawl found, and where playback left off.
//!
//! SQLite, and — following `proton-drive-linux` — **forward-only**: never edit a
//! shipped `MIGRATION_V*`, add a new one and bump [`SCHEMA_VERSION`]. A database
//! newer than this build understands is a hard refuse-to-open rather than a
//! downgrade, because silently reinterpreting a newer schema is how watch state
//! gets corrupted.
//!
//! Unlike the daemon's cache, nothing here is irreplaceable: the catalog is
//! rebuilt by crawling again. Watch positions are the exception, which is why
//! they live in their own table keyed by `(share_id, link_id)` — stable across
//! recrawls, and untouched when the node rows are replaced.

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::Result;
use crate::library::TitleKind;
use crate::metadata::{
    Enrichment, EpisodeGuide, EpisodeMetadata, MetadataRecord, ProviderId, TitleDetails,
    TitleMetadata,
};
use crate::naming::{self, ParsedName};

/// Bump when adding a migration.
pub const SCHEMA_VERSION: i64 = 12;

const MIGRATION_V1: &str = r#"
CREATE TABLE nodes (
    share_id            TEXT NOT NULL,
    link_id             TEXT NOT NULL,
    volume_id           TEXT NOT NULL,
    parent_link_id      TEXT,
    name                TEXT NOT NULL,
    is_folder           INTEGER NOT NULL,
    media_type          TEXT,
    size                INTEGER,
    active_revision_id  TEXT,
    title               TEXT,
    season              INTEGER,
    episode             INTEGER,
    year                INTEGER,
    PRIMARY KEY (share_id, link_id)
);
CREATE INDEX nodes_by_parent ON nodes (share_id, parent_link_id);
CREATE INDEX nodes_by_title  ON nodes (title, season, episode);

CREATE TABLE watch_state (
    share_id       TEXT NOT NULL,
    link_id        TEXT NOT NULL,
    position_secs  REAL NOT NULL,
    duration_secs  REAL,
    watched        INTEGER NOT NULL DEFAULT 0,
    updated_at     INTEGER NOT NULL,
    PRIMARY KEY (share_id, link_id)
);
"#;

/// Episode names, which the first schema had nowhere to put: a library that
/// numbers its files (`S01E01-Mother and Children.mkv`) has the episode's own
/// name in the filename, and a poster-wall UI wants it.
const MIGRATION_V2: &str = r#"
ALTER TABLE nodes ADD COLUMN episode_title TEXT;
"#;

/// What a metadata provider said about a title, including that it said nothing.
///
/// Keyed by [`crate::library::title_key`] rather than by anything from a share:
/// a title merges across shares, and the answer is about the *title*. That also
/// means these rows survive a recrawl, like watch state and unlike node rows —
/// re-matching a whole library because one share was refreshed would be a few
/// hundred needless requests to a third party.
///
/// `matched = 0` is a real, cached answer: the provider was asked and had
/// nothing. `genres` is a newline-separated list, which is enough for something
/// never queried by genre and avoids a table for it.
const MIGRATION_V3: &str = r#"
CREATE TABLE title_metadata (
    title_key      TEXT PRIMARY KEY,
    provider       TEXT NOT NULL,
    matched        INTEGER NOT NULL,
    fetched_at     INTEGER NOT NULL,
    remote_id      TEXT,
    name           TEXT,
    original_name  TEXT,
    overview       TEXT,
    year           INTEGER,
    kind           TEXT,
    poster_url     TEXT,
    backdrop_url   TEXT,
    rating         REAL,
    genres         TEXT,
    episodes       INTEGER,
    url            TEXT
);
"#;

/// What a provider said about the individual episodes of a title.
///
/// Separate from `title_metadata` rather than a JSON blob in it, because this
/// is looked up per row while a season is drawn — once per episode, hundreds of
/// times a scroll — and because a title's answer and its episode list are
/// fetched on different requests and can each exist without the other.
///
/// `season = -1` stands for the provider numbering episodes straight through
/// rather than by season; SQLite has no nullable primary-key column that would
/// compare equal, and a sentinel here is cheaper than a second index.
const MIGRATION_V4: &str = r#"
CREATE TABLE episode_metadata (
    title_key   TEXT NOT NULL,
    season      INTEGER NOT NULL,
    number      INTEGER NOT NULL,
    provider    TEXT NOT NULL,
    fetched_at  INTEGER NOT NULL,
    name        TEXT,
    overview    TEXT,
    still_url   TEXT,
    air_date    TEXT,
    PRIMARY KEY (title_key, season, number)
);
"#;

/// Which rows the viewer chose themselves.
///
/// A column rather than a separate table of overrides, because it changes
/// nothing about what the row *holds* — only about who put it there, and
/// therefore whether a match run may replace it. See
/// [`MetadataRecord::manual`].
const MIGRATION_V5: &str = r#"
ALTER TABLE title_metadata ADD COLUMN manual INTEGER NOT NULL DEFAULT 0;
"#;

/// Content kept explicitly by the viewer.  Unlike the block cache, these rows
/// describe complete local files and therefore survive a disconnected launch.
const MIGRATION_V6: &str = r#"
CREATE TABLE offline_files (
    share_id TEXT NOT NULL, link_id TEXT NOT NULL, revision_id TEXT NOT NULL,
    block_sizes TEXT NOT NULL,
    PRIMARY KEY (share_id, link_id)
);
"#;

/// Language choices are a property of a show, not of an episode's muxing.
const MIGRATION_V7: &str = r#"
CREATE TABLE title_track_prefs (
    title_key TEXT PRIMARY KEY, audio_language TEXT, subtitle_language TEXT,
    subtitles INTEGER NOT NULL DEFAULT 1
);
"#;

/// A language cannot tell two English subtitle tracks apart — "Signs & Songs"
/// and "Full Subtitles" — so the track's title is remembered beside it.
const MIGRATION_V8: &str = r#"
ALTER TABLE title_track_prefs ADD COLUMN audio_title TEXT;
ALTER TABLE title_track_prefs ADD COLUMN subtitle_title TEXT;
"#;

/// Episodes numbered three ways, and a record of which titles were enriched.
///
/// A release numbers its files by TVDB season, by absolute number inside a
/// season folder, or by AniList entry; an episode row now carries all three so
/// `EpisodeGuide::get` can meet whichever the filename used. `enriched_at` is
/// set once a matched title's episodes and art were asked for — empty answers
/// included, which is what stops a film being re-asked on every run.
///
/// The deletes are a repair, not a reset. Every AniList episode row came from
/// `streamingEpisodes` — partial, numbered however a streaming site numbered
/// it, and in places a later season captioned with season one's names — and
/// every AniList backdrop was a 4.75:1 banner drawn into a 16:9 tile. Both are
/// wrong rather than stale, so they go; the matches stay, hand-picked ones
/// included, and the next match run enriches them again. See
/// `docs/METADATA.md`.
const MIGRATION_V9: &str = r#"
ALTER TABLE episode_metadata ADD COLUMN absolute INTEGER;
ALTER TABLE episode_metadata ADD COLUMN entry_season INTEGER;
ALTER TABLE episode_metadata ADD COLUMN entry_number INTEGER;
ALTER TABLE title_metadata ADD COLUMN enriched_at INTEGER;
DELETE FROM episode_metadata WHERE provider = 'anilist';
UPDATE title_metadata SET backdrop_url = NULL WHERE provider = 'anilist';
"#;

/// What the library is sorted, grouped and shelved by, and when each file
/// first appeared.
///
/// `details` is [`TitleDetails`] as JSON — see there for why one column.
/// `NULL` means it was never fetched, which a match run reads as "fetch it"
/// rather than as "there is none"; every existing match starts that way.
/// `chain` is the provider ids enrichment walked, newline-separated; every
/// title is marked un-enriched so the next run records it.
///
/// `node_first_seen` is kept apart from `nodes` because a recrawl replaces
/// every node row and "recently added" has to survive that. Files already in
/// the catalog are recorded as seen at 0 — when they arrived is not known, and
/// pretending it was today would put the whole library at the top.
const MIGRATION_V10: &str = r#"
ALTER TABLE title_metadata ADD COLUMN details TEXT;
ALTER TABLE title_metadata ADD COLUMN chain TEXT;
CREATE TABLE node_first_seen (
    share_id  TEXT NOT NULL,
    link_id   TEXT NOT NULL,
    seen_at   INTEGER NOT NULL,
    PRIMARY KEY (share_id, link_id)
);
INSERT OR IGNORE INTO node_first_seen (share_id, link_id, seen_at)
    SELECT share_id, link_id, 0 FROM nodes WHERE is_folder = 0;
UPDATE title_metadata SET enriched_at = NULL;
"#;

/// What the last crawl of each share came to.
///
/// Per installation, like the rest of the catalog: when this device last
/// looked at a share is nothing another device needs. `crawled_at` is the last
/// crawl that finished, `attempted_at` the last that was tried, so a share that
/// keeps failing is not retried on every launch. `added` is how many files the
/// last finished crawl found that had never been seen before.
const MIGRATION_V11: &str = r#"
CREATE TABLE share_crawls (
    share_id      TEXT PRIMARY KEY,
    crawled_at    INTEGER NOT NULL,
    attempted_at  INTEGER NOT NULL,
    files         INTEGER NOT NULL,
    added         INTEGER NOT NULL,
    error         TEXT
);
"#;

/// Subtitle files beside a video, which the crawl used to drop with the
/// `.nfo` files. Rebuilt by every crawl, like `nodes`; a subtitle belongs to
/// one video, the one it was paired with by name.
const MIGRATION_V12: &str = r#"
CREATE TABLE sidecars (
    share_id       TEXT NOT NULL,
    link_id        TEXT NOT NULL,
    volume_id      TEXT NOT NULL,
    video_link_id  TEXT NOT NULL,
    name           TEXT NOT NULL,
    language       TEXT,
    title          TEXT,
    PRIMARY KEY (share_id, link_id)
);
CREATE INDEX sidecars_by_video ON sidecars (share_id, video_link_id);
"#;

/// How old a share's last crawl may be before an app looks again on its own.
/// Long enough that a launch rarely costs a crawl; short enough that a new
/// episode turns up the same day.
pub const CRAWL_STALE_AFTER_SECS: i64 = 6 * 60 * 60;

/// One share's row of `share_crawls`. See [`MIGRATION_V11`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrawlRecord {
    /// Unix seconds; `0` for a share no crawl of has finished.
    pub crawled_at: i64,
    pub attempted_at: i64,
    /// Playable files the last finished crawl stored.
    pub files: u64,
    /// Of those, how many no earlier crawl had seen.
    pub added: u64,
    /// Why the last attempt failed, when it did.
    pub error: Option<String>,
}

impl CrawlRecord {
    /// Whether the share is due another look: nothing has been tried within
    /// `age_secs` of `now`.
    pub fn is_stale(&self, now: i64, age_secs: i64) -> bool {
        now - self.crawled_at.max(self.attempted_at) >= age_secs
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflineFile {
    pub revision_id: String,
    pub block_sizes: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TitleTrackPrefs {
    pub audio_language: Option<String>,
    pub subtitle_language: Option<String>,
    pub subtitles: bool,
    /// The name the muxer gave the chosen audio track, when it had one. What
    /// picks between two tracks in the same language on the next episode.
    #[serde(default)]
    pub audio_title: Option<String>,
    #[serde(default)]
    pub subtitle_title: Option<String>,
}
impl Default for TitleTrackPrefs {
    fn default() -> Self {
        Self {
            audio_language: None,
            subtitle_language: None,
            subtitles: true,
            audio_title: None,
            subtitle_title: None,
        }
    }
}

/// One `title_track_prefs` row, its columns starting at `first`.
fn title_track_prefs_row(
    row: &rusqlite::Row<'_>,
    first: usize,
) -> rusqlite::Result<TitleTrackPrefs> {
    Ok(TitleTrackPrefs {
        audio_language: row.get(first)?,
        subtitle_language: row.get(first + 1)?,
        subtitles: row.get::<_, i64>(first + 2)? != 0,
        audio_title: row.get(first + 3)?,
        subtitle_title: row.get(first + 4)?,
    })
}

/// What `season = -1` means in `episode_metadata`: no season at all.
const ABSOLUTE_SEASON: i64 = -1;

/// How long after an episode airs its guide keeps being asked about daily
/// while the provider has no name for it.
///
/// A guide is otherwise trusted as long as a match, a month, and one fetched
/// the week before an episode aired lists it unnamed — "Episode 2" with a
/// date — until the month is up. A name usually follows the air date by a day
/// or so; one that has not come in two weeks is not coming, and asking every
/// day for the rest of time would not change that.
const GUIDE_WAIT_DAYS: i64 = 14;

/// One catalog row: a file or folder inside a share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogNode {
    pub share_id: String,
    pub link_id: String,
    pub volume_id: String,
    pub parent_link_id: Option<String>,
    pub name: String,
    pub is_folder: bool,
    pub media_type: Option<String>,
    /// Plaintext size as claimed by the uploader, when the share reports one.
    pub size: Option<i64>,
    /// The revision the row describes. This is the cache-validity key: it
    /// advances if and only if a new revision was sealed, which `(mtime, size)`
    /// only approximates.
    pub active_revision_id: Option<String>,
    /// What [`naming::parse`] made of the file name.
    pub parsed: ParsedName,
}

/// A subtitle file paired with the video beside it. See [`build_sidecars`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sidecar {
    pub share_id: String,
    pub link_id: String,
    pub volume_id: String,
    /// The video this subtitle is for.
    pub video_link_id: String,
    pub name: String,
    /// The [`crate::language::LANGUAGES`] tag its name states, if any.
    pub language: Option<String>,
    /// What else its name says about it — "forced", "SDH" — for the picker.
    pub title: Option<String>,
}

/// Where playback left off.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WatchState {
    pub position_secs: f64,
    pub duration_secs: Option<f64>,
    pub watched: bool,
    pub updated_at: i64,
}

/// The catalog database.
pub struct Catalog {
    conn: Connection,
}

impl Catalog {
    /// Open (or create) the catalog at `path`.
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    /// An in-memory catalog, for tests.
    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let mut catalog = Self { conn };
        catalog.migrate()?;
        Ok(catalog)
    }

    /// How many rows this connection has written since it was opened.
    ///
    /// A cache key, not a statistic: anything derived from the catalog stays
    /// valid exactly as long as this does not move. SQLite counts it for us, so
    /// no write path has to remember to invalidate anything — which is the
    /// failure mode a hand-maintained counter has.
    pub fn writes(&self) -> u64 {
        self.conn.total_changes()
    }

    fn migrate(&mut self) -> Result<()> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;

        if version > SCHEMA_VERSION {
            // Refuse rather than guess. A newer build wrote this; reinterpreting
            // it under older rules would silently mangle watch state.
            return Err(crate::error::Error::Config(format!(
                "catalog schema is version {version}, newer than this build supports \
                 ({SCHEMA_VERSION}); upgrade proton-stream or delete the catalog"
            )));
        }

        if version < 1 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(MIGRATION_V1)?;
            tx.pragma_update(None, "user_version", 1)?;
            tx.commit()?;
        }

        if version < 2 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(MIGRATION_V2)?;
            tx.pragma_update(None, "user_version", 2)?;
            tx.commit()?;
        }

        if version < 3 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(MIGRATION_V3)?;
            tx.pragma_update(None, "user_version", 3)?;
            tx.commit()?;
        }

        if version < 4 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(MIGRATION_V4)?;
            tx.pragma_update(None, "user_version", 4)?;
            tx.commit()?;
        }

        if version < 5 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(MIGRATION_V5)?;
            tx.pragma_update(None, "user_version", 5)?;
            tx.commit()?;
        }
        if version < 6 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(MIGRATION_V6)?;
            tx.pragma_update(None, "user_version", 6)?;
            tx.commit()?;
        }
        if version < 7 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(MIGRATION_V7)?;
            tx.pragma_update(None, "user_version", 7)?;
            tx.commit()?;
        }
        if version < 8 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(MIGRATION_V8)?;
            tx.pragma_update(None, "user_version", 8)?;
            tx.commit()?;
        }
        if version < 9 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(MIGRATION_V9)?;
            tx.pragma_update(None, "user_version", 9)?;
            tx.commit()?;
        }
        if version < 10 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(MIGRATION_V10)?;
            tx.pragma_update(None, "user_version", 10)?;
            tx.commit()?;
        }
        if version < 11 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(MIGRATION_V11)?;
            tx.pragma_update(None, "user_version", 11)?;
            tx.commit()?;
        }
        if version < 12 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(MIGRATION_V12)?;
            tx.pragma_update(None, "user_version", 12)?;
            tx.commit()?;
        }

        Ok(())
    }

    /// Replace everything known about one share with `nodes`.
    ///
    /// Done as one transaction so a failed crawl leaves the previous catalog
    /// intact rather than a half-replaced one. Watch state is keyed separately
    /// and deliberately untouched — a recrawl must not lose where you were.
    pub fn replace_share(&mut self, share_id: &str, nodes: &[CatalogNode]) -> Result<()> {
        self.replace_share_inner(share_id, nodes, false)
    }

    /// Replace nodes while retaining stale offline path keys for explicit
    /// filesystem cleanup by a front end.
    pub fn replace_share_retaining_offline(
        &mut self,
        share_id: &str,
        nodes: &[CatalogNode],
    ) -> Result<()> {
        self.replace_share_inner(share_id, nodes, true)
    }

    fn replace_share_inner(
        &mut self,
        share_id: &str,
        nodes: &[CatalogNode],
        retain_offline: bool,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM nodes WHERE share_id = ?1", params![share_id])?;

        {
            let mut insert = tx.prepare(
                "INSERT INTO nodes (
                     share_id, link_id, volume_id, parent_link_id, name, is_folder,
                     media_type, size, active_revision_id, title, season, episode, year,
                     episode_title
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            )?;
            for node in nodes {
                insert.execute(params![
                    node.share_id,
                    node.link_id,
                    node.volume_id,
                    node.parent_link_id,
                    node.name,
                    node.is_folder as i64,
                    node.media_type,
                    node.size,
                    node.active_revision_id,
                    node.parsed.title,
                    node.parsed.season,
                    node.parsed.episode,
                    node.parsed.year,
                    node.parsed.episode_title,
                ])?;
            }
        }

        // Only files: a folder appearing is not something anyone watches.
        let now = unix_now();
        let added = tx.execute(
            "INSERT OR IGNORE INTO node_first_seen (share_id, link_id, seen_at)
             SELECT share_id, link_id, ?2 FROM nodes WHERE share_id = ?1 AND is_folder = 0",
            params![share_id, now],
        )?;
        let files = nodes.iter().filter(|node| !node.is_folder).count();
        tx.execute(
            "INSERT INTO share_crawls (share_id, crawled_at, attempted_at, files, added, error)
             VALUES (?1, ?2, ?2, ?3, ?4, NULL)
             ON CONFLICT (share_id) DO UPDATE SET
                 crawled_at   = excluded.crawled_at,
                 attempted_at = excluded.attempted_at,
                 files        = excluded.files,
                 added        = excluded.added,
                 error        = NULL",
            params![share_id, now, files as i64, added as i64],
        )?;

        if !retain_offline {
            tx.execute("DELETE FROM offline_files WHERE share_id = ?1 AND NOT EXISTS (SELECT 1 FROM nodes WHERE nodes.share_id = offline_files.share_id AND nodes.link_id = offline_files.link_id AND nodes.active_revision_id = offline_files.revision_id)", params![share_id])?;
        }

        tx.commit()?;
        Ok(())
    }

    /// Replace one share's subtitle files with `sidecars`.
    ///
    /// Apart from [`Self::replace_share`] rather than inside it: a subtitle
    /// left over from the crawl before is one that fails to open, quietly,
    /// which is no reason to make every caller of that pass these along.
    pub fn replace_sidecars(&mut self, share_id: &str, sidecars: &[Sidecar]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM sidecars WHERE share_id = ?1",
            params![share_id],
        )?;
        {
            let mut insert = tx.prepare(
                "INSERT OR REPLACE INTO sidecars (
                     share_id, link_id, volume_id, video_link_id, name, language, title
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for sidecar in sidecars {
                insert.execute(params![
                    share_id,
                    sidecar.link_id,
                    sidecar.volume_id,
                    sidecar.video_link_id,
                    sidecar.name,
                    sidecar.language,
                    sidecar.title,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The subtitle files paired with one video, in name order.
    pub fn sidecars(&self, share_id: &str, video_link_id: &str) -> Result<Vec<Sidecar>> {
        let mut select = self.conn.prepare_cached(
            "SELECT share_id, link_id, volume_id, video_link_id, name, language, title
             FROM sidecars WHERE share_id = ?1 AND video_link_id = ?2 ORDER BY name",
        )?;
        let rows = select.query_map(params![share_id, video_link_id], |row| {
            Ok(Sidecar {
                share_id: row.get(0)?,
                link_id: row.get(1)?,
                volume_id: row.get(2)?,
                video_link_id: row.get(3)?,
                name: row.get(4)?,
                language: row.get(5)?,
                title: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Record that a crawl of `share_id` failed, keeping what the last
    /// finished one stored.
    pub fn record_crawl_failure(&self, share_id: &str, error: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO share_crawls (share_id, crawled_at, attempted_at, files, added, error)
             VALUES (?1, 0, ?2, 0, 0, ?3)
             ON CONFLICT (share_id) DO UPDATE SET
                 attempted_at = excluded.attempted_at,
                 error        = excluded.error",
            params![share_id, unix_now(), error],
        )?;
        Ok(())
    }

    /// What the last crawl of each share came to, by share id. A share never
    /// crawled has no entry.
    pub fn crawls(&self) -> Result<HashMap<String, CrawlRecord>> {
        let mut statement = self.conn.prepare(
            "SELECT share_id, crawled_at, attempted_at, files, added, error FROM share_crawls",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                CrawlRecord {
                    crawled_at: row.get(1)?,
                    attempted_at: row.get(2)?,
                    files: row.get::<_, i64>(3)?.max(0) as u64,
                    added: row.get::<_, i64>(4)?.max(0) as u64,
                    error: row.get(5)?,
                },
            ))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Every file in a share, folders excluded, ordered for display.
    pub fn files(&self, share_id: &str) -> Result<Vec<CatalogNode>> {
        let mut statement = self.conn.prepare(
            "SELECT share_id, link_id, volume_id, parent_link_id, name, is_folder,
                    media_type, size, active_revision_id, title, season, episode, year,
                    episode_title
             FROM nodes
             WHERE share_id = ?1 AND is_folder = 0
             ORDER BY title, season, episode, name",
        )?;
        let rows = statement.query_map(params![share_id], row_to_node)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Every file across every share, grouped by parsed title.
    pub fn all_files(&self) -> Result<Vec<CatalogNode>> {
        let mut statement = self.conn.prepare(
            "SELECT share_id, link_id, volume_id, parent_link_id, name, is_folder,
                    media_type, size, active_revision_id, title, season, episode, year,
                    episode_title
             FROM nodes
             WHERE is_folder = 0
             ORDER BY title, season, episode, name",
        )?;
        let rows = statement.query_map([], row_to_node)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// When each file first appeared in a crawl, keyed `(share_id, link_id)`.
    /// `0` for a file that was already there before this was recorded.
    pub fn first_seen(&self) -> Result<HashMap<(String, String), i64>> {
        let mut statement = self
            .conn
            .prepare("SELECT share_id, link_id, seen_at FROM node_first_seen")?;
        let rows = statement.query_map([], |row| {
            Ok(((row.get(0)?, row.get(1)?), row.get::<_, i64>(2)?))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Every share the catalog holds files for. One no longer configured was
    /// forgotten without [`Catalog::remove_share`] — by an older `pstr remove`
    /// — and its rows still show up in the library beside whatever replaced it.
    pub fn share_ids(&self) -> Result<Vec<String>> {
        let mut statement = self
            .conn
            .prepare("SELECT DISTINCT share_id FROM nodes ORDER BY share_id")?;
        let rows = statement.query_map([], |row| row.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Drop everything a share contributed except where it was watched.
    ///
    /// Watch state stays, as through a recrawl. It is history, and not only
    /// this device's: with an account signed in, the next sync would bring it
    /// straight back from the other devices' files, so deleting it here would
    /// only make it flicker. Nothing shows it — the library pairs positions
    /// with files, and the files are gone — until the share is added again,
    /// when it picks up where it left off.
    ///
    /// It is also copied, newer wins as in a merge, onto every other share that
    /// holds the same file. A public link swapped for the account's own folder
    /// reaches the very same links under a new share id, and without the copy
    /// the swap reads as a library nobody has watched.
    pub fn remove_share(&mut self, share_id: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO watch_state (share_id, link_id, position_secs, duration_secs, watched, updated_at)
             SELECT other.share_id, w.link_id, w.position_secs, w.duration_secs, w.watched, w.updated_at
             FROM watch_state w
             JOIN nodes own ON own.share_id = w.share_id AND own.link_id = w.link_id
             JOIN nodes other ON other.volume_id = own.volume_id
                             AND other.link_id = own.link_id
                             AND other.share_id <> own.share_id
             WHERE w.share_id = ?1
             ON CONFLICT (share_id, link_id) DO UPDATE SET
                 position_secs = excluded.position_secs,
                 duration_secs = excluded.duration_secs,
                 watched       = excluded.watched,
                 updated_at    = excluded.updated_at
             WHERE excluded.updated_at > watch_state.updated_at
                OR (excluded.updated_at = watch_state.updated_at
                    AND (excluded.watched > watch_state.watched
                         OR (excluded.watched = watch_state.watched
                             AND excluded.position_secs > watch_state.position_secs)))",
            params![share_id],
        )?;
        tx.execute("DELETE FROM nodes WHERE share_id = ?1", params![share_id])?;
        tx.execute(
            "DELETE FROM sidecars WHERE share_id = ?1",
            params![share_id],
        )?;
        tx.execute(
            "DELETE FROM offline_files WHERE share_id = ?1",
            params![share_id],
        )?;
        tx.execute(
            "DELETE FROM node_first_seen WHERE share_id = ?1",
            params![share_id],
        )?;
        tx.execute(
            "DELETE FROM share_crawls WHERE share_id = ?1",
            params![share_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn offline_file(&self, share_id: &str, link_id: &str) -> Result<Option<OfflineFile>> {
        self.conn.query_row("SELECT revision_id, block_sizes FROM offline_files WHERE share_id=?1 AND link_id=?2", params![share_id, link_id], |row| {
            let sizes: String = row.get(1)?;
            Ok(OfflineFile { revision_id: row.get(0)?, block_sizes: serde_json::from_str(&sizes).map_err(|e| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e)))? })
        }).optional().map_err(Into::into)
    }

    pub fn set_offline_file(
        &self,
        share_id: &str,
        link_id: &str,
        file: &OfflineFile,
    ) -> Result<()> {
        let sizes = serde_json::to_string(&file.block_sizes)
            .map_err(|e| crate::error::Error::Config(e.to_string()))?;
        self.conn.execute("INSERT INTO offline_files (share_id,link_id,revision_id,block_sizes) VALUES (?1,?2,?3,?4) ON CONFLICT(share_id,link_id) DO UPDATE SET revision_id=excluded.revision_id,block_sizes=excluded.block_sizes", params![share_id,link_id,file.revision_id,sizes])?;
        Ok(())
    }

    /// Record a completed download and force its WAL transaction to stable
    /// storage before the caller discards its recovery journal.
    pub fn set_offline_file_durable(
        &self,
        share_id: &str,
        link_id: &str,
        file: &OfflineFile,
    ) -> Result<()> {
        self.set_offline_file(share_id, link_id, file)?;
        self.conn
            .query_row("PRAGMA wal_checkpoint(FULL)", [], |_| Ok(()))?;
        Ok(())
    }

    /// Every completed local revision, keyed like watch state.
    pub fn all_offline_files(&self) -> Result<HashMap<(String, String), OfflineFile>> {
        let mut statement = self
            .conn
            .prepare("SELECT share_id, link_id, revision_id, block_sizes FROM offline_files")?;
        let rows = statement.query_map([], |row| {
            let sizes: String = row.get(3)?;
            let block_sizes = serde_json::from_str(&sizes).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            Ok((
                (row.get(0)?, row.get(1)?),
                OfflineFile {
                    revision_id: row.get(2)?,
                    block_sizes,
                },
            ))
        })?;
        Ok(rows.collect::<std::result::Result<HashMap<_, _>, _>>()?)
    }

    /// Completed local revisions belonging to one share.
    ///
    /// Callers that replace or remove a share need the paths before deleting
    /// the index rows. Keeping this query explicit prevents an SQL cascade
    /// from orphaning opaque media files on disk.
    pub fn offline_files_for_share(&self, share_id: &str) -> Result<HashMap<String, OfflineFile>> {
        let mut statement = self.conn.prepare(
            "SELECT link_id, revision_id, block_sizes FROM offline_files WHERE share_id=?1",
        )?;
        let rows = statement.query_map(params![share_id], |row| {
            let sizes: String = row.get(2)?;
            let block_sizes = serde_json::from_str(&sizes).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            Ok((
                row.get(0)?,
                OfflineFile {
                    revision_id: row.get(1)?,
                    block_sizes,
                },
            ))
        })?;
        Ok(rows.collect::<std::result::Result<HashMap<_, _>, _>>()?)
    }

    pub fn remove_offline_file(&self, share_id: &str, link_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM offline_files WHERE share_id=?1 AND link_id=?2",
            params![share_id, link_id],
        )?;
        Ok(())
    }

    pub fn title_track_prefs(&self, title_key: &str) -> Result<Option<TitleTrackPrefs>> {
        self.conn
            .query_row(
                "SELECT audio_language, subtitle_language, subtitles, audio_title, subtitle_title \
                 FROM title_track_prefs WHERE title_key=?1",
                params![title_key],
                |row| title_track_prefs_row(row, 0),
            )
            .optional()
            .map_err(Into::into)
    }
    pub fn set_title_track_prefs(&self, title_key: &str, prefs: &TitleTrackPrefs) -> Result<()> {
        self.conn.execute(
            "INSERT INTO title_track_prefs \
             (title_key,audio_language,subtitle_language,subtitles,audio_title,subtitle_title) \
             VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(title_key) DO UPDATE SET \
             audio_language=excluded.audio_language,subtitle_language=excluded.subtitle_language,\
             subtitles=excluded.subtitles,audio_title=excluded.audio_title,\
             subtitle_title=excluded.subtitle_title",
            params![
                title_key,
                prefs.audio_language,
                prefs.subtitle_language,
                prefs.subtitles as i64,
                prefs.audio_title,
                prefs.subtitle_title
            ],
        )?;
        Ok(())
    }

    pub fn all_title_track_prefs(&self) -> Result<HashMap<String, TitleTrackPrefs>> {
        let mut statement = self.conn.prepare(
            "SELECT title_key, audio_language, subtitle_language, subtitles, audio_title, \
             subtitle_title FROM title_track_prefs",
        )?;
        let rows =
            statement.query_map([], |row| Ok((row.get(0)?, title_track_prefs_row(row, 1)?)))?;
        Ok(rows.collect::<std::result::Result<HashMap<_, _>, _>>()?)
    }

    /// Every watch position there is, keyed by `(share_id, link_id)`.
    ///
    /// What [`crate::library::Library::build`] joins against. A grid that asked
    /// per tile would pay a round-trip per poster on every render.
    pub fn all_watch_states(&self) -> Result<HashMap<(String, String), WatchState>> {
        let mut statement = self.conn.prepare(
            "SELECT share_id, link_id, position_secs, duration_secs, watched, updated_at
             FROM watch_state",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                (row.get(0)?, row.get(1)?),
                WatchState {
                    position_secs: row.get(2)?,
                    duration_secs: row.get(3)?,
                    watched: row.get::<_, i64>(4)? != 0,
                    updated_at: row.get(5)?,
                },
            ))
        })?;
        Ok(rows.collect::<std::result::Result<HashMap<_, _>, _>>()?)
    }

    /// Record where playback is.
    pub fn set_watch_state(&self, share_id: &str, link_id: &str, state: &WatchState) -> Result<()> {
        // Cached: playback saves its position every few seconds, and this is
        // the one write that runs on a clock rather than on a click.
        let mut upsert = self.conn.prepare_cached(
            "INSERT INTO watch_state (share_id, link_id, position_secs, duration_secs, watched, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (share_id, link_id) DO UPDATE SET
                 position_secs = excluded.position_secs,
                 duration_secs = excluded.duration_secs,
                 watched       = excluded.watched,
                 updated_at    = excluded.updated_at",
        )?;
        upsert.execute(params![
            share_id,
            link_id,
            state.position_secs,
            state.duration_secs,
            state.watched as i64,
            state.updated_at,
        ])?;
        Ok(())
    }

    /// Take each of `states` that is newer than what is recorded, in one
    /// transaction, returning how many were taken.
    ///
    /// Newer is a later `updated_at`. A tie is broken by the state itself —
    /// watched over not, then the further position — rather than by who is
    /// asking, so two devices holding the same tie settle on the same answer
    /// instead of each keeping its own forever. See [`crate::sync`].
    pub fn merge_watch_states(&mut self, states: &[(String, String, WatchState)]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut taken = 0;
        {
            let mut upsert = tx.prepare_cached(
                "INSERT INTO watch_state (share_id, link_id, position_secs, duration_secs, watched, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (share_id, link_id) DO UPDATE SET
                     position_secs = excluded.position_secs,
                     duration_secs = excluded.duration_secs,
                     watched       = excluded.watched,
                     updated_at    = excluded.updated_at
                 WHERE excluded.updated_at > watch_state.updated_at
                    OR (excluded.updated_at = watch_state.updated_at
                        AND (excluded.watched > watch_state.watched
                             OR (excluded.watched = watch_state.watched
                                 AND excluded.position_secs > watch_state.position_secs)))",
            )?;
            for (share_id, link_id, state) in states {
                taken += upsert.execute(params![
                    share_id,
                    link_id,
                    state.position_secs,
                    state.duration_secs,
                    state.watched as i64,
                    state.updated_at,
                ])?;
            }
        }
        tx.commit()?;
        Ok(taken)
    }

    /// Where playback left off, if it ever started.
    pub fn watch_state(&self, share_id: &str, link_id: &str) -> Result<Option<WatchState>> {
        let state = self
            .conn
            .query_row(
                "SELECT position_secs, duration_secs, watched, updated_at
                 FROM watch_state WHERE share_id = ?1 AND link_id = ?2",
                params![share_id, link_id],
                |row| {
                    Ok(WatchState {
                        position_secs: row.get(0)?,
                        duration_secs: row.get(1)?,
                        watched: row.get::<_, i64>(2)? != 0,
                        updated_at: row.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(state)
    }

    // -------------------------------------------------------------- metadata

    /// Every stored provider answer, keyed by title key.
    ///
    /// Read whole, like watch state and for the same reason: the poster wall
    /// wants all of it at once, and asking per tile is a round-trip per tile.
    /// Rows naming a provider this build does not know are dropped rather than
    /// failing the read — the title simply looks unmatched and gets re-asked.
    pub fn all_metadata(&self) -> Result<HashMap<String, MetadataRecord>> {
        let mut statement = self.conn.prepare(
            "SELECT title_key, provider, matched, fetched_at, remote_id, name, original_name,
                    overview, year, kind, poster_url, backdrop_url, rating, genres, episodes, url,
                    manual, details, chain
             FROM title_metadata",
        )?;
        let rows = statement.query_map([], row_to_metadata)?;

        let mut records = HashMap::new();
        for row in rows {
            if let Some(record) = row? {
                records.insert(record.title_key.clone(), record);
            }
        }
        Ok(records)
    }

    /// One title's stored answer.
    pub fn metadata(&self, title_key: &str) -> Result<Option<MetadataRecord>> {
        let record = self
            .conn
            .query_row(
                "SELECT title_key, provider, matched, fetched_at, remote_id, name, original_name,
                        overview, year, kind, poster_url, backdrop_url, rating, genres, episodes,
                        url, manual, details, chain
                 FROM title_metadata WHERE title_key = ?1",
                params![title_key],
                row_to_metadata,
            )
            .optional()?;
        Ok(record.flatten())
    }

    /// Store what a provider said, replacing whatever was there.
    ///
    /// A miss is stored too — see [`MetadataRecord::metadata`]. Storing only
    /// matches would leave every unmatched title asking again on every render.
    ///
    /// Clears the title's enrichment record, so the answer is enriched again;
    /// see [`Catalog::set_enrichment`]. And when the answer is a different
    /// entry from the one stored — or none — the old entry's episode rows go
    /// at once: they are not about this one, and a front end that pins a match
    /// without enriching it must not show them in the meantime.
    pub fn set_metadata(&self, record: &MetadataRecord) -> Result<()> {
        let data = record.metadata.as_ref();
        self.conn.execute(
            "DELETE FROM episode_metadata WHERE title_key = ?1 AND NOT EXISTS (
                 SELECT 1 FROM title_metadata
                 WHERE title_key = ?1 AND provider = ?2 AND remote_id IS ?3
             )",
            params![
                record.title_key,
                record.provider.as_str(),
                data.map(|data| data.remote_id.as_str()),
            ],
        )?;
        let details = data
            .and_then(|data| data.details.as_ref())
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| crate::error::Error::Config(error.to_string()))?;
        // `same` below is whether the row already describes this entry. When
        // it does, what enrichment added — the fanart, the chain, the record
        // that it was asked — still describes it and is kept: refreshing a
        // match's details must not cost the library its episode lists.
        self.conn.execute(
            &"INSERT INTO title_metadata (
                 title_key, provider, matched, fetched_at, remote_id, name, original_name,
                 overview, year, kind, poster_url, backdrop_url, rating, genres, episodes, url,
                 manual, details
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                       ?17, ?18)
             ON CONFLICT (title_key) DO UPDATE SET
                 provider      = excluded.provider,
                 matched       = excluded.matched,
                 fetched_at    = excluded.fetched_at,
                 remote_id     = excluded.remote_id,
                 name          = excluded.name,
                 original_name = excluded.original_name,
                 overview      = excluded.overview,
                 year          = excluded.year,
                 kind          = excluded.kind,
                 poster_url    = excluded.poster_url,
                 backdrop_url  = CASE WHEN same THEN COALESCE(excluded.backdrop_url,
                                                              title_metadata.backdrop_url)
                                      ELSE excluded.backdrop_url END,
                 rating        = excluded.rating,
                 genres        = excluded.genres,
                 episodes      = excluded.episodes,
                 url           = excluded.url,
                 manual        = excluded.manual,
                 details       = excluded.details,
                 chain         = CASE WHEN same THEN title_metadata.chain END,
                 enriched_at   = CASE WHEN same THEN title_metadata.enriched_at END"
                .replace(
                    "same",
                    "(title_metadata.provider = excluded.provider
                      AND title_metadata.remote_id IS excluded.remote_id
                      AND excluded.matched = 1)",
                ),
            params![
                record.title_key,
                record.provider.as_str(),
                data.is_some() as i64,
                record.fetched_at,
                data.map(|data| data.remote_id.as_str()),
                data.map(|data| data.name.as_str()),
                data.and_then(|data| data.original_name.as_deref()),
                data.and_then(|data| data.overview.as_deref()),
                data.and_then(|data| data.year),
                data.map(|data| kind_name(data.kind)),
                data.and_then(|data| data.poster_url.as_deref()),
                data.and_then(|data| data.backdrop_url.as_deref()),
                data.and_then(|data| data.rating),
                data.map(|data| data.genres.join("\n")),
                data.and_then(|data| data.episodes),
                data.and_then(|data| data.url.as_deref()),
                record.manual as i64,
                details,
            ],
        )?;
        Ok(())
    }

    /// Forget one title's answer entirely, episodes included.
    ///
    /// Different from storing a miss: a miss says "asked, nothing there" and is
    /// trusted for a while, where this leaves no row at all and the next match
    /// run treats the title as one it has never seen. That is what "match this
    /// one automatically again" has to mean after a hand-picked match, since a
    /// hand-picked one never expires.
    pub fn forget_metadata(&self, title_key: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM title_metadata WHERE title_key = ?1",
            params![title_key],
        )?;
        self.conn.execute(
            "DELETE FROM episode_metadata WHERE title_key = ?1",
            params![title_key],
        )?;
        Ok(())
    }

    /// Forget every stored answer.
    ///
    /// What "match again" and "turn enrichment off" both do. Deliberately not
    /// called on a recrawl: the answers are about titles, not about shares.
    pub fn clear_metadata(&self) -> Result<()> {
        self.conn.execute("DELETE FROM title_metadata", [])?;
        self.conn.execute("DELETE FROM episode_metadata", [])?;
        Ok(())
    }

    /// Replace everything stored about one title's episodes, and nothing else.
    pub fn set_episode_metadata(
        &mut self,
        title_key: &str,
        provider: ProviderId,
        fetched_at: i64,
        episodes: &[EpisodeMetadata],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        replace_episodes(&tx, title_key, provider, fetched_at, episodes)?;
        tx.commit()?;
        Ok(())
    }

    /// Store what enriching a matched title found, and that it was asked.
    ///
    /// The episode list is replaced even when the new one is empty — a title
    /// re-pinned from a series to its film must not keep the series' episode
    /// names. The backdrop only changes when one was found. And the title is
    /// marked enriched either way, so a film with no episodes is not asked
    /// about again on every run; [`Catalog::set_metadata`] clears the mark when
    /// the match itself changes.
    pub fn set_enrichment(
        &mut self,
        title_key: &str,
        provider: ProviderId,
        fetched_at: i64,
        enrichment: &Enrichment,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        replace_episodes(&tx, title_key, provider, fetched_at, &enrichment.episodes)?;
        tx.execute(
            "UPDATE title_metadata
             SET enriched_at = ?3, backdrop_url = COALESCE(?4, backdrop_url), chain = ?5
             WHERE title_key = ?1 AND provider = ?2",
            params![
                title_key,
                provider.as_str(),
                fetched_at,
                enrichment.backdrop_url.as_deref(),
                enrichment.chain.join("\n"),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Every episode answer there is, keyed by title.
    ///
    /// Read whole, for the same reason watch state and title metadata are: a
    /// season drawn a row at a time must not be a SQLite round-trip per row.
    pub fn all_episode_metadata(&self) -> Result<HashMap<String, EpisodeGuide>> {
        let mut statement = self.conn.prepare(
            "SELECT title_key, season, number, name, overview, still_url, air_date,
                    absolute, entry_season, entry_number
             FROM episode_metadata",
        )?;
        let rows = statement.query_map([], |row| {
            let title_key: String = row.get(0)?;
            let season: i64 = row.get(1)?;
            Ok((
                title_key,
                EpisodeMetadata {
                    season: (season != ABSOLUTE_SEASON).then_some(season as u32),
                    number: row.get::<_, i64>(2)? as u32,
                    absolute: row.get(7)?,
                    entry_season: row.get(8)?,
                    entry_number: row.get(9)?,
                    name: row.get(3)?,
                    overview: row.get(4)?,
                    still_url: row.get(5)?,
                    air_date: row.get(6)?,
                },
            ))
        })?;

        let mut by_title: HashMap<String, Vec<EpisodeMetadata>> = HashMap::new();
        for row in rows {
            let (title_key, episode) = row?;
            by_title.entry(title_key).or_default().push(episode);
        }
        Ok(by_title
            .into_iter()
            .map(|(title_key, episodes)| (title_key, EpisodeGuide::new(episodes)))
            .collect())
    }

    /// When each matched title was last enriched, for the provider it is
    /// matched against. A title missing here has never been, its match has
    /// changed since, or its guide is waiting on the name of an episode that
    /// has aired since — see `GUIDE_WAIT_DAYS`.
    pub fn enrichment_ages(&self) -> Result<HashMap<String, (ProviderId, i64)>> {
        self.enrichment_ages_at(unix_now())
    }

    fn enrichment_ages_at(&self, now: i64) -> Result<HashMap<String, (ProviderId, i64)>> {
        let mut statement = self.conn.prepare(
            "SELECT title_key, provider, enriched_at FROM title_metadata t
             WHERE enriched_at IS NOT NULL
               AND NOT (enriched_at < ?1 - 86400
                        AND EXISTS (
                            SELECT 1 FROM episode_metadata e
                            WHERE e.title_key = t.title_key
                              AND e.name IS NULL
                              AND e.air_date <= date(?1, 'unixepoch')
                              AND e.air_date >= date(t.enriched_at, 'unixepoch',
                                                     '-' || ?2 || ' days')))",
        )?;
        let rows = statement.query_map(params![now, GUIDE_WAIT_DAYS], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;

        let mut ages = HashMap::new();
        for row in rows {
            let (title_key, provider, enriched_at) = row?;
            if let Some(provider) = ProviderId::parse(&provider) {
                ages.insert(title_key, (provider, enriched_at));
            }
        }
        drop(statement);

        // A file that arrived after the guide was fetched, and that the guide
        // has no name for, is an episode the guide predates: the next one of a
        // running show. Asked again once — the refetch is newer than the file,
        // so a name the provider never gives does not make it ask forever.
        if !ages.is_empty() {
            let first_seen = self.first_seen()?;
            let guides = self.all_episode_metadata()?;
            for node in self.all_files()? {
                let Some(number) = node.parsed.episode else {
                    continue;
                };
                let key = crate::library::title_key(&node.parsed.title);
                let Some((_, enriched_at)) = ages.get(&key) else {
                    continue;
                };
                let seen = first_seen
                    .get(&(node.share_id, node.link_id))
                    .copied()
                    .unwrap_or(0);
                let named = guides
                    .get(&key)
                    .and_then(|guide| guide.get(node.parsed.season, number))
                    .is_some_and(|episode| episode.name.is_some());
                if seen > *enriched_at && !named {
                    ages.remove(&key);
                }
            }
        }
        Ok(ages)
    }
}

/// Replace every episode row of one title with `episodes`, inside `tx`.
///
/// A replace rather than an upsert per row: a provider that has *dropped* an
/// episode — a special that was recounted, a season renumbered — should leave
/// nothing behind, and an episode list is small enough that the difference is a
/// few hundred microseconds.
fn replace_episodes(
    tx: &rusqlite::Transaction<'_>,
    title_key: &str,
    provider: ProviderId,
    fetched_at: i64,
    episodes: &[EpisodeMetadata],
) -> Result<()> {
    tx.execute(
        "DELETE FROM episode_metadata WHERE title_key = ?1",
        params![title_key],
    )?;
    let mut insert = tx.prepare(
        "INSERT OR REPLACE INTO episode_metadata (
             title_key, season, number, provider, fetched_at,
             name, overview, still_url, air_date,
             absolute, entry_season, entry_number
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )?;
    for episode in episodes {
        insert.execute(params![
            title_key,
            episode.season.map_or(ABSOLUTE_SEASON, i64::from),
            episode.number,
            provider.as_str(),
            fetched_at,
            episode.name.as_deref(),
            episode.overview.as_deref(),
            episode.still_url.as_deref(),
            episode.air_date.as_deref(),
            episode.absolute,
            episode.entry_season,
            episode.entry_number,
        ])?;
    }
    Ok(())
}

/// The `details` and `chain` columns, read back.
///
/// JSON that will not parse is treated as never fetched rather than failing
/// the read: the next match run fetches it again, where an error here would
/// take the whole grid down over one row.
fn details_column(details: Option<String>, chain: Option<String>) -> Option<TitleDetails> {
    let mut details: TitleDetails = serde_json::from_str(&details?).ok()?;
    details.chain = chain
        .unwrap_or_default()
        .lines()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect();
    Some(details)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// `None` for a row whose provider column this build does not recognise.
fn row_to_metadata(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<MetadataRecord>> {
    let title_key: String = row.get(0)?;
    let Some(provider) = ProviderId::parse(&row.get::<_, String>(1)?) else {
        return Ok(None);
    };
    let matched: i64 = row.get(2)?;
    let fetched_at: i64 = row.get(3)?;

    let metadata = if matched == 0 {
        None
    } else {
        let genres: Option<String> = row.get(13)?;
        Some(TitleMetadata {
            provider,
            remote_id: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
            name: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
            original_name: row.get(6)?,
            overview: row.get(7)?,
            year: row.get(8)?,
            kind: parse_kind(row.get::<_, Option<String>>(9)?.as_deref()),
            poster_url: row.get(10)?,
            backdrop_url: row.get(11)?,
            rating: row.get(12)?,
            genres: genres
                .unwrap_or_default()
                .lines()
                .filter(|genre| !genre.is_empty())
                .map(str::to_string)
                .collect(),
            episodes: row.get(14)?,
            url: row.get(15)?,
            details: details_column(row.get(17)?, row.get(18)?),
        })
    };

    Ok(Some(MetadataRecord {
        title_key,
        provider,
        metadata,
        fetched_at,
        manual: row.get::<_, i64>(16)? != 0,
    }))
}

fn kind_name(kind: TitleKind) -> &'static str {
    match kind {
        TitleKind::Series => "series",
        TitleKind::Film => "film",
    }
}

/// Anything but `series` reads as a film, including a value from a newer build.
/// The kind only chooses a badge; guessing wrong is a cosmetic error, and a
/// hard failure here would take the whole grid down with it.
fn parse_kind(text: Option<&str>) -> TitleKind {
    match text {
        Some("series") => TitleKind::Series,
        _ => TitleKind::Film,
    }
}

fn row_to_node(row: &rusqlite::Row<'_>) -> rusqlite::Result<CatalogNode> {
    Ok(CatalogNode {
        share_id: row.get(0)?,
        link_id: row.get(1)?,
        volume_id: row.get(2)?,
        parent_link_id: row.get(3)?,
        name: row.get(4)?,
        is_folder: row.get::<_, i64>(5)? != 0,
        media_type: row.get(6)?,
        size: row.get(7)?,
        active_revision_id: row.get(8)?,
        parsed: ParsedName {
            title: row.get(9)?,
            season: row.get(10)?,
            episode: row.get(11)?,
            year: row.get(12)?,
            episode_title: row.get(13)?,
        },
    })
}

/// Build a catalog row from a crawled [`Node`](proton_drive_rs::Node), from the
/// filename alone.
///
/// [`build_rows`] is what callers want: a filename in isolation usually does not
/// say what series it belongs to.
pub fn from_node(share_id: &str, node: &proton_drive_rs::Node) -> CatalogNode {
    use proton_drive_rs::NodeKind;

    let (is_folder, media_type, size, active_revision_id) = match &node.kind {
        NodeKind::Folder => (true, None, None, None),
        NodeKind::File {
            media_type,
            claimed_size,
            active_revision_id,
            ..
        } => (
            false,
            Some(media_type.clone()),
            *claimed_size,
            active_revision_id.clone(),
        ),
    };

    CatalogNode {
        share_id: share_id.to_string(),
        link_id: node.uid.link_id.to_string(),
        volume_id: node.uid.volume_id.to_string(),
        parent_link_id: node.parent_uid.as_ref().map(|uid| uid.link_id.to_string()),
        name: node.name.clone(),
        is_folder,
        media_type,
        size,
        active_revision_id,
        parsed: naming::parse(&node.name),
    }
}

/// Turn a whole crawl into catalog rows, reading each file's title from its
/// **folder path** rather than from its name alone.
///
/// This is the difference between a browsable catalog and an unusable one. A
/// real library is laid out as
///
/// ```text
/// Sousou no Frieren/Season 01/S01E01.mkv
/// Neon Genesis Evangelion/Movies/[Anime Time] … - Death & Rebirth-004.mkv
/// Fullmetal Alchemist Brotherhood/OVA/[Reaktor] … OVA - E1 v2 [1080p].mkv
/// ```
///
/// — where `S01E01.mkv` states no title at all, and the two below it state one
/// that is really the *episode's* name. So:
///
/// - **the folder names the series**, because that is how the user organised it;
/// - **the filename names the numbering**, falling back to a `Season NN` folder;
/// - grouping folders (`Movies`, `OVA`, `Specials`) are stepped over, so their
///   contents group under the series above them;
/// - whatever the filename claimed as a title, when it differs, is kept as the
///   episode's own name — with the redundant series prefix stripped.
///
/// Folders and non-video files are dropped: a share holds `.nfo` and `.jpg`
/// alongside the video, and a catalog that offers those as episodes is worse
/// than one that omits an exotic container.
pub fn build_rows(share_id: &str, nodes: &[proton_drive_rs::Node]) -> Vec<CatalogNode> {
    use std::collections::HashMap;

    let by_link: HashMap<&str, &proton_drive_rs::Node> = nodes
        .iter()
        .map(|node| (node.uid.link_id.as_str(), node))
        .collect();

    nodes
        .iter()
        .filter(|node| !node.is_folder() && naming::is_video_file(&node.name))
        .map(|node| {
            let mut row = from_node(share_id, node);
            let context = ancestry(node, &by_link);

            if let Some(title) = context.title {
                // The filename's own title, if it had one and it differs, is the
                // episode's name rather than the series'.
                if row.parsed.episode_title.is_none()
                    && !row.parsed.title.is_empty()
                    && !row.parsed.title.eq_ignore_ascii_case(&title)
                {
                    let rest = strip_prefix_title(&row.parsed.title, &title);
                    // …unless what is left is a season marker. `Oshi no Ko 3rd
                    // Season - 07.mkv` under a folder called `Oshi no Ko` leaves
                    // `3rd Season`, and calling every episode of season three
                    // that is worse than calling none of them anything.
                    if naming::is_season_marker(&rest) {
                        row.parsed.season = row.parsed.season.or(naming::parse(&rest).season);
                    } else if !rest.eq_ignore_ascii_case(&title) {
                        // Nor when the strip found nothing to remove: a filename
                        // whose title only *differs* from the folder's — spacing,
                        // punctuation, a stray article — names the series, not the
                        // episode, and taking it hands every row the same text.
                        row.parsed.episode_title = Some(rest);
                    }
                }
                row.parsed.title = title;
            }
            if row.parsed.season.is_none() {
                row.parsed.season = context.season;
            }
            if row.parsed.year.is_none() {
                row.parsed.year = context.year;
            }
            row
        })
        .collect()
}

/// Pair each subtitle file in a crawl with the video it is for.
///
/// By name, the way every player looks for one, in the three layouts shares
/// actually have:
///
/// ```text
/// Show/Season 01/S01E01.mkv + S01E01.srt, S01E01.en.srt, S01E01.en.forced.ass
/// Show/Season 01/Subs/S01E01.en.srt          a subtitles folder beside the videos
/// Film (2020)/Subs/Film (2020)/2_English.srt a folder per video inside that
/// ```
///
/// A subtitle whose name starts with no video's is still paired when its
/// folder holds one video only — `Film/film.mkv` beside `English.srt`. One
/// that matches nothing is left out: offering it under the wrong episode is
/// worse than not offering it.
pub fn build_sidecars(share_id: &str, nodes: &[proton_drive_rs::Node]) -> Vec<Sidecar> {
    use std::collections::HashMap;

    let by_link: HashMap<&str, &proton_drive_rs::Node> = nodes
        .iter()
        .map(|node| (node.uid.link_id.as_str(), node))
        .collect();
    let parent_of = |node: &proton_drive_rs::Node| {
        node.parent_uid
            .as_ref()
            .and_then(|uid| by_link.get(uid.link_id.as_str()).copied())
    };
    let mut videos: HashMap<&str, Vec<(&proton_drive_rs::Node, &str)>> = HashMap::new();
    for node in nodes {
        if !node.is_folder()
            && naming::is_video_file(&node.name)
            && let Some(parent) = &node.parent_uid
        {
            videos
                .entry(parent.link_id.as_str())
                .or_default()
                .push((node, stem(&node.name)));
        }
    }

    let mut sidecars: Vec<Sidecar> = nodes
        .iter()
        .filter(|node| !node.is_folder() && naming::is_subtitle_file(&node.name))
        .filter_map(|node| {
            let own = stem(&node.name);
            let parent = parent_of(node)?;
            let (video, rest) = if is_subtitles_folder(&parent.name) {
                // `Subs/S01E01.en.srt`, for the videos beside `Subs`.
                beside(
                    videos.get(parent.parent_uid.as_ref()?.link_id.as_str())?,
                    own,
                )?
            } else if let Some(holder) = parent_of(parent)
                && is_subtitles_folder(&holder.name)
            {
                // `Subs/<video>/2_English.srt`: the folder is named for the video.
                let candidates = videos.get(holder.parent_uid.as_ref()?.link_id.as_str())?;
                let (video, _) = candidates
                    .iter()
                    .find(|(_, video)| video.eq_ignore_ascii_case(&parent.name))?;
                (*video, own)
            } else {
                beside(videos.get(node.parent_uid.as_ref()?.link_id.as_str())?, own)?
            };
            let (language, title) = describe_subtitle(rest);
            Some(Sidecar {
                share_id: share_id.to_owned(),
                link_id: node.uid.link_id.to_string(),
                volume_id: node.uid.volume_id.to_string(),
                video_link_id: video.uid.link_id.to_string(),
                name: node.name.clone(),
                language,
                title,
            })
        })
        .collect();
    sidecars.sort_by(|a, b| (&a.video_link_id, &a.name).cmp(&(&b.video_link_id, &b.name)));
    sidecars
}

/// Which of a video's subtitle files to turn on when the video itself has
/// no subtitle playing: the first in the preferred language, or with no
/// preference the first of all. One in no stated language is taken over
/// nothing, since it is most often the only one there is.
///
/// Never one in another language: a viewer who asked for English subtitles
/// and gets German ones has not been helped.
pub fn preferred_sidecar(sidecars: &[Sidecar], language: Option<&str>) -> Option<usize> {
    let Some(wanted) = language else {
        return (!sidecars.is_empty()).then_some(0);
    };
    sidecars
        .iter()
        .position(|sidecar| {
            sidecar
                .language
                .as_deref()
                .is_some_and(|have| crate::language::same_language(wanted, have))
        })
        .or_else(|| {
            sidecars
                .iter()
                .position(|sidecar| sidecar.language.is_none())
        })
}

/// A file name without its extension.
fn stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

fn is_subtitles_folder(name: &str) -> bool {
    ["subs", "sub", "subtitles", "subtitle"]
        .iter()
        .any(|known| known.eq_ignore_ascii_case(name.trim()))
}

/// Which of the videos in one folder a subtitle named `own` is for, and what
/// its name says past the video's.
///
/// The longest video name the subtitle's starts with, followed by a dot or
/// nothing: `S01E1.en` is not `S01E10`'s. Failing that, the only video there.
fn beside<'a, 'b>(
    videos: &[(&'a proton_drive_rs::Node, &str)],
    own: &'b str,
) -> Option<(&'a proton_drive_rs::Node, &'b str)> {
    let lower = own.to_lowercase();
    let named = videos
        .iter()
        .filter_map(|(video, name)| {
            let name = name.to_lowercase();
            if lower == name {
                Some((*video, name.len(), ""))
            } else if lower.starts_with(&format!("{name}.")) {
                // Lowercasing keeps the length of what these names hold;
                // `get` covers the names where it does not.
                Some((*video, name.len(), own.get(name.len() + 1..)?))
            } else {
                None
            }
        })
        .max_by_key(|(_, length, _)| *length)
        .map(|(video, _, rest)| (video, rest));
    named.or(match videos {
        [(video, _)] => Some((*video, own)),
        _ => None,
    })
}

/// The language and the rest of what a subtitle's name says past its video's:
/// `en.forced` is English, "forced"; `2_English_SDH` is English, "SDH".
///
/// The last word naming a language is the language — in `Film.2020.English`
/// the words before it are the film's. Numbers are an index, not a title.
fn describe_subtitle(rest: &str) -> (Option<String>, Option<String>) {
    let words: Vec<&str> = rest
        .split(['.', '_', ' ', '-', '(', ')', '[', ']'])
        .filter(|word| !word.is_empty())
        .collect();
    let found = words
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, word)| crate::language::language_tag(word).map(|tag| (index, tag)));
    let title_words: Vec<&str> = words
        .iter()
        .enumerate()
        .filter(|(index, word)| {
            Some(*index) != found.map(|(at, _)| at) && !word.chars().all(|c| c.is_ascii_digit())
        })
        .map(|(_, word)| *word)
        .collect();
    let title = (!title_words.is_empty()).then(|| title_words.join(" "));
    (found.map(|(_, tag)| tag.to_owned()), title)
}

/// What a file's ancestors say about it.
#[derive(Debug, Default)]
struct Ancestry {
    title: Option<String>,
    season: Option<u32>,
    year: Option<u32>,
}

/// Walk up from `node`, taking the nearest season and the nearest naming folder.
///
/// The share root is skipped: it is the folder that was shared (`anime`), and
/// using it as a title would label every top-level file identically.
fn ancestry(
    node: &proton_drive_rs::Node,
    by_link: &std::collections::HashMap<&str, &proton_drive_rs::Node>,
) -> Ancestry {
    use crate::naming::FolderRole;

    let mut found = Ancestry::default();
    let mut cursor = node.parent_uid.as_ref().map(|uid| uid.link_id.to_string());

    while let Some(link_id) = cursor {
        let Some(ancestor) = by_link.get(link_id.as_str()) else {
            // Off the top of the share: the parent is outside what we can see,
            // so this was the root.
            break;
        };
        // The root itself names the share, not a series.
        if ancestor.parent_uid.is_none()
            || !by_link.contains_key(
                ancestor
                    .parent_uid
                    .as_ref()
                    .map(|uid| uid.link_id.as_str())
                    .unwrap_or_default(),
            )
        {
            break;
        }

        match crate::naming::classify_folder(&ancestor.name) {
            FolderRole::Season(season) => found.season = found.season.or(Some(season)),
            FolderRole::Container => {}
            FolderRole::Title(parsed) => {
                if found.title.is_none() && !parsed.title.is_empty() {
                    found.title = Some(parsed.title.clone());
                    found.year = found.year.or(parsed.year);
                    // `Game of Thrones - Season 1` names both; the season it
                    // states belongs to the files under it.
                    found.season = found.season.or(parsed.season);
                }
            }
        }

        cursor = ancestor
            .parent_uid
            .as_ref()
            .map(|uid| uid.link_id.to_string());
    }

    found
}

/// Drop a redundant series prefix from an episode name.
///
/// `Neon Genesis Evangelion - Death & Rebirth` under a folder called
/// `Neon Genesis Evangelion` should read as `Death & Rebirth`.
///
/// Case-insensitively, because the two names come from different places: the
/// folder is typed by whoever organised the share and the prefix is whatever a
/// release group capitalised. `[Anime Time] Attack On Titan - 38.mkv` under a
/// folder called `Attack on Titan` differs in exactly one letter, and a
/// case-sensitive strip leaves the whole series name behind as the episode's.
fn strip_prefix_title(episode: &str, series: &str) -> String {
    let stripped = episode
        .get(..series.len())
        .filter(|head| head.eq_ignore_ascii_case(series))
        .map(|_| episode[series.len()..].trim_start_matches([' ', '-', '_', '.', ':']))
        .unwrap_or(episode)
        .trim();

    if stripped.is_empty() {
        episode.to_string()
    } else {
        stripped.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(share: &str, link: &str, name: &str) -> CatalogNode {
        CatalogNode {
            share_id: share.into(),
            link_id: link.into(),
            volume_id: "vol".into(),
            parent_link_id: Some("root".into()),
            name: name.into(),
            is_folder: false,
            media_type: Some("video/x-matroska".into()),
            size: Some(1 << 30),
            active_revision_id: Some("rev-1".into()),
            parsed: naming::parse(name),
        }
    }

    fn found() -> TitleMetadata {
        TitleMetadata {
            provider: ProviderId::AniList,
            remote_id: "16498".into(),
            name: "Attack on Titan".into(),
            original_name: Some("進撃の巨人".into()),
            overview: Some("Several hundred years ago…".into()),
            year: Some(2013),
            kind: TitleKind::Series,
            poster_url: Some("https://example.invalid/p.jpg".into()),
            backdrop_url: None,
            rating: Some(8.4),
            genres: vec!["Action".into(), "Drama".into()],
            episodes: Some(25),
            url: Some("https://anilist.co/anime/16498".into()),
            details: None,
        }
    }

    #[test]
    fn metadata_round_trips_through_the_catalog() {
        let catalog = Catalog::in_memory().expect("open");
        let record = MetadataRecord {
            title_key: "attack on titan".into(),
            provider: ProviderId::AniList,
            metadata: Some(found()),
            fetched_at: 1_700_000_000,
            manual: false,
        };

        catalog.set_metadata(&record).expect("store");
        assert_eq!(
            catalog.metadata("attack on titan").expect("read"),
            Some(record.clone())
        );
        assert_eq!(catalog.all_metadata().expect("read all").len(), 1);

        // Second write is an update, not a duplicate row.
        catalog.set_metadata(&record).expect("store again");
        assert_eq!(catalog.all_metadata().expect("read all").len(), 1);
    }

    /// A hand-picked match has to read back as hand-picked, or the next match
    /// run overwrites the one thing the viewer corrected themselves.
    #[test]
    fn a_hand_picked_match_says_so_when_it_is_read_back() {
        let catalog = Catalog::in_memory().expect("open");
        let record = MetadataRecord {
            title_key: "fate stay night heavens feel".into(),
            provider: ProviderId::AniList,
            metadata: Some(found()),
            fetched_at: 1_700_000_000,
            manual: true,
        };
        catalog.set_metadata(&record).expect("store");

        let read = catalog.metadata(&record.title_key).expect("read");
        assert_eq!(read.as_ref().map(|read| read.manual), Some(true));
        assert_eq!(read, Some(record.clone()));

        // And an automatic write over the top says so in turn: the flag is the
        // row's, not the title's.
        catalog
            .set_metadata(&MetadataRecord {
                manual: false,
                ..record.clone()
            })
            .expect("store again");
        assert_eq!(
            catalog
                .metadata(&record.title_key)
                .expect("read")
                .map(|read| read.manual),
            Some(false)
        );
    }

    /// Forgetting is not the same as storing a miss: it must leave no row at
    /// all, so the next match run treats the title as one it never asked about.
    #[test]
    fn forgetting_a_title_leaves_neither_its_match_nor_its_episodes() {
        let mut catalog = Catalog::in_memory().expect("open");
        catalog
            .set_metadata(&MetadataRecord {
                title_key: "attack on titan".into(),
                provider: ProviderId::AniList,
                metadata: Some(found()),
                fetched_at: 1,
                manual: true,
            })
            .expect("store");
        catalog
            .set_episode_metadata(
                "attack on titan",
                ProviderId::AniList,
                1,
                &[EpisodeMetadata {
                    season: None,
                    number: 1,
                    absolute: None,
                    entry_season: None,
                    entry_number: None,
                    name: Some("To You, in 2000 Years".into()),
                    overview: None,
                    still_url: None,
                    air_date: None,
                }],
            )
            .expect("store episodes");

        catalog.forget_metadata("attack on titan").expect("forget");
        assert!(catalog.metadata("attack on titan").expect("read").is_none());
        assert!(catalog.all_episode_metadata().expect("read").is_empty());
    }

    /// The negative answer is the one that has to survive a round trip: without
    /// it every render re-asks about every unmatched title.
    #[test]
    fn a_stored_miss_reads_back_as_a_miss_and_not_as_absence() {
        let catalog = Catalog::in_memory().expect("open");
        let record = MetadataRecord {
            title_key: "something obscure".into(),
            provider: ProviderId::AniList,
            metadata: None,
            fetched_at: 1_700_000_000,
            manual: false,
        };
        catalog.set_metadata(&record).expect("store");

        let read = catalog.metadata("something obscure").expect("read");
        assert_eq!(read, Some(record));
        assert!(
            catalog
                .metadata("never asked about")
                .expect("read")
                .is_none()
        );
    }

    /// A row written by a build that knows a provider this one does not must
    /// read as "unmatched", never as a failure that takes the whole grid down.
    #[test]
    fn a_row_from_an_unknown_provider_is_skipped_rather_than_fatal() {
        let catalog = Catalog::in_memory().expect("open");
        catalog
            .conn
            .execute(
                "INSERT INTO title_metadata (title_key, provider, matched, fetched_at, name)
                 VALUES ('x', 'letterboxd', 1, 1, 'Something')",
                [],
            )
            .expect("insert");

        assert!(catalog.all_metadata().expect("read all").is_empty());
        assert!(catalog.metadata("x").expect("read").is_none());
    }

    /// Metadata is about titles, not shares — a recrawl replaces node rows and
    /// must leave the answers alone, exactly as it does watch state.
    #[test]
    fn replacing_a_share_does_not_drop_stored_metadata() {
        let mut catalog = Catalog::in_memory().expect("open");
        catalog
            .set_metadata(&MetadataRecord {
                title_key: "attack on titan".into(),
                provider: ProviderId::AniList,
                metadata: Some(found()),
                fetched_at: 1,
                manual: false,
            })
            .expect("store");

        catalog
            .replace_share(
                "share-a",
                &[file("share-a", "l1", "Attack on Titan S01E01.mkv")],
            )
            .expect("replace");

        assert_eq!(catalog.all_metadata().expect("read all").len(), 1);
        catalog.clear_metadata().expect("clear");
        assert!(catalog.all_metadata().expect("read all").is_empty());
    }

    #[test]
    fn episode_answers_round_trip_and_replace_rather_than_accumulate() {
        let mut catalog = Catalog::in_memory().expect("open");
        let episodes = vec![
            EpisodeMetadata {
                season: None,
                number: 57,
                absolute: None,
                entry_season: None,
                entry_number: None,
                name: Some("The Immortal Legion".into()),
                overview: Some("Ed and Al…".into()),
                still_url: None,
                air_date: Some("2010-11-14".into()),
            },
            EpisodeMetadata {
                season: Some(1),
                number: 1,
                absolute: None,
                entry_season: None,
                entry_number: None,
                name: Some("Fullmetal Alchemist".into()),
                overview: None,
                still_url: Some("still.jpg".into()),
                air_date: None,
            },
        ];
        catalog
            .set_episode_metadata("fma", ProviderId::AniList, 100, &episodes)
            .expect("store");

        let stored = catalog.all_episode_metadata().expect("read");
        let guide = stored.get("fma").expect("a guide for the title");
        assert_eq!(guide.len(), 2);
        assert_eq!(
            guide.get(None, 57).and_then(|e| e.name.clone()),
            Some("The Immortal Legion".to_string())
        );
        // The absolute/season distinction has to survive the round trip, or an
        // absolutely-numbered file stops matching after a restart.
        assert_eq!(guide.get(None, 57).map(|e| e.season), Some(None));
        assert_eq!(guide.get(Some(1), 1).map(|e| e.season), Some(Some(1)));

        // A shorter list replaces rather than merges: an episode the provider
        // has dropped must not linger.
        catalog
            .set_episode_metadata("fma", ProviderId::AniList, 200, &episodes[..1])
            .expect("store again");
        let stored = catalog.all_episode_metadata().expect("read");
        assert_eq!(stored["fma"].len(), 1);
    }

    /// The three numberings a release might use have to survive a restart, or
    /// an absolutely-numbered season folder stops finding its names.
    #[test]
    fn every_numbering_of_an_episode_round_trips() {
        let mut catalog = Catalog::in_memory().expect("open");
        let episode = EpisodeMetadata {
            season: Some(2),
            number: 11,
            absolute: Some(39),
            entry_season: Some(3),
            entry_number: Some(1),
            name: Some("Lightning's End".into()),
            overview: None,
            still_url: None,
            air_date: None,
        };
        catalog
            .set_episode_metadata(
                "mushishi",
                ProviderId::AniList,
                1,
                std::slice::from_ref(&episode),
            )
            .expect("store");
        let stored = catalog.all_episode_metadata().expect("read");
        assert_eq!(stored["mushishi"].get(Some(2), 11), Some(&episode));
        assert_eq!(stored["mushishi"].get(Some(3), 1), Some(&episode));
    }

    /// **What left Jujutsu Kaisen 0 with the TV series' episode names.** The
    /// title was re-pinned to the film, the film had no episodes, and an empty
    /// list was never written — so the series' list stayed.
    #[test]
    fn an_empty_enrichment_clears_the_episodes_the_last_match_left() {
        let mut catalog = Catalog::in_memory().expect("open");
        let record = MetadataRecord {
            title_key: "jujutsu kaisen 0 movie".into(),
            provider: ProviderId::AniList,
            metadata: Some(found()),
            fetched_at: 1,
            manual: true,
        };
        catalog.set_metadata(&record).expect("store");
        catalog
            .set_episode_metadata(
                &record.title_key,
                ProviderId::AniList,
                1,
                &[EpisodeMetadata {
                    season: Some(1),
                    number: 1,
                    absolute: None,
                    entry_season: None,
                    entry_number: None,
                    name: Some("Ryomen Sukuna".into()),
                    overview: None,
                    still_url: None,
                    air_date: None,
                }],
            )
            .expect("store episodes");

        // Storing the same entry again keeps them; it is the same entry.
        catalog.set_metadata(&record).expect("store again");
        assert_eq!(catalog.all_episode_metadata().expect("read").len(), 1);

        catalog
            .set_enrichment(
                &record.title_key,
                ProviderId::AniList,
                5,
                &Enrichment::default(),
            )
            .expect("enrich");
        assert!(catalog.all_episode_metadata().expect("read").is_empty());
        // Asked, and the answer was nothing: that is recorded, or a film is
        // asked again on every run.
        assert_eq!(
            catalog
                .enrichment_ages()
                .expect("ages")
                .get(&record.title_key),
            Some(&(ProviderId::AniList, 5))
        );
    }

    /// Android pins a match without enriching it; until the next match run,
    /// the title must show no episode names rather than the old entry's.
    #[test]
    fn pinning_a_different_entry_drops_the_old_entrys_episodes_at_once() {
        let mut catalog = Catalog::in_memory().expect("open");
        let series = MetadataRecord {
            title_key: "jujutsu kaisen 0 movie".into(),
            provider: ProviderId::AniList,
            metadata: Some(found()),
            fetched_at: 1,
            manual: false,
        };
        catalog.set_metadata(&series).expect("store");
        catalog
            .set_episode_metadata(
                &series.title_key,
                ProviderId::AniList,
                1,
                &[EpisodeMetadata {
                    season: Some(1),
                    number: 1,
                    absolute: None,
                    entry_season: None,
                    entry_number: None,
                    name: Some("Ryomen Sukuna".into()),
                    overview: None,
                    still_url: None,
                    air_date: None,
                }],
            )
            .expect("store episodes");

        let film = MetadataRecord {
            metadata: Some(TitleMetadata {
                remote_id: "131573".into(),
                ..found()
            }),
            manual: true,
            ..series
        };
        catalog.set_metadata(&film).expect("pin");
        assert!(catalog.all_episode_metadata().expect("read").is_empty());
    }

    /// Enrichment adds a backdrop when it found one and otherwise leaves the
    /// match's own alone; a new match makes the title un-enriched again.
    #[test]
    fn enrichment_keeps_the_backdrop_it_did_not_replace_and_a_different_match_resets_it() {
        let mut catalog = Catalog::in_memory().expect("open");
        let record = MetadataRecord {
            title_key: "attack on titan".into(),
            provider: ProviderId::AniList,
            metadata: Some(TitleMetadata {
                backdrop_url: Some("old.jpg".into()),
                ..found()
            }),
            fetched_at: 1,
            manual: false,
        };
        catalog.set_metadata(&record).expect("store");
        let backdrop = |catalog: &Catalog| {
            catalog
                .metadata("attack on titan")
                .expect("read")
                .and_then(|record| record.metadata)
                .and_then(|found| found.backdrop_url)
        };

        catalog
            .set_enrichment(
                "attack on titan",
                ProviderId::AniList,
                2,
                &Enrichment::default(),
            )
            .expect("enrich");
        assert_eq!(backdrop(&catalog).as_deref(), Some("old.jpg"));

        let enrichment = Enrichment {
            backdrop_url: Some("fanart.jpg".into()),
            ..Enrichment::default()
        };
        catalog
            .set_enrichment("attack on titan", ProviderId::AniList, 3, &enrichment)
            .expect("enrich again");
        assert_eq!(backdrop(&catalog).as_deref(), Some("fanart.jpg"));

        catalog
            .set_metadata(&MetadataRecord {
                metadata: Some(TitleMetadata {
                    remote_id: "99147".into(),
                    ..found()
                }),
                ..record
            })
            .expect("match something else");
        assert!(catalog.enrichment_ages().expect("ages").is_empty());
    }

    #[test]
    fn a_file_newer_than_its_guide_that_the_guide_cannot_name_asks_for_the_guide_again() {
        let mut catalog = Catalog::in_memory().expect("open");
        catalog
            .set_metadata(&MetadataRecord {
                title_key: "kusuriya".into(),
                provider: ProviderId::AniList,
                metadata: Some(found()),
                fetched_at: 1_000,
                manual: false,
            })
            .expect("store");
        let enrichment = Enrichment {
            episodes: vec![EpisodeMetadata {
                season: Some(3),
                number: 1,
                absolute: None,
                entry_season: None,
                entry_number: None,
                name: Some("Locusts".into()),
                overview: None,
                still_url: None,
                air_date: None,
            }],
            backdrop_url: None,
            chain: Vec::new(),
        };
        catalog
            .set_enrichment("kusuriya", ProviderId::AniList, 1_000, &enrichment)
            .expect("enrich");
        let trusted = |catalog: &Catalog| {
            catalog
                .enrichment_ages()
                .expect("ages")
                .contains_key("kusuriya")
        };

        // Newer than the guide, but the guide names it.
        let first = file("s", "e1", "Kusuriya S03E01.mkv");
        catalog
            .replace_share("s", std::slice::from_ref(&first))
            .expect("crawl");
        assert!(trusted(&catalog));

        // The next episode, which the guide does not know yet.
        catalog
            .replace_share("s", &[first, file("s", "e2", "Kusuriya S03E02.mkv")])
            .expect("crawl again");
        assert!(!trusted(&catalog));

        // Fetched again since, still without it: not asked about again.
        catalog
            .set_enrichment("kusuriya", ProviderId::AniList, unix_now() + 1, &enrichment)
            .expect("enrich again");
        assert!(trusted(&catalog));
    }

    #[test]
    fn a_guide_fetched_before_an_episode_had_a_name_is_asked_again_once_it_airs() {
        const DAY: i64 = 86_400;
        // 2026-10-04, when the guide was fetched; episode 2 airs on the 9th.
        let fetched = 1_791_072_000;
        let mut catalog = Catalog::in_memory().expect("open");
        catalog
            .set_metadata(&MetadataRecord {
                title_key: "kusuriya".into(),
                provider: ProviderId::AniList,
                metadata: Some(found()),
                fetched_at: fetched,
                manual: false,
            })
            .expect("store");
        let episode = |number: u32, name: Option<&str>, air_date: &str| EpisodeMetadata {
            season: Some(3),
            number,
            absolute: None,
            entry_season: Some(1),
            entry_number: Some(number),
            name: name.map(str::to_owned),
            overview: None,
            still_url: None,
            air_date: Some(air_date.into()),
        };
        let enrichment = Enrichment {
            episodes: vec![
                episode(1, Some("Locusts"), "2026-10-02"),
                episode(2, None, "2026-10-09"),
            ],
            backdrop_url: None,
            chain: Vec::new(),
        };
        catalog
            .set_enrichment("kusuriya", ProviderId::AniList, fetched, &enrichment)
            .expect("enrich");
        let trusted = |catalog: &Catalog, now: i64| {
            catalog
                .enrichment_ages_at(now)
                .expect("ages")
                .contains_key("kusuriya")
        };

        // Before episode 2 airs, a nameless future episode is no reason.
        assert!(trusted(&catalog, fetched + 4 * DAY));
        // Once it has aired, the guide is asked again.
        assert!(!trusted(&catalog, fetched + 8 * DAY));

        // Asked on the 12th and still nameless: not again the same day…
        let refetched = fetched + 8 * DAY;
        catalog
            .set_enrichment("kusuriya", ProviderId::AniList, refetched, &enrichment)
            .expect("enrich again");
        assert!(trusted(&catalog, refetched + DAY / 2));
        // …but the next.
        assert!(!trusted(&catalog, refetched + DAY + 1));

        // Two weeks past the air date it is given up on.
        let given_up = fetched + 21 * DAY;
        catalog
            .set_enrichment("kusuriya", ProviderId::AniList, given_up, &enrichment)
            .expect("enrich late");
        assert!(trusted(&catalog, given_up + 2 * DAY));
    }

    /// Details round-trip, and refreshing a match's details — the same entry
    /// stored again — keeps what enrichment found for it.
    #[test]
    fn refreshing_the_same_match_keeps_its_enrichment_and_a_new_one_does_not() {
        let mut catalog = Catalog::in_memory().expect("open");
        let record = MetadataRecord {
            title_key: "mushishi".into(),
            provider: ProviderId::AniList,
            metadata: Some(found()),
            fetched_at: 1,
            manual: true,
        };
        catalog.set_metadata(&record).expect("store");
        let enrichment = Enrichment {
            episodes: vec![EpisodeMetadata {
                season: Some(1),
                number: 1,
                absolute: Some(1),
                entry_season: Some(1),
                entry_number: Some(1),
                name: Some("The Green Seat".into()),
                overview: None,
                still_url: None,
                air_date: None,
            }],
            backdrop_url: Some("fanart.jpg".into()),
            chain: vec!["457".into(), "20595".into()],
        };
        catalog
            .set_enrichment("mushishi", ProviderId::AniList, 2, &enrichment)
            .expect("enrich");

        let refreshed = MetadataRecord {
            metadata: Some(TitleMetadata {
                details: Some(TitleDetails {
                    studios: vec!["Artland".into()],
                    ..TitleDetails::default()
                }),
                ..found()
            }),
            fetched_at: 3,
            ..record.clone()
        };
        catalog.set_metadata(&refreshed).expect("refresh");
        let read = catalog.metadata("mushishi").expect("read").expect("a row");
        let data = read.metadata.expect("matched");
        assert_eq!(data.backdrop_url.as_deref(), Some("fanart.jpg"));
        let details = data.details.expect("details");
        assert_eq!(details.studios, vec!["Artland"]);
        assert_eq!(details.chain, vec!["457", "20595"]);
        assert!(
            catalog
                .enrichment_ages()
                .expect("ages")
                .contains_key("mushishi")
        );
        assert_eq!(
            catalog.all_episode_metadata().expect("read")["mushishi"].len(),
            1
        );

        // A different entry is a different answer: nothing carries over.
        catalog
            .set_metadata(&MetadataRecord {
                metadata: Some(TitleMetadata {
                    remote_id: "20526".into(),
                    ..found()
                }),
                ..record
            })
            .expect("repin");
        let read = catalog.metadata("mushishi").expect("read").expect("a row");
        assert_eq!(read.metadata.and_then(|data| data.backdrop_url), None);
        assert!(catalog.enrichment_ages().expect("ages").is_empty());
    }

    #[test]
    fn a_crawl_records_when_it_finished_and_what_it_added() {
        let mut catalog = Catalog::in_memory().expect("open");
        catalog
            .replace_share("s", &[file("s", "a", "Show S01E01.mkv")])
            .expect("crawl");
        catalog
            .replace_share(
                "s",
                &[
                    file("s", "a", "Show S01E01.mkv"),
                    file("s", "b", "Show S01E02.mkv"),
                ],
            )
            .expect("recrawl");
        let record = &catalog.crawls().expect("read")["s"];
        assert!(record.crawled_at > 0);
        assert_eq!(record.attempted_at, record.crawled_at);
        assert_eq!((record.files, record.added), (2, 1));
        assert_eq!(record.error, None);
    }

    /// A failure says why without forgetting what the last good crawl found,
    /// and the next good crawl clears it.
    #[test]
    fn a_failed_crawl_keeps_the_last_good_one_until_the_next() {
        let mut catalog = Catalog::in_memory().expect("open");
        catalog
            .replace_share("s", &[file("s", "a", "Show S01E01.mkv")])
            .expect("crawl");
        catalog
            .conn
            .execute(
                "UPDATE share_crawls SET crawled_at = 5, attempted_at = 5",
                [],
            )
            .expect("age it");
        catalog
            .record_crawl_failure("s", "link expired")
            .expect("fail");
        let record = catalog.crawls().expect("read")["s"].clone();
        assert_eq!((record.crawled_at, record.files), (5, 1));
        assert!(record.attempted_at > 5);
        assert_eq!(record.error.as_deref(), Some("link expired"));
        assert!(!record.is_stale(record.attempted_at + 10, 60));

        catalog
            .replace_share("s", &[file("s", "a", "Show S01E01.mkv")])
            .expect("recrawl");
        assert_eq!(catalog.crawls().expect("read")["s"].error, None);
    }

    #[test]
    fn a_removed_share_forgets_its_crawls() {
        let mut catalog = Catalog::in_memory().expect("open");
        catalog
            .replace_share("s", &[file("s", "a", "Show S01E01.mkv")])
            .expect("crawl");
        catalog.remove_share("s").expect("remove");
        assert!(catalog.crawls().expect("read").is_empty());
    }

    #[test]
    fn migration_v11_opens_a_v10_catalog() {
        let conn = Connection::open_in_memory().expect("open");
        for migration in [
            MIGRATION_V1,
            MIGRATION_V2,
            MIGRATION_V3,
            MIGRATION_V4,
            MIGRATION_V5,
            MIGRATION_V6,
            MIGRATION_V7,
            MIGRATION_V8,
            MIGRATION_V9,
            MIGRATION_V10,
        ] {
            conn.execute_batch(migration).expect("migrate");
        }
        conn.pragma_update(None, "user_version", 10)
            .expect("set version");
        let catalog = Catalog::from_connection(conn).expect("upgrade");
        assert!(catalog.crawls().expect("read").is_empty());
    }

    #[test]
    fn migration_v12_opens_a_v11_catalog() {
        let conn = Connection::open_in_memory().expect("open");
        for migration in [
            MIGRATION_V1,
            MIGRATION_V2,
            MIGRATION_V3,
            MIGRATION_V4,
            MIGRATION_V5,
            MIGRATION_V6,
            MIGRATION_V7,
            MIGRATION_V8,
            MIGRATION_V9,
            MIGRATION_V10,
            MIGRATION_V11,
        ] {
            conn.execute_batch(migration).expect("migrate");
        }
        conn.pragma_update(None, "user_version", 11)
            .expect("set version");
        let catalog = Catalog::from_connection(conn).expect("upgrade");
        assert!(catalog.sidecars("s", "a").expect("read").is_empty());
    }

    /// A file keeps the moment it first appeared across recrawls, and one
    /// that was there before this was recorded reads as 0.
    #[test]
    fn a_file_remembers_when_it_first_appeared() {
        let mut catalog = Catalog::in_memory().expect("open");
        catalog
            .replace_share("s", &[file("s", "a", "Show S01E01.mkv")])
            .expect("crawl");
        let first = catalog.first_seen().expect("read")[&("s".to_string(), "a".to_string())];
        assert!(first > 0);
        catalog
            .conn
            .execute("UPDATE node_first_seen SET seen_at = 5", [])
            .expect("age it");
        catalog
            .replace_share(
                "s",
                &[
                    file("s", "a", "Show S01E01.mkv"),
                    file("s", "b", "Show S01E02.mkv"),
                ],
            )
            .expect("recrawl");
        let seen = catalog.first_seen().expect("read");
        assert_eq!(seen[&("s".to_string(), "a".to_string())], 5);
        assert!(seen[&("s".to_string(), "b".to_string())] > 5);
    }

    /// Schema v9 repairs what AniList's first episode source wrote: its
    /// episode rows and its banners go, and the matches — hand-picked ones
    /// included — stay.
    #[test]
    fn upgrading_drops_anilists_episode_rows_and_banners_but_keeps_its_matches() {
        let conn = Connection::open_in_memory().expect("open");
        for migration in [
            MIGRATION_V1,
            MIGRATION_V2,
            MIGRATION_V3,
            MIGRATION_V4,
            MIGRATION_V5,
            MIGRATION_V6,
            MIGRATION_V7,
            MIGRATION_V8,
        ] {
            conn.execute_batch(migration).expect("migrate");
        }
        conn.pragma_update(None, "user_version", 8)
            .expect("set version");
        conn.execute_batch(
            "INSERT INTO title_metadata (title_key, provider, matched, fetched_at, remote_id,
                                         name, kind, backdrop_url, manual)
             VALUES ('aot', 'anilist', 1, 1, '16498', 'Attack on Titan', 'series',
                     'banner.jpg', 1),
                    ('got', 'tmdb', 1, 1, '1399', 'Game of Thrones', 'series',
                     'backdrop.jpg', 0);
             INSERT INTO episode_metadata (title_key, season, number, provider, fetched_at, name)
             VALUES ('aot', 2, 1, 'anilist', 1, 'Season one''s first, wrongly'),
                    ('got', 1, 1, 'tmdb', 1, 'Winter Is Coming');",
        )
        .expect("seed");

        let catalog = Catalog::from_connection(conn).expect("upgrade");
        let records = catalog.all_metadata().expect("read");
        let aot = records["aot"].metadata.as_ref().expect("still matched");
        assert!(records["aot"].manual);
        assert_eq!(aot.backdrop_url, None);
        assert_eq!(
            records["got"]
                .metadata
                .as_ref()
                .and_then(|got| got.backdrop_url.as_deref()),
            Some("backdrop.jpg")
        );

        let episodes = catalog.all_episode_metadata().expect("read");
        assert!(!episodes.contains_key("aot"));
        assert_eq!(episodes["got"].len(), 1);
        assert!(catalog.enrichment_ages().expect("ages").is_empty());
    }

    #[test]
    fn clearing_metadata_takes_the_episode_answers_with_it() {
        let mut catalog = Catalog::in_memory().expect("open");
        catalog
            .set_episode_metadata(
                "fma",
                ProviderId::AniList,
                100,
                &[EpisodeMetadata {
                    season: None,
                    number: 1,
                    absolute: None,
                    entry_season: None,
                    entry_number: None,
                    name: Some("Fullmetal Alchemist".into()),
                    overview: None,
                    still_url: None,
                    air_date: None,
                }],
            )
            .expect("store");
        catalog.clear_metadata().expect("clear");
        assert!(catalog.all_episode_metadata().expect("read").is_empty());
    }

    #[test]
    fn a_fresh_catalog_is_at_the_current_schema_version() {
        let catalog = Catalog::in_memory().expect("open");
        let version: i64 = catalog
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("read version");
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn a_titles_track_choice_round_trips_with_the_track_names() {
        let catalog = Catalog::in_memory().expect("open");
        assert_eq!(catalog.title_track_prefs("show").expect("read"), None);

        let prefs = TitleTrackPrefs {
            audio_language: Some("jpn".into()),
            subtitle_language: Some("eng".into()),
            subtitles: true,
            audio_title: None,
            subtitle_title: Some("Full Subtitles".into()),
        };
        catalog
            .set_title_track_prefs("show", &prefs)
            .expect("write");
        assert_eq!(
            catalog.title_track_prefs("show").expect("read"),
            Some(prefs.clone())
        );

        let changed = TitleTrackPrefs {
            subtitle_title: Some("Signs & Songs".into()),
            ..prefs
        };
        catalog
            .set_title_track_prefs("show", &changed)
            .expect("overwrite");
        let all = catalog.all_title_track_prefs().expect("read all");
        assert_eq!(all.get("show"), Some(&changed));
    }

    /// A newer schema is refused, not downgraded — reinterpreting it would
    /// silently mangle whatever the newer build recorded.
    #[test]
    fn a_newer_schema_refuses_to_open() {
        let conn = Connection::open_in_memory().expect("open");
        conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .expect("set version");

        let error = match Catalog::from_connection(conn) {
            Err(error) => error,
            Ok(_) => panic!("a newer schema must refuse to open"),
        };
        assert!(
            error.to_string().contains("newer than this build supports"),
            "says why: {error}"
        );
    }

    #[test]
    fn nodes_round_trip_through_a_share_replace() {
        let mut catalog = Catalog::in_memory().expect("open");
        let nodes = vec![
            file("s1", "a", "Show.Name.S01E01.1080p.mkv"),
            file("s1", "b", "Show.Name.S01E02.1080p.mkv"),
        ];
        catalog.replace_share("s1", &nodes).expect("replace");

        let read = catalog.files("s1").expect("read");
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].parsed.title, "Show Name");
        assert_eq!(read[0].parsed.episode, Some(1));
        assert_eq!(read[1].parsed.episode, Some(2));
    }

    /// A recrawl replaces the share's rows wholesale, so files deleted upstream
    /// disappear rather than lingering as unplayable ghosts.
    #[test]
    fn replacing_a_share_drops_rows_that_are_no_longer_there() {
        let mut catalog = Catalog::in_memory().expect("open");
        catalog
            .replace_share(
                "s1",
                &[
                    file("s1", "a", "One.S01E01.mkv"),
                    file("s1", "b", "One.S01E02.mkv"),
                ],
            )
            .expect("first crawl");
        catalog
            .replace_share("s1", &[file("s1", "a", "One.S01E01.mkv")])
            .expect("second crawl");

        let read = catalog.files("s1").expect("read");
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].link_id, "a");
    }

    /// One share's crawl must not touch another's rows.
    #[test]
    fn replacing_one_share_leaves_the_others_alone() {
        let mut catalog = Catalog::in_memory().expect("open");
        catalog
            .replace_share("s1", &[file("s1", "a", "One.S01E01.mkv")])
            .expect("s1");
        catalog
            .replace_share("s2", &[file("s2", "c", "Two.S01E01.mkv")])
            .expect("s2");
        catalog.replace_share("s1", &[]).expect("s1 emptied");

        assert!(catalog.files("s1").expect("read s1").is_empty());
        assert_eq!(catalog.files("s2").expect("read s2").len(), 1);
    }

    /// **The one thing here that is not rebuildable.** A recrawl must not lose
    /// where you were in an episode.
    #[test]
    fn a_recrawl_preserves_watch_state() {
        let mut catalog = Catalog::in_memory().expect("open");
        catalog
            .replace_share("s1", &[file("s1", "a", "One.S01E01.mkv")])
            .expect("crawl");

        let state = WatchState {
            position_secs: 942.5,
            duration_secs: Some(1440.0),
            watched: false,
            updated_at: 1_700_000_000,
        };
        catalog.set_watch_state("s1", "a", &state).expect("set");

        catalog
            .replace_share("s1", &[file("s1", "a", "One.S01E01.mkv")])
            .expect("recrawl");

        assert_eq!(
            catalog.watch_state("s1", "a").expect("read"),
            Some(state),
            "the recrawl must not have cleared the resume position"
        );
    }

    /// Resuming overwrites rather than accumulating rows.
    #[test]
    fn watch_state_updates_in_place() {
        let catalog = Catalog::in_memory().expect("open");
        for position in [10.0, 200.0, 3000.0] {
            catalog
                .set_watch_state(
                    "s1",
                    "a",
                    &WatchState {
                        position_secs: position,
                        duration_secs: Some(3600.0),
                        watched: false,
                        updated_at: 1,
                    },
                )
                .expect("set");
        }
        let state = catalog.watch_state("s1", "a").expect("read").expect("some");
        assert_eq!(state.position_secs, 3000.0);
    }

    // ---- build_rows: titles from the folder path -------------------------

    use proton_drive_rs::{Node, NodeKind};
    use proton_sdk::ids::NodeUid;

    fn uid(link: &str) -> NodeUid {
        NodeUid::new("vol".into(), link.into())
    }

    fn folder(link: &str, parent: Option<&str>, name: &str) -> Node {
        Node {
            uid: uid(link),
            parent_uid: parent.map(uid),
            kind: NodeKind::Folder,
            name: name.into(),
            creation_time: 0,
            modification_time: 0,
            trashed: false,
            is_shared: true,
            is_shared_publicly: true,
            signature_email: None,
            membership: None,
            photo: None,
            album: None,
            verification: Default::default(),
            direct_role: None,
            share_id: None,
        }
    }

    fn video(link: &str, parent: &str, name: &str) -> Node {
        Node {
            kind: NodeKind::File {
                media_type: "video/x-matroska".into(),
                total_size_on_storage: 0,
                active_revision_state: None,
                active_revision_id: Some("rev".into()),
                claimed_size: Some(1 << 30),
                claimed_modification_time: None,
                content_sha1: None,
            },
            ..folder(link, Some(parent), name)
        }
    }

    /// The share root, whose parent is outside what a visitor can see.
    fn root() -> Node {
        folder("root", Some("outside-the-share"), "anime")
    }

    fn row<'a>(rows: &'a [CatalogNode], link: &str) -> &'a CatalogNode {
        rows.iter()
            .find(|row| row.link_id == link)
            .unwrap_or_else(|| panic!("no row for {link}"))
    }

    /// **The layout nearly every real library uses.** `S01E01.mkv` names no
    /// series at all; without walking the folder path the whole catalog would be
    /// untitled.
    #[test]
    fn a_files_title_comes_from_its_series_folder() {
        let nodes = vec![
            root(),
            folder("frieren", Some("root"), "Sousou no Frieren"),
            folder("s1", Some("frieren"), "Season 01"),
            video("ep1", "s1", "S01E01.mkv"),
        ];
        let rows = build_rows("share", &nodes);

        assert_eq!(rows.len(), 1, "folders are not catalog entries");
        let episode = row(&rows, "ep1");
        assert_eq!(episode.parsed.title, "Sousou no Frieren");
        assert_eq!(episode.parsed.season, Some(1));
        assert_eq!(episode.parsed.episode, Some(1));
    }

    /// A season folder supplies the season when the filename states only an
    /// episode marker.
    #[test]
    fn a_season_folder_supplies_the_season() {
        let nodes = vec![
            root(),
            folder("kaijuu", Some("root"), "Kaijuu 8-gou"),
            folder("s2", Some("kaijuu"), "Season 02"),
            video("e4", "s2", "E04.mkv"),
        ];
        let episode = build_rows("share", &nodes);
        let episode = row(&episode, "e4");
        assert_eq!(episode.parsed.title, "Kaijuu 8-gou");
        assert_eq!(episode.parsed.season, Some(2), "from the folder");
        assert_eq!(episode.parsed.episode, Some(4), "from the filename");
    }

    /// A grouping folder is stepped over, so its contents group under the series
    /// above it rather than under a folder called `Movies`.
    #[test]
    fn a_grouping_folder_is_stepped_over_to_reach_the_series() {
        let nodes = vec![
            root(),
            folder("eva", Some("root"), "Neon Genesis Evangelion"),
            folder("movies", Some("eva"), "Movies"),
            video(
                "film",
                "movies",
                "[Anime Time] Neon Genesis Evangelion - The End of Evangelion-003.mkv",
            ),
        ];
        let film = build_rows("share", &nodes);
        let film = row(&film, "film");
        assert_eq!(film.parsed.title, "Neon Genesis Evangelion");
        assert_eq!(
            film.parsed.episode_title.as_deref(),
            Some("The End of Evangelion"),
            "the filename's own title is the film's name, minus the series prefix"
        );
    }

    /// **What every episode of Oshi no Ko season three was called.** The
    /// filename states the season in words, the folder states the series, and
    /// the difference between the two is a season marker — not the name of the
    /// episode.
    #[test]
    fn a_season_stated_in_the_filename_does_not_become_the_episodes_name() {
        let nodes = vec![
            root(),
            folder("onk", Some("root"), "Oshi no Ko"),
            video(
                "e7",
                "onk",
                "[DB]Oshi no Ko 3rd Season_-_07_(Dual Audio_10bit_1080p_x265).mkv",
            ),
        ];
        let rows = build_rows("share", &nodes);
        let episode = row(&rows, "e7");

        assert_eq!(episode.parsed.title, "Oshi no Ko");
        assert_eq!(episode.parsed.season, Some(3));
        assert_eq!(episode.parsed.episode, Some(7));
        assert_eq!(episode.parsed.episode_title, None);
    }

    /// **What every episode of Attack on Titan was called.** The folder and the
    /// release group capitalised the same series two ways, and a case-sensitive
    /// prefix strip left the whole series name behind — so all fifty-nine rows
    /// read `Attack On Titan`, naming nothing and telling the viewer apart from
    /// nothing.
    #[test]
    fn a_series_name_cased_differently_is_not_the_episodes_name() {
        let nodes = vec![
            root(),
            folder("aot", Some("root"), "Attack on Titan"),
            video("e38", "aot", "[Anime Time] Attack On Titan - 38.mkv"),
        ];
        let rows = build_rows("share", &nodes);
        let episode = row(&rows, "e38");

        assert_eq!(episode.parsed.title, "Attack on Titan", "the folder's");
        assert_eq!(episode.parsed.episode, Some(38));
        assert_eq!(episode.parsed.episode_title, None);
    }

    /// The prefix strip is case-insensitive for the same reason: what follows
    /// the series name is the episode's, however the group capitalised it.
    #[test]
    fn a_prefix_cased_differently_is_still_stripped() {
        let nodes = vec![
            root(),
            folder("eva", Some("root"), "Neon Genesis Evangelion"),
            video(
                "film",
                "eva",
                "NEON GENESIS EVANGELION - Death & Rebirth.mkv",
            ),
        ];
        let rows = build_rows("share", &nodes);

        assert_eq!(
            row(&rows, "film").parsed.episode_title.as_deref(),
            Some("Death & Rebirth"),
        );
    }

    /// The deepest naming folder wins, so films inside a collection folder are
    /// titled individually rather than by the collection.
    #[test]
    fn the_deepest_naming_folder_wins() {
        let nodes = vec![
            root(),
            folder("kon", Some("root"), "Satoshi Kon Movies"),
            folder("millennium", Some("kon"), "Millennium Actress"),
            video("film", "millennium", "Millennium.Actress.1080p.mkv"),
        ];
        let film = build_rows("share", &nodes);
        assert_eq!(row(&film, "film").parsed.title, "Millennium Actress");
    }

    /// The share root names the *share*, not a series — using it would label
    /// every top-level file identically.
    #[test]
    fn the_share_root_is_not_used_as_a_title() {
        let nodes = vec![root(), video("loose", "root", "Some.Film.2019.mkv")];
        let film = build_rows("share", &nodes);
        assert_eq!(
            row(&film, "loose").parsed.title,
            "Some Film",
            "the filename's title stands, rather than the root folder's name"
        );
    }

    /// A folder year reaches the files under it.
    #[test]
    fn a_year_in_the_folder_name_reaches_its_files() {
        let nodes = vec![
            root(),
            folder("gits", Some("root"), "Ghost in the Shell (1995)"),
            video("film", "gits", "gits.bd.1080p.mkv"),
        ];
        let film = build_rows("share", &nodes);
        let film = row(&film, "film");
        assert_eq!(film.parsed.title, "Ghost in the Shell");
        assert_eq!(film.parsed.year, Some(1995));
    }

    /// Non-video files are dropped: a share holds `.nfo` and `.jpg` next to the
    /// video, and offering those as episodes is worse than omitting them.
    #[test]
    fn non_video_files_do_not_enter_the_catalog() {
        let nodes = vec![
            root(),
            folder("gits", Some("root"), "Ghost in the Shell (1995)"),
            video("film", "gits", "gits.mkv"),
            video("info", "gits", "torrent.info.nfo"),
            video("art", "gits", "poster.jpg"),
        ];
        let rows = build_rows("share", &nodes);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].link_id, "film");
    }

    #[test]
    fn removing_a_share_keeps_its_watch_state_out_of_sight() {
        let mut catalog = Catalog::in_memory().expect("open");
        catalog
            .replace_share("s1", &[file("s1", "a", "Show S01E01.mkv")])
            .expect("crawl");
        let state = WatchState {
            position_secs: 120.0,
            duration_secs: Some(1440.0),
            watched: false,
            updated_at: 10,
        };
        catalog.set_watch_state("s1", "a", &state).expect("set");

        catalog.remove_share("s1").expect("remove");
        let library = crate::library::Library::build(
            catalog.all_files().expect("files"),
            &catalog.all_watch_states().expect("watch"),
        );
        assert!(library.titles.is_empty());
        assert_eq!(catalog.watch_state("s1", "a").expect("read"), Some(state));

        catalog
            .replace_share("s1", &[file("s1", "a", "Show S01E01.mkv")])
            .expect("re-add");
        let library = crate::library::Library::build(
            catalog.all_files().expect("files"),
            &catalog.all_watch_states().expect("watch"),
        );
        assert_eq!(library.titles[0].seasons[0].episodes[0].watch, Some(state));
    }

    #[test]
    fn removing_a_share_hands_its_watch_state_to_another_share_holding_the_same_file() {
        let mut catalog = Catalog::in_memory().expect("open");
        let at = |position_secs: f64, updated_at: i64| WatchState {
            position_secs,
            duration_secs: Some(1440.0),
            watched: false,
            updated_at,
        };
        catalog
            .replace_share(
                "link",
                &[
                    file("link", "a", "Show S01E01.mkv"),
                    file("link", "b", "Show S01E02.mkv"),
                    file("link", "gone", "Show S01E03.mkv"),
                ],
            )
            .expect("crawl the link");
        catalog
            .replace_share(
                "folder",
                &[
                    file("folder", "a", "Show S01E01.mkv"),
                    file("folder", "b", "Show S01E02.mkv"),
                ],
            )
            .expect("crawl the folder");
        catalog
            .set_watch_state("link", "a", &at(100.0, 10))
            .unwrap();
        catalog
            .set_watch_state("link", "b", &at(100.0, 10))
            .unwrap();
        catalog
            .set_watch_state("link", "gone", &at(100.0, 10))
            .unwrap();
        // Watched through the folder since: the link's older position must
        // not overwrite it.
        catalog
            .set_watch_state("folder", "b", &at(900.0, 20))
            .unwrap();

        catalog.remove_share("link").expect("remove");

        assert_eq!(
            catalog.watch_state("folder", "a").unwrap(),
            Some(at(100.0, 10))
        );
        assert_eq!(
            catalog.watch_state("folder", "b").unwrap(),
            Some(at(900.0, 20))
        );
        assert_eq!(catalog.watch_state("folder", "gone").unwrap(), None);
        assert_eq!(
            catalog.watch_state("link", "a").unwrap(),
            Some(at(100.0, 10))
        );
    }

    #[test]
    fn merging_takes_only_states_newer_than_the_recorded_ones() {
        let mut catalog = Catalog::in_memory().expect("open");
        let at = |position_secs: f64, updated_at: i64| WatchState {
            position_secs,
            duration_secs: Some(1440.0),
            watched: false,
            updated_at,
        };
        catalog
            .set_watch_state("s1", "kept", &at(100.0, 50))
            .unwrap();
        catalog
            .set_watch_state("s1", "taken", &at(100.0, 50))
            .unwrap();
        let taken = catalog
            .merge_watch_states(&[
                ("s1".to_owned(), "kept".to_owned(), at(900.0, 40)),
                ("s1".to_owned(), "taken".to_owned(), at(900.0, 60)),
                ("s1".to_owned(), "new".to_owned(), at(5.0, 10)),
            ])
            .expect("merge");
        assert_eq!(taken, 2);
        assert_eq!(
            catalog.watch_state("s1", "kept").unwrap(),
            Some(at(100.0, 50))
        );
        assert_eq!(
            catalog.watch_state("s1", "taken").unwrap(),
            Some(at(900.0, 60))
        );
        assert_eq!(catalog.watch_state("s1", "new").unwrap(), Some(at(5.0, 10)));
    }

    #[test]
    fn a_merge_tie_settles_the_same_way_whichever_side_holds_it() {
        let state = |position_secs: f64, watched: bool| WatchState {
            position_secs,
            duration_secs: Some(1440.0),
            watched,
            updated_at: 70,
        };
        for (local, remote) in [
            (state(300.0, false), state(1440.0, true)),
            (state(1440.0, true), state(300.0, false)),
            (state(300.0, false), state(600.0, false)),
            (state(600.0, false), state(300.0, false)),
        ] {
            let mut catalog = Catalog::in_memory().expect("open");
            catalog.set_watch_state("s1", "a", &local).unwrap();
            catalog
                .merge_watch_states(&[("s1".to_owned(), "a".to_owned(), remote)])
                .unwrap();
            let winner =
                if (local.watched, local.position_secs) > (remote.watched, remote.position_secs) {
                    local
                } else {
                    remote
                };
            assert_eq!(catalog.watch_state("s1", "a").unwrap(), Some(winner));
        }
    }

    #[test]
    fn an_unwatched_file_has_no_watch_state() {
        let catalog = Catalog::in_memory().expect("open");
        assert_eq!(
            catalog.watch_state("s1", "never-played").expect("read"),
            None
        );
    }

    #[test]
    fn removing_an_offline_record_keeps_the_online_catalog_and_other_downloads() {
        let catalog = Catalog::in_memory().expect("open");
        let first = OfflineFile {
            revision_id: "revision-1".to_owned(),
            block_sizes: vec![3, 5],
        };
        let second = OfflineFile {
            revision_id: "revision-2".to_owned(),
            block_sizes: vec![7],
        };
        catalog
            .set_offline_file("share", "first", &first)
            .expect("record first");
        catalog
            .set_offline_file("share", "second", &second)
            .expect("record second");

        catalog
            .remove_offline_file("share", "first")
            .expect("remove first");

        assert_eq!(
            catalog.offline_file("share", "first").expect("read first"),
            None
        );
        assert_eq!(
            catalog
                .offline_file("share", "second")
                .expect("read second"),
            Some(second)
        );
    }

    #[test]
    fn recrawl_retains_stale_offline_index_until_bytes_can_be_removed() {
        let mut catalog = Catalog::in_memory().expect("open");
        let mut old = file("share", "episode", "Show.S01E01.mkv");
        old.active_revision_id = Some("old".to_owned());
        catalog.replace_share("share", &[old]).expect("first crawl");
        let offline = OfflineFile {
            revision_id: "old".to_owned(),
            block_sizes: vec![4, 7],
        };
        catalog
            .set_offline_file("share", "episode", &offline)
            .expect("record offline revision");

        let mut new = file("share", "episode", "Show.S01E01.mkv");
        new.active_revision_id = Some("new".to_owned());
        catalog
            .replace_share_retaining_offline("share", &[new])
            .expect("recrawl");

        assert_eq!(
            catalog
                .offline_file("share", "episode")
                .expect("read retained path key"),
            Some(offline),
            "filesystem owner must delete old bytes before dropping this path key"
        );
    }

    #[test]
    fn ordinary_recrawl_prunes_stale_offline_index_for_legacy_callers() {
        let mut catalog = Catalog::in_memory().expect("open");
        let mut old = file("share", "episode", "Show.S01E01.mkv");
        old.active_revision_id = Some("old".to_owned());
        catalog.replace_share("share", &[old]).expect("first crawl");
        catalog
            .set_offline_file(
                "share",
                "episode",
                &OfflineFile {
                    revision_id: "old".to_owned(),
                    block_sizes: vec![4],
                },
            )
            .expect("offline row");
        let mut new = file("share", "episode", "Show.S01E01.mkv");
        new.active_revision_id = Some("new".to_owned());
        catalog.replace_share("share", &[new]).expect("recrawl");
        assert_eq!(
            catalog
                .offline_file("share", "episode")
                .expect("read offline row"),
            None
        );
    }

    // ---- build_sidecars: subtitles paired by name --------------------------

    fn paired(sidecars: &[Sidecar]) -> Vec<(&str, &str, Option<&str>, Option<&str>)> {
        sidecars
            .iter()
            .map(|sidecar| {
                (
                    sidecar.name.as_str(),
                    sidecar.video_link_id.as_str(),
                    sidecar.language.as_deref(),
                    sidecar.title.as_deref(),
                )
            })
            .collect()
    }

    #[test]
    fn a_subtitle_named_after_a_video_is_paired_with_it_and_says_its_language() {
        let nodes = vec![
            root(),
            folder("s1", Some("root"), "Season 01"),
            video("ep1", "s1", "S01E1.mkv"),
            video("ep10", "s1", "S01E10.mkv"),
            video("plain", "s1", "S01E1.srt"),
            video("en", "s1", "S01E10.en.srt"),
            video("forced", "s1", "S01E10.eng.forced.ass"),
            video("nfo", "s1", "S01E10.nfo"),
        ];
        assert_eq!(
            paired(&build_sidecars("share", &nodes)),
            vec![
                ("S01E1.srt", "ep1", None, None),
                ("S01E10.en.srt", "ep10", Some("eng"), None),
                ("S01E10.eng.forced.ass", "ep10", Some("eng"), Some("forced")),
            ]
        );
    }

    #[test]
    fn a_subtitles_folder_beside_the_videos_holds_theirs() {
        let nodes = vec![
            root(),
            folder("film", Some("root"), "Film (2020)"),
            video("video", "film", "Film (2020).mkv"),
            folder("subs", Some("film"), "Subs"),
            video("de", "subs", "Film (2020).German.srt"),
            folder("per-video", Some("subs"), "Film (2020)"),
            video("sdh", "per-video", "3_English_SDH.srt"),
        ];
        assert_eq!(
            paired(&build_sidecars("share", &nodes)),
            vec![
                ("3_English_SDH.srt", "video", Some("eng"), Some("SDH")),
                ("Film (2020).German.srt", "video", Some("ger"), None),
            ]
        );
    }

    /// One video in the folder can only be the one; two leave a subtitle
    /// named after neither unpaired rather than guessed.
    #[test]
    fn a_subtitle_named_after_no_video_is_paired_only_when_its_folder_has_one() {
        let nodes = vec![
            root(),
            folder("film", Some("root"), "Film"),
            video("video", "film", "film.1080p.mkv"),
            video("english", "film", "English.srt"),
            folder("s1", Some("root"), "Season 01"),
            video("ep1", "s1", "S01E01.mkv"),
            video("ep2", "s1", "S01E02.mkv"),
            video("stray", "s1", "English.srt"),
        ];
        assert_eq!(
            paired(&build_sidecars("share", &nodes)),
            vec![("English.srt", "video", Some("eng"), None)]
        );
    }

    #[test]
    fn a_recrawl_replaces_a_shares_subtitles_and_removing_the_share_drops_them() {
        let mut catalog = Catalog::in_memory().expect("open");
        let sidecar = |link: &str| Sidecar {
            share_id: "s".into(),
            link_id: link.into(),
            volume_id: "vol".into(),
            video_link_id: "a".into(),
            name: format!("{link}.srt"),
            language: Some("eng".into()),
            title: None,
        };
        catalog
            .replace_sidecars("s", &[sidecar("old")])
            .expect("crawl");
        catalog
            .replace_sidecars("s", &[sidecar("new")])
            .expect("recrawl");
        assert_eq!(
            catalog.sidecars("s", "a").expect("read"),
            vec![sidecar("new")]
        );
        catalog.remove_share("s").expect("remove");
        assert!(catalog.sidecars("s", "a").expect("read").is_empty());
    }

    #[test]
    fn the_subtitle_turned_on_is_one_in_the_preferred_language_and_never_another() {
        let sidecar = |language: Option<&str>| Sidecar {
            share_id: "s".into(),
            link_id: "l".into(),
            volume_id: "v".into(),
            video_link_id: "a".into(),
            name: "a.srt".into(),
            language: language.map(str::to_owned),
            title: None,
        };
        let ger_eng = [sidecar(Some("ger")), sidecar(Some("eng"))];
        assert_eq!(preferred_sidecar(&ger_eng, Some("en")), Some(1));
        assert_eq!(preferred_sidecar(&ger_eng, None), Some(0));
        assert_eq!(preferred_sidecar(&ger_eng, Some("jpn")), None);
        let ger_unknown = [sidecar(Some("ger")), sidecar(None)];
        assert_eq!(preferred_sidecar(&ger_unknown, Some("eng")), Some(1));
        assert_eq!(preferred_sidecar(&[], None), None);
    }
}
