use anyhow::bail;
use anyhow::Result;
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct ConfigFile {
    tvdb_api_key: Option<String>,
}

pub fn get_tvdb_api_key() -> Result<String> {
    // First, check environment variable
    if let Ok(key) = env::var("TVDB_API_KEY") {
        return Ok(key);
    }

    // Then, check config file
    let config_path = get_config_path();
    if config_path.exists() {
        let config_content = fs::read_to_string(&config_path)?;
        let config: ConfigFile = toml::from_str(&config_content)?;
        if let Some(key) = config.tvdb_api_key {
            return Ok(key);
        }
    }

    bail!("TVDB API key not found. Set TVDB_API_KEY environment variable or create config file at $HOME/.episode-matcher/config.toml with tvdb_api_key = \"your-key\"")
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
