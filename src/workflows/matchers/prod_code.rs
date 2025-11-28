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
            let input = DefaultEditor::new()?.readline(">> ")?;
            let input = input.trim().to_string();

            let episode = cache.get_episode(series_id, &input).cloned().or_else(|| {
                parse_sxxexx(&input).ok().and_then(|(season, episode)| {
                    cache
                        .get_episode_by_sxxexx(series_id, season, episode)
                        .cloned()
                })
            });
            return Ok(episode);
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
