//! My List: the titles the viewer put aside to watch, kept in
//! `my-list.json` beside the other settings.
//!
//! Keyed by title key, like a hand-picked match, so a title keeps its place on
//! the list across a recrawl and on every device that has the same share. A
//! title that leaves the library stays on the list, unseen, and is back on it
//! when the share brings it back.
//!
//! It travels with the profile, one `list/<title key>` register per title
//! ([`crate::profile`]): adding a title on the phone puts it on the laptop's
//! list, and taking it off on one takes it off on all of them.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::{AppDirs, read_json, write_json};
use crate::error::Result;

/// The titles on the list, by key, with when each was put there in Unix
/// seconds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MyList {
    #[serde(default)]
    titles: BTreeMap<String, i64>,
}

impl MyList {
    pub fn contains(&self, key: &str) -> bool {
        self.titles.contains_key(key)
    }

    pub fn is_empty(&self) -> bool {
        self.titles.is_empty()
    }

    /// When `key` was put on the list, if it is on it.
    pub fn added_at(&self, key: &str) -> Option<i64> {
        self.titles.get(key).copied()
    }

    /// Put `key` on the list at `at`. One already there keeps its place.
    pub fn add(&mut self, key: &str, at: i64) {
        self.titles.entry(key.to_owned()).or_insert(at);
    }

    pub fn remove(&mut self, key: &str) {
        self.titles.remove(key);
    }

    /// Put `key` on the list or take it off, saying which.
    pub fn toggle(&mut self, key: &str, at: i64) -> bool {
        if self.contains(key) {
            self.remove(key);
            false
        } else {
            self.add(key, at);
            true
        }
    }

    /// Every key with when it was added, in key order.
    pub fn entries(&self) -> impl Iterator<Item = (&str, i64)> {
        self.titles.iter().map(|(key, at)| (key.as_str(), *at))
    }

    /// The keys, most recently added first: the order the row is in.
    pub fn newest_first(&self) -> Vec<&str> {
        let mut keys: Vec<(&str, i64)> = self.entries().collect();
        keys.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
        keys.into_iter().map(|(key, _)| key).collect()
    }

    /// Set `key`'s entry as another device had it: on the list since `at`,
    /// or, with `None`, off it.
    pub(crate) fn set(&mut self, key: &str, at: Option<i64>) {
        match at {
            Some(at) => {
                self.titles.insert(key.to_owned(), at);
            }
            None => self.remove(key),
        }
    }
}

pub(crate) fn file(dirs: &AppDirs) -> PathBuf {
    dirs.config.join("my-list.json")
}

/// The list, empty when there is no file yet.
pub fn load(dirs: &AppDirs) -> Result<MyList> {
    Ok(read_json::<MyList>(&file(dirs))?.unwrap_or_default())
}

pub fn save(dirs: &AppDirs, list: &MyList) -> Result<()> {
    write_json(&file(dirs), list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_added_twice_keeps_the_time_it_was_first_added() {
        let mut list = MyList::default();
        list.add("frieren", 10);
        list.add("frieren", 20);
        assert_eq!(list.added_at("frieren"), Some(10));
    }

    #[test]
    fn toggling_puts_a_title_on_the_list_and_takes_it_off_again() {
        let mut list = MyList::default();
        assert!(list.toggle("frieren", 10));
        assert!(list.contains("frieren"));
        assert!(!list.toggle("frieren", 20));
        assert!(list.is_empty());
    }

    #[test]
    fn the_row_shows_the_most_recently_added_title_first() {
        let mut list = MyList::default();
        list.add("a", 30);
        list.add("b", 10);
        list.add("c", 20);
        assert_eq!(list.newest_first(), ["a", "c", "b"]);
    }

    #[test]
    fn the_list_survives_a_save_and_a_load() {
        let root = std::env::temp_dir().join(format!("pstr-mylist-{}", std::process::id()));
        let dirs = AppDirs::from_paths(root.join("c"), root.join("d"), root.join("k")).unwrap();
        assert!(load(&dirs).unwrap().is_empty());
        let mut list = MyList::default();
        list.add("one piece", 5);
        save(&dirs, &list).unwrap();
        assert_eq!(load(&dirs).unwrap(), list);
        let _ = std::fs::remove_dir_all(&root);
    }
}
