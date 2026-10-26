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
    /// Runtime in minutes, as listed on TVDB.
    #[serde(default)]
    pub runtime: Option<u32>,
}

/// Parses an `S01E05`-style code: case-insensitive, one or two digits for each part.
pub fn parse_sxxexx(input: &str) -> Result<(u64, u64)> {
    let re = regex::Regex::new(r"(?i)^s(\d{1,2})e(\d{1,2})$").unwrap();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sxxexx_valid() {
        assert_eq!(parse_sxxexx("S01E01").unwrap(), (1, 1));
        assert_eq!(parse_sxxexx("s01e01").unwrap(), (1, 1));
        assert_eq!(parse_sxxexx("S1E1").unwrap(), (1, 1));
        assert_eq!(parse_sxxexx("S10E20").unwrap(), (10, 20));
        assert_eq!(parse_sxxexx("s99e99").unwrap(), (99, 99));
    }

    #[test]
    fn test_parse_sxxexx_invalid() {
        assert!(parse_sxxexx("0101").is_err());
        assert!(parse_sxxexx("S01").is_err());
        assert!(parse_sxxexx("E01").is_err());
        assert!(parse_sxxexx("S01E").is_err());
        assert!(parse_sxxexx("Episode 1").is_err());
        assert!(parse_sxxexx("S123E01").is_err()); // Currently regex limits to 2 digits
    }
}
