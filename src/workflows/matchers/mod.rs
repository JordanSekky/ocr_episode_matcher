use anyhow::Result;
use std::path::Path;

use crate::domain::models::EpisodeEntry;
use crate::infra::cache::Cache;

pub trait Matcher {
    fn match_episode(
        &self,
        file_path: &Path,
        series_id: &str,
        cache: &mut Cache,
    ) -> Result<Option<EpisodeEntry>>;
}

pub mod prod_code;
pub mod subtitle;

