//! Rows of titles that share something, for the top of the library.
//!
//! A grid sorted one way answers one question. Shelves answer the ones a
//! viewer did not think to ask: what is still airing, what else that studio
//! made, the director whose four films happen to be in the share. They are
//! picked from what the provider said, and only where there is enough of
//! something to be worth a row — three titles from one studio is a shelf, one
//! is a fact on its page.

use std::collections::{BTreeMap, HashMap};

use crate::library::Title;
use crate::metadata::MetadataRecord;

/// How many franchises a person or studio needs before it earns a row.
///
/// Franchises, not titles: Made in Abyss and its three films are four titles
/// with one director, and a "Directed by" row holding nothing but them says
/// what the franchise's own tile already does.
const MIN_TITLES: usize = 3;

/// A genre on more than this share of the matched titles says nothing: an
/// "Action" row in an anime library is half the library again — which is what
/// it was, at 0.6, in the library this was tuned on.
const MAX_GENRE_SHARE: f32 = 0.4;

/// One row: a heading, and the keys of the titles under it, in library order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shelf {
    pub label: String,
    pub keys: Vec<String>,
}

/// Up to `limit` shelves for `titles`, most specific first: what is airing,
/// then directors, studios and genres, each by how many titles it holds.
///
/// Two shelves holding exactly the same titles are one shelf — the first,
/// which is the more specific.
pub fn pick(
    titles: &[Title],
    metadata: &HashMap<String, MetadataRecord>,
    limit: usize,
) -> Vec<Shelf> {
    let mut airing = Vec::new();
    let mut directors: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut studios: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut genres: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut matched = 0usize;
    let refs: Vec<&Title> = titles.iter().collect();
    let franchise_of: HashMap<&str, usize> = crate::franchise::group(&refs, metadata)
        .into_iter()
        .enumerate()
        .flat_map(|(at, franchise)| {
            franchise
                .members
                .into_iter()
                .map(move |member| (titles[member].key.as_str(), at))
        })
        .collect();
    let franchises = |keys: &[String]| {
        keys.iter()
            .filter_map(|key| franchise_of.get(key.as_str()))
            .collect::<std::collections::HashSet<_>>()
            .len()
    };

    for title in titles {
        let Some(found) = metadata
            .get(&title.key)
            .and_then(|record| record.metadata.as_ref())
        else {
            continue;
        };
        matched += 1;
        for genre in &found.genres {
            genres.entry(genre).or_default().push(title.key.clone());
        }
        let Some(details) = &found.details else {
            continue;
        };
        if details.is_airing() {
            airing.push(title.key.clone());
        }
        for director in &details.directors {
            directors
                .entry(director)
                .or_default()
                .push(title.key.clone());
        }
        for studio in &details.studios {
            studios.entry(studio).or_default().push(title.key.clone());
        }
    }

    let ranked = |groups: BTreeMap<&str, Vec<String>>, label: &dyn Fn(&str) -> String| {
        let mut rows: Vec<Shelf> = groups
            .into_iter()
            .filter(|(_, keys)| franchises(keys) >= MIN_TITLES)
            .map(|(name, keys)| Shelf {
                label: label(name),
                keys,
            })
            .collect();
        // Stable, so equal counts keep their alphabetical order.
        rows.sort_by_key(|shelf| std::cmp::Reverse(shelf.keys.len()));
        rows
    };

    let genre_ceiling = (matched as f32 * MAX_GENRE_SHARE).floor() as usize;
    let genres: BTreeMap<&str, Vec<String>> = genres
        .into_iter()
        .filter(|(_, keys)| keys.len() <= genre_ceiling)
        .collect();

    let mut shelves = Vec::new();
    if !airing.is_empty() {
        shelves.push(Shelf {
            label: "Airing now".into(),
            keys: airing,
        });
    }
    shelves.extend(ranked(directors, &|name| format!("Directed by {name}")));
    shelves.extend(ranked(studios, &|name| format!("From {name}")));
    shelves.extend(ranked(genres, &|name| name.to_string()));

    let mut picked: Vec<Shelf> = Vec::new();
    for shelf in shelves {
        if picked.len() == limit {
            break;
        }
        if !picked.iter().any(|other| other.keys == shelf.keys) {
            picked.push(shelf);
        }
    }
    picked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::TitleKind;
    use crate::metadata::{ProviderId, TitleDetails, TitleMetadata};

    fn title(key: &str) -> Title {
        Title {
            key: key.into(),
            name: key.into(),
            year: None,
            kind: TitleKind::Film,
            seasons: Vec::new(),
            share_ids: Vec::new(),
        }
    }

    fn record(key: &str, genres: &[&str], details: TitleDetails) -> (String, MetadataRecord) {
        (
            key.to_string(),
            MetadataRecord {
                title_key: key.into(),
                provider: ProviderId::AniList,
                metadata: Some(TitleMetadata {
                    provider: ProviderId::AniList,
                    remote_id: key.into(),
                    name: key.into(),
                    original_name: None,
                    overview: None,
                    year: None,
                    kind: TitleKind::Film,
                    poster_url: None,
                    backdrop_url: None,
                    rating: None,
                    genres: genres.iter().map(|genre| genre.to_string()).collect(),
                    episodes: None,
                    url: None,
                    details: Some(details),
                }),
                fetched_at: 0,
                manual: false,
            },
        )
    }

    fn kon(studio: &str) -> TitleDetails {
        TitleDetails {
            directors: vec!["Satoshi Kon".into()],
            studios: vec![studio.into()],
            ..TitleDetails::default()
        }
    }

    /// The case this exists for: four Satoshi Kon films in one share.
    #[test]
    fn a_director_with_enough_titles_gets_a_shelf_ahead_of_a_studio() {
        let titles: Vec<Title> = [
            "perfect blue",
            "millennium actress",
            "tokyo godfathers",
            "paprika",
        ]
        .map(title)
        .to_vec();
        let metadata = HashMap::from([
            record("perfect blue", &["Drama"], kon("Madhouse")),
            record("millennium actress", &["Drama"], kon("Madhouse")),
            record("tokyo godfathers", &["Drama"], kon("Madhouse")),
            record("paprika", &["Sci-Fi"], kon("Madhouse")),
        ]);
        let shelves = pick(&titles, &metadata, 5);
        assert_eq!(shelves[0].label, "Directed by Satoshi Kon");
        assert_eq!(shelves[0].keys.len(), 4);
        // Madhouse holds exactly the same four, so it is not a second row.
        assert!(!shelves.iter().any(|shelf| shelf.label == "From Madhouse"));
        // Drama is three of four: past the share a genre may cover.
        assert!(!shelves.iter().any(|shelf| shelf.label == "Drama"));
    }

    /// Made in Abyss: four titles, one director, one story. Not a shelf.
    #[test]
    fn one_franchise_is_not_a_shelf_however_many_titles_it_has() {
        let titles: Vec<Title> = ["abyss", "film one", "film two", "film three"]
            .map(title)
            .to_vec();
        let kojima = |related: &[&str]| TitleDetails {
            directors: vec!["Masayuki Kojima".into()],
            related: related.iter().map(|id| id.to_string()).collect(),
            ..TitleDetails::default()
        };
        let metadata = HashMap::from([
            record(
                "abyss",
                &[],
                kojima(&["film one", "film two", "film three"]),
            ),
            record("film one", &[], kojima(&["abyss"])),
            record("film two", &[], kojima(&["abyss"])),
            record("film three", &[], kojima(&["abyss"])),
        ]);
        assert!(pick(&titles, &metadata, 5).is_empty());
    }

    #[test]
    fn what_is_airing_comes_first_and_too_few_titles_make_no_shelf() {
        let titles: Vec<Title> = ["a", "b"].map(title).to_vec();
        let metadata = HashMap::from([
            record(
                "a",
                &[],
                TitleDetails {
                    status: Some("RELEASING".into()),
                    studios: vec!["MAPPA".into()],
                    ..TitleDetails::default()
                },
            ),
            record("b", &[], TitleDetails::default()),
        ]);
        let shelves = pick(&titles, &metadata, 5);
        assert_eq!(shelves.len(), 1);
        assert_eq!(shelves[0].label, "Airing now");
        assert_eq!(shelves[0].keys, vec!["a"]);
    }
}
