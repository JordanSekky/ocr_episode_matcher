use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::domain::models::EpisodeEntry;
use crate::infra::cache::Cache;
use crate::infra::metadata::{MetadataProvider, SeriesHit};

const TMDB_API_BASE: &str = "https://api.themoviedb.org/3";

#[derive(Debug, Clone)]
pub struct TmdbClient {
    api_key: String,
    client: reqwest::blocking::Client,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    results: Vec<SearchResult>,
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    id: u64,
    name: String,
    first_air_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SeriesResponse {
    name: String,
    #[serde(default)]
    seasons: Vec<SeasonSummary>,
}

#[derive(Debug, Deserialize)]
struct SeasonSummary {
    season_number: u64,
}

#[derive(Debug, Deserialize)]
struct SeasonResponse {
    #[serde(default)]
    episodes: Vec<Episode>,
}

#[derive(Debug, Deserialize)]
struct Episode {
    season_number: u64,
    episode_number: u64,
    name: String,
    overview: Option<String>,
    air_date: Option<String>,
    runtime: Option<u32>,
    production_code: Option<String>,
}

/// TMDB sends an empty string for what it doesn't know.
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

impl From<Episode> for EpisodeEntry {
    fn from(ep: Episode) -> Self {
        EpisodeEntry {
            production_code: non_empty(ep.production_code),
            season_number: ep.season_number,
            episode_number: ep.episode_number,
            name: ep.name,
            overview: non_empty(ep.overview),
            aired: non_empty(ep.air_date),
            runtime: ep.runtime,
        }
    }
}

impl TmdbClient {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            client: reqwest::blocking::Client::new(),
        }
    }

    /// A v4 read access token is a JWT and goes in a header; a v3 API key goes in the query.
    fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, &str)]) -> Result<Option<T>> {
        let mut request = self
            .client
            .get(format!("{TMDB_API_BASE}{path}"))
            .query(&[("language", "en-US")])
            .query(query);
        request = if self.api_key.contains('.') {
            request.bearer_auth(&self.api_key)
        } else {
            request.query(&[("api_key", self.api_key.as_str())])
        };

        let response = request.send()?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            bail!("TMDB rejected the API key (HTTP 401)");
        }
        if !status.is_success() {
            bail!("TMDB request for {path} failed: HTTP {status}");
        }
        Ok(Some(serde_json::from_str(&response.text()?).with_context(
            || format!("unexpected TMDB response for {path}"),
        )?))
    }

    fn series(&self, id: &str) -> Result<SeriesResponse> {
        self.get(&format!("/tv/{id}"), &[])?
            .with_context(|| format!("TMDB has no series {id}"))
    }
}

impl MetadataProvider for TmdbClient {
    fn search_series(&mut self, query: &str) -> Result<Vec<SeriesHit>> {
        let Some(response) = self.get::<SearchResponse>("/search/tv", &[("query", query)])? else {
            return Ok(Vec::new());
        };
        Ok(response
            .results
            .into_iter()
            .map(|r| SeriesHit {
                id: r.id.to_string(),
                name: r.name,
                year: r
                    .first_air_date
                    .and_then(|d| d.get(..4).map(str::to_string)),
            })
            .collect())
    }

    fn series_name(&mut self, id: &str) -> Result<String> {
        Ok(self.series(id)?.name)
    }

    fn preload_episodes(&mut self, id: &str, cache_key: &str, cache: &mut Cache) -> Result<()> {
        let series = self.series(id)?;
        cache.set_series_name(cache_key.to_string(), series.name);

        let mut total = 0;
        for (idx, season) in series.seasons.iter().enumerate() {
            println!(
                "  Fetching season {} ({}/{})...",
                season.season_number,
                idx + 1,
                series.seasons.len()
            );
            let path = format!("/tv/{id}/season/{}", season.season_number);
            let Some(response) = self.get::<SeasonResponse>(&path, &[])? else {
                continue;
            };
            for episode in response.episodes {
                cache.set_episode(cache_key, &episode.into());
                total += 1;
            }
        }
        if total == 0 {
            bail!("TMDB lists no episodes for series {id}");
        }
        println!("Cached {total} episodes.");

        cache.mark_series_detailed(cache_key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_response() {
        let json = r#"{"page":1,"results":[
            {"id":926,"name":"Scooby-Doo, Where Are You!","first_air_date":"1969-09-13","origin_country":["US"]},
            {"id":1,"name":"Undated","first_air_date":""},
            {"id":2,"name":"Missing date"}
        ],"total_pages":1}"#;
        let resp: SearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.results.len(), 3);
        assert_eq!(resp.results[0].id, 926);
        assert_eq!(
            resp.results[0].first_air_date.as_deref(),
            Some("1969-09-13")
        );
        assert!(resp.results[2].first_air_date.is_none());
    }

    #[test]
    fn test_series_response() {
        let json = r#"{"id":926,"name":"Scooby-Doo, Where Are You!","seasons":[
            {"season_number":0,"episode_count":2},{"season_number":1,"episode_count":17}
        ]}"#;
        let resp: SeriesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.name, "Scooby-Doo, Where Are You!");
        let numbers: Vec<u64> = resp.seasons.iter().map(|s| s.season_number).collect();
        assert_eq!(numbers, vec![0, 1]);
    }

    #[test]
    fn test_season_response_maps_to_entries() {
        let json = r#"{"episodes":[
            {"episode_number":1,"season_number":1,"name":"What a Night for a Knight",
             "overview":"Armor comes alive.","air_date":"1969-09-13","runtime":22,
             "production_code":"","id":1},
            {"episode_number":2,"season_number":1,"name":"A Clue for Scooby-Doo",
             "overview":"","air_date":null,"runtime":null,"production_code":"A-2"}
        ]}"#;
        let resp: SeasonResponse = serde_json::from_str(json).unwrap();
        let entries: Vec<EpisodeEntry> = resp.episodes.into_iter().map(Into::into).collect();

        assert_eq!(entries[0].name, "What a Night for a Knight");
        assert_eq!(entries[0].runtime, Some(22));
        assert_eq!(entries[0].aired.as_deref(), Some("1969-09-13"));
        assert!(entries[0].production_code.is_none());

        assert!(entries[1].overview.is_none());
        assert!(entries[1].aired.is_none());
        assert!(entries[1].runtime.is_none());
        assert_eq!(entries[1].production_code.as_deref(), Some("A-2"));
    }

    #[test]
    fn test_season_without_episodes() {
        let resp: SeasonResponse = serde_json::from_str(r#"{"id":5}"#).unwrap();
        assert!(resp.episodes.is_empty());
    }
}
