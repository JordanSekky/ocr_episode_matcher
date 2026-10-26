use anyhow::{bail, Result};
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use std::path::Path;

use super::{llm, MatchContext, Matcher};
use crate::domain::models::{parse_sxxexx, EpisodeEntry};
use crate::infra::cache::Cache;
use crate::infra::ollama::OllamaClient;
use crate::media::{ffmpeg, ocr, subtitles};

pub struct SubtitleMatcher {
    pub duration_seconds: u64,
    pub prefer_codec: subtitles::SubtitleCodec,
    pub skip_confirm: bool,
    pub ollama: Option<OllamaClient>,
}

impl Matcher for SubtitleMatcher {
    fn match_episode(
        &self,
        file_path: &Path,
        series_id: &str,
        cache: &mut Cache,
        ctx: &MatchContext,
    ) -> Result<Option<EpisodeEntry>> {
        let track = subtitles::find_best_subtitle_track(file_path, self.prefer_codec)?;
        println!("Using subtitle track {} ({:?})", track.index, track.codec);

        let temp_dir = tempfile::TempDir::new()?;
        let subtitle_path = subtitles::extract_subtitles(
            file_path,
            track.index,
            &track.codec,
            temp_dir.path(),
            self.duration_seconds,
        )?;
        println!("Extracted subtitle to {subtitle_path:?}");

        let ocr_engine = if track.codec.needs_ocr() {
            Some(ocr::create_ocr_engine()?)
        } else {
            None
        };

        let lines = subtitles::extract_text(&subtitle_path, &track.codec, ocr_engine)?;

        // The file length is only a prompt hint, so failing to read it isn't fatal
        let file_minutes = match ffmpeg::get_duration_minutes(file_path) {
            Ok(minutes) => Some(minutes),
            Err(e) => {
                eprintln!("Warning: could not read the file length: {e}");
                None
            }
        };

        if let Some(client) = &self.ollama {
            match suggest_episode(client, &lines, series_id, cache, ctx, file_minutes) {
                Ok(Suggestion::Episode(episode, suggestion)) => {
                    println!(
                        "Suggested: S{:02}E{:02} - {} (confidence: {})",
                        episode.season_number,
                        episode.episode_number,
                        episode.name,
                        suggestion.confidence
                    );
                    if !suggestion.reasoning.is_empty() {
                        println!("  {}", suggestion.reasoning);
                    }

                    if self.skip_confirm {
                        return Ok(Some(episode));
                    }
                    return confirm_suggestion(Some(episode), &lines, series_id, cache);
                }
                Ok(Suggestion::NoMatch { reasoning }) => {
                    println!("Ollama found no matching episode (possibly an extra or other non-episode file).");
                    if !reasoning.is_empty() {
                        println!("  {reasoning}");
                    }

                    // Never block a batch run waiting for input; the caller warns and skips it.
                    if self.skip_confirm {
                        return Ok(None);
                    }
                    return confirm_suggestion(None, &lines, series_id, cache);
                }
                Ok(Suggestion::Unusable) => {
                    eprintln!("Ollama did not return a usable match, falling back to manual entry.")
                }
                Err(e) => eprintln!("Ollama matching failed ({e}), falling back to manual entry."),
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
        Ok(found)
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

enum Suggestion {
    Episode(EpisodeEntry, llm::LlmMatch),
    /// The model decided this file isn't one of the episodes.
    NoMatch {
        reasoning: String,
    },
    /// The model answered, but not in a way we can use.
    Unusable,
}

/// Asks the model which cached episode the subtitles belong to, if any.
fn suggest_episode(
    client: &OllamaClient,
    lines: &[String],
    series_id: &str,
    cache: &Cache,
    ctx: &MatchContext,
    file_minutes: Option<u64>,
) -> Result<Suggestion> {
    let all = cache.episodes_for_series(series_id);
    if all.is_empty() {
        bail!("no cached episodes for series {series_id}");
    }

    let candidates = llm::select_candidates(&all, ctx);
    println!(
        "Asking Ollama to match against {} candidate episode(s)...",
        candidates.len()
    );

    let (system, user) = llm::build_prompt(lines, &candidates, ctx, file_minutes);
    let content = client.chat_json(&system, &user, llm::response_schema())?;

    Ok(match llm::parse_response(&content, &candidates) {
        Some(llm::LlmVerdict::Match(suggestion)) => {
            match lookup_episode(cache, series_id, suggestion.season, suggestion.episode) {
                Some(episode) => Suggestion::Episode(episode, suggestion),
                None => Suggestion::Unusable,
            }
        }
        Some(llm::LlmVerdict::NoMatch { reasoning }) => Suggestion::NoMatch { reasoning },
        None => Suggestion::Unusable,
    })
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
