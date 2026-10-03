//! AniList, over its public GraphQL API.
//!
//! No API key and no account, which is why it is the default: turning
//! enrichment on costs the viewer a decision about privacy, and it should not
//! also cost them a signup. The trade is coverage — AniList is anime and nothing
//! else, so a library of films wants [`crate::tmdb`].
//!
//! One query shape is used for every search: a `Page` of `media` matching the
//! search string, with every name AniList knows the show by. The alias list is
//! the whole reason this works — see [`crate::matching`].
//!
//! AniList identifies a title and keeps nothing worth having about its
//! episodes, so enriching a match leans on [`crate::anizip`] for those, walking
//! AniList's own `SEQUEL` relations to find every entry the library's seasons
//! are spread across.

use std::collections::HashSet;

use pstr_core::library::{self, TitleKind};
use pstr_core::metadata::{
    Enrichment, EpisodeGuide, EpisodeMetadata, ProviderId, TitleDetails, TitleMetadata,
};
use serde::Deserialize;

use crate::anizip::AniZip;
use crate::error::{Error, Result};
use crate::limiter::RateLimiter;
use crate::matching::{Candidate, Query};
use crate::provider::Provider;

const ENDPOINT: &str = "https://graphql.anilist.co";

/// What AniList documents as its ceiling: ninety requests a minute.
///
/// A starting point, not a belief — the API has been running in a degraded mode
/// that allows thirty for a long time now, and says so in `X-RateLimit-Limit` on
/// every answer. The limiter retunes itself off that header after the first
/// request, so the only cost of starting optimistic is one `429` on a run that
/// begins during a degraded window.
const REQUESTS_PER_MINUTE: u32 = 90;

/// How many times to sit out a `429` before giving up on one request.
///
/// Two, because the pause it asks for is the rest of its rate-limit window and
/// the limiter holds every other lookup for the same stretch — a third round is
/// minutes of a scan spent waiting rather than matching. Giving up returns
/// [`Error::RateLimited`], which is a failure and therefore not cached: the
/// title stays askable and the next run picks it up.
const RETRIES: usize = 2;

/// How many answers to score. AniList's own relevance ordering is good, and the
/// right title is essentially always in the first handful; asking for more costs
/// them bandwidth and buys nothing.
const PAGE_SIZE: u32 = 8;

/// AniList scores out of 100 and the rest of the app works out of 10.
const SCORE_SCALE: f32 = 10.0;

/// Everything one entry is asked for, by search and by id alike.
///
/// The relations are the franchise's — see `pstr_core::franchise` — and the
/// staff are only the most relevant few, which is where a director sits.
macro_rules! entry_fragment {
    () => {
        r#"
fragment entry on Media {
  id
  title { romaji english native }
  synonyms
  description(asHtml: false)
  startDate { year month day }
  season
  coverImage { extraLarge large }
  bannerImage
  averageScore
  popularity
  genres
  tags { name rank isMediaSpoiler }
  episodes
  format
  status
  siteUrl
  studios(isMain: true) { nodes { name } }
  staff(sort: RELEVANCE, perPage: 6) { edges { role node { name { full } } } }
  nextAiringEpisode { episode airingAt }
  relations { edges { relationType node { id type } } }
}
"#
    };
}

const SEARCH: &str = concat!(
    r#"
query ($search: String, $perPage: Int) {
  Page(page: 1, perPage: $perPage) {
    media(search: $search, type: ANIME, sort: SEARCH_MATCH) { ...entry }
  }
}
"#,
    entry_fragment!()
);

/// One entry by the id a match already found — how a stored match gets the
/// fields it was stored without, without being searched for and decided
/// again.
const BY_ID: &str = concat!(
    r#"
query ($id: Int) {
  Media(id: $id, type: ANIME) { ...entry }
}
"#,
    entry_fragment!()
);

/// The relations that keep to one story. `SPIN_OFF`, `CHARACTER` and `OTHER`
/// are left out: they are how one franchise reaches into another, and
/// following them merged unrelated shows into one tile.
const STORY_RELATIONS: [&str; 8] = [
    "PREQUEL",
    "SEQUEL",
    "PARENT",
    "SIDE_STORY",
    "SUMMARY",
    "ALTERNATIVE",
    "COMPILATION",
    "CONTAINS",
];

/// A tag ranked below this is AniList's community being unsure of it.
const MIN_TAG_RANK: u32 = 60;

/// How many tags to keep: the rest are increasingly specific trivia.
const MAX_TAGS: usize = 8;

/// The entries an entry leads on to.
///
/// One hop at a time rather than a nested query several levels deep: most
/// titles need no hop at all, and the ones that do stop as soon as the
/// library's files are answered.
const RELATIONS: &str = r#"
query ($id: Int) {
  Media(id: $id, type: ANIME) {
    relations { edges { relationType node { id type format } } }
  }
}
"#;

/// How many entries of one franchise to read before giving up on files that
/// still have no answer.
///
/// Attack on Titan's four seasons are seven entries and Bleach's seventeen are
/// five, so twelve covers every long run in a real library while bounding what
/// a file no entry will ever answer — a special, a recap — can cost.
const MAX_CHAIN_ENTRIES: usize = 12;

pub struct AniList {
    http: reqwest::Client,
    /// Shared by every lookup in flight — see [`crate::limiter`].
    limiter: RateLimiter,
    anizip: AniZip,
}

impl AniList {
    /// `language` is the one episode names are wanted in, where ani.zip has
    /// them in it.
    pub fn new(http: reqwest::Client, language: String) -> Self {
        Self {
            anizip: AniZip::new(http.clone(), language),
            http,
            limiter: RateLimiter::per_minute(REQUESTS_PER_MINUTE),
        }
    }
}

impl Provider for AniList {
    fn id(&self) -> ProviderId {
        ProviderId::AniList
    }

    async fn search(&self, query: &Query) -> Result<Vec<Candidate>> {
        // Each term is asked only because the one before it came back with
        // nothing at all — see `search_terms`. A page of candidates, however
        // weak, is the matcher's business rather than this one's.
        for term in search_terms(&query.name) {
            let data: Option<SearchData> = self
                .query(serde_json::json!({
                    "query": SEARCH,
                    "variables": { "search": term, "perPage": PAGE_SIZE },
                }))
                .await?;

            let candidates: Vec<Candidate> = data
                .map(|data| data.page.media)
                .unwrap_or_default()
                .into_iter()
                .map(Media::into_candidate)
                .collect();
            if !candidates.is_empty() {
                return Ok(candidates);
            }
        }
        Ok(Vec::new())
    }

    /// The episodes of every entry the library's files are spread across, and
    /// the series' fanart.
    ///
    /// AniList files a sequel as its own entry — Bleach's seventeenth season is
    /// four of them, Mushoku Tensei's first is two — so a title's match is
    /// only where its episodes *start*. From there the `SEQUEL` relations are
    /// followed one entry at a time, each entry's episodes taken from ani.zip,
    /// until every numbered file in `title` has an answer, the franchise ends,
    /// or [`MAX_CHAIN_ENTRIES`] is reached. A one-season show is one ani.zip
    /// request and no relations query at all.
    ///
    /// Following relations rather than searching for `<title> 2nd Season` is
    /// the point: that search found the base entry again for Attack on Titan,
    /// whose seasons two to four were then captioned with season one's names.
    async fn enrich(&self, title: &library::Title, found: &TitleMetadata) -> Result<Enrichment> {
        // The id came from our own search response, so a non-numeric one is a
        // record written by some other version — not worth an error, and there
        // is nothing to ask about.
        let Ok(first) = found.remote_id.parse::<i64>() else {
            return Ok(Enrichment::default());
        };
        let wanted: Vec<(Option<u32>, u32)> = title
            .episodes()
            .filter_map(|episode| Some((episode.node.parsed.season, episode.node.parsed.episode?)))
            .collect();

        let mut enrichment = Enrichment::default();
        let mut taken = HashSet::new();
        let mut visited = HashSet::new();
        // The same ids as `visited`, in the order they were read.
        let mut chain = Vec::new();
        let mut series_entries = 0;
        let mut next = Some(Link {
            id: first,
            series: found.kind == TitleKind::Series,
        });
        while let Some(link) = next.take() {
            visited.insert(link.id);
            chain.push(link.id.to_string());
            let entry_season = link.series.then(|| {
                series_entries += 1;
                series_entries
            });
            let mapping = self.anizip.mapping(link.id, entry_season).await?;
            if enrichment.backdrop_url.is_none() {
                enrichment.backdrop_url = mapping.fanart;
            }
            // Where two entries both list an episode, the earlier one — the
            // one closer to the match — keeps it.
            enrichment.episodes.extend(
                mapping
                    .episodes
                    .into_iter()
                    .filter(|episode| taken.insert((episode.season, episode.number))),
            );

            if answers_all(&enrichment.episodes, &wanted) || visited.len() >= MAX_CHAIN_ENTRIES {
                break;
            }
            next = self.sequel(link.id, &visited).await?;
        }

        tracing::debug!(
            "{}: {} episodes over {} entries",
            found.name,
            enrichment.episodes.len(),
            visited.len()
        );
        enrichment.chain = chain;
        Ok(enrichment)
    }
}

impl AniList {
    /// The entry the AniList id `id` names, as search would have found it.
    /// `None` for an id AniList no longer has.
    pub async fn entry(&self, id: &str) -> Result<Option<TitleMetadata>> {
        let Ok(id) = id.parse::<i64>() else {
            return Ok(None);
        };
        let data: Option<EntryData> = self
            .query(serde_json::json!({
                "query": BY_ID,
                "variables": { "id": id },
            }))
            .await?;
        Ok(data
            .and_then(|data| data.media)
            .map(|media| media.into_candidate().metadata))
    }

    /// The entry `id` continues into, if any.
    async fn sequel(&self, id: i64, visited: &HashSet<i64>) -> Result<Option<Link>> {
        let data: Option<RelationsData> = self
            .query(serde_json::json!({
                "query": RELATIONS,
                "variables": { "id": id },
            }))
            .await?;
        let edges = data
            .and_then(|data| data.media)
            .map(|media| media.relations.edges)
            .unwrap_or_default();
        Ok(next_in_chain(edges, visited))
    }

    /// One GraphQL requestimpl AniList {
    /// One GraphQL request, paced, with AniList's two ways of failing — an HTTP
    /// status and an `errors` array under a 200 — folded into one.
    ///
    /// A `429` is waited out rather than returned: a scan sends hundreds of
    /// these, and the request that trips the limit is not the one that was
    /// wrong. See [`crate::limiter`].
    async fn query<T: serde::de::DeserializeOwned>(
        &self,
        body: serde_json::Value,
    ) -> Result<Option<T>> {
        let mut attempts = 0usize;
        let response = loop {
            self.limiter.acquire().await;
            let response = self.http.post(ENDPOINT).json(&body).send().await?;
            let status = response.status();

            if status.as_u16() == 429 {
                // The pause holds every other lookup too, so the rest of the
                // run does not spend the window earning more 429s.
                let waited = self.limiter.throttled(response.headers()).await;
                attempts += 1;
                if attempts > RETRIES {
                    return Err(Error::RateLimited);
                }
                tracing::debug!("AniList is throttling; waiting {waited:?}");
                continue;
            }

            self.limiter.observe(response.headers()).await;
            if !status.is_success() {
                return Err(Error::Http(format!("AniList answered {status}")));
            }
            break response;
        };

        let payload: Response<T> = response.json().await?;
        if let Some(errors) = payload.errors
            && !errors.is_empty()
        {
            let message = errors
                .into_iter()
                .map(|error| error.message)
                .collect::<Vec<_>>()
                .join("; ");
            return Err(Error::Http(format!("AniList: {message}")));
        }
        Ok(payload.data)
    }
}

/// The search strings to try, in order, until one of them returns anything.
///
/// AniList does not do fuzzy search. Every word of the query has to be a word
/// its index actually holds — only the *last* one is matched as a prefix — and
/// all of them have to match, so a single word AniList has never seen empties
/// the whole result set. `ghost in the shel` finds the film; `cowbo bebop`
/// finds nothing.
///
/// That turns one filesystem habit into a total miss: a name written to disk
/// has had its apostrophes taken out, because plenty of tools and shares still
/// dislike them. `Fate/stay night [Heaven's Feel]` indexes the words `heaven`
/// and `s`, and the folder is called `Fate stay night Heavens Feel` — whose
/// `heavens` is not a word in the index and not a prefix of one either. AniList
/// answers with an empty page, the title is stored as a miss, and it stays
/// unmatched for as long as the record is fresh.
///
/// So the fallback puts the apostrophe back the only way that survives
/// tokenisation: by dropping the `s` it was holding on to. It is asked for only
/// after the name as written found nothing, which is what keeps it from
/// broadening a search that was already working.
fn search_terms(name: &str) -> Vec<String> {
    let mut terms = vec![name.to_string()];

    let depossessed = without_possessive_s(name);
    if depossessed != name {
        terms.push(depossessed);
    }
    terms
}

/// `Heavens Feel` → `Heaven Feel`.
///
/// Only words long enough to still mean something without it, and only where
/// the `s` follows a letter — a word that kept its apostrophe (`Heaven's`) is
/// already what the index holds, and cutting the `s` off it would leave the
/// apostrophe behind as its own token.
fn without_possessive_s(name: &str) -> String {
    name.split(' ')
        .map(|word| {
            let mut characters = word.chars().rev();
            let last = characters.next();
            let previous = characters.next();
            let long_enough = word.chars().count() >= 4;

            match (last, previous) {
                (Some('s' | 'S'), Some(previous)) if long_enough && previous.is_alphabetic() => {
                    &word[..word.len() - 1]
                }
                _ => word,
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Deserialize)]
struct Response<T> {
    data: Option<T>,
    errors: Option<Vec<GraphQlError>>,
}

#[derive(Deserialize)]
struct GraphQlError {
    message: String,
}

#[derive(Deserialize)]
struct SearchData {
    #[serde(rename = "Page")]
    page: Page,
}

/// One step along a franchise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Link {
    id: i64,
    /// Whether the entry is part of the series proper — TV, a short or a web
    /// series — rather than a film or a special, which no release counts as a
    /// season.
    series: bool,
}

/// Which of an entry's relations the franchise continues through.
///
/// A sequel that is itself a series wins over one that is a film or a
/// special, but a special is still followed when it is the only way on:
/// Mushishi's first series leads to its second only through an OVA.
fn next_in_chain(edges: Vec<RelationEdge>, visited: &HashSet<i64>) -> Option<Link> {
    edges
        .into_iter()
        .filter(|edge| edge.relation_type.as_deref() == Some("SEQUEL"))
        .filter_map(|edge| edge.node)
        // A sequel can be the manga it was adapted into; only anime has
        // episodes to number.
        .filter(|node| node.kind.as_deref() == Some("ANIME") && !visited.contains(&node.id))
        .map(|node| Link {
            id: node.id,
            series: is_series_format(node.format.as_deref()),
        })
        .min_by_key(|link| !link.series)
}

fn is_series_format(format: Option<&str>) -> bool {
    matches!(format, Some("TV" | "TV_SHORT" | "ONA"))
}

/// Whether every numbered file has an answer in `episodes`.
fn answers_all(episodes: &[EpisodeMetadata], wanted: &[(Option<u32>, u32)]) -> bool {
    let guide = EpisodeGuide::new(episodes.to_vec());
    wanted
        .iter()
        .all(|&(season, number)| guide.get(season, number).is_some())
}

#[derive(Deserialize)]
struct RelationsData {
    #[serde(rename = "Media")]
    media: Option<RelationsMedia>,
}

#[derive(Deserialize)]
struct RelationsMedia {
    relations: Relations,
}

#[derive(Deserialize)]
struct Relations {
    #[serde(default)]
    edges: Vec<RelationEdge>,
}

#[derive(Deserialize)]
struct RelationEdge {
    #[serde(rename = "relationType")]
    relation_type: Option<String>,
    node: Option<RelationNode>,
}

#[derive(Deserialize)]
struct RelationNode {
    id: i64,
    #[serde(rename = "type")]
    kind: Option<String>,
    format: Option<String>,
}

#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    media: Vec<Media>,
}

#[derive(Deserialize)]
struct Media {
    id: i64,
    title: Title,
    #[serde(default)]
    synonyms: Vec<String>,
    description: Option<String>,
    #[serde(rename = "startDate")]
    start_date: Option<FuzzyDate>,
    #[serde(rename = "coverImage")]
    cover_image: Option<CoverImage>,
    #[serde(rename = "bannerImage")]
    banner_image: Option<String>,
    #[serde(rename = "averageScore")]
    average_score: Option<f32>,
    popularity: Option<u32>,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    tags: Vec<Tag>,
    episodes: Option<u32>,
    format: Option<String>,
    season: Option<String>,
    status: Option<String>,
    #[serde(rename = "siteUrl")]
    site_url: Option<String>,
    studios: Option<Studios>,
    staff: Option<Staff>,
    #[serde(rename = "nextAiringEpisode")]
    next_airing_episode: Option<NextAiring>,
    relations: Option<Relations>,
}

#[derive(Deserialize)]
struct EntryData {
    #[serde(rename = "Media")]
    media: Option<Media>,
}

#[derive(Deserialize)]
struct Tag {
    name: String,
    rank: Option<u32>,
    #[serde(rename = "isMediaSpoiler", default)]
    spoiler: bool,
}

#[derive(Deserialize)]
struct Studios {
    #[serde(default)]
    nodes: Vec<Named>,
}

#[derive(Deserialize)]
struct Named {
    name: String,
}

#[derive(Deserialize)]
struct Staff {
    #[serde(default)]
    edges: Vec<StaffEdge>,
}

#[derive(Deserialize)]
struct StaffEdge {
    role: Option<String>,
    node: Option<Person>,
}

#[derive(Deserialize)]
struct Person {
    name: PersonName,
}

#[derive(Deserialize)]
struct PersonName {
    full: Option<String>,
}

#[derive(Deserialize)]
struct NextAiring {
    episode: Option<u32>,
    #[serde(rename = "airingAt")]
    airing_at: Option<i64>,
}

#[derive(Deserialize)]
struct Title {
    romaji: Option<String>,
    english: Option<String>,
    native: Option<String>,
}

#[derive(Deserialize)]
struct FuzzyDate {
    year: Option<u32>,
    month: Option<u32>,
    day: Option<u32>,
}

impl FuzzyDate {
    /// `2013-04-07`, `2013-04` or `2013` — as much as AniList knows.
    fn text(&self) -> Option<String> {
        let year = self.year?;
        Some(match (self.month, self.day) {
            (Some(month), Some(day)) => format!("{year:04}-{month:02}-{day:02}"),
            (Some(month), None) => format!("{year:04}-{month:02}"),
            _ => format!("{year:04}"),
        })
    }
}

#[derive(Deserialize)]
struct CoverImage {
    #[serde(rename = "extraLarge")]
    extra_large: Option<String>,
    large: Option<String>,
}

impl Media {
    fn into_candidate(self) -> Candidate {
        let aliases: Vec<String> = [
            self.title.english.clone(),
            self.title.romaji.clone(),
            self.title.native.clone(),
        ]
        .into_iter()
        .flatten()
        .chain(self.synonyms)
        .collect();

        // English first when there is one: it is what the viewer's library is
        // most likely named after, and romaji is the fallback AniList itself
        // uses.
        let name = self
            .title
            .english
            .clone()
            .or_else(|| self.title.romaji.clone())
            .or_else(|| self.title.native.clone())
            .unwrap_or_else(|| format!("AniList #{}", self.id));

        let original_name = self
            .title
            .native
            .clone()
            .or_else(|| self.title.romaji.clone())
            .filter(|original| *original != name);

        let cover = self
            .cover_image
            .and_then(|cover| cover.extra_large.or(cover.large));

        let details = TitleDetails {
            romaji: self.title.romaji.clone(),
            banner_url: self.banner_image,
            format: self.format.clone(),
            season: self.season,
            start_date: self.start_date.as_ref().and_then(FuzzyDate::text),
            studios: self
                .studios
                .map(|studios| {
                    studios
                        .nodes
                        .into_iter()
                        .map(|studio| studio.name)
                        .collect()
                })
                .unwrap_or_default(),
            directors: self
                .staff
                .map(|staff| {
                    staff
                        .edges
                        .into_iter()
                        // `Director`, and not `Assistant Director`,
                        // `Episode Director` or `Director (eps 3, 7)` — those
                        // are many people per show, and a shelf of one
                        // storyboard artist's episodes is not what a
                        // "directed by" row means.
                        .filter(|edge| edge.role.as_deref() == Some("Director"))
                        .filter_map(|edge| edge.node.and_then(|person| person.name.full))
                        .collect()
                })
                .unwrap_or_default(),
            tags: self
                .tags
                .into_iter()
                .filter(|tag| !tag.spoiler && tag.rank.unwrap_or(0) >= MIN_TAG_RANK)
                .take(MAX_TAGS)
                .map(|tag| tag.name)
                .collect(),
            popularity: self.popularity,
            status: self.status,
            next_episode: self
                .next_airing_episode
                .as_ref()
                .and_then(|next| next.episode),
            next_airing_at: self.next_airing_episode.and_then(|next| next.airing_at),
            related: self
                .relations
                .map(|relations| {
                    relations
                        .edges
                        .into_iter()
                        .filter(|edge| {
                            edge.relation_type
                                .as_deref()
                                .is_some_and(|kind| STORY_RELATIONS.contains(&kind))
                        })
                        .filter_map(|edge| edge.node)
                        .filter(|node| node.kind.as_deref() == Some("ANIME"))
                        .map(|node| node.id.to_string())
                        .collect()
                })
                .unwrap_or_default(),
            chain: Vec::new(),
        };

        Candidate {
            metadata: TitleMetadata {
                provider: ProviderId::AniList,
                remote_id: self.id.to_string(),
                name,
                original_name,
                overview: self.description.map(|text| strip_markup(&text)),
                year: self.start_date.and_then(|date| date.year),
                kind: kind_of(self.format.as_deref()),
                poster_url: cover,
                // Not `bannerImage`: that is a 4.75:1 strip, and drawn into a
                // 16:9 tile it is a blurred sliver of its own middle. The
                // backdrop is ani.zip's fanart, found when the match is
                // enriched.
                backdrop_url: None,
                rating: self.average_score.map(|score| score / SCORE_SCALE),
                genres: self.genres,
                episodes: self.episodes,
                url: self.site_url,
                details: Some(details),
            },
            aliases,
            // Null on anything AniList has not seen air yet, which is not the
            // same as "it is a film". See `Candidate::kind_known`.
            kind_known: self.format.is_some(),
        }
    }
}

/// AniList's formats, reduced to the two kinds a library has.
///
/// Anything episodic is a series; a film, a one-shot special or an unknown
/// format is not. An unknown format reading as a film costs a badge, and the
/// match itself only nudges on kind — see [`crate::matching::score`].
fn kind_of(format: Option<&str>) -> TitleKind {
    match format {
        Some("TV" | "TV_SHORT" | "ONA" | "OVA" | "SPECIAL") => TitleKind::Series,
        _ => TitleKind::Film,
    }
}

/// AniList descriptions carry a little HTML even with `asHtml: false` — `<br>`
/// mostly, and the occasional `<i>`. egui draws no markup, so tags become
/// nothing and `<br>` becomes the line break it stands for.
fn strip_markup(text: &str) -> String {
    let text = text
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n");

    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for character in text.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(character),
            _ => {}
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn episodic_formats_are_series_and_everything_else_is_not() {
        assert_eq!(kind_of(Some("TV")), TitleKind::Series);
        assert_eq!(kind_of(Some("OVA")), TitleKind::Series);
        assert_eq!(kind_of(Some("MOVIE")), TitleKind::Film);
        assert_eq!(kind_of(None), TitleKind::Film);
    }

    /// The miss this exists for: a folder named after a title whose apostrophe
    /// the filesystem never got, searched against an index that tokenised the
    /// apostrophe into a word boundary.
    #[test]
    fn a_name_that_lost_its_apostrophe_is_searched_for_again_without_the_s() {
        assert_eq!(
            search_terms("Fate stay night Heavens Feel"),
            vec![
                "Fate stay night Heavens Feel".to_string(),
                "Fate stay night Heaven Feel".to_string(),
            ]
        );
    }

    /// One request, not two, for the names that need no repair — the fallback
    /// is a second round trip and a title that already matched must not pay it.
    #[test]
    fn a_name_with_nothing_to_repair_is_searched_for_once() {
        assert_eq!(search_terms("Cowboy Bebop"), vec!["Cowboy Bebop"]);
        // Short words keep their `s`: `Kids on the Slope` is not `Kid on the
        // Slope`, and a name that still has its apostrophe is already what the
        // index holds.
        assert_eq!(search_terms("Heaven's Feel"), vec!["Heaven's Feel"]);
        assert_eq!(
            search_terms("Kids on the Bus"),
            vec!["Kids on the Bus".to_string(), "Kid on the Bus".to_string()],
            "a four-letter word is long enough; a three-letter one is not"
        );
    }

    /// Byte slicing, on names that are not ASCII.
    #[test]
    fn a_multi_byte_name_is_not_cut_inside_a_character() {
        for name in ["進撃の巨人", "Kaguya-sama: Love is War？", "café"] {
            let terms = search_terms(name);
            assert_eq!(terms[0], name);
        }
    }

    #[test]
    fn markup_becomes_text_and_breaks_become_newlines() {
        assert_eq!(
            strip_markup("A <i>story</i>.<br>Then more.<br />And more."),
            "A story.\nThen more.\nAnd more."
        );
        assert_eq!(strip_markup("  plain  "), "plain");
    }

    /// The alias list is what makes a romaji-named library match an
    /// English-named entry, so it has to hold every name AniList gave.
    #[test]
    fn a_candidate_carries_every_name_the_show_is_known_by() {
        let media: Media = serde_json::from_str(
            r#"{
                "id": 16498,
                "title": {"romaji": "Shingeki no Kyojin", "english": "Attack on Titan",
                          "native": "進撃の巨人"},
                "synonyms": ["AoT"],
                "description": "Several hundred years ago…",
                "startDate": {"year": 2013},
                "coverImage": {"extraLarge": "big.jpg", "large": "small.jpg"},
                "bannerImage": null,
                "averageScore": 84,
                "genres": ["Action"],
                "episodes": 25,
                "format": "TV",
                "siteUrl": "https://anilist.co/anime/16498"
            }"#,
        )
        .expect("parse");

        let candidate = media.into_candidate();
        assert_eq!(candidate.metadata.name, "Attack on Titan");
        assert_eq!(
            candidate.metadata.original_name.as_deref(),
            Some("進撃の巨人")
        );
        assert!(
            candidate
                .aliases
                .contains(&"Shingeki no Kyojin".to_string())
        );
        assert!(candidate.aliases.contains(&"AoT".to_string()));
        // Out of 10, not out of 100.
        assert_eq!(candidate.metadata.rating, Some(8.4));
        assert_eq!(candidate.metadata.poster_url.as_deref(), Some("big.jpg"));
    }

    fn edge(relation: &str, id: i64, kind: &str, format: &str) -> RelationEdge {
        RelationEdge {
            relation_type: Some(relation.into()),
            node: Some(RelationNode {
                id,
                kind: Some(kind.into()),
                format: Some(format.into()),
            }),
        }
    }

    /// Attack on Titan's relations: an adaptation, two compilation films, a
    /// prequel OVA and a spin-off around the one sequel that is the next season.
    #[test]
    fn the_chain_continues_through_the_sequel_and_nothing_else() {
        let edges = vec![
            edge("ADAPTATION", 53390, "MANGA", "MANGA"),
            edge("ALTERNATIVE", 20691, "ANIME", "MOVIE"),
            edge("PREQUEL", 20811, "ANIME", "OVA"),
            edge("SPIN_OFF", 21281, "ANIME", "TV"),
            edge("SEQUEL", 20958, "ANIME", "TV"),
        ];
        assert_eq!(
            next_in_chain(edges, &HashSet::new()),
            Some(Link {
                id: 20958,
                series: true
            })
        );
    }

    /// Mushishi's first series leads on only through an OVA, which is followed
    /// — but not counted as a season of the series.
    #[test]
    fn a_special_is_followed_when_it_is_the_only_way_on() {
        let edges = vec![edge("SEQUEL", 20526, "ANIME", "SPECIAL")];
        assert_eq!(
            next_in_chain(edges, &HashSet::new()),
            Some(Link {
                id: 20526,
                series: false
            })
        );
        // With a series beside it, the series wins.
        let edges = vec![
            edge("SEQUEL", 20752, "ANIME", "SPECIAL"),
            edge("SEQUEL", 20751, "ANIME", "TV"),
        ];
        assert_eq!(
            next_in_chain(edges, &HashSet::new()).map(|link| link.id),
            Some(20751)
        );
    }

    /// A franchise whose relations loop must end rather than walk forever.
    #[test]
    fn an_entry_already_read_is_not_read_again() {
        let edges = vec![edge("SEQUEL", 1, "ANIME", "TV")];
        assert_eq!(next_in_chain(edges, &HashSet::from([1])), None);
        let manga = vec![edge("SEQUEL", 2, "MANGA", "MANGA")];
        assert_eq!(next_in_chain(manga, &HashSet::new()), None);
    }

    /// The walk stops once every file is answered and not before: a season
    /// folder of absolute numbers counts as answered by its own season.
    #[test]
    fn the_walk_stops_when_every_numbered_file_has_an_answer() {
        let episode = |season, number, absolute| EpisodeMetadata {
            season: Some(season),
            number,
            absolute: Some(absolute),
            entry_season: None,
            entry_number: None,
            name: None,
            overview: None,
            still_url: None,
            air_date: None,
        };
        let season_one = vec![episode(1, 1, 1), episode(1, 2, 2)];
        assert!(answers_all(&season_one, &[(Some(1), 1), (None, 2)]));
        assert!(!answers_all(&season_one, &[(Some(1), 1), (Some(2), 26)]));

        let both = vec![episode(1, 1, 1), episode(2, 1, 26)];
        assert!(answers_all(&both, &[(Some(1), 1), (Some(2), 26)]));
        // A film: nothing numbered, nothing to walk for.
        assert!(answers_all(&[], &[]));
    }

    /// Missing fields are the normal case    /// Missing fields are the normal case for a recently added entry, and must
    /// not fail the whole page of results.
    #[test]
    fn an_entry_with_almost_nothing_in_it_still_parses() {
        let media: Media = serde_json::from_str(
            r#"{"id": 1, "title": {"romaji": null, "english": null, "native": null}}"#,
        )
        .expect("parse");
        let candidate = media.into_candidate();
        assert_eq!(candidate.metadata.name, "AniList #1");
        assert!(candidate.aliases.is_empty());
    }
}
