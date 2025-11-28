use crate::config::get_cache_path;
use crate::domain::models::EpisodeEntry;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Cache {
    pub series: HashMap<String, String>, // series_id -> series_name
    pub episodes_by_production_code: HashMap<String, HashMap<String, EpisodeEntry>>, // series_id -> production_code -> episode_info
    pub episodes_by_sxxexx: HashMap<String, HashMap<u64, HashMap<u64, EpisodeEntry>>>, // series_id -> season_number -> episode_number -> episode_info
}

impl Cache {
    pub fn load() -> Self {
        let cache_path = get_cache_path();
        if cache_path.exists() {
            if let Ok(content) = fs::read_to_string(&cache_path) {
                if let Ok(cache) = serde_json::from_str(&content) {
                    return cache;
                }
            }
        }
        Cache::default()
    }

    pub fn save(&self) -> Result<()> {
        let cache_path = get_cache_path();

        // Create parent directory if it doesn't exist
        if let Some(parent) = cache_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_string_pretty(self)?;
        fs::write(&cache_path, content)?;
        Ok(())
    }

    pub fn get_series_name(&self, series_id: &str) -> Option<&String> {
        self.series.get(series_id)
    }

    pub fn set_series_name(&mut self, series_id: String, name: String) {
        self.series.insert(series_id, name);
    }

    pub fn get_episode(&self, series_id: &str, production_code: &str) -> Option<&EpisodeEntry> {
        // Lookup is case-insensitive
        let key = production_code.to_lowercase();
        self.episodes_by_production_code
            .get(series_id)
            .and_then(|episodes| episodes.get(&key))
    }

    pub fn get_episode_by_sxxexx(
        &self,
        series_id: &str,
        season_number: u64,
        episode_number: u64,
    ) -> Option<&EpisodeEntry> {
        self.episodes_by_sxxexx.get(series_id).and_then(|seasons| {
            seasons
                .get(&season_number)
                .and_then(|episodes| episodes.get(&episode_number))
        })
    }

    pub fn set_episode(&mut self, series_id: &str, episode: &EpisodeEntry) {
        // Store in lowercase for case-insensitive lookup
        if let Some(key) = episode
            .clone()
            .production_code
            .map(|code| code.to_lowercase())
        {
            self.episodes_by_production_code
                .entry(series_id.to_string())
                .or_default()
                .insert(key.clone(), episode.clone());
        }
        self.episodes_by_sxxexx
            .entry(series_id.to_string())
            .or_default()
            .entry(episode.season_number)
            .or_default()
            .insert(episode.episode_number, episode.clone());
    }

    pub fn has_series_episodes(&self, series_id: &str) -> bool {
        // Check if we have any episodes cached for this series
        self.episodes_by_production_code.contains_key(series_id)
            || self.episodes_by_sxxexx.contains_key(series_id)
    }
}
