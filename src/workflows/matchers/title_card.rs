use anyhow::{bail, Result};
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use std::path::Path;

use super::{jev, EpisodeMatch, MatchContext, Matcher};
use crate::domain::models::{parse_sxxexx, select_candidates, EpisodeEntry};
use crate::domain::title_match::{rank_episodes, TitleScore};
use crate::infra::cache::Cache;
use crate::media::ocr;

/// A title this close to what the card says is taken without asking under `--no-confirm`, as
/// long as nothing else is within `CONFIDENT_MARGIN` of it. Measured on 24 title cards, every
/// title that clears both was the right one.
const CONFIDENT_SCORE: f64 = 0.8;
const CONFIDENT_MARGIN: f64 = 0.15;
/// Below this the best title is a guess no better than noise, so it isn't offered as the default.
const WEAK_SCORE: f64 = 0.65;
/// How many of the closest titles are shown to pick from.
const SHOWN: usize = 3;

/// Matches a short by reading its title card and finding the episode with the closest title.
pub struct TitleCardMatcher {
    pub start_seconds: u64,
    pub duration_seconds: u64,
    pub skip_confirm: bool,
    /// Only look at episodes from this season.
    pub season: Option<u64>,
    /// Look at specials (season 0) as well as regular episodes.
    pub include_specials: bool,
}

impl Matcher for TitleCardMatcher {
    fn match_episode(
        &self,
        file_path: &Path,
        series_id: &str,
        cache: &mut Cache,
        _ctx: &MatchContext,
    ) -> Result<Option<EpisodeMatch>> {
        let all = cache.episodes_for_series(series_id);
        let candidates = select_candidates(&all, self.season, self.include_specials);
        if candidates.is_empty() {
            bail!("no cached episodes to match titles against for series {series_id}");
        }

        let texts =
            ocr::extract_title_card_text(file_path, self.start_seconds, self.duration_seconds)?;
        let ranked = rank_episodes(&texts, &candidates);
        print_ranking(&ranked);

        let confident = is_confident(&ranked);
        if self.skip_confirm {
            // Never guess or block a batch run; the caller warns and skips the file.
            return Ok(confident.then(|| EpisodeMatch::unconfirmed(ranked[0].episode.clone())));
        }

        let suggestion = (ranked[0].score >= WEAK_SCORE).then(|| ranked[0].episode.clone());
        Ok(
            confirm_choice(suggestion, &ranked, &texts, series_id, cache)?
                .map(EpisodeMatch::confirmed),
        )
    }
}

/// Whether the closest title is close enough, and far enough ahead of the next, to go without
/// asking.
fn is_confident(ranked: &[TitleScore]) -> bool {
    let Some(best) = ranked.first() else {
        return false;
    };
    let runner_up = ranked.get(1).map_or(0.0, |r| r.score);
    best.score >= CONFIDENT_SCORE && best.score - runner_up >= CONFIDENT_MARGIN
}

fn describe(entry: &TitleScore) -> String {
    format!(
        "{} ({:.0}% match)",
        jev::episode_label(&entry.episode),
        entry.score * 100.0
    )
}

fn print_ranking(ranked: &[TitleScore]) {
    println!("Closest titles to the title card:");
    for (i, entry) in ranked.iter().take(SHOWN).enumerate() {
        println!("  {}: {}", i + 1, describe(entry));
    }
}

fn lookup_episode(
    cache: &Cache,
    series_id: &str,
    season: u64,
    episode: u64,
) -> Option<EpisodeEntry> {
    cache
        .get_episode_by_sxxexx(series_id, season, episode)
        .cloned()
}

/// Enter accepts the suggestion (or skips the file when there is none), a number picks one of
/// the listed titles, SxxExx names an episode, `s` skips the file and `v` shows what was read
/// from the card.
fn confirm_choice(
    suggested: Option<EpisodeEntry>,
    ranked: &[TitleScore],
    texts: &[String],
    series_id: &str,
    cache: &Cache,
) -> Result<Option<EpisodeEntry>> {
    let prompt = if suggested.is_some() {
        "Enter to accept 1, 2-3 to pick another, SXXEXX to assign, 's' to skip, 'v' to view OCR text >> "
    } else {
        "No close match. Enter to skip, 1-3 to pick one, SXXEXX to assign, 'v' to view OCR text >> "
    };

    let mut rl = DefaultEditor::new()?;
    loop {
        match rl.readline(prompt) {
            Ok(line) => {
                let input = line.trim();
                if input.is_empty() {
                    return Ok(suggested);
                }
                if input.eq_ignore_ascii_case("s") {
                    return Ok(None);
                }
                if input.eq_ignore_ascii_case("v") {
                    for text in texts {
                        println!("  {:?}", text.trim());
                    }
                    continue;
                }
                if let Ok(choice) = input.parse::<usize>() {
                    match ranked.iter().take(SHOWN).nth(choice.wrapping_sub(1)) {
                        Some(entry) => return Ok(Some(entry.episode.clone())),
                        None => println!(
                            "Please pick a number from 1 to {}.",
                            SHOWN.min(ranked.len())
                        ),
                    }
                    continue;
                }
                match parse_sxxexx(input) {
                    Ok((season, episode)) => {
                        match lookup_episode(cache, series_id, season, episode) {
                            Some(found) => return Ok(Some(found)),
                            None => println!(
                                "S{season:02}E{episode:02} is not in the cache. Please try again."
                            ),
                        }
                    }
                    Err(_) => println!(
                        "Invalid format. Press Enter, a number, 's', 'v', or SXXEXX (e.g. S01E01)."
                    ),
                }
            }
            Err(ReadlineError::Interrupted) => bail!("Interrupted"),
            Err(ReadlineError::Eof) => bail!("EOF"),
            Err(err) => return Err(err.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflows::matchers::tests::episode;

    fn scored(score: f64) -> TitleScore {
        TitleScore {
            episode: episode(1, 1),
            score,
        }
    }

    #[test]
    fn confident_needs_a_close_title_with_clear_daylight_to_the_next() {
        assert!(is_confident(&[scored(1.0), scored(0.6)]));
        assert!(is_confident(&[scored(0.8), scored(0.65)]));
        // close, but another title is nearly as close
        assert!(!is_confident(&[scored(0.9), scored(0.8)]));
        // clear of the others, but not close enough to the card
        assert!(!is_confident(&[scored(0.75), scored(0.3)]));
        assert!(!is_confident(&[]));
    }

    #[test]
    fn a_lone_candidate_only_needs_to_be_close() {
        assert!(is_confident(&[scored(0.85)]));
        assert!(!is_confident(&[scored(0.7)]));
    }
}
