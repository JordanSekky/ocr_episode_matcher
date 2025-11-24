use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Cache {
    pub series: HashMap<String, String>, // series_id -> series_name
    pub episodes: HashMap<String, EpisodeCache>, // production_code -> episode_info
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EpisodeCache {
    pub series_id: String,
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

    pub fn get_episode(&self, production_code: &str) -> Option<&EpisodeCache> {
        // Lookup is case-insensitive
        let key = production_code.to_lowercase();
        self.episodes.get(&key)
    }

    pub fn set_episode(&mut self, production_code: String, episode: EpisodeCache) {
        // Store in lowercase for case-insensitive lookup
        let key = production_code.to_lowercase();
        self.episodes.insert(key, episode);
    }

    pub fn has_series_episodes(&self, series_id: &str) -> bool {
        // Check if we have any episodes cached for this series
        self.episodes.values().any(|ep| ep.series_id == series_id)
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
