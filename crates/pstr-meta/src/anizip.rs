//! ani.zip: what AniList does not say about one of its own entries.
//!
//! AniList is where a title is *identified* — its search and its aliases are
//! what make a romaji folder name match — but it keeps no episode list worth
//! the name and no landscape art. ani.zip answers both, keyed by the AniList id
//! the match already produced, so asking it sends nothing about the library the
//! match did not already send. No key and no account, like AniList.
//!
//! The episode list is TVDB's, and that is the point of it: release groups
//! number their files the way TVDB does — `Bleach - S17E01` is TVDB's season
//! 17, not AniList's — and every episode here carries TVDB's season, number and
//! absolute number together. See `docs/METADATA.md`.
//!
//! It is a community service rather than an institution, so everything it says
//! is stored, and an entry it does not know (`404`) is an empty answer rather
//! than a failure.

use pstr_core::metadata::EpisodeMetadata;
use serde::Deserialize;

use crate::error::{Error, Result};
use crate::limiter::RateLimiter;
use crate::tmdb::TmdbId;

const ENDPOINT: &str = "https://api.ani.zip/mappings";

/// ani.zip publishes no limit. One a second is far below anything a CDN-cached
/// JSON endpoint notices, and a library's enrichment is a one-off burst.
const REQUESTS_PER_MINUTE: u32 = 60;

/// TVDB's own placeholder for an episode it has not named yet.
const UNNAMED: &str = "TBA";

/// What ani.zip knows about one AniList entry.
#[derive(Debug, Default)]
pub struct Mapping {
    pub episodes: Vec<EpisodeMetadata>,
    /// TVDB's fanart for the whole series: 16:9, usually 1920×1080.
    pub fanart: Option<String>,
    /// TVDB's clear logo: the series' name as lettering, a transparent PNG.
    pub logo: Option<String>,
    /// The same entry on TMDB, where TVDB has none of the films.
    pub tmdb: Option<TmdbId>,
}

pub struct AniZip {
    http: reqwest::Client,
    /// The episode-title language tried first; English, then romaji, then
    /// Japanese after it.
    language: String,
    limiter: RateLimiter,
}

impl AniZip {
    pub fn new(http: reqwest::Client, language: String) -> Self {
        Self {
            http,
            language,
            limiter: RateLimiter::per_minute(REQUESTS_PER_MINUTE),
        }
    }

    /// Everything ani.zip has for the AniList entry `anilist_id`.
    ///
    /// `entry_season` is where this entry sits among the franchise's series
    /// entries — see [`EpisodeMetadata::entry_season`] — and `None` for a film
    /// or a special, whose episodes no release numbers that way.
    pub async fn mapping(&self, anilist_id: i64, entry_season: Option<u32>) -> Result<Mapping> {
        self.limiter.acquire().await;
        let response = self
            .http
            .get(ENDPOINT)
            .query(&[("anilist_id", anilist_id)])
            .send()
            .await?;
        let status = response.status();
        if status.as_u16() == 404 {
            return Ok(Mapping::default());
        }
        if status.as_u16() == 429 {
            self.limiter.throttled(response.headers()).await;
            return Err(Error::RateLimited);
        }
        if !status.is_success() {
            return Err(Error::Http(format!("ani.zip answered {status}")));
        }
        let payload: Payload = response.json().await?;
        Ok(payload.into_mapping(&self.language, entry_season))
    }
}

#[derive(Deserialize, Default)]
struct Payload {
    #[serde(default)]
    episodes: std::collections::BTreeMap<String, Episode>,
    #[serde(default)]
    images: Vec<Image>,
    mappings: Option<Ids>,
}

/// The entry's ids elsewhere.
#[derive(Deserialize)]
struct Ids {
    /// AniList's format, upper-cased: `TV`, `MOVIE`, `OVA`…
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default, deserialize_with = "lenient_number")]
    themoviedb_id: Option<u32>,
}

impl Ids {
    /// The TMDB entry, where its kind says which list the id is from. An OVA
    /// or a special is filed either way on TMDB, so it is left alone rather
    /// than guessed.
    fn tmdb(&self) -> Option<TmdbId> {
        let id = self.themoviedb_id?;
        match self.kind.as_deref()? {
            "MOVIE" => Some(TmdbId::Movie(id)),
            "TV" | "TV_SHORT" | "ONA" => Some(TmdbId::Tv(id)),
            _ => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Episode {
    #[serde(default, deserialize_with = "lenient_number")]
    season_number: Option<u32>,
    #[serde(default, deserialize_with = "lenient_number")]
    episode_number: Option<u32>,
    #[serde(default, deserialize_with = "lenient_number")]
    absolute_episode_number: Option<u32>,
    title: Option<std::collections::HashMap<String, Option<String>>>,
    /// TVDB's synopsis.
    overview: Option<String>,
    /// AniDB's, which is there when TVDB's is not.
    summary: Option<String>,
    image: Option<String>,
    air_date: Option<String>,
    /// TVDB's running time, in minutes.
    #[serde(default, deserialize_with = "lenient_number")]
    runtime: Option<u32>,
    /// AniDB's, which is there when TVDB's is not.
    #[serde(default, deserialize_with = "lenient_number")]
    length: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Image {
    cover_type: Option<String>,
    url: Option<String>,
}

impl Payload {
    fn into_mapping(self, language: &str, entry_season: Option<u32>) -> Mapping {
        let image = |kind: &str| {
            self.images
                .iter()
                .find(|image| image.cover_type.as_deref() == Some(kind))
                .and_then(|image| text(image.url.clone()))
        };
        let fanart = image("Fanart");
        let logo = image("Clearlogo");
        let tmdb = self.mappings.as_ref().and_then(Ids::tmdb);

        let episodes = self
            .episodes
            .into_iter()
            .filter_map(|(key, episode)| {
                // An episode TVDB has not placed has nothing a filename could
                // be matched against — a film's own entry is the usual case.
                let season = episode.season_number?;
                let number = episode.episode_number?;
                // Keys are the entry's own numbering: `"1"`, `"2"` for its
                // episodes and `"S1"` for its specials, which no release counts
                // as part of the season.
                let entry_number = entry_season.and(key.parse::<u32>().ok());
                Some(EpisodeMetadata {
                    season: Some(season),
                    number,
                    absolute: episode.absolute_episode_number,
                    entry_season: entry_number.and(entry_season),
                    entry_number,
                    name: episode
                        .title
                        .as_ref()
                        .and_then(|titles| title_in(titles, language)),
                    overview: text(episode.overview).or_else(|| text(episode.summary)),
                    still_url: text(episode.image),
                    air_date: text(episode.air_date),
                    runtime: episode
                        .runtime
                        .or(episode.length)
                        .filter(|minutes| *minutes > 0),
                })
            })
            .collect();

        Mapping {
            episodes,
            fanart,
            logo,
            tmdb,
        }
    }
}

/// The episode's name in `language`, else English, romaji or Japanese.
fn title_in(
    titles: &std::collections::HashMap<String, Option<String>>,
    language: &str,
) -> Option<String> {
    [language, "en", "x-jat", "ja"]
        .into_iter()
        .filter_map(|language| titles.get(language).cloned().flatten())
        .map(|title| title.trim().to_string())
        .find(|title| !title.is_empty() && title != UNNAMED)
}

fn text(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// A number, whether it arrives as one or as a string of digits.
///
/// ani.zip writes its regular episodes' numbers as JSON numbers and its
/// specials' as strings; one strict field would fail the whole entry over the
/// specials.
fn lenient_number<'de, D>(deserializer: D) -> std::result::Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::Number(number) => number.as_u64().and_then(|n| u32::try_from(n).ok()),
        serde_json::Value::String(text) => text.trim().parse().ok(),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Attack on Titan Season 2 as ani.zip answers it, trimmed: a regular
    /// episode numbered in numbers, a special numbered in strings, a film-like
    /// entry TVDB never placed, and the series' art.
    const SEASON_TWO: &str = r#"{
        "episodes": {
            "1": {"seasonNumber": 2, "episodeNumber": 1, "absoluteEpisodeNumber": 26,
                  "episode": "1",
                  "title": {"en": "Beast Titan", "ja": "獣の巨人", "x-jat": "Kemono no Kyojin"},
                  "overview": "Titans appear inside Wall Rose.",
                  "image": "https://artworks.thetvdb.com/still.jpg",
                  "airDate": "2017-04-01", "runtime": 24, "length": 25},
            "S1": {"seasonNumber": "0", "episodeNumber": "7", "episode": "S1",
                   "title": {"en": "TBA", "x-jat": "Ilse no Techou"},
                   "summary": "An OVA.", "length": "30"},
            "2": {"episode": "2", "title": {"en": "Unplaced"}}
        },
        "images": [
            {"coverType": "Banner", "url": "banner.jpg"},
            {"coverType": "Fanart", "url": "fanart.jpg"},
            {"coverType": "Clearlogo", "url": "clearlogo.png"}
        ]
    }"#;

    fn mapping(entry_season: Option<u32>) -> Mapping {
        serde_json::from_str::<Payload>(SEASON_TWO)
            .expect("parse")
            .into_mapping("en", entry_season)
    }

    #[test]
    fn an_episode_carries_tvdbs_numbering_and_its_entrys() {
        let mapping = mapping(Some(2));
        let first = mapping
            .episodes
            .iter()
            .find(|episode| episode.season == Some(2))
            .expect("the regular episode");
        assert_eq!(first.number, 1);
        assert_eq!(first.absolute, Some(26));
        assert_eq!((first.entry_season, first.entry_number), (Some(2), Some(1)));
        assert_eq!(first.name.as_deref(), Some("Beast Titan"));
        assert_eq!(
            first.overview.as_deref(),
            Some("Titans appear inside Wall Rose.")
        );
        assert_eq!(first.air_date.as_deref(), Some("2017-04-01"));
        // TVDB's running time wins over AniDB's.
        assert_eq!(first.runtime, Some(24));
    }

    /// A special is numbered in strings and filed under season zero, and no
    /// release counts it as an episode of the entry it came with.
    #[test]
    fn a_special_keeps_its_tvdb_place_and_takes_no_entry_number() {
        let mapping = mapping(Some(2));
        let special = mapping
            .episodes
            .iter()
            .find(|episode| episode.season == Some(0))
            .expect("the special");
        assert_eq!(special.number, 7);
        assert_eq!(special.entry_number, None);
        assert_eq!(special.entry_season, None);
        // `TBA` is TVDB saying it has no name yet, so the next language answers.
        assert_eq!(special.name.as_deref(), Some("Ilse no Techou"));
        // AniDB's summary stands in for TVDB's missing overview.
        assert_eq!(special.overview.as_deref(), Some("An OVA."));
        // And AniDB's length for TVDB's missing running time.
        assert_eq!(special.runtime, Some(30));
    }

    #[test]
    fn an_episode_tvdb_never_placed_is_left_out() {
        assert_eq!(mapping(Some(2)).episodes.len(), 2);
    }

    /// A film or a special as an entry has no place among the series' seasons.
    #[test]
    fn an_entry_outside_the_series_numbers_nothing_by_entry() {
        assert!(
            mapping(None)
                .episodes
                .iter()
                .all(|episode| episode.entry_season.is_none() && episode.entry_number.is_none())
        );
    }

    /// The 16:9 picture, not the banner — the banner is the 4.75:1 strip that
    /// looked wrong on every desktop tile.
    #[test]
    fn the_fanart_is_taken_and_the_banner_is_not() {
        assert_eq!(mapping(None).fanart.as_deref(), Some("fanart.jpg"));
    }

    #[test]
    fn the_clear_logo_is_kept() {
        assert_eq!(mapping(None).logo.as_deref(), Some("clearlogo.png"));
    }

    #[test]
    fn an_entry_ani_zip_has_nothing_for_parses_to_nothing() {
        let mapping = serde_json::from_str::<Payload>("{}")
            .expect("parse")
            .into_mapping("en", Some(1));
        assert!(mapping.episodes.is_empty());
        assert_eq!(mapping.fanart, None);
        assert_eq!(mapping.logo, None);
        assert_eq!(mapping.tmdb, None);
    }

    /// Paprika: a film with no TVDB art at all, and a TMDB id written as a
    /// string.
    #[test]
    fn a_films_tmdb_id_is_kept_as_a_film() {
        let mapping = serde_json::from_str::<Payload>(
            r#"{"images": [], "mappings": {"type": "MOVIE", "themoviedb_id": "4977"}}"#,
        )
        .expect("parse")
        .into_mapping("en", None);
        assert_eq!(mapping.tmdb, Some(TmdbId::Movie(4977)));
    }

    #[test]
    fn a_series_tmdb_id_is_kept_and_a_specials_is_not() {
        let tmdb = |kind: &str| {
            serde_json::from_str::<Payload>(&format!(
                r#"{{"mappings": {{"type": "{kind}", "themoviedb_id": 209867}}}}"#
            ))
            .expect("parse")
            .into_mapping("en", Some(1))
            .tmdb
        };
        assert_eq!(tmdb("TV"), Some(TmdbId::Tv(209867)));
        assert_eq!(tmdb("OVA"), None);
    }
}
