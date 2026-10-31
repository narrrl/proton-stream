//! AniSkip: where an episode's opening and ending are, as its viewers marked
//! them.
//!
//! For the files that come with no chapters saying so — which is most of what
//! is not a fansub release. Asked once per episode, when it starts playing, by
//! MyAnimeList id and episode number, with the file's length so the answer is
//! one marked against a cut of the same length. Nothing is stored: the answer
//! is the player's for as long as the file is open. Like ani.zip it needs no
//! key and no account, and it learns nothing about the library that the match
//! did not already send AniList.

use pstr_core::chapters::{ChapterRole, SkipRange};
use serde::Deserialize;

use crate::error::{Error, Result};

const ENDPOINT: &str = "https://api.aniskip.com/v2/skip-times";

/// Where `episode` of the MyAnimeList entry `mal_id` has its opening and its
/// ending, in a cut `length` seconds long. Empty when nobody has marked it.
pub async fn skip_times(
    http: &reqwest::Client,
    mal_id: u32,
    episode: u32,
    length: f64,
) -> Result<Vec<SkipRange>> {
    let response = http
        .get(format!("{ENDPOINT}/{mal_id}/{episode}"))
        .query(&[
            ("types", "op"),
            ("types", "ed"),
            ("episodeLength", &format!("{length:.0}")),
        ])
        .send()
        .await?;
    let status = response.status();
    // "No skip times found" comes back as a 404 with a body saying so.
    if status.as_u16() == 404 {
        return Ok(Vec::new());
    }
    if !status.is_success() {
        return Err(Error::Http(format!("AniSkip answered {status}")));
    }
    let payload: Payload = response.json().await?;
    Ok(payload.ranges())
}

#[derive(Deserialize, Default)]
struct Payload {
    #[serde(default)]
    found: bool,
    #[serde(default)]
    results: Vec<Skip>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Skip {
    interval: Interval,
    skip_type: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Interval {
    start_time: f64,
    end_time: f64,
}

impl Payload {
    fn ranges(self) -> Vec<SkipRange> {
        if !self.found {
            return Vec::new();
        }
        self.results
            .into_iter()
            .filter_map(|skip| {
                let role = match skip.skip_type.as_str() {
                    "op" => ChapterRole::Opening,
                    "ed" => ChapterRole::Ending,
                    _ => return None,
                };
                (skip.interval.end_time > skip.interval.start_time).then_some(SkipRange {
                    role,
                    start: skip.interval.start_time.max(0.0),
                    end: skip.interval.end_time,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_parses_into_an_opening_and_an_ending() {
        let payload: Payload = serde_json::from_str(
            r#"{"found":true,"results":[
                {"interval":{"startTime":1.35,"endTime":91.35},"skipType":"op",
                 "skipId":"9a85542c","episodeLength":1469.979},
                {"interval":{"startTime":1367,"endTime":1459},"skipType":"ed",
                 "skipId":"f6f905e7","episodeLength":1471}],
               "message":"Successfully found skip times","statusCode":200}"#,
        )
        .expect("parse");
        assert_eq!(
            payload.ranges(),
            vec![
                SkipRange {
                    role: ChapterRole::Opening,
                    start: 1.35,
                    end: 91.35,
                },
                SkipRange {
                    role: ChapterRole::Ending,
                    start: 1367.0,
                    end: 1459.0,
                },
            ]
        );
    }

    #[test]
    fn nothing_found_and_kinds_not_asked_for_parse_to_nothing() {
        let none: Payload = serde_json::from_str(
            r#"{"found":false,"results":[],"message":"No skip times found","statusCode":404}"#,
        )
        .expect("parse");
        assert!(none.ranges().is_empty());

        let recap: Payload = serde_json::from_str(
            r#"{"found":true,"results":[{"interval":{"startTime":0,"endTime":60},"skipType":"recap"}]}"#,
        )
        .expect("parse");
        assert!(recap.ranges().is_empty());
    }
}
