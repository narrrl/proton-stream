//! TVmaze, over its public REST API.
//!
//! Television AniList does not have — which is everything that is not anime —
//! with no API key and no account. It is asked only about what AniList found
//! nothing for (see [`crate::service`]), so a library of anime sends it
//! nothing, and a library with one live-action show in it sends it that one
//! name.
//!
//! What it does not have is film: TVmaze indexes shows and nothing else, so a
//! title the library knows to be a film is not asked about at all. A show's
//! name is also a film's often enough that asking would mostly find the wrong
//! thing.
//!
//! Its art is good where it matters here: a 16:9 still for most episodes and a
//! full-HD background for most shows. Its "typography" image would be a logo
//! but is a JPEG on a solid ground, so there is no logo from here.

use pstr_core::library::{Title, TitleKind};
use pstr_core::metadata::{Enrichment, EpisodeMetadata, ProviderId, TitleDetails, TitleMetadata};
use serde::Deserialize;

use crate::anilist::strip_markup;
use crate::error::{Error, Result};
use crate::limiter::RateLimiter;
use crate::matching::{Candidate, Query};
use crate::provider::Provider;

const API_BASE: &str = "https://api.tvmaze.com";

/// TVmaze allows at least twenty calls every ten seconds per address. A little
/// under it, because the limit is counted on its side and a burst that lands
/// at the edge of its window gets a 429 for nothing.
const REQUESTS_PER_MINUTE: u32 = 100;

/// As [`crate::anilist`]'s.
const RETRIES: usize = 2;

/// The background width worth asking for. Shows often carry a 4K one beside
/// it, which is four times the bytes for a band no window draws that wide.
const BACKDROP_WIDTH: u32 = 1920;

pub struct TvMaze {
    http: reqwest::Client,
    limiter: RateLimiter,
}

impl TvMaze {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            limiter: RateLimiter::per_minute(REQUESTS_PER_MINUTE),
        }
    }
}

impl Provider for TvMaze {
    fn id(&self) -> ProviderId {
        ProviderId::TvMaze
    }

    async fn search(&self, query: &Query) -> Result<Vec<Candidate>> {
        if !asks_about(query) {
            return Ok(Vec::new());
        }
        let hits: Vec<SearchHit> = self
            .get("/search/shows", &[("q", query.name.as_str())])
            .await?
            .unwrap_or_default();
        Ok(hits
            .into_iter()
            .map(|hit| hit.show.into_candidate())
            .collect())
    }

    /// Every episode and the show's background, in one request.
    async fn enrich(&self, _title: &Title, found: &TitleMetadata) -> Result<Enrichment> {
        let show: Option<Show> = self
            .get(
                &format!("/shows/{}", found.remote_id),
                &[("embed[]", "episodes"), ("embed[]", "images")],
            )
            .await?;
        Ok(show.map(Show::into_enrichment).unwrap_or_default())
    }
}

impl TvMaze {
    /// The show the TVmaze id `id` names, as search would have found it.
    /// `None` for an id TVmaze no longer has.
    pub async fn entry(&self, id: &str) -> Result<Option<TitleMetadata>> {
        let show: Option<Show> = self.get(&format!("/shows/{id}"), &[]).await?;
        Ok(show.map(|show| show.into_candidate().metadata))
    }

    /// One GET, paced. `None` for a 404, which TVmaze answers for an id it
    /// does not have.
    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<Option<T>> {
        let mut attempts = 0usize;
        let response = loop {
            self.limiter.acquire().await;
            let response = self
                .http
                .get(format!("{API_BASE}{path}"))
                .query(query)
                .send()
                .await?;
            let status = response.status();

            if status.as_u16() == 429 {
                let waited = self.limiter.throttled(response.headers()).await;
                attempts += 1;
                if attempts > RETRIES {
                    return Err(Error::RateLimited);
                }
                tracing::debug!("TVmaze is throttling; waiting {waited:?}");
                continue;
            }
            if status.as_u16() == 404 {
                return Ok(None);
            }
            if !status.is_success() {
                return Err(Error::Http(format!("TVmaze answered {status}")));
            }
            break response;
        };
        Ok(Some(response.json().await?))
    }
}

/// Whether TVmaze could have `query` at all: not when the library knows it is
/// a film.
fn asks_about(query: &Query) -> bool {
    !(query.kind_known && query.kind == TitleKind::Film)
}

#[derive(Deserialize)]
struct SearchHit {
    show: Show,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Show {
    id: i64,
    name: String,
    url: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
    status: Option<String>,
    runtime: Option<u32>,
    average_runtime: Option<u32>,
    premiered: Option<String>,
    rating: Option<Rating>,
    network: Option<Channel>,
    web_channel: Option<Channel>,
    image: Option<Image>,
    summary: Option<String>,
    #[serde(rename = "_embedded")]
    embedded: Option<Embedded>,
}

#[derive(Deserialize)]
struct Rating {
    average: Option<f32>,
}

#[derive(Deserialize)]
struct Channel {
    name: String,
}

#[derive(Deserialize)]
struct Image {
    medium: Option<String>,
    original: Option<String>,
}

#[derive(Deserialize, Default)]
struct Embedded {
    #[serde(default)]
    episodes: Vec<Episode>,
    #[serde(default)]
    images: Vec<Picture>,
}

#[derive(Deserialize)]
struct Episode {
    name: Option<String>,
    season: Option<u32>,
    /// `null` on a special, which is numbered nowhere a file name would say.
    number: Option<u32>,
    airdate: Option<String>,
    runtime: Option<u32>,
    image: Option<Image>,
    summary: Option<String>,
}

#[derive(Deserialize)]
struct Picture {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    main: bool,
    resolutions: Resolutions,
}

#[derive(Deserialize)]
struct Resolutions {
    original: Option<Resolution>,
}

#[derive(Deserialize)]
struct Resolution {
    url: String,
    width: Option<u32>,
}

impl Show {
    fn into_candidate(self) -> Candidate {
        let status = match self.status.as_deref() {
            Some("Running") => Some("RELEASING"),
            Some("Ended") => Some("FINISHED"),
            Some("To Be Determined") => Some("HIATUS"),
            Some("In Development") => Some("NOT_YET_RELEASED"),
            _ => None,
        };
        let channel = self.network.or(self.web_channel);
        let details = TitleDetails {
            start_date: self.premiered.clone().filter(|date| !date.is_empty()),
            // Not a studio, but what the facts line names in its place: who
            // made it, as a viewer would put it.
            studios: channel.map(|channel| channel.name).into_iter().collect(),
            status: status.map(str::to_string),
            runtime: self
                .runtime
                .or(self.average_runtime)
                .filter(|minutes| *minutes > 0),
            revision: TitleDetails::REVISION,
            ..TitleDetails::default()
        };

        Candidate {
            aliases: vec![self.name.clone()],
            metadata: TitleMetadata {
                provider: ProviderId::TvMaze,
                remote_id: self.id.to_string(),
                year: self
                    .premiered
                    .as_deref()
                    .and_then(|date| date.get(..4)?.parse().ok()),
                name: self.name,
                original_name: None,
                overview: self.summary.as_deref().and_then(overview),
                kind: TitleKind::Series,
                poster_url: self.image.and_then(|image| image.original.or(image.medium)),
                backdrop_url: None,
                logo_url: None,
                rating: self
                    .rating
                    .and_then(|rating| rating.average)
                    .filter(|rating| *rating > 0.0),
                genres: self.genres,
                episodes: None,
                url: self.url,
                details: Some(details),
            },
            // Always: TVmaze has nothing but shows.
            kind_known: true,
        }
    }

    fn into_enrichment(self) -> Enrichment {
        let embedded = self.embedded.unwrap_or_default();
        let episodes = embedded
            .episodes
            .into_iter()
            .filter_map(|episode| {
                Some(EpisodeMetadata {
                    season: episode.season,
                    number: episode.number?,
                    absolute: None,
                    entry_season: None,
                    entry_number: None,
                    name: episode.name.filter(|name| !name.is_empty()),
                    overview: episode.summary.as_deref().and_then(overview),
                    // The row draws a still about this wide; the original is
                    // a full-HD frame per episode.
                    still_url: episode
                        .image
                        .and_then(|image| image.medium.or(image.original)),
                    air_date: episode.airdate.filter(|date| !date.is_empty()),
                    runtime: episode.runtime.filter(|minutes| *minutes > 0),
                    mal_id: None,
                })
            })
            .collect();
        Enrichment {
            episodes,
            backdrop_url: backdrop(&embedded.images),
            logo_url: None,
            chain: Vec::new(),
        }
    }
}

/// The background to draw: the show's main one if it marked one, else the one
/// nearest [`BACKDROP_WIDTH`].
fn backdrop(images: &[Picture]) -> Option<String> {
    images
        .iter()
        .filter(|picture| picture.kind.as_deref() == Some("background"))
        .filter_map(|picture| Some((picture, picture.resolutions.original.as_ref()?)))
        .min_by_key(|(picture, resolution)| {
            (
                !picture.main,
                resolution.width.unwrap_or(0).abs_diff(BACKDROP_WIDTH),
            )
        })
        .map(|(_, resolution)| resolution.url.clone())
}

/// TVmaze's summaries are HTML paragraphs. Each one keeps its own line.
fn overview(html: &str) -> Option<String> {
    let text = strip_markup(&html.replace("</p>", "</p>\n"));
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOW: &str = r#"{
        "id": 82, "url": "https://www.tvmaze.com/shows/82/game-of-thrones",
        "name": "Game of Thrones", "type": "Scripted",
        "genres": ["Drama", "Adventure", "Fantasy"], "status": "Ended",
        "runtime": 60, "averageRuntime": 61, "premiered": "2011-04-17",
        "rating": {"average": 8.9}, "network": {"id": 8, "name": "HBO"},
        "webChannel": null,
        "image": {"medium": "https://static.tvmaze.com/m/1.jpg",
                  "original": "https://static.tvmaze.com/o/1.jpg"},
        "summary": "<p>Based on the novels.</p><p>Seven noble families fight.</p>",
        "_embedded": {
            "episodes": [
                {"name": "Winter is Coming", "season": 1, "number": 1,
                 "type": "regular", "airdate": "2011-04-17", "runtime": 60,
                 "image": {"medium": "https://static.tvmaze.com/m/e1.jpg",
                           "original": "https://static.tvmaze.com/o/e1.jpg"},
                 "summary": "<p>Lord Stark is troubled.</p>"},
                {"name": "The Kingsroad", "season": 1, "number": 2,
                 "type": "regular", "airdate": "2011-04-24", "runtime": null,
                 "image": null, "summary": null},
                {"name": "Inside the Making", "season": 1, "number": null,
                 "type": "significant_special", "airdate": "2011-04-10"}
            ],
            "images": [
                {"type": "poster", "main": true, "resolutions":
                    {"original": {"url": "poster", "width": 2030}}},
                {"type": "background", "main": false, "resolutions":
                    {"original": {"url": "uhd", "width": 3840}}},
                {"type": "background", "main": false, "resolutions":
                    {"original": {"url": "hd", "width": 1920}}},
                {"type": "typography", "main": false, "resolutions":
                    {"original": {"url": "lettering", "width": 840}}}
            ]
        }
    }"#;

    #[test]
    fn a_show_becomes_a_series_with_its_details() {
        let show: Show = serde_json::from_str(SHOW).expect("parse");
        let found = show.into_candidate().metadata;
        assert_eq!(found.provider, ProviderId::TvMaze);
        assert_eq!(found.remote_id, "82");
        assert_eq!(found.kind, TitleKind::Series);
        assert_eq!(found.year, Some(2011));
        assert_eq!(found.rating, Some(8.9));
        assert_eq!(
            found.poster_url.as_deref(),
            Some("https://static.tvmaze.com/o/1.jpg")
        );
        assert_eq!(
            found.overview.as_deref(),
            Some("Based on the novels.\nSeven noble families fight.")
        );

        let details = found.details.expect("details");
        assert!(details.is_current());
        assert_eq!(details.studios, vec!["HBO"]);
        assert_eq!(details.status.as_deref(), Some("FINISHED"));
        assert_eq!(details.runtime, Some(60));
        assert_eq!(details.start_date.as_deref(), Some("2011-04-17"));
    }

    /// Specials are numbered nowhere a file would be, so they are left out
    /// rather than colliding with the episode they share a season with.
    #[test]
    fn enrichment_keeps_numbered_episodes_and_the_full_hd_background() {
        let show: Show = serde_json::from_str(SHOW).expect("parse");
        let enrichment = show.into_enrichment();
        assert_eq!(enrichment.episodes.len(), 2);

        let first = &enrichment.episodes[0];
        assert_eq!((first.season, first.number), (Some(1), 1));
        assert_eq!(first.name.as_deref(), Some("Winter is Coming"));
        assert_eq!(first.overview.as_deref(), Some("Lord Stark is troubled."));
        assert_eq!(
            first.still_url.as_deref(),
            Some("https://static.tvmaze.com/m/e1.jpg")
        );
        assert_eq!(first.runtime, Some(60));
        assert_eq!(enrichment.episodes[1].runtime, None);

        assert_eq!(enrichment.backdrop_url.as_deref(), Some("hd"));
        assert_eq!(enrichment.logo_url, None);
    }

    #[test]
    fn a_background_the_show_marked_main_wins_over_a_better_sized_one() {
        let images: Vec<Picture> = serde_json::from_str(
            r#"[{"type": "background", "main": false, "resolutions":
                    {"original": {"url": "hd", "width": 1920}}},
                {"type": "background", "main": true, "resolutions":
                    {"original": {"url": "chosen", "width": 1280}}}]"#,
        )
        .expect("parse");
        assert_eq!(backdrop(&images).as_deref(), Some("chosen"));
        assert_eq!(backdrop(&[]), None);
    }

    /// TVmaze has no films, and a show sharing a film's name is the wrong
    /// answer far more often than it is an adaptation.
    #[test]
    fn a_known_film_is_not_asked_about_and_a_guessed_one_is() {
        let film = Query::new("Paprika", None, TitleKind::Film);
        assert!(!asks_about(&film));
        assert!(asks_about(&film.with_guessed_kind()));
        assert!(asks_about(&Query::new(
            "Game of Thrones",
            None,
            TitleKind::Series
        )));
    }
}
