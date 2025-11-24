use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Cache {
    pub series: HashMap<String, String>, // series_id -> series_name
    pub episodes: HashMap<String, HashMap<String, EpisodeCache>>, // series_id -> production_code -> episode_info
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EpisodeCache {
    pub season_number: u32,
    pub episode_number: u32,
    pub name: String,
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

    pub fn get_episode(&self, series_id: &str, production_code: &str) -> Option<&EpisodeCache> {
        // Lookup is case-insensitive
        let key = production_code.to_lowercase();
        self.episodes
            .get(series_id)
            .and_then(|episodes| episodes.get(&key))
    }

    pub fn get_episode_by_sxxexx(
        &self,
        series_id: &str,
        identifier: &str,
    ) -> Option<&EpisodeCache> {
        // Lookup is case-insensitive, matches S01E01 or s01e01 etc.
        let re = regex::Regex::new(r"(?i)^s(\d{1,2})e(\d{1,2})$").ok()?;
        let caps = re.captures(identifier)?;
        let season: u32 = caps.get(1)?.as_str().parse().ok()?;
        let episode: u32 = caps.get(2)?.as_str().parse().ok()?;
        self.episodes
            .get(series_id)?
            .values()
            .find(|ep| ep.season_number == season && ep.episode_number == episode)
    }

    pub fn set_episode(
        &mut self,
        series_id: String,
        production_code: String,
        episode: EpisodeCache,
    ) {
        // Store in lowercase for case-insensitive lookup
        let key = production_code.to_lowercase();
        self.episodes
            .entry(series_id)
            .or_insert_with(HashMap::new)
            .insert(key, episode);
    }

    pub fn has_series_episodes(&self, series_id: &str) -> bool {
        // Check if we have any episodes cached for this series
        self.episodes.contains_key(series_id)
    }
}

fn get_cache_path() -> PathBuf {
    std::env::var("HOME")
        .map(|home| {
            PathBuf::from(home)
                .join(".episode-matcher")
                .join("cache.json")
        })
        .unwrap_or_else(|_| PathBuf::from(".episode-matcher").join("cache.json"))
}
