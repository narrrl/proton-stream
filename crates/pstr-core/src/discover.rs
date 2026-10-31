//! Getting from one title to the next: "More like this", the "Because you
//! watched" row, and the pages a genre, studio or person chip opens.
//!
//! All of it is read off what the provider already said about each title —
//! genres, tags, studios, directors, format — so none of it costs a request,
//! and it only ever offers what the library holds.

use std::collections::{HashMap, HashSet};

use crate::library::Title;
use crate::metadata::{MetadataRecord, TitleMetadata, TitleNames};
use crate::shelves::Shelf;

/// What a chip on a title page stands for, and the page it opens.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Facet {
    Genre(String),
    Studio(String),
    /// A director, or a voice actor in the cast.
    Person(String),
}

impl Facet {
    /// The name on the chip, and the heading of its page.
    pub fn name(&self) -> &str {
        match self {
            Self::Genre(name) | Self::Studio(name) | Self::Person(name) => name,
        }
    }

    /// What kind of thing the name is, said over the heading.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Genre(_) => "Genre",
            Self::Studio(_) => "Studio",
            Self::Person(_) => "Person",
        }
    }

    /// Whether a title the provider described as `found` belongs on the page.
    pub fn admits(&self, found: &TitleMetadata) -> bool {
        let details = found.details.as_ref();
        match self {
            Self::Genre(name) => found.genres.iter().any(|genre| genre == name),
            Self::Studio(name) => {
                details.is_some_and(|details| details.studios.iter().any(|studio| studio == name))
            }
            Self::Person(name) => details.is_some_and(|details| {
                details.directors.iter().any(|person| person == name)
                    || details
                        .cast
                        .iter()
                        .any(|member| member.actor.as_deref() == Some(name.as_str()))
            }),
        }
    }

    /// The keys of the titles in `titles` the page lists, in library order.
    pub fn titles(
        &self,
        titles: &[Title],
        metadata: &HashMap<String, MetadataRecord>,
    ) -> Vec<String> {
        titles
            .iter()
            .filter(|title| {
                metadata
                    .get(&title.key)
                    .and_then(|record| record.metadata.as_ref())
                    .is_some_and(|found| self.admits(found))
            })
            .map(|title| title.key.clone())
            .collect()
    }
}

/// One episode a search found by the name the provider gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpisodeMatch {
    pub title_key: String,
    pub share_id: String,
    pub link_id: String,
    /// `S01E05`, as the row everywhere else says it.
    pub label: String,
    /// The provider's name for it.
    pub name: String,
}

/// What a search found besides titles: episodes by name, and the people,
/// studios and genres whose names hold the query, each with how many titles
/// in the library carry it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Found {
    pub episodes: Vec<EpisodeMatch>,
    /// Directors and studios.
    pub people: Vec<(Facet, usize)>,
    pub genres: Vec<(Facet, usize)>,
}

impl Found {
    pub fn is_empty(&self) -> bool {
        self.episodes.is_empty() && self.people.is_empty() && self.genres.is_empty()
    }
}

/// How many episodes a search lists: past this the query is too loose for a
/// list of episodes to help.
const EPISODES: usize = 12;

/// Everything but titles that `query` finds. Nothing for a query under two
/// letters, which would hold half of every name.
pub fn find(
    library: &crate::library::Library,
    metadata: &HashMap<String, MetadataRecord>,
    guides: &HashMap<String, crate::metadata::EpisodeGuide>,
    query: &str,
) -> Found {
    let needle = query.trim().to_lowercase();
    if needle.chars().count() < 2 {
        return Found::default();
    }
    let holds = |name: &str| name.to_lowercase().contains(&needle);

    let mut episodes = Vec::new();
    'titles: for title in &library.titles {
        let Some(guide) = guides.get(&title.key) else {
            continue;
        };
        for episode in title.episodes() {
            let node = &episode.node;
            let Some(name) = node
                .parsed
                .episode
                .and_then(|number| guide.get(node.parsed.season, number))
                .and_then(|found| found.name.as_deref())
                .filter(|name| holds(name))
            else {
                continue;
            };
            episodes.push(EpisodeMatch {
                title_key: title.key.clone(),
                share_id: node.share_id.clone(),
                link_id: node.link_id.clone(),
                label: episode.numbering().unwrap_or_else(|| episode.label()),
                name: name.to_owned(),
            });
            if episodes.len() == EPISODES {
                break 'titles;
            }
        }
    }

    let mut people: HashMap<Facet, usize> = HashMap::new();
    let mut genres: HashMap<Facet, usize> = HashMap::new();
    for title in &library.titles {
        let Some(found) = metadata
            .get(&title.key)
            .and_then(|record| record.metadata.as_ref())
        else {
            continue;
        };
        for genre in found.genres.iter().filter(|genre| holds(genre)) {
            *genres.entry(Facet::Genre(genre.clone())).or_default() += 1;
        }
        let Some(details) = &found.details else {
            continue;
        };
        // One count per title, however many characters a voice actor has
        // in it.
        let made: HashSet<Facet> = details
            .directors
            .iter()
            .chain(
                details
                    .cast
                    .iter()
                    .filter_map(|member| member.actor.as_ref()),
            )
            .map(|name| Facet::Person(name.clone()))
            .chain(
                details
                    .studios
                    .iter()
                    .map(|name| Facet::Studio(name.clone())),
            )
            .filter(|facet| holds(facet.name()))
            .collect();
        for facet in made {
            *people.entry(facet).or_default() += 1;
        }
    }
    // Most titles first, then by name, so the order does not change from one
    // keystroke to the next.
    let ranked = |found: HashMap<Facet, usize>| {
        let mut found: Vec<(Facet, usize)> = found.into_iter().collect();
        found.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name().cmp(b.0.name())));
        found
    };
    Found {
        episodes,
        people: ranked(people),
        genres: ranked(genres),
    }
}

/// How much one shared trait says two titles are alike. A director is close
/// to a signature; a genre is one of three or four on every entry.
const DIRECTOR: u32 = 4;
const STUDIO: u32 = 3;
const GENRE: u32 = 2;
const TAG: u32 = 1;
const FORMAT: u32 = 1;

/// How many of a title's tags count. A provider ranks them, and the tail of
/// the list ("Male Protagonist", "Primarily Teen Cast") is on half of
/// everything.
const TAGS: usize = 8;

/// The least score a title needs to be offered: one director, or a studio and
/// a tag, or two genres. One genre in common is not a recommendation.
const MIN_SCORE: u32 = 4;

/// What a title is, for comparing: only the parts that score.
struct Traits<'a> {
    genres: HashSet<&'a str>,
    tags: HashSet<&'a str>,
    studios: HashSet<&'a str>,
    directors: HashSet<&'a str>,
    format: Option<&'a str>,
    popularity: u32,
}

impl<'a> Traits<'a> {
    fn of(found: &'a TitleMetadata) -> Self {
        let details = found.details.as_ref();
        let strings = |list: Option<&'a Vec<String>>, take: usize| -> HashSet<&'a str> {
            list.into_iter()
                .flatten()
                .take(take)
                .map(String::as_str)
                .collect()
        };
        Self {
            genres: found.genres.iter().map(String::as_str).collect(),
            tags: strings(details.map(|details| &details.tags), TAGS),
            studios: strings(details.map(|details| &details.studios), usize::MAX),
            directors: strings(details.map(|details| &details.directors), usize::MAX),
            format: details.and_then(|details| details.format.as_deref()),
            popularity: details.and_then(|details| details.popularity).unwrap_or(0),
        }
    }

    fn score(&self, other: &Self) -> u32 {
        let shared = |a: &HashSet<&str>, b: &HashSet<&str>| a.intersection(b).count() as u32;
        let traits = DIRECTOR * shared(&self.directors, &other.directors)
            + STUDIO * shared(&self.studios, &other.studios)
            + GENRE * shared(&self.genres, &other.genres)
            + TAG * shared(&self.tags, &other.tags);
        // Only on top of something else: a TV series is like every other one.
        if traits > 0 && self.format.is_some() && self.format == other.format {
            traits + FORMAT
        } else {
            traits
        }
    }
}

/// Up to `limit` keys of the titles most like `key`, best first.
///
/// Never its own franchise — that is the row above it — and one title per
/// other franchise, its best scoring: three films of one story are one
/// suggestion, not three.
pub fn more_like(
    key: &str,
    titles: &[Title],
    metadata: &HashMap<String, MetadataRecord>,
    limit: usize,
) -> Vec<String> {
    ranked(key, titles, metadata, |_| true, limit)
}

/// The row seeded by the title most recently watched to the end: titles like
/// it the viewer has not started. `None` when nothing is finished, or nothing
/// is like it.
pub fn because_you_watched(
    titles: &[Title],
    metadata: &HashMap<String, MetadataRecord>,
    names: TitleNames,
    limit: usize,
) -> Option<Shelf> {
    let seed = titles
        .iter()
        .filter(|title| {
            title.episode_count() > 0
                && title.watched_count() == title.episode_count()
                && metadata
                    .get(&title.key)
                    .is_some_and(|record| record.metadata.is_some())
        })
        .max_by_key(|title| title.last_played())?;
    let keys = ranked(
        &seed.key,
        titles,
        metadata,
        |title| title.last_played() == 0,
        limit,
    );
    (!keys.is_empty()).then(|| Shelf {
        label: format!(
            "Because you watched {}",
            crate::browse::display_name(seed, metadata, names)
        ),
        keys,
    })
}

fn ranked(
    key: &str,
    titles: &[Title],
    metadata: &HashMap<String, MetadataRecord>,
    admit: impl Fn(&Title) -> bool,
    limit: usize,
) -> Vec<String> {
    let found = |title: &Title| {
        metadata
            .get(&title.key)
            .and_then(|record| record.metadata.as_ref())
    };
    let Some(seed) = titles
        .iter()
        .find(|title| title.key == key)
        .and_then(found)
        .map(Traits::of)
    else {
        return Vec::new();
    };
    let refs: Vec<&Title> = titles.iter().collect();
    let franchise_of: Vec<usize> = {
        let mut of = vec![0; titles.len()];
        for (at, franchise) in crate::franchise::group(&refs, metadata)
            .into_iter()
            .enumerate()
        {
            for member in franchise.members {
                of[member] = at;
            }
        }
        of
    };
    let own = titles
        .iter()
        .position(|title| title.key == key)
        .map(|at| franchise_of[at]);

    // The best of each franchise: (score, popularity, index).
    let mut best: HashMap<usize, (u32, u32, usize)> = HashMap::new();
    for (index, title) in titles.iter().enumerate() {
        if Some(franchise_of[index]) == own || !admit(title) {
            continue;
        }
        let Some(traits) = found(title).map(Traits::of) else {
            continue;
        };
        let score = seed.score(&traits);
        if score < MIN_SCORE {
            continue;
        }
        let candidate = (score, traits.popularity, index);
        best.entry(franchise_of[index])
            .and_modify(|held| {
                if (candidate.0, candidate.1) > (held.0, held.1) {
                    *held = candidate;
                }
            })
            .or_insert(candidate);
    }
    let mut picked: Vec<(u32, u32, usize)> = best.into_values().collect();
    // Best score, then most followed, then library order for a stable row.
    picked.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    picked
        .into_iter()
        .take(limit)
        .map(|(_, _, index)| titles[index].key.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{CatalogNode, WatchState};
    use crate::library::{Library, TitleKind};
    use crate::metadata::{ProviderId, TitleDetails};

    struct Entry {
        key: &'static str,
        genres: &'static [&'static str],
        studio: &'static str,
        director: &'static str,
        related: &'static [&'static str],
    }

    fn record(entry: &Entry) -> (String, MetadataRecord) {
        (
            entry.key.into(),
            MetadataRecord {
                title_key: entry.key.into(),
                provider: ProviderId::AniList,
                metadata: Some(TitleMetadata {
                    provider: ProviderId::AniList,
                    remote_id: entry.key.into(),
                    name: entry.key.into(),
                    original_name: None,
                    overview: None,
                    year: None,
                    kind: TitleKind::Series,
                    poster_url: None,
                    backdrop_url: None,
                    logo_url: None,
                    rating: None,
                    genres: entry.genres.iter().map(|genre| genre.to_string()).collect(),
                    episodes: None,
                    url: None,
                    details: Some(TitleDetails {
                        studios: vec![entry.studio.into()],
                        directors: vec![entry.director.into()],
                        related: entry.related.iter().map(|id| id.to_string()).collect(),
                        format: Some("TV".into()),
                        ..TitleDetails::default()
                    }),
                }),
                fetched_at: 0,
                manual: false,
            },
        )
    }

    fn file(key: &str) -> CatalogNode {
        let name = format!("{key} - 01.mkv");
        CatalogNode {
            share_id: "s".into(),
            link_id: key.into(),
            volume_id: "v".into(),
            parent_link_id: None,
            name: name.clone(),
            is_folder: false,
            media_type: Some("video/x-matroska".into()),
            size: Some(1),
            active_revision_id: Some("r".into()),
            parsed: crate::naming::parse(&name),
        }
    }

    const ENTRIES: [Entry; 5] = [
        Entry {
            key: "frieren",
            genres: &["Adventure", "Drama", "Fantasy"],
            studio: "Madhouse",
            director: "Keiichirou Saitou",
            related: &[],
        },
        Entry {
            key: "frieren film",
            genres: &["Adventure", "Drama", "Fantasy"],
            studio: "Madhouse",
            director: "Keiichirou Saitou",
            related: &["frieren"],
        },
        Entry {
            key: "bocchi",
            genres: &["Comedy", "Music"],
            studio: "CloverWorks",
            director: "Keiichirou Saitou",
            related: &[],
        },
        Entry {
            key: "mushishi",
            genres: &["Adventure", "Drama", "Fantasy", "Mystery"],
            studio: "Artland",
            director: "Hiroshi Nagahama",
            related: &[],
        },
        Entry {
            key: "lucky star",
            genres: &["Comedy"],
            studio: "Kyoto Animation",
            director: "Yasuhiro Takemoto",
            related: &[],
        },
    ];

    fn library(watched: &[&str]) -> (Library, HashMap<String, MetadataRecord>) {
        let watch: HashMap<(String, String), WatchState> = watched
            .iter()
            .enumerate()
            .map(|(at, key)| {
                (
                    ("s".to_owned(), key.to_string()),
                    WatchState {
                        position_secs: 0.0,
                        duration_secs: Some(1_440.0),
                        watched: true,
                        updated_at: 100 + at as i64,
                    },
                )
            })
            .collect();
        let library = Library::build(
            ENTRIES.iter().map(|entry| file(entry.key)).collect(),
            &watch,
        );
        // The library's keys are the folder names, normalised; the fixture's
        // already are.
        (library, ENTRIES.iter().map(record).collect())
    }

    #[test]
    fn more_like_this_leaves_out_the_franchise_and_ranks_a_shared_director_and_genres() {
        let (library, metadata) = library(&[]);
        let similar = more_like("frieren", &library.titles, &metadata, 10);
        // Mushishi shares three genres, Bocchi only the director; both the
        // format.
        // Lucky Star shares nothing and its film is Frieren's own franchise.
        assert_eq!(similar, ["mushishi", "bocchi"]);
    }

    #[test]
    fn one_genre_in_common_is_not_a_recommendation() {
        let (library, metadata) = library(&[]);
        assert!(more_like("lucky star", &library.titles, &metadata, 10).is_empty());
    }

    #[test]
    fn because_you_watched_follows_the_last_finished_title_and_skips_what_was_started() {
        let (library, metadata) = library(&["mushishi", "frieren"]);
        let row =
            because_you_watched(&library.titles, &metadata, TitleNames::default(), 10).unwrap();
        assert_eq!(row.label, "Because you watched frieren");
        // Mushishi is like it, but already watched.
        assert_eq!(row.keys, ["bocchi"]);
        assert!(
            because_you_watched(&library.titles, &HashMap::new(), TitleNames::default(), 10)
                .is_none()
        );
    }

    #[test]
    fn a_search_finds_episodes_by_name_and_the_people_and_genres_it_names() {
        let (library, metadata) = library(&[]);
        let guide = crate::metadata::EpisodeGuide::new(vec![crate::metadata::EpisodeMetadata {
            season: None,
            number: 1,
            absolute: None,
            entry_season: None,
            entry_number: None,
            name: Some("The Journey's End".into()),
            overview: None,
            still_url: None,
            air_date: None,
            runtime: None,
        }]);
        let guides = HashMap::from([("frieren".to_owned(), guide)]);

        let found = find(&library, &metadata, &guides, "journey");
        assert_eq!(found.episodes.len(), 1);
        assert_eq!(found.episodes[0].title_key, "frieren");
        assert_eq!(found.episodes[0].name, "The Journey's End");

        let found = find(&library, &metadata, &guides, "saitou");
        assert_eq!(
            found.people,
            [(Facet::Person("Keiichirou Saitou".into()), 3)]
        );
        let found = find(&library, &metadata, &guides, "com");
        assert_eq!(found.genres, [(Facet::Genre("Comedy".into()), 2)]);
        assert!(find(&library, &metadata, &guides, "c").is_empty());
    }

    #[test]
    fn a_facet_page_lists_the_titles_that_carry_it() {
        let (library, metadata) = library(&[]);
        let person = Facet::Person("Keiichirou Saitou".into());
        assert_eq!(
            person.titles(&library.titles, &metadata).len(),
            3,
            "both Frieren titles and Bocchi"
        );
        assert_eq!(person.kind(), "Person");
        let genre = Facet::Genre("Comedy".into());
        let mut comedy = genre.titles(&library.titles, &metadata);
        comedy.sort();
        assert_eq!(comedy, ["bocchi", "lucky star"]);
        assert!(
            Facet::Studio("MAPPA".into())
                .titles(&library.titles, &metadata)
                .is_empty()
        );
    }
}
