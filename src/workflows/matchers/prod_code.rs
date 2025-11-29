use anyhow::{anyhow, Result};
use rustyline::DefaultEditor;
use std::path::Path;

use super::Matcher;
use crate::domain::models::EpisodeEntry;
use crate::infra::cache::Cache;
use crate::media::ocr;

pub struct ProductionCodeMatcher {
    pub prompt_size: Option<u64>,
}

impl Matcher for ProductionCodeMatcher {
    fn match_episode(
        &self,
        file_path: &Path,
        series_id: &str,
        cache: &mut Cache,
    ) -> Result<Option<EpisodeEntry>> {
        // Extract production code
        let production_code_candidates =
            ocr::extract_production_code_candidates(file_path.to_str().unwrap())?;

        if let Some(episode) = production_code_candidates
            .into_iter()
            .find_map(|code| cache.get_episode(series_id, &code).cloned())
        {
            return Ok(Some(episode));
        }

        if self.prompt_size.is_some() && file_path.metadata()?.len() > self.prompt_size.unwrap() {
            println!("Please enter the production code or SXXEXX manually.");
            let mut rl = DefaultEditor::new()?;
            loop {
                let input = rl.readline(">> ")?;
                let input = input.trim().to_string();

                let episode = cache.get_episode(series_id, &input).cloned().or_else(|| {
                    parse_sxxexx(&input).ok().and_then(|(season, episode)| {
                        cache
                            .get_episode_by_sxxexx(series_id, season, episode)
                            .cloned()
                    })
                });

                if let Some(episode) = episode {
                    return Ok(Some(episode));
                }
                println!("Episode not found or invalid format. Please try again.");
            }
        }

        Ok(None)
    }
}

fn parse_sxxexx(input: &str) -> Result<(u64, u64)> {
    let re = regex::Regex::new(r"(?i)^s(\d{1,2})e(\d{1,2})$").unwrap();
    let caps = re.captures(input).ok_or(anyhow!("Invalid SXXEXX format"))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sxxexx_valid() {
        assert_eq!(parse_sxxexx("S01E01").unwrap(), (1, 1));
        assert_eq!(parse_sxxexx("s01e01").unwrap(), (1, 1));
        assert_eq!(parse_sxxexx("S1E1").unwrap(), (1, 1));
        assert_eq!(parse_sxxexx("S10E20").unwrap(), (10, 20));
        assert_eq!(parse_sxxexx("s99e99").unwrap(), (99, 99));
    }

    #[test]
    fn test_parse_sxxexx_invalid() {
        assert!(parse_sxxexx("0101").is_err());
        assert!(parse_sxxexx("S01").is_err());
        assert!(parse_sxxexx("E01").is_err());
        assert!(parse_sxxexx("S01E").is_err());
        assert!(parse_sxxexx("Episode 1").is_err());
        assert!(parse_sxxexx("S123E01").is_err()); // Currently regex limits to 2 digits
    }
}
