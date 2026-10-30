//! What arrived since the viewer last looked: the **New** and **3 new**
//! badges, the "New episodes" and "Recently added" rows, and the countdown on
//! an airing show's tile.
//!
//! All of it is read off [`crate::catalog::Catalog::first_seen`], the time a
//! crawl first found each file. A file the first crawl of its share found is
//! not new, however recent that crawl: it was there before this device looked,
//! and a share added today would otherwise badge its whole library.

use std::collections::HashMap;

use crate::library::{Episode, Title, TitleKind};
use crate::shelves::Shelf;

/// How long a title that arrived stays **New**, unless it is started first.
pub const NEW_FOR: i64 = 14 * 86_400;

/// What a tile says about what arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Badge {
    /// A title that arrived in the last [`NEW_FOR`] and was never played.
    New,
    /// A series being watched, with this many unwatched episodes that arrived
    /// after it was last played.
    Episodes(usize),
}

impl Badge {
    pub fn label(self) -> String {
        match self {
            Self::New => "New".to_owned(),
            Self::Episodes(count) => format!("{count} new"),
        }
    }
}

/// When each file arrived, read against when its share was first crawled.
pub struct Arrivals<'a> {
    first_seen: &'a HashMap<(String, String), i64>,
    /// The earliest sighting in each share: its first crawl.
    first_crawl: HashMap<&'a str, i64>,
}

impl<'a> Arrivals<'a> {
    pub fn new(first_seen: &'a HashMap<(String, String), i64>) -> Self {
        let mut first_crawl: HashMap<&str, i64> = HashMap::new();
        for ((share, _), seen) in first_seen {
            first_crawl
                .entry(share.as_str())
                .and_modify(|first| *first = (*first).min(*seen))
                .or_insert(*seen);
        }
        Self {
            first_seen,
            first_crawl,
        }
    }

    /// When `episode`'s file arrived, if a crawl after its share's first one
    /// found it.
    pub fn arrived(&self, episode: &Episode) -> Option<i64> {
        let node = &episode.node;
        let seen = *self
            .first_seen
            .get(&(node.share_id.clone(), node.link_id.clone()))?;
        let first = self.first_crawl.get(node.share_id.as_str())?;
        (seen > *first).then_some(seen)
    }

    /// The badge for `title`, if it has one.
    pub fn badge(&self, title: &Title, now: i64) -> Option<Badge> {
        let played = title.last_played();
        if played == 0 {
            // New only if every file of it arrived: a show the first crawl
            // found is not new because a crawl since added an episode.
            let arrived: Option<Vec<i64>> = title
                .episodes()
                .map(|episode| self.arrived(episode))
                .collect();
            let first = arrived?.into_iter().min()?;
            return (now - first < NEW_FOR).then_some(Badge::New);
        }
        if title.kind != TitleKind::Series {
            return None;
        }
        let count = title
            .episodes()
            .filter(|episode| !episode.is_watched())
            .filter(|episode| self.arrived(episode).is_some_and(|at| at > played))
            .count();
        (count > 0).then_some(Badge::Episodes(count))
    }

    /// When `title` last gained a file that counts as arrived.
    fn newest(&self, title: &Title) -> Option<i64> {
        title
            .episodes()
            .filter_map(|episode| self.arrived(episode))
            .max()
    }
}

/// Every badge in `titles`, by key.
pub fn badges(
    titles: &[Title],
    first_seen: &HashMap<(String, String), i64>,
    now: i64,
) -> HashMap<String, Badge> {
    let arrivals = Arrivals::new(first_seen);
    titles
        .iter()
        .filter_map(|title| Some((title.key.clone(), arrivals.badge(title, now)?)))
        .collect()
}

/// The two rows of what arrived, newest arrival first. `None` for a row with
/// nothing in it.
#[derive(Debug, Default)]
pub struct Rows {
    /// "New episodes": the shows being watched that gained episodes.
    pub episodes: Option<Shelf>,
    /// "Recently added": the titles that are [`Badge::New`].
    pub added: Option<Shelf>,
}

pub fn rows(titles: &[Title], first_seen: &HashMap<(String, String), i64>, now: i64) -> Rows {
    let arrivals = Arrivals::new(first_seen);
    let mut episodes: Vec<(&Title, i64)> = Vec::new();
    let mut added: Vec<(&Title, i64)> = Vec::new();
    for title in titles {
        let (Some(badge), Some(newest)) = (arrivals.badge(title, now), arrivals.newest(title))
        else {
            continue;
        };
        match badge {
            Badge::Episodes(_) => episodes.push((title, newest)),
            Badge::New => added.push((title, newest)),
        }
    }
    let shelf = |label: &str, mut row: Vec<(&Title, i64)>| {
        row.sort_by_key(|(_, newest)| std::cmp::Reverse(*newest));
        (!row.is_empty()).then(|| Shelf {
            label: label.to_owned(),
            keys: row
                .into_iter()
                .map(|(title, _)| title.key.clone())
                .collect(),
        })
    };
    Rows {
        episodes: shelf("New episodes", episodes),
        added: shelf("Recently added", added),
    }
}

/// What an airing show's tile says about its next episode: `Ep 8 · in 2 d`,
/// `Ep 8 · in 5 h`, `Ep 8 · soon`.
pub fn countdown(episode: u32, at: i64, now: i64) -> String {
    let left = (at - now).max(0);
    let when = match left {
        0..3_600 => "soon".to_owned(),
        3_600..86_400 => format!("in {} h", left / 3_600),
        _ => format!("in {} d", left / 86_400),
    };
    format!("Ep {episode} · {when}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{CatalogNode, WatchState};
    use crate::library::Library;

    const DAY: i64 = 86_400;

    fn file(share: &str, link: &str, title: &str, episode: u32) -> CatalogNode {
        CatalogNode {
            share_id: share.into(),
            link_id: link.into(),
            volume_id: "v".into(),
            parent_link_id: None,
            name: format!("{title} - {episode:02}.mkv"),
            is_folder: false,
            media_type: Some("video/x-matroska".into()),
            size: Some(1),
            active_revision_id: Some("r".into()),
            parsed: crate::naming::parse(&format!("{title} - {episode:02}.mkv")),
        }
    }

    fn watched(at: i64) -> WatchState {
        WatchState {
            position_secs: 1_400.0,
            duration_secs: Some(1_440.0),
            watched: true,
            updated_at: at,
        }
    }

    /// One share whose first crawl at day 1 found Frieren 1–2, and Dandadan 1
    /// arriving at day 20 next to Frieren 3.
    fn share(frieren_played: Option<i64>) -> (Library, HashMap<(String, String), i64>) {
        let files = vec![
            file("s", "f1", "Frieren", 1),
            file("s", "f2", "Frieren", 2),
            file("s", "f3", "Frieren", 3),
            file("s", "d1", "Dandadan", 1),
        ];
        let mut watch = HashMap::new();
        if let Some(at) = frieren_played {
            watch.insert(("s".to_owned(), "f1".to_owned()), watched(at));
            watch.insert(("s".to_owned(), "f2".to_owned()), watched(at));
        }
        let seen = [("f1", 1), ("f2", 1), ("f3", 20), ("d1", 20)]
            .into_iter()
            .map(|(link, day)| (("s".to_owned(), link.to_owned()), day * DAY))
            .collect();
        (Library::build(files, &watch), seen)
    }

    fn named<'a>(library: &'a Library, name: &str) -> &'a Title {
        library
            .titles
            .iter()
            .find(|title| title.name == name)
            .unwrap()
    }

    #[test]
    fn what_the_first_crawl_of_a_share_found_is_never_new() {
        let (library, seen) = share(None);
        let arrivals = Arrivals::new(&seen);
        let frieren = named(&library, "Frieren");
        assert_eq!(arrivals.badge(frieren, 21 * DAY), None);
    }

    #[test]
    fn a_title_that_arrived_is_new_for_two_weeks_unless_it_is_started() {
        let (library, seen) = share(None);
        let arrivals = Arrivals::new(&seen);
        let dandadan = named(&library, "Dandadan");
        assert_eq!(arrivals.badge(dandadan, 21 * DAY), Some(Badge::New));
        assert_eq!(arrivals.badge(dandadan, 20 * DAY + NEW_FOR), None);
    }

    #[test]
    fn a_show_being_watched_counts_the_episodes_that_arrived_after_the_last_watch() {
        let (library, seen) = share(Some(10 * DAY));
        let arrivals = Arrivals::new(&seen);
        let frieren = named(&library, "Frieren");
        assert_eq!(arrivals.badge(frieren, 21 * DAY), Some(Badge::Episodes(1)));
        assert_eq!(Badge::Episodes(1).label(), "1 new");

        // Watched after episode 3 arrived: nothing is new to the viewer.
        let (library, seen) = share(Some(25 * DAY));
        let arrivals = Arrivals::new(&seen);
        assert_eq!(arrivals.badge(named(&library, "Frieren"), 26 * DAY), None);
    }

    #[test]
    fn the_rows_hold_new_episodes_and_recent_arrivals_and_leave_out_an_empty_one() {
        let (library, seen) = share(Some(10 * DAY));
        let found = rows(&library.titles, &seen, 21 * DAY);
        let labels: Vec<(&str, Vec<&str>)> = [&found.episodes, &found.added]
            .into_iter()
            .flatten()
            .map(|row| {
                let names = row
                    .keys
                    .iter()
                    .map(|key| {
                        library
                            .titles
                            .iter()
                            .find(|title| &title.key == key)
                            .unwrap()
                            .name
                            .as_str()
                    })
                    .collect();
                (row.label.as_str(), names)
            })
            .collect();
        assert_eq!(
            labels,
            [
                ("New episodes", vec!["Frieren"]),
                ("Recently added", vec!["Dandadan"])
            ]
        );

        let (library, seen) = share(None);
        let found = rows(&library.titles, &seen, 21 * DAY);
        assert!(found.episodes.is_none());
        assert_eq!(found.added.unwrap().label, "Recently added");
    }

    #[test]
    fn the_countdown_says_days_then_hours_then_soon() {
        assert_eq!(countdown(8, 3 * DAY + 100, 0), "Ep 8 · in 3 d");
        assert_eq!(countdown(8, 5 * 3_600 + 10, 0), "Ep 8 · in 5 h");
        assert_eq!(countdown(8, 600, 0), "Ep 8 · soon");
    }
}
