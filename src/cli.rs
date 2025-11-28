use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Clone, ValueEnum, PartialEq, Eq)]
pub enum MatchMode {
    ProductionCode,
    Subtitles,
}

#[derive(Parser)]
#[command(name = "episode-matcher")]
#[command(about = "Extract production codes from video files and rename them using TVDB data")]
pub struct Cli {
    /// Input files or directories to process
    #[arg(required = true)]
    pub inputs: Vec<PathBuf>,

    /// Show name to search in TVDB
    #[arg(long)]
    pub show: Option<String>,

    /// Direct TVDB show ID
    #[arg(long)]
    pub show_id: Option<String>,

    /// Skip confirmation prompts
    #[arg(long)]
    pub no_confirm: bool,

    /// Recursively scan directories for MKV files
    #[arg(short = 'r', long = "recursive")]
    pub recursive: bool,
    /// File size where the user is prompted for the production code

    #[arg(long = "prompt-size")]
    pub prompt_size: Option<u64>,

    /// Matching mode
    #[arg(long, default_value = "production-code")]
    pub match_mode: MatchMode,
}
