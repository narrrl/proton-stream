//! Which titles in a library are one story.
//!
//! A share files a franchise however its uploader did: Made in Abyss as a
//! series folder and three film folders, Evangelion as a series, two films and
//! a folder of four more. The provider knows they belong together — AniList
//! records prequels, sequels, side stories, recaps and retellings as relations
//! — so the library can show one tile for the lot and the parts on its page.
//!
//! Grouping is by shared provider id, not by name. Two titles are one
//! franchise when one names the other as related, or when both name the same
//! entry: Mushishi's *Suzu no Shizuku* film relates to *Zoku Shou 2*, which the
//! library does not hold as a title of its own but which the series' episode
//! walk passed through (see [`crate::metadata::TitleDetails::chain`]).

use std::collections::HashMap;

use crate::library::{Title, TitleKind};
use crate::metadata::MetadataRecord;

/// One franchise: indices into the titles it was built from, in release order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Franchise {
    pub members: Vec<usize>,
}

impl Franchise {
    /// The member that stands for the rest: the first series to air, or the
    /// first film when there is no series. What a viewer means by "Made in
    /// Abyss" is the show, not the recap film that came out after it.
    pub fn lead(&self, titles: &[&Title]) -> usize {
        self.members
            .iter()
            .copied()
            .find(|&index| titles[index].kind == TitleKind::Series)
            .unwrap_or(self.members[0])
    }
}

/// Group `titles` into franchises, every title in exactly one.
///
/// Titles with no match are franchises of one. The groups come back in the
/// order of their first member in `titles`, so a sorted list stays sorted.
pub fn group(titles: &[&Title], metadata: &HashMap<String, MetadataRecord>) -> Vec<Franchise> {
    let mut sets = DisjointSets::new(titles.len());
    // The first title to name each provider id, for later titles naming it too.
    let mut named: HashMap<(crate::metadata::ProviderId, &str), usize> = HashMap::new();
    for (index, title) in titles.iter().enumerate() {
        let Some(found) = metadata
            .get(&title.key)
            .and_then(|record| record.metadata.as_ref())
        else {
            continue;
        };
        let details = found.details.as_ref();
        let ids = std::iter::once(found.remote_id.as_str())
            .chain(details.into_iter().flat_map(|details| {
                details
                    .related
                    .iter()
                    .chain(&details.chain)
                    .map(String::as_str)
            }))
            .filter(|id| !id.is_empty());
        for id in ids {
            match named.get(&(found.provider, id)) {
                Some(&other) => sets.union(index, other),
                None => {
                    named.insert((found.provider, id), index);
                }
            }
        }
    }

    let mut groups: Vec<Franchise> = Vec::new();
    let mut slot: HashMap<usize, usize> = HashMap::new();
    for index in 0..titles.len() {
        let root = sets.find(index);
        match slot.get(&root) {
            Some(&at) => groups[at].members.push(index),
            None => {
                slot.insert(root, groups.len());
                groups.push(Franchise {
                    members: vec![index],
                });
            }
        }
    }
    for franchise in &mut groups {
        franchise
            .members
            .sort_by_cached_key(|&index| release_key(titles[index], metadata));
    }
    groups
}

/// What release order sorts on: the provider's start date, else the year the
/// files state, else last.
pub fn release_key(title: &Title, metadata: &HashMap<String, MetadataRecord>) -> String {
    let found = metadata
        .get(&title.key)
        .and_then(|record| record.metadata.as_ref());
    found
        .and_then(|found| found.details.as_ref())
        .and_then(|details| details.start_date.clone())
        .or_else(|| {
            found
                .and_then(|found| found.year)
                .map(|year| year.to_string())
        })
        .or_else(|| title.year.map(|year| year.to_string()))
        // `~` sorts after every digit: undated goes last.
        .unwrap_or_else(|| "~".to_string())
}

struct DisjointSets {
    parent: Vec<usize>,
}

impl DisjointSets {
    fn new(size: usize) -> Self {
        Self {
            parent: (0..size).collect(),
        }
    }

    fn find(&mut self, mut index: usize) -> usize {
        while self.parent[index] != index {
            self.parent[index] = self.parent[self.parent[index]];
            index = self.parent[index];
        }
        index
    }

    fn union(&mut self, left: usize, right: usize) {
        let (left, right) = (self.find(left), self.find(right));
        if left != right {
            // The earlier title stays the root, which keeps `group` stable.
            let (root, child) = (left.min(right), left.max(right));
            self.parent[child] = root;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{ProviderId, TitleDetails, TitleMetadata};

    fn title(key: &str, kind: TitleKind) -> Title {
        Title {
            key: key.into(),
            name: key.into(),
            year: None,
            kind,
            seasons: Vec::new(),
            share_ids: Vec::new(),
        }
    }

    fn matched(
        key: &str,
        id: &str,
        start: &str,
        related: &[&str],
        chain: &[&str],
    ) -> MetadataRecord {
        MetadataRecord {
            title_key: key.into(),
            provider: ProviderId::AniList,
            metadata: Some(TitleMetadata {
                provider: ProviderId::AniList,
                remote_id: id.into(),
                name: key.into(),
                original_name: None,
                overview: None,
                year: None,
                kind: TitleKind::Series,
                poster_url: None,
                backdrop_url: None,
                rating: None,
                genres: Vec::new(),
                episodes: None,
                url: None,
                details: Some(TitleDetails {
                    start_date: Some(start.into()),
                    related: related.iter().map(|id| id.to_string()).collect(),
                    chain: chain.iter().map(|id| id.to_string()).collect(),
                    ..TitleDetails::default()
                }),
            }),
            fetched_at: 0,
            manual: false,
        }
    }

    fn keys(groups: &[Franchise], titles: &[&Title]) -> Vec<Vec<String>> {
        groups
            .iter()
            .map(|group| {
                group
                    .members
                    .iter()
                    .map(|&index| titles[index].key.clone())
                    .collect()
            })
            .collect()
    }

    /// Made in Abyss: the films name the series, the series names the films.
    /// Paprika names nothing in the library and stays alone.
    #[test]
    fn titles_that_name_each_other_are_one_franchise_in_release_order() {
        let owned = [
            title("dawn of the deep soul", TitleKind::Film),
            title("made in abyss", TitleKind::Series),
            title("paprika", TitleKind::Film),
        ];
        let titles: Vec<&Title> = owned.iter().collect();
        let metadata = HashMap::from([
            (
                "dawn of the deep soul".to_string(),
                matched(
                    "dawn of the deep soul",
                    "100643",
                    "2020-01-17",
                    &["97986"],
                    &[],
                ),
            ),
            (
                "made in abyss".to_string(),
                matched("made in abyss", "97986", "2017-07-07", &["100643"], &[]),
            ),
            (
                "paprika".to_string(),
                matched("paprika", "1943", "2006-11-25", &[], &[]),
            ),
        ]);

        let groups = group(&titles, &metadata);
        assert_eq!(
            keys(&groups, &titles),
            vec![
                vec!["made in abyss", "dawn of the deep soul"],
                vec!["paprika"]
            ]
        );
        assert_eq!(titles[groups[0].lead(&titles)].key, "made in abyss");
    }

    /// Mushishi: the special names an entry the series only reached by
    /// walking its sequels for episodes. That is still the same story.
    #[test]
    fn an_entry_the_series_walked_through_joins_what_names_it() {
        let owned = [
            title("mushishi", TitleKind::Series),
            title("suzu no shizuku", TitleKind::Film),
        ];
        let titles: Vec<&Title> = owned.iter().collect();
        let metadata = HashMap::from([
            (
                "mushishi".to_string(),
                matched(
                    "mushishi",
                    "457",
                    "2005-10-23",
                    &["20526"],
                    &["457", "20595", "20751"],
                ),
            ),
            (
                "suzu no shizuku".to_string(),
                matched("suzu no shizuku", "21008", "2015-05-16", &["20751"], &[]),
            ),
        ]);
        assert_eq!(group(&titles, &metadata).len(), 1);
    }

    /// Nothing matched, nothing to group by — and two unmatched titles are
    /// not one franchise because they share an empty id.
    #[test]
    fn unmatched_titles_stand_alone() {
        let owned = [
            title("game of thrones", TitleKind::Series),
            title("something else", TitleKind::Series),
        ];
        let titles: Vec<&Title> = owned.iter().collect();
        assert_eq!(group(&titles, &HashMap::new()).len(), 2);
    }
}
