use anyhow::bail;
use anyhow::Result;
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct Config {
    tvdb_api_key: Option<String>,
}

pub fn get_tvdb_api_key() -> Result<String> {
    // First, check environment variable
    if let Ok(key) = env::var("TVDB_API_KEY") {
        return Ok(key);
    }

    // Then, check config file
    let config_path = get_config_path()?;
    if config_path.exists() {
        let config_content = fs::read_to_string(&config_path)?;
        let config: Config = toml::from_str(&config_content)?;
        if let Some(key) = config.tvdb_api_key {
            return Ok(key);
        }
    }

    bail!("TVDB API key not found. Set TVDB_API_KEY environment variable or create config file at $HOME/.episode-matcher/config.toml with tvdb_api_key = \"your-key\"")
}

fn get_config_path() -> Result<PathBuf> {
    let home = env::var("HOME")?;
    let mut path = PathBuf::from(home);
    path.push(".episode-matcher");
    path.push("config.toml");
    Ok(path)
}
