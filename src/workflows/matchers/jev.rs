//! Request construction and response handling for Jev-assisted subtitle matching.
//! Everything here is pure so it can be tested without calling Jev.

use serde::Deserialize;
use std::collections::HashMap;

use crate::domain::models::EpisodeEntry;

const MAX_SUBTITLE_CHARS: usize = 24_000;
const MAX_OVERVIEW_CHARS: usize = 1_000;

/// The option Jev picks when the file isn't any of the offered episodes.
pub const NO_MATCH: &str = "NO MATCH";

/// The one question asked of Jev, keyed by this name in the request and the response.
const QUESTION: &str = "episode";

const INSTRUCTIONS: &str = "Which TV episode is this subtitle excerpt from? The excerpt is \
OCR'd dialogue and may come from any point within the episode. Each option is an episode, \
described by its plot; pick NO MATCH if it is none of them, such as an extra, trailer or menu.";

#[derive(Debug, PartialEq)]
pub struct JevMatch {
    pub season: u64,
    pub episode: u64,
    /// How likely Jev rates its pick, from 0 to 1.
    pub probability: f64,
    /// Every option with its probability, most likely first.
    pub ranked: Vec<(String, f64)>,
}

#[derive(Debug, PartialEq)]
pub enum JevVerdict {
    Match(JevMatch),
    /// Jev decided the file isn't any of the offered episodes.
    NoMatch {
        ranked: Vec<(String, f64)>,
    },
}

#[derive(Debug, Deserialize)]
struct RawResponse {
    answers: HashMap<String, RawAnswer>,
}

#[derive(Debug, Deserialize)]
struct RawAnswer {
    choice: String,
    #[serde(default)]
    probabilities: HashMap<String, f64>,
    confidence: Option<f64>,
}

/// How Jev refers to an episode: the option names, and the only answers it can give.
pub fn episode_label(ep: &EpisodeEntry) -> String {
    let name = ep.name.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("S{:02}E{:02} - {name}", ep.season_number, ep.episode_number)
}

/// The episodes offered to Jev: one season if the user named one, otherwise the whole series.
/// Specials are mostly extras and short webisodes that crowd out the real episodes, so they are
/// left out unless asked for (or named as the season), or unless the series has nothing else.
pub fn select_candidates(
    all: &[EpisodeEntry],
    season: Option<u64>,
    include_specials: bool,
) -> Vec<EpisodeEntry> {
    let has_regular = all.iter().any(|ep| ep.season_number != 0);
    all.iter()
        .filter(|ep| match season {
            Some(season) => ep.season_number == season,
            None => include_specials || !has_regular || ep.season_number != 0,
        })
        .cloned()
        .collect()
}

/// A multiple-choice question over the candidates, each described by its TVDB overview, with
/// the subtitles as the text to judge. serde_json's `preserve_order` keeps the options in airing
/// order with NO MATCH last.
pub fn build_request(
    model: &str,
    subtitle_lines: &[String],
    candidates: &[EpisodeEntry],
    show_name: &str,
) -> serde_json::Value {
    let mut criteria = serde_json::Map::new();
    for ep in candidates {
        let overview = ep
            .overview
            .as_deref()
            .map(|o| {
                truncate_chars(
                    &o.split_whitespace().collect::<Vec<_>>().join(" "),
                    MAX_OVERVIEW_CHARS,
                )
            })
            .filter(|o| !o.is_empty());
        criteria.insert(episode_label(ep), overview.into());
    }
    criteria.insert(NO_MATCH.to_string(), "None of these episodes.".into());

    let state = format!(
        "Show: {show_name}\n\nSubtitle excerpt:\n{}",
        truncate_chars(&subtitle_text(subtitle_lines), MAX_SUBTITLE_CHARS)
    );

    serde_json::json!({
        "model": model,
        "state": state,
        "questions": {
            QUESTION: {
                "type": "choice",
                "instructions": INSTRUCTIONS,
                "criteria": criteria,
            }
        }
    })
}

/// The subtitles as Jev sees them. Caption positioning leaves indentation and blank lines that
/// only cost tokens.
fn subtitle_text(subtitle_lines: &[String]) -> String {
    subtitle_lines
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parses Jev's reply. `Err` carries why it was unusable: malformed, or a choice that isn't one
/// of the episodes offered.
pub fn parse_response(body: &str, candidates: &[EpisodeEntry]) -> Result<JevVerdict, String> {
    let mut raw: RawResponse =
        serde_json::from_str(body).map_err(|e| format!("the reply was not valid: {e}"))?;
    let answer = raw
        .answers
        .remove(QUESTION)
        .ok_or_else(|| "the reply had no answer to the question".to_string())?;

    let mut ranked: Vec<(String, f64)> = answer.probabilities.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));

    if answer.choice == NO_MATCH {
        return Ok(JevVerdict::NoMatch { ranked });
    }

    let Some(candidate) = candidates
        .iter()
        .find(|ep| episode_label(ep) == answer.choice)
    else {
        return Err(format!(
            "\"{}\" is not one of the episodes it was offered",
            answer.choice
        ));
    };

    let probability = ranked
        .iter()
        .find(|(label, _)| *label == answer.choice)
        .map(|&(_, p)| p)
        .or(answer.confidence)
        .unwrap_or(0.0);

    Ok(JevVerdict::Match(JevMatch {
        season: candidate.season_number,
        episode: candidate.episode_number,
        probability,
        ranked,
    }))
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(max).collect();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflows::matchers::tests::episode;

    fn series(seasons: u64, per_season: u64) -> Vec<EpisodeEntry> {
        (1..=seasons)
            .flat_map(|s| (1..=per_season).map(move |e| episode(s, e)))
            .collect()
    }

    fn numbers(eps: &[EpisodeEntry]) -> Vec<(u64, u64)> {
        eps.iter()
            .map(|e| (e.season_number, e.episode_number))
            .collect()
    }

    #[test]
    fn test_select_candidates_without_season_returns_the_series() {
        let all = series(2, 24);
        assert_eq!(select_candidates(&all, None, false).len(), 48);
    }

    #[test]
    fn test_select_candidates_keeps_only_the_named_season() {
        let all = series(3, 24);
        let got = numbers(&select_candidates(&all, Some(2), false));
        assert_eq!(got.len(), 24);
        assert!(got.iter().all(|&(season, _)| season == 2));
        assert!(select_candidates(&all, Some(9), false).is_empty());
    }

    #[test]
    fn test_select_candidates_specials() {
        let mut all = series(1, 3);
        all.push(episode(0, 1));

        assert_eq!(select_candidates(&all, None, false).len(), 3);
        assert_eq!(select_candidates(&all, None, true).len(), 4);
        // Naming season 0 asks for the specials.
        assert_eq!(
            numbers(&select_candidates(&all, Some(0), false)),
            vec![(0, 1)]
        );
        // A series with nothing but specials keeps them.
        let specials = vec![episode(0, 1), episode(0, 2)];
        assert_eq!(select_candidates(&specials, None, false).len(), 2);
    }

    #[test]
    fn test_build_request() {
        let mut candidates = vec![episode(1, 2), episode(1, 3)];
        candidates[1].overview = None;
        let lines = vec![
            "    WHAT'S THAT?   ".to_string(),
            String::new(),
            "        THE GHOST!".to_string(),
        ];

        let request = build_request("jev-latest", &lines, &candidates, "The Show");

        assert_eq!(request["model"], "jev-latest");
        assert_eq!(
            request["state"],
            "Show: The Show\n\nSubtitle excerpt:\nWHAT'S THAT?\nTHE GHOST!"
        );
        let question = &request["questions"]["episode"];
        assert_eq!(question["type"], "choice");
        assert!(question["instructions"]
            .as_str()
            .unwrap()
            .contains("any point within the episode"));
        let criteria = question["criteria"].as_object().unwrap();
        // In airing order, with NO MATCH last and a missing overview left empty.
        assert_eq!(
            criteria.keys().collect::<Vec<_>>(),
            ["S01E02 - Episode 1x2", "S01E03 - Episode 1x3", "NO MATCH"]
        );
        assert_eq!(criteria["S01E02 - Episode 1x2"], "Overview of 1x2");
        assert!(criteria["S01E03 - Episode 1x3"].is_null());
    }

    #[test]
    fn test_build_request_truncates_long_input() {
        let mut ep = episode(1, 1);
        ep.overview = Some("word ".repeat(500));
        let lines = vec!["a".repeat(MAX_SUBTITLE_CHARS * 2)];

        let request = build_request("m", &lines, &[ep], "Show");

        let state = request["state"].as_str().unwrap();
        assert!(state.chars().count() < MAX_SUBTITLE_CHARS + 100);
        assert!(state.ends_with('…'));
        let overview = request["questions"]["episode"]["criteria"]["S01E01 - Episode 1x1"]
            .as_str()
            .unwrap();
        assert_eq!(overview.chars().count(), MAX_OVERVIEW_CHARS + 1);
    }

    fn reply(choice: &str) -> String {
        format!(
            r#"{{"model":"jev-1.14.0","answers":{{"episode":{{"type":"choice","choice":"{choice}",
            "probabilities":{{"S01E02 - Episode 1x2":0.1,"S01E03 - Episode 1x3":0.85,"NO MATCH":0.05}},
            "confidence":0.8}}}},"usage":{{"input_tokens":10,"output_tokens":1}}}}"#
        )
    }

    #[test]
    fn test_parse_response() {
        let candidates = vec![episode(1, 2), episode(1, 3)];

        let verdict = parse_response(&reply("S01E03 - Episode 1x3"), &candidates).unwrap();
        let JevVerdict::Match(found) = verdict else {
            panic!("expected a match, got {verdict:?}");
        };
        assert_eq!((found.season, found.episode), (1, 3));
        assert_eq!(found.probability, 0.85);
        let order: Vec<&str> = found.ranked.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(
            order,
            ["S01E03 - Episode 1x3", "S01E02 - Episode 1x2", "NO MATCH"]
        );

        assert!(parse_response("not json", &candidates).is_err());
        assert!(parse_response(r#"{"answers":{}}"#, &candidates).is_err());
        // Not an episode we offered.
        let err = parse_response(&reply("S05E05 - Episode 5x5"), &candidates).unwrap_err();
        assert!(err.contains("S05E05"), "{err}");
    }

    #[test]
    fn test_parse_response_falls_back_to_confidence() {
        let body = r#"{"answers":{"episode":{"choice":"S01E02 - Episode 1x2","confidence":0.7}}}"#;
        let Ok(JevVerdict::Match(found)) = parse_response(body, &[episode(1, 2)]) else {
            panic!("expected a match");
        };
        assert_eq!(found.probability, 0.7);
        assert!(found.ranked.is_empty());
    }

    #[test]
    fn test_parse_response_no_match() {
        let verdict = parse_response(&reply("NO MATCH"), &[episode(1, 2)]).unwrap();
        let JevVerdict::NoMatch { ranked } = verdict else {
            panic!("expected no match, got {verdict:?}");
        };
        assert_eq!(ranked[0].0, "S01E03 - Episode 1x3");
    }

    #[test]
    fn test_episode_label() {
        let mut ep = episode(2, 1);
        ep.name = "  Nowhere   to\nHyde ".to_string();
        assert_eq!(episode_label(&ep), "S02E01 - Nowhere to Hyde");
    }
}
