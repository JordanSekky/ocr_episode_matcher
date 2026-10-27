use anyhow::Result;
use std::path::Path;

use crate::domain::models::EpisodeEntry;
use crate::infra::cache::Cache;

pub struct MatchContext<'a> {
    pub show_name: &'a str,
}

/// The episode a matcher settled on for a file.
#[derive(Debug)]
pub struct EpisodeMatch {
    pub episode: EpisodeEntry,
    /// The user accepted or chose this episode at a prompt, so the rename needs no second
    /// confirmation.
    pub confirmed: bool,
}

impl EpisodeMatch {
    pub fn confirmed(episode: EpisodeEntry) -> Self {
        Self {
            episode,
            confirmed: true,
        }
    }

    pub fn unconfirmed(episode: EpisodeEntry) -> Self {
        Self {
            episode,
            confirmed: false,
        }
    }
}

pub trait Matcher {
    fn match_episode(
        &self,
        file_path: &Path,
        series_id: &str,
        cache: &mut Cache,
        ctx: &MatchContext,
    ) -> Result<Option<EpisodeMatch>>;
}

pub mod jev;
pub mod prod_code;
pub mod subtitle;

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn episode(season: u64, episode: u64) -> EpisodeEntry {
        EpisodeEntry {
            production_code: None,
            season_number: season,
            episode_number: episode,
            name: format!("Episode {season}x{episode}"),
            overview: Some(format!("Overview of {season}x{episode}")),
            aired: None,
            runtime: None,
        }
    }
}
