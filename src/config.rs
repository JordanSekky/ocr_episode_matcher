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

const DEFAULT_OLLAMA_NUM_CTX: u32 = 32768;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OllamaConfig {
    pub url: String,
    pub model: String,
    pub num_ctx: u32,
}

/// Ollama is opt-in: `EPISODE_MATCHER_OLLAMA_URL` enables it, and the model is then required.
pub fn get_ollama_config() -> Result<Option<OllamaConfig>> {
    ollama_config_from(
        env::var("EPISODE_MATCHER_OLLAMA_URL").ok(),
        env::var("EPISODE_MATCHER_OLLAMA_MODEL").ok(),
        env::var("EPISODE_MATCHER_OLLAMA_NUM_CTX").ok(),
    )
}

fn ollama_config_from(
    url: Option<String>,
    model: Option<String>,
    num_ctx: Option<String>,
) -> Result<Option<OllamaConfig>> {
    let Some(url) = url.filter(|u| !u.trim().is_empty()) else {
        return Ok(None);
    };
    let Some(model) = model.filter(|m| !m.trim().is_empty()) else {
        bail!("EPISODE_MATCHER_OLLAMA_URL is set but EPISODE_MATCHER_OLLAMA_MODEL is not");
    };
    let num_ctx = match num_ctx {
        Some(value) => value.trim().parse().map_err(|_| {
            anyhow::anyhow!(
                "EPISODE_MATCHER_OLLAMA_NUM_CTX must be a positive integer, got '{value}'"
            )
        })?,
        None => DEFAULT_OLLAMA_NUM_CTX,
    };

    Ok(Some(OllamaConfig {
        url: url.trim().trim_end_matches('/').to_string(),
        model: model.trim().to_string(),
        num_ctx,
    }))
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
    fn test_ollama_disabled_without_url() {
        assert_eq!(ollama_config_from(None, s("m"), None).unwrap(), None);
        assert_eq!(ollama_config_from(s("  "), s("m"), None).unwrap(), None);
    }

    #[test]
    fn test_ollama_requires_model() {
        assert!(ollama_config_from(s("http://localhost:11434"), None, None).is_err());
        assert!(ollama_config_from(s("http://localhost:11434"), s(""), None).is_err());
    }

    #[test]
    fn test_ollama_defaults_and_overrides() {
        let cfg = ollama_config_from(s("http://localhost:11434/"), s("llama3"), None)
            .unwrap()
            .unwrap();
        assert_eq!(cfg.url, "http://localhost:11434");
        assert_eq!(cfg.model, "llama3");
        assert_eq!(cfg.num_ctx, 32768);

        let cfg = ollama_config_from(s("http://h"), s("m"), s("65536"))
            .unwrap()
            .unwrap();
        assert_eq!(cfg.num_ctx, 65536);

        assert!(ollama_config_from(s("http://h"), s("m"), s("lots")).is_err());
    }
}
