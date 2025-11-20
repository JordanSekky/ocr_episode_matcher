use serde::Deserialize;
use std::collections::HashMap;

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
    #[serde(rename = "absoluteNumber")]
    pub absolute_number: Option<u32>,
    #[serde(rename = "seasonNumber")]
    pub season_number: u32,
    #[serde(rename = "number")]
    pub episode_number: u32,
    #[serde(rename = "id")]
    pub id: u32,
    pub name: String,
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
    #[serde(rename = "id")]
    pub id: u32,
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

    pub fn login(&mut self) -> Result<(), Box<dyn std::error::Error>> {
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
            return Err(format!("TVDB login failed: HTTP {}", response.status()).into());
        }

        let login_resp: LoginResponse = serde_json::from_str(&response.text()?)?;
        self.token = Some(login_resp.data.token);
        Ok(())
    }

    fn ensure_authenticated(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.token.is_none() {
            self.login()?;
        }
        Ok(())
    }

    pub fn search_series(
        &mut self,
        query: &str,
    ) -> Result<Vec<SearchResult>, Box<dyn std::error::Error>> {
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
            return Err(format!("TVDB search failed: HTTP {}", response.status()).into());
        }

        let search_resp: SearchResponse = serde_json::from_str(&response.text()?)?;
        Ok(search_resp.data)
    }

    pub fn get_series_name(
        &mut self,
        series_id: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
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
            return Err(format!("TVDB series lookup failed: HTTP {}", response.status()).into());
        }

        let series_resp: SeriesResponse = serde_json::from_str(&response.text()?)?;

        return Ok(series_resp.data.name);
    }

    pub fn find_episode_by_production_code(
        &mut self,
        series_id: &str,
        production_code: &str,
    ) -> Result<Option<Episode>, Box<dyn std::error::Error>> {
        self.ensure_authenticated()?;

        // Get all episodes for the series
        let client = reqwest::blocking::Client::new();
        let mut page = 0;
        let mut all_episodes = Vec::new();

        loop {
            // Use the default episodes endpoint
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
                return Err(format!("TVDB episodes lookup failed: HTTP {}", status).into());
            }

            let episodes_resp: EpisodesResponse = serde_json::from_str(&response_text)?;
            let episodes = episodes_resp.data.episodes;

            if episodes.is_empty() {
                break;
            }

            all_episodes.extend(episodes);
            page += 1;
        }

        // Search for episode with matching production code by checking extended episode details
        let production_code_lower = production_code.to_lowercase();
        let client = reqwest::blocking::Client::new();

        for episode in &all_episodes {
            // Fetch extended episode details to get production code
            let extended_url = format!("{}/episodes/{}/extended", TVDB_API_BASE, episode.id);
            let extended_response = client
                .get(&extended_url)
                .header(
                    "Authorization",
                    format!("Bearer {}", self.token.as_ref().unwrap()),
                )
                .send()?;

            if extended_response.status().is_success() {
                let extended_resp: ExtendedEpisodeResponse =
                    serde_json::from_str(&extended_response.text()?)?;
                if let Some(code) = &extended_resp.data.production_code {
                    if code.to_lowercase() == production_code_lower {
                        // Return episode with data from extended response
                        return Ok(Some(Episode {
                            absolute_number: episode.absolute_number,
                            season_number: extended_resp.data.season_number,
                            episode_number: extended_resp.data.episode_number,
                            id: extended_resp.data.id,
                            name: extended_resp.data.name,
                        }));
                    }
                }
            }
        }

        Ok(None)
    }
}
