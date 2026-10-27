use crate::config::get_cache_path;
use crate::domain::models::EpisodeEntry;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Cache {
    pub series: HashMap<String, String>, // series_id -> series_name
    pub episodes_by_production_code: HashMap<String, HashMap<String, EpisodeEntry>>, // series_id -> production_code -> episode_info
    pub episodes_by_sxxexx: HashMap<String, HashMap<u64, HashMap<u64, EpisodeEntry>>>, // series_id -> season_number -> episode_number -> episode_info
    /// Series whose episodes were cached with overviews, air dates and runtimes. Caches from
    /// before those were stored lack this, so such series are refreshed once when they're needed.
    #[serde(default)]
    pub detailed_series: HashSet<String>,
}

fn legacy_key(key: String) -> String {
    if key.contains('-') {
        key
    } else {
        format!("tvdb-{key}")
    }
}

impl Cache {
    pub fn load() -> Self {
        let cache_path = get_cache_path();
        if cache_path.exists() {
            match fs::read_to_string(&cache_path) {
                Ok(content) => match serde_json::from_str::<Cache>(&content) {
                    Ok(cache) => return cache.migrate_legacy_keys(),
                    Err(e) => eprintln!(
                        "Warning: Ignoring unreadable cache {}: {e}",
                        cache_path.display()
                    ),
                },
                Err(e) => eprintln!(
                    "Warning: Failed to read cache {}: {e}",
                    cache_path.display()
                ),
            }
        }
        Cache::default()
    }

    /// Series used to be keyed by their bare TVDB id. Providers' ids overlap, so keys now carry
    /// the provider (`tvdb-78260`); a bare key can only have come from TVDB.
    fn migrate_legacy_keys(mut self) -> Self {
        fn rekey<V>(map: HashMap<String, V>) -> HashMap<String, V> {
            map.into_iter().map(|(k, v)| (legacy_key(k), v)).collect()
        }
        self.series = rekey(self.series);
        self.episodes_by_production_code = rekey(self.episodes_by_production_code);
        self.episodes_by_sxxexx = rekey(self.episodes_by_sxxexx);
        self.detailed_series = self.detailed_series.into_iter().map(legacy_key).collect();
        self
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

    /// All cached episodes for a series in aired order, with specials (season 0) last.
    pub fn episodes_for_series(&self, series_id: &str) -> Vec<EpisodeEntry> {
        let mut episodes: Vec<EpisodeEntry> = self
            .episodes_by_sxxexx
            .get(series_id)
            .map(|seasons| {
                seasons
                    .values()
                    .flat_map(|episodes| episodes.values().cloned())
                    .collect()
            })
            .unwrap_or_default();
        episodes.sort_by_key(|ep| (ep.season_number == 0, ep.season_number, ep.episode_number));
        episodes
    }

    pub fn mark_series_detailed(&mut self, series_id: &str) {
        self.detailed_series.insert(series_id.to_string());
    }

    pub fn is_series_detailed(&self, series_id: &str) -> bool {
        self.detailed_series.contains(series_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_operations() {
        let mut cache = Cache::default();
        let series_id = "12345";

        // Test series name
        cache.set_series_name(series_id.to_string(), "Test Show".to_string());
        assert_eq!(
            cache.get_series_name(series_id),
            Some(&"Test Show".to_string())
        );

        // Test episode storage and retrieval
        let episode = EpisodeEntry {
            production_code: Some("prod123".to_string()),
            season_number: 1,
            episode_number: 1,
            name: "Pilot".to_string(),
            overview: None,
            aired: None,
            runtime: None,
        };

        cache.set_episode(series_id, &episode);

        // Check retrieval by production code (exact)
        let retrieved = cache.get_episode(series_id, "prod123");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, "Pilot");

        // Check retrieval by production code (case insensitive)
        let retrieved = cache.get_episode(series_id, "PROD123");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, "Pilot");

        // Check retrieval by SxxExx
        let retrieved = cache.get_episode_by_sxxexx(series_id, 1, 1);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, "Pilot");

        // Check has_series_episodes
        assert!(cache.has_series_episodes(series_id));
        assert!(!cache.has_series_episodes("other_id"));
    }

    fn entry(season: u64, episode: u64, overview: Option<&str>) -> EpisodeEntry {
        EpisodeEntry {
            production_code: None,
            season_number: season,
            episode_number: episode,
            name: format!("S{season}E{episode}"),
            overview: overview.map(str::to_string),
            aired: None,
            runtime: None,
        }
    }

    #[test]
    fn test_episodes_for_series_order() {
        let mut cache = Cache::default();
        for (s, e) in [(1, 2), (0, 1), (2, 1), (1, 1)] {
            cache.set_episode("1", &entry(s, e, None));
        }

        let order: Vec<(u64, u64)> = cache
            .episodes_for_series("1")
            .iter()
            .map(|ep| (ep.season_number, ep.episode_number))
            .collect();
        assert_eq!(order, vec![(1, 1), (1, 2), (2, 1), (0, 1)]);
        assert!(cache.episodes_for_series("missing").is_empty());
    }

    #[test]
    fn test_series_detailed_marker() {
        let mut cache = Cache::default();
        cache.set_episode("1", &entry(1, 1, None));
        assert!(!cache.is_series_detailed("1"));
        cache.mark_series_detailed("1");
        assert!(cache.is_series_detailed("1"));
        assert!(!cache.is_series_detailed("2"));
    }

    #[test]
    fn test_legacy_series_keys_become_tvdb_keys() {
        let json = r#"{
            "series": {"78260": "Scooby", "tmdb-926": "Scooby"},
            "episodes_by_production_code": {"78260": {}},
            "episodes_by_sxxexx": {"78260": {"1": {"1": {
                "production_code": null,
                "season_number": 1,
                "episode_number": 1,
                "name": "Pilot"
            }}}},
            "detailed_series": ["78260"]
        }"#;
        let cache = serde_json::from_str::<Cache>(json)
            .unwrap()
            .migrate_legacy_keys();
        assert_eq!(cache.get_series_name("tvdb-78260").unwrap(), "Scooby");
        assert_eq!(cache.get_series_name("tmdb-926").unwrap(), "Scooby");
        assert!(cache.get_series_name("78260").is_none());
        assert!(cache.has_series_episodes("tvdb-78260"));
        assert_eq!(
            cache
                .get_episode_by_sxxexx("tvdb-78260", 1, 1)
                .unwrap()
                .name,
            "Pilot"
        );
        assert!(cache.is_series_detailed("tvdb-78260"));
    }

    #[test]
    fn test_old_cache_without_overview_deserializes() {
        let json = r#"{
            "series": {},
            "episodes_by_production_code": {},
            "episodes_by_sxxexx": {"1": {"1": {"1": {
                "production_code": null,
                "season_number": 1,
                "episode_number": 1,
                "name": "Pilot"
            }}}}
        }"#;
        let cache: Cache = serde_json::from_str(json).unwrap();
        let ep = cache.get_episode_by_sxxexx("1", 1, 1).unwrap();
        assert_eq!(ep.name, "Pilot");
        assert!(ep.overview.is_none());
        assert!(ep.runtime.is_none());
        assert!(!cache.is_series_detailed("1"));
    }
}
