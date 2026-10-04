//! Finding and ordering titles: what both library grids show, and in what
//! order.
//!
//! Here rather than in a front end so the desktop and Android agree: the same
//! search finds the same titles, a franchise folds into the same tile, and
//! "Newest" means the same thing on both.

use std::collections::{HashMap, HashSet};

use crate::library::{Library, Title, sort_name};
use crate::metadata::{MetadataRecord, TitleNames};

/// What order a grid is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    /// By the name shown, ignoring a leading article.
    #[default]
    Name,
    /// Most recently played first.
    Recent,
    /// Most recently given a new file first.
    Added,
    /// Newest first by when it aired, undated last.
    Release,
    /// Best rated first, unrated last.
    Rating,
    /// Most followed at the provider first.
    Popularity,
}

impl Sort {
    pub const ALL: [Self; 6] = [
        Self::Name,
        Self::Recent,
        Self::Added,
        Self::Release,
        Self::Rating,
        Self::Popularity,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "A – Z",
            Self::Recent => "Recently watched",
            Self::Added => "Recently added",
            Self::Release => "Newest",
            Self::Rating => "Highest rated",
            Self::Popularity => "Most popular",
        }
    }
}

/// The name to show for `title`: the provider's, in the form the viewer
/// picked, else the share's.
pub fn display_name<'a>(
    title: &'a Title,
    metadata: &'a HashMap<String, MetadataRecord>,
    names: TitleNames,
) -> &'a str {
    metadata
        .get(&title.key)
        .and_then(|record| record.metadata.as_ref())
        .and_then(|found| found.display_name(names))
        .unwrap_or(&title.name)
}

/// The titles `query` finds, in library order.
///
/// By the share's name and every filename under it — a release group, an
/// episode name — and by every name the provider gave: a folder called
/// `Sousou no Frieren` is found by typing "Frieren: Beyond".
pub fn search<'a>(
    library: &'a Library,
    metadata: &HashMap<String, MetadataRecord>,
    query: &str,
) -> Vec<&'a Title> {
    let needle = query.trim().to_lowercase();
    let by_files: HashSet<*const Title> = library
        .search(query)
        .into_iter()
        .map(|title| title as *const Title)
        .collect();
    library
        .titles
        .iter()
        .filter(|title| {
            by_files.contains(&(*title as *const Title))
                || metadata
                    .get(&title.key)
                    .and_then(|record| record.metadata.as_ref())
                    .is_some_and(|found| {
                        std::iter::once(found.name.as_str())
                            .chain(found.original_name.as_deref())
                            .chain(
                                found
                                    .details
                                    .as_ref()
                                    .and_then(|details| details.romaji.as_deref()),
                            )
                            .any(|name| name.to_lowercase().contains(&needle))
                    })
        })
        .collect()
}

/// How to order a grid.
#[derive(Debug, Clone, Copy)]
pub struct Order<'a> {
    pub sort: Sort,
    /// One tile per franchise rather than one per title.
    pub grouped: bool,
    pub names: TitleNames,
    pub metadata: &'a HashMap<String, MetadataRecord>,
    /// When each title last gained a file, by key — see
    /// [`Library::added_at`].
    pub added: &'a HashMap<String, i64>,
}

/// One tile: the title drawn, and every title it stands for — itself
/// included, in release order — as indices into what was arranged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tile {
    pub lead: usize,
    pub members: Vec<usize>,
}

/// `titles` as tiles, in `order`.
///
/// Every sort is stable over the name order, so ties — and the undated,
/// unrated and unplayed — keep their alphabetical order.
pub fn arrange<'t>(titles: &[&'t Title], order: Order<'_>) -> Vec<Tile> {
    let metadata = order.metadata;
    let found = |title: &Title| {
        metadata
            .get(&title.key)
            .and_then(|record| record.metadata.as_ref())
    };

    let mut by_name: Vec<usize> = (0..titles.len()).collect();
    by_name
        .sort_by_cached_key(|&index| sort_name(display_name(titles[index], metadata, order.names)));
    let sorted: Vec<&Title> = by_name.iter().map(|&index| titles[index]).collect();

    let mut tiles: Vec<Tile> = if order.grouped {
        crate::franchise::group(&sorted, metadata)
            .into_iter()
            .map(|franchise| Tile {
                lead: by_name[franchise.lead(&sorted)],
                members: franchise
                    .members
                    .iter()
                    .map(|&member| by_name[member])
                    .collect(),
            })
            .collect()
    } else {
        by_name
            .iter()
            .map(|&index| Tile {
                lead: index,
                members: vec![index],
            })
            .collect()
    };

    // A folded franchise files under the name on its tile — the lead's — and
    // not under whichever member sorted first: "Made in Abyss" belongs at M
    // even when its film is called "Dawn of the Deep Soul".
    tiles.sort_by_cached_key(|tile| {
        sort_name(display_name(titles[tile.lead], metadata, order.names))
    });

    let members = |tile: &Tile| -> Vec<&'t Title> {
        tile.members.iter().map(|&index| titles[index]).collect()
    };
    match order.sort {
        Sort::Name => {}
        Sort::Recent => tiles.sort_by_key(|tile| {
            std::cmp::Reverse(members(tile).iter().map(|title| title.last_played()).max())
        }),
        Sort::Added => tiles.sort_by_key(|tile| {
            std::cmp::Reverse(
                members(tile)
                    .iter()
                    .filter_map(|title| order.added.get(&title.key))
                    .max()
                    .copied(),
            )
        }),
        Sort::Release => tiles.sort_by_cached_key(|tile| {
            std::cmp::Reverse(
                members(tile)
                    .iter()
                    .map(|title| crate::franchise::release_key(title, metadata))
                    // `~` is how the key says "undated"; it sorts last here
                    // rather than first.
                    .filter(|key| key != "~")
                    .max(),
            )
        }),
        Sort::Rating => tiles.sort_by_key(|tile| {
            std::cmp::Reverse(
                found(titles[tile.lead])
                    .and_then(|found| found.rating)
                    .map(|rating| (rating * 100.0) as i64),
            )
        }),
        Sort::Popularity => tiles.sort_by_key(|tile| {
            std::cmp::Reverse(
                found(titles[tile.lead])
                    .and_then(|found| found.details.as_ref())
                    .and_then(|details| details.popularity),
            )
        }),
    }
    tiles
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::TitleKind;
    use crate::metadata::{ProviderId, TitleDetails, TitleMetadata};

    fn title(key: &str, name: &str, kind: TitleKind) -> Title {
        Title {
            key: key.into(),
            name: name.into(),
            year: None,
            kind,
            seasons: Vec::new(),
            share_ids: Vec::new(),
        }
    }

    fn matched(
        key: &str,
        id: &str,
        english: &str,
        start: &str,
        rating: f32,
        related: &[&str],
    ) -> (String, MetadataRecord) {
        (
            key.into(),
            MetadataRecord {
                title_key: key.into(),
                provider: ProviderId::AniList,
                metadata: Some(TitleMetadata {
                    provider: ProviderId::AniList,
                    remote_id: id.into(),
                    name: english.into(),
                    original_name: None,
                    overview: None,
                    year: None,
                    kind: TitleKind::Series,
                    poster_url: None,
                    backdrop_url: None,
                    rating: Some(rating),
                    genres: Vec::new(),
                    episodes: None,
                    url: None,
                    details: Some(TitleDetails {
                        romaji: Some(key.into()),
                        start_date: Some(start.into()),
                        related: related.iter().map(|id| id.to_string()).collect(),
                        ..TitleDetails::default()
                    }),
                }),
                fetched_at: 0,
                manual: false,
            },
        )
    }

    fn fixture() -> (Vec<Title>, HashMap<String, MetadataRecord>) {
        let titles = vec![
            title("abyss film", "Made in Abyss Dawn", TitleKind::Film),
            title("made in abyss", "Made in Abyss", TitleKind::Series),
            title("sousou no frieren", "Sousou no Frieren", TitleKind::Series),
        ];
        let metadata = HashMap::from([
            matched(
                "abyss film",
                "2",
                "Dawn of the Deep Soul",
                "2020-01-17",
                8.5,
                &["1"],
            ),
            matched(
                "made in abyss",
                "1",
                "Made in Abyss",
                "2017-07-07",
                8.4,
                &["2"],
            ),
            matched(
                "sousou no frieren",
                "3",
                "Frieren: Beyond Journey's End",
                "2023-09-29",
                9.1,
                &[],
            ),
        ]);
        (titles, metadata)
    }

    fn keys(tiles: &[Tile], titles: &[&Title]) -> Vec<String> {
        tiles
            .iter()
            .map(|tile| titles[tile.lead].key.clone())
            .collect()
    }

    #[test]
    fn a_franchise_folds_into_its_series_and_sorts_by_the_name_shown() {
        let (owned, metadata) = fixture();
        let titles: Vec<&Title> = owned.iter().collect();
        let added = HashMap::new();
        let order = Order {
            sort: Sort::Name,
            grouped: true,
            names: TitleNames::English,
            metadata: &metadata,
            added: &added,
        };
        let tiles = arrange(&titles, order);
        // "Frieren" before "Made in Abyss" in English; the film is folded.
        assert_eq!(
            keys(&tiles, &titles),
            vec!["sousou no frieren", "made in abyss"]
        );
        assert_eq!(tiles[1].members, vec![1, 0]);

        let tiles = arrange(
            &titles,
            Order {
                names: TitleNames::Library,
                grouped: false,
                ..order
            },
        );
        assert_eq!(
            keys(&tiles, &titles),
            vec!["made in abyss", "abyss film", "sousou no frieren"]
        );
    }

    #[test]
    fn newest_and_best_rated_read_the_provider_and_put_the_unknown_last() {
        let (mut owned, metadata) = fixture();
        owned.push(title(
            "game of thrones",
            "Game of Thrones",
            TitleKind::Series,
        ));
        let titles: Vec<&Title> = owned.iter().collect();
        let added = HashMap::new();
        let order = Order {
            sort: Sort::Release,
            grouped: true,
            names: TitleNames::Library,
            metadata: &metadata,
            added: &added,
        };
        // The Made in Abyss franchise dates from its newest part, 2020.
        assert_eq!(
            keys(&arrange(&titles, order), &titles),
            vec!["sousou no frieren", "made in abyss", "game of thrones"]
        );
        let order = Order {
            sort: Sort::Rating,
            ..order
        };
        assert_eq!(
            keys(&arrange(&titles, order), &titles)
                .last()
                .map(String::as_str),
            Some("game of thrones")
        );
    }

    #[test]
    fn search_finds_a_title_by_any_name_the_provider_gave_it() {
        let (owned, metadata) = fixture();
        let library = Library { titles: owned };
        let found = search(&library, &metadata, "beyond journey");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].key, "sousou no frieren");
        assert_eq!(search(&library, &metadata, "").len(), 3);
    }
}
