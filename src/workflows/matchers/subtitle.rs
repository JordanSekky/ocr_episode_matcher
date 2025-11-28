use anyhow::{anyhow, bail, Result};
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use std::path::Path;

use super::Matcher;
use crate::domain::models::EpisodeEntry;
use crate::infra::cache::Cache;
use crate::media::{ocr, subtitles};

pub struct SubtitleMatcher;

impl Matcher for SubtitleMatcher {
    fn match_episode(
        &self,
        file_path: &Path,
        series_id: &str,
        cache: &mut Cache,
    ) -> Result<Option<EpisodeEntry>> {
        let track = subtitles::find_best_subtitle_track(file_path)?;
        println!("Using subtitle track {} ({:?})", track.index, track.codec);

        let temp_dir = tempfile::TempDir::new()?;
        let subtitle_path = subtitles::extract_subtitles(
            file_path,
            track.index,
            &track.codec,
            temp_dir.path(),
        )?;
        println!("Extracted subtitle to {subtitle_path:?}");

        let ocr_engine = match track.codec {
            subtitles::SubtitleCodec::Pgs => Some(ocr::create_ocr_engine()?),
            _ => None,
        };

        subtitles::process_and_display(&subtitle_path, &track.codec, ocr_engine)?;

        let (season, episode) = get_sxxexx_from_stdin()?;
        match cache.get_episode_by_sxxexx(series_id, season, episode) {
            Some(ep) => Ok(Some(ep.clone())),
            None => {
                eprintln!(
                    "Failed to find episode matching 'S{}E{}' in cache for series {}",
                    season, episode, series_id
                );
                Ok(None)
            }
        }
    }
}

fn get_sxxexx_from_stdin() -> Result<(u64, u64)> {
    println!("Please enter SXXEXX (e.g. S01E01):");
    let mut rl = DefaultEditor::new()?;
    let readline = rl.readline(">> ");
    match readline {
        Ok(line) => {
            let (season, episode) = parse_sxxexx(&line)?;
            return Ok((season, episode));
        }
        Err(ReadlineError::Interrupted) => {
            bail!("Interrupted");
        }
        Err(ReadlineError::Eof) => {
            bail!("EOF");
        }
        Err(err) => Err(err.into()),
    }
}

fn parse_sxxexx(input: &str) -> Result<(u64, u64)> {
    let re = regex::Regex::new(r"(?i)^s(\d{1,2})e(\d{1,2})$").unwrap();
    let caps = re
        .captures(input)
        .ok_or(anyhow!("Invalid SXXEXX format"))?;
    let season: u64 = caps
        .get(1)
        .ok_or(anyhow!("Invalid SXXEXX format"))?
        .as_str()
        .parse()?;
    let episode: u64 = caps
        .get(2)
        .ok_or(anyhow!("Invalid SXXEXX format"))?
        .as_str()
        .parse()?;
    Ok((season, episode))
}

