use serde::Deserialize;
use std::collections::HashMap;

use anyhow::{bail, Result};

const TVDB_API_BASE: &str = "https://api4.thetvdb.com/v4";

#[derive(Debug, Clone)]
pub struct TvdbClient {
    api_key: String,
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoginResponse {
    data: LoginData,
}

#[derive(Debug, Deserialize)]
struct LoginData {
    token: String,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    data: Vec<SearchResult>,
}

#[derive(Debug, Deserialize)]
pub struct SearchResult {
    pub tvdb_id: String,
    #[serde(rename = "translations")]
    pub name: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
struct EpisodesResponse {
    data: EpisodesData,
}

#[derive(Debug, Deserialize)]
struct EpisodesData {
    episodes: Vec<Episode>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Episode {
    pub id: u32,
}

#[derive(Debug, Deserialize)]
struct ExtendedEpisodeResponse {
    data: ExtendedEpisodeData,
}

#[derive(Debug, Deserialize)]
struct ExtendedEpisodeData {
    #[serde(rename = "productionCode")]
    pub production_code: Option<String>,
    #[serde(rename = "seasonNumber")]
    pub season_number: u32,
    #[serde(rename = "number")]
    pub episode_number: u32,
    pub name: String,
}

#[derive(Debug, Deserialize)]
struct SeriesResponse {
    data: SeriesData,
}

#[derive(Debug, Deserialize)]
struct SeriesData {
    pub name: String,
}

impl TvdbClient {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            token: None,
        }
    }

    pub fn login(&mut self) -> Result<()> {
        let client = reqwest::blocking::Client::new();
        let body = serde_json::json!({
            "apikey": self.api_key
        });
        let response = client
            .post(&format!("{}/login", TVDB_API_BASE))
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()?;

        if !response.status().is_success() {
            bail!("TVDB login failed: HTTP {}", response.status());
        }

        let login_resp: LoginResponse = serde_json::from_str(&response.text()?)?;
        self.token = Some(login_resp.data.token);
        Ok(())
    }

    fn ensure_authenticated(&mut self) -> Result<()> {
        if self.token.is_none() {
            self.login()?;
        }
        Ok(())
    }

    pub fn search_series(&mut self, query: &str) -> Result<Vec<SearchResult>> {
        self.ensure_authenticated()?;

        let client = reqwest::blocking::Client::new();
        let response = client
            .get(&format!("{}/search", TVDB_API_BASE))
            .header(
                "Authorization",
                format!("Bearer {}", self.token.as_ref().unwrap()),
            )
            .query(&[("query", query), ("type", "series")])
            .send()?;

        if !response.status().is_success() {
            bail!("TVDB search failed: HTTP {}", response.status());
        }

        let search_resp: SearchResponse = serde_json::from_str(&response.text()?)?;
        Ok(search_resp.data)
    }

    pub fn get_series_name(&mut self, series_id: &str) -> Result<String> {
        self.ensure_authenticated()?;

        let client = reqwest::blocking::Client::new();
        let response = client
            .get(&format!("{}/series/{}", TVDB_API_BASE, series_id))
            .header(
                "Authorization",
                format!("Bearer {}", self.token.as_ref().unwrap()),
            )
            .send()?;

        if !response.status().is_success() {
            bail!("TVDB series lookup failed: HTTP {}", response.status());
        }

        let series_resp: SeriesResponse = serde_json::from_str(&response.text()?)?;

        Ok(series_resp.data.name)
    }

    pub fn preload_episodes(
        &mut self,
        series_id: &str,
        cache: &mut crate::cache::Cache,
    ) -> Result<()> {
        self.ensure_authenticated()?;

        // Get all episodes for the series
        let client = reqwest::blocking::Client::new();
        let mut page = 0;
        let mut all_episodes = Vec::new();

        loop {
            let url = format!("{}/series/{}/episodes/default", TVDB_API_BASE, series_id);
            let response = client
                .get(&url)
                .header(
                    "Authorization",
                    format!("Bearer {}", self.token.as_ref().unwrap()),
                )
                .query(&[("page", page.to_string())])
                .send()?;

            let status = response.status();
            let response_text = response.text()?;

            if !status.is_success() {
                if status == 404 {
                    break;
                }
                bail!("TVDB episodes lookup failed: HTTP {}", status);
            }

            let episodes_resp: EpisodesResponse = serde_json::from_str(&response_text)?;
            let episodes = episodes_resp.data.episodes;

            if episodes.is_empty() {
                break;
            }

            all_episodes.extend(episodes);
            page += 1;
        }

        // Fetch extended details for each episode and cache them
        println!("Caching {} episodes...", all_episodes.len());
        for (idx, episode) in all_episodes.iter().enumerate() {
            if (idx + 1) % 50 == 0 {
                println!("  Cached {}/{} episodes...", idx + 1, all_episodes.len());
            }

            let extended_url = format!("{}/episodes/{}/extended", TVDB_API_BASE, episode.id);
            let extended_response = client
                .get(&extended_url)
                .header(
                    "Authorization",
                    format!("Bearer {}", self.token.as_ref().unwrap()),
                )
                .send()?;

            if extended_response.status().is_success() {
                if let Ok(extended_resp) =
                    serde_json::from_str::<ExtendedEpisodeResponse>(&extended_response.text()?)
                {
                    if let Some(code) = &extended_resp.data.production_code {
                        let ep_cache = crate::cache::EpisodeCache {
                            season_number: extended_resp.data.season_number,
                            episode_number: extended_resp.data.episode_number,
                            name: extended_resp.data.name,
                        };
                        cache.set_episode(series_id.to_string(), code.clone(), ep_cache);
                    }
                }
            }
        }

        Ok(())
    }
}
