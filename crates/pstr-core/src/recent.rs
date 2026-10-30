//! The last few searches, offered again under an empty search box.
//!
//! Kept on this device only, in `recent-searches.json` beside the other
//! settings, and never synced: what someone typed is not a setting, and
//! another device's list is not what they were just looking for.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::{AppDirs, read_json, write_json};
use crate::error::Result;

/// How many searches are kept.
pub const KEEP: usize = 8;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentSearches {
    /// Newest first.
    #[serde(default)]
    pub queries: Vec<String>,
}

impl RecentSearches {
    /// Put `query` first, saying whether the list changed. The same search
    /// in other letter case moves rather than repeats, and one under two
    /// letters is not kept.
    pub fn remember(&mut self, query: &str) -> bool {
        let query = query.trim();
        if query.chars().count() < 2 {
            return false;
        }
        if self.queries.first().map(String::as_str) == Some(query) {
            return false;
        }
        let lower = query.to_lowercase();
        self.queries.retain(|kept| kept.to_lowercase() != lower);
        self.queries.insert(0, query.to_owned());
        self.queries.truncate(KEEP);
        true
    }
}

fn file(dirs: &AppDirs) -> PathBuf {
    dirs.config.join("recent-searches.json")
}

/// The list, empty when there is no file yet.
pub fn load(dirs: &AppDirs) -> Result<RecentSearches> {
    Ok(read_json::<RecentSearches>(&file(dirs))?.unwrap_or_default())
}

pub fn save(dirs: &AppDirs, recent: &RecentSearches) -> Result<()> {
    write_json(&file(dirs), recent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_repeated_moves_to_the_front_instead_of_appearing_twice() {
        let mut recent = RecentSearches::default();
        assert!(recent.remember("frieren"));
        assert!(recent.remember("kon"));
        assert!(recent.remember("Frieren "));
        assert_eq!(recent.queries, ["Frieren", "kon"]);
        assert!(!recent.remember("Frieren"));
        assert!(!recent.remember("x"));
    }

    #[test]
    fn only_the_newest_searches_are_kept() {
        let mut recent = RecentSearches::default();
        for at in 0..KEEP + 3 {
            recent.remember(&format!("search {at}"));
        }
        assert_eq!(recent.queries.len(), KEEP);
        assert_eq!(recent.queries[0], format!("search {}", KEEP + 2));
    }
}
