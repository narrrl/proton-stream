//! Catalog rows, grouped into what a poster wall shows.
//!
//! The catalog is flat — one row per file, with whatever [`crate::naming`] made
//! of its path. A UI wants the other shape: titles, each with seasons, each with
//! episodes, and the one episode the viewer should land on when they click. That
//! transform is here rather than in the app because it is pure, table-testable,
//! and the CLI wants the same view of the same data.
//!
//! Two decisions worth stating:
//!
//! * **Titles merge across shares.** Two shares holding the same series are one
//!   poster, not two. The grouping key is the normalised title, so a share added
//!   later slots into the title that is already there.
//! * **Watch state is joined in, not looked up per draw.** A grid render must
//!   not become one SQLite round-trip per tile — see the negative-caching trap
//!   in `docs/DEVELOPMENT.md`, which is the same lesson one layer up.

use std::collections::HashMap;

use crate::catalog::{CatalogNode, WatchState};

/// A file has been watched once playback passed this fraction of it. Short of
/// 1.0 because nobody sits through the credits, and a title that never reaches
/// "finished" keeps offering an episode the viewer is done with.
pub const WATCHED_FRACTION: f64 = 0.9;

/// Playback below this many seconds is not a resume point — it is a file the
/// viewer opened and immediately left.
pub const RESUME_FLOOR_SECS: f64 = 30.0;

/// Whether a title is a series or a one-off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TitleKind {
    Series,
    Film,
}

/// One playable file, with where the viewer left off in it.
#[derive(Debug, Clone, PartialEq)]
pub struct Episode {
    pub node: CatalogNode,
    pub watch: Option<WatchState>,
}

impl Episode {
    /// `S01E02`, `#014`, or nothing when the name numbers nothing.
    pub fn numbering(&self) -> Option<String> {
        match (self.node.parsed.season, self.node.parsed.episode) {
            (Some(season), Some(episode)) => Some(format!("S{season:02}E{episode:02}")),
            (None, Some(episode)) => Some(format!("#{episode:03}")),
            _ => None,
        }
    }

    /// What to put on the row: the episode's own name when the filename stated
    /// one, else the numbering, else the filename.
    pub fn label(&self) -> String {
        if let Some(title) = &self.node.parsed.episode_title {
            return title.clone();
        }
        self.numbering().unwrap_or_else(|| self.node.name.clone())
    }

    /// What to put on a row that already shows the numbering beside it: the
    /// episode's own name when the filename stated one, else the filename.
    ///
    /// Deliberately not [`Self::label`] — that falls back to the numbering,
    /// which on an episode row prints it twice.
    pub fn detail(&self) -> &str {
        self.node
            .parsed
            .episode_title
            .as_deref()
            .unwrap_or(&self.node.name)
    }

    /// Fraction of the file played, when both a position and a duration are
    /// known. `None` for a file that was never opened.
    pub fn progress(&self) -> Option<f64> {
        let watch = self.watch?;
        let duration = watch.duration_secs?;
        if duration <= 0.0 {
            return None;
        }
        Some((watch.position_secs / duration).clamp(0.0, 1.0))
    }

    /// Whether this counts as seen. Either the flag was set explicitly, or
    /// playback got far enough that setting it is what the viewer meant.
    pub fn is_watched(&self) -> bool {
        self.watch.is_some_and(|watch| watch.watched)
            || self.progress().is_some_and(|p| p >= WATCHED_FRACTION)
    }

    /// Where to resume, or `None` to start from the beginning. A file barely
    /// started, or effectively finished, starts over.
    pub fn resume_at(&self) -> Option<f64> {
        let watch = self.watch?;
        if self.is_watched() || watch.position_secs < RESUME_FLOOR_SECS {
            return None;
        }
        Some(watch.position_secs)
    }

    /// When playback last touched this file. `0` for one never opened, so it
    /// sorts below everything that was.
    pub fn last_played(&self) -> i64 {
        self.watch.map_or(0, |watch| watch.updated_at)
    }
}

/// The episodes of one season, or the unnumbered pile when a library uses
/// absolute numbering.
#[derive(Debug, Clone, PartialEq)]
pub struct Season {
    /// `None` when the names state no season. That is not season 1: an
    /// absolute-numbered anime and a film both land here.
    pub number: Option<u32>,
    pub episodes: Vec<Episode>,
}

impl Season {
    pub fn label(&self) -> String {
        match self.number {
            Some(number) => format!("Season {number}"),
            None => "Episodes".to_string(),
        }
    }
}

/// A series or a film: everything under one title, across every share.
#[derive(Debug, Clone, PartialEq)]
pub struct Title {
    /// Stable identity for the UI to route on and for caches to key on.
    pub key: String,
    /// What to print.
    pub name: String,
    pub year: Option<u32>,
    pub kind: TitleKind,
    pub seasons: Vec<Season>,
    /// Every share contributing files, in first-seen order.
    pub share_ids: Vec<String>,
}

impl Title {
    /// Every episode, in display order.
    pub fn episodes(&self) -> impl Iterator<Item = &Episode> {
        self.seasons
            .iter()
            .flat_map(|season| season.episodes.iter())
    }

    pub fn episode_count(&self) -> usize {
        self.seasons
            .iter()
            .map(|season| season.episodes.len())
            .sum()
    }

    pub fn watched_count(&self) -> usize {
        self.episodes().filter(|e| e.is_watched()).count()
    }

    /// Whether the files themselves say this is episodic, rather than
    /// [`Title::kind`] having been inferred from how many of them there are.
    ///
    /// The difference matters to whoever is matching this against a provider: a
    /// folder of three unnumbered films is a `Series` here purely because it
    /// holds three files, and it must not be scored down against the film
    /// entries it is actually made of. Numbering is a statement; a file count
    /// is a guess.
    pub fn states_its_numbering(&self) -> bool {
        self.episodes()
            .any(|episode| episode.node.parsed.is_episode() || episode.node.parsed.season.is_some())
    }

    /// The file to pull a poster thumbnail from: the first episode there is.
    /// Proton renders a thumbnail per file, so any of them is a frame of the
    /// right show; the first is the one most likely already cached.
    pub fn poster_node(&self) -> Option<&CatalogNode> {
        self.episodes().next().map(|episode| &episode.node)
    }

    /// The episode that is part-watched, most recently played first.
    pub fn resume(&self) -> Option<&Episode> {
        self.episodes()
            .filter(|episode| episode.resume_at().is_some())
            .max_by_key(|episode| episode.last_played())
    }

    /// What a click on the poster should play: whatever is part-watched, else
    /// the first unwatched episode, else the first episode.
    pub fn next_up(&self) -> Option<&Episode> {
        self.resume()
            .or_else(|| self.episodes().find(|episode| !episode.is_watched()))
            .or_else(|| self.episodes().next())
    }

    /// Where a file sits in display order, if it is under this title at all.
    ///
    /// Identity is `(share_id, link_id)`, not the name: a title merged across
    /// two shares can hold two files called `E01.mkv`, and "play the next one"
    /// must not be able to jump between shares by accident.
    pub fn position_of(&self, share_id: &str, link_id: &str) -> Option<usize> {
        self.episodes().position(|episode| {
            episode.node.share_id == share_id && episode.node.link_id == link_id
        })
    }

    /// What comes after a file, in display order. `None` at the end of the
    /// title, and for a file that is not part of it.
    pub fn following(&self, share_id: &str, link_id: &str) -> Option<&Episode> {
        let position = self.position_of(share_id, link_id)?;
        self.episodes().nth(position + 1)
    }

    /// What comes before it.
    pub fn preceding(&self, share_id: &str, link_id: &str) -> Option<&Episode> {
        let position = self.position_of(share_id, link_id)?;
        self.episodes().nth(position.checked_sub(1)?)
    }

    /// When this title was last played at all.
    pub fn last_played(&self) -> i64 {
        self.episodes().map(Episode::last_played).max().unwrap_or(0)
    }
}

/// Every title the catalog holds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Library {
    pub titles: Vec<Title>,
}

impl Library {
    /// Group catalog rows into titles, joining in watch state keyed by
    /// `(share_id, link_id)`.
    ///
    /// Folder rows are ignored: a poster wall shows playable files, and the
    /// folder structure has already been consumed by the parser that produced
    /// these titles.
    pub fn build(nodes: Vec<CatalogNode>, watch: &HashMap<(String, String), WatchState>) -> Self {
        // Insertion-ordered grouping: key -> index into `titles`, so the output
        // is deterministic before it is sorted.
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut titles: Vec<Title> = Vec::new();

        for node in nodes {
            if node.is_folder {
                continue;
            }
            let key = title_key(&node.parsed.title);
            let position = match index.get(&key) {
                Some(position) => *position,
                None => {
                    titles.push(Title {
                        key: key.clone(),
                        name: node.parsed.title.clone(),
                        year: node.parsed.year,
                        // Provisional: settled once every file is in.
                        kind: TitleKind::Film,
                        seasons: Vec::new(),
                        share_ids: Vec::new(),
                    });
                    index.insert(key, titles.len() - 1);
                    titles.len() - 1
                }
            };

            let title = &mut titles[position];
            if title.year.is_none() {
                title.year = node.parsed.year;
            }
            if !title.share_ids.contains(&node.share_id) {
                title.share_ids.push(node.share_id.clone());
            }

            let watch_state = watch
                .get(&(node.share_id.clone(), node.link_id.clone()))
                .copied();
            let season_number = node.parsed.season;
            let episode = Episode {
                node,
                watch: watch_state,
            };

            match title
                .seasons
                .iter_mut()
                .find(|season| season.number == season_number)
            {
                Some(season) => season.episodes.push(episode),
                None => title.seasons.push(Season {
                    number: season_number,
                    episodes: vec![episode],
                }),
            }
        }

        for title in &mut titles {
            // A season number, an episode number or more than one file all mean
            // series. A lone unnumbered file is a film.
            let numbered = title.states_its_numbering();
            title.kind = if numbered || title.episode_count() > 1 {
                TitleKind::Series
            } else {
                TitleKind::Film
            };

            // Unnumbered seasons last: they are the specials pile, not season 0.
            title
                .seasons
                .sort_by_key(|season| (season.number.is_none(), season.number));
            for season in &mut title.seasons {
                season.episodes.sort_by(|left, right| {
                    left.node
                        .parsed
                        .episode
                        .cmp(&right.node.parsed.episode)
                        .then_with(|| left.node.name.cmp(&right.node.name))
                });
            }
        }

        titles.sort_by_key(|title| sort_name(&title.name));
        Self { titles }
    }

    pub fn is_empty(&self) -> bool {
        self.titles.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&Title> {
        self.titles.iter().find(|title| title.key == key)
    }

    /// Titles with something part-watched, most recent first. The top row of
    /// the library.
    pub fn continue_watching(&self) -> Vec<&Title> {
        let mut titles: Vec<&Title> = self
            .titles
            .iter()
            .filter(|title| title.resume().is_some())
            .collect();
        titles.sort_by_key(|title| std::cmp::Reverse(title.last_played()));
        titles
    }

    /// Substring match over the display name and every filename under it, so
    /// searching for a release group or an episode name finds its title.
    pub fn search(&self, query: &str) -> Vec<&Title> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return self.titles.iter().collect();
        }
        self.titles
            .iter()
            .filter(|title| {
                title.name.to_lowercase().contains(&needle)
                    || title
                        .episodes()
                        .any(|episode| episode.node.name.to_lowercase().contains(&needle))
            })
            .collect()
    }
}

/// The grouping key: case- and punctuation-insensitive, so `Fullmetal
/// Alchemist: Brotherhood` and `Fullmetal Alchemist Brotherhood` are one title.
///
/// An apostrophe is the one punctuation mark that closes up rather than
/// separating: `Heaven's Feel` keys as `heavens feel`, which is what a folder
/// that had to give its apostrophe up to the filesystem is called. Splitting it
/// into `heaven s feel` instead files the two apart and, worse, costs the
/// matcher a real match on the difference.
pub fn title_key(title: &str) -> String {
    let mut key = String::with_capacity(title.len());
    let mut pending_space = false;
    for character in title.chars() {
        if matches!(character, '\'' | '\u{2019}' | '\u{02bc}') {
            continue;
        }
        if character.is_alphanumeric() {
            if pending_space && !key.is_empty() {
                key.push(' ');
            }
            pending_space = false;
            key.extend(character.to_lowercase());
        } else {
            pending_space = true;
        }
    }
    key
}

/// Sort key for the shelf: case-insensitive, and a leading article ignored, so
/// `The Expanse` files under E where a viewer looks for it.
fn sort_name(name: &str) -> String {
    let lower = name.to_lowercase();
    for article in ["the ", "a ", "an "] {
        if let Some(rest) = lower.strip_prefix(article) {
            return rest.to_string();
        }
    }
    lower
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::naming::ParsedName;

    fn node(
        share: &str,
        link: &str,
        name: &str,
        season: Option<u32>,
        episode: Option<u32>,
    ) -> CatalogNode {
        CatalogNode {
            share_id: share.into(),
            link_id: link.into(),
            volume_id: "vol".into(),
            parent_link_id: None,
            name: name.into(),
            is_folder: false,
            media_type: Some("video/x-matroska".into()),
            size: Some(1024),
            active_revision_id: Some("rev".into()),
            parsed: ParsedName {
                title: name.split(" - ").next().unwrap_or(name).to_string(),
                season,
                episode,
                year: None,
                episode_title: None,
            },
        }
    }

    #[test]
    fn the_next_episode_is_the_next_one_in_display_order_across_seasons() {
        let library = Library::build(
            vec![
                node("s1", "a", "Show - S01E01.mkv", Some(1), Some(1)),
                node("s1", "b", "Show - S01E02.mkv", Some(1), Some(2)),
                node("s1", "c", "Show - S02E01.mkv", Some(2), Some(1)),
            ],
            &HashMap::new(),
        );
        let title = &library.titles[0];

        assert_eq!(
            title.following("s1", "a").map(|e| e.node.link_id.clone()),
            Some("b".to_string())
        );
        // Autoplay has to walk off the end of a season into the next one, or it
        // stops two thirds of the way through a series for no reason a viewer
        // can see.
        assert_eq!(
            title.following("s1", "b").map(|e| e.node.link_id.clone()),
            Some("c".to_string())
        );
        assert_eq!(title.following("s1", "c"), None);

        assert_eq!(
            title.preceding("s1", "c").map(|e| e.node.link_id.clone()),
            Some("b".to_string())
        );
        assert_eq!(title.preceding("s1", "a"), None);
    }

    #[test]
    fn a_file_from_another_share_is_not_this_titles_neighbour() {
        let library = Library::build(
            vec![
                node("s1", "a", "Show - S01E01.mkv", Some(1), Some(1)),
                node("s1", "b", "Show - S01E02.mkv", Some(1), Some(2)),
            ],
            &HashMap::new(),
        );
        let title = &library.titles[0];
        // Same link id, different share: two shares of the same series really
        // do collide on names, and "next" must not cross over on a false match.
        assert_eq!(title.position_of("other", "a"), None);
        assert_eq!(title.following("other", "a"), None);
    }

    fn watch(position: f64, duration: f64, watched: bool, at: i64) -> WatchState {
        WatchState {
            position_secs: position,
            duration_secs: Some(duration),
            watched,
            updated_at: at,
        }
    }

    #[test]
    fn groups_episodes_into_seasons() {
        let nodes = vec![
            node("s", "1", "Frieren - S01E02", Some(1), Some(2)),
            node("s", "2", "Frieren - S01E01", Some(1), Some(1)),
            node("s", "3", "Frieren - S02E01", Some(2), Some(1)),
        ];
        let library = Library::build(nodes, &HashMap::new());

        assert_eq!(library.titles.len(), 1);
        let title = &library.titles[0];
        assert_eq!(title.kind, TitleKind::Series);
        assert_eq!(title.seasons.len(), 2);
        assert_eq!(title.seasons[0].number, Some(1));
        // Sorted by episode number, not by insertion order.
        assert_eq!(title.seasons[0].episodes[0].node.link_id, "2");
        assert_eq!(title.episode_count(), 3);
    }

    #[test]
    fn a_row_detail_never_repeats_the_numbering() {
        let mut file = node("s", "1", "Show - S01E01", Some(1), Some(1));
        file.name = "[Group] Show - 01 (1080p).mkv".into();
        let library = Library::build(vec![file], &HashMap::new());
        let episode = &library.titles[0].seasons[0].episodes[0];

        // No episode name in the filename: the row shows the filename, while
        // the card — which has no numbering column — shows the numbering.
        assert_eq!(episode.detail(), "[Group] Show - 01 (1080p).mkv");
        assert_eq!(episode.label(), "S01E01");
    }

    #[test]
    fn a_stated_episode_name_wins_on_both() {
        let mut file = node("s", "1", "Show - S01E01", Some(1), Some(1));
        file.parsed.episode_title = Some("Mother and Children".into());
        let library = Library::build(vec![file], &HashMap::new());
        let episode = &library.titles[0].seasons[0].episodes[0];

        assert_eq!(episode.detail(), "Mother and Children");
        assert_eq!(episode.label(), "Mother and Children");
    }

    #[test]
    fn unnumbered_seasons_sort_last() {
        let nodes = vec![
            node("s", "1", "Show - OVA", None, None),
            node("s", "2", "Show - S01E01", Some(1), Some(1)),
        ];
        let library = Library::build(nodes, &HashMap::new());
        let seasons = &library.titles[0].seasons;
        assert_eq!(seasons[0].number, Some(1));
        assert_eq!(seasons[1].number, None);
        assert_eq!(seasons[1].label(), "Episodes");
    }

    #[test]
    fn a_lone_unnumbered_file_is_a_film() {
        let nodes = vec![node("s", "1", "Spirited Away", None, None)];
        let library = Library::build(nodes, &HashMap::new());
        assert_eq!(library.titles[0].kind, TitleKind::Film);
    }

    #[test]
    fn titles_merge_across_shares() {
        let nodes = vec![
            node("a", "1", "Frieren - S01E01", Some(1), Some(1)),
            node("b", "2", "frieren! - S01E02", Some(1), Some(2)),
        ];
        let library = Library::build(nodes, &HashMap::new());
        assert_eq!(library.titles.len(), 1);
        assert_eq!(library.titles[0].share_ids, vec!["a", "b"]);
    }

    #[test]
    fn next_up_prefers_a_resume_over_an_unwatched_episode() {
        let nodes = vec![
            node("s", "1", "Show - S01E01", Some(1), Some(1)),
            node("s", "2", "Show - S01E02", Some(1), Some(2)),
            node("s", "3", "Show - S01E03", Some(1), Some(3)),
        ];
        let mut states = HashMap::new();
        // Episode 1 finished, episode 2 half-watched.
        states.insert(("s".into(), "1".into()), watch(1400.0, 1440.0, false, 10));
        states.insert(("s".into(), "2".into()), watch(700.0, 1440.0, false, 20));

        let library = Library::build(nodes, &states);
        let title = &library.titles[0];

        assert_eq!(title.watched_count(), 1);
        assert_eq!(title.next_up().unwrap().node.link_id, "2");
        assert_eq!(title.resume().unwrap().resume_at(), Some(700.0));
        assert_eq!(library.continue_watching().len(), 1);
    }

    #[test]
    fn next_up_falls_through_to_the_first_unwatched() {
        let nodes = vec![
            node("s", "1", "Show - S01E01", Some(1), Some(1)),
            node("s", "2", "Show - S01E02", Some(1), Some(2)),
        ];
        let mut states = HashMap::new();
        states.insert(("s".into(), "1".into()), watch(1440.0, 1440.0, true, 10));
        let library = Library::build(nodes, &states);
        assert_eq!(library.titles[0].next_up().unwrap().node.link_id, "2");
    }

    #[test]
    fn a_barely_started_file_starts_over() {
        let nodes = vec![node("s", "1", "Show - S01E01", Some(1), Some(1))];
        let mut states = HashMap::new();
        states.insert(("s".into(), "1".into()), watch(4.0, 1440.0, false, 10));
        let library = Library::build(nodes, &states);
        let episode = &library.titles[0].seasons[0].episodes[0];
        assert_eq!(episode.resume_at(), None);
        assert!(library.continue_watching().is_empty());
    }

    #[test]
    fn a_nearly_finished_file_counts_as_watched() {
        let nodes = vec![node("s", "1", "Show - S01E01", Some(1), Some(1))];
        let mut states = HashMap::new();
        states.insert(("s".into(), "1".into()), watch(1400.0, 1440.0, false, 10));
        let library = Library::build(nodes, &states);
        assert!(library.titles[0].seasons[0].episodes[0].is_watched());
    }

    #[test]
    fn search_matches_filenames_as_well_as_titles() {
        let mut first = node("s", "1", "Frieren - S01E01", Some(1), Some(1));
        first.name = "[SubsPlease] Frieren - 01 (1080p).mkv".into();
        let library = Library::build(vec![first], &HashMap::new());

        assert_eq!(library.search("subsplease").len(), 1);
        assert_eq!(library.search("frieren").len(), 1);
        assert_eq!(library.search("bebop").len(), 0);
        assert_eq!(library.search("  ").len(), 1);
    }

    #[test]
    fn sorting_ignores_a_leading_article() {
        let nodes = vec![
            node("s", "1", "Zoo", None, Some(1)),
            node("s", "2", "The Expanse", None, Some(1)),
            node("s", "3", "Akira", None, Some(1)),
        ];
        let library = Library::build(nodes, &HashMap::new());
        let names: Vec<&str> = library.titles.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["Akira", "The Expanse", "Zoo"]);
    }

    #[test]
    fn title_key_ignores_punctuation_and_case() {
        assert_eq!(
            title_key("Fullmetal Alchemist: Brotherhood"),
            title_key("fullmetal alchemist brotherhood")
        );
        assert_eq!(title_key("K-On!!"), "k on");
    }

    /// An apostrophe closes up rather than separating, and a name written to
    /// disk has usually lost it — the two have to key alike, or a folder called
    /// `Heavens Feel` is a different title to the one the provider has.
    #[test]
    fn an_apostrophe_keys_the_same_as_its_absence() {
        assert_eq!(title_key("Heaven's Feel"), "heavens feel");
        assert_eq!(title_key("Heaven\u{2019}s Feel"), title_key("Heavens Feel"));
        assert_eq!(title_key("Devils' Line"), title_key("Devils Line"));
    }

    #[test]
    fn folders_are_not_episodes() {
        let mut folder = node("s", "1", "Show", None, None);
        folder.is_folder = true;
        let library = Library::build(vec![folder], &HashMap::new());
        assert!(library.is_empty());
    }
}
