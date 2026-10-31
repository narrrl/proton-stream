//! The Movie Database, over its v3 REST API.
//!
//! Film and television, which is the coverage AniList does not have. The cost is
//! an API key: TMDB's are free but they require an account, so this is not the
//! default and the UI has somewhere to put the key before it can be chosen.
//!
//! `search/multi` rather than `search/tv` or `search/movie`: a library holds both
//! and the parser's guess at which one a title is comes from a filename, which is
//! not evidence enough to pick an endpoint. The `media_type` on each result is
//! better evidence than the guess, and [`crate::matching`] only nudges on kind
//! anyway.
//!
//! A stored key is also used with AniList chosen, by id alone: ani.zip names
//! the TMDB entry an AniList match is, and [`Tmdb::art`] fetches its backdrop
//! and logo — what the films TVDB has no art for are missing. No title is
//! searched for that way.

use pstr_core::library::{Title, TitleKind};
use pstr_core::metadata::{Enrichment, EpisodeMetadata, ProviderId, TitleMetadata};
use serde::Deserialize;

use crate::error::{Error, Result};
use crate::limiter::RateLimiter;
use crate::matching::{Candidate, Query};
use crate::provider::Provider;

const ENDPOINT: &str = "https://api.themoviedb.org/3/search/multi";
const API_BASE: &str = "https://api.themoviedb.org/3";

/// How many seasons of episodes to ask for in one go.
///
/// TMDB's `append_to_response` takes at most twenty sub-requests, which is more
/// seasons than anything in a library actually has; a show past that gets its
/// first twenty and no error.
const MAX_APPENDED_SEASONS: usize = 20;

/// A frame from an episode. `w300` is what the row draws it at, near enough.
const STILL_SIZE: &str = "w300";

/// Where TMDB serves artwork from, and the sizes worth asking for.
///
/// `w500` for a poster and a logo and `w1280` for a backdrop are what the
/// images are drawn at here, give or take — a backdrop fills the width of the
/// title page and the library's banner; `original` is several megabytes of
/// print-resolution artwork per title, which is bandwidth spent on pixels no
/// window shows.
const IMAGE_BASE: &str = "https://image.tmdb.org/t/p";

const POSTER_SIZE: &str = "w500";
const BACKDROP_SIZE: &str = "w1280";
const LOGO_SIZE: &str = "w500";

/// TMDB's published ceiling is around fifty requests a second and it no longer
/// enforces a daily one. This is well under it: the point of pacing here is a
/// burst that would look like abuse, not a rate anyone is close to.
const REQUESTS_PER_MINUTE: u32 = 600;

/// As [`crate::anilist`]'s.
const RETRIES: usize = 2;

pub struct Tmdb {
    http: reqwest::Client,
    api_key: String,
    language: String,
    limiter: RateLimiter,
}

impl Tmdb {
    pub fn new(http: reqwest::Client, api_key: String, language: String) -> Self {
        Self {
            http,
            api_key,
            language,
            limiter: RateLimiter::per_minute(REQUESTS_PER_MINUTE),
        }
    }
}

/// A TMDB entry, by id. Films and television are numbered separately, so the
/// id alone does not name one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TmdbId {
    Movie(u32),
    Tv(u32),
}

impl TmdbId {
    fn path(self) -> String {
        match self {
            Self::Movie(id) => format!("movie/{id}"),
            Self::Tv(id) => format!("tv/{id}"),
        }
    }
}

/// An entry's wide picture and its logo.
#[derive(Debug, Default, PartialEq)]
pub struct Art {
    pub backdrop: Option<String>,
    pub logo: Option<String>,
}

impl Provider for Tmdb {
    fn id(&self) -> ProviderId {
        ProviderId::Tmdb
    }

    async fn search(&self, query: &Query) -> Result<Vec<Candidate>> {
        if self.api_key.trim().is_empty() {
            return Err(Error::MissingApiKey(ProviderId::Tmdb));
        }

        let payload: SearchResponse = self
            .get(
                ENDPOINT,
                &[("query", query.name.as_str()), ("include_adult", "false")],
            )
            .await?;
        Ok(payload
            .results
            .into_iter()
            // `search/multi` also returns people, who have neither a poster of a
            // show nor a year — and whose `name` would match a title named after
            // its lead.
            .filter(|result| matches!(result.media_type.as_deref(), Some("tv" | "movie")))
            .map(Result_::into_candidate)
            .collect())
    }

    /// The episode list and the logo. The match already carried a proper
    /// backdrop, and TMDB files every season under one show, so the library's
    /// seasons need no walking.
    async fn enrich(&self, _title: &Title, found: &TitleMetadata) -> Result<Enrichment> {
        let logo_url = match found.remote_id.parse() {
            Ok(id) => {
                self.art_or_nothing(match found.kind {
                    TitleKind::Series => TmdbId::Tv(id),
                    TitleKind::Film => TmdbId::Movie(id),
                })
                .await
                .logo
            }
            Err(_) => None,
        };
        Ok(Enrichment {
            episodes: self.episodes(found).await?,
            backdrop_url: None,
            logo_url,
            chain: Vec::new(),
        })
    }
}

impl Tmdb {
    /// Two requests: the show, to learn which seasons exist, and then those
    /// seasons appended onto one more.
    ///
    /// `append_to_response` is why this is two rather than one-per-season — a
    /// five-season show costs the same as a one-season show, which matters when
    /// a library of thirty series is matched in one run.
    async fn episodes(&self, title: &TitleMetadata) -> Result<Vec<EpisodeMetadata>> {
        // Films have no episodes, and asking `/tv/{id}` about one answers 404
        // for an id that belongs to a film.
        if title.kind != TitleKind::Series {
            return Ok(Vec::new());
        }
        let Ok(id) = title.remote_id.parse::<i64>() else {
            return Ok(Vec::new());
        };

        let show: Show = self.get(&format!("{API_BASE}/tv/{id}"), &[]).await?;
        let numbers: Vec<u32> = show
            .seasons
            .iter()
            .map(|season| season.season_number)
            // Season zero is TMDB's bin for specials and recaps, which are not
            // numbered the way a release names them and would collide with the
            // real season they sit beside.
            .filter(|number| *number > 0)
            .take(MAX_APPENDED_SEASONS)
            .collect();
        if numbers.is_empty() {
            return Ok(Vec::new());
        }

        let append = numbers
            .iter()
            .map(|number| format!("season/{number}"))
            .collect::<Vec<_>>()
            .join(",");
        let detail: SeasonsResponse = self
            .get(
                &format!("{API_BASE}/tv/{id}"),
                &[("append_to_response", append.as_str())],
            )
            .await?;

        let mut episodes = Vec::new();
        for number in numbers {
            let Some(season) = detail.season(number) else {
                continue;
            };
            episodes.extend(season.episodes.iter().map(|episode| {
                EpisodeMetadata {
                    season: Some(number),
                    number: episode.episode_number,
                    absolute: None,
                    entry_season: None,
                    entry_number: None,
                    name: episode.name.clone().filter(|name| !name.is_empty()),
                    overview: episode.overview.clone().filter(|text| !text.is_empty()),
                    still_url: episode
                        .still_path
                        .as_deref()
                        .map(|path| image_url(STILL_SIZE, path)),
                    air_date: episode.air_date.clone().filter(|date| !date.is_empty()),
                    runtime: episode.runtime.filter(|minutes| *minutes > 0),
                    mal_id: None,
                }
            }));
        }
        Ok(episodes)
    }

    /// The best backdrop and logo TMDB has for `id`.
    ///
    /// Artwork in the viewer's language, English or Japanese is asked for, and
    /// artwork with no language at all, which for a backdrop is the one without
    /// lettering on it. One request.
    pub async fn art(&self, id: TmdbId) -> Result<Art> {
        let languages = format!("{},en,ja,null", self.language_code());
        let images: Images = self
            .get(
                &format!("{API_BASE}/{}/images", id.path()),
                &[("include_image_language", languages.as_str())],
            )
            .await?;
        Ok(images.into_art(self.language_code()))
    }

    /// [`Tmdb::art`], with a failure logged and taken as no art: the art is
    /// decoration on an answer that is already complete, and a bad key or a
    /// slow minute must not stop the episodes beside it being stored.
    pub async fn art_or_nothing(&self, id: TmdbId) -> Art {
        self.art(id)
            .await
            .inspect_err(|error| tracing::debug!("TMDB art for {id:?}: {error}"))
            .unwrap_or_default()
    }

    /// `de` out of `de-DE`: images are tagged by language alone.
    fn language_code(&self) -> &str {
        self.language.split('-').next().unwrap_or("en")
    }

    /// One authenticated GET, paced, with TMDB's failures named.
    ///
    /// TMDB is far more generous than AniList and a library-sized run rarely
    /// troubles it, but the pacing costs nothing here and the `429` handling is
    /// the same code — see [`crate::limiter`].
    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        extra: &[(&str, &str)],
    ) -> Result<T> {
        if self.api_key.trim().is_empty() {
            return Err(Error::MissingApiKey(ProviderId::Tmdb));
        }

        let mut attempts = 0usize;
        let response = loop {
            let mut request = self.http.get(url).query(&[
                ("api_key", self.api_key.as_str()),
                ("language", self.language.as_str()),
            ]);
            for pair in extra {
                request = request.query(&[pair]);
            }

            self.limiter.acquire().await;
            let response = request.send().await?;
            let status = response.status();

            if status.as_u16() == 429 {
                let waited = self.limiter.throttled(response.headers()).await;
                attempts += 1;
                if attempts > RETRIES {
                    return Err(Error::RateLimited);
                }
                tracing::debug!("TMDB is throttling; waiting {waited:?}");
                continue;
            }

            if !status.is_success() {
                return Err(match status.as_u16() {
                    401 => Error::MissingApiKey(ProviderId::Tmdb),
                    _ => Error::Http(format!("TMDB answered {status}")),
                });
            }
            break response;
        };

        Ok(response.json().await?)
    }
}

/// `/tv/{id}`, for the season list alone.
#[derive(Deserialize)]
struct Show {
    #[serde(default)]
    seasons: Vec<SeasonSummary>,
}

#[derive(Deserialize)]
struct SeasonSummary {
    season_number: u32,
}

/// The same endpoint with seasons appended. The appended objects arrive under
/// the key that asked for them — `"season/1"` — alongside every ordinary field
/// of the show, so the map is collected untyped and each season is parsed out
/// of it: typing the map as a season would make `"id": 1234` a parse error and
/// lose the whole response.
#[derive(Deserialize)]
struct SeasonsResponse {
    #[serde(flatten)]
    fields: std::collections::HashMap<String, serde_json::Value>,
}

impl SeasonsResponse {
    fn season(&self, number: u32) -> Option<SeasonDetail> {
        let value = self.fields.get(&format!("season/{number}"))?;
        serde_json::from_value(value.clone())
            .inspect_err(|error| tracing::debug!("TMDB season {number}: {error}"))
            .ok()
    }
}

#[derive(Deserialize)]
struct SeasonDetail {
    #[serde(default)]
    episodes: Vec<EpisodeRow>,
}

#[derive(Deserialize)]
struct EpisodeRow {
    episode_number: u32,
    name: Option<String>,
    overview: Option<String>,
    still_path: Option<String>,
    air_date: Option<String>,
    /// Minutes.
    runtime: Option<u32>,
}

/// `/{movie,tv}/{id}/images`.
#[derive(Deserialize, Default)]
struct Images {
    #[serde(default)]
    backdrops: Vec<Image>,
    #[serde(default)]
    logos: Vec<Image>,
}

#[derive(Deserialize)]
struct Image {
    file_path: String,
    iso_639_1: Option<String>,
    #[serde(default)]
    vote_average: f32,
}

impl Images {
    fn into_art(self, language: &str) -> Art {
        // Bare is best for a backdrop: a logo or a title is drawn over it,
        // and lettering already in the picture fights that.
        let backdrop = best(&self.backdrops, |image| match image.iso_639_1.as_deref() {
            None => 2,
            Some(tag) if tag == language || tag == "en" => 1,
            Some(_) => 0,
        });
        // A logo is lettering, so it is wanted in a language the viewer reads.
        // Only PNGs: an SVG is nothing the art loaders here can draw.
        let logos: Vec<&Image> = self
            .logos
            .iter()
            .filter(|image| image.file_path.ends_with(".png"))
            .collect();
        let logo = best(&logos, |image| match image.iso_639_1.as_deref() {
            Some(tag) if tag == language => 3,
            Some("en") => 2,
            None => 1,
            Some(_) => 0,
        });
        Art {
            backdrop: backdrop.map(|image| image_url(BACKDROP_SIZE, &image.file_path)),
            logo: logo.map(|image| image_url(LOGO_SIZE, &image.file_path)),
        }
    }
}

/// The image `rank` likes most, the better-voted one among equals.
fn best<T: std::borrow::Borrow<Image>>(
    images: &[T],
    rank: impl Fn(&Image) -> u8,
) -> Option<&Image> {
    images.iter().map(|image| image.borrow()).max_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then(a.vote_average.total_cmp(&b.vote_average))
    })
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<Result_>,
}

/// One `search/multi` hit. TMDB names the same field `name` on television and
/// `title` on film, and likewise for the dates, so both are optional here and
/// resolved after parsing.
#[derive(Deserialize)]
struct Result_ {
    id: i64,
    media_type: Option<String>,
    name: Option<String>,
    title: Option<String>,
    original_name: Option<String>,
    original_title: Option<String>,
    overview: Option<String>,
    first_air_date: Option<String>,
    release_date: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    vote_average: Option<f32>,
}

impl Result_ {
    fn into_candidate(self) -> Candidate {
        let kind = match self.media_type.as_deref() {
            Some("tv") => TitleKind::Series,
            _ => TitleKind::Film,
        };
        let name = self
            .name
            .clone()
            .or_else(|| self.title.clone())
            .unwrap_or_else(|| format!("TMDB #{}", self.id));
        let original_name = self
            .original_name
            .clone()
            .or_else(|| self.original_title.clone())
            .filter(|original| *original != name);

        let aliases = [
            self.name,
            self.title,
            self.original_name,
            self.original_title,
        ]
        .into_iter()
        .flatten()
        .collect();

        Candidate {
            metadata: TitleMetadata {
                provider: ProviderId::Tmdb,
                remote_id: self.id.to_string(),
                name,
                original_name,
                overview: self.overview.filter(|text| !text.is_empty()),
                year: year_of(self.first_air_date.or(self.release_date).as_deref()),
                kind,
                poster_url: self.poster_path.map(|path| image_url(POSTER_SIZE, &path)),
                backdrop_url: self
                    .backdrop_path
                    .map(|path| image_url(BACKDROP_SIZE, &path)),
                logo_url: None,
                rating: self.vote_average.filter(|rating| *rating > 0.0),
                // `search/multi` carries neither, and a detail request per
                // candidate would be a round-trip per *result* rather than per
                // title. Not worth it for a genre list.
                genres: Vec::new(),
                episodes: None,
                url: Some(format!(
                    "https://www.themoviedb.org/{}/{}",
                    match kind {
                        TitleKind::Series => "tv",
                        TitleKind::Film => "movie",
                    },
                    self.id
                )),
                // Nothing here yet: TMDB's would come from a detail request
                // per title, and the search answer has none of it.
                details: None,
            },
            aliases,
            // Always: `search/multi` results are filtered on `media_type`
            // before they get here, so it is never the fallback.
            kind_known: true,
        }
    }
}

fn image_url(size: &str, path: &str) -> String {
    format!("{IMAGE_BASE}/{size}{path}")
}

/// The year out of a TMDB `YYYY-MM-DD`, tolerating the empty string it uses for
/// "unknown" and anything else that is not a date.
fn year_of(date: Option<&str>) -> Option<u32> {
    date?.get(..4)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_year_is_taken_from_the_front_of_a_date() {
        assert_eq!(year_of(Some("1998-04-03")), Some(1998));
        assert_eq!(year_of(Some("")), None);
        assert_eq!(year_of(Some("soon")), None);
        assert_eq!(year_of(None), None);
    }

    /// The appended-season response carries the show's own fields beside the
    /// seasons, and those must not stop the seasons parsing.
    #[test]
    fn seasons_are_picked_out_of_a_response_full_of_other_fields() {
        let response: SeasonsResponse = serde_json::from_str(
            r#"{
                "id": 31911,
                "name": "Fullmetal Alchemist: Brotherhood",
                "number_of_seasons": 1,
                "genres": [{"id": 16, "name": "Animation"}],
                "season/1": {
                    "episodes": [
                        {"episode_number": 1, "name": "Fullmetal Alchemist",
                         "overview": "Ed and Al…", "still_path": "/still.jpg",
                         "air_date": "2009-04-05"},
                        {"episode_number": 2, "name": "The First Day",
                         "overview": "", "still_path": null, "air_date": null}
                    ]
                }
            }"#,
        )
        .expect("parse");

        let season = response.season(1).expect("season one");
        assert_eq!(season.episodes.len(), 2);
        assert_eq!(
            season.episodes[0].name.as_deref(),
            Some("Fullmetal Alchemist")
        );
        assert_eq!(season.episodes[0].still_path.as_deref(), Some("/still.jpg"));
        assert!(response.season(2).is_none());
    }

    /// Television and film name the same things differently; both have to land
    /// in the same fields.
    #[test]
    fn television_and_film_results_normalise_to_the_same_shape() {
        let series: Result_ = serde_json::from_str(
            r#"{"id": 1, "media_type": "tv", "name": "Cowboy Bebop",
                "original_name": "カウボーイビバップ", "first_air_date": "1998-04-03",
                "poster_path": "/p.jpg", "backdrop_path": "/b.jpg", "vote_average": 8.6}"#,
        )
        .expect("parse");
        let series = series.into_candidate();
        assert_eq!(series.metadata.kind, TitleKind::Series);
        assert_eq!(series.metadata.year, Some(1998));
        assert_eq!(
            series.metadata.poster_url.as_deref(),
            Some("https://image.tmdb.org/t/p/w500/p.jpg")
        );
        assert_eq!(
            series.metadata.backdrop_url.as_deref(),
            Some("https://image.tmdb.org/t/p/w1280/b.jpg")
        );

        let film: Result_ = serde_json::from_str(
            r#"{"id": 2, "media_type": "movie", "title": "Akira",
                "release_date": "1988-07-16"}"#,
        )
        .expect("parse");
        let film = film.into_candidate();
        assert_eq!(film.metadata.kind, TitleKind::Film);
        assert_eq!(film.metadata.name, "Akira");
        assert_eq!(film.metadata.year, Some(1988));
    }

    /// Paprika's images as TMDB answers them, trimmed: a bare backdrop beside
    /// better-voted lettered ones, and logos in three languages and two
    /// formats.
    #[test]
    fn the_bare_backdrop_and_a_png_logo_in_the_viewers_language_are_chosen() {
        let images: Images = serde_json::from_str(
            r#"{
                "backdrops": [
                    {"file_path": "/en.jpg", "iso_639_1": "en", "vote_average": 5.5},
                    {"file_path": "/bare.jpg", "iso_639_1": null, "vote_average": 5.2},
                    {"file_path": "/bare-worse.jpg", "iso_639_1": null, "vote_average": 4.0},
                    {"file_path": "/ja.jpg", "iso_639_1": "ja", "vote_average": 6.0}
                ],
                "logos": [
                    {"file_path": "/ja.png", "iso_639_1": "ja", "vote_average": 6.0},
                    {"file_path": "/de.svg", "iso_639_1": "de", "vote_average": 5.0},
                    {"file_path": "/en.png", "iso_639_1": "en", "vote_average": 3.0},
                    {"file_path": "/de.png", "iso_639_1": "de", "vote_average": 1.0}
                ]
            }"#,
        )
        .expect("parse");
        let art = images.into_art("en");
        assert_eq!(
            art.backdrop.as_deref(),
            Some("https://image.tmdb.org/t/p/w1280/bare.jpg")
        );
        assert_eq!(
            art.logo.as_deref(),
            Some("https://image.tmdb.org/t/p/w500/en.png")
        );
    }

    #[test]
    fn a_logo_in_the_viewers_own_language_beats_an_english_one() {
        let images: Images = serde_json::from_str(
            r#"{"logos": [
                {"file_path": "/en.png", "iso_639_1": "en", "vote_average": 9.0},
                {"file_path": "/de.svg", "iso_639_1": "de", "vote_average": 9.0},
                {"file_path": "/de.png", "iso_639_1": "de", "vote_average": 1.0}
            ]}"#,
        )
        .expect("parse");
        assert_eq!(
            images.into_art("de").logo.as_deref(),
            Some("https://image.tmdb.org/t/p/w500/de.png")
        );
    }

    #[test]
    fn an_entry_with_no_images_has_no_art() {
        let images: Images = serde_json::from_str("{}").expect("parse");
        assert_eq!(images.into_art("en"), Art::default());
    }

    /// A rating of zero means "nobody has voted", not "everybody hated it", and
    /// showing it as 0.0/10 would be a lie about the title.
    #[test]
    fn an_unrated_title_reports_no_rating() {
        let result: Result_ = serde_json::from_str(
            r#"{"id": 3, "media_type": "movie", "title": "New", "vote_average": 0.0}"#,
        )
        .expect("parse");
        assert_eq!(result.into_candidate().metadata.rating, None);
    }
}
