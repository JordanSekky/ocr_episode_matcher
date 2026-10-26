//! Prompt construction and response handling for LLM-assisted subtitle matching.
//! Everything here is pure so it can be tested without a running Ollama.

use serde::Deserialize;

use super::MatchContext;
use crate::domain::models::EpisodeEntry;

/// Episodes either side of the expected position that are still sent to the model.
const LOCALITY_WINDOW: usize = 7;
/// Neighbouring files either side of the current one listed in the prompt.
const NEIGHBOUR_FILES: usize = 5;
const MAX_SUBTITLE_CHARS: usize = 24_000;
const MAX_OVERVIEW_CHARS: usize = 1_000;

/// What the model answers when the file isn't any of the offered episodes.
pub const NO_MATCH: &str = "NO MATCH";

const SYSTEM_PROMPT: &str = "You identify which TV episode a subtitle excerpt comes from. \
You are given the show's episode list with descriptions, a listing of the video files from the \
same disc rip, and the subtitle text from the start of the current file. \
The dialogue and events in the subtitles are the main evidence; match them against the episode \
descriptions. Theme songs, opening titles and credits are the same in every episode, so they \
tell you nothing: look past them to the story. \
Many episodes open with a \"previously on\" recap: a minute or two of short clips from earlier \
episodes, jumping between unrelated scenes. Recap dialogue comes from earlier episodes, so it \
tells you which episodes came before this one, not which one this is. This matters most early in \
a show, when the first episodes are replayed in the recaps of the ones that follow. Find where \
the recap ends and the scenes start running continuously, identify the episode from the dialogue \
after that point, and expect it to come after the episodes the recap showed. The episode descriptions are short and often \
leave out names and details, so match on the premise and the kind of story rather than \
requiring names from the dialogue to appear in a description; an episode's title is often \
echoed in its dialogue. \
Files from a disc rip are usually, but not always, in episode order, so neighbouring files are \
often consecutive episodes: use the known matches of nearby files as a prior, but let the \
dialogue win. \
Duplicates happen but are the exception: a rip may hold several copies of the same episode, \
such as a play-all track or an extended cut, so a file can match the same episode as another \
file. A run of neighbouring files all matched to one episode is more likely an earlier mistake \
being repeated than a run of copies, so in that case check the dialogue against the other \
candidates before agreeing with the neighbours. \
Some files are long and contain several or all of the episodes on the disc back to back; the \
subtitles you are shown only cover the start of the file, so answer with the first episode in \
it and mention in your analysis if you suspect the file is combined. \
You are also told the length of the current file in minutes and, where known, each episode's \
runtime. Treat this as a weak hint only: listed runtimes are often broadcast slots including ad \
breaks, so a file several minutes shorter than the listed runtime is normal. A file roughly \
double the length of an episode or more is probably combined, and one far shorter than any \
episode is probably an extra or a recap. Never let the length override what the dialogue says. \
The file names in the rip listing only show the order. They may carry names from an earlier \
run, which can be wrong, so never use them as evidence; the current file's name is withheld. \
The subtitles are OCR output and may contain errors. Only choose from the listed episodes. \
Not every file is an episode: discs also hold extras, featurettes, trailers, recaps, play-all \
tracks and menus, and the subtitles may be empty or too short to tell. \"No match\" is an \
acceptable and often correct answer. If the subtitles do not clearly correspond to one of the \
listed episodes, answer NO MATCH rather than guessing. \
Write your analysis first: say whether the file opens with a recap and where it ends, note the \
distinctive names, places and events in the dialogue after it, \
compare them with the episode descriptions, and say which candidate fits best and why. \
Then give your answer: copy the label of that candidate exactly as it appears at the start of \
its line (for example \"S02E01 - Title\"), or NO MATCH. Finally give your confidence.";

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
    #[serde(default)]
    analysis: String,
    answer: String,
    #[serde(default)]
    confidence: String,
}

/// How the model refers to an episode: the label that starts its line in the candidate list, and
/// the only values its answer is allowed to take.
pub fn episode_label(ep: &EpisodeEntry) -> String {
    let name = ep.name.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("S{:02}E{:02} - {name}", ep.season_number, ep.episode_number)
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
        // The current file's name is withheld: it may hold an earlier run's (possibly wrong) answer
        let name = if i == ctx.index {
            "(current file)".to_string()
        } else {
            ctx.rip.files[i]
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
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
        user.push_str(&episode_label(ep));
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
        user.push_str(&format!(" | {overview}\n"));
    }

    // Caption positioning leaves indentation and blank lines that only cost tokens
    let subtitles = subtitle_lines
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    user.push_str("\nSubtitle text from the current file:\n\"\"\"\n");
    user.push_str(&truncate_chars(subtitles.trim(), MAX_SUBTITLE_CHARS));
    user.push_str("\n\"\"\"\n");

    (SYSTEM_PROMPT.to_string(), user)
}

/// The model fills in object keys in the order they are serialised, and `serde_json` writes them
/// alphabetically, so the key names are what put the analysis before the answer. Without that the
/// model commits to an episode and a confidence before it has said anything, then rationalises.
/// The answer is restricted to the offered labels, so it can't name anything else.
pub fn response_schema(candidates: &[EpisodeEntry]) -> serde_json::Value {
    let mut answers: Vec<String> = candidates.iter().map(episode_label).collect();
    answers.push(NO_MATCH.to_string());

    serde_json::json!({
        "type": "object",
        "properties": {
            "analysis": { "type": "string" },
            "answer": { "type": "string", "enum": answers },
            "confidence": { "type": "string", "enum": ["high", "medium", "low"] }
        },
        "required": ["analysis", "answer", "confidence"]
    })
}

/// Lowercased letters and digits only, so a quote or dash that differs between the model's answer
/// and the episode's real name doesn't matter.
fn normalize_label(label: &str) -> String {
    label
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Parses the model's reply. `Err` carries why it was unusable: malformed, or an answer that isn't
/// one of the episodes offered.
pub fn parse_response(content: &str, candidates: &[EpisodeEntry]) -> Result<LlmVerdict, String> {
    let raw: RawAnswer =
        serde_json::from_str(content).map_err(|e| format!("the reply was not valid: {e}"))?;

    if normalize_label(&raw.answer) == normalize_label(NO_MATCH) {
        return Ok(LlmVerdict::NoMatch {
            reasoning: raw.analysis,
        });
    }

    let answer = normalize_label(&raw.answer);
    let Some(candidate) = candidates
        .iter()
        .find(|ep| normalize_label(&episode_label(ep)) == answer)
    else {
        return Err(format!(
            "\"{}\" is not one of the episodes it was offered",
            raw.answer
        ));
    };

    Ok(LlmVerdict::Match(LlmMatch {
        season: candidate.season_number,
        episode: candidate.episode_number,
        confidence: raw.confidence,
        reasoning: raw.analysis,
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

    /// (season, episode) at a position in `series(_, 24)`.
    fn at(pos: usize) -> (u64, u64) {
        ((pos / 24) as u64 + 1, (pos % 24) as u64 + 1)
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
        assert_eq!(got.len(), 2 * LOCALITY_WINDOW + 1);
        assert_eq!(got[LOCALITY_WINDOW], at(30)); // the expected position, centred
        assert_eq!(got[0], at(30 - LOCALITY_WINDOW));
        assert_eq!(*got.last().unwrap(), at(30 + LOCALITY_WINDOW));
    }

    #[test]
    fn test_select_candidates_clamps_to_ends() {
        let all = series(1, 40);

        // Anchor at position 1, one file back: expected position 2.
        let mut near_start = rip(4);
        near_start.matches[0] = Some(episode(1, 2));
        let ctx = MatchContext {
            rip: &near_start,
            index: 1,
            show_name: "Show",
        };
        let got = numbers(&select_candidates(&all, &ctx));
        assert_eq!(got.first(), Some(&(1, 1)));
        assert_eq!(got.len(), 2 + LOCALITY_WINDOW + 1);

        // Anchor at position 38, ten files back: expected position clamps to the last one, 39.
        let mut near_end = rip(30);
        near_end.matches[0] = Some(episode(1, 39));
        let ctx = MatchContext {
            rip: &near_end,
            index: 10,
            show_name: "Show",
        };
        let got = numbers(&select_candidates(&all, &ctx));
        assert_eq!(got.last(), Some(&(1, 40)));
        assert_eq!(got.first(), Some(&(1, 40 - LOCALITY_WINDOW as u64)));
        assert_eq!(got.len(), LOCALITY_WINDOW + 1);
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
        assert_eq!(got.len(), 2 * LOCALITY_WINDOW + 1);
        assert_eq!(got[LOCALITY_WINDOW], at(30));

        // The first file is expected right at the hint.
        let ctx = MatchContext {
            rip: &rip,
            index: 0,
            show_name: "Show",
        };
        let got = numbers(&select_candidates(&all, &ctx));
        assert_eq!(got[LOCALITY_WINDOW], (2, 5));
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
        assert_eq!(got[LOCALITY_WINDOW], (3, 11));
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
        assert!(system.contains("Duplicates happen but are the exception"));
        assert!(system.contains("several copies of the same episode"));
        // A streak of one episode should prompt a second look, not reassure the model.
        assert!(system.contains("more likely an earlier mistake"));
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
        assert!(user.contains("S01E01 - Episode 1x1 | aired 1993-09-10 | 45 min | "));
        assert!(user.contains("S01E02 - Episode 1x2 | Overview of 1x2"));
        assert!(system.contains("length of the current file in minutes"));
        assert!(
            system.contains("roughly double the length of an episode or more is probably combined")
        );
        // Listed runtimes are often broadcast slots, so a shorter file must not count against one.
        assert!(system.contains("broadcast slots including ad breaks"));
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
        assert!(user.contains("2. (current file) <- CURRENT"));
        // The current file's name may carry an earlier run's answer, so it's withheld.
        assert!(!user.contains("title_01.mkv"));
        assert!(system.contains("never use them as evidence"));
        assert!(!user.contains("3. title_02.mkv ->"));
        assert!(user.contains("S01E02 - Episode 1x2 | aired 1993-09-24 | Overview of 1x2"));
        assert!(user.contains("S01E03 - Episode 1x3 | (no description)"));
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

    fn reply(answer: &str) -> String {
        format!(r#"{{"analysis":"Mentions a liver","answer":"{answer}","confidence":"high"}}"#)
    }

    #[test]
    fn test_parse_response() {
        let candidates = vec![episode(1, 2), episode(1, 3)];

        assert_eq!(
            parse_response(&reply("S01E03 - Episode 1x3"), &candidates),
            Ok(LlmVerdict::Match(LlmMatch {
                season: 1,
                episode: 3,
                confidence: "high".to_string(),
                reasoning: "Mentions a liver".to_string(),
            }))
        );

        assert!(parse_response("not json", &candidates).is_err());
        assert!(parse_response(r#"{"analysis":"x"}"#, &candidates).is_err());
        // Not an episode we offered.
        let err = parse_response(&reply("S05E05 - Episode 5x5"), &candidates).unwrap_err();
        assert!(err.contains("S05E05"), "{err}");
        // An old-style numeric answer is not a label.
        assert!(parse_response(&reply("3"), &candidates).is_err());
    }

    #[test]
    fn test_parse_response_answer_names_one_episode() {
        let mut first = episode(2, 1);
        first.name = "Nowhere to Hyde".to_string();
        let mut second = episode(2, 2);
        second.name = "Mystery Mask Mix-Up".to_string();
        let candidates = vec![first, second];

        // A title with the wrong number can't be expressed: the label is the whole answer.
        assert!(parse_response(&reply("S02E02 - Nowhere to Hyde"), &candidates).is_err());
        assert!(matches!(
            parse_response(&reply("S02E01 - Nowhere to Hyde"), &candidates),
            Ok(LlmVerdict::Match(LlmMatch {
                season: 2,
                episode: 1,
                ..
            }))
        ));
    }

    #[test]
    fn test_parse_response_label_comparison_ignores_punctuation_and_case() {
        let mut ep = episode(2, 2);
        ep.name = "Mystery Mask Mix-Up".to_string();

        // Includes the non-breaking hyphen a model once produced in place of "-".
        for answer in [
            "s02e02 - mystery mask mix-up",
            "S02E02 - Mystery Mask Mix Up",
            "S02E02 - Mystery Mask Mix\u{2011}Up",
        ] {
            assert!(
                matches!(
                    parse_response(&reply(answer), std::slice::from_ref(&ep)),
                    Ok(LlmVerdict::Match(_))
                ),
                "{answer}"
            );
        }
    }

    #[test]
    fn test_parse_response_no_match() {
        let candidates = vec![episode(1, 2)];

        for answer in ["NO MATCH", "no match", "No Match."] {
            assert_eq!(
                parse_response(&reply(answer), &candidates),
                Ok(LlmVerdict::NoMatch {
                    reasoning: "Mentions a liver".to_string()
                }),
                "{answer}"
            );
        }
    }

    #[test]
    fn test_episode_label() {
        let mut ep = episode(2, 1);
        ep.name = "  Nowhere   to\nHyde ".to_string();
        assert_eq!(episode_label(&ep), "S02E01 - Nowhere to Hyde");
    }

    #[test]
    fn test_schema_restricts_answer_to_offered_labels() {
        let schema = response_schema(&[episode(1, 2), episode(1, 3)]);
        assert_eq!(
            schema["properties"]["answer"]["enum"],
            serde_json::json!(["S01E02 - Episode 1x2", "S01E03 - Episode 1x3", "NO MATCH"])
        );
        let required = schema["required"].as_array().unwrap();
        for key in ["analysis", "answer", "confidence"] {
            assert!(required.contains(&serde_json::json!(key)), "{key}");
        }
    }

    /// The model fills keys in serialised order, and serde_json writes them alphabetically, so the
    /// key names must sort with the analysis first. If this fails the model answers before it
    /// reasons, which is how it ended up naming one episode in its analysis and another as its
    /// answer.
    #[test]
    fn test_schema_serialises_analysis_before_answer_before_confidence() {
        let json = serde_json::to_string(&response_schema(&[episode(1, 1)])).unwrap();
        let properties = &json[json.find("\"properties\"").unwrap()..];
        let at = |key: &str| properties.find(&format!("\"{key}\":")).unwrap();
        assert!(at("analysis") < at("answer"));
        assert!(at("answer") < at("confidence"));
    }

    #[test]
    fn test_prompt_allows_no_match() {
        let rip = rip(1);
        let ctx = MatchContext {
            rip: &rip,
            index: 0,
            show_name: "Show",
        };
        let (system, _) = build_prompt(&[], &[episode(1, 1)], &ctx, None);
        assert!(system.contains("\"No match\" is an acceptable"));
        assert!(system.contains("answer NO MATCH rather than guessing"));
        assert!(system.contains("Theme songs, opening titles and credits are the same"));
    }

    /// Recaps replay earlier episodes, which once matched a whole disc to the pilot.
    #[test]
    fn test_prompt_discounts_recaps() {
        let rip = rip(1);
        let ctx = MatchContext {
            rip: &rip,
            index: 0,
            show_name: "Show",
        };
        let (system, _) = build_prompt(&[], &[episode(1, 1)], &ctx, None);
        assert!(system.contains("\"previously on\" recap"));
        assert!(system.contains("not which one this is"));
        assert!(system.contains("whether the file opens with a recap"));
    }

    #[test]
    fn test_build_prompt_normalises_subtitle_whitespace() {
        let rip = rip(1);
        let ctx = MatchContext {
            rip: &rip,
            index: 0,
            show_name: "Show",
        };
        let lines = vec![
            "    WHAT'S THAT?   ".to_string(),
            String::new(),
            "        THE GHOST!".to_string(),
        ];
        let (_, user) = build_prompt(&lines, &[episode(1, 1)], &ctx, None);
        assert!(user.contains("\"\"\"\nWHAT'S THAT?\nTHE GHOST!\n\"\"\""));
    }
}
