use anyhow::{bail, Result};
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use std::path::Path;

use super::{jev, EpisodeMatch, MatchContext, Matcher};
use crate::domain::models::{parse_sxxexx, EpisodeEntry};
use crate::infra::cache::Cache;
use crate::infra::jev::JevClient;
use crate::media::{ocr, subtitles};
use crate::workflows::renamer;

pub struct SubtitleMatcher {
    pub start_seconds: u64,
    pub duration_seconds: u64,
    pub prefer_codec: subtitles::SubtitleCodec,
    pub skip_confirm: bool,
    /// Don't ask about a suggestion that would leave the file's name as it is.
    pub accept_unchanged: bool,
    /// Only offer Jev episodes from this season.
    pub season: Option<u64>,
    /// Offer specials (season 0) to Jev as well as regular episodes.
    pub include_specials: bool,
    pub jev: Option<JevClient>,
}

impl Matcher for SubtitleMatcher {
    fn match_episode(
        &self,
        file_path: &Path,
        series_id: &str,
        cache: &mut Cache,
        ctx: &MatchContext,
    ) -> Result<Option<EpisodeMatch>> {
        let track = subtitles::find_best_subtitle_track(file_path, self.prefer_codec)?;
        println!("Using subtitle track {} ({:?})", track.index, track.codec);

        let temp_dir = tempfile::TempDir::new()?;
        let subtitle_path = subtitles::extract_subtitles(
            file_path,
            track.index,
            &track.codec,
            temp_dir.path(),
            self.start_seconds,
            self.duration_seconds,
        )?;
        println!("Extracted subtitle to {subtitle_path:?}");

        let ocr_engine = if track.codec.needs_ocr() {
            Some(ocr::create_ocr_engine()?)
        } else {
            None
        };

        let lines = subtitles::extract_text(&subtitle_path, &track.codec, ocr_engine)?;

        if let Some(client) = &self.jev {
            match self.suggest_episode(client, &lines, series_id, cache, ctx) {
                Ok(Suggestion::Episode(episode, suggestion)) => {
                    println!(
                        "Suggested: S{:02}E{:02} - {} ({})",
                        episode.season_number,
                        episode.episode_number,
                        episode.name,
                        percent(suggestion.probability)
                    );
                    print_runners_up("Next most likely", &suggestion.ranked, Some(&episode));
                    // The episode's own summary, to check Jev's pick against
                    let summary = episode
                        .overview
                        .as_deref()
                        .map(str::trim)
                        .filter(|o| !o.is_empty())
                        .unwrap_or("(no summary)");
                    println!("Episode summary: {summary}");

                    if self.skip_confirm {
                        return Ok(Some(EpisodeMatch::unconfirmed(episode)));
                    }
                    // Accepting would change nothing, so there is nothing to ask
                    if self.accept_unchanged && named_for(file_path, ctx.show_name, &episode) {
                        return Ok(Some(EpisodeMatch::confirmed(episode)));
                    }
                    return Ok(confirm_suggestion(Some(episode), &lines, series_id, cache)?
                        .map(EpisodeMatch::confirmed));
                }
                Ok(Suggestion::NoMatch { ranked }) => {
                    println!("Jev found no matching episode (possibly an extra or other non-episode file).");
                    print_runners_up("Most likely episodes", &ranked, None);

                    // Never block a batch run waiting for input; the caller warns and skips it.
                    if self.skip_confirm {
                        return Ok(None);
                    }
                    return Ok(confirm_suggestion(None, &lines, series_id, cache)?
                        .map(EpisodeMatch::confirmed));
                }
                Ok(Suggestion::Unusable(reason)) => {
                    eprintln!("Jev's answer was rejected: {reason}.");

                    // Never guess or block a batch run; the caller warns and skips the file.
                    if self.skip_confirm {
                        return Ok(None);
                    }
                    eprintln!("Falling back to manual entry.");
                }
                Err(e) => eprintln!("Jev matching failed ({e}), falling back to manual entry."),
            }
        }

        subtitles::display_lines(&lines)?;

        let (season, episode) = get_sxxexx_from_stdin()?;
        let found = lookup_episode(cache, series_id, season, episode);
        if found.is_none() {
            eprintln!(
                "Failed to find episode matching 'S{season}E{episode}' in cache for series {series_id}"
            );
        }
        // A typed-in code gets the rename prompt as a second look at what it resolved to
        Ok(found.map(EpisodeMatch::unconfirmed))
    }
}

/// Whether the file already carries the name a match to `episode` would give it.
fn named_for(file_path: &Path, show_name: &str, episode: &EpisodeEntry) -> bool {
    let name = renamer::generate_filename(
        show_name,
        episode.season_number,
        episode.episode_number,
        &episode.name,
    );
    file_path
        .file_name()
        .is_some_and(|current| current == name.as_str())
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

/// A probability as a whole percentage, e.g. "93% likely".
fn percent(probability: f64) -> String {
    format!("{:.0}% likely", probability * 100.0)
}

/// Prints the next few options Jev rated, skipping the episode already suggested and any it
/// all but ruled out.
fn print_runners_up(heading: &str, ranked: &[(String, f64)], suggested: Option<&EpisodeEntry>) {
    let suggested = suggested.map(jev::episode_label);
    let others: Vec<String> = ranked
        .iter()
        .filter(|(label, p)| Some(label) != suggested.as_ref() && *p >= 0.01)
        .take(3)
        .map(|(label, p)| format!("{label} ({:.0}%)", p * 100.0))
        .collect();
    if !others.is_empty() {
        println!("  {heading}: {}", others.join(", "));
    }
}

enum Suggestion {
    Episode(EpisodeEntry, jev::JevMatch),
    /// Jev decided this file isn't one of the episodes.
    NoMatch {
        ranked: Vec<(String, f64)>,
    },
    /// Jev answered, but not in a way we can use, with the reason why.
    Unusable(String),
}

impl SubtitleMatcher {
    /// Asks Jev which cached episode the subtitles belong to, if any.
    fn suggest_episode(
        &self,
        client: &JevClient,
        lines: &[String],
        series_id: &str,
        cache: &Cache,
        ctx: &MatchContext,
    ) -> Result<Suggestion> {
        let all = cache.episodes_for_series(series_id);
        if all.is_empty() {
            bail!("no cached episodes for series {series_id}");
        }
        let candidates = jev::select_candidates(&all, self.season, self.include_specials);
        if candidates.is_empty() {
            bail!("no cached episodes to offer for this season");
        }
        println!(
            "Asking Jev to match against {} candidate episode(s)...",
            candidates.len()
        );

        let request = jev::build_request(client.model(), lines, &candidates, ctx.show_name);
        let reply = client.ask(&request)?;

        Ok(match jev::parse_response(&reply, &candidates) {
            Ok(jev::JevVerdict::Match(suggestion)) => {
                match lookup_episode(cache, series_id, suggestion.season, suggestion.episode) {
                    Some(episode) => Suggestion::Episode(episode, suggestion),
                    None => Suggestion::Unusable("the episode is not in the cache".to_string()),
                }
            }
            Ok(jev::JevVerdict::NoMatch { ranked }) => Suggestion::NoMatch { ranked },
            Err(reason) => Suggestion::Unusable(reason),
        })
    }
}

/// Enter accepts the suggestion (or skips the file when there is none), `v` views the
/// subtitles, and SxxExx overrides it.
fn confirm_suggestion(
    suggested: Option<EpisodeEntry>,
    lines: &[String],
    series_id: &str,
    cache: &Cache,
) -> Result<Option<EpisodeEntry>> {
    let prompt = if suggested.is_some() {
        "Enter to accept, SXXEXX to override, 'v' to view subtitles >> "
    } else {
        "Enter to skip this file, SXXEXX to assign an episode, 'v' to view subtitles >> "
    };

    let mut rl = DefaultEditor::new()?;
    loop {
        let readline = rl.readline(prompt);
        match readline {
            Ok(line) => {
                let input = line.trim();
                if input.is_empty() {
                    return Ok(suggested);
                }
                if input.eq_ignore_ascii_case("v") {
                    subtitles::display_lines(lines)?;
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
                    Err(_) => {
                        println!("Invalid format. Press Enter, 'v', or SXXEXX (e.g. S01E01).")
                    }
                }
            }
            Err(ReadlineError::Interrupted) => bail!("Interrupted"),
            Err(ReadlineError::Eof) => bail!("EOF"),
            Err(err) => return Err(err.into()),
        }
    }
}

fn get_sxxexx_from_stdin() -> Result<(u64, u64)> {
    println!("Please enter SXXEXX (e.g. S01E01):");
    let mut rl = DefaultEditor::new()?;
    loop {
        let readline = rl.readline(">> ");
        match readline {
            Ok(line) => match parse_sxxexx(line.trim()) {
                Ok((season, episode)) => return Ok((season, episode)),
                Err(_) => println!("Invalid format. Please try again (e.g. S01E01)."),
            },
            Err(ReadlineError::Interrupted) => {
                bail!("Interrupted");
            }
            Err(ReadlineError::Eof) => {
                bail!("EOF");
            }
            Err(err) => return Err(err.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflows::matchers::tests::episode;

    #[test]
    fn test_named_for() {
        let ep = episode(1, 5);
        let named = Path::new("/disc/Show - S01E05 - Episode 1x5.mkv");
        assert!(named_for(named, "Show", &ep));
        assert!(!named_for(Path::new("/disc/title_t02.mkv"), "Show", &ep));
        // A copy suffix from an earlier clash would be renamed away, so it counts as a change.
        let copy = Path::new("/disc/Show - S01E05 - Episode 1x5 [copy 1].mkv");
        assert!(!named_for(copy, "Show", &ep));
        assert!(!named_for(named, "Show", &episode(1, 6)));
    }
}
