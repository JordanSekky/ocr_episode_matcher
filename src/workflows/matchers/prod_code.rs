use anyhow::Result;
use rustyline::DefaultEditor;
use std::path::Path;

use super::{MatchContext, Matcher};
use crate::domain::models::{parse_sxxexx, EpisodeEntry};
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
        _ctx: &MatchContext,
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
