use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::domain::models::EpisodeEntry;
use crate::infra::cache::Cache;

/// The files being processed together, in sorted order, along with the episode each has been
/// matched to so far. Discs tend to keep episodes in order, so neighbours are strong hints.
#[derive(Debug, Default)]
pub struct RipContext {
    pub files: Vec<PathBuf>,
    pub matches: Vec<Option<EpisodeEntry>>,
    /// Where the user says the first file sits in the series, as (season, episode). Stands in
    /// for a confirmed match until a file in the rip has one.
    pub hint: Option<(u64, u64)>,
}

impl RipContext {
    pub fn new(files: Vec<PathBuf>) -> Self {
        let matches = vec![None; files.len()];
        Self {
            files,
            matches,
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: Option<(u64, u64)>) -> Self {
        self.hint = hint;
        self
    }

    /// The nearest earlier file that has a confirmed match, as (file index, episode).
    pub fn anchor(&self, index: usize) -> Option<(usize, &EpisodeEntry)> {
        (0..index.min(self.matches.len()))
            .rev()
            .find_map(|i| self.matches[i].as_ref().map(|ep| (i, ep)))
    }
}

pub struct MatchContext<'a> {
    pub rip: &'a RipContext,
    pub index: usize,
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

pub mod llm;
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

    #[test]
    fn test_anchor_is_nearest_earlier_match() {
        let mut rip = RipContext::new((0..5).map(|i| PathBuf::from(format!("t{i}.mkv"))).collect());
        assert!(rip.anchor(3).is_none());

        rip.matches[0] = Some(episode(1, 1));
        rip.matches[1] = Some(episode(1, 2));
        let (idx, ep) = rip.anchor(4).unwrap();
        assert_eq!((idx, ep.episode_number), (1, 2));

        // Only earlier files count, never the file itself or later ones.
        assert_eq!(rip.anchor(1).unwrap().0, 0);
        assert!(rip.anchor(0).is_none());
    }
}
