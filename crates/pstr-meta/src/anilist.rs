//! AniList, over its public GraphQL API.
//!
//! No API key and no account, which is why it is the default: turning
//! enrichment on costs the viewer a decision about privacy, and it should not
//! also cost them a signup. The trade is coverage — AniList is anime and nothing
//! else, so a library of films wants [`crate::tmdb`].
//!
//! One query shape is used for everything: a `Page` of `media` matching the
//! search string, with every name AniList knows the show by. The alias list is
//! the whole reason this works — see [`crate::matching`].

use pstr_core::library::TitleKind;
use pstr_core::metadata::{EpisodeMetadata, ProviderId, TitleMetadata};
use serde::Deserialize;

use crate::error::{Error, Result};
use crate::matching::{Candidate, Query};
use crate::provider::Provider;

const ENDPOINT: &str = "https://graphql.anilist.co";

/// How many answers to score. AniList's own relevance ordering is good, and the
/// right title is essentially always in the first handful; asking for more costs
/// them bandwidth and buys nothing.
const PAGE_SIZE: u32 = 8;

/// AniList scores out of 100 and the rest of the app works out of 10.
const SCORE_SCALE: f32 = 10.0;

const SEARCH: &str = r#"
query ($search: String, $perPage: Int) {
  Page(page: 1, perPage: $perPage) {
    media(search: $search, type: ANIME, sort: SEARCH_MATCH) {
      id
      title { romaji english native }
      synonyms
      description(asHtml: false)
      startDate { year }
      coverImage { extraLarge large }
      bannerImage
      averageScore
      genres
      episodes
      format
      siteUrl
    }
  }
}
"#;

/// Episode titles, such as AniList has them.
///
/// `streamingEpisodes` is the only place AniList keeps them — there is no
/// per-episode overview in its API at all — and the entries are what the
/// streaming sites published: `"Episode 57 - The Immortal Legion"`, in airing
/// order, occasionally with a gap. So the number is read out of the title where
/// it says one, and only falls back to the position in the list.
const EPISODES: &str = r#"
query ($id: Int) {
  Media(id: $id, type: ANIME) {
    episodes
    streamingEpisodes { title thumbnail }
  }
}
"#;

pub struct AniList {
    http: reqwest::Client,
}

impl AniList {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }
}

impl Provider for AniList {
    fn id(&self) -> ProviderId {
        ProviderId::AniList
    }

    /// A sequel is its own entry here, numbering its episodes from one.
    fn seasons_are_separate_entries(&self) -> bool {
        true
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

    async fn episodes(&self, title: &TitleMetadata) -> Result<Vec<EpisodeMetadata>> {
        // The id came from our own search response, so a non-numeric one is a
        // record written by some other version — not worth an error, and there
        // is nothing to ask about.
        let Ok(id) = title.remote_id.parse::<i64>() else {
            return Ok(Vec::new());
        };

        let data: Option<MediaData> = self
            .query(serde_json::json!({
                "query": EPISODES,
                "variables": { "id": id },
            }))
            .await?;

        Ok(data
            .and_then(|data| data.media)
            .map(|media| media.into_episodes())
            .unwrap_or_default())
    }
}

impl AniList {
    /// One GraphQL request, with AniList's two ways of failing — an HTTP status
    /// and an `errors` array under a 200 — folded into one.
    async fn query<T: serde::de::DeserializeOwned>(
        &self,
        body: serde_json::Value,
    ) -> Result<Option<T>> {
        let response = self.http.post(ENDPOINT).json(&body).send().await?;
        let status = response.status();
        if !status.is_success() {
            // 429 is the one worth naming: AniList rate-limits by the minute,
            // and a caller that knows it was throttled can back off rather than
            // cache a miss for a title that would have matched.
            return Err(if status.as_u16() == 429 {
                Error::RateLimited
            } else {
                Error::Http(format!("AniList answered {status}"))
            });
        }

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

#[derive(Deserialize)]
struct MediaData {
    #[serde(rename = "Media")]
    media: Option<EpisodeMedia>,
}

#[derive(Deserialize)]
struct EpisodeMedia {
    #[serde(rename = "streamingEpisodes", default)]
    streaming_episodes: Vec<StreamingEpisode>,
}

#[derive(Deserialize)]
struct StreamingEpisode {
    title: Option<String>,
    thumbnail: Option<String>,
}

impl EpisodeMedia {
    fn into_episodes(self) -> Vec<EpisodeMetadata> {
        self.streaming_episodes
            .into_iter()
            .enumerate()
            .map(|(index, episode)| {
                let raw = episode.title.unwrap_or_default();
                let (number, name) = split_numbering(&raw);
                EpisodeMetadata {
                    // AniList counts straight through, with no seasons: a
                    // sequel is a separate entry, not a second season.
                    season: None,
                    // The position is the fallback and not the answer: the
                    // lists have gaps, and an off-by-one here would caption
                    // every episode of a show with the next one's name.
                    number: number.unwrap_or(index as u32 + 1),
                    name: name.filter(|name| !name.is_empty()),
                    // AniList has no per-episode synopsis to give.
                    overview: None,
                    still_url: episode.thumbnail,
                    air_date: None,
                }
            })
            .collect()
    }
}

/// Split `"Episode 57 - The Immortal Legion"` into its number and its name.
///
/// Both parts are optional: plenty of entries are just `"Episode 12"`, and a
/// few are only a name.
fn split_numbering(title: &str) -> (Option<u32>, Option<String>) {
    let trimmed = title.trim();
    let rest = trimmed
        .strip_prefix("Episode ")
        .or_else(|| trimmed.strip_prefix("episode "))
        .or_else(|| trimmed.strip_prefix("EP"))
        .or_else(|| trimmed.strip_prefix("E"));

    let Some(rest) = rest else {
        return (None, Some(trimmed.to_string()).filter(|t| !t.is_empty()));
    };

    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return (None, Some(trimmed.to_string()));
    }

    let name = rest[digits.len()..]
        .trim_start()
        .trim_start_matches(['-', '–', ':'])
        .trim()
        .to_string();
    (digits.parse().ok(), Some(name).filter(|n| !n.is_empty()))
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
    #[serde(default)]
    genres: Vec<String>,
    episodes: Option<u32>,
    format: Option<String>,
    #[serde(rename = "siteUrl")]
    site_url: Option<String>,
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
                // Usually `null`. AniList only has banners for the better-known
                // shows, which is why a tile has to cope with a poster's shape.
                backdrop_url: self.banner_image,
                rating: self.average_score.map(|score| score / SCORE_SCALE),
                genres: self.genres,
                episodes: self.episodes,
                url: self.site_url,
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

    #[test]
    fn an_episode_title_gives_up_its_number_and_its_name() {
        assert_eq!(
            split_numbering("Episode 57 - The Immortal Legion"),
            (Some(57), Some("The Immortal Legion".to_string()))
        );
        assert_eq!(split_numbering("Episode 12"), (Some(12), None));
        // Some entries are only a name, and some sites write it differently.
        assert_eq!(
            split_numbering("The Day of the Beginning"),
            (None, Some("The Day of the Beginning".to_string()))
        );
        assert_eq!(
            split_numbering("E3 – Whatever"),
            (Some(3), Some("Whatever".to_string()))
        );
        assert_eq!(split_numbering(""), (None, None));
    }

    #[test]
    fn episode_numbers_come_from_the_titles_and_not_from_the_order() {
        // AniList lists what streaming sites published, and those lists have
        // gaps. Numbering by position would caption episode 4 as episode 3.
        let media: EpisodeMedia = serde_json::from_str(
            r#"{"streamingEpisodes": [
                   {"title": "Episode 1 - Fullmetal Alchemist", "thumbnail": "one.jpg"},
                   {"title": "Episode 3 - City of Heresy", "thumbnail": null}
               ]}"#,
        )
        .expect("parse");

        let episodes = media.into_episodes();
        assert_eq!(episodes[0].number, 1);
        assert_eq!(episodes[0].name.as_deref(), Some("Fullmetal Alchemist"));
        assert_eq!(episodes[0].still_url.as_deref(), Some("one.jpg"));
        assert_eq!(episodes[1].number, 3);
        // AniList counts straight through; nothing here is a season.
        assert!(episodes.iter().all(|episode| episode.season.is_none()));
    }

    #[test]
    fn an_untitled_list_still_numbers_its_episodes() {
        let media: EpisodeMedia = serde_json::from_str(
            r#"{"streamingEpisodes": [{"title": null, "thumbnail": null},
                                      {"title": "", "thumbnail": null}]}"#,
        )
        .expect("parse");
        let episodes = media.into_episodes();
        assert_eq!(episodes[0].number, 1);
        assert_eq!(episodes[1].number, 2);
        assert!(episodes.iter().all(|episode| episode.name.is_none()));
    }

    /// Missing fields are the normal case for a recently added entry, and must
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
