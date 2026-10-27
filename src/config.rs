use anyhow::bail;
use anyhow::Result;
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::PathBuf;

use crate::infra::metadata::Provider;

#[derive(Debug, Deserialize)]
struct ConfigFile {
    tmdb_api_key: Option<String>,
    tvdb_api_key: Option<String>,
    jev_api_key: Option<String>,
}

fn read_config_file() -> Result<Option<ConfigFile>> {
    let config_path = get_config_path();
    if !config_path.exists() {
        return Ok(None);
    }
    Ok(Some(toml::from_str(&fs::read_to_string(&config_path)?)?))
}

/// The environment wins over the config file; blank values count as unset.
fn key_from(env_key: Option<String>, file_key: Option<String>) -> Option<String> {
    let non_empty = |v: Option<String>| v.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    non_empty(env_key).or_else(|| non_empty(file_key))
}

pub fn get_tmdb_api_key() -> Result<Option<String>> {
    Ok(key_from(
        env::var("TMDB_API_KEY").ok(),
        read_config_file()?.and_then(|c| c.tmdb_api_key),
    ))
}

pub fn get_tvdb_api_key() -> Result<Option<String>> {
    Ok(key_from(
        env::var("TVDB_API_KEY").ok(),
        read_config_file()?.and_then(|c| c.tvdb_api_key),
    ))
}

fn missing_key_message(provider: Provider) -> &'static str {
    match provider {
        Provider::Tmdb => "TMDB API key not found. Set the TMDB_API_KEY environment variable or add tmdb_api_key = \"your-key\" to $HOME/.config/episode-matcher/config.toml",
        Provider::Tvdb => "TVDB API key not found. Set the TVDB_API_KEY environment variable or add tvdb_api_key = \"your-key\" to $HOME/.config/episode-matcher/config.toml",
    }
}

/// Picks the provider to use and its key. TMDB is the default; TVDB is used when asked for, or
/// when it's the only one with a key.
pub fn choose_provider(
    requested: Option<Provider>,
    tmdb_key: Option<String>,
    tvdb_key: Option<String>,
) -> Result<(Provider, String)> {
    match (requested, tmdb_key, tvdb_key) {
        (Some(Provider::Tmdb), Some(key), _) | (None, Some(key), _) => Ok((Provider::Tmdb, key)),
        (Some(Provider::Tvdb), _, Some(key)) => Ok((Provider::Tvdb, key)),
        (None, None, Some(key)) => {
            println!("No TMDB API key set; using TVDB instead.");
            Ok((Provider::Tvdb, key))
        }
        (Some(provider), _, _) => bail!(missing_key_message(provider)),
        (None, None, None) => bail!(missing_key_message(Provider::Tmdb)),
    }
}

pub const DEFAULT_JEV_URL: &str = "https://api.typesafe.ai";
const DEFAULT_JEV_MODEL: &str = "jev-latest";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JevConfig {
    pub url: String,
    pub api_key: String,
    pub model: String,
}

/// Jev is opt-in: a key in `JEV_API_KEY` or the config file's `jev_api_key` enables it.
pub fn get_jev_config() -> Result<Option<JevConfig>> {
    Ok(jev_config_from(
        env::var("JEV_API_KEY").ok(),
        read_config_file()?.and_then(|c| c.jev_api_key),
        env::var("EPISODE_MATCHER_JEV_MODEL").ok(),
        env::var("EPISODE_MATCHER_JEV_URL").ok(),
    ))
}

fn jev_config_from(
    env_key: Option<String>,
    file_key: Option<String>,
    model: Option<String>,
    url: Option<String>,
) -> Option<JevConfig> {
    let non_empty = |v: Option<String>| v.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let api_key = non_empty(env_key).or_else(|| non_empty(file_key))?;
    Some(JevConfig {
        url: non_empty(url)
            .unwrap_or_else(|| DEFAULT_JEV_URL.to_string())
            .trim_end_matches('/')
            .to_string(),
        api_key,
        model: non_empty(model).unwrap_or_else(|| DEFAULT_JEV_MODEL.to_string()),
    })
}

pub fn get_cache_path() -> PathBuf {
    println!(
        "Using cache path: {}",
        get_config_dir_path().join("cache.json").display()
    );
    get_config_dir_path().join("cache.json")
}

fn get_config_dir_path() -> PathBuf {
    xdir::config()
        .map(|path| path.join("episode-matcher"))
        // If the standard path could not be found (e.g.`$HOME` is not set),
        // default to the current directory.
        .unwrap_or_default()
}

fn get_config_path() -> PathBuf {
    get_config_dir_path().join("config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    #[test]
    fn test_key_from() {
        assert_eq!(key_from(s(" env \n"), s("file")), s("env"));
        assert_eq!(key_from(s("  "), s("file")), s("file"));
        assert_eq!(key_from(None, s("")), None);
    }

    #[test]
    fn test_choose_provider() {
        let both = |req| choose_provider(req, s("m"), s("v")).unwrap();
        assert_eq!(both(None), (Provider::Tmdb, "m".to_string()));
        assert_eq!(
            both(Some(Provider::Tmdb)),
            (Provider::Tmdb, "m".to_string())
        );
        assert_eq!(
            both(Some(Provider::Tvdb)),
            (Provider::Tvdb, "v".to_string())
        );

        // With only a TVDB key, no request falls back to it, but asking for TMDB is an error.
        assert_eq!(
            choose_provider(None, None, s("v")).unwrap(),
            (Provider::Tvdb, "v".to_string())
        );
        let err = choose_provider(Some(Provider::Tmdb), None, s("v")).unwrap_err();
        assert!(err.to_string().contains("TMDB_API_KEY"));

        // And asking for TVDB without its key doesn't quietly use TMDB.
        let err = choose_provider(Some(Provider::Tvdb), s("m"), None).unwrap_err();
        assert!(err.to_string().contains("TVDB_API_KEY"));

        let err = choose_provider(None, None, None).unwrap_err();
        assert!(err.to_string().contains("TMDB_API_KEY"));
    }

    #[test]
    fn test_jev_disabled_without_key() {
        assert_eq!(jev_config_from(None, None, s("m"), None), None);
        assert_eq!(jev_config_from(s("  "), s(""), None, None), None);
    }

    #[test]
    fn test_jev_defaults_and_overrides() {
        let cfg = jev_config_from(s(" key \n"), None, None, None).unwrap();
        assert_eq!(cfg.api_key, "key");
        assert_eq!(cfg.url, "https://api.typesafe.ai");
        assert_eq!(cfg.model, "jev-latest");

        let cfg = jev_config_from(None, s("file-key"), s("jev-1.14.0"), s("http://gw/")).unwrap();
        assert_eq!(cfg.api_key, "file-key");
        assert_eq!(cfg.model, "jev-1.14.0");
        assert_eq!(cfg.url, "http://gw");

        // The environment wins over the config file.
        assert_eq!(
            jev_config_from(s("env"), s("file"), None, None)
                .unwrap()
                .api_key,
            "env"
        );
    }
}
