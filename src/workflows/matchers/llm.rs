//! Prompt construction and response handling for LLM-assisted subtitle matching.
//! Everything here is pure so it can be tested without a running Ollama.

use serde::Deserialize;

use super::MatchContext;
use crate::domain::models::EpisodeEntry;

/// Episodes either side of the expected position that are still sent to the model.
const LOCALITY_WINDOW: usize = 15;
/// Neighbouring files either side of the current one listed in the prompt.
const NEIGHBOUR_FILES: usize = 5;
const MAX_SUBTITLE_CHARS: usize = 24_000;
const MAX_OVERVIEW_CHARS: usize = 1_000;

const SYSTEM_PROMPT: &str = "You identify which TV episode a subtitle excerpt comes from. \
You are given the show's episode list with descriptions, a listing of the video files from the \
same disc rip, and the subtitle text from the start of the current file. \
The dialogue and events in the subtitles are the main evidence; match them against the episode \
descriptions. Files from a disc rip are usually in episode order, so neighbouring files are \
usually consecutive episodes: use the known matches of nearby files as a strong prior. \
Duplicates are fine: rips often contain several copies of the same episode, so a file may \
legitimately match the same episode as another file, and that is not a reason to doubt it. \
Some files are long and contain several or all of the episodes on the disc back to back; the \
subtitles you are shown only cover the start of the file, so answer with the first episode in \
it and mention in your reasoning if you suspect the file is combined. \
You are also told the length of the current file in minutes and, where known, each episode's \
runtime. Use them as a hint, not proof, since runtimes are approximate: a file about the length \
of one episode is probably a single episode, one far longer is probably combined, and one \
much shorter is probably an extra or a recap. \
The subtitles are OCR output and may contain errors. Only choose from the listed episodes. \
Not every file is an episode: discs also hold extras, featurettes, trailers, recaps, play-all \
tracks and menus, and the subtitles may be empty or too short to tell. \"No match\" is an \
acceptable and often correct answer. If the subtitles do not clearly correspond to one of the \
listed episodes, set matched to false and use 0 for season and episode rather than guessing. \
Give a short reasoning first, then whether it matched, the season, episode and your confidence.";

#[derive(Debug, PartialEq, Eq)]
pub struct LlmMatch {
    pub season: u64,
    pub episode: u64,
    pub confidence: String,
    pub reasoning: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LlmVerdict {
    Match(LlmMatch),
    /// The model decided the file isn't any of the offered episodes.
    NoMatch {
        reasoning: String,
    },
}

#[derive(Debug, Deserialize)]
struct RawAnswer {
    matched: bool,
    #[serde(default)]
    season: u64,
    #[serde(default)]
    episode: u64,
    #[serde(default)]
    confidence: String,
    #[serde(default)]
    reasoning: String,
}

fn position_of(all: &[EpisodeEntry], season: u64, episode: u64) -> Option<usize> {
    all.iter()
        .position(|ep| ep.season_number == season && ep.episode_number == episode)
}

/// Where this file is expected to sit in `all`, as (position of the reference episode, number of
/// files between the reference and this one). The reference is the nearest earlier file that has
/// been matched, or else the user's hint standing in for the first file.
fn expected_reference(all: &[EpisodeEntry], ctx: &MatchContext) -> Option<(usize, usize)> {
    if let Some((anchor_index, anchor)) = ctx.rip.anchor(ctx.index) {
        let pos = position_of(all, anchor.season_number, anchor.episode_number)?;
        return Some((pos, ctx.index - anchor_index));
    }

    let (season, episode) = ctx.rip.hint?;
    Some((position_of(all, season, episode)?, ctx.index))
}

/// Narrows `all` (in aired order) to the episodes near where this file is expected to be, based
/// on the nearest earlier file that has already been matched or, failing that, the user's hint.
/// With neither, returns everything.
pub fn select_candidates(all: &[EpisodeEntry], ctx: &MatchContext) -> Vec<EpisodeEntry> {
    let Some((reference_pos, distance)) = expected_reference(all, ctx) else {
        return all.to_vec();
    };

    let expected = (reference_pos + distance).min(all.len() - 1);
    let start = expected.saturating_sub(LOCALITY_WINDOW);
    let end = (expected + LOCALITY_WINDOW).min(all.len() - 1);
    all[start..=end].to_vec()
}

pub fn build_prompt(
    subtitle_lines: &[String],
    candidates: &[EpisodeEntry],
    ctx: &MatchContext,
    file_minutes: Option<u64>,
) -> (String, String) {
    let mut user = String::new();
    user.push_str(&format!("Show: {}\n\n", ctx.show_name));

    user.push_str(&format!(
        "Files in this rip, in sorted order (current file is {} of {}):\n",
        ctx.index + 1,
        ctx.rip.files.len()
    ));
    let first = ctx.index.saturating_sub(NEIGHBOUR_FILES);
    let last = (ctx.index + NEIGHBOUR_FILES).min(ctx.rip.files.len().saturating_sub(1));
    for i in first..=last {
        let name = ctx.rip.files[i]
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        user.push_str(&format!("{}. {name}", i + 1));
        if let Some(Some(ep)) = ctx.rip.matches.get(i) {
            user.push_str(&format!(
                " -> S{:02}E{:02} \"{}\"",
                ep.season_number, ep.episode_number, ep.name
            ));
        }
        if i == ctx.index {
            user.push_str(" <- CURRENT");
        }
        user.push('\n');
    }

    if let Some(minutes) = file_minutes {
        user.push_str(&format!("\nLength of the current file: {minutes} min.\n"));
    }

    if let (None, Some((season, episode))) = (ctx.rip.anchor(ctx.index), ctx.rip.hint) {
        user.push_str(&format!(
            "\nThe user says the first file in this rip is around S{season:02}E{episode:02}.\n"
        ));
    }

    user.push_str("\nCandidate episodes:\n");
    for ep in candidates {
        user.push_str(&format!(
            "S{:02}E{:02}",
            ep.season_number, ep.episode_number
        ));
        if let Some(aired) = &ep.aired {
            user.push_str(&format!(" | aired {aired}"));
        }
        if let Some(runtime) = ep.runtime {
            user.push_str(&format!(" | {runtime} min"));
        }
        let overview = ep
            .overview
            .as_deref()
            .map(|o| {
                truncate_chars(
                    &o.split_whitespace().collect::<Vec<_>>().join(" "),
                    MAX_OVERVIEW_CHARS,
                )
            })
            .filter(|o| !o.is_empty())
            .unwrap_or_else(|| "(no description)".to_string());
        user.push_str(&format!(" | \"{}\" | {overview}\n", ep.name));
    }

    let subtitles = subtitle_lines.join("\n");
    user.push_str("\nSubtitle text from the current file:\n\"\"\"\n");
    user.push_str(&truncate_chars(subtitles.trim(), MAX_SUBTITLE_CHARS));
    user.push_str("\n\"\"\"\n");

    (SYSTEM_PROMPT.to_string(), user)
}

/// Reasoning comes first so the model works through the evidence before committing to an answer.
pub fn response_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "reasoning": { "type": "string" },
            "matched": { "type": "boolean" },
            "season": { "type": "integer" },
            "episode": { "type": "integer" },
            "confidence": { "type": "string", "enum": ["high", "medium", "low"] }
        },
        "required": ["reasoning", "matched", "season", "episode", "confidence"]
    })
}

/// Parses the model's reply. `None` means it was unusable: malformed, or it claimed a match
/// that isn't one of the episodes offered to it.
pub fn parse_response(content: &str, candidates: &[EpisodeEntry]) -> Option<LlmVerdict> {
    let raw: RawAnswer = serde_json::from_str(content).ok()?;

    if !raw.matched {
        return Some(LlmVerdict::NoMatch {
            reasoning: raw.reasoning,
        });
    }

    let offered = candidates
        .iter()
        .any(|ep| ep.season_number == raw.season && ep.episode_number == raw.episode);
    if !offered {
        return None;
    }

    Some(LlmVerdict::Match(LlmMatch {
        season: raw.season,
        episode: raw.episode,
        confidence: raw.confidence,
        reasoning: raw.reasoning,
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
    use crate::workflows::matchers::RipContext;
    use std::path::PathBuf;

    fn series(seasons: u64, per_season: u64) -> Vec<EpisodeEntry> {
        (1..=seasons)
            .flat_map(|s| (1..=per_season).map(move |e| episode(s, e)))
            .collect()
    }

    fn rip(n: usize) -> RipContext {
        RipContext::new(
            (0..n)
                .map(|i| PathBuf::from(format!("/disc/title_{i:02}.mkv")))
                .collect(),
        )
    }

    fn numbers(eps: &[EpisodeEntry]) -> Vec<(u64, u64)> {
        eps.iter()
            .map(|e| (e.season_number, e.episode_number))
            .collect()
    }

    #[test]
    fn test_select_candidates_without_anchor_returns_all() {
        let all = series(2, 24);
        let rip = rip(6);
        let ctx = MatchContext {
            rip: &rip,
            index: 0,
            show_name: "Show",
        };
        assert_eq!(select_candidates(&all, &ctx).len(), 48);
    }

    #[test]
    fn test_select_candidates_windows_around_expected_position() {
        let all = series(3, 24);
        let mut rip = rip(10);
        // File 2 matched S02E05 (position 28); file 4 is two files later -> position 30.
        rip.matches[2] = Some(episode(2, 5));
        let ctx = MatchContext {
            rip: &rip,
            index: 4,
            show_name: "Show",
        };

        let got = numbers(&select_candidates(&all, &ctx));
        assert_eq!(got.len(), 31);
        assert_eq!(got[15], (2, 7)); // the expected position, centred
        assert_eq!(got[0], (1, 16));
        assert_eq!(*got.last().unwrap(), (2, 22));
    }

    #[test]
    fn test_select_candidates_clamps_to_ends() {
        let all = series(1, 40);

        let mut near_start = rip(4);
        near_start.matches[0] = Some(episode(1, 2));
        let ctx = MatchContext {
            rip: &near_start,
            index: 1,
            show_name: "Show",
        };
        let got = numbers(&select_candidates(&all, &ctx));
        assert_eq!(got.first(), Some(&(1, 1)));
        assert_eq!(got.len(), 18); // expected position 2: 0..=17

        let mut near_end = rip(30);
        near_end.matches[0] = Some(episode(1, 39));
        let ctx = MatchContext {
            rip: &near_end,
            index: 10,
            show_name: "Show",
        };
        let got = numbers(&select_candidates(&all, &ctx));
        assert_eq!(got.last(), Some(&(1, 40)));
        assert_eq!(got.first(), Some(&(1, 25))); // expected position clamps to 39: 24..=39
    }

    #[test]
    fn test_select_candidates_unknown_anchor_returns_all() {
        let all = series(1, 10);
        let mut rip = rip(3);
        rip.matches[0] = Some(episode(9, 9));
        let ctx = MatchContext {
            rip: &rip,
            index: 1,
            show_name: "Show",
        };
        assert_eq!(select_candidates(&all, &ctx).len(), 10);
    }

    #[test]
    fn test_select_candidates_uses_hint_without_anchor() {
        let all = series(3, 24);
        // The hint (S02E05, position 28) stands in for the first file, so file 2 is expected
        // two episodes later at position 30.
        let rip = rip(10).with_hint(Some((2, 5)));
        let ctx = MatchContext {
            rip: &rip,
            index: 2,
            show_name: "Show",
        };

        let got = numbers(&select_candidates(&all, &ctx));
        assert_eq!(got.len(), 31);
        assert_eq!(got[15], (2, 7));

        // The first file is expected right at the hint.
        let ctx = MatchContext {
            rip: &rip,
            index: 0,
            show_name: "Show",
        };
        let got = numbers(&select_candidates(&all, &ctx));
        assert_eq!(got[15], (2, 5));
    }

    #[test]
    fn test_select_candidates_anchor_beats_hint() {
        let all = series(3, 24);
        let mut rip = rip(10).with_hint(Some((1, 1)));
        rip.matches[2] = Some(episode(3, 10));
        let ctx = MatchContext {
            rip: &rip,
            index: 3,
            show_name: "Show",
        };

        let got = numbers(&select_candidates(&all, &ctx));
        assert_eq!(got[15], (3, 11));
    }

    #[test]
    fn test_select_candidates_unknown_hint_returns_all() {
        let all = series(1, 10);
        let rip = rip(3).with_hint(Some((9, 9)));
        let ctx = MatchContext {
            rip: &rip,
            index: 1,
            show_name: "Show",
        };
        assert_eq!(select_candidates(&all, &ctx).len(), 10);
    }

    #[test]
    fn test_build_prompt_mentions_hint_only_without_anchor() {
        let mut rip = rip(3).with_hint(Some((2, 5)));
        let ctx = MatchContext {
            rip: &rip,
            index: 1,
            show_name: "Show",
        };
        let (_, user) = build_prompt(&[], &[episode(2, 5)], &ctx, None);
        assert!(user.contains("first file in this rip is around S02E05"));

        rip.matches[0] = Some(episode(2, 5));
        let ctx = MatchContext {
            rip: &rip,
            index: 1,
            show_name: "Show",
        };
        let (_, user) = build_prompt(&[], &[episode(2, 6)], &ctx, None);
        assert!(!user.contains("first file in this rip"));
    }

    #[test]
    fn test_system_prompt_allows_duplicates_and_combined_files() {
        let rip = rip(1);
        let ctx = MatchContext {
            rip: &rip,
            index: 0,
            show_name: "Show",
        };
        let (system, _) = build_prompt(&[], &[episode(1, 1)], &ctx, None);
        assert!(system.contains("Duplicates are fine"));
        assert!(system.contains("several copies of the same episode"));
        assert!(system.contains("all of the episodes on the disc back to back"));
        assert!(system.contains("answer with the first episode"));
        // The old rule against reusing an already-matched episode must be gone.
        assert!(!system.contains("already matched"));
    }

    #[test]
    fn test_build_prompt_includes_file_length_and_runtimes() {
        let rip = rip(2);
        let ctx = MatchContext {
            rip: &rip,
            index: 1,
            show_name: "Show",
        };
        let mut with_runtime = episode(1, 1);
        with_runtime.aired = Some("1993-09-10".to_string());
        with_runtime.runtime = Some(45);
        let mut without_runtime = episode(1, 2);
        without_runtime.runtime = None;

        let (system, user) = build_prompt(&[], &[with_runtime, without_runtime], &ctx, Some(92));

        assert!(user.contains("Length of the current file: 92 min."));
        assert!(user.contains("S01E01 | aired 1993-09-10 | 45 min | \"Episode 1x1\""));
        assert!(user.contains("S01E02 | \"Episode 1x2\""));
        assert!(system.contains("length of the current file in minutes"));
        assert!(system.contains("far longer is probably combined"));
    }

    #[test]
    fn test_build_prompt_omits_file_length_when_unknown() {
        let rip = rip(1);
        let ctx = MatchContext {
            rip: &rip,
            index: 0,
            show_name: "Show",
        };
        let (_, user) = build_prompt(&[], &[episode(1, 1)], &ctx, None);
        assert!(!user.contains("Length of the current file"));
    }

    #[test]
    fn test_build_prompt_contents() {
        let mut rip = rip(3);
        rip.matches[0] = Some(episode(1, 1));
        let ctx = MatchContext {
            rip: &rip,
            index: 1,
            show_name: "The Show",
        };
        let mut candidates = vec![episode(1, 2), episode(1, 3)];
        candidates[0].aired = Some("1993-09-24".to_string());
        candidates[1].overview = None;
        let lines = vec!["Hello there.".to_string(), "General Kenobi.".to_string()];

        let (system, user) = build_prompt(&lines, &candidates, &ctx, None);

        assert!(system.contains("neighbouring files"));
        assert!(user.contains("Show: The Show"));
        assert!(user.contains("current file is 2 of 3"));
        assert!(user.contains("1. title_00.mkv -> S01E01 \"Episode 1x1\""));
        assert!(user.contains("2. title_01.mkv <- CURRENT"));
        assert!(!user.contains("3. title_02.mkv ->"));
        assert!(user.contains("S01E02 | aired 1993-09-24 | \"Episode 1x2\" | Overview of 1x2"));
        assert!(user.contains("S01E03 | \"Episode 1x3\" | (no description)"));
        assert!(user.contains("Hello there.\nGeneral Kenobi."));
    }

    #[test]
    fn test_build_prompt_truncates_long_input() {
        let rip = rip(1);
        let ctx = MatchContext {
            rip: &rip,
            index: 0,
            show_name: "Show",
        };
        let mut ep = episode(1, 1);
        ep.overview = Some("word ".repeat(500));
        let lines = vec!["a".repeat(MAX_SUBTITLE_CHARS * 2)];

        let (_, user) = build_prompt(&lines, &[ep], &ctx, None);

        assert!(user.chars().count() < MAX_SUBTITLE_CHARS + MAX_OVERVIEW_CHARS + 1_000);
        assert!(user.contains('…'));
    }

    #[test]
    fn test_build_prompt_only_lists_nearby_files() {
        let rip = rip(40);
        let ctx = MatchContext {
            rip: &rip,
            index: 20,
            show_name: "Show",
        };
        let (_, user) = build_prompt(&[], &[episode(1, 1)], &ctx, None);
        // Index 20 shows files 15..=25, numbered from 1.
        assert!(user.contains("16. title_15.mkv"));
        assert!(user.contains("26. title_25.mkv"));
        assert!(!user.contains("title_14.mkv"));
        assert!(!user.contains("title_26.mkv"));
    }

    #[test]
    fn test_parse_response() {
        let candidates = vec![episode(1, 2), episode(1, 3)];

        let ok = parse_response(
            r#"{"reasoning":"Mentions a liver","matched":true,"season":1,"episode":3,"confidence":"high"}"#,
            &candidates,
        );
        assert_eq!(
            ok,
            Some(LlmVerdict::Match(LlmMatch {
                season: 1,
                episode: 3,
                confidence: "high".to_string(),
                reasoning: "Mentions a liver".to_string(),
            }))
        );

        assert!(parse_response("not json", &candidates).is_none());
        assert!(parse_response(r#"{"season":1}"#, &candidates).is_none());
        // Claims a match, but not with an episode we offered.
        assert!(parse_response(
            r#"{"reasoning":"","matched":true,"season":5,"episode":5,"confidence":"low"}"#,
            &candidates
        )
        .is_none());
    }

    #[test]
    fn test_parse_response_no_match() {
        let candidates = vec![episode(1, 2)];

        // Season/episode are ignored when the model says there is no match, even if they
        // happen to name an offered episode or are left out entirely.
        for content in [
            r#"{"reasoning":"Behind the scenes featurette","matched":false,"season":0,"episode":0,"confidence":"high"}"#,
            r#"{"reasoning":"Behind the scenes featurette","matched":false,"season":1,"episode":2,"confidence":"low"}"#,
            r#"{"reasoning":"Behind the scenes featurette","matched":false}"#,
        ] {
            assert_eq!(
                parse_response(content, &candidates),
                Some(LlmVerdict::NoMatch {
                    reasoning: "Behind the scenes featurette".to_string()
                })
            );
        }
    }

    #[test]
    fn test_prompt_and_schema_allow_no_match() {
        let rip = rip(1);
        let ctx = MatchContext {
            rip: &rip,
            index: 0,
            show_name: "Show",
        };
        let (system, _) = build_prompt(&[], &[episode(1, 1)], &ctx, None);
        assert!(system.contains("\"No match\" is an acceptable"));
        assert!(system.contains("matched to false"));

        let schema = response_schema();
        assert_eq!(schema["properties"]["matched"]["type"], "boolean");
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("matched")));
    }
}
