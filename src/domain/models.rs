use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EpisodeEntry {
    pub production_code: Option<String>,
    pub season_number: u64,
    pub episode_number: u64,
    pub name: String,
    #[serde(default)]
    pub overview: Option<String>,
    #[serde(default)]
    pub aired: Option<String>,
    /// Runtime in minutes, as listed by the metadata provider.
    #[serde(default)]
    pub runtime: Option<u32>,
}

/// Parses an `S01E05`-style code: case-insensitive, one or two digits for the episode and up to
/// four for the season, since some series number their seasons by year (`S1940E34`).
pub fn parse_sxxexx(input: &str) -> Result<(u64, u64)> {
    let re = regex::Regex::new(r"(?i)^s(\d{1,4})e(\d{1,2})$").unwrap();
    let caps = re.captures(input).ok_or(anyhow!("Invalid SXXEXX format"))?;
    let season: u64 = caps
        .get(1)
        .ok_or(anyhow!("Invalid SXXEXX format"))?
        .as_str()
        .parse()?;
    let episode: u64 = caps
        .get(2)
        .ok_or(anyhow!("Invalid SXXEXX format"))?
        .as_str()
        .parse()?;
    Ok((season, episode))
}

/// The episodes to choose between when matching a file: one season if the user named one,
/// otherwise the whole series. Specials are mostly extras and short webisodes that crowd out the
/// real episodes, so they are left out unless asked for (or named as the season), or unless the
/// series has nothing else.
pub fn select_candidates(
    all: &[EpisodeEntry],
    season: Option<u64>,
    include_specials: bool,
) -> Vec<EpisodeEntry> {
    let has_regular = all.iter().any(|ep| ep.season_number != 0);
    all.iter()
        .filter(|ep| match season {
            Some(season) => ep.season_number == season,
            None => include_specials || !has_regular || ep.season_number != 0,
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(season: u64, episode: u64) -> EpisodeEntry {
        EpisodeEntry {
            production_code: None,
            season_number: season,
            episode_number: episode,
            name: format!("Episode {season}x{episode}"),
            overview: None,
            aired: None,
            runtime: None,
        }
    }

    fn series(seasons: u64, per_season: u64) -> Vec<EpisodeEntry> {
        (1..=seasons)
            .flat_map(|s| (1..=per_season).map(move |e| episode(s, e)))
            .collect()
    }

    fn numbers(eps: &[EpisodeEntry]) -> Vec<(u64, u64)> {
        eps.iter()
            .map(|e| (e.season_number, e.episode_number))
            .collect()
    }

    #[test]
    fn test_select_candidates_without_season_returns_the_series() {
        let all = series(2, 24);
        assert_eq!(select_candidates(&all, None, false).len(), 48);
    }

    #[test]
    fn test_select_candidates_keeps_only_the_named_season() {
        let all = series(3, 24);
        let got = numbers(&select_candidates(&all, Some(2), false));
        assert_eq!(got.len(), 24);
        assert!(got.iter().all(|&(season, _)| season == 2));
        assert!(select_candidates(&all, Some(9), false).is_empty());
    }

    #[test]
    fn test_select_candidates_specials() {
        let mut all = series(1, 3);
        all.push(episode(0, 1));

        assert_eq!(select_candidates(&all, None, false).len(), 3);
        assert_eq!(select_candidates(&all, None, true).len(), 4);
        // Naming season 0 asks for the specials.
        assert_eq!(
            numbers(&select_candidates(&all, Some(0), false)),
            vec![(0, 1)]
        );
        // A series with nothing but specials keeps them.
        let specials = vec![episode(0, 1), episode(0, 2)];
        assert_eq!(select_candidates(&specials, None, false).len(), 2);
    }

    #[test]
    fn test_parse_sxxexx_valid() {
        assert_eq!(parse_sxxexx("S01E01").unwrap(), (1, 1));
        assert_eq!(parse_sxxexx("s01e01").unwrap(), (1, 1));
        assert_eq!(parse_sxxexx("S1E1").unwrap(), (1, 1));
        assert_eq!(parse_sxxexx("S10E20").unwrap(), (10, 20));
        assert_eq!(parse_sxxexx("s99e99").unwrap(), (99, 99));
        assert_eq!(parse_sxxexx("S1940E34").unwrap(), (1940, 34));
    }

    #[test]
    fn test_parse_sxxexx_invalid() {
        assert!(parse_sxxexx("0101").is_err());
        assert!(parse_sxxexx("S01").is_err());
        assert!(parse_sxxexx("E01").is_err());
        assert!(parse_sxxexx("S01E").is_err());
        assert!(parse_sxxexx("Episode 1").is_err());
        assert!(parse_sxxexx("S12345E01").is_err()); // Currently regex limits to 4 digits
        assert!(parse_sxxexx("S01E123").is_err()); // and episodes to 2
    }
}
