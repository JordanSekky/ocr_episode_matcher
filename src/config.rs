use anyhow::bail;
use anyhow::Result;
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct ConfigFile {
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

pub fn get_tvdb_api_key() -> Result<String> {
    // First, check environment variable
    if let Ok(key) = env::var("TVDB_API_KEY") {
        return Ok(key);
    }

    // Then, check config file
    if let Some(key) = read_config_file()?.and_then(|c| c.tvdb_api_key) {
        return Ok(key);
    }

    bail!("TVDB API key not found. Set TVDB_API_KEY environment variable or create config file at $HOME/.episode-matcher/config.toml with tvdb_api_key = \"your-key\"")
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
