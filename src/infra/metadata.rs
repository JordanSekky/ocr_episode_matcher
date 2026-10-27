use anyhow::Result;
use clap::ValueEnum;

use crate::infra::cache::Cache;

/// Where a series and its episodes come from.
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum Provider {
    Tmdb,
    Tvdb,
}

impl Provider {
    pub fn prefix(self) -> &'static str {
        match self {
            Provider::Tmdb => "tmdb",
            Provider::Tvdb => "tvdb",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Provider::Tmdb => "TMDB",
            Provider::Tvdb => "TVDB",
        }
    }
}

/// A series found by a search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesHit {
    pub id: String,
    pub name: String,
    pub year: Option<String>,
}

pub trait MetadataProvider {
    fn search_series(&mut self, query: &str) -> Result<Vec<SeriesHit>>;

    fn series_name(&mut self, id: &str) -> Result<String>;

    /// Fetches every episode of the series into `cache` under `cache_key`, and records the
    /// series name and that the episodes carry overviews and runtimes.
    fn preload_episodes(&mut self, id: &str, cache_key: &str, cache: &mut Cache) -> Result<()>;
}

/// The key a series is stored under in the cache: provider ids overlap, so the provider is part
/// of it.
pub fn cache_key(provider: Provider, id: &str) -> String {
    format!("{}-{id}", provider.prefix())
}

/// Splits a `--show-id` value into its provider and id: `tmdb-926` (the form Plex and Jellyfin
/// use in folder names, with or without braces) names the provider, a bare `926` doesn't.
pub fn parse_show_id(input: &str) -> (Option<Provider>, String) {
    let input = input.trim().trim_matches(|c| c == '{' || c == '}');
    for provider in [Provider::Tmdb, Provider::Tvdb] {
        if let Some(id) = input
            .to_lowercase()
            .strip_prefix(&format!("{}-", provider.prefix()))
        {
            return (Some(provider), id.trim().to_string());
        }
    }
    (None, input.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_show_id() {
        assert_eq!(parse_show_id("926"), (None, "926".to_string()));
        assert_eq!(
            parse_show_id("tmdb-926"),
            (Some(Provider::Tmdb), "926".to_string())
        );
        assert_eq!(
            parse_show_id("{TVDB-72860}"),
            (Some(Provider::Tvdb), "72860".to_string())
        );
    }

    #[test]
    fn test_cache_key() {
        assert_eq!(cache_key(Provider::Tmdb, "926"), "tmdb-926");
        assert_eq!(cache_key(Provider::Tvdb, "78260"), "tvdb-78260");
    }
}
