use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EpisodeEntry {
    pub production_code: Option<String>,
    pub season_number: u64,
    pub episode_number: u64,
    pub name: String,
}
