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
use crate::metadata::{EpisodeGuide, EpisodeMetadata, MetadataRecord, ProviderId, TitleMetadata};
use crate::naming::{self, ParsedName};

/// Bump when adding a migration.
pub const SCHEMA_VERSION: i64 = 7;

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
}
impl Default for TitleTrackPrefs {
    fn default() -> Self {
        Self {
            audio_language: None,
            subtitle_language: None,
            subtitles: true,
        }
    }
}

/// What `season = -1` means in `episode_metadata`: no season at all.
const ABSOLUTE_SEASON: i64 = -1;

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

        if !retain_offline {
            tx.execute("DELETE FROM offline_files WHERE share_id = ?1 AND NOT EXISTS (SELECT 1 FROM nodes WHERE nodes.share_id = offline_files.share_id AND nodes.link_id = offline_files.link_id AND nodes.active_revision_id = offline_files.revision_id)", params![share_id])?;
        }

        tx.commit()?;
        Ok(())
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

    /// Drop everything a share contributed, watch positions included.
    ///
    /// Unlike a recrawl — which replaces rows and deliberately leaves watch
    /// state alone — this is the viewer saying they are done with the share.
    /// Keeping positions for files they can no longer reach would only leave
    /// the "continue watching" row pointing at nothing.
    pub fn remove_share(&mut self, share_id: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM nodes WHERE share_id = ?1", params![share_id])?;
        tx.execute(
            "DELETE FROM watch_state WHERE share_id = ?1",
            params![share_id],
        )?;
        tx.execute(
            "DELETE FROM offline_files WHERE share_id = ?1",
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
        self.conn.query_row("SELECT audio_language, subtitle_language, subtitles FROM title_track_prefs WHERE title_key=?1", params![title_key], |r| Ok(TitleTrackPrefs { audio_language:r.get(0)?, subtitle_language:r.get(1)?, subtitles:r.get::<_,i64>(2)? != 0 })).optional().map_err(Into::into)
    }
    pub fn set_title_track_prefs(&self, title_key: &str, prefs: &TitleTrackPrefs) -> Result<()> {
        self.conn.execute("INSERT INTO title_track_prefs (title_key,audio_language,subtitle_language,subtitles) VALUES (?1,?2,?3,?4) ON CONFLICT(title_key) DO UPDATE SET audio_language=excluded.audio_language,subtitle_language=excluded.subtitle_language,subtitles=excluded.subtitles", params![title_key,prefs.audio_language,prefs.subtitle_language,prefs.subtitles as i64])?;
        Ok(())
    }

    pub fn all_title_track_prefs(&self) -> Result<HashMap<String, TitleTrackPrefs>> {
        let mut statement = self.conn.prepare(
            "SELECT title_key, audio_language, subtitle_language, subtitles FROM title_track_prefs",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get(0)?,
                TitleTrackPrefs {
                    audio_language: row.get(1)?,
                    subtitle_language: row.get(2)?,
                    subtitles: row.get::<_, i64>(3)? != 0,
                },
            ))
        })?;
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
        self.conn.execute(
            "INSERT INTO watch_state (share_id, link_id, position_secs, duration_secs, watched, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (share_id, link_id) DO UPDATE SET
                 position_secs = excluded.position_secs,
                 duration_secs = excluded.duration_secs,
                 watched       = excluded.watched,
                 updated_at    = excluded.updated_at",
            params![
                share_id,
                link_id,
                state.position_secs,
                state.duration_secs,
                state.watched as i64,
                state.updated_at,
            ],
        )?;
        Ok(())
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
                    manual
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
                        url, manual
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
    pub fn set_metadata(&self, record: &MetadataRecord) -> Result<()> {
        let data = record.metadata.as_ref();
        self.conn.execute(
            "INSERT INTO title_metadata (
                 title_key, provider, matched, fetched_at, remote_id, name, original_name,
                 overview, year, kind, poster_url, backdrop_url, rating, genres, episodes, url,
                 manual
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
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
                 backdrop_url  = excluded.backdrop_url,
                 rating        = excluded.rating,
                 genres        = excluded.genres,
                 episodes      = excluded.episodes,
                 url           = excluded.url,
                 manual        = excluded.manual",
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

    /// Replace everything stored about one title's episodes.
    ///
    /// A replace rather than an upsert per row: a provider that has *dropped*
    /// an episode — a special that was recounted, a season renumbered — should
    /// leave nothing behind, and an episode list is small enough that the
    /// difference is a few hundred microseconds.
    pub fn set_episode_metadata(
        &mut self,
        title_key: &str,
        provider: ProviderId,
        fetched_at: i64,
        episodes: &[EpisodeMetadata],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM episode_metadata WHERE title_key = ?1",
            params![title_key],
        )?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO episode_metadata (
                     title_key, season, number, provider, fetched_at,
                     name, overview, still_url, air_date
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
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
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Every episode answer there is, keyed by title.
    ///
    /// Read whole, for the same reason watch state and title metadata are: a
    /// season drawn a row at a time must not be a SQLite round-trip per row.
    pub fn all_episode_metadata(&self) -> Result<HashMap<String, EpisodeGuide>> {
        let mut statement = self.conn.prepare(
            "SELECT title_key, season, number, name, overview, still_url, air_date
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

    /// Which titles already have an episode list stored, and how fresh.
    ///
    /// Enough to decide whether to ask again without reading every row: the
    /// answer is only ever compared against a TTL.
    pub fn episode_metadata_ages(&self) -> Result<HashMap<String, (ProviderId, i64)>> {
        let mut statement = self.conn.prepare(
            "SELECT title_key, provider, MIN(fetched_at) FROM episode_metadata GROUP BY title_key",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;

        let mut ages = HashMap::new();
        for row in rows {
            let (title_key, provider, fetched_at) = row?;
            if let Some(provider) = ProviderId::parse(&provider) {
                ages.insert(title_key, (provider, fetched_at));
            }
        }
        Ok(ages)
    }
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
                    && row.parsed.title != title
                {
                    let rest = strip_prefix_title(&row.parsed.title, &title);
                    // …unless what is left is a season marker. `Oshi no Ko 3rd
                    // Season - 07.mkv` under a folder called `Oshi no Ko` leaves
                    // `3rd Season`, and calling every episode of season three
                    // that is worse than calling none of them anything.
                    if naming::is_season_marker(&rest) {
                        row.parsed.season = row.parsed.season.or(naming::parse(&rest).season);
                    } else {
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
fn strip_prefix_title(episode: &str, series: &str) -> String {
    let stripped = episode
        .strip_prefix(series)
        .map(|rest| rest.trim_start_matches([' ', '-', '_', '.', ':']))
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
                name: Some("The Immortal Legion".into()),
                overview: Some("Ed and Al…".into()),
                still_url: None,
                air_date: Some("2010-11-14".into()),
            },
            EpisodeMetadata {
                season: Some(1),
                number: 1,
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

        let ages = catalog.episode_metadata_ages().expect("ages");
        assert_eq!(ages["fma"], (ProviderId::AniList, 200));
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
}
